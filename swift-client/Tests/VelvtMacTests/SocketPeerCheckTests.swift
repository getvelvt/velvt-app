import Combine
import Darwin
import Foundation
import Security
import XCTest

@testable import VelvtMac

/// PR A0 (`docs/architecture/personal-vault-and-devices.md` §9.3): in a Release
/// build the app uses the IPC socket only when the process serving it is the
/// helper its own team signed. The helper makes the mirror check
/// (`rust-service/src/ipc/peer.rs`, whose tests pin the same requirement text).
final class SocketPeerCheckTests: XCTestCase {

    // MARK: - The requirement

    func testATeamSignedBuildPinsTheTeamAndTheIdentifier() {
        XCTAssertEqual(
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: "ABCDE12345"),
            "anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" and identifier \"com.velvt.service\""
        )
    }

    func testAnAdHocBuildHasNoTeamToPinAndRequiresTheIdentifier() {
        XCTAssertEqual(
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: nil),
            "identifier \"com.velvt.service\""
        )
    }

    /// The team comes from this app's own signature and is spliced into
    /// requirement language, so anything not shaped like a team identifier is
    /// refused rather than quoted.
    func testATeamOrIdentifierThatCannotBeQuotedIsRefused() {
        for team in ["", "abcde12345", "ABCDE1234", "ABCDE123456", "ABCDE\"1234", "ABCDE 1234"] {
            XCTAssertNil(
                SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: team),
                "team \(team)"
            )
        }
        for identifier in ["", "com.velvt.service\" or anchor apple", "com velvt"] {
            XCTAssertNil(
                SocketPeerCheck.requirement(peerIdentifier: identifier, teamIdentifier: nil),
                "identifier \(identifier)"
            )
        }
    }

    /// What a Release build checks: the helper's identifier, and this
    /// process's own team when it has one.
    func testTheReleaseCheckIsBuiltFromThisProcesssOwnSignature() throws {
        let identity = try SigningIdentity.ofThisProcess().get()
        let expected = try XCTUnwrap(
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: identity.teamIdentifier))
        XCTAssertEqual(SocketPeerCheck.sameSigner(peerIdentifier: "com.velvt.service").policy, .required(expected))
    }

    /// `swift test` builds Debug, as a developer's app is: nothing is checked.
    func testDebugBuildsSkipTheCheck() {
        XCTAssertEqual(SocketPeerCheck.forThisBuild.policy, .skipped)
        XCTAssertEqual(SocketPeerCheck.helperIdentifier, "com.velvt.service")
    }

    // MARK: - The check, over a real socket

    /// The test runner stands in for anything that is not the signed helper:
    /// it is not signed as `com.velvt.service`, by Velvt's team or at all. It
    /// is refused under both forms of the Release requirement, and the
    /// connection is closed before the app sends a byte.
    func testAServerThatIsNotTheSignedHelperIsRefused() async throws {
        for requirement in [
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: "ABCDE12345"),
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: nil),
        ] {
            let server = try OneConnectionUnixSocketServer()
            defer { server.close() }
            let transport = UnixSocketTransport(
                peerCheck: SocketPeerCheck(policy: .required(try XCTUnwrap(requirement))))

            do {
                try await transport.connect(to: server.path)
                XCTFail("A server that is not the signed helper was used")
            } catch {
                XCTAssertEqual(error as? IPCError, .peerRejected(code: errSecCSReqFailed))
            }
            XCTAssertEqual(server.waitForEndOfStream(), Data(), "the refused server was sent something")
            await transport.close()
        }
    }

    /// The same check passes a server that satisfies the requirement, and the
    /// connection then carries frames both ways: the refusal above is the
    /// requirement's doing.
    func testAServerThatSatisfiesTheRequirementIsUsed() async throws {
        let identifier = try XCTUnwrap(SigningIdentity.ofThisProcess().get().identifier)
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let transport = UnixSocketTransport(
            peerCheck: SocketPeerCheck(policy: .required("identifier \"\(identifier)\"")))

        try await transport.connect(to: server.path)
        server.write("{\"type\":\"server_hello\"}\n")
        let frame = try await transport.receiveFrame()
        XCTAssertEqual(String(decoding: frame, as: UTF8.self), "{\"type\":\"server_hello\"}")
        try await transport.send(frame: Data("client_hello".utf8))
        await transport.close()
        XCTAssertEqual(String(decoding: server.waitForEndOfStream(), as: UTF8.self), "client_hello\n")
    }

    /// The audit token names the process serving the socket, not the app: a
    /// separate, Apple-signed `nc` listening on it passes as Apple's code and
    /// is refused as the helper.
    func testTheCheckIsMadeOnTheServingProcess() async throws {
        let netcat = "/usr/bin/nc"
        try XCTSkipUnless(FileManager.default.isExecutableFile(atPath: netcat), "nc is not installed")
        for (requirement, passes) in [("anchor apple", true), ("identifier \"com.velvt.service\"", false)] {
            let path = "/tmp/velvt-nc-\(UUID().uuidString.prefix(8)).sock"
            let process = Process()
            process.executableURL = URL(fileURLWithPath: netcat)
            process.arguments = ["-lU", path]
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            defer {
                process.terminate()
                process.waitUntilExit()
                unlink(path)
            }
            let deadline = Date().addingTimeInterval(5)
            while !FileManager.default.fileExists(atPath: path), Date() < deadline {
                try await Task.sleep(for: .milliseconds(20))
            }

            let transport = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .required(requirement)))
            do {
                try await transport.connect(to: path)
                XCTAssertTrue(passes, "nc passed as \(requirement)")
            } catch {
                XCTAssertFalse(passes, "nc was refused as \(requirement): \(error)")
                XCTAssertEqual(error as? IPCError, .peerRejected(code: errSecCSReqFailed))
            }
            await transport.close()
        }
    }

    /// A Release build that cannot read its own signature trusts nobody.
    func testABuildThatCannotReadItsOwnSignatureUsesNoServer() async throws {
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let transport = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .unavailable(code: -67062)))
        do {
            try await transport.connect(to: server.path)
            XCTFail("A server was used with no requirement to check it against")
        } catch {
            XCTAssertEqual(error as? IPCError, .peerRejected(code: -67062))
        }
    }

    // MARK: - The transport the check needed

    /// The app no longer goes through `NWConnection`, which handled SIGPIPE for
    /// it. A write to a helper that has gone must fail, not kill the app.
    func testWritingToAServerThatHasGoneFailsInsteadOfRaisingSIGPIPE() async throws {
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let transport = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .skipped))
        try await transport.connect(to: server.path)
        server.closeConnection()

        var failure: IPCError?
        for _ in 0..<10 where failure == nil {
            do {
                try await transport.send(frame: Data(repeating: 0x61, count: 64 * 1024))
            } catch {
                failure = error as? IPCError
            }
        }
        XCTAssertEqual(failure, .socket(code: EPIPE))
        await transport.close()
    }

    /// A server that closes the connection ends the read with
    /// `connectionClosed`, as `NWConnection`'s completed receive did.
    func testAServerClosingTheConnectionEndsTheRead() async throws {
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let transport = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .skipped))
        try await transport.connect(to: server.path)
        server.write("partial frame with no newline")
        server.closeConnection()
        do {
            _ = try await transport.receiveFrame()
            XCTFail("A closed connection produced a frame")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
        await transport.close()
    }

    /// Frames larger than the socket buffer, sent one after another and all at
    /// once, each arrive whole: one send's bytes never land inside another's.
    /// Sent one after another, they also keep their order.
    func testSendsArriveWholeAndSequentialSendsInOrder() async throws {
        let frames = (0..<20).map { index in
            String(repeating: Character("\(index % 10)"), count: 20_000) + "#\(index)"
        }

        let sequentialServer = try OneConnectionUnixSocketServer()
        defer { sequentialServer.close() }
        let sequential = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .skipped))
        try await sequential.connect(to: sequentialServer.path)
        let sequentialBytes = Task.detached { sequentialServer.waitForEndOfStream() }
        for frame in frames {
            try await sequential.send(frame: Data(frame.utf8))
        }
        await sequential.close()
        let inOrder = String(decoding: await sequentialBytes.value, as: UTF8.self)
        XCTAssertEqual(inOrder, frames.map { $0 + "\n" }.joined())

        let concurrentServer = try OneConnectionUnixSocketServer()
        defer { concurrentServer.close() }
        let concurrent = UnixSocketTransport(peerCheck: SocketPeerCheck(policy: .skipped))
        try await concurrent.connect(to: concurrentServer.path)
        let concurrentBytes = Task.detached { concurrentServer.waitForEndOfStream() }
        try await withThrowingTaskGroup(of: Void.self) { group in
            for frame in frames {
                group.addTask { try await concurrent.send(frame: Data(frame.utf8)) }
            }
            try await group.waitForAll()
        }
        await concurrent.close()
        let received = String(decoding: await concurrentBytes.value, as: UTF8.self)
        XCTAssertEqual(
            received.split(separator: "\n").map(String.init).sorted(),
            frames.sorted()
        )
    }
}

final class PeerRejectedRecoveryTests: XCTestCase {
    /// A helper from before an update fails the peer check before it can
    /// answer with its version. It is reclaimed like the version-mismatch
    /// orphan it is, and the app dials again.
    func testAPeerRejectedAtTheFirstDialReclaimsTheOrphanAndRedials() async {
        let client = ScriptedConnectClient(failures: [IPCError.peerRejected(code: -67034)])
        let reclaims = Counter()
        let prompts = Counter()

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: {
                reclaims.increment()
                return true
            },
            presentVersionMismatch: { _, _ in
                prompts.increment()
                return false
            }
        )

        XCTAssertEqual(reclaims.value, 1)
        XCTAssertEqual(client.connectCount, 2)
        XCTAssertEqual(prompts.value, 0, "a refused peer is never a reason to ask the person anything")
    }

    /// Anything this app cannot identify as its own helper stays refused, and
    /// the client's own reconnect is left to keep trying.
    func testAPeerRejectedWithNothingToReclaimIsLeftToTheReconnect() async {
        let client = ScriptedConnectClient(failures: [IPCError.peerRejected(code: -67050)])
        let reclaims = Counter()
        let prompts = Counter()

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: {
                reclaims.increment()
                return false
            },
            presentVersionMismatch: { _, _ in
                prompts.increment()
                return true
            }
        )

        XCTAssertEqual(reclaims.value, 1)
        XCTAssertEqual(client.connectCount, 1)
        XCTAssertEqual(prompts.value, 0)
    }

    /// The reclaim is bounded, as it is for a version mismatch.
    func testAPeerThatStaysRejectedIsReclaimedAFiniteNumberOfTimes() async {
        let client = ScriptedConnectClient(alwaysFailWith: IPCError.peerRejected(code: -67034))
        let reclaims = Counter()

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: {
                reclaims.increment()
                return true
            },
            presentVersionMismatch: { _, _ in false }
        )

        XCTAssertEqual(reclaims.value, AppDelegate.maximumOrphanReclaimAttempts)
        XCTAssertEqual(client.connectCount, AppDelegate.maximumOrphanReclaimAttempts + 1)
    }
}

// MARK: - Test doubles

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    var value: Int { lock.withLock { count } }

    func increment() {
        lock.withLock { count += 1 }
    }
}

private final class ScriptedConnectClient: IPCClientProtocol, @unchecked Sendable {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        Just(.disconnected).eraseToAnyPublisher()
    }

    private let lock = NSLock()
    private var failures: [any Error]
    private let persistentFailure: (any Error)?
    private var dials = 0

    var connectCount: Int { lock.withLock { dials } }

    init(failures: [any Error]) {
        self.failures = failures
        persistentFailure = nil
    }

    init(alwaysFailWith failure: any Error) {
        failures = []
        persistentFailure = failure
    }

    func connect() async throws {
        let failure = lock.withLock { () -> (any Error)? in
            dials += 1
            if let persistentFailure {
                return persistentFailure
            }
            return failures.isEmpty ? nil : failures.removeFirst()
        }
        if let failure {
            throw failure
        }
    }

    func disconnect() {}

    func send(_ message: ClientMessage) async throws {}
}

/// A Unix-domain socket in this process that accepts one connection, under a
/// short `/tmp` path (`sockaddr_un` holds 104 bytes; the per-user temp
/// directory is long).
private final class OneConnectionUnixSocketServer: @unchecked Sendable {
    let path: String
    private let listeningDescriptor: Int32
    private let lock = NSLock()
    private var acceptedDescriptor: Int32 = -1
    private let accepted = DispatchSemaphore(value: 0)

    init() throws {
        path = "/tmp/velvt-peer-\(UUID().uuidString.prefix(8)).sock"
        listeningDescriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listeningDescriptor >= 0 else { throw ServerError.unavailable }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: Array(path.utf8)) }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(listeningDescriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(listeningDescriptor, 1) == 0 else {
            Darwin.close(listeningDescriptor)
            throw ServerError.unavailable
        }
        DispatchQueue(label: "com.velvt.mac.tests.peer-server").async { [self] in
            let descriptor = accept(listeningDescriptor, nil, nil)
            lock.withLock { acceptedDescriptor = descriptor }
            accepted.signal()
        }
    }

    private func connection() -> Int32 {
        _ = accepted.wait(timeout: .now() + 5)
        accepted.signal()
        return lock.withLock { acceptedDescriptor }
    }

    func write(_ text: String) {
        let descriptor = connection()
        let bytes = Array(text.utf8)
        _ = bytes.withUnsafeBytes { Darwin.write(descriptor, $0.baseAddress, $0.count) }
    }

    /// Everything the client sent, read until it closes its end.
    func waitForEndOfStream() -> Data {
        let descriptor = connection()
        guard descriptor >= 0 else { return Data() }
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var received = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let count = Darwin.read(descriptor, &buffer, buffer.count)
            guard count > 0 else { break }
            received.append(contentsOf: buffer[..<count])
        }
        return received
    }

    func closeConnection() {
        let descriptor = connection()
        lock.withLock { acceptedDescriptor = -1 }
        if descriptor >= 0 {
            Darwin.close(descriptor)
        }
    }

    func close() {
        let descriptor = lock.withLock { () -> Int32 in
            defer { acceptedDescriptor = -1 }
            return acceptedDescriptor
        }
        if descriptor >= 0 {
            Darwin.close(descriptor)
        }
        Darwin.close(listeningDescriptor)
        unlink(path)
    }

    enum ServerError: Error {
        case unavailable
    }
}
