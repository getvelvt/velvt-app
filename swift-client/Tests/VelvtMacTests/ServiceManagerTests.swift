import XCTest
import Combine
@testable import VelvtMac

// MARK: - Mock registrar

final class MockServiceRegistrar: ServiceRegistrar {
    var isEnabled = false
    var registerCallCount = 0
    var unregisterCallCount = 0
    var shouldThrowOnRegister = false
    var shouldThrowOnUnregister = false

    func register() throws {
        if shouldThrowOnRegister { throw MockServiceError.registrationFailed }
        registerCallCount += 1
        isEnabled = true
    }

    func unregister() throws {
        if shouldThrowOnUnregister { throw MockServiceError.unregistrationFailed }
        unregisterCallCount += 1
        isEnabled = false
    }
}

enum MockServiceError: LocalizedError {
    case registrationFailed
    case unregistrationFailed
    var errorDescription: String? {
        switch self {
        case .registrationFailed: return "Mock registration failure."
        case .unregistrationFailed: return "Mock unregistration failure."
        }
    }
}

// MARK: - Test helper

@MainActor
private func makeManager(
    tempDir: URL,
    registrar: any ServiceRegistrar,
    bundledBinaryURL: URL? = nil,
    bundledVersion: String = "1.0.0",
    installedVersion: String? = nil
) throws -> ServiceManager {
    let support = tempDir.appendingPathComponent("support", isDirectory: true)
    let launchAgents = tempDir.appendingPathComponent("LaunchAgents", isDirectory: true)
    try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: launchAgents, withIntermediateDirectories: true)

    // Write bundled version sidecar.
    let bundledVersionURL = tempDir.appendingPathComponent("bundled.version")
    try bundledVersion.write(to: bundledVersionURL, atomically: true, encoding: .utf8)

    // Optionally write installed version sidecar (simulates a prior install).
    if let iv = installedVersion {
        let ivURL = support.appendingPathComponent("velvt-service.version")
        try iv.write(to: ivURL, atomically: true, encoding: .utf8)
    }

    // Write a minimal plist template.
    let templateURL = tempDir.appendingPathComponent("com.velvt.service.plist.template")
    let templateContent = """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>Label</key><string>com.velvt.service</string>
        <key>ProgramArguments</key><array><string>{{BINARY_PATH}}</string></array>
        <key>RunAtLoad</key><true/>
        <key>KeepAlive</key><true/>
        <key>StandardErrorPath</key><string>/tmp/velvt-service.err</string>
    </dict>
    </plist>
    """
    try templateContent.write(to: templateURL, atomically: true, encoding: .utf8)

    // Write a fake binary if requested.
    let binaryURL = bundledBinaryURL ?? {
        let u = tempDir.appendingPathComponent("velvt-service")
        FileManager.default.createFile(atPath: u.path, contents: Data("fake".utf8))
        return u
    }()

    // Write a fake bundled taxonomy — the helper resolves it beside its own
    // executable, so install() must carry it into the support directory.
    let taxonomyURL = tempDir.appendingPathComponent("abstraction-taxonomy-mvp-1.json")
    try #"{"version":"test"}"#.write(to: taxonomyURL, atomically: true, encoding: .utf8)

    return ServiceManager(
        fileManager: .default,
        supportDir: support,
        launchAgentsDir: launchAgents,
        registrar: registrar,
        bundledBinaryProvider: { binaryURL },
        bundledVersionProvider: { bundledVersionURL },
        bundledTemplateProvider: { templateURL },
        bundledTaxonomyProvider: { taxonomyURL }
    )
}

// MARK: - Tests

@MainActor
final class ServiceManagerTests: XCTestCase {

    private var tempDir: URL!
    private var cancellables = Set<AnyCancellable>()

    override func setUp() async throws {
        try await super.setUp()
        tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: tempDir)
        cancellables.removeAll()
        try await super.tearDown()
    }

    // MARK: 1 — ensureInstalled is idempotent

    func testEnsureInstalledIdempotent() async throws {
        let registrar = MockServiceRegistrar()
        let manager = try makeManager(tempDir: tempDir, registrar: registrar)

        await manager.ensureInstalled()
        await manager.ensureInstalled()   // second call must be a no-op

        XCTAssertEqual(registrar.registerCallCount, 1,
            "register() must be called exactly once even when ensureInstalled is called twice")
        XCTAssertEqual(manager.state, .running)
    }

    // MARK: 2 — ensureUpToDate no-op when versions match

    func testEnsureUpToDateVersionsMatchIsNoOp() async throws {
        let registrar = MockServiceRegistrar()
        let manager = try makeManager(
            tempDir: tempDir,
            registrar: registrar,
            bundledVersion: "1.0.0",
            installedVersion: "1.0.0"   // same version already installed
        )

        await manager.ensureUpToDate()

        XCTAssertEqual(registrar.registerCallCount, 0,
            "register() must not be called when versions match")
        XCTAssertEqual(registrar.unregisterCallCount, 0,
            "unregister() must not be called when versions match")
        XCTAssertEqual(manager.state, .notInstalled, "state must not change on version match no-op")
    }

    // MARK: 3 — ensureUpToDate triggers update cycle with correct state transitions

    func testEnsureUpToDateVersionMismatchTriggersCycle() async throws {
        let registrar = MockServiceRegistrar()
        let manager = try makeManager(
            tempDir: tempDir,
            registrar: registrar,
            bundledVersion: "1.1.0",
            installedVersion: "1.0.0"   // stale installed version
        )
        manager.state = .running

        var observedStates: [ManagedServiceState] = []
        manager.$state
            .sink { observedStates.append($0) }
            .store(in: &cancellables)

        await manager.ensureUpToDate()

        XCTAssertEqual(registrar.unregisterCallCount, 1,
            "unregister() must be called before overwriting the binary")
        XCTAssertEqual(registrar.registerCallCount, 1,
            "register() must be called after overwriting the binary")
        XCTAssertEqual(manager.state, .running)
        XCTAssertTrue(
            observedStates.contains(.updateInProgress),
            "state must pass through .updateInProgress during the update cycle"
        )
        XCTAssertEqual(observedStates.last, .running,
            "state must be .running after a successful update")
    }

    // MARK: 4 — ensureInstalled with missing bundle binary → .failed

    func testEnsureInstalledMissingBundledBinaryFails() async throws {
        let registrar = MockServiceRegistrar()
        let support = tempDir.appendingPathComponent("support", isDirectory: true)
        let launchAgents = tempDir.appendingPathComponent("LaunchAgents", isDirectory: true)
        let bundledVersionURL = tempDir.appendingPathComponent("bundled.version")
        let templateURL = tempDir.appendingPathComponent("com.velvt.service.plist.template")
        try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: launchAgents, withIntermediateDirectories: true)
        try "1.0.0".write(to: bundledVersionURL, atomically: true, encoding: .utf8)
        try "<plist/>".write(to: templateURL, atomically: true, encoding: .utf8)

        let manager = ServiceManager(
            fileManager: .default,
            supportDir: support,
            launchAgentsDir: launchAgents,
            registrar: registrar,
            bundledBinaryProvider: { throw ServiceManagerError.binaryNotFoundInBundle },
            bundledVersionProvider: { bundledVersionURL },
            bundledTemplateProvider: { templateURL }
        )

        await manager.ensureInstalled()

        if case .failed(let error) = manager.state {
            XCTAssertTrue(
                error.localizedDescription.contains("velvt-service"),
                "Error description must mention the binary name"
            )
        } else {
            XCTFail("state must be .failed when the bundled binary is missing, got \(manager.state)")
        }
        XCTAssertEqual(registrar.registerCallCount, 0,
            "register() must not be called when binary is missing")
    }

    // MARK: 5 — SMAppService.register() throws → .failed, no crash

    func testRegisterThrowsTransitionsToFailed() async throws {
        let registrar = MockServiceRegistrar()
        registrar.shouldThrowOnRegister = true
        let manager = try makeManager(tempDir: tempDir, registrar: registrar)

        await manager.ensureInstalled()

        if case .failed(let error) = manager.state {
            XCTAssertEqual(
                error.localizedDescription, MockServiceError.registrationFailed.errorDescription
            )
        } else {
            XCTFail("state must be .failed when register() throws, got \(manager.state)")
        }
    }

    // MARK: 5b — Taxonomy travels with the binary

    func testInstallCopiesTaxonomyBesideBinary() async throws {
        let registrar = MockServiceRegistrar()
        let manager = try makeManager(tempDir: tempDir, registrar: registrar)

        await manager.ensureInstalled()

        // The helper resolves its taxonomy as a sibling of its own executable
        // and has no environment variable to fall back on under launchd. If
        // this copy is missing it silently exits at startup, which takes
        // sign-in down with it.
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: manager.taxonomyDest.path),
            "install() must copy the taxonomy into the support directory"
        )
        XCTAssertEqual(
            manager.taxonomyDest.deletingLastPathComponent(),
            manager.binaryDest.deletingLastPathComponent(),
            "taxonomy must be installed beside the binary, not merely somewhere"
        )
    }

    // MARK: 6 — Plist template substitution and no EnvironmentVariables key

    func testRenderedPlistHasNoEnvironmentVariablesKey() async throws {
        let registrar = MockServiceRegistrar()
        let manager = try makeManager(tempDir: tempDir, registrar: registrar)
        await manager.ensureInstalled()

        let plistPath = tempDir
            .appendingPathComponent("LaunchAgents")
            .appendingPathComponent("com.velvt.service.plist")

        XCTAssertTrue(
            FileManager.default.fileExists(atPath: plistPath.path),
            "com.velvt.service.plist must exist in LaunchAgents directory"
        )

        let contents = try String(contentsOf: plistPath, encoding: .utf8)
        XCTAssertFalse(
            contents.contains("EnvironmentVariables"),
            "LaunchAgent plist must not contain an EnvironmentVariables key"
        )
        XCTAssertFalse(
            contents.contains("{{BINARY_PATH}}"),
            "Template token {{BINARY_PATH}} must be substituted"
        )
        XCTAssertTrue(
            contents.contains(manager.binaryDest.path),
            "Rendered plist must contain the installed binary path"
        )
        // Verify the rendered plist points into the configured support directory
        // (the real install uses ~/Library/Application Support/Velvt/).
        XCTAssertTrue(
            contents.contains(manager.supportDir.path),
            "Rendered plist must reference a path within the configured support directory"
        )
    }
}
