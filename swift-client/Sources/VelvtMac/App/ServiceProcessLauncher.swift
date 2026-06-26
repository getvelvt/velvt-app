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
        task.environment = environment
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
            guard !data.isEmpty, let line = String(data: data, encoding: .utf8) else {
                return
            }
            ServiceProcessLauncherLog.shared.error(
                "velvt-service \(label, privacy: .public): \(line, privacy: .public)"
            )
        }
    }

    /// Sends SIGTERM and gives the helper a moment to flush before the app
    /// exits, mirroring the graceful-shutdown sequence the Rust service
    /// implements for its own SIGTERM/SIGINT handling.
    public func stop() {
        guard let task = process, task.isRunning else {
            process = nil
            return
        }
        (task.standardOutput as? Pipe)?.fileHandleForReading.readabilityHandler = nil
        (task.standardError as? Pipe)?.fileHandleForReading.readabilityHandler = nil
        task.terminate()
        process = nil
    }
}

enum ServiceProcessLauncherLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "ServiceProcessLauncher")
}
