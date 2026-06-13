import Combine
import XCTest
@testable import VelvtMac

final class UnixSocketIPCClientTests: XCTestCase {
    private var cancellables: Set<AnyCancellable> = []

    func testHandshakeSuccessSendsClientHelloAndConnects() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 1)))),
                .success(try frame(.acknowledged(Acknowledged())))
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
            .clientHello(ClientHello(protocolVersion: 1, clientVersion: "1.0.0"))
        )
        XCTAssertEqual(statuses.suffix(3), [.connecting, .handshaking, .connected])
        client.disconnect()
    }

    func testVersionMismatchThrowsAndDisconnects() async throws {
        let transport = ScriptedIPCTransport(
            receives: [
                .success(try frame(.serverHello(ServerHello(protocolVersion: 2)))),
                .success(try frame(.versionMismatch(VersionMismatch(expected: 2, got: 1))))
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
                .failure(IPCError.connectionClosed)
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
                .success(Data(#"{"type":"future_message","raw_title":"not-retained"}"#.utf8)),
                .success(try frame(.serviceStatus(ServiceStatus(state: .ready, reason: nil))))
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
                .failure(IPCError.connectionClosed)
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
    private let connectError: Error?
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
        blockHandshakeResponse: Bool = false,
        blockPublicSend: Bool = false
    ) {
        self.connectError = connectError
        self.receives = receives
        self.blockHandshakeResponse = blockHandshakeResponse
        self.blockPublicSend = blockPublicSend
    }

    func connect(to path: String) async throws {
        if let connectError {
            throw connectError
        }
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
            try await Task.sleep(for: .seconds(60))
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
