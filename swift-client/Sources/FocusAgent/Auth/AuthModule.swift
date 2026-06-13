import Foundation

/// Auth module - owns local authentication state and Keychain token storage.
/// Does NOT store tokens in SQLite, upload events, or expose raw activity data.

/// Access and refresh tokens stored only in Keychain.
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

/// User authentication state.
public enum AuthState: Equatable, Sendable {
    case signedOut
    case authenticated
    case refreshRequired
}

/// Stores authentication tokens in Keychain.
public protocol TokenStoring: AnyObject {
    func load() throws -> AuthTokens?
    func save(_ tokens: AuthTokens) throws
    func clear() throws
}

/// Coordinates local authentication state without owning cloud transport.
public protocol AuthManaging: AnyObject {
    var state: AuthState { get }
    func currentTokens() throws -> AuthTokens?
    func update(tokens: AuthTokens) throws
    func signOut() throws
}

/// Concrete Keychain token-store placeholder.
public final class KeychainTokenStore: TokenStoring {
    public init() {}

    public func load() throws -> AuthTokens? {
        fatalError("not implemented")
    }

    public func save(_ tokens: AuthTokens) throws {
        fatalError("not implemented")
    }

    public func clear() throws {
        fatalError("not implemented")
    }
}

public enum AuthError: Error, Equatable {
    case keychain(code: Int)
    case authenticationRequired
}

