import Combine
import Foundation
import UserNotifications

// MARK: - InterventionDeliveryRecording

/// Where the notifier reports what became of each drift offer's notification
/// (proto v35). `WorkBlockCoordinator` sends it to the service, which stores it
/// beside the offer's outcome. Until this existed the fact reached only the
/// unified log, and an offer that never rang was stored like one that did.
@MainActor
public protocol InterventionDeliveryRecording: AnyObject {
    func reportInterventionDelivery(blockID: UUID, channel: InterventionDeliveryChannel)
}

// MARK: - NotificationAlertStyleReading

/// The alert style the person chose for Velvt in System Settings.
public enum NotificationAlertStyle: Equatable, Sendable {
    /// "None": a posted notification is listed in Notification Center and
    /// never shown on screen.
    case none
    case banner
    case alert
    case unknown
}

/// Reads the alert style, which the permission check cannot see: a Mac where
/// notifications are allowed with alert style None reports `granted`, and the
/// notification centre accepts every request without showing any of them.
public protocol NotificationAlertStyleReading: AnyObject, Sendable {
    func alertStyle() async -> NotificationAlertStyle
}

public final class SystemNotificationAlertStyleReader: NotificationAlertStyleReading, @unchecked Sendable {
    private let center: UNUserNotificationCenter

    public init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    public func alertStyle() async -> NotificationAlertStyle {
        let settings = await center.notificationSettings()
        switch settings.alertStyle {
        case .none: return .none
        case .banner: return .banner
        case .alert: return .alert
        @unknown default: return .unknown
        }
    }
}

// MARK: - InterventionNotifier

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
/// of its own — it moves an approved offer to a surface the person can see,
/// reports what became of it (proto v35), and takes the banner down once the
/// offer is gone.
@MainActor
public final class InterventionNotifier: ObservableObject {
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
        /// Already older than `maximumPostingAge` when this process first saw
        /// it: a relaunch while the offer was live. A banner then would be a
        /// claim about a moment that has passed, so it is never posted. Sticky.
        case expired
    }

    /// How old an offer may be, when this process first sees it, and still be
    /// posted. An offer reaches a running app within moments of being made,
    /// so only a relaunch or a long-stalled connection meets an older one.
    public static let maximumPostingAge: TimeInterval = 120

    private let scheduler: any InterventionNotificationScheduling
    private let permissionManager: any PermissionManagerProtocol
    private let reporter: any NotificationDeliveryReporting
    private weak var deliveryFacts: (any InterventionDeliveryRecording)?
    private let alertStyleReader: (any NotificationAlertStyleReading)?
    private let now: () -> Date
    private var cancellables = Set<AnyCancellable>()

    private var currentKey: OfferKey?
    private var currentOffer: ActiveIntervention? {
        didSet { refreshOfferWaiting() }
    }
    private var disposition: Disposition? {
        didSet { refreshOfferWaiting() }
    }
    /// The last channel this offer settled on, and the last one sent to the
    /// service for it. Kept apart so a republished snapshot that retries a
    /// denied offer does not send the same fact again.
    private var settledChannel: InterventionDeliveryChannel? {
        didSet { refreshOfferWaiting() }
    }
    private var reportedChannel: InterventionDeliveryChannel?

    /// True while an offer is live and no banner or alert for it has been put
    /// on screen: notifications are off, unanswered or set to alert style
    /// None, the offer is quiet, or it was too old to post. The menu bar shows
    /// a passive "offer waiting" state for it, the one surface outside the
    /// popover the person can see while they are elsewhere. False while the
    /// first attempt is still settling, so a posted offer never flickers it.
    @Published public private(set) var isOfferWaitingUnshown = false
    /// The system prompt is asked for at most once per offer, so a retry loop
    /// cannot turn one offer into repeated authorization requests.
    private var hasRequestedPermission = false
    private var isAttempting = false
    /// Numbers each attempt, so one that `forgetCurrentOffer` cancelled can
    /// tell, when its await finally returns, that it no longer owns the
    /// one-at-a-time slot.
    private var attemptNumber = 0

    /// The most recent delivery task. Exposed so tests can await the
    /// permission-check/schedule work rather than racing it.
    public private(set) var inFlightTask: Task<Void, Never>?

    /// - Parameters:
    ///   - deliveryFacts: Receives what became of each offer's notification.
    ///     Held weakly; `nil` reports to the log alone.
    ///   - alertStyle: Tells a posted notification that was shown on screen
    ///     from one listed silently under alert style None. `nil` reports
    ///     every accepted request as `notification_posted`.
    ///   - now: The clock an offer's age is read against.
    public init(
        scheduler: any InterventionNotificationScheduling,
        permissionManager: any PermissionManagerProtocol,
        reporter: any NotificationDeliveryReporting = OSLogNotificationDeliveryReporter(),
        deliveryFacts: (any InterventionDeliveryRecording)? = nil,
        alertStyle: (any NotificationAlertStyleReading)? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.scheduler = scheduler
        self.permissionManager = permissionManager
        self.reporter = reporter
        self.deliveryFacts = deliveryFacts
        self.alertStyleReader = alertStyle
        self.now = now
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
        let isFirstSight = key != currentKey
        if isFirstSight {
            // A different offer replaced this one without a snapshot in
            // between: the old banner is no longer true either.
            if let replaced = currentKey {
                removeBanner(for: replaced)
            }
            currentKey = key
            disposition = nil
            settledChannel = nil
            reportedChannel = nil
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
                settle(.quiet, for: key)
            }
            return nil
        }
        // Only a relaunch, or a connection that stalled for minutes, meets an
        // offer this old the first time. Whatever an earlier process reported
        // for it still stands, so nothing is sent: this is not an attempt.
        if isFirstSight, now().timeIntervalSince(intervention.offeredAt) > Self.maximumPostingAge {
            disposition = .expired
            reporter.report(.skippedAsStale, surface: .driftOffer)
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
        // The offer is over: answered, withdrawn by a return, or the block
        // ended. A banner left in Notification Center would go on claiming
        // drift that is no longer true, and a tap on it would open a popover
        // with no card. The identifier is the offer's own, so this also takes
        // down a banner an earlier process posted for it.
        if let key = currentKey {
            removeBanner(for: key)
        }
        currentKey = nil
        currentOffer = nil
        disposition = nil
        settledChannel = nil
        reportedChannel = nil
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
        attemptNumber += 1
        let attempt = attemptNumber

        let task = Task { @MainActor [weak self, scheduler, permissionManager, reporter, alertStyleReader] in
            // `notDetermined` maps to `.unknown`. Onboarding can reach a first
            // work block without ever having asked about notifications, so
            // checking alone would drop the offer in silence. Ask once, at the
            // moment there is something worth showing.
            let checked = await permissionManager.checkStatus(for: .notifications)
            // Cancelled means the service withdrew the offer while the check
            // was in flight. Said out loud: this used to be the one exit that
            // left nothing on the log, which is exactly what the founder's Mac
            // showed on 2026-09-25 — one settings query, then silence.
            guard !Task.isCancelled else {
                self?.finishAttempt(attempt, key: key, disposition: .undelivered, channel: .withdrawn)
                reporter.report(.withdrawnBeforeDelivery, surface: .driftOffer)
                return
            }
            var status = checked
            if checked == .unknown, self?.hasRequestedPermission != true {
                self?.hasRequestedPermission = true
                status = await permissionManager.requestPermission(for: .notifications)
            }
            guard !Task.isCancelled else {
                self?.finishAttempt(attempt, key: key, disposition: .undelivered, channel: .withdrawn)
                reporter.report(.withdrawnBeforeDelivery, surface: .driftOffer)
                return
            }
            guard status == .granted else {
                // Not a decision this app made, so the offer is not spent:
                // it stays eligible while it is still on screen.
                self?.finishAttempt(
                    attempt, key: key, disposition: .undelivered,
                    channel: status == .denied ? .permissionDenied : .permissionUnknown)
                reporter.report(.blockedByPermission(status), surface: .driftOffer)
                return
            }
            let scheduled = await scheduler.scheduleIntervention(
                id: Self.notificationID(for: key),
                title: offer.title,
                body: offer.body
            )
            guard scheduled else {
                self?.finishAttempt(attempt, key: key, disposition: .undelivered, channel: .rejected)
                reporter.report(.rejectedByNotificationCentre, surface: .driftOffer)
                return
            }
            // Accepted is not the same as shown. Alert style None passes the
            // permission check and takes every request, and shows none of them.
            let style = await alertStyleReader?.alertStyle() ?? .unknown
            let channel: InterventionDeliveryChannel = style == .none ? .alertStyleNone : .notificationPosted
            self?.finishAttempt(attempt, key: key, disposition: .delivered, channel: channel)
            reporter.report(style == .none ? .deliveredWithAlertStyleNone : .delivered, surface: .driftOffer)
        }
        inFlightTask = task
        return task
    }

    private func finishAttempt(
        _ attempt: Int,
        key: OfferKey,
        disposition attempted: Disposition,
        channel: InterventionDeliveryChannel
    ) {
        // An offer withdrawn while its request was already on the way to the
        // centre is posted after `forgetCurrentOffer` took its banner down.
        // Take this one down too: it is no longer true.
        if attempted == .delivered, currentKey != key {
            removeBanner(for: key)
        }
        // Whatever became of the attempt is a fact about that offer, whether
        // or not it is still the current one.
        settle(channel, for: key)
        // An attempt `forgetCurrentOffer` cancelled still comes back here,
        // once the await it was in returns: the settings query and the system
        // alert both ignore cancellation. By then its slot was freed, and the
        // next offer's attempt may hold it. Clearing the slot here let that
        // offer start a second attempt and orphan the first, which could then
        // post after that offer was withdrawn too.
        guard attempt == attemptNumber, isAttempting else { return }
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

    /// Reports `channel` for `key`'s offer to the service, once per change.
    ///
    /// The current offer's last report is remembered, so the retry a
    /// republished snapshot makes for a denied offer does not send the same
    /// fact each time. A fact about an offer that is already gone (an attempt
    /// that settled after the withdrawal) is sent as it is.
    private func settle(_ channel: InterventionDeliveryChannel, for key: OfferKey) {
        if key == currentKey {
            settledChannel = channel
            guard reportedChannel != channel else { return }
            reportedChannel = channel
        }
        deliveryFacts?.reportInterventionDelivery(blockID: key.blockID, channel: channel)
    }

    private func removeBanner(for key: OfferKey) {
        scheduler.removeDeliveredIntervention(id: Self.notificationID(for: key))
    }

    private func refreshOfferWaiting() {
        let waiting: Bool
        if currentOffer == nil {
            waiting = false
        } else if disposition == .expired {
            waiting = true
        } else if let settledChannel {
            waiting = !settledChannel.wasShownOnScreen
        } else {
            waiting = false
        }
        if isOfferWaitingUnshown != waiting {
            isOfferWaitingUnshown = waiting
        }
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
