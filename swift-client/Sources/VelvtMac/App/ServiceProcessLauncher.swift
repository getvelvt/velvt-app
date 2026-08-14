import Foundation
import os

protocol OwnedServiceProcess: AnyObject {
    var isRunning: Bool { get }
    func terminate()
    func waitUntilExit()
    func stopReadingOutput()
}

private final class SystemOwnedServiceProcess: OwnedServiceProcess {
    let process: Process

    init(process: Process) {
        self.process = process
    }

    var isRunning: Bool { process.isRunning }
    func terminate() { process.terminate() }
    func waitUntilExit() { process.waitUntilExit() }

    func stopReadingOutput() {
        (process.standardOutput as? Pipe)?.fileHandleForReading.readabilityHandler = nil
        (process.standardError as? Pipe)?.fileHandleForReading.readabilityHandler = nil
    }
}

struct ServiceRelaunchPolicy: Sendable {
    let maximumAttempts: Int
    let stableRunInterval: TimeInterval
    private let backoff: ReconnectBackoff

    init(
        maximumAttempts: Int = 5,
        stableRunInterval: TimeInterval = 60,
        baseDelay: TimeInterval = 0.5,
        maximumDelay: TimeInterval = 8
    ) {
        self.maximumAttempts = maximumAttempts
        self.stableRunInterval = stableRunInterval
        backoff = ReconnectBackoff(
            baseDelay: baseDelay,
            maximumDelay: maximumDelay,
            jitter: { 1 }
        )
    }

    func delay(forAttempt attempt: Int) -> TimeInterval? {
        guard attempt > 0, attempt <= maximumAttempts else { return nil }
        return backoff.delay(forAttempt: attempt)
    }
}

/// Owns the bundled service process and relaunches unexpected exits with a
/// capped retry budget. User-requested shutdown invalidates all scheduled
/// work, so quitting can never race a delayed relaunch.
@MainActor
public final class ServiceProcessLauncher {
    typealias ProcessStarter = (
        URL,
        [String: String],
        @escaping (Int32) -> Void
    ) throws -> any OwnedServiceProcess
    typealias Scheduler = (TimeInterval, @escaping @MainActor () -> Void) -> Void

    private var process: (any OwnedServiceProcess)?
    private var desiredRunning = false
    private var launchGeneration = 0
    private var relaunchAttempt = 0
    private var lastEnvironment: [String: String] = [:]
    private let serviceURLProvider: () -> URL?
    private let processStarter: ProcessStarter
    private let scheduler: Scheduler
    private let relaunchPolicy: ServiceRelaunchPolicy

    public convenience init() {
        self.init(
            serviceURLProvider: {
                let candidate = Bundle.main.bundleURL
                    .appendingPathComponent("Contents/Resources/velvt-service")
                return FileManager.default.isExecutableFile(atPath: candidate.path)
                    ? candidate
                    : nil
            },
            processStarter: Self.startSystemProcess,
            scheduler: { delay, action in
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                    Task { @MainActor in action() }
                }
            }
        )
    }

    init(process: Process) {
        self.process = SystemOwnedServiceProcess(process: process)
        serviceURLProvider = { nil }
        processStarter = Self.startSystemProcess
        scheduler = { _, _ in }
        relaunchPolicy = ServiceRelaunchPolicy()
    }

    init(
        serviceURLProvider: @escaping () -> URL?,
        processStarter: @escaping ProcessStarter,
        scheduler: @escaping Scheduler,
        relaunchPolicy: ServiceRelaunchPolicy = ServiceRelaunchPolicy()
    ) {
        self.serviceURLProvider = serviceURLProvider
        self.processStarter = processStarter
        self.scheduler = scheduler
        self.relaunchPolicy = relaunchPolicy
    }

    public func bundledServiceURL(bundle: Bundle = .main) -> URL? {
        let candidate = bundle.bundleURL
            .appendingPathComponent("Contents/Resources/velvt-service")
        return FileManager.default.isExecutableFile(atPath: candidate.path) ? candidate : nil
    }

    public func start(environment: [String: String] = ProcessInfo.processInfo.environment) {
        guard process == nil else { return }
        guard serviceURLProvider() != nil else {
            ServiceProcessLauncherLog.shared.info("No bundled velvt-service helper found")
            return
        }
        desiredRunning = true
        lastEnvironment = environment
        relaunchAttempt = 0
        launchService()
    }

    private func launchService() {
        guard desiredRunning, process == nil, let serviceURL = serviceURLProvider() else { return }
        launchGeneration += 1
        let generation = launchGeneration
        var serviceEnvironment = lastEnvironment
        serviceEnvironment.removeValue(forKey: "VELVT_API_BASE_URL")
        if let taxonomyURL = Bundle.main.url(
            forResource: "abstraction-taxonomy-mvp-1",
            withExtension: "json"
        ) {
            serviceEnvironment["VELVT_ABSTRACTION_TAXONOMY_PATH"] = taxonomyURL.path
        }
        // Pass the socket path the client will actually dial rather than
        // trusting the helper's own default to agree with ours. Both sides
        // derive from proto/ipc_socket_path, but a silent disagreement here
        // presents as a service that never connects, so make it explicit.
        if let socketPath = Bundle.main.object(forInfoDictionaryKey: "VelvtSocketPath") as? String,
            !socketPath.isEmpty {
            serviceEnvironment["VELVT_IPC_SOCKET_PATH"] = socketPath
        }

        do {
            process = try processStarter(serviceURL, serviceEnvironment) { [weak self] status in
                Task { @MainActor [weak self] in
                    self?.handleTermination(status: status, generation: generation)
                }
            }
            ServiceProcessLauncherLog.shared.info("Launched bundled velvt-service helper")
            scheduler(relaunchPolicy.stableRunInterval) { [weak self] in
                guard
                    let self,
                    self.launchGeneration == generation,
                    self.process?.isRunning == true
                else { return }
                self.relaunchAttempt = 0
            }
        } catch {
            ServiceProcessLauncherLog.shared.error("Failed to launch bundled velvt-service helper")
            scheduleRelaunch(afterGeneration: generation)
        }
    }

    private func handleTermination(status: Int32, generation: Int) {
        guard generation == launchGeneration else { return }
        process?.stopReadingOutput()
        process = nil
        ServiceProcessLauncherLog.shared.error(
            "velvt-service exited status=\(status, privacy: .public)"
        )
        scheduleRelaunch(afterGeneration: generation)
    }

    private func scheduleRelaunch(afterGeneration generation: Int) {
        guard desiredRunning else { return }
        relaunchAttempt += 1
        guard let delay = relaunchPolicy.delay(forAttempt: relaunchAttempt) else {
            desiredRunning = false
            ServiceProcessLauncherLog.shared.error(
                "velvt-service relaunch budget exhausted"
            )
            return
        }
        scheduler(delay) { [weak self] in
            guard
                let self,
                self.desiredRunning,
                self.launchGeneration == generation,
                self.process == nil
            else { return }
            self.launchService()
        }
    }

    private static func startSystemProcess(
        serviceURL: URL,
        environment: [String: String],
        onTermination: @escaping (Int32) -> Void
    ) throws -> any OwnedServiceProcess {
        let task = Process()
        task.executableURL = serviceURL
        task.environment = environment
        let outputPipe = Pipe()
        let errorPipe = Pipe()
        task.standardOutput = outputPipe
        task.standardError = errorPipe
        stream(pipe: outputPipe, label: "stdout")
        stream(pipe: errorPipe, label: "stderr")
        task.terminationHandler = { process in
            onTermination(process.terminationStatus)
        }
        try task.run()
        return SystemOwnedServiceProcess(process: task)
    }

    private static func stream(pipe: Pipe, label: String) {
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            ServiceProcessLauncherLog.shared.error(
                "\(redactedPipeDiagnostic(label: label, byteCount: data.count), privacy: .public)"
            )
        }
    }

    nonisolated static func redactedPipeDiagnostic(label: String, byteCount: Int) -> String {
        "velvt-service \(label) emitted \(byteCount) bytes; content redacted"
    }

    public func stop() {
        desiredRunning = false
        launchGeneration += 1
        relaunchAttempt = 0
        guard let task = process, task.isRunning else {
            process = nil
            return
        }
        task.terminate()
        task.waitUntilExit()
        task.stopReadingOutput()
        process = nil
    }

    public func restart() {
        let environment = lastEnvironment.isEmpty
            ? ProcessInfo.processInfo.environment
            : lastEnvironment
        stop()
        let generation = launchGeneration
        scheduler(0.75) { [weak self] in
            guard let self, self.launchGeneration == generation else { return }
            self.start(environment: environment)
        }
    }
}

enum ServiceProcessLauncherLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "ServiceProcessLauncher")
}
