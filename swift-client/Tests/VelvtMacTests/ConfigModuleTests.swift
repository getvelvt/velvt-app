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
