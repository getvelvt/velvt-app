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

    private let scheduler: any InterventionNotificationScheduling
    private let permissionManager: any PermissionManagerProtocol
    private var cancellables = Set<AnyCancellable>()
    private var lastNotified: OfferKey?

    /// The most recent delivery task. Exposed so tests can await the
    /// permission-check/schedule work rather than racing it.
    public private(set) var inFlightTask: Task<Void, Never>?

    public init(
        scheduler: any InterventionNotificationScheduling,
        permissionManager: any PermissionManagerProtocol
    ) {
        self.scheduler = scheduler
        self.permissionManager = permissionManager
    }

    /// - Parameter snapshots: the coordinator's published work-block state.
    public func start(snapshots: some Publisher<WorkBlockSnapshot?, Never>) {
        snapshots
            .receive(on: RunLoop.main)
            .sink { [weak self] snapshot in
                self?.handle(snapshot)
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
            lastNotified = nil
            return nil
        }

        let key = OfferKey(blockID: blockID, offeredAt: intervention.offeredAt)
        guard key != lastNotified else { return nil }

        // A quiet offer is the backoff state: the person pushed the last one
        // away, so this one renders in-app and does not ring. Claim the key
        // regardless, so raising salience later cannot re-ring the same offer.
        lastNotified = key
        guard intervention.salience == .normal else { return nil }

        let task = Task { @MainActor [scheduler, permissionManager] in
            // `notDetermined` maps to `.unknown`. Onboarding can reach a first
            // work block without ever having asked about notifications, so
            // checking alone would drop the offer in silence. Ask once, at the
            // moment there is something worth showing.
            let checked = await permissionManager.checkStatus(for: .notifications)
            guard !Task.isCancelled else { return }
            let status =
                checked == .unknown
                ? await permissionManager.requestPermission(for: .notifications)
                : checked
            guard status == .granted, !Task.isCancelled else { return }
            await scheduler.scheduleIntervention(
                id: Self.notificationID(for: key),
                title: intervention.title,
                body: intervention.body
            )
        }
        inFlightTask = task
        return task
    }

    /// Stable per-offer identifier, so a redelivery of the same offer replaces
    /// its banner instead of stacking a second one.
    private static func notificationID(for key: OfferKey) -> String {
        "velvt.intervention.\(key.blockID.uuidString).\(Int(key.offeredAt.timeIntervalSince1970))"
    }
}
