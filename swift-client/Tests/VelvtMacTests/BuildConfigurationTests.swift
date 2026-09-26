import XCTest

final class BuildConfigurationTests: XCTestCase {
    private let focusStatusUsageDescription =
        "Velvt reads whether Focus is active to avoid interrupting you and report notification protection accurately."

    func testAppBundleDoesNotDeclareLSUIElementSoLaunchServicesCanIndexIt() throws {
        for path in configPaths {
            let contents = try String(contentsOf: path, encoding: .utf8)
            XCTAssertTrue(contents.contains("INFOPLIST_KEY_CFBundleName = Velvt"))
            XCTAssertTrue(contents.contains("INFOPLIST_KEY_CFBundleDisplayName = Velvt"))
            XCTAssertTrue(contents.contains("INFOPLIST_KEY_CFBundleIconFile = AppIcon"))
            XCTAssertTrue(contents.contains("INFOPLIST_KEY_CFBundleIconName = AppIcon"))
            XCTAssertFalse(
                contents.contains("INFOPLIST_KEY_LSUIElement = YES"),
                "\(path.lastPathComponent) must not make the app an LSUIElement agent; runtime activation policy keeps it menu-bar-only."
            )
        }
    }

    func testBuildConfigurationsDeclareFocusStatusUsageDescription() throws {
        let expectedSetting =
            "INFOPLIST_KEY_NSFocusStatusUsageDescription = \(focusStatusUsageDescription)"

        for path in configPaths {
            let contents = try String(contentsOf: path, encoding: .utf8)
            XCTAssertTrue(
                contents.contains(expectedSetting),
                "\(path.lastPathComponent) must declare the nonempty Focus status usage description."
            )
        }
    }

    private var configPaths: [URL] {
        let packageRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return [
            packageRoot.appendingPathComponent("Configs/Debug.xcconfig"),
            packageRoot.appendingPathComponent("Configs/Release.xcconfig"),
        ]
    }
}
