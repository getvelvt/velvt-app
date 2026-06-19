import Foundation
import os
import ServiceManagement

/// Lifecycle state of the bundled Rust service.
public enum ManagedServiceState {
    case notInstalled
    case installing
    case running
    case stopped
    case updateInProgress
    case failed(Error)
}

// Equatable conformance: failed carries an opaque Error, so we compare by case only.
extension ManagedServiceState: Equatable {
    public static func == (lhs: ManagedServiceState, rhs: ManagedServiceState) -> Bool {
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

// MARK: - ServiceRegistrar

/// Abstracts SMAppService operations for testability.
protocol ServiceRegistrar {
    var isEnabled: Bool { get }
    func register() throws
    func unregister() throws
}

/// Production registrar backed by SMAppService.
final class SMServiceRegistrar: ServiceRegistrar {
    private let plistName: String

    init(plistName: String) { self.plistName = plistName }

    var isEnabled: Bool {
        SMAppService.agent(plistName: plistName).status == .enabled
    }

    func register() throws {
        try SMAppService.agent(plistName: plistName).register()
    }

    func unregister() throws {
        try SMAppService.agent(plistName: plistName).unregister()
    }
}

// MARK: - ServiceManager

/// Manages installation, versioning, and SMAppService registration of the
/// bundled Rust helper binary. All launchd interaction goes through
/// SMAppService (macOS 13+). No shell-outs, no Process().
///
/// Version sidecar: the Run Script build phase writes the Rust binary's
/// semver string to Contents/Resources/velvt-service.version next to the
/// binary. ServiceManager copies that sidecar alongside the binary to
/// ~/Library/Application Support/Velvt/velvt-service.version and compares
/// the two strings on each launch to detect updates.
@MainActor
public final class ServiceManager: ObservableObject {
    @Published public var state: ManagedServiceState = .notInstalled

    private let fileManager: FileManager
    private let plistName: String
    let supportDir: URL      // internal for testing
    let binaryDest: URL      // internal for testing
    let versionSidecar: URL  // internal for testing
    private let launchAgentsDir: URL
    let registrar: any ServiceRegistrar  // internal for testing

    // Resource providers — closures so tests can substitute temp files.
    var bundledBinaryProvider: () throws -> URL
    var bundledVersionProvider: () throws -> URL
    var bundledTemplateProvider: () throws -> URL

    private static let log = Logger(subsystem: "com.velvt.mac", category: "ServiceManager")

    // MARK: - Initialisers

    public convenience init() {
        let bundle = Bundle.main
        let fm = FileManager.default
        let appSupport = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let support = appSupport.appendingPathComponent("Velvt", isDirectory: true)
        let launchAgents = fm.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents", isDirectory: true)

        self.init(
            fileManager: fm,
            supportDir: support,
            launchAgentsDir: launchAgents,
            registrar: SMServiceRegistrar(plistName: "com.velvt.service.plist"),
            bundledBinaryProvider: {
                guard let url = bundle.url(forResource: "velvt-service", withExtension: nil)
                else { throw ServiceManagerError.binaryNotFoundInBundle }
                return url
            },
            bundledVersionProvider: {
                guard let url = bundle.url(forResource: "velvt-service", withExtension: "version")
                else { throw ServiceManagerError.versionSidecarNotFoundInBundle }
                return url
            },
            bundledTemplateProvider: {
                guard let url = bundle.url(forResource: "com.velvt.service", withExtension: "plist.template")
                else { throw ServiceManagerError.templateNotFoundInBundle }
                return url
            }
        )
    }

    /// Designated initialiser — used directly by unit tests.
    init(
        fileManager: FileManager = .default,
        supportDir: URL,
        launchAgentsDir: URL,
        registrar: any ServiceRegistrar,
        bundledBinaryProvider: @escaping () throws -> URL,
        bundledVersionProvider: @escaping () throws -> URL,
        bundledTemplateProvider: @escaping () throws -> URL
    ) {
        self.fileManager = fileManager
        self.supportDir = supportDir
        self.binaryDest = supportDir.appendingPathComponent("velvt-service")
        self.versionSidecar = supportDir.appendingPathComponent("velvt-service.version")
        self.launchAgentsDir = launchAgentsDir
        self.plistName = "com.velvt.service.plist"
        self.registrar = registrar
        self.bundledBinaryProvider = bundledBinaryProvider
        self.bundledVersionProvider = bundledVersionProvider
        self.bundledTemplateProvider = bundledTemplateProvider
    }

    // MARK: - Public API (non-throwing; errors set state = .failed)

    /// Installs the binary and registers the LaunchAgent if not already present.
    /// Idempotent: returns immediately if the binary already exists at the install path.
    public func ensureInstalled() async {
        guard !fileManager.fileExists(atPath: binaryDest.path) else { return }
        state = .installing
        do {
            try install()
        } catch {
            state = .failed(error)
        }
    }

    /// Compares bundled vs installed version sidecar. Re-installs only if they differ.
    public func ensureUpToDate() async {
        do {
            let bundledVersion = try bundledVersionString()
            let installedVersion = installedVersionString()
            guard bundledVersion != installedVersion else { return }
            state = .updateInProgress
            try? registrar.unregister()
            try install()
        } catch {
            state = .failed(error)
        }
    }

    /// Registers the LaunchAgent if not already enabled. No-op when already running.
    public func start() async {
        guard !registrar.isEnabled else { state = .running; return }
        do {
            try registrar.register()
            state = .running
        } catch {
            state = .failed(error)
        }
    }

    /// Unregisters the LaunchAgent.
    public func stop() async {
        do {
            try registrar.unregister()
            state = .stopped
        } catch {
            state = .failed(error)
        }
    }

    // MARK: - Private

    private func install() throws {
        try fileManager.createDirectory(at: supportDir, withIntermediateDirectories: true)

        let bundledBinary = try bundledBinaryProvider()
        if fileManager.fileExists(atPath: binaryDest.path) {
            try fileManager.removeItem(at: binaryDest)
        }
        try fileManager.copyItem(at: bundledBinary, to: binaryDest)
        try fileManager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: binaryDest.path)

        let bundledSidecar = try bundledVersionProvider()
        if fileManager.fileExists(atPath: versionSidecar.path) {
            try fileManager.removeItem(at: versionSidecar)
        }
        try fileManager.copyItem(at: bundledSidecar, to: versionSidecar)

        try writeLaunchAgentPlist()
        try registrar.register()
        state = .running
        Self.log.info("velvt-service installed and registered")
    }

    private func writeLaunchAgentPlist() throws {
        try fileManager.createDirectory(at: launchAgentsDir, withIntermediateDirectories: true)
        let templateURL = try bundledTemplateProvider()
        var contents = try String(contentsOf: templateURL, encoding: .utf8)
        contents = contents.replacingOccurrences(of: "{{BINARY_PATH}}", with: binaryDest.path)
        let destURL = launchAgentsDir.appendingPathComponent(plistName)
        try contents.write(to: destURL, atomically: true, encoding: .utf8)
    }

    private func bundledVersionString() throws -> String {
        let url = try bundledVersionProvider()
        return try String(contentsOf: url, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func installedVersionString() -> String? {
        (try? String(contentsOf: versionSidecar, encoding: .utf8))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
    }
}

// MARK: - Errors

public enum ServiceManagerError: LocalizedError {
    case binaryNotFoundInBundle
    case versionSidecarNotFoundInBundle
    case templateNotFoundInBundle

    public var errorDescription: String? {
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
