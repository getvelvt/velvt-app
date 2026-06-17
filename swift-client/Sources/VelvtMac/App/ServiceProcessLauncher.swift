import Foundation

// ServiceProcessLauncher is the pre-SMAppService subprocess-based launcher retained
// for local `swift run` development workflows only. Release builds use ServiceManager.
#if DEBUG

/// Launches and stops the bundled `velvt-service` helper binary.
///
/// `make build-app` copies the Rust release binary into
/// `Contents/MacOS/velvt-service` alongside the Swift executable (see
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
            .appendingPathComponent("Contents/MacOS/velvt-service")
        return FileManager.default.isExecutableFile(atPath: candidate.path) ? candidate : nil
    }

    /// Starts the bundled helper if present and not already running.
    /// Failures are logged, never fatal: the IPC client already retries
    /// with backoff if the socket is not yet available.
    public func start(environment: [String: String] = ProcessInfo.processInfo.environment) {
        guard process == nil else { return }
        guard let serviceURL = bundledServiceURL() else { return }

        let task = Process()
        task.executableURL = serviceURL
        task.environment = environment
        do {
            try task.run()
            process = task
        } catch {
            // The IPC client's reconnect-with-backoff covers the case where
            // no backend is reachable; this is a startup diagnostic only.
            ServiceProcessLauncherLog.shared.error(
                "Failed to launch bundled velvt-service helper: \(error.localizedDescription)"
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
        task.terminate()
        process = nil
    }
}

import os

enum ServiceProcessLauncherLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "ServiceProcessLauncher")
}

#endif
