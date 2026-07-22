import Combine
import XCTest
@testable import VelvtMac

@MainActor
final class UpdateControllerTests: XCTestCase {
    func testConfigurationEnablesOnlyStrictHTTPSAndSignatureSettings() {
        let sut = AppUpdateConfiguration(infoDictionary: validInfoDictionary())

        XCTAssertTrue(sut.isEnabled)
    }

    func testConfigurationRejectsMissingPublicKey() {
        var info = validInfoDictionary()
        info["SUPublicEDKey"] = ""

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsInsecureFeed() {
        var info = validInfoDictionary()
        info["SUFeedURL"] = "http://updates.example.com/appcast.xml"

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsFeedQueryParameters() {
        var info = validInfoDictionary()
        info["SUFeedURL"] = "https://updates.example.com/appcast.xml?device=custom"

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsFeedCredentialsFragmentAndNondefaultPort() {
        for feed in [
            "https://user:password@updates.example.com/appcast.xml",
            "https://updates.example.com/appcast.xml#preview",
            "https://updates.example.com:8443/appcast.xml",
        ] {
            var info = validInfoDictionary()
            info["SUFeedURL"] = feed
            XCTAssertFalse(
                AppUpdateConfiguration(infoDictionary: info).isEnabled,
                "Expected updater configuration to reject \(feed)"
            )
        }
    }

    func testConfigurationRejectsInvalidPublicKey() {
        var info = validInfoDictionary()
        info["SUPublicEDKey"] = "not-a-valid-ed25519-public-key"

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsExpiringSignedFeedFailure() {
        var info = validInfoDictionary()
        info["SUSignedFeedFailureExpirationInterval"] = 1

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsSystemProfiling() {
        var info = validInfoDictionary()
        info["SUEnableSystemProfiling"] = true

        XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
    }

    func testConfigurationRejectsDisabledSignatureEnforcement() {
        for key in ["SURequireSignedFeed", "SUVerifyUpdateBeforeExtraction"] {
            var info = validInfoDictionary()
            info[key] = false
            XCTAssertFalse(AppUpdateConfiguration(infoDictionary: info).isEnabled)
        }
    }

    func testControllerChecksOnlyWhenConfiguredAndAvailable() {
        let checker = TestUpdateChecker(canCheckForUpdates: true)
        let sut = AppUpdateController(
            configuration: AppUpdateConfiguration(infoDictionary: validInfoDictionary()),
            checker: checker
        )

        sut.checkForUpdates()

        XCTAssertEqual(checker.checkCount, 1)
        XCTAssertTrue(sut.canCheckForUpdates)
    }

    func testControllerDoesNotCheckWhenConfigurationIsDisabled() {
        let checker = TestUpdateChecker(canCheckForUpdates: true)
        let sut = AppUpdateController(
            configuration: AppUpdateConfiguration(infoDictionary: [:]),
            checker: checker
        )

        sut.checkForUpdates()

        XCTAssertEqual(checker.checkCount, 0)
        XCTAssertFalse(sut.canCheckForUpdates)
    }

    func testControllerRefreshesAvailability() {
        let checker = TestUpdateChecker(canCheckForUpdates: false)
        let sut = AppUpdateController(
            configuration: AppUpdateConfiguration(infoDictionary: validInfoDictionary()),
            checker: checker
        )
        XCTAssertFalse(sut.canCheckForUpdates)

        checker.canCheckForUpdates = true
        sut.refreshAvailability()

        XCTAssertTrue(sut.canCheckForUpdates)
    }

    func testControllerObservesSparkleAvailabilityChanges() async {
        let checker = TestUpdateChecker(canCheckForUpdates: false)
        let sut = AppUpdateController(
            configuration: AppUpdateConfiguration(infoDictionary: validInfoDictionary()),
            checker: checker
        )
        let enabled = expectation(description: "Updater action becomes available")
        let cancellable = sut.$canCheckForUpdates
            .dropFirst()
            .sink { isAvailable in
                if isAvailable { enabled.fulfill() }
            }

        checker.canCheckForUpdates = true
        await fulfillment(of: [enabled], timeout: 1)

        XCTAssertTrue(sut.canCheckForUpdates)
        withExtendedLifetime(cancellable) {}
    }

    private func validInfoDictionary() -> [String: Any] {
        [
            "VelvtUpdaterEnabled": true,
            "SUFeedURL": "https://updates.example.com/appcast.xml",
            "SUPublicEDKey": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "SURequireSignedFeed": true,
            "SUVerifyUpdateBeforeExtraction": true,
            "SUSignedFeedFailureExpirationInterval": 0,
            "SUEnableSystemProfiling": false,
        ]
    }
}

@MainActor
private final class TestUpdateChecker: UpdateChecking {
    private let availability: CurrentValueSubject<Bool, Never>
    var canCheckForUpdates: Bool {
        get { availability.value }
        set { availability.send(newValue) }
    }
    var canCheckForUpdatesPublisher: AnyPublisher<Bool, Never> {
        availability.eraseToAnyPublisher()
    }
    private(set) var checkCount = 0

    init(canCheckForUpdates: Bool) {
        availability = CurrentValueSubject(canCheckForUpdates)
    }

    func checkForUpdates() {
        checkCount += 1
    }
}
