import Combine
import XCTest
@testable import VelvtMac

final class PermissionModuleTests: XCTestCase {
    private var cancellables: Set<AnyCancellable> = []

    func testPermissionTypeContainsExactlyTheApprovedPermissions() {
        XCTAssertEqual(Set(PermissionType.allCases), [.accessibility, .notifications])
    }

    func testPermissionTypeExtensionCannotExpandTheApprovedPermissionSet() {
        XCTAssertEqual(
            Set(PermissionType.testOnlyAuditedCases),
            Set(PermissionType.allCases)
        )
    }

    func testFakePermissionManagerPublishesInjectedStatusesAndRecordsRequests() async {
        let manager = FakePermissionManager()
        var snapshots: [[PermissionType: PermissionStatus]] = []
        manager.statusPublisher.sink { snapshots.append($0) }.store(in: &cancellables)

        manager.setStatus(.denied, for: .accessibility)
        let requested = await manager.requestPermission(for: .notifications)

        XCTAssertEqual(snapshots.last?[.accessibility], .denied)
        XCTAssertEqual(manager.requestedPermissions, [.notifications])
        XCTAssertEqual(requested, .unknown)
    }

    func testPermissionManagerMapsAllNotificationStatuses() async {
        let notifications = FakeNotificationPermissionClient()
        let manager = PermissionManager(
            accessibilityClient: FakeAccessibilityPermissionClient(isTrusted: false),
            notificationClient: notifications
        )

        for (authorizationStatus, expected) in [
            (NotificationAuthorizationStatus.notDetermined, PermissionStatus.unknown),
            (.authorized, .granted),
            (.provisional, .granted),
            (.ephemeral, .granted),
            (.denied, .denied),
            (.restricted, .restricted)
        ] {
            notifications.status = authorizationStatus
            let actual = await manager.checkStatus(for: .notifications)
            XCTAssertEqual(actual, expected)
        }
    }

    func testPermissionManagerChecksAccessibilityWithoutPrompting() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await manager.checkStatus(for: .accessibility)

        XCTAssertEqual(status, .granted)
        XCTAssertEqual(accessibility.promptValues, [false])
    }

    func testNotificationDenialPreventsSubsequentAuthorizationRequests() async {
        let notifications = FakeNotificationPermissionClient()
        notifications.requestResult = false
        let manager = PermissionManager(
            accessibilityClient: FakeAccessibilityPermissionClient(isTrusted: false),
            notificationClient: notifications
        )

        let first = await manager.requestPermission(for: .notifications)
        let second = await manager.requestPermission(for: .notifications)

        XCTAssertEqual(first, .denied)
        XCTAssertEqual(second, .denied)
        XCTAssertEqual(notifications.requestCallCount, 1)
    }

    func testAccessibilityRequestFromBackgroundThreadRunsSystemClientOnMainThread() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await Task.detached {
            await manager.requestPermission(for: .accessibility)
        }.value

        XCTAssertEqual(status, .granted)
        XCTAssertEqual(accessibility.mainThreadValues, [true])
    }

    func testAccessibilityRequestSkipsPromptWhenAlreadyTrusted() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await manager.requestPermission(for: .accessibility)

        XCTAssertEqual(status, .granted)
        // Only the non-prompting check should run — re-prompting on every
        // launch after access was already granted is exactly the bug being fixed.
        XCTAssertEqual(accessibility.promptValues, [false])
    }

    func testAccessibilityRequestPromptsWhenNotYetTrusted() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: false)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await manager.requestPermission(for: .accessibility)

        XCTAssertEqual(status, .denied)
        // Not yet trusted: the non-prompting check runs first, then falls
        // through to the prompting check so the system dialog can appear.
        XCTAssertEqual(accessibility.promptValues, [false, true])
    }

    func testLaunchRefreshPromptsWhenAccessibilityGrantIsMissing() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: false)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await manager.refreshAccessibilityPermissionOnLaunch()

        XCTAssertEqual(status, .denied)
        XCTAssertEqual(accessibility.promptValues, [false, true])
    }

    func testLaunchRefreshPreservesAnExistingAccessibilityGrant() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient()
        )

        let status = await manager.refreshAccessibilityPermissionOnLaunch()

        XCTAssertEqual(status, .granted)
        XCTAssertEqual(accessibility.promptValues, [false])
    }

    func testAccessibilityRequestPollsForGrantAfterSystemSettingsToggleWhileInactive() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: false)
        let promptPollScheduler = FakePermissionMonitorScheduler()
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient(),
            applicationIsActive: { false },
            monitorScheduler: FakePermissionMonitorScheduler(),
            accessibilityPromptPollScheduler: promptPollScheduler,
            accessibilityPromptPollInterval: 1,
            accessibilityPromptPollLimit: 3
        )
        var observed: [PermissionStatus] = []
        manager.statusPublisher
            .sink { observed.append($0[.accessibility] ?? .unknown) }
            .store(in: &cancellables)

        let status = await manager.requestPermission(for: .accessibility)
        accessibility.isTrusted = true
        promptPollScheduler.fire()

        XCTAssertEqual(status, .denied)
        XCTAssertEqual(observed.last, .granted)
        XCTAssertEqual(promptPollScheduler.startCallCount, 1)
        XCTAssertEqual(promptPollScheduler.stopCallCount, 1)
        XCTAssertEqual(accessibility.promptValues, [false, true, false])
    }

    func testBackgroundedAppSkipsAccessibilityMonitorCycle() {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let scheduler = FakePermissionMonitorScheduler()
        let activityNotifications = NotificationCenter()
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient(),
            applicationIsActive: { false },
            monitorScheduler: scheduler,
            activityNotifications: activityNotifications
        )

        manager.startMonitoring()
        scheduler.fire()

        XCTAssertEqual(scheduler.startCallCount, 0)
        XCTAssertTrue(accessibility.promptValues.isEmpty)
    }

    func testBecomingActiveRechecksAccessibilityImmediatelyWithoutWaitingForFirstTick() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let scheduler = FakePermissionMonitorScheduler()
        let activityNotifications = NotificationCenter()
        let manager = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient(),
            applicationIsActive: { true },
            monitorScheduler: scheduler,
            activityNotifications: activityNotifications
        )
        var statuses: [[PermissionType: PermissionStatus]] = []
        manager.statusPublisher.sink { statuses.append($0) }.store(in: &cancellables)

        manager.startMonitoring()

        // The immediate re-check runs on a detached Task; give it a chance
        // to complete without relying on the (never-fired) periodic timer.
        for _ in 0..<50 where statuses.last?[.accessibility] != .granted {
            await Task.yield()
        }

        XCTAssertEqual(statuses.last?[.accessibility], .granted)
        XCTAssertEqual(scheduler.startCallCount, 1, "periodic monitoring should still be scheduled")
    }

    func testAccessibilityMonitorPausesWhenAppMovesToBackground() {
        var isActive = true
        let scheduler = FakePermissionMonitorScheduler()
        let activityNotifications = NotificationCenter()
        let manager = PermissionManager(
            accessibilityClient: FakeAccessibilityPermissionClient(isTrusted: true),
            notificationClient: FakeNotificationPermissionClient(),
            applicationIsActive: { isActive },
            monitorScheduler: scheduler,
            activityNotifications: activityNotifications
        )

        manager.startMonitoring()
        isActive = false
        activityNotifications.post(name: NSApplication.willResignActiveNotification, object: nil)

        XCTAssertEqual(scheduler.startCallCount, 1)
        XCTAssertEqual(scheduler.stopCallCount, 1)
    }

    func testAccessibilityRevocationDuringMonitorCycleStopsCollectionAndShowsRecovery() async {
        let accessibility = FakeAccessibilityPermissionClient(isTrusted: true)
        let scheduler = FakePermissionMonitorScheduler()
        let permissions = PermissionManager(
            accessibilityClient: accessibility,
            notificationClient: FakeNotificationPermissionClient(),
            applicationIsActive: { true },
            monitorScheduler: scheduler
        )
        let collection = RecordingCollectionAgent()
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissions,
            collectionAgent: collection
        )
        let presentation = PermissionPresentationModel(
            permissionManager: permissions,
            onboardingStateStore: InMemoryOnboardingStateStore(hasCompletedOnboarding: true)
        )
        let recoveryShown = expectation(description: "Recovery appears after monitor cycle")
        presentation.$statuses
            .dropFirst()
            .sink { statuses in
                if statuses[.accessibility] == .denied {
                    recoveryShown.fulfill()
                }
            }
            .store(in: &cancellables)

        coordinator.start()
        permissions.startMonitoring()
        _ = await permissions.checkStatus(for: .accessibility)
        accessibility.isTrusted = false
        scheduler.fire()
        await fulfillment(of: [recoveryShown], timeout: 1)

        XCTAssertEqual(collection.startCallCount, 1)
        XCTAssertEqual(collection.stopCallCount, 1)
        XCTAssertTrue(presentation.showsAccessibilityRecovery)
    }

    func testCollectionCoordinatorStopsOnAccessibilityDenialAndStartsOnRegrant() {
        let permissions = FakePermissionManager()
        let collection = RecordingCollectionAgent()
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissions,
            collectionAgent: collection
        )
        var statuses: [PermissionCollectionStatus] = []
        coordinator.statusPublisher.sink { statuses.append($0) }.store(in: &cancellables)

        coordinator.start()
        permissions.setStatus(.denied, for: .accessibility)
        permissions.setStatus(.granted, for: .accessibility)

        XCTAssertEqual(collection.stopCallCount, 1)
        XCTAssertEqual(collection.startCallCount, 1)
        XCTAssertEqual(statuses.last, .collecting)
    }

    func testCollectionContinuesWhenServiceDisconnectedAndOfflineCollectionEnabled() {
        let permissions = FakePermissionManager()
        let collection = RecordingCollectionAgent()
        let connection = CurrentValueSubject<ConnectionStatus, Never>(.connected)
        let settings = CollectionSettingsModel(
            defaults: UserDefaults(suiteName: "offline.enabled.\(UUID().uuidString)")!
        )
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissions,
            collectionAgent: collection,
            connectionStatus: connection.eraseToAnyPublisher(),
            collectionSettings: settings
        )

        coordinator.start()
        permissions.setStatus(.granted, for: .accessibility)
        connection.send(.disconnected)

        XCTAssertEqual(collection.startCallCount, 1)
        XCTAssertEqual(collection.stopCallCount, 0)
    }

    func testCollectionPausesWhenServiceDisconnectedAndOfflineCollectionDisabled() {
        let permissions = FakePermissionManager()
        let collection = RecordingCollectionAgent()
        let connection = CurrentValueSubject<ConnectionStatus, Never>(.connected)
        let settings = CollectionSettingsModel(
            defaults: UserDefaults(suiteName: "offline.disabled.\(UUID().uuidString)")!
        )
        settings.offlineEventCollectionEnabled = false
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissions,
            collectionAgent: collection,
            connectionStatus: connection.eraseToAnyPublisher(),
            collectionSettings: settings
        )

        coordinator.start()
        permissions.setStatus(.granted, for: .accessibility)
        connection.send(.disconnected)

        XCTAssertEqual(collection.startCallCount, 1)
        XCTAssertEqual(collection.stopCallCount, 1)
    }

    func testCollectionCoordinatorStopsOnRestrictedAccessibility() {
        let permissions = FakePermissionManager()
        let collection = RecordingCollectionAgent()
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissions,
            collectionAgent: collection
        )
        var statuses: [PermissionCollectionStatus] = []
        coordinator.statusPublisher.sink { statuses.append($0) }.store(in: &cancellables)

        coordinator.start()
        permissions.setStatus(.restricted, for: .accessibility)

        XCTAssertEqual(collection.stopCallCount, 1)
        XCTAssertEqual(statuses.last, .permissionRequired)
    }

    func testFakePermissionManagerCanPublishEveryPermissionStatus() {
        let manager = FakePermissionManager()
        var observed: [PermissionStatus] = []
        manager.statusPublisher
            .sink { observed.append($0[.accessibility] ?? .unknown) }
            .store(in: &cancellables)

        manager.setStatus(.granted, for: .accessibility)
        manager.setStatus(.denied, for: .accessibility)
        manager.setStatus(.restricted, for: .accessibility)

        XCTAssertEqual(observed, [.unknown, .granted, .denied, .restricted])
    }

    func testPresentationShowsOnboardingOnlyOnFirstLaunch() {
        let store = InMemoryOnboardingStateStore()
        let firstLaunch = PermissionPresentationModel(
            permissionManager: FakePermissionManager(),
            onboardingStateStore: store
        )

        XCTAssertTrue(firstLaunch.showsOnboarding)

        firstLaunch.completeOnboarding()
        let secondLaunch = PermissionPresentationModel(
            permissionManager: FakePermissionManager(),
            onboardingStateStore: store
        )

        XCTAssertFalse(secondLaunch.showsOnboarding)
    }

    func testPresentationShowsAccessibilityRecoveryAfterDenial() {
        let permissions = FakePermissionManager()
        let presentation = PermissionPresentationModel(
            permissionManager: permissions,
            onboardingStateStore: InMemoryOnboardingStateStore(hasCompletedOnboarding: true)
        )

        permissions.setStatus(.denied, for: .accessibility)

        XCTAssertTrue(presentation.showsAccessibilityRecovery)
    }

    @MainActor
    func testOnboardingRequestsAccessibilityThenNotificationsBeforeCompleting() async {
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .accessibility)
        permissions.setStatus(.denied, for: .notifications)
        var completionCount = 0
        let model = PermissionOnboardingModel(permissionManager: permissions) {
            completionCount += 1
        }

        await model.requestCurrentPermission()
        XCTAssertEqual(model.step, .notifications)
        XCTAssertEqual(completionCount, 0)
        XCTAssertFalse(model.isComplete)

        await model.requestCurrentPermission()
        XCTAssertEqual(permissions.requestedPermissions, [.accessibility, .notifications])
        XCTAssertEqual(completionCount, 1)
        XCTAssertTrue(model.isComplete)
    }

    @MainActor
    func testBothPermissionsDeniedOnFirstLaunchStillShowsMenuBarRecoveryState() async {
        let permissions = FakePermissionManager()
        permissions.setStatus(.denied, for: .accessibility)
        permissions.setStatus(.denied, for: .notifications)
        let store = InMemoryOnboardingStateStore()
        let presentation = PermissionPresentationModel(
            permissionManager: permissions,
            onboardingStateStore: store
        )
        let model = PermissionOnboardingModel(permissionManager: permissions) {
            presentation.completeOnboarding()
        }

        await model.requestCurrentPermission()
        await model.requestCurrentPermission()

        XCTAssertFalse(presentation.showsOnboarding)
        XCTAssertTrue(presentation.showsAccessibilityRecovery)
        XCTAssertEqual(presentation.statuses[.notifications], .denied)
    }
}

private extension PermissionType {
    // Swift extensions cannot add enum cases; this verifies extension helpers
    // cannot expand the compile-time permission allowlist.
    static var testOnlyAuditedCases: [PermissionType] {
        [.accessibility, .notifications]
    }
}

private final class FakeAccessibilityPermissionClient: AccessibilityPermissionClient {
    var isTrusted: Bool
    private(set) var promptValues: [Bool] = []
    private(set) var mainThreadValues: [Bool] = []

    init(isTrusted: Bool) {
        self.isTrusted = isTrusted
    }

    func isProcessTrusted(prompt: Bool) -> Bool {
        promptValues.append(prompt)
        mainThreadValues.append(Thread.isMainThread)
        return isTrusted
    }
}

private final class FakeNotificationPermissionClient: NotificationPermissionClient {
    var status = NotificationAuthorizationStatus.notDetermined
    var requestResult = true
    private(set) var requestCallCount = 0

    func authorizationStatus() async -> NotificationAuthorizationStatus {
        status
    }

    func requestAuthorization() async throws -> Bool {
        requestCallCount += 1
        status = requestResult ? .authorized : .denied
        return requestResult
    }
}

private final class RecordingCollectionAgent: CollectionAgentProtocol {
    var status: AnyPublisher<CollectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private let statusSubject = CurrentValueSubject<CollectionStatus, Never>(.idle)
    private(set) var startCallCount = 0
    private(set) var stopCallCount = 0

    func start() throws {
        startCallCount += 1
        statusSubject.send(.running)
    }

    func stop() {
        stopCallCount += 1
        statusSubject.send(.idle)
    }
}

private final class FakePermissionMonitorScheduler: PermissionMonitorScheduling {
    private var handler: (() -> Void)?
    private(set) var startCallCount = 0
    private(set) var stopCallCount = 0

    func start(interval: TimeInterval, handler: @escaping () -> Void) {
        startCallCount += 1
        self.handler = handler
    }

    func stop() {
        stopCallCount += 1
        handler = nil
    }

    func fire() {
        handler?()
    }
}
