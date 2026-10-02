import Combine
import Foundation

/// Connection lifecycle state exposed to the application UI.
public enum ConnectionStatus: Equatable, Sendable {
    case disconnected
    case connecting
    case handshaking
    case connected
    case reconnecting(attempt: Int, nextRetryIn: TimeInterval)
}

/// Errors produced by IPC transport and protocol negotiation.
public enum IPCError: Error, Equatable, Sendable {
    case socket(code: Int32)
    case malformedMessage
    case connectionClosed
    case notConnected
    case versionMismatch(expected: Int, got: Int)
    case handshakeFailed
}

/// A handshake the service answered with a different protocol version.
public struct IPCVersionMismatch: Equatable, Sendable {
    public let expected: Int
    public let got: Int

    public init(expected: Int, got: Int) {
        self.expected = expected
        self.got = got
    }
}

/// Interface used by application modules to communicate with the Rust service.
public protocol IPCClientProtocol: AnyObject {
    func connect() async throws
    func disconnect()
    func send(_ message: ClientMessage) async throws
    var incomingMessages: AsyncStream<ServerMessage> { get }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> { get }
    /// A version mismatch met while reconnecting on the client's own.
    ///
    /// `connect()` throws a mismatch to its caller, but a reconnect has no
    /// caller: it stops, because re-dialling cannot change the version a
    /// helper speaks, and publishes `.disconnected`, which looks like any
    /// other drop. This is how the app hears that it needs to act.
    var versionMismatches: AnyPublisher<IPCVersionMismatch, Never> { get }
}

extension IPCClientProtocol {
    /// Clients with no reconnect of their own never meet a mismatch there.
    public var versionMismatches: AnyPublisher<IPCVersionMismatch, Never> {
        Empty().eraseToAnyPublisher()
    }
}
