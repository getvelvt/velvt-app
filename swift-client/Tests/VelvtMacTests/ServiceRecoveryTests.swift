import Combine
import XCTest

@testable import VelvtMac

// MARK: - IPC version mismatch retry

/// `versionMismatch` is the one `IPCError` the client does not arm a reconnect
/// for, so the only way back is an explicit re-dial from the app.
final class IPCVersionMismatchRetryTests: XCTestCase {

    func testAcceptedRetryDialsTheSocketAgain() async {
        let client = ScriptedConnectIPCClient(failures: [IPCError.versionMismatch(expected: 3, got: 2)])
        let prompt = VersionMismatchPromptRecorder(answer: true)

        await AppDelegate.connectRetryingVersionMismatch(client) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(prompt.count, 1)
        XCTAssertEqual(prompt.lastExpected, 3)
        XCTAssertEqual(prompt.lastGot, 2)
        XCTAssertEqual(client.connectCount, 2, "the retry has to dial again, not just dismiss the alert")
    }

    func testDeclinedRetryStopsAtOneDial() async {
        let client = ScriptedConnectIPCClient(failures: [IPCError.versionMismatch(expected: 3, got: 2)])
        let prompt = VersionMismatchPromptRecorder(answer: false)

        await AppDelegate.connectRetryingVersionMismatch(client) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(prompt.count, 1)
        XCTAssertEqual(client.connectCount, 1)
    }

    func testTransportFailureIsLeftToTheClientsOwnReconnect() async {
        let client = ScriptedConnectIPCClient(failures: [IPCError.socket(code: 61)])
        let prompt = VersionMismatchPromptRecorder(answer: true)

        await AppDelegate.connectRetryingVersionMismatch(client) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(prompt.count, 0, "only a version mismatch is the app's problem to re-dial")
        XCTAssertEqual(client.connectCount, 1)
    }
}

// MARK: - Service launcher re-arm and helper diagnostics

@MainActor
final class ServiceProcessLauncherRearmTests: XCTestCase {

    func testSpentRelaunchBudgetReArmsOnTheSlowTimerRatherThanLatchingOff() async {
        let harness = RearmLauncherHarness()
        let launcher = harness.makeLauncher()
        launcher.start()
        XCTAssertEqual(harness.launchCount, 1)

        // Burn the single fast attempt.
        harness.processes[0].exit(status: 1)
        await Task.yield()
        harness.run(delay: 0.5)
        XCTAssertEqual(harness.launchCount, 2)

        // Budget spent. No fast retry remains, but the helper is not abandoned
        // for the lifetime of the app: a cause that clears on its own gets
        // another chance on the slow timer.
        harness.processes[1].exit(status: 1)
        await Task.yield()
        XCTAssertFalse(harness.hasScheduled(delay: 0.5))
        harness.run(delay: 120)
        XCTAssertEqual(harness.launchCount, 3)

        // The fast budget is armed again for the next crash.
        harness.processes[2].exit(status: 1)
        await Task.yield()
        XCTAssertTrue(harness.hasScheduled(delay: 0.5))
    }

    func testStopInvalidatesAPendingSlowReArm() async {
        let harness = RearmLauncherHarness()
        let launcher = harness.makeLauncher()
        launcher.start()
        harness.processes[0].exit(status: 1)
        await Task.yield()
        harness.run(delay: 0.5)
        harness.processes[1].exit(status: 1)
        await Task.yield()

        launcher.stop()
        harness.run(delay: 120)

        XCTAssertEqual(harness.launchCount, 2, "quitting must never race a delayed re-arm")
    }

    func testHelperDiagnosticCarriesTheErrorCodeAndLeavesTheRestRedacted() {
        let line = """
            2026-08-31T09:00:00.000000Z ERROR velvt_service: another velvt-service instance is \
            already listening on this socket; exiting error_code="duplicate_service_instance"
            """

        let codes = ServiceProcessLauncher.errorCodes(inPipeChunk: line)
        XCTAssertEqual(codes, ["duplicate_service_instance"])

        let diagnostic = ServiceProcessLauncher.redactedPipeDiagnostic(
            label: "stderr",
            byteCount: line.utf8.count,
            errorCodes: codes
        )
        XCTAssertTrue(diagnostic.contains("duplicate_service_instance"))
        XCTAssertTrue(diagnostic.contains("content redacted"))
        XCTAssertFalse(diagnostic.contains("already listening"))
    }

    func testHelperDiagnosticRefusesAnythingThatIsNotABareToken() {
        XCTAssertTrue(
            ServiceProcessLauncher.errorCodes(
                inPipeChunk: "velvt-service: startup halted: cannot open /Users/someone/Divorce paperwork.sqlite"
            ).isEmpty
        )
        XCTAssertTrue(
            ServiceProcessLauncher.errorCodes(
                inPipeChunk: #"error_code="/Users/someone/Divorce paperwork.sqlite""#
            ).isEmpty
        )
    }
}

// MARK: - Test doubles

/// IPCClientProtocol test double that fails a scripted sequence of dials and
/// counts every one of them.
private final class ScriptedConnectIPCClient: IPCClientProtocol, @unchecked Sendable {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        Just(.disconnected).eraseToAnyPublisher()
    }

    private let lock = NSLock()
    private var failures: [any Error]
    private var dials = 0

    var connectCount: Int { lock.withLock { dials } }

    init(failures: [any Error]) {
        self.failures = failures
    }

    func connect() async throws {
        let failure = lock.withLock { () -> (any Error)? in
            dials += 1
            return failures.isEmpty ? nil : failures.removeFirst()
        }
        if let failure {
            throw failure
        }
    }

    func disconnect() {}

    func send(_ message: ClientMessage) async throws {}
}

/// Stands in for the version-mismatch alert, recording what it was told to show
/// and returning a fixed answer for whether the person asked to retry.
private final class VersionMismatchPromptRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private let reply: Bool
    private var presented: [(expected: Int, got: Int)] = []

    init(answer: Bool) {
        reply = answer
    }

    var count: Int { lock.withLock { presented.count } }
    var lastExpected: Int? { lock.withLock { presented.last?.expected } }
    var lastGot: Int? { lock.withLock { presented.last?.got } }

    func answer(expected: Int, got: Int) -> Bool {
        lock.withLock { presented.append((expected, got)) }
        return reply
    }
}

/// Drives `ServiceProcessLauncher` with a scheduler that runs nothing until it
/// is asked to. The three intervals are deliberately distinct so a scheduled
/// action can be identified by its delay: 0.5 is the single fast retry, 60 the
/// stable-run reset, 120 the slow re-arm.
@MainActor
private final class RearmLauncherHarness {
    private struct ScheduledAction {
        let delay: TimeInterval
        let action: @MainActor () -> Void
    }

    private(set) var processes: [RearmFakeServiceProcess] = []
    private var scheduled: [ScheduledAction] = []
    var launchCount: Int { processes.count }

    func makeLauncher() -> ServiceProcessLauncher {
        ServiceProcessLauncher(
            serviceURLProvider: { URL(fileURLWithPath: "/tmp/velvt-service") },
            processStarter: { [weak self] _, _, onTermination in
                guard let self else { throw HarnessError.released }
                let process = RearmFakeServiceProcess(onTermination: onTermination)
                self.processes.append(process)
                return process
            },
            scheduler: { [weak self] delay, action in
                self?.scheduled.append(ScheduledAction(delay: delay, action: action))
            },
            relaunchPolicy: ServiceRelaunchPolicy(
                maximumAttempts: 1,
                stableRunInterval: 60,
                rearmInterval: 120,
                baseDelay: 0.5,
                maximumDelay: 2
            )
        )
    }

    func hasScheduled(delay: TimeInterval) -> Bool {
        scheduled.contains { $0.delay == delay }
    }

    func run(delay: TimeInterval) {
        guard let index = scheduled.firstIndex(where: { $0.delay == delay }) else {
            XCTFail("Expected an action scheduled for \(delay)s")
            return
        }
        scheduled.remove(at: index).action()
    }

    enum HarnessError: Error {
        case released
    }
}

private final class RearmFakeServiceProcess: OwnedServiceProcess {
    private(set) var isRunning = true
    private let onTermination: (Int32) -> Void

    init(onTermination: @escaping (Int32) -> Void) {
        self.onTermination = onTermination
    }

    func exit(status: Int32) {
        isRunning = false
        onTermination(status)
    }

    func terminate() {
        isRunning = false
    }

    func waitUntilExit() {}
    func stopReadingOutput() {}
}
