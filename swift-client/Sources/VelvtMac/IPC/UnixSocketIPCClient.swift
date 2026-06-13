import Combine
import Darwin
import Foundation
import Network

/// Async newline-delimited frame transport used by the IPC client actor.
protocol IPCTransportProtocol: Actor {
    func connect(to path: String) async throws
    func send(frame: Data) async throws
    func receiveFrame() async throws -> Data
    func close() async
}

/// Injectable sleep operation used by reconnect scheduling.
protocol IPCSleeping: Actor {
    func sleep(for delay: TimeInterval) async throws
}

actor TaskSleeper: IPCSleeping {
    func sleep(for delay: TimeInterval) async throws {
        try await Task.sleep(for: .seconds(delay))
    }
}

/// Unix-domain socket IPC client with an actor-isolated connection state machine.
public actor UnixSocketIPCClient: IPCClientProtocol {
    public nonisolated let incomingMessages: AsyncStream<ServerMessage>
    public nonisolated var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private nonisolated let statusSubject = CurrentValueSubject<ConnectionStatus, Never>(.disconnected)
    private nonisolated let incomingContinuation: AsyncStream<ServerMessage>.Continuation
    private let socketPath: String
    private let protocolVersion: Int
    private let clientVersion: String
    private let backoff: ReconnectBackoff
    private let sleeper: any IPCSleeping
    private let transportFactory: @Sendable () -> any IPCTransportProtocol
    private let encoder = IPCMessageCodec.makeEncoder()
    private let decoder = IPCMessageCodec.makeDecoder()
    private var transport: (any IPCTransportProtocol)?
    private var receiveTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var connectionState = ConnectionStatus.disconnected
    private var inFlightSendCount = 0
    private var deferredCloseTransport: (any IPCTransportProtocol)?
    private var stopped = true

    public init(socketPath: String, protocolVersion: Int, clientVersion: String) {
        var continuation: AsyncStream<ServerMessage>.Continuation?
        incomingMessages = AsyncStream { continuation = $0 }
        guard let continuation else {
            preconditionFailure("AsyncStream did not create a continuation")
        }
        incomingContinuation = continuation
        self.socketPath = socketPath
        self.protocolVersion = protocolVersion
        self.clientVersion = clientVersion
        backoff = ReconnectBackoff()
        sleeper = TaskSleeper()
        transportFactory = { UnixSocketTransport() }
    }

    init(
        socketPath: String,
        protocolVersion: Int,
        clientVersion: String,
        backoff: ReconnectBackoff = ReconnectBackoff(),
        sleeper: any IPCSleeping = TaskSleeper(),
        transportFactory: @escaping @Sendable () -> any IPCTransportProtocol
    ) {
        var continuation: AsyncStream<ServerMessage>.Continuation?
        incomingMessages = AsyncStream { continuation = $0 }
        guard let continuation else {
            preconditionFailure("AsyncStream did not create a continuation")
        }
        incomingContinuation = continuation
        self.socketPath = socketPath
        self.protocolVersion = protocolVersion
        self.clientVersion = clientVersion
        self.backoff = backoff
        self.sleeper = sleeper
        self.transportFactory = transportFactory
    }

    public func connect() async throws {
        stopped = false
        reconnectTask?.cancel()
        reconnectTask = nil
        do {
            try await connectOnce()
            startReceiveLoop()
        } catch let error as IPCError {
            await closeTransport()
            if case .versionMismatch = error {
                publish(.disconnected)
            } else {
                await handleConnectionLoss()
            }
            throw error
        } catch {
            await closeTransport()
            await handleConnectionLoss()
            throw error
        }
    }

    public nonisolated func disconnect() {
        Task {
            await stop()
        }
    }

    public func send(_ message: ClientMessage) async throws {
        guard connectionState == .connected, let transport else {
            throw IPCError.notConnected
        }
        inFlightSendCount += 1
        do {
            try await transport.send(frame: encoder.encode(message))
            await finishSend()
        } catch {
            await finishSend()
            throw error
        }
    }

    private func connectOnce() async throws {
        publish(.connecting)
        let nextTransport = transportFactory()
        transport = nextTransport
        try await nextTransport.connect(to: expandedSocketPath())
        publish(.handshaking)

        let hello = try decoder.decode(ServerMessage.self, from: await nextTransport.receiveFrame())
        guard case .serverHello = hello else {
            throw IPCError.handshakeFailed
        }

        try await nextTransport.send(
            frame: encoder.encode(
                ClientMessage.clientHello(
                    ClientHello(expectedProtocolVersion: protocolVersion, clientVersion: clientVersion)
                )
            )
        )

        let response = try decoder.decode(ServerMessage.self, from: await nextTransport.receiveFrame())
        switch response {
        case .acknowledged:
            publish(.connected)
        case let .versionMismatch(mismatch):
            throw IPCError.versionMismatch(
                expected: mismatch.serverProtocolVersion,
                got: mismatch.clientProtocolVersion
            )
        default:
            throw IPCError.handshakeFailed
        }
    }

    private func startReceiveLoop() {
        receiveTask?.cancel()
        receiveTask = Task {
            do {
                while !Task.isCancelled, let transport {
                    let message = try decoder.decode(ServerMessage.self, from: await transport.receiveFrame())
                    incomingContinuation.yield(message)
                }
            } catch is CancellationError {
                return
            } catch {
                await handleConnectionLoss()
            }
        }
    }

    private func handleConnectionLoss() async {
        guard !stopped, reconnectTask == nil else {
            return
        }
        await detachActiveTransport()
        reconnectTask = Task {
            await reconnect()
        }
    }

    private func reconnect() async {
        var attempt = 1
        while !stopped, !Task.isCancelled {
            let delay = backoff.delay(forAttempt: attempt)
            publish(.reconnecting(attempt: attempt, nextRetryIn: delay))
            do {
                try await sleeper.sleep(for: delay)
                try await connectOnce()
                reconnectTask = nil
                startReceiveLoop()
                return
            } catch is CancellationError {
                reconnectTask = nil
                return
            } catch let error as IPCError {
                await closeTransport()
                if case .versionMismatch = error {
                    publish(.disconnected)
                    reconnectTask = nil
                    return
                }
                attempt += 1
            } catch {
                await closeTransport()
                attempt += 1
            }
        }
        reconnectTask = nil
    }

    private func stop() async {
        stopped = true
        receiveTask?.cancel()
        reconnectTask?.cancel()
        receiveTask = nil
        reconnectTask = nil
        await closeTransport()
        publish(.disconnected)
    }

    private func closeTransport() async {
        await transport?.close()
        transport = nil
    }

    private func publish(_ status: ConnectionStatus) {
        connectionState = status
        statusSubject.send(status)
    }

    private func detachActiveTransport() async {
        guard let activeTransport = transport else {
            return
        }
        transport = nil
        if inFlightSendCount > 0 {
            deferredCloseTransport = activeTransport
        } else {
            await activeTransport.close()
        }
    }

    private func finishSend() async {
        inFlightSendCount -= 1
        guard inFlightSendCount == 0, let deferredCloseTransport else {
            return
        }
        self.deferredCloseTransport = nil
        await deferredCloseTransport.close()
    }

    private func expandedSocketPath() -> String {
        NSString(string: socketPath).expandingTildeInPath
    }
}

actor UnixSocketTransport: IPCTransportProtocol {
    private var connection: NWConnection?
    private var bufferedData = Data()

    func connect(to path: String) async throws {
        guard FileManager.default.fileExists(atPath: path) else {
            throw IPCError.socket(code: ENOENT)
        }
        let connection = NWConnection(to: .unix(path: path), using: .tcp)
        self.connection = connection
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let gate = ContinuationGate(continuation)
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    gate.resume()
                case let .failed(error):
                    gate.resume(throwing: IPCError.socket(code: error.safeCode))
                case .cancelled:
                    gate.resume(throwing: IPCError.connectionClosed)
                default:
                    break
                }
            }
            connection.start(queue: DispatchQueue(label: "com.velvt.mac.ipc.socket"))
        }
        connection.stateUpdateHandler = nil
    }

    func send(frame: Data) async throws {
        guard let connection else {
            throw IPCError.connectionClosed
        }
        var framed = frame
        framed.append(0x0A)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            connection.send(content: framed, completion: .contentProcessed { error in
                if let error {
                    continuation.resume(throwing: IPCError.socket(code: error.safeCode))
                } else {
                    continuation.resume()
                }
            })
        }
    }

    func receiveFrame() async throws -> Data {
        while true {
            if let newline = bufferedData.firstIndex(of: 0x0A) {
                let frame = bufferedData[..<newline]
                bufferedData.removeSubrange(...newline)
                return Data(frame)
            }
            guard let connection else {
                throw IPCError.connectionClosed
            }
            let chunk = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Data, Error>) in
                connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { data, _, complete, error in
                    if let error {
                        continuation.resume(throwing: IPCError.socket(code: error.safeCode))
                    } else if complete, data?.isEmpty != false {
                        continuation.resume(throwing: IPCError.connectionClosed)
                    } else {
                        continuation.resume(returning: data ?? Data())
                    }
                }
            }
            bufferedData.append(chunk)
        }
    }

    func close() async {
        connection?.cancel()
        connection = nil
        bufferedData.removeAll(keepingCapacity: false)
    }
}

private final class ContinuationGate: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Error>?

    init(_ continuation: CheckedContinuation<Void, Error>) {
        self.continuation = continuation
    }

    func resume() {
        take()?.resume()
    }

    func resume(throwing error: Error) {
        take()?.resume(throwing: error)
    }

    private func take() -> CheckedContinuation<Void, Error>? {
        lock.withLock {
            defer { continuation = nil }
            return continuation
        }
    }
}

private extension NWError {
    var safeCode: Int32 {
        switch self {
        case let .posix(code):
            return code.rawValue
        case let .dns(code):
            return Int32(code)
        case let .tls(code):
            return Int32(code)
        @unknown default:
            return -1
        }
    }
}
