import XCTest
@testable import VelvtMac

final class AppModuleTests: XCTestCase {
    func testScaffoldTargetIsWired() {
        XCTAssertTrue(true)
    }

    @MainActor
    func testDelegateUsesItsConfiguredIPCClientBeforeLaunching() {
        let client = FakeIPCClient()
        let permissionManager = PermissionManager(
            accessibilityClient: TestAccessibilityPermissionClient(),
            notificationClient: TestNotificationPermissionClient(),
            applicationIsActive: { false }
        )
        let delegate = AppDelegate(
            permissionManager: permissionManager,
            accountStateManager: AccountStateManager(keychain: FakeKeychain()),
            ipcClientFactory: { client }
        )

        XCTAssertTrue((delegate.ipcClient as AnyObject) === client)
    }

    @MainActor
    func testAuthGatedCollectionStartsOnlyWhenLoggedIn() async {
        let recorder = CollectionGateRecorder()
        let controller = AuthGatedCollectionController(
            startCollection: { recorder.start() },
            stopCollection: { recorder.stop() }
        )

        controller.apply(accountState: .loggedOut)
        var counts = recorder.counts()
        XCTAssertEqual(counts.starts, 0)
        XCTAssertEqual(counts.stops, 0)

        controller.apply(accountState: .loggedIn(userId: "user-1"))
        counts = recorder.counts()
        XCTAssertEqual(counts.starts, 1)
        XCTAssertEqual(counts.stops, 0)

        controller.apply(accountState: .loggedIn(userId: "user-1"))
        counts = recorder.counts()
        XCTAssertEqual(counts.starts, 1)
    }

    @MainActor
    func testAuthGatedCollectionStopsWhenLeavingLoggedInState() async {
        let recorder = CollectionGateRecorder()
        let controller = AuthGatedCollectionController(
            startCollection: { recorder.start() },
            stopCollection: { recorder.stop() }
        )

        controller.apply(accountState: .loggedIn(userId: "user-1"))
        controller.apply(accountState: .loggingOut)

        var counts = recorder.counts()
        XCTAssertEqual(counts.starts, 1)
        XCTAssertEqual(counts.stops, 1)

        controller.apply(accountState: .loggedOut)
        counts = recorder.counts()
        XCTAssertEqual(counts.stops, 1)
    }
}

private final class TestAccessibilityPermissionClient: AccessibilityPermissionClient {
    func isProcessTrusted(prompt: Bool) -> Bool { false }
}

private final class TestNotificationPermissionClient: NotificationPermissionClient {
    func authorizationStatus() async -> NotificationAuthorizationStatus { .notDetermined }
    func requestAuthorization() async throws -> Bool { false }
}

private final class CollectionGateRecorder {
    private var startCount = 0
    private var stopCount = 0

    func start() {
        startCount += 1
    }

    func stop() {
        stopCount += 1
    }

    func counts() -> (starts: Int, stops: Int) {
        (startCount, stopCount)
    }
}
