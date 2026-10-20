import ServiceManagement
import XCTest

@testable import VelvtMac

final class OpenAtLoginDefaultTests: XCTestCase {
    private var suiteName: String!
    private var defaults: UserDefaults!
    private let applications = URL(fileURLWithPath: "/Applications", isDirectory: true)
    private let userApplications = URL(fileURLWithPath: "/Users/someone/Applications", isDirectory: true)

    override func setUp() {
        super.setUp()
        suiteName = "OpenAtLoginDefaultTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        defaults = nil
        super.tearDown()
    }

    func testFirstLaunchFromApplicationsRegistersAndRecordsIt() {
        let item = FakeLoginItem(status: .notRegistered)

        XCTAssertEqual(makeSUT(item, bundle: "/Applications/Velvt.app").apply(), .registered)

        XCTAssertEqual(item.registerCalls, 1)
        XCTAssertTrue(defaults.bool(forKey: OpenAtLoginDefault.appliedDefaultsKey))
    }

    /// The off switch: once someone removes Velvt from Login Items, no later
    /// launch puts it back.
    func testALaterLaunchLeavesAnItemThePersonRemovedAlone() {
        let item = FakeLoginItem(status: .notRegistered)
        makeSUT(item, bundle: "/Applications/Velvt.app").apply()
        item.status = .notRegistered

        XCTAssertEqual(makeSUT(item, bundle: "/Applications/Velvt.app").apply(), .alreadyApplied)

        XCTAssertEqual(item.registerCalls, 1)
    }

    /// A DMG, a translocated download, or a development build: registering
    /// any of these would point Login Items at a copy that goes away. Nothing
    /// is recorded, so the launch from Applications still applies it.
    func testOutsideAnApplicationsFolderNothingIsRegisteredOrRecorded() {
        for path in [
            "/Volumes/Velvt/Velvt.app",
            "/private/var/folders/xy/T/AppTranslocation/0A1B/d/Velvt.app",
            "/Users/someone/Library/Developer/Xcode/DerivedData/VelvtMac/Build/Products/Debug/Velvt.app",
            "/Applications/Velvt",
        ] {
            let item = FakeLoginItem(status: .notRegistered)

            XCTAssertEqual(makeSUT(item, bundle: path).apply(), .notInApplications, path)

            XCTAssertEqual(item.registerCalls, 0, path)
            XCTAssertFalse(defaults.bool(forKey: OpenAtLoginDefault.appliedDefaultsKey), path)
        }

        let item = FakeLoginItem(status: .notRegistered)
        XCTAssertEqual(makeSUT(item, bundle: "/Applications/Velvt.app").apply(), .registered)
    }

    func testTheUserApplicationsFolderCounts() {
        let item = FakeLoginItem(status: .notRegistered)

        XCTAssertEqual(
            makeSUT(item, bundle: "/Users/someone/Applications/Velvt.app").apply(),
            .registered
        )
    }

    func testAnItemAlreadyInLoginItemsIsRecordedWithoutRegisteringAgain() {
        for status in [SMAppService.Status.enabled, .requiresApproval] {
            defaults.removeObject(forKey: OpenAtLoginDefault.appliedDefaultsKey)
            let item = FakeLoginItem(status: status)

            XCTAssertEqual(
                makeSUT(item, bundle: "/Applications/Velvt.app").apply(),
                .alreadyRegistered
            )

            XCTAssertEqual(item.registerCalls, 0)
            XCTAssertTrue(defaults.bool(forKey: OpenAtLoginDefault.appliedDefaultsKey))
        }
    }

    func testARefusalIsNotRecordedSoTheNextLaunchTriesAgain() {
        let item = FakeLoginItem(status: .notRegistered)
        item.registerError = NSError(domain: "SMAppServiceErrorDomain", code: 1)

        XCTAssertEqual(makeSUT(item, bundle: "/Applications/Velvt.app").apply(), .failed)
        XCTAssertFalse(defaults.bool(forKey: OpenAtLoginDefault.appliedDefaultsKey))

        item.registerError = nil
        XCTAssertEqual(makeSUT(item, bundle: "/Applications/Velvt.app").apply(), .registered)
        XCTAssertEqual(item.registerCalls, 2)
    }

    /// Onboarding's sentence is shown only while it is true.
    func testOpensAtLoginOnlyWhileMacOSWillOpenIt() {
        let expectations: [(SMAppService.Status, Bool)] = [
            (.enabled, true),
            (.requiresApproval, false),
            (.notRegistered, false),
            (.notFound, false),
        ]
        for (status, expected) in expectations {
            XCTAssertEqual(
                makeSUT(FakeLoginItem(status: status), bundle: "/Applications/Velvt.app").opensAtLogin,
                expected
            )
        }
    }

    private func makeSUT(_ item: FakeLoginItem, bundle path: String) -> OpenAtLoginDefault {
        OpenAtLoginDefault(
            defaults: defaults,
            loginItem: item,
            bundleURL: URL(fileURLWithPath: path, isDirectory: true),
            applicationsDirectories: [applications, userApplications]
        )
    }
}

private final class FakeLoginItem: LoginItemRegistering {
    var status: SMAppService.Status
    var registerError: Error?
    private(set) var registerCalls = 0

    init(status: SMAppService.Status) {
        self.status = status
    }

    func register() throws {
        registerCalls += 1
        if let registerError { throw registerError }
        status = .enabled
    }
}
