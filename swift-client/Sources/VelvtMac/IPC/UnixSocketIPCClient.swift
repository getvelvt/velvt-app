import Combine
import Darwin
import Foundation

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

    public nonisolated var versionMismatches: AnyPublisher<IPCVersionMismatch, Never> {
        versionMismatchSubject.eraseToAnyPublisher()
    }

    private nonisolated let statusSubject = CurrentValueSubject<ConnectionStatus, Never>(.disconnected)
    private nonisolated let versionMismatchSubject = PassthroughSubject<IPCVersionMismatch, Never>()
    private nonisolated let incomingContinuation: AsyncStream<ServerMessage>.Continuation
    private let socketPath: String
    private let protocolVersion: Int
    private let clientVersion: String
    private let connectionTimeout: Duration
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
        connectionTimeout = .seconds(5)
        // A fast initial retry cadence, not the default 1s base: the very
        // first connection attempt typically races the bundled
        // velvt-service binary's own startup (ServiceProcessLauncher spawns
        // it and AppDelegate connects concurrently), so the first failure
        // almost always just means "give it a moment," not "unreachable."
        // Still escalates exponentially up to the same 60s cap for a
        // genuinely down service.
        backoff = ReconnectBackoff(baseDelay: 0.25)
        sleeper = TaskSleeper()
        transportFactory = { UnixSocketTransport() }
    }

    init(
        socketPath: String,
        protocolVersion: Int,
        clientVersion: String,
        connectionTimeout: Duration = .seconds(5),
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
        self.connectionTimeout = connectionTimeout
        self.backoff = backoff
        self.sleeper = sleeper
        self.transportFactory = transportFactory
    }

    public func connect() async throws {
        stopped = false
        reconnectTask?.cancel()
        reconnectTask = nil
        do {
            try await connectOnceWithTimeout()
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
        case .versionMismatch(let mismatch):
            throw IPCError.versionMismatch(
                expected: mismatch.serverProtocolVersion,
                got: mismatch.clientProtocolVersion
            )
        default:
            throw IPCError.handshakeFailed
        }
    }

    private func connectOnceWithTimeout() async throws {
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask { [self] in
                try await connectOnce()
            }
            group.addTask { [connectionTimeout] in
                try await Task.sleep(for: connectionTimeout)
                throw IPCError.connectionClosed
            }
            defer { group.cancelAll() }
            _ = try await group.next()
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
                try await connectOnceWithTimeout()
                reconnectTask = nil
                startReceiveLoop()
                return
            } catch is CancellationError {
                reconnectTask = nil
                return
            } catch let error as IPCError {
                await closeTransport()
                if case .versionMismatch(let expected, let got) = error {
                    publish(.disconnected)
                    reconnectTask = nil
                    // Nothing awaits a reconnect, so the mismatch goes where
                    // the first connect's would: to the app's recovery path.
                    versionMismatchSubject.send(IPCVersionMismatch(expected: expected, got: got))
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

/// The transport the app uses: a BSD Unix-domain stream socket.
///
/// It owns the socket descriptor rather than going through `NWConnection`,
/// which never exposes one, because the peer check (`SocketPeerCheck`) has to
/// ask the kernel about the very connection it then uses: a second connection
/// made only to check could reach a different process.
actor UnixSocketTransport: IPCTransportProtocol {
    private let peerCheck: SocketPeerCheck
    private var connection: UnixSocketConnection?
    private var bufferedData = Data()

    init(peerCheck: SocketPeerCheck = .forThisBuild) {
        self.peerCheck = peerCheck
    }

    deinit {
        connection?.close()
    }

    func connect(to path: String) async throws {
        guard FileManager.default.fileExists(atPath: path) else {
            throw IPCError.socket(code: ENOENT)
        }
        connection?.close()
        connection = try await UnixSocketConnection.connect(to: path, checkingPeerWith: peerCheck)
    }

    func send(frame: Data) async throws {
        guard let connection else {
            throw IPCError.connectionClosed
        }
        var framed = frame
        framed.append(0x0A)
        // A blocked write returns only when the socket is shut down, so that
        // is what cancelling the task does. Without it the connect timeout
        // could not fire during the handshake: its task group waits for every
        // child.
        try await withTaskCancellationHandler(
            operation: {
                try await connection.write(framed)
            },
            onCancel: {
                connection.shutdown()
            })
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
            // A peer that accepted the connection and then went silent parks
            // this read forever. Shutting the socket down is what returns it,
            // so the connect timeout can still unwind a stalled handshake.
            let chunk = try await withTaskCancellationHandler(
                operation: {
                    try await connection.read(maximumLength: 64 * 1024)
                },
                onCancel: {
                    connection.shutdown()
                })
            bufferedData.append(chunk)
        }
    }

    func close() async {
        connection?.close()
        connection = nil
        bufferedData.removeAll(keepingCapacity: false)
    }
}

/// One connected Unix-domain stream socket.
///
/// Reads and writes block on the connection's own serial queues, never on
/// Swift's cooperative pool, so a read parked on a silent peer holds one
/// thread and nothing else, and writes leave in the order they were sent.
/// `shutdown()` wakes whatever is blocked; the descriptor itself is closed
/// only once nothing is using it, so its number can never be reused under a
/// read that is still running.
final class UnixSocketConnection: @unchecked Sendable {
    private let descriptor: Int32
    private let lock = NSLock()
    private var operationsInFlight = 0
    private var isClosed = false
    private let readQueue = DispatchQueue(label: "com.velvt.mac.ipc.socket.read")
    private let writeQueue = DispatchQueue(label: "com.velvt.mac.ipc.socket.write")

    private init(descriptor: Int32) {
        self.descriptor = descriptor
    }

    deinit {
        if !isClosed {
            Darwin.close(descriptor)
        }
    }

    /// Connects to the socket at `path` and checks the process serving it.
    ///
    /// A refused connection fails at once with `ECONNREFUSED`, so the client's
    /// fast retry cadence covers a helper that has not bound the socket yet.
    static func connect(
        to path: String,
        checkingPeerWith peerCheck: SocketPeerCheck
    ) async throws -> UnixSocketConnection {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(with: Result { try connectBlocking(to: path, checkingPeerWith: peerCheck) })
            }
        }
    }

    private static func connectBlocking(
        to path: String,
        checkingPeerWith peerCheck: SocketPeerCheck
    ) throws -> UnixSocketConnection {
        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else {
            throw IPCError.socket(code: errno)
        }
        let connection = UnixSocketConnection(descriptor: descriptor)
        do {
            // A write to a peer that has gone returns EPIPE instead of
            // killing the app with SIGPIPE.
            var noSignal: Int32 = 1
            guard
                setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
                    == 0
            else {
                throw IPCError.socket(code: errno)
            }
            _ = fcntl(descriptor, F_SETFD, FD_CLOEXEC)

            var address = sockaddr_un()
            address.sun_family = sa_family_t(AF_UNIX)
            address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
            let pathBytes = Array(path.utf8)
            guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
                throw IPCError.socket(code: ENAMETOOLONG)
            }
            withUnsafeMutableBytes(of: &address.sun_path) { destination in
                destination.copyBytes(from: pathBytes)
            }
            let connected = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                    Darwin.connect(descriptor, socketAddress, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard connected == 0 else {
                throw IPCError.socket(code: errno)
            }

            do {
                try peerCheck.verifyPeer(ofSocket: descriptor)
            } catch IPCError.peerRejected(let code) {
                SocketPeerCheckLog.shared.error(
                    "IPC peer failed the code-signature check; connection dropped. error_code=ipc_peer_rejected code=\(code, privacy: .public)"
                )
                throw IPCError.peerRejected(code: code)
            }
            return connection
        } catch {
            connection.close()
            throw error
        }
    }

    func read(maximumLength: Int) async throws -> Data {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Data, Error>) in
            readQueue.async { [self] in
                continuation.resume(
                    with: withOpenDescriptor { descriptor in
                        var buffer = [UInt8](repeating: 0, count: maximumLength)
                        while true {
                            let count = Darwin.read(descriptor, &buffer, maximumLength)
                            if count > 0 {
                                return .success(Data(buffer[..<count]))
                            }
                            if count == 0 {
                                return .failure(IPCError.connectionClosed)
                            }
                            let code = errno
                            if code != EINTR {
                                return .failure(IPCError.socket(code: code))
                            }
                        }
                    })
            }
        }
    }

    func write(_ data: Data) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            writeQueue.async { [self] in
                continuation.resume(
                    with: withOpenDescriptor { descriptor in
                        data.withUnsafeBytes { bytes -> Result<Void, Error> in
                            guard let base = bytes.baseAddress else {
                                return .success(())
                            }
                            var offset = 0
                            while offset < bytes.count {
                                let written = Darwin.write(descriptor, base + offset, bytes.count - offset)
                                if written > 0 {
                                    offset += written
                                    continue
                                }
                                let code = written < 0 ? errno : EPIPE
                                if code != EINTR {
                                    return .failure(IPCError.socket(code: code))
                                }
                            }
                            return .success(())
                        }
                    })
            }
        }
    }

    /// Wakes a blocked read or write, which then fails. The descriptor stays
    /// open until `close()`.
    func shutdown() {
        lock.withLock {
            guard !isClosed else { return }
            _ = Darwin.shutdown(descriptor, SHUT_RDWR)
        }
    }

    func close() {
        let closeNow = lock.withLock { () -> Bool in
            guard !isClosed else { return false }
            isClosed = true
            _ = Darwin.shutdown(descriptor, SHUT_RDWR)
            return operationsInFlight == 0
        }
        if closeNow {
            Darwin.close(descriptor)
        }
    }

    private func withOpenDescriptor<T>(_ body: (Int32) -> Result<T, Error>) -> Result<T, Error> {
        let open = lock.withLock { () -> Bool in
            guard !isClosed else { return false }
            operationsInFlight += 1
            return true
        }
        guard open else {
            return .failure(IPCError.connectionClosed)
        }
        defer {
            let closeNow = lock.withLock { () -> Bool in
                operationsInFlight -= 1
                return isClosed && operationsInFlight == 0
            }
            if closeNow {
                Darwin.close(descriptor)
            }
        }
        return body(descriptor)
    }
}
