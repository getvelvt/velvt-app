import XCTest
@testable import VelvtMac

@MainActor
final class ServiceProcessLauncherTests: XCTestCase {
    func testPipeDiagnosticRedactsHelperOutputContent() {
        let sensitiveOutput = "Secret Window Title"

        let diagnostic = ServiceProcessLauncher.redactedPipeDiagnostic(
            label: "stderr",
            byteCount: sensitiveOutput.utf8.count
        )

        XCTAssertTrue(diagnostic.contains("stderr"))
        XCTAssertTrue(diagnostic.contains("\(sensitiveOutput.utf8.count)"))
        XCTAssertFalse(diagnostic.contains(sensitiveOutput))
    }

    func testStopWaitsForOwnedHelperToExit() throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sleep")
        process.arguments = ["30"]
        try process.run()
        let launcher = ServiceProcessLauncher(process: process)

        launcher.stop()

        XCTAssertFalse(process.isRunning)
    }

    func testUnexpectedExitRelaunchesWithCappedExponentialBackoff() async {
        let harness = ServiceLauncherHarness()
        let launcher = harness.makeLauncher(maximumAttempts: 2)
        launcher.start(environment: ["VELVT_API_BASE_URL": "https://developer.invalid"])

        XCTAssertEqual(harness.launchCount, 1)
        XCTAssertNil(harness.environments[0]["VELVT_API_BASE_URL"])

        harness.processes[0].exit(status: 1)
        await Task.yield()
        XCTAssertEqual(harness.relaunchDelays, [0.5])
        harness.runNextRelaunch()
        XCTAssertEqual(harness.launchCount, 2)

        harness.processes[1].exit(status: 2)
        await Task.yield()
        XCTAssertEqual(harness.relaunchDelays, [1])
        harness.runNextRelaunch()
        XCTAssertEqual(harness.launchCount, 3)

        harness.processes[2].exit(status: 3)
        await Task.yield()
        XCTAssertTrue(harness.relaunchDelays.isEmpty)
    }

    func testStopInvalidatesPendingCrashRelaunch() async {
        let harness = ServiceLauncherHarness()
        let launcher = harness.makeLauncher(maximumAttempts: 3)
        launcher.start()
        harness.processes[0].exit(status: 1)
        await Task.yield()
        XCTAssertEqual(harness.relaunchDelays, [0.5])

        launcher.stop()
        harness.runNextRelaunch()

        XCTAssertEqual(harness.launchCount, 1)
    }

    func testStableRunResetsRelaunchBackoff() async {
        let harness = ServiceLauncherHarness()
        let launcher = harness.makeLauncher(maximumAttempts: 3)
        launcher.start()
        harness.processes[0].exit(status: 1)
        await Task.yield()
        harness.runNextRelaunch()
        XCTAssertEqual(harness.launchCount, 2)

        harness.runStableTimer(forLaunch: 1)
        harness.processes[1].exit(status: 1)
        await Task.yield()

        XCTAssertEqual(harness.relaunchDelays, [0.5])
    }
}

@MainActor
private final class ServiceLauncherHarness {
    struct ScheduledAction {
        let delay: TimeInterval
        let action: @MainActor () -> Void
    }

    private(set) var processes: [FakeOwnedServiceProcess] = []
    private(set) var environments: [[String: String]] = []
    private var scheduled: [ScheduledAction] = []
    var launchCount: Int { processes.count }
    var relaunchDelays: [TimeInterval] {
        scheduled.filter { $0.delay < 10 }.map(\.delay)
    }

    func makeLauncher(maximumAttempts: Int) -> ServiceProcessLauncher {
        ServiceProcessLauncher(
            serviceURLProvider: { URL(fileURLWithPath: "/tmp/velvt-service") },
            processStarter: { [weak self] _, environment, onTermination in
                guard let self else { throw HarnessError.released }
                let process = FakeOwnedServiceProcess(onTermination: onTermination)
                self.processes.append(process)
                self.environments.append(environment)
                return process
            },
            scheduler: { [weak self] delay, action in
                self?.scheduled.append(ScheduledAction(delay: delay, action: action))
            },
            relaunchPolicy: ServiceRelaunchPolicy(
                maximumAttempts: maximumAttempts,
                stableRunInterval: 10,
                baseDelay: 0.5,
                maximumDelay: 2
            )
        )
    }

    func runNextRelaunch() {
        guard let index = scheduled.firstIndex(where: { $0.delay < 10 }) else {
            XCTFail("Expected a scheduled relaunch")
            return
        }
        scheduled.remove(at: index).action()
    }

    func runStableTimer(forLaunch index: Int) {
        let stableTimers = scheduled.indices.filter { scheduled[$0].delay == 10 }
        guard stableTimers.indices.contains(index) else {
            XCTFail("Expected a stable-run timer")
            return
        }
        scheduled.remove(at: stableTimers[index]).action()
    }

    enum HarnessError: Error {
        case released
    }
}

private final class FakeOwnedServiceProcess: OwnedServiceProcess {
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
