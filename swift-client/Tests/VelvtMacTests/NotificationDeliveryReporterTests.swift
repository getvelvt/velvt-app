import XCTest
import os

@testable import VelvtMac

/// Every delivery line has to survive to `log show`. An `.info` line lives only
/// in memory, so on the founder's Mac a replay of the delivery log after the
/// fact could not tell a delivered drift offer from one that was never
/// attempted. `scripts/watch_notifications.sh` greps for these exact tokens.
final class NotificationDeliveryReporterTests: XCTestCase {
    private let outcomes: [NotificationDeliveryOutcome] = [
        .delivered,
        .suppressedBySalience,
        .withdrawnBeforeDelivery,
        .blockedByPermission(.denied),
        .rejectedByNotificationCentre,
        .listedBehindVisibleCard,
        .bannerWhileActive,
    ]

    func testEveryOutcomeIsLoggedAtALevelTheUnifiedLogPersists() {
        for outcome in outcomes {
            let level = OSLogNotificationDeliveryReporter.line(for: outcome, surface: .driftOffer).level
            XCTAssertTrue(
                level == .default || level == .error || level == .fault,
                "\(outcome) is logged at a level the unified log keeps only in memory"
            )
        }
    }

    func testFailuresAreErrorsAndEverythingElseIsNotice() {
        let levels = outcomes.map {
            OSLogNotificationDeliveryReporter.line(for: $0, surface: .driftOffer).level
        }
        XCTAssertEqual(levels, [.default, .default, .default, .error, .error, .default, .default])
    }

    func testLinesCarryTheTokensTheWatchScriptLooksFor() {
        let messages = outcomes.map {
            OSLogNotificationDeliveryReporter.line(for: $0, surface: .driftOffer).message
        }
        XCTAssertEqual(
            messages,
            [
                "notification_delivered surface=drift_offer",
                "notification_suppressed_by_salience surface=drift_offer",
                "notification_withdrawn_before_delivery surface=drift_offer",
                "error_code=notification_permission_blocked surface=drift_offer status=denied",
                "error_code=notification_centre_rejected surface=drift_offer",
                "notification_presented_while_active surface=drift_offer presentation=list_behind_visible_card",
                "notification_presented_while_active surface=drift_offer presentation=banner",
            ]
        )
    }
}
