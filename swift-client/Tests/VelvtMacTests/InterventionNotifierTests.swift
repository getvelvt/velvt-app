import Combine
import XCTest

@testable import VelvtMac

@MainActor
final class InterventionNotifierTests: XCTestCase {
    private func snapshot(
        blockID: UUID,
        offeredAt: Date?,
        salience: InterventionSalience = .normal
    ) -> WorkBlockSnapshot {
        WorkBlockSnapshot(
            stateVersion: 1,
            phase: .active,
            blockID: blockID,
            intention: "Local intention",
            purpose: .deepWork,
            intensity: .medium,
            plannedDurationSeconds: 1_500,
            elapsedDurationSeconds: 600,
            remainingDurationSeconds: 900,
            startedAt: Date(timeIntervalSince1970: 1_800_000_000),
            endsAt: Date(timeIntervalSince1970: 1_800_001_500),
            pausedAt: nil,
            recoveredAfterRestart: false,
            currentCategory: "COMMUNICATION",
            anchorCategory: nil,
            classificationStatus: .classified,
            confidence: .high,
            statusLine: "Current category: Communication.",
            result: nil,
            activeIntervention: offeredAt.map {
                ActiveIntervention(
                    actionID: "protect_next_10",
                    title: "Your work block is running",
                    body: "Velvt observed 4 switches away from focus work in the last 10 minutes.",
                    anchorCategory: "FOCUS_WORK",
                    switchCount: 4,
                    windowSeconds: 600,
                    offeredAt: $0,
                    salience: salience
                )
            }
        )
    }

    private func makeNotifier(
        status: PermissionStatus = .granted
    ) -> (InterventionNotifier, FakeNotificationScheduler, StubPermissionManager) {
        let scheduler = FakeNotificationScheduler()
        let permissions = StubPermissionManager(status: status)
        let notifier = InterventionNotifier(scheduler: scheduler, permissionManager: permissions)
        return (notifier, scheduler, permissions)
    }

    /// The exact `work_block_state` push Rust emits when a drift offer is
    /// recorded, byte-shaped as it arrives on the socket. Decoding it here
    /// means the test exercises the wire the running app reads, not a
    /// hand-assembled Swift value that happens to resemble it.
    private func driftOfferPush(
        blockID: UUID,
        offeredAtEpoch: Int,
        salience: String = "normal"
    ) throws -> ServerMessage {
        let offeredAt = ISO8601DateFormatter().string(
            from: Date(timeIntervalSince1970: TimeInterval(offeredAtEpoch)))
        let json = """
            {
              "type": "work_block_state",
              "payload": {
                "state_version": 1,
                "phase": "active",
                "block_id": "\(blockID.uuidString.lowercased())",
                "intention": "Ship the drift notification",
                "purpose": "deep_work",
                "intensity": "medium",
                "planned_duration_seconds": 1500,
                "elapsed_duration_seconds": 624,
                "remaining_duration_seconds": 876,
                "started_at": "\(offeredAt)",
                "ends_at": "\(offeredAt)",
                "paused_at": null,
                "recovered_after_restart": false,
                "current_category": "COMMUNICATION",
                "classification_status": "classified",
                "confidence": "high",
                "status_line": "Current category: Communication.",
                "result": null,
                "active_intervention": {
                  "action_id": "protect_next_10",
                  "title": "Your work block is running",
                  "body": "Velvt observed 4 switches away from focus work in the last 10 minutes.",
                  "anchor_category": "FOCUS_WORK",
                  "switch_count": 4,
                  "window_seconds": 600,
                  "offered_at": "\(offeredAt)",
                  "salience": "\(salience)"
                }
              }
            }
            """
        return try IPCMessageCodec.makeDecoder().decode(
            ServerMessage.self, from: Data(json.utf8))
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        condition: @escaping @MainActor () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            if clock.now >= deadline {
                XCTFail("condition timed out")
                return
            }
            await Task.yield()
        }
    }

    /// The regression this whole type exists for: an offer that only ever
    /// rendered in the popover reached nobody who had drifted away from it.
    func test_a_normal_offer_is_delivered_as_a_notification() async {
        let (notifier, scheduler, _) = makeNotifier()

        await notifier.handle(snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value

        XCTAssertEqual(scheduler.scheduledInterventions.count, 1)
        let delivered = scheduler.scheduledInterventions.first
        XCTAssertEqual(delivered?.title, "Your work block is running")
        XCTAssertEqual(
            delivered?.body,
            "Velvt observed 4 switches away from focus work in the last 10 minutes.",
            "Rust authors the copy; the notifier must pass it through unchanged"
        )
    }

    /// The snapshot republishes for as long as the offer is unanswered, so
    /// naive delivery would ring once per state change.
    func test_the_same_offer_is_never_delivered_twice() async {
        let (notifier, scheduler, _) = makeNotifier()
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        for _ in 0..<5 {
            await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value
        }

        XCTAssertEqual(scheduler.scheduledInterventions.count, 1)
    }

    /// Invariant 2: a dismissal buys quiet. A quiet offer shows the in-app
    /// card and must not ring.
    func test_a_quiet_offer_does_not_ring() async {
        let (notifier, scheduler, _) = makeNotifier()

        await notifier.handle(
            snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000), salience: .quiet)
        )?.value

        XCTAssertTrue(scheduler.scheduledInterventions.isEmpty)
    }

    /// A quiet offer already claimed its identity, so a later snapshot of the
    /// same offer cannot ring by arriving with a different salience.
    func test_a_quiet_offer_cannot_ring_later_under_the_same_identity() async {
        let (notifier, scheduler, _) = makeNotifier()
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt, salience: .quiet))?.value
        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt, salience: .normal))?.value

        XCTAssertTrue(scheduler.scheduledInterventions.isEmpty)
    }

    /// A fresh offer after the previous one resolved is a different offer.
    func test_a_new_offer_after_the_first_resolves_is_delivered() async {
        let (notifier, scheduler, _) = makeNotifier()
        let block = UUID()

        await notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value
        // Answered: the snapshot returns with no active offer.
        await notifier.handle(snapshot(blockID: block, offeredAt: nil))?.value
        await notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 3000)))?
            .value

        XCTAssertEqual(scheduler.scheduledInterventions.count, 2)
        XCTAssertNotEqual(
            scheduler.scheduledInterventions[0].id,
            scheduler.scheduledInterventions[1].id,
            "each offer needs its own identifier so one banner cannot replace the other"
        )
    }

    func test_nothing_is_delivered_when_notifications_are_denied() async {
        let (notifier, scheduler, _) = makeNotifier(status: .denied)

        await notifier.handle(snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value

        XCTAssertTrue(scheduler.scheduledInterventions.isEmpty)
    }

    /// Onboarding can reach a first work block without ever asking about
    /// notifications. Checking alone would drop every offer in silence.
    func test_an_undetermined_permission_is_requested_rather_than_assumed() async {
        let (notifier, scheduler, permissions) = makeNotifier(status: .unknown)
        permissions.statusAfterRequest = .granted

        await notifier.handle(snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value

        XCTAssertEqual(permissions.requestCount, 1)
        XCTAssertEqual(scheduler.scheduledInterventions.count, 1)
    }

    // MARK: - Delivery that survives an unauthorized moment

    /// The regression from the first drift intervention this product ever
    /// fired: the card rendered, and no notification was ever posted.
    ///
    /// Both surfaces read the same `active_intervention` off the same
    /// publisher. The notifier adds one hop the card does not have — a
    /// notifications-permission check — and the offer's identity used to be
    /// claimed *before* that hop. So an offer that met a non-granted status at
    /// the instant it was live was marked notified and could never ring, not
    /// on the next republish and not after the person turned notifications on.
    ///
    /// This drives the whole path the running app runs: a Rust
    /// `work_block_state` push, decoded by the IPC codec, into
    /// `WorkBlockCoordinator`, out of its `@Published` snapshot, into the
    /// notifier, into the real `UNNotificationScheduler` — and asserts a
    /// request reached `UNUserNotificationCenter.add(_:)`.
    func test_an_offer_blocked_by_permission_reaches_the_centre_once_permission_arrives()
        async throws
    {
        let center = FakeUNUserNotificationCenter()
        let scheduler = UNNotificationScheduler(center: center)
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: scheduler,
            permissionManager: permissions,
            reporter: reporter
        )

        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let coordinator = WorkBlockCoordinator(ipcClient: client)
        coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
        notifier.start(snapshots: coordinator.$snapshot)

        let block = UUID()
        messages.send(try driftOfferPush(blockID: block, offeredAtEpoch: 1_787_536_953))
        try await waitUntil { coordinator.snapshot?.activeIntervention != nil }
        try await waitUntil { reporter.entries.count == 1 }
        XCTAssertEqual(
            reporter.entries.first?.outcome,
            .blockedByPermission(.denied),
            "the drop has to be recorded, not returned in silence"
        )
        XCTAssertTrue(
            center.addedRequests.isEmpty,
            "nothing can be posted while notifications are not authorized"
        )

        // The person sees the card, turns notifications on, and the app comes
        // forward — which is exactly when PermissionManager republishes.
        permissions.setStatus(.granted, for: .notifications)
        try await waitUntil { reporter.entries.count == 2 }
        XCTAssertEqual(reporter.entries.last?.outcome, .delivered)

        XCTAssertEqual(center.addedRequests.count, 1)
        let request = try XCTUnwrap(center.addedRequests.first)
        XCTAssertEqual(request.content.title, "Your work block is running")
        XCTAssertEqual(
            request.content.body,
            "Velvt observed 4 switches away from focus work in the last 10 minutes."
        )
        XCTAssertEqual(
            request.content.userInfo[interventionNotificationUserInfoKey] as? Bool,
            true,
            "a tap has to open the popover where the reply buttons are, not an insight date"
        )
        XCTAssertEqual(
            request.identifier,
            "velvt.intervention.\(block.uuidString).1787536953",
            "the identifier stays stable per offer so a redelivery replaces its banner"
        )
    }

    /// A permission alert the user walks away from must not take drift
    /// delivery down with it. `isAttempting` is set before the permission
    /// round trip and cleared only when that trip returns, so an unanswered
    /// system alert left the one-at-a-time guard latched for the life of the
    /// process — and the alert is only ever shown on a machine that has not
    /// answered it before, so this was a fresh install losing every offer it
    /// would ever make, silently, starting with its first.
    func test_an_unanswered_permission_alert_does_not_latch_out_later_offers() async throws {
        let center = FakeUNUserNotificationCenter()
        let scheduler = UNNotificationScheduler(center: center)
        let permissions = HangingPermissionManager()
        let notifier = InterventionNotifier(scheduler: scheduler, permissionManager: permissions)

        // The first offer asks, and the ask never comes back.
        let stalled = notifier.handle(
            snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))
        XCTAssertNotNil(stalled, "the first offer starts an attempt")
        try await waitUntil { permissions.requestCount == 1 }
        XCTAssertTrue(center.addedRequests.isEmpty, "nothing can ring while the ask is open")

        // The block ends; the offer it belonged to is gone.
        await notifier.handle(snapshot(blockID: UUID(), offeredAt: nil))?.value

        // A later block, on a machine where permission has since been granted.
        // The property under test is that this offer is taken up PROMPTLY.
        // Awaiting the stalled attempt instead would pass either way: the
        // unanswered alert does eventually return, and delivery then works —
        // it is the interval before it does, with every offer turned away,
        // that is the defect.
        permissions.setStatus(.granted, for: .notifications)
        _ = notifier.handle(
            snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 2000)))
        try await waitUntil(timeout: .seconds(2)) { center.addedRequests.count >= 1 }

        stalled?.cancel()
    }

    /// The offer is republished for as long as it is unanswered. A drop that
    /// was never a product decision has to be reconsidered on the next one.
    func test_an_offer_blocked_by_permission_is_retried_on_the_next_snapshot() async throws {
        let center = FakeUNUserNotificationCenter()
        let scheduler = UNNotificationScheduler(center: center)
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let notifier = InterventionNotifier(scheduler: scheduler, permissionManager: permissions)
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value
        XCTAssertTrue(center.addedRequests.isEmpty)

        permissions.setStatus(.granted, for: .notifications)
        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value

        XCTAssertEqual(center.addedRequests.count, 1)

        // And still exactly once: the republished snapshot cannot ring twice.
        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value
        XCTAssertEqual(center.addedRequests.count, 1)
    }

    /// A request the centre refuses is not a decision this product made, so
    /// the offer stays eligible while it is still on screen.
    func test_a_request_rejected_by_the_centre_leaves_the_offer_eligible() async throws {
        let center = FakeUNUserNotificationCenter()
        center.rejectAdds(
            with: NSError(domain: "UNErrorDomain", code: 1, userInfo: nil))
        let scheduler = UNNotificationScheduler(center: center)
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let notifier = InterventionNotifier(scheduler: scheduler, permissionManager: permissions)
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value
        XCTAssertTrue(center.addedRequests.isEmpty)

        center.acceptAdds()
        await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value

        XCTAssertEqual(center.addedRequests.count, 1)
    }

    /// A permission gate that returns in silence makes a channel that has
    /// never delivered anything look exactly like a healthy one.
    func test_a_blocked_delivery_is_reported_rather_than_dropped_in_silence() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = StubPermissionManager(status: .denied)
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: scheduler, permissionManager: permissions, reporter: reporter)

        await notifier.handle(snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value

        XCTAssertEqual(
            reporter.entries,
            [.init(outcome: .blockedByPermission(.denied), surface: .driftOffer)]
        )
    }

    /// Quiet is a decision, not a circumstance: it is reported as suppression
    /// and never retried, however many times the offer republishes.
    func test_a_quiet_offer_is_reported_as_suppressed_exactly_once() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = StubPermissionManager(status: .granted)
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: scheduler, permissionManager: permissions, reporter: reporter)
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        for _ in 0..<3 {
            await notifier.handle(
                snapshot(blockID: block, offeredAt: offeredAt, salience: .quiet))?.value
        }

        XCTAssertTrue(scheduler.scheduledInterventions.isEmpty)
        XCTAssertEqual(
            reporter.entries,
            [.init(outcome: .suppressedBySalience, surface: .driftOffer)]
        )
    }

    /// The founder's Mac on 2026-09-25: one notification-settings query, no
    /// request added, and nothing on the log. The offer had been withdrawn
    /// while the query was in flight, and that exit was the only one that
    /// reported nothing. Not posting is right — the offer was no longer true —
    /// but it has to be said.
    func test_an_offer_withdrawn_during_the_permission_check_is_reported_as_withdrawn() async throws {
        let center = FakeUNUserNotificationCenter()
        let permissions = GatedPermissionManager()
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: UNNotificationScheduler(center: center),
            permissionManager: permissions,
            reporter: reporter
        )
        let block = UUID()

        let attempt = notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 1000)))
        try await waitUntil { permissions.checkCount == 1 }
        XCTAssertNil(notifier.handle(snapshot(blockID: block, offeredAt: nil)))
        permissions.release(with: .granted)
        await attempt?.value

        XCTAssertTrue(center.addedRequests.isEmpty, "a withdrawn offer is not posted")
        XCTAssertEqual(
            reporter.entries,
            [.init(outcome: .withdrawnBeforeDelivery, surface: .driftOffer)]
        )
    }

    /// Withdrawn while the system alert was up is still withdrawn. It used to
    /// be logged as `notification_permission_blocked`, with whatever status
    /// the interrupted request came back with.
    func test_an_offer_withdrawn_during_the_permission_request_is_not_reported_as_blocked() async throws {
        let permissions = HangingPermissionManager()
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: FakeNotificationScheduler(), permissionManager: permissions, reporter: reporter)
        let block = UUID()

        let attempt = notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 1000)))
        try await waitUntil { permissions.requestCount == 1 }
        XCTAssertNil(notifier.handle(snapshot(blockID: block, offeredAt: nil)))
        await attempt?.value

        XCTAssertEqual(
            reporter.entries,
            [.init(outcome: .withdrawnBeforeDelivery, surface: .driftOffer)]
        )
    }

    /// A withdrawn offer's attempt can still be waiting on its settings query
    /// when the next offer arrives and starts its own. When the stale one
    /// finally returns it must leave that attempt alone. It used to free the
    /// one-at-a-time slot, so the new offer got a second attempt and the first
    /// was orphaned: withdrawing the new offer then cancelled only the second,
    /// and the orphan posted a banner for an offer that was no longer true.
    func test_a_withdrawn_attempt_that_returns_late_leaves_the_next_offers_attempt_alone() async throws {
        let center = FakeUNUserNotificationCenter()
        let permissions = GatedPermissionManager()
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: UNNotificationScheduler(center: center),
            permissionManager: permissions,
            reporter: reporter
        )
        let block = UUID()

        let first = notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 1000)))
        try await waitUntil { permissions.checkCount == 1 }
        XCTAssertNil(notifier.handle(snapshot(blockID: block, offeredAt: nil)))
        let second = notifier.handle(snapshot(blockID: block, offeredAt: Date(timeIntervalSince1970: 2000)))
        XCTAssertNotNil(second, "the withdrawn offer's attempt no longer holds the slot")
        try await waitUntil { permissions.checkCount == 2 }

        permissions.release(with: .granted)
        await first?.value
        XCTAssertEqual(permissions.checkCount, 2, "the late return starts no second attempt for the new offer")

        XCTAssertNil(notifier.handle(snapshot(blockID: block, offeredAt: nil)))
        permissions.release(with: .granted)
        await second?.value

        XCTAssertTrue(center.addedRequests.isEmpty, "neither withdrawn offer is posted")
        XCTAssertEqual(
            reporter.entries,
            [
                .init(outcome: .withdrawnBeforeDelivery, surface: .driftOffer),
                .init(outcome: .withdrawnBeforeDelivery, surface: .driftOffer),
            ]
        )
    }

    /// With in-progress reports (proto v32) the offer arrives while the person
    /// is away and is withdrawn when they come back. The two pushes, decoded
    /// from the wire, through the coordinator the app wires: the banner is
    /// posted in between, once.
    func test_an_offer_pushed_while_away_is_posted_before_the_return_withdraws_it() async throws {
        let wired = wiredNotifier(permissionManager: StubPermissionManager(status: .granted))
        let block = UUID()

        wired.messages.send(try driftOfferPush(blockID: block, offeredAtEpoch: 1_790_388_157))
        try await waitUntil { wired.center.addedRequests.count == 1 }

        wired.messages.send(.workBlockState(snapshot(blockID: block, offeredAt: nil)))
        try await waitUntil { wired.coordinator.snapshot?.activeIntervention == nil }

        XCTAssertEqual(wired.center.addedRequests.count, 1)
        XCTAssertEqual(wired.reporter.entries, [.init(outcome: .delivered, surface: .driftOffer)])
    }

    /// On the Mac the report that withdrew the offer came about a second after
    /// the push that made it (22:03:26 to 22:03:27), and the settings query in
    /// between took about 5 ms (22:03:26.293 to .298). Posting waits on that
    /// one round trip and nothing else, so an offer withdrawn a second after
    /// its push has already been posted. The withdrawal is sent on a timer,
    /// not after the post is seen, so any debounce, coalescing or delay added
    /// between the socket and the notification centre fails here.
    func test_an_offer_withdrawn_a_second_after_its_push_was_already_posted() async throws {
        let permissions = LatencyPermissionManager(latency: .milliseconds(5))
        let wired = wiredNotifier(permissionManager: permissions)
        let block = UUID()

        wired.messages.send(try driftOfferPush(blockID: block, offeredAtEpoch: 1_790_388_157))
        try await Task.sleep(for: .seconds(1))
        wired.messages.send(.workBlockState(snapshot(blockID: block, offeredAt: nil)))
        try await waitUntil { wired.coordinator.snapshot?.activeIntervention == nil }

        XCTAssertEqual(
            wired.center.addedRequests.map(\.identifier),
            ["velvt.intervention.\(block.uuidString).1790388157"]
        )
        XCTAssertEqual(wired.reporter.entries, [.init(outcome: .delivered, surface: .driftOffer)])
        XCTAssertEqual(permissions.checkCount, 1)
    }

    /// The Mac's pair, offer and withdrawal back to back, still happens under
    /// protocol 32 for a glance away shorter than the settings round trip.
    /// Fed through the coordinator, with a settings query that, like the real
    /// XPC call, ignores cancellation and answers a few milliseconds later.
    /// Nothing can be posted. The attempt still has to end in a line on the
    /// log, and not in the one settings query and silence the Mac showed.
    func test_an_offer_withdrawn_right_behind_its_push_is_reported_rather_than_dropped() async throws {
        let permissions = LatencyPermissionManager(latency: .milliseconds(5))
        let wired = wiredNotifier(permissionManager: permissions)
        let block = UUID()

        wired.messages.send(try driftOfferPush(blockID: block, offeredAtEpoch: 1_790_388_157))
        wired.messages.send(.workBlockState(snapshot(blockID: block, offeredAt: nil)))
        try await waitUntil { !wired.reporter.entries.isEmpty }
        try await Task.sleep(for: .milliseconds(50))

        XCTAssertEqual(permissions.checkCount, 1, "one settings query, as on the Mac")
        XCTAssertTrue(wired.center.addedRequests.isEmpty, "a withdrawn offer is not posted")
        XCTAssertEqual(
            wired.reporter.entries,
            [.init(outcome: .withdrawnBeforeDelivery, surface: .driftOffer)]
        )
        XCTAssertNil(wired.coordinator.snapshot?.activeIntervention)
    }

    private struct WiredNotifier {
        let messages: PassthroughSubject<ServerMessage, Never>
        let coordinator: WorkBlockCoordinator
        let notifier: InterventionNotifier
        let center: FakeUNUserNotificationCenter
        let reporter: RecordingNotificationDeliveryReporter
    }

    /// The notifier fed from the coordinator's snapshot, as `AppModule` wires
    /// it, with pushes sent in as the socket delivers them.
    private func wiredNotifier(permissionManager: any PermissionManagerProtocol) -> WiredNotifier {
        let center = FakeUNUserNotificationCenter()
        let reporter = RecordingNotificationDeliveryReporter()
        let notifier = InterventionNotifier(
            scheduler: UNNotificationScheduler(center: center),
            permissionManager: permissionManager,
            reporter: reporter
        )
        let coordinator = WorkBlockCoordinator(ipcClient: FakeIPCClient())
        let messages = PassthroughSubject<ServerMessage, Never>()
        coordinator.start(
            messages: messages,
            connectionStatus: Empty<ConnectionStatus, Never>(),
            workspaceNotifications: NotificationCenter(),
            systemNotifications: NotificationCenter()
        )
        notifier.start(snapshots: coordinator.$snapshot)
        return WiredNotifier(
            messages: messages,
            coordinator: coordinator,
            notifier: notifier,
            center: center,
            reporter: reporter
        )
    }

    /// Retrying must not become a way to re-ask for authorization: the system
    /// prompt is worth at most one appearance per offer.
    func test_a_retried_offer_asks_for_authorization_at_most_once() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = StubPermissionManager(status: .unknown)
        permissions.statusAfterRequest = .denied
        let notifier = InterventionNotifier(scheduler: scheduler, permissionManager: permissions)
        let block = UUID()
        let offeredAt = Date(timeIntervalSince1970: 1000)

        for _ in 0..<4 {
            await notifier.handle(snapshot(blockID: block, offeredAt: offeredAt))?.value
        }

        XCTAssertEqual(permissions.requestCount, 1)
        XCTAssertTrue(scheduler.scheduledInterventions.isEmpty)
    }
}

private final class StubPermissionManager: PermissionManagerProtocol, @unchecked Sendable {
    private let status: PermissionStatus
    var statusAfterRequest: PermissionStatus = .denied
    private(set) var requestCount = 0

    init(status: PermissionStatus) {
        self.status = status
    }

    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        Just([.notifications: status]).eraseToAnyPublisher()
    }

    func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        permission == .notifications ? status : .unknown
    }

    func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        requestCount += 1
        return statusAfterRequest
    }
}

/// The system permission alert a user opens and never answers. `Task.sleep`
/// is cancellation-aware, so a cancelled attempt unwinds here exactly as a
/// real one would.
private final class HangingPermissionManager: PermissionManagerProtocol, @unchecked Sendable {
    private let subject = CurrentValueSubject<[PermissionType: PermissionStatus], Never>(
        [.notifications: .unknown])
    private(set) var requestCount = 0

    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        subject.eraseToAnyPublisher()
    }

    func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        subject.value[permission] ?? .unknown
    }

    func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        requestCount += 1
        try? await Task.sleep(nanoseconds: 60_000_000_000)
        return subject.value[permission] ?? .unknown
    }

    func setStatus(_ status: PermissionStatus, for permission: PermissionType) {
        var statuses = subject.value
        statuses[permission] = status
        subject.send(statuses)
    }
}

/// Notification-settings queries that answer only when the test says so,
/// oldest first, so an offer can be withdrawn while its query is in flight.
private final class GatedPermissionManager: PermissionManagerProtocol, @unchecked Sendable {
    private let subject = CurrentValueSubject<[PermissionType: PermissionStatus], Never>(
        [.notifications: .granted])
    private let lock = NSLock()
    private var pending: [CheckedContinuation<PermissionStatus, Never>] = []
    private var checks = 0

    var checkCount: Int { lock.withLock { checks } }

    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        subject.eraseToAnyPublisher()
    }

    func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        await withCheckedContinuation { continuation in
            lock.withLock {
                pending.append(continuation)
                checks += 1
            }
        }
    }

    func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        .granted
    }

    /// Answers the oldest query still waiting.
    func release(with status: PermissionStatus) {
        let oldest = lock.withLock { pending.isEmpty ? nil : pending.removeFirst() }
        oldest?.resume(returning: status)
    }
}

/// A notification-settings query that answers `.granted` after a fixed
/// latency. Like the real `notificationSettings()` XPC call, it does not
/// observe task cancellation: a withdrawn offer's attempt still waits it out.
private final class LatencyPermissionManager: PermissionManagerProtocol, @unchecked Sendable {
    private let latency: DispatchTimeInterval
    private let lock = NSLock()
    private var checks = 0

    init(latency: DispatchTimeInterval) {
        self.latency = latency
    }

    var checkCount: Int { lock.withLock { checks } }

    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        Just([.notifications: .granted]).eraseToAnyPublisher()
    }

    func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        lock.withLock { checks += 1 }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global().asyncAfter(deadline: .now() + latency) {
                continuation.resume()
            }
        }
        return .granted
    }

    func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        .granted
    }
}
