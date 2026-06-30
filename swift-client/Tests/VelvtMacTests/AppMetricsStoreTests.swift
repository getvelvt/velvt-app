import XCTest
@testable import VelvtMac

final class AppMetricsStoreTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let suiteName = "AppMetricsStoreTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return defaults
    }

    func testMetricsPersistAcrossStoreInstances() {
        let defaults = defaults()
        let first = AppMetricsStore(defaults: defaults)

        first.incrementActionsLogged()
        first.incrementActionsLogged()
        first.incrementInterventions()

        let second = AppMetricsStore(defaults: defaults)
        XCTAssertEqual(second.actionsLogged, 2)
        XCTAssertEqual(second.interventions, 1)
    }
}
