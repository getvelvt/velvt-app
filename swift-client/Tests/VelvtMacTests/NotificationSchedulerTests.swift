import Combine
import UserNotifications
import XCTest

@testable import VelvtMac

final class UNNotificationSchedulerTests: XCTestCase {

    func testSchedulesImmediatelyWhenNoDoNotDisturb() async {
        let center = FakeUNUserNotificationCenter()
        let metrics = AppMetricsStore(
            defaults: UserDefaults(suiteName: "NotificationSchedulerTests.\(UUID().uuidString)")!)
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

        // `AppMetricsStore` republishes its `@Published` mirror on the main
        // queue, and `schedule` does not run there, so the mirror is read after
        // that hop rather than before it -- waiting on the publisher, not on a
        // clock.
        let reachedOne = expectation(description: "interventions mirror reaches 1")
        reachedOne.assertForOverFulfill = false
        let cancellable = metrics.$interventions
            .filter { $0 == 1 }
            .sink { _ in reachedOne.fulfill() }

        await sut.schedule(payload)

        XCTAssertEqual(center.addedRequests.count, 1)
        XCTAssertNil(center.addedRequests.first?.trigger)
        XCTAssertEqual(center.addedRequests.first?.identifier, payload.notificationID.uuidString)

        await fulfillment(of: [reachedOne], timeout: 5)
        cancellable.cancel()

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
        XCTAssertNotNil(content?.sound)
    }

    /// The reminder carries its marker and nothing else: no prompt id, no
    /// text, no insight date. It is posted at once, under one identifier so a
    /// new one replaces the last, and it is not an intervention.
    func testTheCategoryReminderIsImmediateMarkedAndNotCountedAsAnIntervention() async {
        let center = FakeUNUserNotificationCenter()
        let metrics = CountingMetrics()
        let sut = UNNotificationScheduler(center: center, now: Date.init, metrics: metrics)

        let posted = await sut.scheduleCategoryPrompt(
            title: "A site needs a category",
            body: "1 site you used this week doesn't have a category yet. Choose once in Velvt.")

        XCTAssertTrue(posted)
        let request = center.addedRequests.first
        XCTAssertEqual(request?.identifier, categoryPromptNotificationIdentifier)
        XCTAssertNil(request?.trigger)
        XCTAssertEqual(request?.content.title, "A site needs a category")
        XCTAssertEqual(
            request?.content.body,
            "1 site you used this week doesn't have a category yet. Choose once in Velvt.")
        XCTAssertEqual(request?.content.userInfo.count, 1)
        XCTAssertEqual(request?.content.userInfo[categoryPromptNotificationUserInfoKey] as? Bool, true)
        XCTAssertEqual(metrics.interventionIncrements, 0)
    }

    func testARejectedCategoryReminderReportsFailure() async {
        let center = FakeUNUserNotificationCenter()
        center.rejectAdds(with: NSError(domain: UNErrorDomain, code: 1))
        let sut = UNNotificationScheduler(center: center, now: Date.init)

        let posted = await sut.scheduleCategoryPrompt(title: "t", body: "b")

        XCTAssertFalse(posted)
        XCTAssertTrue(center.addedRequests.isEmpty)
    }

    func testCancelAllDelegatesToCenter() {
        let center = FakeUNUserNotificationCenter()
        let sut = UNNotificationScheduler(center: center, now: Date.init)

        sut.cancelAll()

        XCTAssertEqual(center.removeAllCallCount, 1)
    }
}

private final class CountingMetrics: AppMetricsCounting, @unchecked Sendable {
    private let lock = NSLock()
    private var interventionCount = 0

    var actionsLogged: Int { 0 }
    var interventions: Int { lock.withLock { interventionCount } }
    var interventionIncrements: Int { interventions }

    func incrementActionsLogged() {}

    func incrementInterventions() {
        lock.withLock { interventionCount += 1 }
    }
}
