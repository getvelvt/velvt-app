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

/// Interface used by application modules to communicate with the Rust service.
public protocol IPCClientProtocol: AnyObject {
    func connect() async throws
    func disconnect()
    func send(_ message: ClientMessage) async throws
    var incomingMessages: AsyncStream<ServerMessage> { get }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> { get }
}
