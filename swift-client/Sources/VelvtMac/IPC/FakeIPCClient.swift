import Combine
import Foundation

/// Test double that records outbound messages and accepts injected server messages.
public final class FakeIPCClient: IPCClientProtocol, @unchecked Sendable {
    public let incomingMessages: AsyncStream<ServerMessage>
    public var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    public var sentMessages: [ClientMessage] {
        lock.withLock { recordedMessages }
    }

    private let lock = NSLock()
    private let statusSubject = CurrentValueSubject<ConnectionStatus, Never>(.disconnected)
    private let continuation: AsyncStream<ServerMessage>.Continuation
    private var recordedMessages: [ClientMessage] = []

    public init() {
        var capturedContinuation: AsyncStream<ServerMessage>.Continuation?
        incomingMessages = AsyncStream { capturedContinuation = $0 }
        guard let capturedContinuation else {
            preconditionFailure("AsyncStream did not create a continuation")
        }
        continuation = capturedContinuation
    }

    public func connect() async throws {
        statusSubject.send(.connected)
    }

    public func disconnect() {
        statusSubject.send(.disconnected)
    }

    /// When set, `send(_:)` throws this error instead of recording the message.
    public var shouldThrowOnSend: (any Error)?

    public func send(_ message: ClientMessage) async throws {
        if let error = shouldThrowOnSend { throw error }
        lock.withLock {
            recordedMessages.append(message)
        }
    }

    public func inject(_ message: ServerMessage) {
        continuation.yield(message)
    }

    /// Finishes the `incomingMessages` stream, simulating a hard IPC disconnect.
    /// After this call no further messages can be injected on this instance.
    public func closeStream() {
        continuation.finish()
    }

    public func setConnectionStatus(_ status: ConnectionStatus) {
        statusSubject.send(status)
    }
}
