import Combine
import Foundation
import XCTest

@testable import VelvtMac

/// The v29 -> v30 protocol bump makes this the actual upgrade path for everyone
/// coming from 1.0.10: the old helper is still holding the socket, still
/// answering the handshake with its own protocol version, and `versionMismatch`
/// is the one `IPCError` that arms no reconnect. Retrying could never succeed
/// while that process lived, so the app now asks it to quit — and the rule for
/// which process that may be is the most safety-critical thing in this file.
final class OrphanedHelperRuleTests: XCTestCase {

    private let helperPath = "/Applications/Velvt.app/Contents/Resources/velvt-service"

    private func facts(
        pid: pid_t,
        ppid: pid_t = 1,
        path: String,
        uid: uid_t = 501
    ) -> RunningProcessFacts {
        RunningProcessFacts(
            processIdentifier: pid,
            parentProcessIdentifier: ppid,
            executablePath: path,
            userIdentifier: uid
        )
    }

    /// An orphan is a process running this app's own helper executable, as this
    /// user, that this app did not start.
    func testAnOrphanOfAnEarlierRunIsIdentified() {
        let orphans = OrphanedHelperRule.terminableOrphans(
            in: [facts(pid: 900, ppid: 1, path: helperPath)],
            helperExecutablePath: helperPath,
            currentUser: 501,
            ownProcessIdentifier: 1_000
        )

        XCTAssertEqual(orphans, [900])
    }

    /// The whole point of matching on the full executable path: anything merely
    /// *called* velvt-service is somebody else's file and is never signalled.
    func testNothingIsEverIdentifiedByName() {
        let lookalikes = [
            facts(pid: 901, path: "/Users/someone/Downloads/velvt-service"),
            facts(pid: 902, path: "/opt/homebrew/bin/velvt-service"),
            facts(pid: 903, path: "/Users/someone/build/debug/velvt-service"),
            facts(pid: 904, path: "/Applications/Velvt.app/Contents/Resources/velvt-service-old"),
        ]

        XCTAssertTrue(
            OrphanedHelperRule.terminableOrphans(
                in: lookalikes,
                helperExecutablePath: helperPath,
                currentUser: 501,
                ownProcessIdentifier: 1_000
            ).isEmpty
        )
    }

    /// A helper belonging to another user account on this Mac is not this app's
    /// to end, even at the identical path.
    func testAHelperOwnedByAnotherUserIsLeftAlone() {
        XCTAssertTrue(
            OrphanedHelperRule.terminableOrphans(
                in: [facts(pid: 905, path: helperPath, uid: 502)],
                helperExecutablePath: helperPath,
                currentUser: 501,
                ownProcessIdentifier: 1_000
            ).isEmpty
        )
    }

    /// The helper this run launched is a direct child of this process. Killing
    /// it would turn a recoverable state into a broken one.
    func testTheHelperThisAppStartedIsNeverTerminated() {
        let orphans = OrphanedHelperRule.terminableOrphans(
            in: [
                facts(pid: 906, ppid: 1_000, path: helperPath),
                facts(pid: 907, ppid: 1, path: helperPath),
            ],
            helperExecutablePath: helperPath,
            currentUser: 501,
            ownProcessIdentifier: 1_000
        )

        XCTAssertEqual(orphans, [907], "only the process this app did not start")
    }

    /// No bundled helper (a `swift run` development build) means there is no
    /// process this app is responsible for, so there is nothing to consider.
    func testAnEmptyHelperPathMatchesNothing() {
        XCTAssertTrue(
            OrphanedHelperRule.terminableOrphans(
                in: [facts(pid: 908, path: "")],
                helperExecutablePath: "",
                currentUser: 501,
                ownProcessIdentifier: 1_000
            ).isEmpty
        )
    }

    func testTheAppsOwnProcessIsNeverACandidate() {
        XCTAssertTrue(
            OrphanedHelperRule.terminableOrphans(
                in: [facts(pid: 1_000, ppid: 1, path: helperPath)],
                helperExecutablePath: helperPath,
                currentUser: 501,
                ownProcessIdentifier: 1_000
            ).isEmpty
        )
    }
}

// MARK: - Reaping

final class OrphanedHelperReaperTests: XCTestCase {

    private let helperPath = "/Applications/Velvt.app/Contents/Resources/velvt-service"

    private func makeReaper(
        table: ReapRecorder,
        helperPath: String?
    ) -> OrphanedHelperReaper {
        OrphanedHelperReaper(
            runningProcesses: { table.processes },
            helperExecutablePath: { helperPath },
            currentUser: { 501 },
            ownProcessIdentifier: { 1_000 },
            requestTermination: { table.signal($0) },
            isRunning: { table.isRunning($0) },
            waitOneInterval: {},
            waitForRelaunch: {}
        )
    }

    func testOnlyTheOrphanIsSignalledAndTheSocketIsReportedFree() async {
        let table = ReapRecorder(
            processes: [
                RunningProcessFacts(
                    processIdentifier: 900,
                    parentProcessIdentifier: 1,
                    executablePath: helperPath,
                    userIdentifier: 501
                ),
                // This app's own helper, and an unrelated lookalike.
                RunningProcessFacts(
                    processIdentifier: 901,
                    parentProcessIdentifier: 1_000,
                    executablePath: helperPath,
                    userIdentifier: 501
                ),
                RunningProcessFacts(
                    processIdentifier: 902,
                    parentProcessIdentifier: 1,
                    executablePath: "/Users/someone/Downloads/velvt-service",
                    userIdentifier: 501
                ),
            ],
            exitsOnSignal: true
        )

        let reclaimed = await makeReaper(table: table, helperPath: helperPath).reclaimSocket()

        XCTAssertTrue(reclaimed)
        XCTAssertEqual(table.signalled, [900], "exactly one process, positively identified")
    }

    /// A helper that ignores the signal is not reported as reclaimed: the
    /// socket is still held, so the person gets the alert rather than a silent
    /// retry loop that cannot succeed.
    func testAHelperThatWillNotExitIsNotReportedAsReclaimed() async {
        let table = ReapRecorder(
            processes: [
                RunningProcessFacts(
                    processIdentifier: 900,
                    parentProcessIdentifier: 1,
                    executablePath: helperPath,
                    userIdentifier: 501
                )
            ],
            exitsOnSignal: false
        )

        let reclaimed = await makeReaper(table: table, helperPath: helperPath)
            .reclaimSocket(pollAttempts: 3)

        XCTAssertFalse(reclaimed)
        XCTAssertEqual(table.signalled, [900])
    }

    func testNothingIsSignalledWhenThereIsNoBundledHelper() async {
        let table = ReapRecorder(
            processes: [
                RunningProcessFacts(
                    processIdentifier: 900,
                    parentProcessIdentifier: 1,
                    executablePath: helperPath,
                    userIdentifier: 501
                )
            ],
            exitsOnSignal: true
        )

        let reclaimed = await makeReaper(table: table, helperPath: nil).reclaimSocket()

        XCTAssertFalse(reclaimed)
        XCTAssertTrue(table.signalled.isEmpty)
    }

    func testAnEmptyProcessTableIsSimplyNothingToDo() async {
        let table = ReapRecorder(processes: [], exitsOnSignal: true)

        let reclaimed = await makeReaper(table: table, helperPath: helperPath).reclaimSocket()

        XCTAssertFalse(reclaimed)
        XCTAssertTrue(table.signalled.isEmpty)
    }
}

// MARK: - The connect loop

final class VersionMismatchRecoveryLoopTests: XCTestCase {

    /// The deadlock, fixed: the upgraded client recovers on its own and the
    /// person is never asked to find a process in a terminal.
    func testAVersionMismatchIsRecoveredWithoutTroublingThePerson() async {
        let client = RecordingConnectClient(failures: [IPCError.versionMismatch(expected: 30, got: 29)])
        let reclaim = ReclaimRecorder(succeeds: true)
        let prompt = PromptRecorder(answer: true)

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: { reclaim.reclaim() }
        ) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(reclaim.count, 1)
        XCTAssertEqual(client.connectCount, 2, "the re-dial has to happen after the socket is freed")
        XCTAssertEqual(prompt.count, 0, "an upgrade that recovers itself needs no dialog")
    }

    /// Nothing identifiable to reclaim: the alert is still the fallback, and
    /// accepting it still re-dials.
    func testFallsBackToTheAlertWhenThereIsNoOrphanToReclaim() async {
        let client = RecordingConnectClient(failures: [IPCError.versionMismatch(expected: 30, got: 29)])
        let reclaim = ReclaimRecorder(succeeds: false)
        let prompt = PromptRecorder(answer: true)

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: { reclaim.reclaim() }
        ) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(reclaim.count, 1)
        XCTAssertEqual(prompt.count, 1)
        XCTAssertEqual(prompt.lastExpected, 30)
        XCTAssertEqual(prompt.lastGot, 29)
        XCTAssertEqual(client.connectCount, 2)
    }

    /// The automatic path is finite. A mismatch that survives being reclaimed
    /// must reach the person rather than spinning forever.
    func testAutomaticRecoveryStopsAndAsksRatherThanLoopingForever() async {
        let client = RecordingConnectClient(alwaysFailWith: IPCError.versionMismatch(expected: 30, got: 29))
        let reclaim = ReclaimRecorder(succeeds: true)
        let prompt = PromptRecorder(answer: false)

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: { reclaim.reclaim() }
        ) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(reclaim.count, AppDelegate.maximumOrphanReclaimAttempts)
        XCTAssertEqual(prompt.count, 1)
        XCTAssertEqual(client.connectCount, AppDelegate.maximumOrphanReclaimAttempts + 1)
    }

    /// Transport failures are still the IPC client's own business: nothing is
    /// reclaimed and nobody is asked anything.
    func testATransportFailureNeitherReclaimsNorPrompts() async {
        let client = RecordingConnectClient(failures: [IPCError.socket(code: 61)])
        let reclaim = ReclaimRecorder(succeeds: true)
        let prompt = PromptRecorder(answer: true)

        await AppDelegate.connectRetryingVersionMismatch(
            client,
            reclaimOrphanedHelper: { reclaim.reclaim() }
        ) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(reclaim.count, 0)
        XCTAssertEqual(prompt.count, 0)
        XCTAssertEqual(client.connectCount, 1)
    }

    /// The default keeps the old shape for any call site that has no recovery
    /// to offer.
    func testWithoutARecoveryClosureTheAlertIsStillTheOnlyPath() async {
        let client = RecordingConnectClient(failures: [IPCError.versionMismatch(expected: 30, got: 29)])
        let prompt = PromptRecorder(answer: false)

        await AppDelegate.connectRetryingVersionMismatch(client) { expected, got in
            prompt.answer(expected: expected, got: got)
        }

        XCTAssertEqual(prompt.count, 1)
        XCTAssertEqual(client.connectCount, 1)
    }
}

// MARK: - Test doubles

/// A fake process table that records which pids were signalled and can decide
/// whether a signalled process actually exits.
private final class ReapRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var table: [RunningProcessFacts]
    private var exited: Set<pid_t> = []
    private var signals: [pid_t] = []
    private let exitsOnSignal: Bool

    init(processes: [RunningProcessFacts], exitsOnSignal: Bool) {
        table = processes
        self.exitsOnSignal = exitsOnSignal
    }

    var processes: [RunningProcessFacts] { lock.withLock { table } }
    var signalled: [pid_t] { lock.withLock { signals } }

    func signal(_ pid: pid_t) -> Bool {
        lock.withLock {
            signals.append(pid)
            if exitsOnSignal {
                exited.insert(pid)
            }
            return true
        }
    }

    func isRunning(_ pid: pid_t) -> Bool {
        lock.withLock { !exited.contains(pid) }
    }
}

/// Counts reclaim attempts and reports a fixed outcome.
private final class ReclaimRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private let succeeds: Bool
    private var attempts = 0

    init(succeeds: Bool) {
        self.succeeds = succeeds
    }

    var count: Int { lock.withLock { attempts } }

    func reclaim() -> Bool {
        lock.withLock { attempts += 1 }
        return succeeds
    }
}

/// Stands in for the alert, recording what it was told to show.
private final class PromptRecorder: @unchecked Sendable {
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

/// Counts dials and fails a scripted sequence of them.
private final class RecordingConnectClient: IPCClientProtocol, @unchecked Sendable {
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
