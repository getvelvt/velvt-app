import XCTest
@testable import VelvtMac

final class ConfigModuleTests: XCTestCase {

    // MARK: - BundleConfigLoader happy path

    func testBundleConfigLoaderAllFieldsRoundTrip() throws {
        let loader = BundleConfigLoader(infoDictionary: validDictionary())
        let config = try loader.load()

        XCTAssertEqual(config.socketPath, "~/.velvt/velvt-service.sock")
        XCTAssertEqual(config.protocolVersion, 19)
        XCTAssertEqual(config.clientVersion, "0.1.0")
        XCTAssertEqual(config.apnsEnvironment, .development)
    }

    func testBundleConfigLoaderProductionAPNSEnv() throws {
        var dict = validDictionary()
        dict["VelvtAPNSEnv"] = "production"
        let config = try BundleConfigLoader(infoDictionary: dict).load()
        XCTAssertEqual(config.apnsEnvironment, .production)
    }

    // MARK: - BundleConfigLoader missing values

    func testBundleConfigLoaderMissingSocketPathThrows() {
        var dict = validDictionary()
        dict.removeValue(forKey: "VelvtSocketPath")
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .missingValue(name: "VelvtSocketPath"))
        }
    }

    func testBundleConfigLoaderEmptySocketPathThrows() {
        var dict = validDictionary()
        dict["VelvtSocketPath"] = ""
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .missingValue(name: "VelvtSocketPath"))
        }
    }

    func testBundleConfigLoaderMissingProtocolVersionThrows() {
        var dict = validDictionary()
        dict.removeValue(forKey: "VelvtProtocolVersion")
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtProtocolVersion"))
        }
    }

    func testBundleConfigLoaderNonNumericProtocolVersionThrows() {
        var dict = validDictionary()
        dict["VelvtProtocolVersion"] = "not-a-number"
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtProtocolVersion"))
        }
    }

    func testBundleConfigLoaderZeroProtocolVersionThrows() {
        var dict = validDictionary()
        dict["VelvtProtocolVersion"] = "0"
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtProtocolVersion"))
        }
    }

    func testBundleConfigLoaderNegativeProtocolVersionThrows() {
        var dict = validDictionary()
        dict["VelvtProtocolVersion"] = "-1"
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtProtocolVersion"))
        }
    }

    func testBundleConfigLoaderMissingClientVersionThrows() {
        var dict = validDictionary()
        dict.removeValue(forKey: "VelvtClientVersion")
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .missingValue(name: "VelvtClientVersion"))
        }
    }

    func testBundleConfigLoaderInvalidAPNSEnvThrows() {
        var dict = validDictionary()
        dict["VelvtAPNSEnv"] = "staging"   // unrecognised value
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtAPNSEnv"))
        }
    }

    func testBundleConfigLoaderMissingAPNSEnvThrows() {
        var dict = validDictionary()
        dict.removeValue(forKey: "VelvtAPNSEnv")
        XCTAssertThrowsError(try BundleConfigLoader(infoDictionary: dict).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VelvtAPNSEnv"))
        }
    }

    func testDistributionPreflightRejectsLocalhost() throws {
        let result = try runDistributionPreflight(url: "https://localhost:8000")
        XCTAssertNotEqual(result, 0)
    }

    func testDistributionPreflightAcceptsHostedHTTPSURL() throws {
        let result = try runDistributionPreflight(url: "https://dev-api.getvelvt.com")
        XCTAssertEqual(result, 0)
    }

    func testProtocolVersionSourcesMatch() throws {
        let root = repositoryRoot
        let proto = try String(
            contentsOf: root.appendingPathComponent("proto/version"),
            encoding: .utf8
        ).trimmingCharacters(in: .whitespacesAndNewlines)
        let debug = try String(
            contentsOf: root.appendingPathComponent("swift-client/Configs/Debug.xcconfig"),
            encoding: .utf8
        )
        let release = try String(
            contentsOf: root.appendingPathComponent("swift-client/Configs/Release.xcconfig"),
            encoding: .utf8
        )

        XCTAssertTrue(debug.contains("VELVT_PROTOCOL_VERSION = \(proto)"))
        XCTAssertTrue(release.contains("VELVT_PROTOCOL_VERSION = \(proto)"))
    }

    // MARK: - EnvironmentConfigLoader (debug builds only)

#if DEBUG
    func testEnvironmentConfigLoaderHappyPath() throws {
        let env: [String: String] = [
            "VELVT_SOCKET_PATH": "~/.velvt/test.sock",
            "VELVT_PROTOCOL_VERSION": "19",
            "VELVT_CLIENT_VERSION": "0.1.0",
        ]
        let config = try EnvironmentConfigLoader(environment: env).load()
        XCTAssertEqual(config.socketPath, "~/.velvt/test.sock")
        XCTAssertEqual(config.protocolVersion, 19)
        XCTAssertEqual(config.clientVersion, "0.1.0")
        XCTAssertEqual(config.apnsEnvironment, .development)
    }

    func testEnvironmentConfigLoaderMissingSocketPath() {
        let env: [String: String] = [
            "VELVT_PROTOCOL_VERSION": "19",
            "VELVT_CLIENT_VERSION": "0.1.0",
        ]
        XCTAssertThrowsError(try EnvironmentConfigLoader(environment: env).load()) { error in
            XCTAssertEqual(error as? ConfigError, .missingValue(name: "VELVT_SOCKET_PATH"))
        }
    }

    func testEnvironmentConfigLoaderInvalidProtocolVersion() {
        let env: [String: String] = [
            "VELVT_SOCKET_PATH": "/tmp/sock",
            "VELVT_PROTOCOL_VERSION": "abc",
            "VELVT_CLIENT_VERSION": "0.1.0",
        ]
        XCTAssertThrowsError(try EnvironmentConfigLoader(environment: env).load()) { error in
            XCTAssertEqual(error as? ConfigError, .invalidValue(name: "VELVT_PROTOCOL_VERSION"))
        }
    }
#endif

    // MARK: - Helpers

    private func validDictionary() -> [String: Any] {
        [
            "VelvtSocketPath": "~/.velvt/velvt-service.sock",
            "VelvtProtocolVersion": "19",
            "VelvtClientVersion": "0.1.0",
            "VelvtAPNSEnv": "development",
            "VelvtAPIBaseURL": "https://staging.api.velvt.test",
        ]
    }

    private var repositoryRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private func runDistributionPreflight(url: String) throws -> Int32 {
        let process = Process()
        process.executableURL = repositoryRoot
            .appendingPathComponent("scripts/preflight_distribution.sh")
        process.arguments = [url]
        process.standardOutput = Pipe()
        process.standardError = Pipe()
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }
}

// MARK: - Declared document-type bound parity

extension ConfigModuleTests {
    /// The document-type bound is written in four places — the Rust constant,
    /// this client's constant, the published JSON schema, and the protocol
    /// changelog. It has already drifted once: the bound was raised to 256
    /// after a census showed Xcode declares 152 document types and Preview 49,
    /// but the client copy stayed at 24, so the client kept abstaining for
    /// exactly the applications the raise was for. Nothing failed. Nothing
    /// logged. The signal was simply absent.
    ///
    /// A stale copy of this number is invisible at runtime, which is why it
    /// gets a test rather than a comment.
    func testDocumentTypeBoundMatchesTheRustConstantAndTheWireSchema() throws {
        let root = repositoryRoot

        let rust = try String(
            contentsOf: root.appendingPathComponent("rust-service/shared-types/src/lib.rs"),
            encoding: .utf8
        )
        let rustBound = try XCTUnwrap(
            firstInteger(in: rust, after: "MAX_DOCUMENT_TYPE_IDS: usize = "),
            "could not read MAX_DOCUMENT_TYPE_IDS from the Rust source"
        )

        let schema = try String(
            contentsOf: root.appendingPathComponent("proto/schema/raw_event.json"),
            encoding: .utf8
        )
        let schemaBound = try XCTUnwrap(
            firstInteger(in: schema, after: "\"maxItems\": "),
            "could not read maxItems from raw_event.json"
        )

        XCTAssertEqual(
            DeclaredDocumentTypeBounds.maximumCount, rustBound,
            "the client bound drifted from the service's — the client will abstain for apps the service would accept"
        )
        XCTAssertEqual(
            schemaBound, rustBound,
            "the published wire contract disagrees with the service it describes"
        )
    }

    private func firstInteger(in haystack: String, after marker: String) -> Int? {
        guard let range = haystack.range(of: marker) else { return nil }
        let digits = haystack[range.upperBound...].prefix { $0.isNumber }
        return Int(digits)
    }
}

// MARK: - Xcode target membership

extension ConfigModuleTests {
    /// Every source file on disk must be in the Xcode target's Sources build
    /// phase.
    ///
    /// SwiftPM globs `Sources/`, so `swift build` and `swift test` compile a
    /// file the moment it exists. The Xcode project compiles an explicit list.
    /// A new file is therefore invisible to every local check and fails only
    /// in `make alpha-dmg` — after a clean, a universal rebuild and minutes of
    /// waiting, with an error ("cannot find type X in scope") that points at
    /// the file that *uses* the missing one rather than the omission itself.
    ///
    /// This has now happened twice: `VelvtTheme.swift` and
    /// `DeclaredAppMetadata.swift`. Both times the whole suite was green.
    func testEverySourceFileIsInTheXcodeTargetSourcesPhase() throws {
        let root = repositoryRoot
        let project = try String(
            contentsOf: root.appendingPathComponent(
                "swift-client/VelvtMac.xcodeproj/project.pbxproj"),
            encoding: .utf8
        )

        // Scoped to the PBXSourcesBuildPhase block on purpose. A `.swift in
        // Sources */` comment also appears on every PBXBuildFile line, so
        // parsing the whole file would make `listed` a superset of what is
        // actually compiled — and a guard that cannot fail is worse than none,
        // because it reads as coverage.
        guard
            let phaseStart = project.range(of: "/* Begin PBXSourcesBuildPhase section */"),
            let phaseEnd = project.range(of: "/* End PBXSourcesBuildPhase section */")
        else {
            return XCTFail("could not locate the Sources build phase in project.pbxproj")
        }
        let phase = project[phaseStart.upperBound..<phaseEnd.lowerBound]

        let listed = Set(
            phase
                .components(separatedBy: " in Sources */")
                .dropLast()
                .compactMap { chunk -> String? in
                    guard let marker = chunk.range(of: "/* ", options: .backwards) else { return nil }
                    let name = chunk[marker.upperBound...]
                    return name.hasSuffix(".swift") ? String(name) : nil
                }
        )
        XCTAssertFalse(listed.isEmpty, "parsed no entries from the Sources phase — the parser is broken")

        let sources = root.appendingPathComponent("swift-client/Sources")
        var onDisk: [String] = []
        let walker = FileManager.default.enumerator(
            at: sources, includingPropertiesForKeys: nil)
        while let url = walker?.nextObject() as? URL {
            if url.pathExtension == "swift" { onDisk.append(url.lastPathComponent) }
        }

        XCTAssertFalse(onDisk.isEmpty, "found no sources to check — the walk is broken, not the project")
        let missing = onDisk.filter { !listed.contains($0) }.sorted()
        XCTAssertTrue(
            missing.isEmpty,
            """
            \(missing.count) source file(s) compile under SwiftPM but are absent from the \
            Xcode target, so the DMG build will fail: \(missing.joined(separator: ", "))
            """
        )
    }
}
