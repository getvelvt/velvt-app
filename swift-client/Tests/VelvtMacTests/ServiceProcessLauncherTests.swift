import XCTest
@testable import VelvtMac

final class ServiceProcessLauncherTests: XCTestCase {
    func testPipeDiagnosticRedactsHelperOutputContent() {
        let sensitiveOutput = "Secret Window Title"

        let diagnostic = ServiceProcessLauncher.redactedPipeDiagnostic(
            label: "stderr",
            byteCount: sensitiveOutput.utf8.count
        )

        XCTAssertTrue(diagnostic.contains("stderr"))
        XCTAssertTrue(diagnostic.contains("\(sensitiveOutput.utf8.count)"))
        XCTAssertFalse(diagnostic.contains(sensitiveOutput))
    }
}
