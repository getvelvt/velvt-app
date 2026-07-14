import XCTest

final class BuildConfigurationTests: XCTestCase {
    func testAppBundleDoesNotDeclareLSUIElementSoLaunchServicesCanIndexIt() throws {
        let packageRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let configPaths = [
            packageRoot.appendingPathComponent("Configs/Debug.xcconfig"),
            packageRoot.appendingPathComponent("Configs/Release.xcconfig"),
        ]

        for path in configPaths {
            let contents = try String(contentsOf: path, encoding: .utf8)
            XCTAssertFalse(
                contents.contains("INFOPLIST_KEY_LSUIElement = YES"),
                "\(path.lastPathComponent) must not make the app an LSUIElement agent; runtime activation policy keeps it menu-bar-only."
            )
        }
    }
}
