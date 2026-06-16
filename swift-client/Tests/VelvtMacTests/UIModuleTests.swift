import Combine
import XCTest
@testable import VelvtMac

// MARK: - OnboardingCoordinator sequencing tests

@MainActor
final class OnboardingCoordinatorTests: XCTestCase {

    func testInitialStepIsWelcome() {
        let sut = makeCoordinator()
        XCTAssertTrue(sut.path.isEmpty, "Path should be empty (Welcome is the NavigationStack root)")
    }

    func testAdvanceFromWelcomePushesPermissions() {
        let sut = makeCoordinator()
        sut.advanceFromWelcome()
        XCTAssertEqual(sut.path, [.permissions])
    }

    func testAdvanceFromPermissionsPushesAuth() {
        let sut = makeCoordinator()
        sut.advanceFromWelcome()
        sut.advanceFromPermissions()
        XCTAssertEqual(sut.path, [.permissions, .auth])
    }

    func testAuthDidCompletePushesComplete() {
        let sut = makeCoordinator()
        sut.advanceFromWelcome()
        sut.advanceFromPermissions()
        sut.authDidComplete()
        XCTAssertEqual(sut.path, [.permissions, .auth, .complete])
    }

    func testCannotReachAuthWithoutGoingThroughPermissions() {
        let sut = makeCoordinator()
        sut.advanceFromWelcome()
        XCTAssertEqual(sut.path.last, .permissions, "First step after welcome must be permissions")
        XCTAssertFalse(sut.path.contains(.auth), "Auth must not appear without permissions step first")
    }

    func testIAlreadyHaveAnAccountSwitchesAuthModeNotNavigationPath() {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let manager = AccountStateManager(keychain: keychain)
        let authVM = AuthViewModel(accountStateManager: manager, ipcClient: client)
        let sut = makeCoordinator(authViewModel: authVM)

        sut.advanceFromWelcome()
        sut.advanceFromPermissions()

        // "I already have an account" toggles mode within AuthView, not the nav path.
        authVM.toggleAuthMode()
        XCTAssertEqual(authVM.authMode, .logIn, "Toggling mode should switch to logIn")
        XCTAssertEqual(sut.path.last, .auth, "Navigation path must not change when toggling mode")
    }

    func testSkipToAuthSetsPathDirectlyToAuth() {
        let sut = makeCoordinator()
        sut.skipToAuth()
        XCTAssertEqual(sut.path, [.auth])
    }

    func testFinishOnboardingCallsCompletionClosure() {
        var completionCalled = false
        let sut = makeCoordinator(onComplete: { completionCalled = true })
        sut.finishOnboarding()
        XCTAssertTrue(completionCalled)
    }

    // MARK: - Helpers

    private func makeCoordinator(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient(),
        authViewModel: AuthViewModel? = nil,
        onComplete: @escaping () -> Void = {}
    ) -> OnboardingCoordinator {
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let vm = authViewModel ?? AuthViewModel(accountStateManager: manager, ipcClient: client)
        return OnboardingCoordinator(
            permissionManager: FakePermissionManager(),
            accountStateManager: manager,
            authViewModel: vm,
            onComplete: onComplete
        )
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
