import Combine
import XCTest
@testable import VelvtMac

@MainActor
final class NotificationDeliveryCoordinatorTests: XCTestCase {

    private func makePayload(doNotDisturbUntil: Date? = nil) -> NotificationPayload {
        NotificationPayload(
            notificationID: UUID(),
            title: "Daily insight",
            body: "Stayed focused through the afternoon.",
            insightDate: "2026-06-15",
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
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)
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
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)
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
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)

        let payload = makePayload()
        await sut.handle(payload).value

        XCTAssertEqual(scheduler.scheduledPayloads, [payload])
    }

    func testHandleDiscardsSilentlyWhenDenied() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    func testHandleDiscardsSilentlyWhenRestricted() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.restricted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    func testHandleDiscardsSilentlyWhenUnknown() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        // .unknown is the FakePermissionManager default for .notifications.
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)

        await sut.handle(makePayload()).value

        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    // MARK: - Non-notification messages are ignored

    func testIgnoresUnrelatedServerMessages() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions)
        let relay = PassthroughSubject<ServerMessage, Never>()
        sut.start(serverMessages: relay)

        relay.send(.accountDeletionAccepted)

        XCTAssertNil(sut.inFlightTask)
        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }
}
