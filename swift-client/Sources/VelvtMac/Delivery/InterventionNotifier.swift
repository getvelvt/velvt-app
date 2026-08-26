import Combine
import Foundation

/// Delivers a live drift offer as an OS notification.
///
/// Without this, an offer exists only as a card inside the menu bar popover —
/// a surface the person is by definition not looking at, because the evidence
/// that produced the offer is that they are somewhere else. The offer would
/// then resolve as `returned` or `expired` and be recorded as delivered,
/// making the pre-registered primary outcome a measurement of a nudge nobody
/// received.
///
/// Rust owns the decision and the copy. This type decides nothing about
/// whether an offer is warranted, never rewrites the text, and adds no claim
/// of its own — it moves an approved offer to a surface the person can see.
@MainActor
public final class InterventionNotifier {
    /// Identity of an offer, used to notify exactly once for it.
    ///
    /// The snapshot carrying `active_intervention` is republished on every
    /// state change for as long as the offer is unanswered, so the offer is
    /// seen many times. `offeredAt` is assigned by Rust when the offer is
    /// recorded and never changes, which makes (block, offeredAt) stable for
    /// one offer and distinct across a re-offer in the same block.
    private struct OfferKey: Equatable {
        let blockID: UUID
        let offeredAt: Date
    }

    /// What has already been settled about the offer currently on screen.
    ///
    /// The distinction that matters is *why* an offer did not ring. A refusal
    /// on purpose is permanent; a refusal by circumstance is not. Collapsing
    /// the two — marking the offer notified before the permission check, as
    /// this type first did — meant an offer dropped because notifications
    /// happened to be unauthorised at that instant could never ring again,
    /// even if authorisation arrived a second later while the card was still
    /// on screen.
    private enum Disposition: Equatable {
        /// Refused on purpose: reduced salience. Sticky, so a later snapshot
        /// arriving with raised salience cannot ring the same offer.
        case suppressed
        /// The request reached the notification centre. Sticky, so the
        /// republished snapshot cannot ring the same offer twice.
        case delivered
        /// Attempted and did not happen for a reason that is not a product
        /// decision: notifications unauthorised, or the centre rejected the
        /// request. The offer is still live, so the next snapshot for it —
        /// or notifications being turned on — tries again.
        case undelivered
    }

    private let scheduler: any InterventionNotificationScheduling
    private let permissionManager: any PermissionManagerProtocol
    private let reporter: any NotificationDeliveryReporting
    private var cancellables = Set<AnyCancellable>()

    private var currentKey: OfferKey?
    private var currentOffer: ActiveIntervention?
    private var disposition: Disposition?
    /// The system prompt is asked for at most once per offer, so a retry loop
    /// cannot turn one offer into repeated authorization requests.
    private var hasRequestedPermission = false
    private var isAttempting = false

    /// The most recent delivery task. Exposed so tests can await the
    /// permission-check/schedule work rather than racing it.
    public private(set) var inFlightTask: Task<Void, Never>?

    public init(
        scheduler: any InterventionNotificationScheduling,
        permissionManager: any PermissionManagerProtocol,
        reporter: any NotificationDeliveryReporting = OSLogNotificationDeliveryReporter()
    ) {
        self.scheduler = scheduler
        self.permissionManager = permissionManager
        self.reporter = reporter
    }

    /// - Parameter snapshots: the coordinator's published work-block state.
    public func start(snapshots: some Publisher<WorkBlockSnapshot?, Never>) {
        snapshots
            .receive(on: RunLoop.main)
            .sink { [weak self] snapshot in
                self?.handle(snapshot)
            }
            .store(in: &cancellables)

        // An offer stays live for minutes, and turning notifications on is the
        // most likely thing a person does *because* they saw the card and
        // nothing rang. Without this, that offer is the one offer that can
        // never ring.
        permissionManager.statusPublisher
            .map { $0[.notifications] ?? .unknown }
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard status == .granted else { return }
                self?.retryPendingOffer()
            }
            .store(in: &cancellables)
    }

    @discardableResult
    public func handle(_ snapshot: WorkBlockSnapshot?) -> Task<Void, Never>? {
        guard let blockID = snapshot?.blockID,
            let intervention = snapshot?.activeIntervention
        else {
            // The offer is gone: answered, expired, or the block ended. Clear
            // the marker so a genuinely new offer in a later block delivers.
            forgetCurrentOffer()
            return nil
        }

        let key = OfferKey(blockID: blockID, offeredAt: intervention.offeredAt)
        if key != currentKey {
            currentKey = key
            disposition = nil
            hasRequestedPermission = false
        }
        currentOffer = intervention

        // A quiet offer is the backoff state, or Velvt's own quiet hours: the
        // offer renders in-app and does not ring. Recorded as suppressed and
        // never revisited, so raising salience later cannot re-ring it.
        guard intervention.salience == .normal else {
            if disposition != .suppressed {
                disposition = .suppressed
                reporter.report(.suppressedBySalience, surface: .driftOffer)
            }
            return nil
        }
        guard disposition == nil || disposition == .undelivered else { return nil }
        return attemptDelivery(key: key, offer: intervention)
    }

    /// Tries the live offer again after a circumstance that blocked it may
    /// have changed. A no-op unless an offer is on screen and its last attempt
    /// failed for a reason that was never a product decision.
    @discardableResult
    public func retryPendingOffer() -> Task<Void, Never>? {
        guard disposition == .undelivered,
            let key = currentKey,
            let offer = currentOffer
        else { return nil }
        return attemptDelivery(key: key, offer: offer)
    }

    private func forgetCurrentOffer() {
        currentKey = nil
        currentOffer = nil
        disposition = nil
        hasRequestedPermission = false
        // An attempt outlives the offer it was for. `requestPermission` waits
        // on a system alert the user may never answer, and until it returns
        // `finishAttempt` never runs, so `isAttempting` stays true and the
        // one-at-a-time guard turns away every later offer for the life of
        // the process. That is a fresh install — where the alert is shown for
        // the first time — losing drift delivery permanently, silently, on
        // the first offer it ever makes. Cancelling here is not cancelling a
        // decision: the permission alert stays on screen and its answer is
        // still recorded by the system.
        inFlightTask?.cancel()
        inFlightTask = nil
        isAttempting = false
    }

    private func attemptDelivery(key: OfferKey, offer: ActiveIntervention) -> Task<Void, Never>? {
        // One attempt at a time: the snapshot republishes far faster than a
        // permission round trip returns, and overlapping attempts would post
        // the same offer more than once.
        guard !isAttempting else { return nil }
        isAttempting = true

        let task = Task { @MainActor [weak self, scheduler, permissionManager, reporter] in
            // `notDetermined` maps to `.unknown`. Onboarding can reach a first
            // work block without ever having asked about notifications, so
            // checking alone would drop the offer in silence. Ask once, at the
            // moment there is something worth showing.
            let checked = await permissionManager.checkStatus(for: .notifications)
            guard !Task.isCancelled else {
                self?.finishAttempt(key: key, disposition: .undelivered)
                return
            }
            var status = checked
            if checked == .unknown, self?.hasRequestedPermission != true {
                self?.hasRequestedPermission = true
                status = await permissionManager.requestPermission(for: .notifications)
            }
            guard status == .granted, !Task.isCancelled else {
                // Not a decision this app made, so the offer is not spent:
                // it stays eligible while it is still on screen.
                self?.finishAttempt(key: key, disposition: .undelivered)
                reporter.report(.blockedByPermission(status), surface: .driftOffer)
                return
            }
            let scheduled = await scheduler.scheduleIntervention(
                id: Self.notificationID(for: key),
                title: offer.title,
                body: offer.body
            )
            self?.finishAttempt(key: key, disposition: scheduled ? .delivered : .undelivered)
            reporter.report(
                scheduled ? .delivered : .rejectedByNotificationCentre, surface: .driftOffer)
        }
        inFlightTask = task
        return task
    }

    private func finishAttempt(key: OfferKey, disposition attempted: Disposition) {
        isAttempting = false
        // The offer may have been answered or replaced while the permission
        // round trip was in flight; that newer state wins.
        guard currentKey == key else {
            // A newer offer arrived mid-attempt and was turned away by the
            // one-at-a-time guard. Give it its turn rather than leaving it to
            // wait for a republish that may not come.
            attemptCurrentOfferIfUntried()
            return
        }
        guard disposition != .suppressed, disposition != .delivered else { return }
        disposition = attempted
    }

    private func attemptCurrentOfferIfUntried() {
        guard disposition == nil,
            let key = currentKey,
            let offer = currentOffer,
            offer.salience == .normal
        else { return }
        _ = attemptDelivery(key: key, offer: offer)
    }

    /// Stable per-offer identifier, so a redelivery of the same offer replaces
    /// its banner instead of stacking a second one.
    private static func notificationID(for key: OfferKey) -> String {
        "velvt.intervention.\(key.blockID.uuidString).\(Int(key.offeredAt.timeIntervalSince1970))"
    }
}
