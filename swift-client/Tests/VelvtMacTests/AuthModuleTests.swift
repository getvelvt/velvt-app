import Combine
import XCTest
@testable import VelvtMac

// MARK: - AccountState transition tests

@MainActor
final class AccountStateManagerTests: XCTestCase {

    // MARK: Valid transitions

    func testLoggedOutToLoggingIn() {
        let sut = makeManager()
        sut.transition(to: .loggingIn)
        XCTAssertEqual(sut.accountState, .loggingIn)
    }

    func testLoggingInToLoggedInViaAuthSuccess() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)
        sut.transition(to: .loggingIn)

        let settled = expectation(description: "loggedIn")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedIn(let uid) = state, uid == "u123" {
                settled.fulfill()
                cancellable?.cancel()
            }
        }

        client.inject(.authSuccess(AuthSuccess(
            userId: "u123",
            accessToken: "tok-access",
            refreshToken: "tok-refresh",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        await fulfillment(of: [settled], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u123"))
        XCTAssertEqual(keychain.storedValue(for: .accessToken), "tok-access")
        XCTAssertEqual(keychain.storedValue(for: .refreshToken), "tok-refresh")
        XCTAssertEqual(keychain.storedValue(for: .userId), "u123")
    }

    func testLoggingInToLoggedOutViaAuthFailure() async {
        let client = FakeIPCClient()
        let sut = makeManager(client: client)
        sut.transition(to: .loggingIn)

        let settled = expectation(description: "loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { settled.fulfill(); cancellable?.cancel() }
        }

        client.inject(.authFailure(AuthFailure(code: .invalidCredentials, message: "Bad creds")))

        await fulfillment(of: [settled], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
    }

    func testLoggedInToLoggingOut() {
        let sut = makeLoggedInManager()
        sut.transition(to: .loggingOut)
        XCTAssertEqual(sut.accountState, .loggingOut)
    }

    func testLoggingOutToLoggedOut() {
        let sut = makeLoggedInManager()
        sut.transition(to: .loggingOut)
        sut.transition(to: .loggedOut)
        XCTAssertEqual(sut.accountState, .loggedOut)
    }

    func testLoggedInToPendingErasure() {
        let sut = makeLoggedInManager()
        sut.transition(to: .pendingErasure)
        XCTAssertEqual(sut.accountState, .pendingErasure)
    }

    func testPendingErasureToLoggedOutViaAck() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain, client: client)
        sut.transition(to: .pendingErasure)

        let settled = expectation(description: "loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { settled.fulfill(); cancellable?.cancel() }
        }

        client.inject(.accountDeletionAccepted)

        await fulfillment(of: [settled], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
        XCTAssertTrue(keychain.isEmpty)
    }

    // MARK: Invalid transitions — must be silently rejected

    func testLoggedInToLoggingInIsRejected() {
        let sut = makeLoggedInManager()
        sut.transition(to: .loggingIn)
        if case .loggedIn = sut.accountState { /* expected */ } else {
            XCTFail("Expected .loggedIn to be preserved; got \(sut.accountState)")
        }
    }

    func testLoggedOutToPendingErasureIsRejected() {
        let sut = makeManager()
        sut.transition(to: .pendingErasure)
        XCTAssertEqual(sut.accountState, .loggedOut)
    }

    func testLoggingOutToPendingErasureIsRejected() {
        let sut = makeLoggedInManager()
        sut.transition(to: .loggingOut)
        sut.transition(to: .pendingErasure)
        XCTAssertEqual(sut.accountState, .loggingOut)
    }

    func testLoggedInToLoggedInIsRejected() {
        let sut = makeLoggedInManager()
        sut.transition(to: .loggedIn(userId: "u2"))
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u1"))
    }

    // MARK: Server push: NeedsReauth

    func testNeedsReauthClearsKeychainAndTransitionsToLoggedOut() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain, client: client)

        let settled = expectation(description: "loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { settled.fulfill(); cancellable?.cancel() }
        }

        client.inject(.needsReauth(NeedsReauth(reason: "token_expired")))

        await fulfillment(of: [settled], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
        XCTAssertTrue(keychain.isEmpty)
    }

    // MARK: Server push: DeviceRevoked

    func testDeviceRevokedClearsKeychainAndSetsFlag() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain, client: client)

        let flagSet = expectation(description: "isDeviceRevoked")
        var cancellable: AnyCancellable?
        cancellable = sut.$isDeviceRevoked.dropFirst().sink { revoked in
            if revoked { flagSet.fulfill(); cancellable?.cancel() }
        }

        client.inject(.deviceRevoked(DeviceRevoked(message: "Device was revoked")))

        await fulfillment(of: [flagSet], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
        XCTAssertTrue(sut.isDeviceRevoked)
        XCTAssertTrue(keychain.isEmpty)
    }

    func testClearDeviceRevokedFlagResetsFlag() async {
        let client = FakeIPCClient()
        let sut = makeLoggedInManager(client: client)

        let flagSet = expectation(description: "flagSet")
        var cancellable: AnyCancellable?
        cancellable = sut.$isDeviceRevoked.dropFirst().sink { if $0 { flagSet.fulfill(); cancellable?.cancel() } }
        client.inject(.deviceRevoked(DeviceRevoked(message: "revoked")))
        await fulfillment(of: [flagSet], timeout: 1)

        sut.clearDeviceRevokedFlag()
        XCTAssertFalse(sut.isDeviceRevoked)
    }

    // MARK: Logout

    func testLogOutClearsKeychainAndTransitionsToLoggedOut() {
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain)
        sut.logOut()
        XCTAssertEqual(sut.accountState, .loggedOut)
        XCTAssertTrue(keychain.isEmpty)
    }

    // MARK: State restoration on init

    func testInitRestoresLoggedInStateFromKeychain() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u42", for: .userId)
        let sut = AccountStateManager(keychain: keychain)
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u42"))
    }

    func testInitIsLoggedOutWhenKeychainEmpty() {
        XCTAssertEqual(AccountStateManager(keychain: FakeKeychain()).accountState, .loggedOut)
    }

    // MARK: cancelPendingErasure

    func testCancelPendingErasureRevertsToLoggedIn() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u99", for: .userId)
        let sut = AccountStateManager(keychain: keychain)
        sut.transition(to: .pendingErasure)
        sut.cancelPendingErasure()
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u99"))
    }

    // MARK: IPC stream-end behaviour

    func testLoggingInRevertsToLoggedOutWhenIPCStreamEnds() async {
        let client = FakeIPCClient()
        let sut = makeManager(client: client)
        sut.transition(to: .loggingIn)

        let reverted = expectation(description: "reverted to loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { reverted.fulfill(); cancellable?.cancel() }
        }

        client.closeStream()

        await fulfillment(of: [reverted], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
    }

    func testLoggingOutRevertsToLoggedOutWhenIPCStreamEnds() async {
        let client = FakeIPCClient()
        let sut = makeLoggedInManager(client: client)
        sut.transition(to: .loggingOut)

        let reverted = expectation(description: "reverted to loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { reverted.fulfill(); cancellable?.cancel() }
        }

        client.closeStream()

        await fulfillment(of: [reverted], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedOut)
    }

    func testPendingErasureIsNotRevertedWhenIPCStreamEnds() async throws {
        // pendingErasure must survive a disconnect: the Rust service may have
        // received the request and the sentinel must persist until confirmed.
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain, client: client)
        sut.transition(to: .pendingErasure)

        // Give the listener task a chance to detect stream end.
        client.closeStream()
        try await Task.sleep(nanoseconds: 100_000_000)

        XCTAssertEqual(sut.accountState, .pendingErasure, "pendingErasure must not be auto-reverted on disconnect")
    }

    // MARK: pendingErasure persistence across relaunches

    func testInitRestoresPendingErasureWhenFlagAndUserIdBothPresent() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u5", for: .userId)
        try keychain.store(token: "1", for: .pendingDeletion)

        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountState, .pendingErasure)
    }

    func testInitIsLoggedInWhenUserIdPresentButNoDeletionFlag() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u5", for: .userId)

        XCTAssertEqual(AccountStateManager(keychain: keychain).accountState, .loggedIn(userId: "u5"))
    }

    func testTransitionToPendingErasureWritesDeletionFlag() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let sut = AccountStateManager(keychain: keychain)

        sut.transition(to: .pendingErasure)

        XCTAssertNotNil(keychain.storedValue(for: .pendingDeletion), "sentinel must be written")
    }

    func testCancelPendingErasureClearsDeletionFlag() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        try keychain.store(token: "1", for: .pendingDeletion)
        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountState, .pendingErasure)
        sut.cancelPendingErasure()

        XCTAssertNil(keychain.storedValue(for: .pendingDeletion), "sentinel must be cleared on cancel")
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u1"))
    }

    func testAccountDeletionAcceptedClearsDeletionFlagViaDeleteAll() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let sut = makeLoggedInManager(keychain: keychain, client: client)
        sut.transition(to: .pendingErasure)
        XCTAssertNotNil(keychain.storedValue(for: .pendingDeletion))

        let settled = expectation(description: "loggedOut")
        var cancellable: AnyCancellable?
        cancellable = sut.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { settled.fulfill(); cancellable?.cancel() }
        }
        client.inject(.accountDeletionAccepted)
        await fulfillment(of: [settled], timeout: 1)

        XCTAssertTrue(keychain.isEmpty, "deleteAll must clear the deletion sentinel")
    }

    func testRelaunchInPendingErasureBlocksNormalTransitionsToLoggedIn() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        try keychain.store(token: "1", for: .pendingDeletion)
        let sut = AccountStateManager(keychain: keychain)

        // A transition to loggedIn is not a valid move from pendingErasure.
        sut.transition(to: .loggedIn(userId: "u1"))

        XCTAssertEqual(sut.accountState, .pendingErasure, "normal use must be blocked during pending erasure")
    }

    // MARK: Helpers

    private func makeManager(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient()
    ) -> AccountStateManager {
        let mgr = AccountStateManager(keychain: keychain)
        mgr.startListening(to: client)
        return mgr
    }

    private func makeLoggedInManager(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient()
    ) -> AccountStateManager {
        // Seed the keychain before constructing so init() restores loggedIn.
        try? keychain.store(token: "u1", for: .userId)
        let mgr = AccountStateManager(keychain: keychain)
        mgr.startListening(to: client)
        return mgr
    }
}

// MARK: - FakeKeychain tests

final class FakeKeychainTests: XCTestCase {
    func testStoreAndLoad() throws {
        let sut = FakeKeychain()
        try sut.store(token: "abc", for: .accessToken)
        XCTAssertEqual(try sut.load(for: .accessToken), "abc")
    }

    func testLoadMissingKeyThrows() {
        let sut = FakeKeychain()
        XCTAssertThrowsError(try sut.load(for: .refreshToken)) { error in
            XCTAssertEqual(error as? AuthError, .keychainItemNotFound)
        }
    }

    func testDeleteRemovesValue() throws {
        let sut = FakeKeychain()
        try sut.store(token: "x", for: .userId)
        try sut.delete(for: .userId)
        XCTAssertNil(sut.storedValue(for: .userId))
    }

    func testDeleteAllClearsAll() throws {
        let sut = FakeKeychain()
        try sut.store(token: "a", for: .accessToken)
        try sut.store(token: "b", for: .refreshToken)
        try sut.store(token: "c", for: .userId)
        sut.deleteAll()
        XCTAssertTrue(sut.isEmpty)
    }

    func testShouldThrowOnStoreInjection() {
        let sut = FakeKeychain()
        sut.shouldThrowOnStore = .keychainItemNotFound
        XCTAssertThrowsError(try sut.store(token: "x", for: .accessToken))
    }

    func testOverwriteExistingKey() throws {
        let sut = FakeKeychain()
        try sut.store(token: "v1", for: .accessToken)
        try sut.store(token: "v2", for: .accessToken)
        XCTAssertEqual(try sut.load(for: .accessToken), "v2")
    }
}

// MARK: - AuthViewModel tests

@MainActor
final class AuthViewModelTests: XCTestCase {

    // MARK: signUp success

    func testSignUpSuccessTransitionsToLoggedInAndStoresTokens() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)
        sut.email = "user@example.com"
        sut.password = "secret"

        let loggedIn = expectation(description: "loggedIn")
        var cancellable: AnyCancellable?
        cancellable = manager.$accountState.dropFirst().sink { state in
            if case .loggedIn = state { loggedIn.fulfill(); cancellable?.cancel() }
        }

        // signUp() sends the IPC message and returns; response arrives asynchronously.
        await sut.signUp()
        client.inject(.authSuccess(AuthSuccess(
            userId: "u-001",
            accessToken: "at-x",
            refreshToken: "rt-x",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        await fulfillment(of: [loggedIn], timeout: 2)

        XCTAssertEqual(manager.accountState, .loggedIn(userId: "u-001"))
        XCTAssertEqual(keychain.storedValue(for: .userId), "u-001")
        XCTAssertEqual(keychain.storedValue(for: .accessToken), "at-x")
        XCTAssertFalse(sut.isLoading)
        XCTAssertNil(sut.errorMessage)
    }

    // MARK: logIn failure

    func testLogInFailureInvalidCredentialsSurfacesError() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)
        sut.email = "user@example.com"
        sut.password = "wrong"

        let errorSet = expectation(description: "errorMessage")
        var cancellable: AnyCancellable?
        cancellable = sut.$errorMessage.compactMap { $0 }.sink { _ in
            errorSet.fulfill(); cancellable?.cancel()
        }

        await sut.logIn()
        client.inject(.authFailure(AuthFailure(code: .invalidCredentials, message: "Invalid email or password")))

        await fulfillment(of: [errorSet], timeout: 2)

        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertEqual(sut.errorMessage, "Invalid email or password")
        XCTAssertFalse(sut.isLoading)
    }

    // MARK: Logout

    func testLogOutClearsKeychainAndTransitionsToLoggedOut() throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        try keychain.store(token: "at", for: .accessToken)
        try keychain.store(token: "rt", for: .refreshToken)
        let manager = AccountStateManager(keychain: keychain)
        let client = FakeIPCClient()
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)

        sut.logOut()

        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertTrue(keychain.isEmpty)
    }

    func testLogOutSendsLogOutIPCMessage() async throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let manager = AccountStateManager(keychain: keychain)
        let client = FakeIPCClient()
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)

        sut.logOut()

        // The Task inside logOut() is fire-and-forget; yield to let it run.
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertTrue(client.sentMessages.contains(.logOut))
    }

    // MARK: Account deletion

    func testDeleteAccountShowsConfirmationDialog() {
        let sut = makeViewModel()
        XCTAssertFalse(sut.showDeleteConfirmation)
        sut.requestAccountDeletion()
        XCTAssertTrue(sut.showDeleteConfirmation)
    }

    func testCancelAccountDeletionHidesDialog() {
        let sut = makeViewModel()
        sut.requestAccountDeletion()
        sut.cancelAccountDeletion()
        XCTAssertFalse(sut.showDeleteConfirmation)
    }

    // MARK: Auth mode toggle

    func testToggleAuthModeSwitchesBetweenSignUpAndLogIn() {
        let sut = makeViewModel()
        XCTAssertEqual(sut.authMode, .signUp)
        sut.toggleAuthMode()
        XCTAssertEqual(sut.authMode, .logIn)
        sut.toggleAuthMode()
        XCTAssertEqual(sut.authMode, .signUp)
    }

    func testToggleAuthModeClearsErrorMessage() {
        let sut = makeViewModel()
        sut.toggleAuthMode()
        XCTAssertNil(sut.errorMessage)
    }

    // MARK: Client-side validation (empty fields)

    func testSignUpBlockedWhenEmailEmpty() async {
        let (sut, manager, client) = makeViewModelWithDependencies()
        sut.email = ""
        sut.password = "secret"
        await sut.signUp()
        XCTAssertEqual(manager.accountState, .loggedOut, "must not leave loggedOut")
        XCTAssertFalse(client.sentMessages.contains(where: { if case .signUp = $0 { return true }; return false }))
        XCTAssertNotNil(sut.errorMessage)
        XCTAssertFalse(sut.isLoading)
    }

    func testSignUpBlockedWhenEmailIsWhitespaceOnly() async {
        let (sut, manager, _) = makeViewModelWithDependencies()
        sut.email = "   "
        sut.password = "secret"
        await sut.signUp()
        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertNotNil(sut.errorMessage)
    }

    func testSignUpBlockedWhenPasswordEmpty() async {
        let (sut, manager, _) = makeViewModelWithDependencies()
        sut.email = "user@example.com"
        sut.password = ""
        await sut.signUp()
        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertNotNil(sut.errorMessage)
    }

    func testLogInBlockedWhenEmailEmpty() async {
        let (sut, manager, _) = makeViewModelWithDependencies()
        sut.email = ""
        sut.password = "secret"
        await sut.logIn()
        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertNotNil(sut.errorMessage)
    }

    func testSignUpDoesNotSendWhenAuthenticationIsAlreadyInProgress() async {
        let (sut, manager, client) = makeViewModelWithDependencies()
        sut.email = "user@example.com"
        sut.password = "secret"
        manager.transition(to: .loggingIn)

        await sut.signUp()

        XCTAssertEqual(manager.accountState, .loggingIn)
        XCTAssertFalse(client.sentMessages.contains(where: { if case .signUp = $0 { return true }; return false }))
        XCTAssertEqual(sut.errorMessage, "Authentication is already in progress.")
        XCTAssertFalse(sut.isLoading)
    }

    // MARK: IPC disconnect mid-login

    func testIPCDisconnectMidLoginShowsConnectionLostError() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)
        sut.email = "user@example.com"
        sut.password = "secret"

        let reverted = expectation(description: "state reverted to loggedOut")
        var cancellable: AnyCancellable?
        cancellable = manager.$accountState.dropFirst().sink { state in
            if case .loggedOut = state { reverted.fulfill(); cancellable?.cancel() }
        }

        await sut.signUp()           // transitions to .loggingIn, sends IPC
        client.closeStream()         // simulate hard disconnect — authSuccess never arrives

        await fulfillment(of: [reverted], timeout: 2)

        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertFalse(sut.isLoading)
        XCTAssertNotNil(sut.errorMessage, "connection-lost error must be surfaced")
        XCTAssertEqual(sut.errorMessage, "Connection lost. Please try again.")
    }

    // MARK: deleteAccount IPC send failure

    func testDeleteAccountIPCSendFailureRevertsToLoggedInAndShowsRetryMessage() async throws {
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let client = FakeIPCClient()
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)

        client.shouldThrowOnSend = IPCError.notConnected

        sut.requestAccountDeletion()
        XCTAssertTrue(sut.showDeleteConfirmation)

        await sut.confirmAccountDeletion()

        // After a failed send the state must return to loggedIn — not pendingErasure.
        XCTAssertEqual(manager.accountState, .loggedIn(userId: "u1"),
                       "state must revert when send fails")
        XCTAssertNotNil(sut.errorMessage, "retry message must be shown")
        XCTAssertFalse(sut.showDeleteConfirmation)
        // Keychain must be intact — no data lost due to a failed request.
        XCTAssertNotNil(keychain.storedValue(for: .userId))
        // Deletion sentinel must be cleared since erasure did not proceed.
        XCTAssertNil(keychain.storedValue(for: .pendingDeletion))
    }

    // MARK: Helpers

    private func makeViewModelWithDependencies(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient()
    ) -> (AuthViewModel, AccountStateManager, FakeIPCClient) {
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let vm = AuthViewModel(accountStateManager: manager, ipcClient: client)
        return (vm, manager, client)
    }

    private func makeViewModel(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient()
    ) -> AuthViewModel {
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        return AuthViewModel(accountStateManager: manager, ipcClient: client)
    }
}
