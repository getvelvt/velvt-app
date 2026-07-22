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

    func testSuccessfulScheduleIsDeduplicatedAcrossCoordinatorInstances() async {
        let tracker = InMemoryScheduledNotificationTracker()
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let payload = makePayload()

        let first = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissions,
            scheduledNotifications: tracker,
            debounceInterval: .milliseconds(1)
        )
        await first.handle(payload).value
        let afterRestart = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissions,
            scheduledNotifications: tracker,
            debounceInterval: .milliseconds(1)
        )
        await afterRestart.handle(payload).value

        XCTAssertEqual(scheduler.scheduledPayloads, [payload])
    }

    func testFailedScheduleRemainsEligibleForRetry() async {
        let tracker = InMemoryScheduledNotificationTracker()
        let scheduler = OutcomeNotificationScheduler(outcomes: [false, true])
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let payload = makePayload()
        let sut = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissions,
            scheduledNotifications: tracker,
            debounceInterval: .milliseconds(1)
        )

        await sut.handle(payload).value
        await sut.handle(payload).value

        XCTAssertEqual(scheduler.attemptedPayloads, [payload, payload])
        XCTAssertTrue(tracker.contains(payload.notificationID))
    }

    func testDeniedPayloadIsNotMarkedScheduled() async {
        let tracker = InMemoryScheduledNotificationTracker()
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let payload = makePayload()
        let sut = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissions,
            scheduledNotifications: tracker,
            debounceInterval: .milliseconds(1)
        )

        await sut.handle(payload).value

        XCTAssertFalse(tracker.contains(payload.notificationID))
    }

    func testPersistentLedgerIsBoundedAndStoresOnlyOpaqueIdentifiers() throws {
        let suite = "NotificationDeliveryCoordinatorTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let tracker = UserDefaultsScheduledNotificationTracker(
            defaults: defaults,
            key: "testNotificationIDs",
            capacity: 2
        )
        let ids = [UUID(), UUID(), UUID()]

        ids.forEach(tracker.record)

        XCTAssertFalse(tracker.contains(ids[0]))
        XCTAssertTrue(tracker.contains(ids[1]))
        XCTAssertTrue(tracker.contains(ids[2]))
        let stored = try XCTUnwrap(defaults.stringArray(forKey: "testNotificationIDs"))
        XCTAssertEqual(stored, ids.suffix(2).map(\.uuidString))
    }

    func testDebugSimulationSchedulesImmediateNativeNotification() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let now = ISO8601DateFormatter().date(from: "2026-06-15T12:00:00Z")!
        let result = await sut.simulateDebugInsightReceipt(now: now).value

        XCTAssertEqual(result, .scheduled)
        XCTAssertEqual(scheduler.scheduledPayloads.count, 1)
        XCTAssertEqual(scheduler.scheduledPayloads.first?.title, "Your Velvt insight is ready")
        XCTAssertEqual(scheduler.scheduledPayloads.first?.insightDate, "2026-06-15")
        XCTAssertNil(scheduler.scheduledPayloads.first?.doNotDisturbUntil)
    }

    func testDebugSimulationRequestsNotificationPermissionWhenUnknown() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = RequestGrantingPermissionManager()
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(5))

        let now = ISO8601DateFormatter().date(from: "2026-06-15T12:00:00Z")!
        let result = await sut.simulateDebugInsightReceipt(now: now).value

        XCTAssertEqual(result, .scheduled)
        XCTAssertEqual(permissions.requestedPermissions, [.notifications])
        XCTAssertEqual(scheduler.scheduledPayloads.count, 1)
        XCTAssertEqual(scheduler.scheduledPayloads.first?.insightDate, "2026-06-15")
        XCTAssertNil(scheduler.scheduledPayloads.first?.doNotDisturbUntil)
    }

    func testDebugSimulationReportsDeniedNotificationPermission() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .notifications)
        let sut = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissions,
            debounceInterval: .milliseconds(5)
        )

        let result = await sut.simulateDebugInsightReceipt().value

        XCTAssertEqual(result, .permissionDenied)
        XCTAssertTrue(scheduler.scheduledPayloads.isEmpty)
    }

    func testRepeatedDebugSimulationsScheduleSeparateNativeNotifications() async {
        let scheduler = FakeNotificationScheduler()
        let permissions = FakePermissionManager()
        permissions.setStatus(.granted, for: .notifications)
        let sut = NotificationDeliveryCoordinator(scheduler: scheduler, permissionManager: permissions, debounceInterval: .milliseconds(20))

        let now = ISO8601DateFormatter().date(from: "2026-06-15T12:00:00Z")!
        let first = sut.simulateDebugInsightReceipt(now: now)
        let second = sut.simulateDebugInsightReceipt(now: now.addingTimeInterval(1))
        _ = await first.value
        _ = await second.value

        XCTAssertEqual(scheduler.scheduledPayloads.count, 2)
        guard scheduler.scheduledPayloads.count == 2 else { return }
        XCTAssertNotEqual(
            scheduler.scheduledPayloads[0].notificationID,
            scheduler.scheduledPayloads[1].notificationID
        )
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

private final class RequestGrantingPermissionManager: PermissionManagerProtocol {
    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        Just([.accessibility: .unknown, .notifications: status]).eraseToAnyPublisher()
    }

    private(set) var requestedPermissions: [PermissionType] = []
    private var status: PermissionStatus = .unknown

    func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        permission == .notifications ? status : .unknown
    }

    func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        requestedPermissions.append(permission)
        status = .granted
        return status
    }
}

private final class InMemoryScheduledNotificationTracker: ScheduledNotificationTracking {
    private var identifiers = Set<UUID>()

    func contains(_ notificationID: UUID) -> Bool {
        identifiers.contains(notificationID)
    }

    func record(_ notificationID: UUID) {
        identifiers.insert(notificationID)
    }
}

private final class OutcomeNotificationScheduler: NotificationSchedulerProtocol {
    private var outcomes: [Bool]
    private(set) var attemptedPayloads: [NotificationPayload] = []

    init(outcomes: [Bool]) {
        self.outcomes = outcomes
    }

    func schedule(_ payload: NotificationPayload) async -> Bool {
        attemptedPayloads.append(payload)
        return outcomes.isEmpty ? false : outcomes.removeFirst()
    }

    func cancelAll() {}
}
