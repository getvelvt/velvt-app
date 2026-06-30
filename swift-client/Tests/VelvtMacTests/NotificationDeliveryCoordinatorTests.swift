import Combine
import XCTest
@testable import VelvtMac

@MainActor
final class NotificationDeliveryCoordinatorTests: XCTestCase {

    private func makePayload(
        insightDate: String = "2026-06-15",
        body: String = "Stayed focused through the afternoon.",
        doNotDisturbUntil: Date? = nil
    ) -> NotificationPayload {
        NotificationPayload(
            notificationID: UUID(),
            title: "Daily insight",
            body: body,
            insightDate: insightDate,
            doNotDisturbUntil: doNotDisturbUntil
        )
    }

    // MARK: - End-to-end via FakeIPCClient

    func testNotificationPayloadFromIPCSchedulesWhenPermissionGranted() async throws {
        let client = FakeIPCClient()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        accountManager.startListening(to: client)

        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))
        sut.start(serverMessages: accountManager.serverMessages)

        let payload = makePayload()
        client.inject(.notificationPayload(payload))

        // Let AccountStateManager's listener task drain the injected message
        // and re-publish it before awaiting the coordinator's own task.
        try await Task.sleep(nanoseconds: 50_000_000)
        await sut.inFlightTask?.value

        XCTAssertEqual(scheduler.scheduledPayloads, [payload])
    }

    func testNotificationPayloadFromIPCDiscardedWhenPermissionDenied() async throws {
        let client = FakeIPCClient()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        accountManager.startListening(to: client)

        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))
        sut.start(serverMessages: accountManager.serverMessages)

        client.inject(.notificationPayload(makePayload()))

        try await Task.sleep(nanoseconds: 50_000_000)
        await sut.inFlightTask?.value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    // MARK: - Direct handle(_:) for permission-matrix coverage

    func testHandleForwardsToSchedulerWhenGranted() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let payload = makePayload()
        await sut.handle(payload).value

        XCTAssertEqual(scheduler.scheduledPayloads, [payload])
    }

    func testDebugSimulationUsesNotificationHandler() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let now = ISO8601DateFormatter().date(from: "2026-06-15T12:00:00Z")!
        await sut.simulateDebugInsightReceipt(now: now).value

        XCTAssertEqual(scheduler.scheduledPayloads.count, 1)
        XCTAssertEqual(scheduler.scheduledPayloads.first?.title, "Your Velvt insight is ready")
        XCTAssertEqual(scheduler.scheduledPayloads.first?.insightDate, "2026-06-15")
    }

    func testHandleDiscardsSilentlyWhenDenied() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    func testHandleDiscardsSilentlyWhenRestricted() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.restricted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    func testHandleDiscardsSilentlyWhenUnknown() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        // .unknown is the FakePermissionManager default for .notifications.
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    // MARK: - Burst de-duplication

    /// Rust sending a rapid burst of corrected payloads for the same insight
    /// date must not flood the user with one system notification per
    /// payload — only the most recently received one is scheduled.
    func testBurstOfPayloadsForTheSameDateSchedulesOnlyTheMostRecent() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let first = makePayload(body: "first")
        let second = makePayload(body: "second")
        let third = makePayload(body: "third")

        let t1 = sut.handle(first)
        let t2 = sut.handle(second)
        let t3 = sut.handle(third)
        await t1.value
        await t2.value
        await t3.value

        XCTAssertEqual(scheduler.scheduledPayloads.count, 1)
        XCTAssertEqual(scheduler.scheduledPayloads.first, third)
    }

    func testBurstOfPayloadsForDifferentDatesSchedulesOnePerDate() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let dayA = makePayload(insightDate: "2026-06-14")
        let dayB = makePayload(insightDate: "2026-06-15")

        let t1 = sut.handle(dayA)
        let t2 = sut.handle(dayB)
        await t1.value
        await t2.value

        XCTAssertEqual(scheduler.scheduledPayloads.count, 2)
        XCTAssertEqual(Set(scheduler.scheduledPayloads.map(\.insightDate)), ["2026-06-14", "2026-06-15"])
    }

    func testBurstViaFakeIPCClientSchedulesOnlyTheMostRecentForTheSameDate() async throws {
        let client = FakeIPCClient()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        accountManager.startListening(to: client)

        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))
        sut.start(serverMessages: accountManager.serverMessages)

        let third = makePayload(body: "third")
        client.inject(.notificationPayload(makePayload(body: "first")))
        client.inject(.notificationPayload(makePayload(body: "second")))
        client.inject(.notificationPayload(third))

        try await Task.sleep(nanoseconds: 80_000_000)
        await sut.inFlightTask?.value

        XCTAssertEqual(scheduler.scheduledPayloads.count, 1)
        XCTAssertEqual(scheduler.scheduledPayloads.first, third)
    }

    // MARK: - Non-notification messages are ignored

    func testIgnoresUnrelatedServerMessages() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))
        let relay = PassthroughSubject<ServerMessage, Never>()
        sut.start(serverMessages: relay)

        relay.send(.accountDeletionAccepted)

        XCTAssertNil(sut.inFlightTask)
        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }
}
