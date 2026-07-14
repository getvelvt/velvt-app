import Combine
import XCTest
@testable import VelvtMac

private struct TestStoredAuthSnapshot: Codable {
    var userId: String
    var email: String?
    var pendingDeletion: Bool
    var session: AuthSession
}

final class SnapshotCountingKeychain: KeychainProtocol {
    private var values: [KeychainKey: String]
    private(set) var loadKeys: [KeychainKey] = []

    init(values: [KeychainKey: String]) {
        self.values = values
    }

    func store(token: String, for key: KeychainKey) throws {
        values[key] = token
    }

    func load(for key: KeychainKey) throws -> String {
        loadKeys.append(key)
        guard let value = values[key] else { throw AuthError.keychainItemNotFound }
        return value
    }

    func loadAll() throws -> [KeychainKey: String] {
        throw AuthError.keychain(status: errSecAuthFailed)
    }

    func delete(for key: KeychainKey) throws {
        values.removeValue(forKey: key)
    }
}

// MARK: - AccountState transition tests

@MainActor
final class AccountStateManagerTests: XCTestCase {

    // MARK: Valid transitions

    func testLoggedOutToLoggingIn() {
        let sut = makeManager()
        sut.transition(to: .loggingIn)
        XCTAssertEqual(sut.accountState, .loggingIn)
    }

    func testLoggingInToLoggedInViaAuthSuccess() async throws {
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
            deviceId: "device-1",
            accessToken: "tok-access",
            refreshToken: "tok-refresh",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        await fulfillment(of: [settled], timeout: 1)
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u123"))
        let snapshot = try XCTUnwrap(decodeSnapshot(from: keychain))
        XCTAssertEqual(snapshot.userId, "u123")
        XCTAssertEqual(snapshot.session.accessToken, "tok-access")
        XCTAssertEqual(snapshot.session.refreshToken, "tok-refresh")
        XCTAssertEqual(snapshot.session.deviceId, "device-1")
        XCTAssertNil(keychain.storedValue(for: .accessToken))
        XCTAssertNil(keychain.storedValue(for: .userId))
    }

    func testAuthSuccessStoresSeparateUserAndDeviceSessions() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)
        sut.transition(to: .loggingIn)
        let deviceExpiresAt = Date(timeIntervalSinceNow: 3600)
        let userExpiresAt = Date(timeIntervalSinceNow: 1800)

        client.inject(.authSuccess(AuthSuccess(
            userId: "u123",
            deviceId: "device-1",
            accessToken: "device-access",
            refreshToken: "device-refresh",
            expiresAt: deviceExpiresAt,
            userAccessToken: "user-access",
            userRefreshToken: "user-refresh",
            userExpiresAt: userExpiresAt
        )))

        try? await Task.sleep(nanoseconds: 10_000_000)
        let snapshot = try XCTUnwrap(decodeSnapshot(from: keychain))
        XCTAssertEqual(snapshot.session.accessToken, "device-access")
        XCTAssertEqual(snapshot.session.refreshToken, "device-refresh")
        XCTAssertEqual(snapshot.session.userAccessToken, "user-access")
        XCTAssertEqual(snapshot.session.userRefreshToken, "user-refresh")
        XCTAssertEqual(snapshot.session.userExpiresAt?.timeIntervalSince1970 ?? 0, userExpiresAt.timeIntervalSince1970, accuracy: 0.001)
    }

    func testStartListeningSendsStoredAuthSessionToRust() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let expiresAt = Date(timeIntervalSinceNow: 3600)
        try seedSnapshot(
            in: keychain,
            userId: "u123",
            session: AuthSession(
                deviceId: "device-1",
                accessToken: "stored-access",
                refreshToken: "stored-refresh",
                expiresAt: expiresAt
            )
        )

        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)
        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        guard case .authSession(let session)? = client.sentMessages.first else {
            return XCTFail("Expected stored auth session to be sent to Rust")
        }
        XCTAssertEqual(session.deviceId, "device-1")
        XCTAssertEqual(session.accessToken, "stored-access")
        XCTAssertEqual(session.refreshToken, "stored-refresh")
        XCTAssertEqual(session.expiresAt.timeIntervalSince1970, expiresAt.timeIntervalSince1970, accuracy: 0.001)
    }

    func testStoredAuthSessionIsNotSentBeforeIPCConnect() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let expiresAt = Date(timeIntervalSinceNow: 3600)
        try seedSnapshot(
            in: keychain,
            userId: "u123",
            session: AuthSession(
                deviceId: "device-1",
                accessToken: "stored-access",
                refreshToken: "stored-refresh",
                expiresAt: expiresAt
            )
        )

        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertTrue(client.sentMessages.isEmpty)

        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        guard case .authSession(let session)? = client.sentMessages.first else {
            return XCTFail("Expected stored auth session after IPC connection")
        }
        XCTAssertEqual(session.deviceId, "device-1")
    }

    func testInitializationReadsOnlySingleAuthSnapshotKey() async throws {
        let client = FakeIPCClient()
        let expiresAt = Date(timeIntervalSinceNow: 3600)
        let rawSnapshot = try encodeSnapshot(TestStoredAuthSnapshot(
            userId: "u123",
            email: nil,
            pendingDeletion: false,
            session: AuthSession(
                deviceId: "device-1",
                accessToken: "stored-access",
                refreshToken: "stored-refresh",
                expiresAt: expiresAt
            )
        ))
        let keychain = SnapshotCountingKeychain(values: [.authSnapshot: rawSnapshot])

        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)
        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u123"))
        XCTAssertEqual(keychain.loadKeys, [.authSnapshot])
        guard case .authSession? = client.sentMessages.first else {
            return XCTFail("Expected stored auth session to be sent to Rust")
        }
    }

    func testSuccessfulAuthenticationStoresEmailInKeychain() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = AccountStateManager(keychain: keychain)
        sut.startListening(to: client)

        XCTAssertTrue(sut.beginAuthentication(email: "ada@example.com"))
        client.inject(.authSuccess(AuthSuccess(
            userId: "u123", deviceId: "device-1", accessToken: "tok-access", refreshToken: "tok-refresh",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        try? await Task.sleep(nanoseconds: 10_000_000)
        XCTAssertEqual(decodeSnapshot(from: keychain)?.email, "ada@example.com")
        XCTAssertEqual(sut.accountEmail, "ada@example.com")
    }

    func testLogoutDeletesStoredEmail() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u123", email: "ada@example.com")
        let sut = AccountStateManager(keychain: keychain)

        sut.logOut()

        XCTAssertNil(keychain.storedValue(for: .authSnapshot))
    }

    func testAccountEmailIsReadOnceThenServedFromMemory() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, email: "ada@example.com")
        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountEmail, "ada@example.com")
        let loadsAfterFirstRead = keychain.loadCount
        XCTAssertEqual(sut.accountEmail, "ada@example.com")
        XCTAssertEqual(keychain.loadCount, loadsAfterFirstRead)
    }

    func testAccountEmailIsCachedDuringInitializationForSettingsDisplay() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, email: "ada@example.com")
        let sut = AccountStateManager(keychain: keychain)
        let loadsAfterInit = keychain.loadCount

        XCTAssertEqual(sut.accountEmail, "ada@example.com")
        XCTAssertEqual(keychain.loadCount, loadsAfterInit)
    }

    func testInitializationRestoresAuthStateWithSingleKeychainReadPass() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u123", email: "ada@example.com")

        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u123"))
        XCTAssertEqual(sut.accountEmail, "ada@example.com")
        XCTAssertEqual(keychain.loadCount, 1)
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
        try seedSnapshot(in: keychain, userId: "u42")
        let sut = AccountStateManager(keychain: keychain)
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u42"))
    }

    func testInitIsLoggedOutWhenKeychainEmpty() {
        XCTAssertEqual(AccountStateManager(keychain: FakeKeychain()).accountState, .loggedOut)
    }

    // MARK: cancelPendingErasure

    func testCancelPendingErasureRevertsToLoggedIn() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u99")
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
        try seedSnapshot(in: keychain, userId: "u5", pendingDeletion: true)

        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountState, .pendingErasure)
    }

    func testInitIsLoggedInWhenUserIdPresentButNoDeletionFlag() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u5")

        XCTAssertEqual(AccountStateManager(keychain: keychain).accountState, .loggedIn(userId: "u5"))
    }

    func testTransitionToPendingErasureWritesDeletionFlag() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u1")
        let sut = AccountStateManager(keychain: keychain)

        sut.transition(to: .pendingErasure)

        XCTAssertEqual(decodeSnapshot(from: keychain)?.pendingDeletion, true, "sentinel must be written")
    }

    func testCancelPendingErasureClearsDeletionFlag() throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u1", pendingDeletion: true)
        let sut = AccountStateManager(keychain: keychain)

        XCTAssertEqual(sut.accountState, .pendingErasure)
        sut.cancelPendingErasure()

        XCTAssertEqual(decodeSnapshot(from: keychain)?.pendingDeletion, false, "sentinel must be cleared on cancel")
        XCTAssertEqual(sut.accountState, .loggedIn(userId: "u1"))
    }

    func testAccountDeletionAcceptedClearsDeletionFlagViaDeleteAll() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let sut = makeLoggedInManager(keychain: keychain, client: client)
        sut.transition(to: .pendingErasure)
        XCTAssertEqual(decodeSnapshot(from: keychain)?.pendingDeletion, true)

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
        try seedSnapshot(in: keychain, userId: "u1", pendingDeletion: true)
        let sut = AccountStateManager(keychain: keychain)

        // A transition to loggedIn is not a valid move from pendingErasure.
        sut.transition(to: .loggedIn(userId: "u1"))

        XCTAssertEqual(sut.accountState, .pendingErasure, "normal use must be blocked during pending erasure")
    }

    // MARK: Helpers

    private func seedSnapshot(
        in keychain: FakeKeychain,
        userId: String = "u1",
        email: String? = nil,
        pendingDeletion: Bool = false,
        session: AuthSession = AuthSession(
            deviceId: "device-1",
            accessToken: "access-token",
            refreshToken: "refresh-token",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )
    ) throws {
        let snapshot = TestStoredAuthSnapshot(
            userId: userId,
            email: email,
            pendingDeletion: pendingDeletion,
            session: session
        )
        try keychain.store(token: encodeSnapshot(snapshot), for: .authSnapshot)
    }

    private func decodeSnapshot(from keychain: FakeKeychain) -> TestStoredAuthSnapshot? {
        guard let rawSnapshot = keychain.storedValue(for: .authSnapshot),
              let data = rawSnapshot.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(TestStoredAuthSnapshot.self, from: data)
    }

    private func encodeSnapshot(_ snapshot: TestStoredAuthSnapshot) throws -> String {
        let data = try JSONEncoder().encode(snapshot)
        return String(data: data, encoding: .utf8)!
    }

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
        try? seedSnapshot(in: keychain, userId: "u1")
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
            deviceId: "device-1",
            accessToken: "at-x",
            refreshToken: "rt-x",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        await fulfillment(of: [loggedIn], timeout: 2)

        XCTAssertEqual(manager.accountState, .loggedIn(userId: "u-001"))
        let snapshot = decodeSnapshot(from: keychain)
        XCTAssertEqual(snapshot?.userId, "u-001")
        XCTAssertEqual(snapshot?.session.accessToken, "at-x")
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
        try seedSnapshot(in: keychain, userId: "u1")
        let manager = AccountStateManager(keychain: keychain)
        let client = FakeIPCClient()
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client)

        sut.logOut()

        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertTrue(keychain.isEmpty)
    }

    func testLogOutSendsLogOutIPCMessage() async throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u1")
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

    func testConnectionStatusTracksTheIPCClient() async {
        let (sut, _, client) = makeViewModelWithDependencies()

        XCTAssertEqual(sut.connectionStatus, .disconnected)

        client.setConnectionStatus(.connected)
        await Task.yield()

        XCTAssertEqual(sut.connectionStatus, .connected)
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

    func testSignUpTimesOutWhenServiceNeverResponds() async {
        let (sut, manager, client) = makeViewModelWithDependencies(timeout: 0.05)
        sut.email = "user@example.com"
        sut.password = "secret"
        client.setConnectionStatus(.connected)

        let errorSet = expectation(description: "timeout error")
        var cancellable: AnyCancellable?
        cancellable = sut.$errorMessage.compactMap { $0 }.sink { message in
            if message == "Authentication timed out. Please try again." {
                errorSet.fulfill()
                cancellable?.cancel()
            }
        }

        await sut.signUp()

        await fulfillment(of: [errorSet], timeout: 1)
        XCTAssertEqual(manager.accountState, .loggedOut)
        XCTAssertFalse(sut.isLoading)
    }

    func testAuthSuccessCancelsTimeout() async {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let sut = AuthViewModel(accountStateManager: manager, ipcClient: client, authResponseTimeout: 0.05)
        sut.email = "user@example.com"
        sut.password = "secret"
        client.setConnectionStatus(.connected)

        let loggedIn = expectation(description: "logged in before timeout")
        var cancellable: AnyCancellable?
        cancellable = manager.$accountState.dropFirst().sink { state in
            if case .loggedIn = state {
                loggedIn.fulfill()
                cancellable?.cancel()
            }
        }

        await sut.logIn()
        client.inject(.authSuccess(AuthSuccess(
            userId: "u-001",
            deviceId: "device-1",
            accessToken: "at-x",
            refreshToken: "rt-x",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )))

        await fulfillment(of: [loggedIn], timeout: 1)
        try? await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertNil(sut.errorMessage)
        XCTAssertFalse(sut.isLoading)
        XCTAssertEqual(manager.accountState, .loggedIn(userId: "u-001"))
    }

    // MARK: deleteAccount IPC send failure

    func testDeleteAccountIPCSendFailureRevertsToLoggedInAndShowsRetryMessage() async throws {
        let keychain = FakeKeychain()
        try seedSnapshot(in: keychain, userId: "u1")
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
        XCTAssertNotNil(keychain.storedValue(for: .authSnapshot))
        // Deletion sentinel must be cleared since erasure did not proceed.
        XCTAssertEqual(decodeSnapshot(from: keychain)?.pendingDeletion, false)
    }

    // MARK: Helpers

    private func seedSnapshot(
        in keychain: FakeKeychain,
        userId: String = "u1",
        email: String? = nil,
        pendingDeletion: Bool = false,
        session: AuthSession = AuthSession(
            deviceId: "device-1",
            accessToken: "access-token",
            refreshToken: "refresh-token",
            expiresAt: Date(timeIntervalSinceNow: 3600)
        )
    ) throws {
        let snapshot = TestStoredAuthSnapshot(
            userId: userId,
            email: email,
            pendingDeletion: pendingDeletion,
            session: session
        )
        let data = try JSONEncoder().encode(snapshot)
        try keychain.store(token: String(data: data, encoding: .utf8)!, for: .authSnapshot)
    }

    private func decodeSnapshot(from keychain: FakeKeychain) -> TestStoredAuthSnapshot? {
        guard let rawSnapshot = keychain.storedValue(for: .authSnapshot),
              let data = rawSnapshot.data(using: .utf8) else {
            return nil
        }
        return try? JSONDecoder().decode(TestStoredAuthSnapshot.self, from: data)
    }

    private func makeViewModelWithDependencies(
        keychain: FakeKeychain = FakeKeychain(),
        client: FakeIPCClient = FakeIPCClient(),
        timeout: TimeInterval = 30
    ) -> (AuthViewModel, AccountStateManager, FakeIPCClient) {
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)
        let vm = AuthViewModel(accountStateManager: manager, ipcClient: client, authResponseTimeout: timeout)
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
