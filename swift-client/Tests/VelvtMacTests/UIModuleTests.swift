import Combine
import XCTest
@testable import VelvtMac

// MARK: - Menu bar navigation tests

@MainActor
final class MenuBarNavigationTests: XCTestCase {

    func testConnectionPresentationUsesRequestedLabelsAndColors() {
        XCTAssertEqual(PopoverConnectionPresentation(status: .connected).label, "Connected")
        XCTAssertEqual(PopoverConnectionPresentation(status: .connecting).label, "Connecting")
        XCTAssertEqual(PopoverConnectionPresentation(status: .disconnected).label, "Disconnected")
    }

    func testServiceConnectionStatusModelReflectsSocketUpdates() async {
        let client = FakeIPCClient()
        let model = ServiceConnectionStatusModel(connectionStatus: client.connectionStatus)

        XCTAssertEqual(model.status, .disconnected)

        client.setConnectionStatus(.reconnecting(attempt: 2, nextRetryIn: 1))
        await Task.yield()

        XCTAssertEqual(model.status, .reconnecting(attempt: 2, nextRetryIn: 1))
    }

    func testSettingsNavigationMovesForwardAndBack() {
        var navigator = MenuBarPopoverNavigator()

        navigator.showSettings()
        XCTAssertEqual(navigator.route, .settings)
        XCTAssertEqual(navigator.direction, .forward)

        navigator.goBack()
        XCTAssertEqual(navigator.route, .main)
        XCTAssertEqual(navigator.direction, .backward)
    }

    func testSettingsBackNavigationAlwaysReturnsToMain() {
        var navigator = MenuBarPopoverNavigator()
        navigator.showSettings()
        navigator.goBack()
        XCTAssertEqual(navigator.route, .main)
        XCTAssertEqual(navigator.direction, .backward)
    }

}

// MARK: - DeviceRevoked integration tests

@MainActor
final class DeviceRevokedUITests: XCTestCase {

    func testDeviceRevokedPushSetsIsDeviceRevokedFlag() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)

        XCTAssertFalse(manager.isDeviceRevoked)

        let flagSet = expectation(description: "isDeviceRevoked set to true")
        var cancellable: AnyCancellable?
        cancellable = manager.$isDeviceRevoked.dropFirst().sink { revoked in
            if revoked { flagSet.fulfill(); cancellable?.cancel() }
        }

        client.inject(.deviceRevoked(DeviceRevoked(message: "Your device was revoked")))

        await fulfillment(of: [flagSet], timeout: 1)
        XCTAssertTrue(manager.isDeviceRevoked)
        XCTAssertEqual(manager.accountState, .loggedOut)
    }

    func testClearingFlagAllowsReauth() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)

        let flagSet = expectation(description: "isDeviceRevoked set")
        var cancellable: AnyCancellable?
        cancellable = manager.$isDeviceRevoked.dropFirst().sink { if $0 { flagSet.fulfill(); cancellable?.cancel() } }
        client.inject(.deviceRevoked(DeviceRevoked(message: "revoked")))
        await fulfillment(of: [flagSet], timeout: 1)

        manager.clearDeviceRevokedFlag()
        XCTAssertFalse(manager.isDeviceRevoked, "Flag must be cleared so re-auth screen can appear")
    }
}
