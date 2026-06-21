import Combine
import Foundation
import os.log
import Security

/// Auth module - owns account state, Keychain token storage, and IPC message
/// routing for auth-related server pushes.
///
/// No HTTP calls are made from Swift. All network operations are performed by
/// the Rust service and communicated back via IPC.

// MARK: - KeychainKey

public enum KeychainKey: String, CaseIterable, Sendable {
    case accessToken = "velvt.access_token"
    case refreshToken = "velvt.refresh_token"
    case userId = "velvt.user_id"
    case email = "velvt.email"
    /// Sentinel written when the state machine enters `.pendingErasure` so that
    /// a relaunch mid-deletion can restore the correct state and block normal use.
    case pendingDeletion = "velvt.pending_deletion"
}

// MARK: - KeychainProtocol

public protocol KeychainProtocol: AnyObject {
    func store(token: String, for key: KeychainKey) throws
    func load(for key: KeychainKey) throws -> String
    func delete(for key: KeychainKey) throws
}

extension KeychainProtocol {
    func deleteAll() {
        for key in KeychainKey.allCases {
            try? delete(for: key)
        }
    }
}

// MARK: - AuthError

public enum AuthError: Error, Equatable {
    case keychain(status: OSStatus)
    case keychainItemNotFound
    case authenticationRequired
}

// MARK: - KeychainService

/// Concrete Keychain implementation backed by Security.framework.
///
/// Each token is stored as a generic password entry keyed by `KeychainKey.rawValue`.
/// Tokens must never appear in logs or string interpolation outside this type.
public final class KeychainService: KeychainProtocol {
    private let service: String

    public init(service: String = "com.velvt.mac") {
        self.service = service
    }

    public func store(token: String, for key: KeychainKey) throws {
        guard let data = token.data(using: .utf8) else {
            throw AuthError.keychain(status: errSecParam)
        }
        let query = baseQuery(for: key)
        let attributes: [String: Any] = [kSecValueData as String: data]

        var status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var addQuery = query
            addQuery[kSecValueData as String] = data
            status = SecItemAdd(addQuery as CFDictionary, nil)
        }
        guard status == errSecSuccess else {
            throw AuthError.keychain(status: status)
        }
    }

    public func load(for key: KeychainKey) throws -> String {
        var query = baseQuery(for: key)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else {
            throw status == errSecItemNotFound
                ? AuthError.keychainItemNotFound
                : AuthError.keychain(status: status)
        }
        guard let data = result as? Data, let token = String(data: data, encoding: .utf8) else {
            throw AuthError.keychain(status: errSecDecode)
        }
        return token
    }

    public func delete(for key: KeychainKey) throws {
        let status = SecItemDelete(baseQuery(for: key) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw AuthError.keychain(status: status)
        }
    }

    private func baseQuery(for key: KeychainKey) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key.rawValue,
        ]
    }
}

// MARK: - FakeKeychain

/// In-memory Keychain test double. Never touches the real Keychain, so tests
/// run without Keychain entitlements.
public final class FakeKeychain: KeychainProtocol {
    private var storage: [KeychainKey: String] = [:]
    public var shouldThrowOnStore: AuthError?
    public var shouldThrowOnLoad: AuthError?
    public var shouldThrowOnDelete: AuthError?

    public init() {}

    public func store(token: String, for key: KeychainKey) throws {
        if let error = shouldThrowOnStore { throw error }
        storage[key] = token
    }

    public func load(for key: KeychainKey) throws -> String {
        if let error = shouldThrowOnLoad { throw error }
        guard let token = storage[key] else { throw AuthError.keychainItemNotFound }
        return token
    }

    public func delete(for key: KeychainKey) throws {
        if let error = shouldThrowOnDelete { throw error }
        storage.removeValue(forKey: key)
    }

    public func storedValue(for key: KeychainKey) -> String? { storage[key] }
    public var isEmpty: Bool { storage.isEmpty }
}

// MARK: - AccountState

public enum AccountState: Equatable, Sendable {
    case loggedOut
    case loggingIn
    case loggedIn(userId: String)
    case loggingOut
    case pendingErasure
}

// MARK: - AccountStateManager

private let authLogger = Logger(subsystem: "com.velvt.mac", category: "AccountStateManager")

/// Owns account state and Keychain tokens. The sole consumer of the IPC
/// `incomingMessages` stream; re-publishes all server messages to `serverMessages`
/// so downstream consumers (AuthViewModel, delivery) can subscribe without
/// competing for the stream.
///
/// All state transitions are validated. External callers use `transition(to:)`.
/// Internal server-push handlers may set `accountState` directly after performing
/// side-effects (Keychain cleanup), since the guard is the trusted internal path.
@MainActor
public final class AccountStateManager: ObservableObject {
    @Published public private(set) var accountState: AccountState
    /// Set to `true` when a `device_revoked` push is received. Reset by calling
    /// `clearDeviceRevokedFlag()` after the recovery UI has been shown.
    @Published public private(set) var isDeviceRevoked: Bool

    /// Fan-out relay for all incoming server messages. Consumers subscribe here
    /// rather than iterating `incomingMessages` directly so only one task owns
    /// the stream.
    public let serverMessages: PassthroughSubject<ServerMessage, Never>

    private let keychain: any KeychainProtocol
    private var listenerTask: Task<Void, Never>?
    private var pendingEmail: String?

    public init(keychain: any KeychainProtocol) {
        self.keychain = keychain
        let storedUserId = try? keychain.load(for: .userId)
        let isPendingDeletion = (try? keychain.load(for: .pendingDeletion)) != nil
        let initialState: AccountState
        if let uid = storedUserId {
            initialState = isPendingDeletion ? .pendingErasure : .loggedIn(userId: uid)
        } else {
            initialState = .loggedOut
        }
        _accountState = Published(wrappedValue: initialState)
        _isDeviceRevoked = Published(wrappedValue: false)
        serverMessages = PassthroughSubject()
    }

    // MARK: - State machine

    /// Applies a validated state transition. Invalid transitions are rejected
    /// with a structured log — never a crash.
    public func transition(to newState: AccountState) {
        guard isValidTransition(from: accountState, to: newState) else {
            authLogger.error(
                "auth.transition: REJECTED \(String(describing: self.accountState)) → \(String(describing: newState))"
            )
            return
        }
        authLogger.debug(
            "auth.transition: \(String(describing: self.accountState)) → \(String(describing: newState))"
        )
        // Persist the pendingDeletion sentinel so a relaunch mid-erasure can
        // restore the correct blocking state.
        if case .pendingErasure = newState {
            try? keychain.store(token: "1", for: .pendingDeletion)
        }
        accountState = newState
    }

    // MARK: - Auth actions

    /// Starts authentication only when no other authentication request is active.
    public func beginAuthentication() -> Bool {
        guard case .loggedOut = accountState else {
            return false
        }
        accountState = .loggingIn
        return true
    }

    /// Retains the email only until the service confirms authentication, then
    /// stores it with the session in Keychain for local account display.
    public func beginAuthentication(email: String) -> Bool {
        guard beginAuthentication() else { return false }
        pendingEmail = email
        return true
    }

    public var accountEmail: String? {
        try? keychain.load(for: .email)
    }

    /// Returns an in-flight authentication request to the logged-out state.
    public func cancelAuthentication() {
        guard case .loggingIn = accountState else {
            return
        }
        accountState = .loggedOut
        pendingEmail = nil
    }

    /// Clears all tokens from Keychain and transitions to `.loggedOut`.
    /// Does NOT send an IPC message — callers are responsible for that if needed.
    public func logOut() {
        transition(to: .loggingOut)
        keychain.deleteAll()
        pendingEmail = nil
        accountState = .loggedOut
    }

    /// Reverts from `.pendingErasure` to `.loggedIn` if the deletion IPC send
    /// failed and the session is still valid. Requires knowing the current userId.
    public func cancelPendingErasure() {
        guard case .pendingErasure = accountState,
              let userId = try? keychain.load(for: .userId) else { return }
        try? keychain.delete(for: .pendingDeletion)
        accountState = .loggedIn(userId: userId)
    }

    /// Resets the device-revoked flag after the recovery UI has been dismissed.
    public func clearDeviceRevokedFlag() {
        isDeviceRevoked = false
    }

    // MARK: - IPC listener

    /// Starts consuming `client.incomingMessages`. Must be called exactly once
    /// after the IPC client is constructed.
    public func startListening(to client: any IPCClientProtocol) {
        listenerTask?.cancel()
        listenerTask = Task { [weak self] in
            for await message in client.incomingMessages {
                guard let self, !Task.isCancelled else { break }
                self.handle(message)
            }
            // Stream ended while the task was not cancelled — the IPC connection
            // dropped unexpectedly. Revert transient states so the UI does not
            // become stuck in a loading/loggingIn/loggingOut limbo.
            // pendingErasure is intentionally NOT reverted: the Rust service may
            // have already received the delete request and the sentinel must
            // survive until the next connection confirms or cancels the erasure.
            guard let self, !Task.isCancelled else { return }
            authLogger.warning(
                "auth.startListening: incomingMessages stream ended unexpectedly, current state=\(String(describing: self.accountState))"
            )
            switch self.accountState {
            case .loggingIn, .loggingOut:
                authLogger.warning(
                    "auth.startListening: reverting \(String(describing: self.accountState)) → loggedOut due to stream drop"
                )
                self.accountState = .loggedOut
            default:
                break
            }
        }
    }

    public func stopListening() {
        listenerTask?.cancel()
        listenerTask = nil
    }

    // MARK: - Private

    private func handle(_ message: ServerMessage) {
        authLogger.debug("IPC message received: \(String(describing: message))")
        serverMessages.send(message)
        switch message {
        case .authSuccess(let success):
            authLogger.debug("auth.handle: authSuccess received for userId \(success.userId)")
            handleAuthSuccess(success)
        case .authFailure(let failure):
            authLogger.debug(
                "auth.handle: authFailure received — code=\(String(describing: failure.code)) message=\(failure.message)"
            )
            if case .loggingIn = accountState {
                accountState = .loggedOut
                pendingEmail = nil
            }
        case .accountDeletionAccepted:
            keychain.deleteAll()
            pendingEmail = nil
            accountState = .loggedOut
        case .needsReauth:
            keychain.deleteAll()
            pendingEmail = nil
            accountState = .loggedOut
        case .deviceRevoked:
            keychain.deleteAll()
            pendingEmail = nil
            accountState = .loggedOut
            isDeviceRevoked = true
        default:
            break
        }
    }

    private func handleAuthSuccess(_ success: AuthSuccess) {
        guard case .loggingIn = accountState else {
            authLogger.warning(
                "auth.handleAuthSuccess: received authSuccess but state is \(String(describing: self.accountState)), ignoring"
            )
            return
        }
        authLogger.debug("auth.handleAuthSuccess: writing tokens to Keychain")
        do {
            try keychain.store(token: success.accessToken, for: .accessToken)
            try keychain.store(token: success.refreshToken, for: .refreshToken)
            try keychain.store(token: success.userId, for: .userId)
            if let pendingEmail {
                try keychain.store(token: pendingEmail, for: .email)
            }
            authLogger.debug(
                "auth.handleAuthSuccess: Keychain writes succeeded, transitioning to loggedIn"
            )
            transition(to: .loggedIn(userId: success.userId))
            pendingEmail = nil
        } catch {
            authLogger.error(
                "auth.handleAuthSuccess: Keychain write failed — \(error.localizedDescription)"
            )
            keychain.deleteAll()
            pendingEmail = nil
            accountState = .loggedOut
        }
    }

    private func isValidTransition(from current: AccountState, to next: AccountState) -> Bool {
        switch (current, next) {
        case (.loggedOut, .loggingIn): return true
        case (.loggingIn, .loggedOut): return true
        case (.loggingIn, .loggedIn): return true
        case (.loggedIn, .loggingOut): return true
        case (.loggingOut, .loggedOut): return true
        case (.loggedIn, .pendingErasure): return true
        case (.pendingErasure, .loggedOut): return true
        default: return false
        }
    }
}

// MARK: - AuthTokens (kept for compatibility; individual keys are preferred)

public struct AuthTokens: Equatable, Sendable {
    public let accessToken: String
    public let refreshToken: String
    public let expiresAt: Date

    public init(accessToken: String, refreshToken: String, expiresAt: Date) {
        self.accessToken = accessToken
        self.refreshToken = refreshToken
        self.expiresAt = expiresAt
    }
}
