import Combine
import Darwin
import Foundation
import XCTest

@testable import VelvtMac

/// The whole client path against a real `velvt-service`: `AXCollectionAgent`,
/// `EventRelay` and `UnixSocketIPCClient`, with only the two macOS adapters
/// scripted, so the Accessibility failure is the one thing that is simulated.
///
/// Skipped unless `VELVT_LIVE_HELPER_SOCKET` names the socket of a helper
/// started for this purpose, with its own HOME, database and socket. Never
/// point it at the socket of a helper that a running Velvt started:
///
///     HOME=$(mktemp -d) VELVT_DATABASE_PATH=<that dir>/velvt.sqlite3 \
///       VELVT_IPC_SOCKET_PATH=/tmp/velvt-trace.sock \
///       VELVT_API_BASE_URL=http://127.0.0.1:9 \
///       VELVT_ABSTRACTION_TAXONOMY_PATH=rust-service/resources/abstraction-taxonomy-mvp-1.json \
///       rust-service/target/debug/velvt-service &
///     VELVT_LIVE_HELPER_SOCKET=/tmp/velvt-trace.sock swift test --package-path swift-client \
///       --filter UnobservableApplicationLiveHelperTests
///
/// It prints one `live-trace` line per pushed snapshot. CI replays the same
/// block from fixed reports in `rust-service/tests/drift_offer_live_helper.rs`.
final class UnobservableApplicationLiveHelperTests: XCTestCase {
    func testADepartureToAnUnobservableApplicationReachesALiveHelperWhenRequested() async throws {
        guard let socket = ProcessInfo.processInfo.environment["VELVT_LIVE_HELPER_SOCKET"] else {
            throw XCTSkip("set VELVT_LIVE_HELPER_SOCKET to the socket of a helper started for this test")
        }
        let client = UnixSocketIPCClient(socketPath: socket, protocolVersion: 33, clientVersion: "live-trace")
        let received = ReceivedSnapshots()
        let reader = Task {
            for await message in client.incomingMessages {
                if case .workBlockState(let snapshot) = message {
                    received.append(snapshot)
                }
            }
        }
        try await client.connect()
        try await client.send(
            .startWorkBlock(
                StartWorkBlock(
                    intention: nil,
                    plannedDurationSeconds: 25 * 60,
                    purpose: .deepWork,
                    intensity: .medium,
                    invitationID: nil
                )))
        let startedAt = try await received.waitForStart()

        let relay = EventRelay(ipcClient: client)
        await relay.start()
        try await Task.sleep(for: .milliseconds(300))

        let clock = SettableClock(startedAt)
        let workspace = ScriptedWorkspace()
        let accessibility = ScriptedAccessibility()
        accessibility.titles = [10: "main.swift", 11: "general"]
        accessibility.unobservable = [12, 13]
        let agent = AXCollectionAgent(
            eventSink: relay,
            permissionChecker: TrustedChecker(),
            workspaceObserver: workspace,
            accessibilityObserver: accessibility,
            metadataProvider: NoMetadata(),
            now: { clock.now }
        )
        try agent.start()

        let xcode = RunningApplication(processIdentifier: 10, appName: "Xcode", bundleIdentifier: "com.apple.dt.Xcode")
        let slack = RunningApplication(
            processIdentifier: 11, appName: "Slack", bundleIdentifier: "com.tinyspeck.slackmacgap")
        let messages = RunningApplication(
            processIdentifier: 12, appName: "Messages", bundleIdentifier: "com.apple.MobileSMS")
        let chrome = RunningApplication(
            processIdentifier: 13, appName: "Google Chrome", bundleIdentifier: "com.google.Chrome")
        let timeline: [(TimeInterval, RunningApplication)] = [
            (10, xcode), (200, slack), (215, xcode), (260, slack), (275, xcode),
            (320, messages), (368, xcode), (600, chrome), (700, xcode), (1_400, slack), (1_520, xcode),
        ]
        var pushed: [TimeInterval: [WorkBlockSnapshot]] = [:]
        for (offset, application) in timeline {
            clock.now = startedAt.addingTimeInterval(offset)
            let before = received.count
            workspace.activate(application)
            try await Task.sleep(for: .milliseconds(250))
            pushed[offset] = received.since(before)
            for snapshot in received.since(before) {
                print(
                    "live-trace t=+\(Int(offset))s \(application.appName): phase=\(snapshot.phase.rawValue) "
                        + "current=\(snapshot.currentCategory ?? "-") anchor=\(snapshot.anchorCategory ?? "-") "
                        + "offer=\(snapshot.activeIntervention.map { "switches=\($0.switchCount)" } ?? "none")"
                )
                if let result = snapshot.result {
                    print(
                        "live-trace result: switch_away_count=\(result.switchAwayCount) "
                            + "longest_uninterrupted_seconds=\(result.longestUninterruptedSeconds) "
                            + "coverage_ratio=\(result.coverageRatio) coverage=\(result.coverage.rawValue)"
                    )
                }
            }
        }
        agent.stop()
        try await Task.sleep(for: .milliseconds(300))
        await relay.stop()
        client.disconnect()
        reader.cancel()

        // Messages could not be observed at window level, and the departure
        // to it is the third switch: the offer is pushed as the person arrives.
        let offer = pushed[320]?.compactMap(\.activeIntervention).first
        XCTAssertEqual(offer?.switchCount, 3)
        XCTAssertEqual(offer?.anchorCategory, "FOCUS_WORK")
        XCTAssertNil(pushed[368]?.last?.activeIntervention, "the return withdraws it")
        let result = pushed.values.joined().compactMap(\.result).first
        XCTAssertEqual(result?.switchAwayCount, 4)
    }
}

/// The real `AXApplicationObserver` against this test process's own
/// application element, which needs no Accessibility permission. The process
/// has no window, so a focused or main window reads as `kAXErrorNoValue`
/// (-25212), the failure that used to leave an application unobserved. The
/// window it then orders in is far offscreen, and the process is never
/// activated, so nothing takes focus from the application in front.
///
/// Skipped unless `VELVT_AX_SELF_PROBE` is set: it needs a window server.
final class AccessibilityObserverSelfProbeTests: XCTestCase {
    @MainActor
    func testTheFirstWindowOfAWindowlessApplicationIsReportedWhenRequested() throws {
        guard ProcessInfo.processInfo.environment["VELVT_AX_SELF_PROBE"] != nil else {
            throw XCTSkip("set VELVT_AX_SELF_PROBE to register against this process's own application element")
        }
        // A test runner is not an application until it finishes launching,
        // and before that its own element answers kAXErrorNotImplemented.
        NSApplication.shared.setActivationPolicy(.accessory)
        NSApplication.shared.finishLaunching()
        RunLoop.main.run(until: Date().addingTimeInterval(0.5))
        let observer = AXApplicationObserver()
        let reported = expectation(description: "the first window is reported")
        var titles: [String?] = []
        let registration = try observer.start(
            observing: RunningApplication(processIdentifier: getpid(), appName: "probe"),
            activityHandler: { activity in
                titles.append(activity.windowTitle)
                reported.fulfill()
            },
            errorHandler: { error in XCTFail("observer failed: \(error)") }
        )
        defer { observer.stop() }

        XCTAssertEqual(registration, .awaitingWindow)

        let window = NSWindow(
            contentRect: NSRect(x: -30_000, y: -30_000, width: 200, height: 100),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.title = "probe window"
        window.orderFrontRegardless()
        window.makeKey()
        defer { window.orderOut(nil) }

        wait(for: [reported], timeout: 5)
        XCTAssertEqual(titles.first, "probe window")
        XCTAssertFalse(NSApplication.shared.isActive)
    }
}

private final class ReceivedSnapshots: @unchecked Sendable {
    private let lock = NSLock()
    private var snapshots: [WorkBlockSnapshot] = []

    var count: Int { lock.withLock { snapshots.count } }

    func append(_ snapshot: WorkBlockSnapshot) {
        lock.withLock { snapshots.append(snapshot) }
    }

    func since(_ index: Int) -> [WorkBlockSnapshot] {
        lock.withLock { Array(snapshots[index...]) }
    }

    func waitForStart() async throws -> Date {
        for _ in 0..<100 {
            if let startedAt = lock.withLock({ snapshots.last(where: { $0.blockID != nil })?.startedAt }) {
                return startedAt
            }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw XCTSkip("the helper never reported a started block")
    }
}

private final class SettableClock: @unchecked Sendable {
    private let lock = NSLock()
    private var instant: Date

    init(_ instant: Date) {
        self.instant = instant
    }

    var now: Date {
        get { lock.withLock { instant } }
        set { lock.withLock { instant = newValue } }
    }
}

private final class TrustedChecker: AccessibilityPermissionChecking {
    func hasPermission() -> Bool { true }
}

private final class NoMetadata: DeclaredAppMetadataReading {
    func metadata(for application: RunningApplication) -> DeclaredAppMetadata { .absent }
}

private final class ScriptedWorkspace: WorkspaceActivationObserving {
    private var handler: ((RunningApplication) -> Void)?

    func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication? {
        handler = activationHandler
        return nil
    }

    func stop() {
        handler = nil
    }

    func activate(_ application: RunningApplication) {
        handler?(application)
    }
}

/// Registers at window level for every application except the ones listed as
/// unobservable, which fail the way an application with no focused or main
/// window at activation does (-25212, `kAXErrorNoValue`).
private final class ScriptedAccessibility: AccessibilityObserving {
    var titles: [pid_t: String] = [:]
    var unobservable: Set<pid_t> = []

    func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> AccessibilityRegistration {
        if unobservable.contains(application.processIdentifier) {
            throw CollectionError.observerRegistrationFailed(code: AXError.noValue.rawValue)
        }
        return .window(FocusedActivity(windowTitle: titles[application.processIdentifier]))
    }

    func stop() {}
}
