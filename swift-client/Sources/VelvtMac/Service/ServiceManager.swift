import Foundation
import os
import ServiceManagement

/// Lifecycle state of the bundled Rust service.
public enum ServiceState {
    case notInstalled
    case installing
    case running
    case stopped
    case updateInProgress
    case failed(Error)
}

// Equatable conformance: failed carries an opaque Error, so we compare by case
// only for the error variant.
extension ServiceState: Equatable {
    public static func == (lhs: ServiceState, rhs: ServiceState) -> Bool {
        switch (lhs, rhs) {
        case (.notInstalled, .notInstalled),
             (.installing, .installing),
             (.running, .running),
             (.stopped, .stopped),
             (.updateInProgress, .updateInProgress):
            return true
        case (.failed, .failed):
            return true
        default:
            return false
        }
    }
}

/// Manages installation, versioning, and SMAppService registration of the
/// bundled Rust helper binary. All launchd interaction goes through
/// SMAppService (macOS 13+). No shell-outs.
///
/// Version sidecar: the Run Script build phase writes the Rust binary's
/// semver string to Contents/Resources/velvt-service.version next to the
/// binary. ServiceManager copies that file alongside the binary to
/// ~/Library/Application Support/Velvt/velvt-service.version and compares
/// the two strings on each launch to detect updates.
@MainActor
public final class ServiceManager: ObservableObject {
    @Published public var state: ServiceState = .notInstalled

    private let plistName = "com.velvt.service.plist"
    private let supportDir: URL
    private let binaryDest: URL
    private let versionSidecar: URL

    private static let log = Logger(subsystem: "com.velvt.mac", category: "ServiceManager")

    public init() {
        let appSupport = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask
        ).first!
        supportDir = appSupport.appendingPathComponent("Velvt", isDirectory: true)
        binaryDest = supportDir.appendingPathComponent("velvt-service")
        versionSidecar = supportDir.appendingPathComponent("velvt-service.version")
    }

    // MARK: - Public API

    /// Installs the binary and registers the LaunchAgent if not already present.
    public func ensureInstalled() async throws {
        guard !FileManager.default.fileExists(atPath: binaryDest.path) else { return }
        state = .installing
        try await install()
    }

    /// Compares bundled vs installed version; re-installs if they differ.
    public func ensureUpToDate() async throws {
        let bundledVersion = try bundledVersionString()
        let installedVersion = (try? String(contentsOf: versionSidecar, encoding: .utf8))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }

        guard bundledVersion != installedVersion else { return }
        state = .updateInProgress
        let service = SMAppService.agent(plistName: plistName)
        try? service.unregister()
        try await install()
    }

    /// Registers and starts the agent. No-op if already running.
    public func start() async throws {
        let service = SMAppService.agent(plistName: plistName)
        if service.status == .enabled { state = .running; return }
        try service.register()
        state = .running
    }

    /// Unregisters the agent.
    public func stop() async throws {
        let service = SMAppService.agent(plistName: plistName)
        try service.unregister()
        state = .stopped
    }

    // MARK: - Private

    private func install() async throws {
        let fm = FileManager.default

        // Ensure ~/Library/Application Support/Velvt/ exists.
        try fm.createDirectory(at: supportDir, withIntermediateDirectories: true)

        // Copy binary from bundle.
        let bundledBinary = try bundledBinaryURL()
        if fm.fileExists(atPath: binaryDest.path) {
            try fm.removeItem(at: binaryDest)
        }
        try fm.copyItem(at: bundledBinary, to: binaryDest)

        // Set executable bit (0o755).
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: binaryDest.path)

        // Copy version sidecar.
        let bundledSidecar = try bundledVersionSidecarURL()
        if fm.fileExists(atPath: versionSidecar.path) {
            try fm.removeItem(at: versionSidecar)
        }
        try fm.copyItem(at: bundledSidecar, to: versionSidecar)

        // Write LaunchAgent plist from template.
        try writeLaunchAgentPlist()

        // Register with SMAppService.
        let service = SMAppService.agent(plistName: plistName)
        try service.register()
        state = .running

        Self.log.info("velvt-service installed and registered")
    }

    private func writeLaunchAgentPlist() throws {
        // SMAppService expects the plist in ~/Library/LaunchAgents/ and resolves
        // it by name — we write it there ourselves from the bundled template.
        let launchAgentsDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        try FileManager.default.createDirectory(
            at: launchAgentsDir, withIntermediateDirectories: true
        )

        let templateURL = try bundledTemplateURL()
        var contents = try String(contentsOf: templateURL, encoding: .utf8)
        contents = contents.replacingOccurrences(of: "{{BINARY_PATH}}", with: binaryDest.path)

        let destURL = launchAgentsDir.appendingPathComponent(plistName)
        try contents.write(to: destURL, atomically: true, encoding: .utf8)
    }

    // MARK: - Bundle helpers

    private func bundledBinaryURL() throws -> URL {
        guard let url = Bundle.main.url(
            forResource: "velvt-service", withExtension: nil, subdirectory: nil
        ) else {
            throw ServiceManagerError.binaryNotFoundInBundle
        }
        return url
    }

    private func bundledVersionSidecarURL() throws -> URL {
        guard let url = Bundle.main.url(
            forResource: "velvt-service", withExtension: "version", subdirectory: nil
        ) else {
            throw ServiceManagerError.versionSidecarNotFoundInBundle
        }
        return url
    }

    private func bundledTemplateURL() throws -> URL {
        guard let url = Bundle.main.url(
            forResource: "com.velvt.service", withExtension: "plist.template"
        ) else {
            throw ServiceManagerError.templateNotFoundInBundle
        }
        return url
    }

    private func bundledVersionString() throws -> String {
        let url = try bundledVersionSidecarURL()
        return try String(contentsOf: url, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

enum ServiceManagerError: LocalizedError {
    case binaryNotFoundInBundle
    case versionSidecarNotFoundInBundle
    case templateNotFoundInBundle

    var errorDescription: String? {
        switch self {
        case .binaryNotFoundInBundle:
            return "The bundled velvt-service binary could not be located in the app bundle."
        case .versionSidecarNotFoundInBundle:
            return "The velvt-service version file could not be located in the app bundle."
        case .templateNotFoundInBundle:
            return "The LaunchAgent plist template could not be located in the app bundle."
        }
    }
}
