import XCTest

@testable import VelvtMac

final class SupportDiagnosticsTests: XCTestCase {
    func testBuildNumberIsTheBundleVersion() {
        XCTAssertEqual(
            SupportDiagnostics.buildNumber(infoDictionary: ["CFBundleVersion": "21"]),
            "21"
        )
    }

    func testBuildNumberIsUnknownWithoutABundleVersion() {
        XCTAssertEqual(SupportDiagnostics.buildNumber(infoDictionary: nil), "unknown")
        XCTAssertEqual(SupportDiagnostics.buildNumber(infoDictionary: [:]), "unknown")
        XCTAssertEqual(SupportDiagnostics.buildNumber(infoDictionary: ["CFBundleVersion": ""]), "unknown")
    }

    func testMacOSVersionAlwaysHasThreeParts() {
        XCTAssertEqual(
            SupportDiagnostics.macOSVersion(
                OperatingSystemVersion(majorVersion: 15, minorVersion: 3, patchVersion: 0)
            ),
            "15.3.0"
        )
        XCTAssertEqual(
            SupportDiagnostics.macOSVersion(
                OperatingSystemVersion(majorVersion: 13, minorVersion: 7, patchVersion: 4)
            ),
            "13.7.4"
        )
    }

    func testArchitectureNamesRosettaSeparatelyFromAnIntelMac() {
        XCTAssertEqual(
            SupportDiagnostics.architecture(processIsARM64: true, processIsTranslated: false),
            "arm64"
        )
        XCTAssertEqual(
            SupportDiagnostics.architecture(processIsARM64: false, processIsTranslated: false),
            "x86_64"
        )
        XCTAssertEqual(
            SupportDiagnostics.architecture(processIsARM64: false, processIsTranslated: true),
            "x86_64_rosetta"
        )
    }

    func testCurrentArchitectureIsOneOfTheThreeValues() {
        XCTAssertTrue(
            ["arm64", "x86_64", "x86_64_rosetta"].contains(SupportDiagnostics.currentArchitecture)
        )
    }

    /// The draft goes to hi@, and its body is the person's line followed by
    /// exactly the diagnostics text, decoded back unchanged, `=` and newlines
    /// included.
    func testMailDraftAddressesSupportAndCarriesTheDiagnosticsUnchanged() throws {
        let diagnostics = [
            "Velvt privacy-safe diagnostics",
            "app_version=1.0.15",
            "build_number=21",
            "last_error_code=rate_limited+retry&later",
        ].joined(separator: "\n")

        let url = try XCTUnwrap(SupportDiagnostics.mailDraftURL(diagnostics: diagnostics))
        let components = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))

        XCTAssertEqual(components.scheme, "mailto")
        XCTAssertEqual(components.path, "hi@getvelvt.com")
        let items = try XCTUnwrap(components.queryItems)
        XCTAssertEqual(items.map(\.name), ["subject", "body"])
        XCTAssertEqual(items.first { $0.name == "subject" }?.value, "Velvt support")
        XCTAssertEqual(
            items.first { $0.name == "body" }?.value,
            "What happened?\n\n\n\n\(diagnostics)\n"
        )
    }

    /// `&`, `=` and `+` inside a value would end or split it for a mail app
    /// reading the query, so none of them may appear raw in the body.
    func testMailDraftEncodesQueryDelimitersInsideTheBody() throws {
        let url = try XCTUnwrap(
            SupportDiagnostics.mailDraftURL(diagnostics: "a=b&c+d")
        )
        let query = try XCTUnwrap(
            URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQuery
        )
        let body = try XCTUnwrap(query.components(separatedBy: "&body=").last)

        XCTAssertFalse(body.contains("&"))
        XCTAssertFalse(body.contains("="))
        XCTAssertFalse(body.contains("+"))
        XCTAssertTrue(body.contains("a%3Db%26c%2Bd"))
    }
}
