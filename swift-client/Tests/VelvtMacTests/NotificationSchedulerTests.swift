import UserNotifications
import XCTest
@testable import VelvtMac

final class UNNotificationSchedulerTests: XCTestCase {

    func testSchedulesImmediatelyWhenNoDoNotDisturb() async {
        let center = FakeUNUserNotificationCenter()
        let metrics = AppMetricsStore(defaults: UserDefaults(suiteName: "NotificationSchedulerTests.\(UUID().uuidString)")!)
        let sut = UNNotificationScheduler(
            center: center,
            now: { Date(timeIntervalSince1970: 1_700_000_000) },
            metrics: metrics
        )
        let payload = NotificationPayload(
            notificationID: UUID(),
            title: "Daily insight",
            body: "Stayed focused through the afternoon.",
            insightDate: "2026-06-15",
            doNotDisturbUntil: nil
        )

        await sut.schedule(payload)

        XCTAssertEqual(center.addedRequests.count, 1)
        XCTAssertNil(center.addedRequests.first?.trigger)
        XCTAssertEqual(center.addedRequests.first?.identifier, payload.notificationID.uuidString)
        XCTAssertEqual(metrics.interventions, 1)
    }

    func testSchedulesImmediatelyWhenDoNotDisturbAlreadyElapsed() async {
        let center = FakeUNUserNotificationCenter()
        let fixedNow = Date(timeIntervalSince1970: 1_700_000_000)
        let sut = UNNotificationScheduler(center: center, now: { fixedNow })
        let payload = NotificationPayload(
            notificationID: UUID(),
            title: "t",
            body: "b",
            insightDate: "2026-06-15",
            doNotDisturbUntil: fixedNow.addingTimeInterval(-60)
        )

        await sut.schedule(payload)

        XCTAssertNil(center.addedRequests.first?.trigger)
    }

    func testSchedulesIntervalTriggerForFutureDoNotDisturb() async {
        let center = FakeUNUserNotificationCenter()
        let fixedNow = Date(timeIntervalSince1970: 1_700_000_000)
        let sut = UNNotificationScheduler(center: center, now: { fixedNow })
        let fiveMinutesOut = fixedNow.addingTimeInterval(5 * 60)
        let payload = NotificationPayload(
            notificationID: UUID(),
            title: "t",
            body: "b",
            insightDate: "2026-06-15",
            doNotDisturbUntil: fiveMinutesOut
        )

        await sut.schedule(payload)

        guard let trigger = center.addedRequests.first?.trigger as? UNTimeIntervalNotificationTrigger else {
            XCTFail("Expected a UNTimeIntervalNotificationTrigger")
            return
        }
        XCTAssertEqual(trigger.timeInterval, 5 * 60, accuracy: 1)
        XCTAssertFalse(trigger.repeats)
    }

    func testRequestContentMatchesPayload() async {
        let center = FakeUNUserNotificationCenter()
        let sut = UNNotificationScheduler(center: center, now: Date.init)
        let payload = NotificationPayload(
            notificationID: UUID(),
            title: "Daily insight",
            body: "Your focus held steady today.",
            insightDate: "2026-06-15",
            doNotDisturbUntil: nil
        )

        await sut.schedule(payload)

        let content = center.addedRequests.first?.content
        XCTAssertEqual(content?.title, "Daily insight")
        XCTAssertEqual(content?.body, "Your focus held steady today.")
        XCTAssertEqual(content?.userInfo["insight_date"] as? String, "2026-06-15")
    }

    func testCancelAllDelegatesToCenter() {
        let center = FakeUNUserNotificationCenter()
        let sut = UNNotificationScheduler(center: center, now: Date.init)

        sut.cancelAll()

        XCTAssertEqual(center.removeAllCallCount, 1)
    }
}
