import Foundation
import os

/// Launches and stops the bundled `velvt-service` helper binary.
///
/// `make build-app` copies the Rust release binary into
/// `Contents/Resources/velvt-service` (see
/// repository root `Makefile`). A real user double-clicking the resulting
/// `.app` has no shell, so the Swift process is responsible for starting its
/// own backend; this is the simplest version of that responsibility — a
/// proper `SMAppService`-registered login item with crash relaunch is a
/// reasonable post-MVP upgrade (see `DEFERRED.md`), but for MVP one Velvt
/// process per launch, terminated on quit, is sufficient.
public final class ServiceProcessLauncher {
    private var process: Process?

    public init() {}

    init(process: Process) {
        self.process = process
    }

    /// Locates the bundled helper. Returns `nil` when running outside an
    /// app bundle (e.g. `swift run`), where the service is expected to be
    /// started separately per the README's development instructions.
    public func bundledServiceURL(bundle: Bundle = .main) -> URL? {
        let candidate = bundle.bundleURL
            .appendingPathComponent("Contents/Resources/velvt-service")
        return FileManager.default.isExecutableFile(atPath: candidate.path) ? candidate : nil
    }

    /// Starts the bundled helper if present and not already running.
    /// Failures are logged, never fatal: the IPC client already retries
    /// with backoff if the socket is not yet available.
    public func start(environment: [String: String] = ProcessInfo.processInfo.environment) {
        guard process == nil else { return }
        guard let serviceURL = bundledServiceURL() else {
            ServiceProcessLauncherLog.shared.info("No bundled velvt-service helper found")
            return
        }

        let task = Process()
        task.executableURL = serviceURL
        var serviceEnvironment = environment
        // Bundled helpers always use the endpoint compiled into the signed
        // artifact, never a developer override inherited by the app process.
        serviceEnvironment.removeValue(forKey: "VELVT_API_BASE_URL")
        if let taxonomyURL = Bundle.main.url(
            forResource: "abstraction-taxonomy-mvp-1",
            withExtension: "json"
        ) {
            serviceEnvironment["VELVT_ABSTRACTION_TAXONOMY_PATH"] = taxonomyURL.path
        }
        task.environment = serviceEnvironment
        let outputPipe = Pipe()
        let errorPipe = Pipe()
        task.standardOutput = outputPipe
        task.standardError = errorPipe
        stream(pipe: outputPipe, label: "stdout")
        stream(pipe: errorPipe, label: "stderr")
        task.terminationHandler = { [weak self] process in
            ServiceProcessLauncherLog.shared.error(
                "velvt-service exited status=\(process.terminationStatus, privacy: .public)"
            )
            DispatchQueue.main.async {
                if self?.process === process {
                    self?.process = nil
                }
            }
        }
        do {
            try task.run()
            process = task
            ServiceProcessLauncherLog.shared.info(
                "Launched bundled velvt-service helper at \(serviceURL.path, privacy: .public)"
            )
        } catch {
            // The IPC client's reconnect-with-backoff covers the case where
            // no backend is reachable; this is a startup diagnostic only.
            ServiceProcessLauncherLog.shared.error(
                "Failed to launch bundled velvt-service helper: \(error.localizedDescription)"
            )
        }
    }

    private func stream(pipe: Pipe, label: String) {
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else {
                return
            }
            ServiceProcessLauncherLog.shared.error(
                "\(Self.redactedPipeDiagnostic(label: label, byteCount: data.count), privacy: .public)"
            )
        }
    }

    static func redactedPipeDiagnostic(label: String, byteCount: Int) -> String {
        "velvt-service \(label) emitted \(byteCount) bytes; content redacted"
    }

    /// Sends SIGTERM and waits for the helper's bounded graceful shutdown to
    /// finish before allowing the app process to exit.
    public func stop() {
        guard let task = process, task.isRunning else {
            process = nil
            return
        }
        task.terminate()
        task.waitUntilExit()
        (task.standardOutput as? Pipe)?.fileHandleForReading.readabilityHandler = nil
        (task.standardError as? Pipe)?.fileHandleForReading.readabilityHandler = nil
        if process === task {
            process = nil
        }
    }

    /// Performs a graceful, user-requested helper restart. The existing IPC
    /// four-second grace interval covers this short launchd-free handoff so
    /// valid displayed data remains visible while the socket is recreated.
    public func restart() {
        stop()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.75) { [weak self] in
            self?.start()
        }
    }
}

enum ServiceProcessLauncherLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "ServiceProcessLauncher")
}
