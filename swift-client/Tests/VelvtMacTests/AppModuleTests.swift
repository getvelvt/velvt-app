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
}

private final class TestAccessibilityPermissionClient: AccessibilityPermissionClient {
    func isProcessTrusted(prompt: Bool) -> Bool { false }
}

private final class TestNotificationPermissionClient: NotificationPermissionClient {
    func authorizationStatus() async -> NotificationAuthorizationStatus { .notDetermined }
    func requestAuthorization() async throws -> Bool { false }
}
