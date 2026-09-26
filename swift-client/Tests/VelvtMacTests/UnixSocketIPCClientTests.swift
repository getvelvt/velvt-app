import Combine
import Darwin
import Network
import XCTest

@testable import VelvtMac

final class UnixSocketIPCClientTests: XCTestCase {
    private var cancellables: Set<AnyCancellable> = []

    func testWaitingConnectionRefusedFailsImmediatelyForFastHelperRetry() {
        let state = NWConnection.State.waiting(.posix(.ECONNREFUSED))

        XCTAssertEqual(
            UnixSocketTransport.connectionError(for: state),
            .socket(code: ECONNREFUSED)
        )
        XCTAssertNil(UnixSocketTransport.connectionError(for: .preparing))
    }

    func testHandshakeSuccessSendsClientHelloAndConnects() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 1)))),
                .success(try frame(.acknowledged(Acknowledged()))),
            ]
        )
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            transportFactory: { transport }
        )
        var statuses: [ConnectionStatus] = []
        client.connectionStatus.sink { statuses.append($0) }.store(in: &cancellables)

        try await client.connect()

        let sent = await transport.sentFrames()
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(
            try IPCMessageCodec.makeDecoder().decode(ClientMessage.self, from: sent[0]),
            .clientHello(ClientHello(expectedProtocolVersion: 1, clientVersion: "1.0.0"))
        )
        XCTAssertEqual(statuses.suffix(3), [.connecting, .handshaking, .connected])
        client.disconnect()
    }

    func testVersionMismatchThrowsAndDisconnects() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 2)))),
                .success(
                    try frame(.versionMismatch(VersionMismatch(serverProtocolVersion: 2, clientProtocolVersion: 1)))
                ),
            ]
        )
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            transportFactory: { transport }
        )
        var latestStatus = ConnectionStatus.disconnected
        client.connectionStatus.sink { latestStatus = $0 }.store(in: &cancellables)

        do {
            try await client.connect()
            XCTFail("Expected version mismatch")
        } catch {
            XCTAssertEqual(error as? IPCError, .versionMismatch(expected: 2, got: 1))
        }

        XCTAssertEqual(latestStatus, .disconnected)
    }

    func testReconnectPublishesDoublingDelaysAfterFailures() async throws {
        let initial = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 1)))),
                .success(try frame(.acknowledged(Acknowledged()))),
                .failure(IPCError.connectionClosed),
            ]
        )
        let failed = ScriptedIPCTransport(connectError: IPCError.socket(code: 61))
        let sleeper = RecordingSleeper(stopAfter: 3)
        let transports = TransportQueue([initial, failed, failed, failed])
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            backoff: ReconnectBackoff(jitter: { 1 }),
            sleeper: sleeper,
            transportFactory: { transports.next() }
        )

        try await client.connect()
        await fulfillment(of: [sleeper.completedExpectation], timeout: 1)

        let delays = await sleeper.delays()
        XCTAssertEqual(delays, [1, 2, 4])
        client.disconnect()
    }

    func testUnavailableServiceAtLaunchEntersReconnectingState() async {
        let transport = ScriptedIPCTransport(connectError: IPCError.socket(code: 61))
        let sleeper = RecordingSleeper(stopAfter: 1)
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            backoff: ReconnectBackoff(jitter: { 1 }),
            sleeper: sleeper,
            transportFactory: { transport }
        )
        var statuses: [ConnectionStatus] = []
        client.connectionStatus.sink { statuses.append($0) }.store(in: &cancellables)

        do {
            try await client.connect()
            XCTFail("Expected initial connection failure")
        } catch {}
        await fulfillment(of: [sleeper.completedExpectation], timeout: 1)

        XCTAssertTrue(statuses.contains(.reconnecting(attempt: 1, nextRetryIn: 1)))
        client.disconnect()
    }

    func testStalledInitialConnectTimesOutAndEntersReconnectingState() async {
        let transport = ScriptedIPCTransport(blockConnect: true)
        let sleeper = RecordingSleeper(stopAfter: 1)
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            connectionTimeout: .milliseconds(10),
            backoff: ReconnectBackoff(jitter: { 1 }),
            sleeper: sleeper,
            transportFactory: { transport }
        )
        var statuses: [ConnectionStatus] = []
        client.connectionStatus.sink { statuses.append($0) }.store(in: &cancellables)

        do {
            try await client.connect()
            XCTFail("Expected stalled connection to time out")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
        await fulfillment(of: [sleeper.completedExpectation], timeout: 1)

        XCTAssertTrue(statuses.contains(.reconnecting(attempt: 1, nextRetryIn: 1)))
        client.disconnect()
    }

    func testStalledHandshakeResponseTimesOutAndEntersReconnectingState() async throws {
        let transport = ScriptedIPCTransport(
            receives: [.success(try frame(.serverHello(ServerHello(protocolVersion: 1))))],
            blockHandshakeResponse: true
        )
        let sleeper = RecordingSleeper(stopAfter: 1)
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            connectionTimeout: .milliseconds(10),
            backoff: ReconnectBackoff(jitter: { 1 }),
            sleeper: sleeper,
            transportFactory: { transport }
        )
        var statuses: [ConnectionStatus] = []
        client.connectionStatus.sink { statuses.append($0) }.store(in: &cancellables)

        do {
            try await client.connect()
            XCTFail("Expected the stalled handshake to time out")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
        await fulfillment(of: [sleeper.completedExpectation], timeout: 1)

        XCTAssertTrue(statuses.contains(.reconnecting(attempt: 1, nextRetryIn: 1)))
        client.disconnect()
    }

    /// The connect timeout can only fire if the phase it is racing can be
    /// unwound. `NWConnection`'s completion handlers do not observe task
    /// cancellation, so a read parked on a peer that accepted and then went
    /// silent holds the whole task group until the transport tears the socket
    /// down itself.
    func testCancellingAParkedReceiveFrameTearsDownTheConnection() async throws {
        let listener = try SilentUnixSocketListener()
        defer { listener.close() }
        let transport = UnixSocketTransport()
        try await transport.connect(to: listener.path)

        let unwound = expectation(description: "receiveFrame returned after cancellation")
        let receive = Task {
            defer { unwound.fulfill() }
            _ = try? await transport.receiveFrame()
        }
        // Let the read park in its continuation first; cancelling before the
        // operation starts exercises the trivial path that already worked.
        try await Task.sleep(for: .milliseconds(100))
        receive.cancel()

        await fulfillment(of: [unwound], timeout: 2)
        await transport.close()
    }

    func testMissingSocketPathThrowsTypedSocketError() async {
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-definitely-missing/socket.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0"
        )

        do {
            try await client.connect()
            XCTFail("Expected socket error")
        } catch {
            guard case .socket = error as? IPCError else {
                return XCTFail("Expected typed socket error, got \(error)")
            }
        }
        client.disconnect()
    }

    func testUnknownServerMessageDoesNotCrashIncomingStream() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 1)))),
                .success(try frame(.acknowledged(Acknowledged()))),
                .success(Data(#"{"type":"future_message","payload":{"raw_title":"not-retained"}}"#.utf8)),
                .success(try frame(.serviceStatus(ServiceStatus(state: .ready, reason: nil)))),
            ]
        )
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            transportFactory: { transport }
        )
        let received = expectation(description: "stream continued after unknown message")

        Task {
            for await message in client.incomingMessages {
                if message == .serviceStatus(ServiceStatus(state: .ready, reason: nil)) {
                    received.fulfill()
                    break
                }
            }
        }

        try await client.connect()
        await fulfillment(of: [received], timeout: 1)
        client.disconnect()
    }

    func testSendWhileConnectingThrowsTypedError() async throws {
        let transport = ScriptedIPCTransport(
            receives: [.success(try frame(.serverHello(ServerHello(protocolVersion: 1))))],
            blockHandshakeResponse: true
        )
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            transportFactory: { transport }
        )
        let connectTask = Task { try await client.connect() }
        await fulfillment(of: [transport.handshakeBlockedExpectation], timeout: 1)

        do {
            try await client.send(.errorResponse(ErrorResponse(code: "safe", message: "safe", relatedEventID: nil)))
            XCTFail("Expected not-connected error")
        } catch {
            XCTAssertEqual(error as? IPCError, .notConnected)
        }

        connectTask.cancel()
        client.disconnect()
    }

    func testReconnectWaitsForInflightSendBeforeClosingTransport() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 1)))),
                .success(try frame(.acknowledged(Acknowledged()))),
                .failure(IPCError.connectionClosed),
            ],
            blockPublicSend: true
        )
        let failed = ScriptedIPCTransport(connectError: IPCError.socket(code: 61))
        let sleeper = RecordingSleeper(stopAfter: 1)
        let transports = TransportQueue([transport, failed])
        let client = UnixSocketIPCClient(
            socketPath: "/tmp/velvt-test.sock",
            protocolVersion: 1,
            clientVersion: "1.0.0",
            backoff: ReconnectBackoff(jitter: { 1 }),
            sleeper: sleeper,
            transportFactory: { transports.next() }
        )

        try await client.connect()
        let sendTask = Task {
            try await client.send(.errorResponse(ErrorResponse(code: "safe", message: "safe", relatedEventID: nil)))
        }
        await fulfillment(of: [transport.publicSendBlockedExpectation], timeout: 1)
        let closedWhileSending = await transport.wasClosed()
        XCTAssertFalse(closedWhileSending)

        await transport.releasePublicSend()
        try await sendTask.value
        await fulfillment(of: [sleeper.completedExpectation], timeout: 1)
        let closedAfterSending = await transport.wasClosed()
        XCTAssertTrue(closedAfterSending)
        client.disconnect()
    }

    private func frame(_ message: ServerMessage) throws -> Data {
        try IPCMessageCodec.makeEncoder().encode(message)
    }
}

private final class TransportQueue: @unchecked Sendable {
    private let lock = NSLock()
    private var transports: [any IPCTransportProtocol]

    init(_ transports: [any IPCTransportProtocol]) {
        self.transports = transports
    }

    func next() -> any IPCTransportProtocol {
        lock.withLock {
            transports.removeFirst()
        }
    }
}

private actor ScriptedIPCTransport: IPCTransportProtocol {
    nonisolated let handshakeBlockedExpectation = XCTestExpectation(description: "handshake response blocked")
    nonisolated let publicSendBlockedExpectation = XCTestExpectation(description: "public send blocked")
    private nonisolated let stallGate = StallGate()
    private let connectError: Error?
    private let blockConnect: Bool
    private let blockHandshakeResponse: Bool
    private let blockPublicSend: Bool
    private var receives: [Result<Data, Error>]
    private var sent: [Data] = []
    private var receiveCount = 0
    private var sendCount = 0
    private var publicSendContinuation: CheckedContinuation<Void, Never>?
    private var connectionLossContinuation: CheckedContinuation<Void, Never>?
    private var closed = false

    init(
        connectError: Error? = nil,
        receives: [Result<Data, Error>] = [],
        blockConnect: Bool = false,
        blockHandshakeResponse: Bool = false,
        blockPublicSend: Bool = false
    ) {
        self.connectError = connectError
        self.receives = receives
        self.blockConnect = blockConnect
        self.blockHandshakeResponse = blockHandshakeResponse
        self.blockPublicSend = blockPublicSend
    }

    func connect(to path: String) async throws {
        if let connectError {
            throw connectError
        }
        if blockConnect {
            try await stall()
        }
    }

    /// Parks the way an `NWConnection` completion handler does: task
    /// cancellation on its own cannot resume it, only an explicit socket
    /// teardown can. `Task.sleep` unwinds for free, so a transport phase that
    /// never installed a cancellation handler would still pass a test built on
    /// it.
    private func stall() async throws {
        try await withTaskCancellationHandler(
            operation: {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    stallGate.park(continuation)
                }
            },
            onCancel: { [stallGate] in
                // The production analogue is `connection.cancel()` in
                // UnixSocketTransport's own onCancel handler.
                stallGate.tearDown()
            })
    }

    func send(frame: Data) async throws {
        sendCount += 1
        if blockPublicSend, sendCount > 1 {
            publicSendBlockedExpectation.fulfill()
            connectionLossContinuation?.resume()
            connectionLossContinuation = nil
            await withCheckedContinuation { publicSendContinuation = $0 }
        }
        sent.append(frame)
    }

    func receiveFrame() async throws -> Data {
        receiveCount += 1
        if blockHandshakeResponse, receiveCount > 1 {
            handshakeBlockedExpectation.fulfill()
            try await stall()
        }
        if blockPublicSend, receiveCount > 2 {
            await withCheckedContinuation { connectionLossContinuation = $0 }
        }
        guard !receives.isEmpty else {
            try await Task.sleep(for: .seconds(60))
            throw CancellationError()
        }
        return try receives.removeFirst().get()
    }

    func close() async {
        closed = true
    }

    func sentFrames() -> [Data] {
        sent
    }

    func releasePublicSend() {
        publicSendContinuation?.resume()
        publicSendContinuation = nil
    }

    func wasClosed() -> Bool {
        closed
    }
}

/// Models an `NWConnection` completion handler: a continuation that only an
/// explicit teardown resumes.
private final class StallGate: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Error>?
    private var tornDown = false

    func park(_ continuation: CheckedContinuation<Void, Error>) {
        let resumeImmediately = lock.withLock { () -> Bool in
            guard !tornDown else { return true }
            self.continuation = continuation
            return false
        }
        if resumeImmediately {
            continuation.resume(throwing: IPCError.connectionClosed)
        }
    }

    func tearDown() {
        let parked = lock.withLock { () -> CheckedContinuation<Void, Error>? in
            tornDown = true
            defer { continuation = nil }
            return continuation
        }
        parked?.resume(throwing: IPCError.connectionClosed)
    }
}

private actor RecordingSleeper: IPCSleeping {
    nonisolated let completedExpectation = XCTestExpectation(description: "recorded reconnect delays")
    private let stopAfter: Int
    private var recorded: [TimeInterval] = []

    init(stopAfter: Int) {
        self.stopAfter = stopAfter
    }

    func sleep(for delay: TimeInterval) async throws {
        recorded.append(delay)
        if recorded.count == stopAfter {
            completedExpectation.fulfill()
            throw CancellationError()
        }
    }

    func delays() -> [TimeInterval] {
        recorded
    }
}

/// A Unix-domain socket that accepts a connection and then says nothing — the
/// peer shape a stalled handshake actually has: a helper whose runtime is
/// starved or stopped, or a foreign process squatting on the socket path.
private final class SilentUnixSocketListener {
    let path: String
    private let listeningDescriptor: Int32
    private let acceptQueue = DispatchQueue(label: "com.velvt.mac.tests.silent-listener")
    private let lock = NSLock()
    private var acceptedDescriptor: Int32 = -1

    init() throws {
        path = "/tmp/velvt-silent-\(UUID().uuidString.prefix(8)).sock"
        listeningDescriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listeningDescriptor >= 0 else { throw ListenerError.socketUnavailable }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let pathBytes = Array(path.utf8)
        guard pathBytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
            throw ListenerError.socketUnavailable
        }
        withUnsafeMutableBytes(of: &address.sun_path) { destination in
            destination.copyBytes(from: pathBytes)
        }

        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                bind(listeningDescriptor, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(listeningDescriptor, 1) == 0 else {
            Darwin.close(listeningDescriptor)
            throw ListenerError.socketUnavailable
        }

        acceptQueue.async { [listeningDescriptor, lock] in
            let accepted = accept(listeningDescriptor, nil, nil)
            guard accepted >= 0 else { return }
            // Hold the accepted end open and never write to it, so the client's
            // read has a live connection with nothing to read.
            lock.withLock { self.acceptedDescriptor = accepted }
        }
    }

    func close() {
        let accepted = lock.withLock { () -> Int32 in
            defer { acceptedDescriptor = -1 }
            return acceptedDescriptor
        }
        if accepted >= 0 {
            Darwin.close(accepted)
        }
        Darwin.close(listeningDescriptor)
        unlink(path)
    }

    enum ListenerError: Error {
        case socketUnavailable
    }
}
