import Combine
import Darwin
import Foundation
import Security
import XCTest

@testable import VelvtMac

/// PR A0 (`docs/architecture/personal-vault-and-devices.md` §9.3): in a Release
/// build the app uses the IPC socket only when the process serving it is a
/// child of this app and the helper its own team signed. The helper makes the
/// mirror check (`rust-service/src/ipc/peer.rs`, whose tests pin the same
/// requirement text).
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
        XCTAssertEqual(
            SocketPeerCheck.sameSigner(peerIdentifier: "com.velvt.service", process: .childOfThisApp).policy,
            .required(expected, process: .childOfThisApp)
        )
    }

    /// `swift test` builds Debug, as a developer's app is: nothing is checked.
    func testDebugBuildsSkipTheCheck() {
        XCTAssertEqual(SocketPeerCheck.forThisBuild.policy, .skipped)
        XCTAssertEqual(SocketPeerCheck.helperIdentifier, "com.velvt.service")
    }

    // MARK: - The check, over a real socket

    /// The test runner stands in for anything that is not the signed helper:
    /// it is not signed as `com.velvt.service`, by Velvt's team or at all. It
    /// is refused under both forms of the Release requirement, even as the
    /// expected process, and the connection is closed before the app sends a
    /// byte.
    func testAServerThatIsNotTheSignedHelperIsRefused() async throws {
        for requirement in [
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: "ABCDE12345"),
            SocketPeerCheck.requirement(peerIdentifier: "com.velvt.service", teamIdentifier: nil),
        ] {
            let server = try OneConnectionUnixSocketServer()
            defer { server.close() }
            let transport = UnixSocketTransport(
                peerCheck: SocketPeerCheck(
                    policy: .required(try XCTUnwrap(requirement), process: .process(getpid()))))

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

    /// The same check passes a server that is the expected process and
    /// satisfies the requirement, and the connection then carries frames both
    /// ways: the refusal above is the requirement's doing.
    func testAServerThatSatisfiesTheRequirementIsUsed() async throws {
        let identifier = try XCTUnwrap(SigningIdentity.ofThisProcess().get().identifier)
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let transport = UnixSocketTransport(
            peerCheck: SocketPeerCheck(policy: .required("identifier \"\(identifier)\"", process: .process(getpid()))))

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
    /// is refused as the helper. It is this test's child, as the helper is the
    /// app's, so the Release pin lets it through to the signature check.
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

            let transport = UnixSocketTransport(
                peerCheck: SocketPeerCheck(policy: .required(requirement, process: .childOfThisApp)))
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

    /// The token names the process at the other end. Here both ends are this
    /// process, which is not its own child: the Release pin refuses it.
    func testTheTokenNamesTheServingProcessAndThisProcessIsNotItsOwnChild() throws {
        let server = try OneConnectionUnixSocketServer()
        defer { server.close() }
        let descriptor = try connectedSocket(to: server.path)
        defer { Darwin.close(descriptor) }

        let token = try SocketPeerCheck.peerToken(ofSocket: descriptor)
        XCTAssertEqual(SocketPeerCheck.processIdentifier(of: token), getpid())
        XCTAssertThrowsError(try SocketPeerCheck.check(getpid(), is: .childOfThisApp)) { error in
            XCTAssertEqual(error as? IPCError, .peerRejected(code: SocketPeerCheck.unexpectedProcessCode))
        }
        XCTAssertNoThrow(try SocketPeerCheck.check(getpid(), is: .process(getpid())))
        XCTAssertThrowsError(try SocketPeerCheck.check(getpid(), is: .process(getpid() + 1)))
    }

    /// The attack the signature alone does not stop (review of PR #88). A
    /// process that is not this app's child binds the socket path, forks a
    /// child that keeps the listening socket and reads whatever arrives, and
    /// execs `/bin/sleep`. Every process involved is Apple's own code, so the
    /// signature check alone passes under `anchor apple`, whichever of them
    /// the token names. The Release pin refuses it, and the forked child is
    /// sent nothing.
    func testASquatterThatForksAndExecsASignedBinaryIsRefused() async throws {
        try XCTSkipUnless(zshHasSockets(), "/bin/zsh with zsh/net/socket is not available")
        let stem = "/tmp/velvt-squat-\(UUID().uuidString.prefix(8))"
        let path = stem + ".sock"
        let received = stem + ".received"
        let processes = stem + ".pids"
        let finished = stem + ".finished"
        // Our child zsh starts a subshell and exits, so the subshell, which
        // binds the path, is nobody's child here. It forks a reader that
        // accepts twice and reads each connection to its end, then execs
        // /bin/sleep.
        let launcher = Process()
        launcher.executableURL = URL(fileURLWithPath: "/bin/zsh")
        launcher.arguments = [
            "-fc",
            """
            zmodload zsh/net/socket zsh/system || exit 3
            (
              zsocket -l "$1" || exit 4
              listener=$REPLY
              ( repeat 2 { zsocket -a $listener && cat <&$REPLY >> "$2" }; print > "$4" ) &
              print $sysparams[pid] $! > "$3"
              exec /bin/sleep 10
            ) &!
            """,
            "zsh", path, received, processes, finished,
        ]
        launcher.standardInput = FileHandle.nullDevice
        launcher.standardOutput = FileHandle.nullDevice
        launcher.standardError = FileHandle.nullDevice
        try launcher.run()
        launcher.waitUntilExit()
        defer {
            let pids = (try? String(contentsOfFile: processes, encoding: .utf8)) ?? ""
            for pid in pids.split(whereSeparator: \.isWhitespace).compactMap({ pid_t($0) }) {
                kill(pid, SIGKILL)
            }
            unlink(path)
            unlink(received)
            unlink(processes)
            unlink(finished)
        }
        let deadline = Date().addingTimeInterval(5)
        while !(FileManager.default.fileExists(atPath: path) && FileManager.default.fileExists(atPath: processes)),
            Date() < deadline
        {
            try await Task.sleep(for: .milliseconds(20))
        }

        // The signature alone is fooled.
        let probe = try connectedSocket(to: path)
        let token = try SocketPeerCheck.peerToken(ofSocket: probe)
        XCTAssertNoThrow(try SocketPeerCheck.checkSignature(of: token, against: "anchor apple"))
        let squatter = SocketPeerCheck.processIdentifier(of: token)
        XCTAssertThrowsError(try SocketPeerCheck.check(squatter, is: .childOfThisApp))
        Darwin.close(probe)

        // The Release check is not.
        let transport = UnixSocketTransport(
            peerCheck: SocketPeerCheck(policy: .required("anchor apple", process: .childOfThisApp)))
        do {
            try await transport.connect(to: path)
            XCTFail("A squatter that is not this app's child was used")
        } catch {
            XCTAssertEqual(error as? IPCError, .peerRejected(code: SocketPeerCheck.unexpectedProcessCode))
        }
        await transport.close()

        // The reader finishes only once both connections have been closed.
        let readerDeadline = Date().addingTimeInterval(5)
        while !FileManager.default.fileExists(atPath: finished), Date() < readerDeadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: finished), "a connection was left open")
        let sent = (try? Data(contentsOf: URL(fileURLWithPath: received))) ?? Data()
        XCTAssertEqual(sent, Data(), "the squatter's child was sent something")
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
    /// An orphaned helper fails the peer check before it can answer with its
    /// version: it is not this app's child, and after an update its code has
    /// changed on disk too. It is reclaimed like the version-mismatch orphan
    /// it is, and the app dials again.
    func testAPeerRejectedAtTheFirstDialReclaimsTheOrphanAndRedials() async {
        for code in [SocketPeerCheck.unexpectedProcessCode, -67034] {
            let client = ScriptedConnectClient(failures: [IPCError.peerRejected(code: code)])
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

            XCTAssertEqual(reclaims.value, 1, "code \(code)")
            XCTAssertEqual(client.connectCount, 2, "code \(code)")
            XCTAssertEqual(prompts.value, 0, "a refused peer is never a reason to ask the person anything")
        }
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

final class AbandonableBlockingCallTests: XCTestCase {
    /// A connect or peer check that never returns cannot hold the connect
    /// timeout, or `close()`, hostage: abandoning returns at once, and what the
    /// blocked work hands back later is discarded.
    func testAnAbandonedCallReturnsAtOnceAndDiscardsTheLateValue() async throws {
        let release = DispatchSemaphore(value: 0)
        let discarded = expectation(description: "the late value is discarded")
        let call = AbandonableBlockingCall<Int>(discarding: { value in
            XCTAssertEqual(value, 7)
            discarded.fulfill()
        })
        let waiting = Task {
            try await call.run {
                release.wait()
                return 7
            }
        }
        try await Task.sleep(for: .milliseconds(50))
        call.abandon()
        do {
            _ = try await waiting.value
            XCTFail("an abandoned call returned a value")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
        release.signal()
        await fulfillment(of: [discarded], timeout: 5)
    }

    func testCancellingTheCallerReturnsAtOnce() async throws {
        let release = DispatchSemaphore(value: 0)
        let discarded = expectation(description: "the late value is discarded")
        let call = AbandonableBlockingCall<Int>(discarding: { _ in discarded.fulfill() })
        let waiting = Task {
            try await call.run {
                release.wait()
                return 1
            }
        }
        try await Task.sleep(for: .milliseconds(50))
        waiting.cancel()
        do {
            _ = try await waiting.value
            XCTFail("a cancelled call returned a value")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
        release.signal()
        await fulfillment(of: [discarded], timeout: 5)
    }

    func testACallAbandonedBeforeItRunsFailsWithoutRunning() async {
        let call = AbandonableBlockingCall<Int>(discarding: { _ in XCTFail("nothing ran") })
        call.abandon()
        do {
            _ = try await call.run {
                XCTFail("an abandoned call ran its work")
                return 0
            }
            XCTFail("an abandoned call returned a value")
        } catch {
            XCTAssertEqual(error as? IPCError, .connectionClosed)
        }
    }

    func testACompletedCallReturnsItsValueAndItsError() async throws {
        let call = AbandonableBlockingCall<Int>(discarding: { _ in XCTFail("the value was wanted") })
        let value = try await call.run { 42 }
        XCTAssertEqual(value, 42)

        let failing = AbandonableBlockingCall<Int>(discarding: { _ in })
        do {
            _ = try await failing.run { throw IPCError.socket(code: ECONNREFUSED) }
            XCTFail("a failed call returned a value")
        } catch {
            XCTAssertEqual(error as? IPCError, .socket(code: ECONNREFUSED))
        }
    }
}

// MARK: - Test doubles

/// A blocking client socket connected to `path`.
private func connectedSocket(to path: String) throws -> Int32 {
    let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
    guard descriptor >= 0 else { throw IPCError.socket(code: errno) }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: Array(path.utf8)) }
    let connected = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard connected == 0 else {
        let code = errno
        Darwin.close(descriptor)
        throw IPCError.socket(code: code)
    }
    return descriptor
}

/// `/bin/zsh` with its socket module, which every supported macOS ships.
private func zshHasSockets() -> Bool {
    let probe = Process()
    probe.executableURL = URL(fileURLWithPath: "/bin/zsh")
    probe.arguments = ["-fc", "zmodload zsh/net/socket zsh/system"]
    probe.standardOutput = FileHandle.nullDevice
    probe.standardError = FileHandle.nullDevice
    guard (try? probe.run()) != nil else { return false }
    probe.waitUntilExit()
    return probe.terminationStatus == 0
}

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
