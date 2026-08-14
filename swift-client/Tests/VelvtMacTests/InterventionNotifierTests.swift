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
            classificationStatus: .classified,
            confidence: .high,
            statusLine: "Current category: Communication.",
            result: nil,
            activeIntervention: offeredAt.map {
                ActiveIntervention(
                    actionID: "protect_next_10",
                    title: "Your work block is still running",
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

    /// The regression this whole type exists for: an offer that only ever
    /// rendered in the popover reached nobody who had drifted away from it.
    func test_a_normal_offer_is_delivered_as_a_notification() async {
        let (notifier, scheduler, _) = makeNotifier()

        await notifier.handle(snapshot(blockID: UUID(), offeredAt: Date(timeIntervalSince1970: 1000)))?
            .value

        XCTAssertEqual(scheduler.scheduledInterventions.count, 1)
        let delivered = scheduler.scheduledInterventions.first
        XCTAssertEqual(delivered?.title, "Your work block is still running")
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
