import Combine
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

    // MARK: - Publish thread

    /// Counters are incremented from the Accessibility collection queue; SwiftUI
    /// requires the publish itself on the main thread.
    func testIncrementFromBackgroundQueuePublishesOnMainThread() {
        let store = AppMetricsStore(defaults: defaults())
        var publishedOnMain: [Bool] = []
        let published = expectation(description: "actionsLogged published")
        published.assertForOverFulfill = false
        let cancellable = store.$actionsLogged
            .dropFirst()
            .sink { _ in
                publishedOnMain.append(Thread.isMainThread)
                published.fulfill()
            }

        DispatchQueue.global().async {
            store.incrementActionsLogged()
        }

        wait(for: [published], timeout: 5)
        cancellable.cancel()

        XCTAssertFalse(publishedOnMain.isEmpty)
        XCTAssertTrue(publishedOnMain.allSatisfy { $0 }, "every publish must happen on the main thread")
        XCTAssertEqual(store.actionsLogged, 1)
    }

    func testInterventionFromBackgroundQueuePublishesOnMainThread() {
        let store = AppMetricsStore(defaults: defaults())
        var publishedOnMain: [Bool] = []
        let published = expectation(description: "interventions published")
        published.assertForOverFulfill = false
        let cancellable = store.$interventions
            .dropFirst()
            .sink { _ in
                publishedOnMain.append(Thread.isMainThread)
                published.fulfill()
            }

        DispatchQueue.global().async {
            store.incrementInterventions()
        }

        wait(for: [published], timeout: 5)
        cancellable.cancel()

        XCTAssertTrue(publishedOnMain.allSatisfy { $0 }, "every publish must happen on the main thread")
        XCTAssertEqual(store.interventions, 1)
    }

    /// The collection path must not queue one main-thread block per Accessibility
    /// event. With the main thread held for the whole burst, every publish scheduled
    /// during it has to coalesce into the single pending hop.
    func testBackgroundBurstCoalescesIntoOnePendingMainThreadPublish() {
        XCTAssertTrue(Thread.isMainThread, "this test must own the main thread to observe coalescing")
        let store = AppMetricsStore(defaults: defaults())
        let burst = 500
        var publishCount = 0
        var publishedOnMain = true
        let cancellable = store.$actionsLogged
            .dropFirst()
            .sink { _ in
                publishedOnMain = publishedOnMain && Thread.isMainThread
                publishCount += 1
            }

        let burstFinished = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            for _ in 0..<burst {
                store.incrementActionsLogged()
            }
            burstFinished.signal()
        }
        XCTAssertEqual(burstFinished.wait(timeout: .now() + 10), .success)

        // Drain whatever the burst left queued on the main thread.
        let drained = expectation(description: "main queue drained")
        DispatchQueue.main.async { drained.fulfill() }
        wait(for: [drained], timeout: 5)
        cancellable.cancel()

        XCTAssertTrue(publishedOnMain, "every publish must happen on the main thread")
        XCTAssertGreaterThanOrEqual(publishCount, 1, "the burst must reach SwiftUI")
        XCTAssertLessThanOrEqual(publishCount, 2, "publishes must coalesce, not queue per event")
        XCTAssertEqual(store.actionsLogged, burst, "the coalesced publish must carry the newest count")
    }

    func testMainThreadIncrementPublishesImmediately() {
        let store = AppMetricsStore(defaults: defaults())
        var publishedValues: [Int] = []
        let cancellable = store.$interventions
            .dropFirst()
            .sink { publishedValues.append($0) }

        store.incrementInterventions()

        XCTAssertEqual(publishedValues, [1], "a main-thread update must not be deferred")
        XCTAssertEqual(store.interventions, 1)
        cancellable.cancel()
    }

    func testSetAuthenticatedFalseFromBackgroundQueueClearsCountersOnMainThread() {
        let suite = defaults()
        let store = AppMetricsStore(defaults: suite)
        store.incrementActionsLogged()
        store.incrementInterventions()
        XCTAssertEqual(store.actionsLogged, 1)

        var publishedOnMain = true
        let cleared = expectation(description: "counters cleared")
        cleared.assertForOverFulfill = false
        let cancellable = store.$actionsLogged
            .dropFirst()
            .sink { value in
                publishedOnMain = publishedOnMain && Thread.isMainThread
                if value == 0 { cleared.fulfill() }
            }

        DispatchQueue.global().async {
            store.setAuthenticated(false)
        }

        wait(for: [cleared], timeout: 5)
        cancellable.cancel()

        XCTAssertTrue(publishedOnMain, "every publish must happen on the main thread")
        XCTAssertEqual(store.actionsLogged, 0)
        XCTAssertEqual(store.interventions, 0)
        XCTAssertFalse(store.isAuthenticated)
        XCTAssertNil(suite.object(forKey: "velvt.metrics.actions_logged"))
        XCTAssertNil(suite.object(forKey: "velvt.metrics.interventions"))
    }

    func testAuthenticatedFlagPublishesOnMainThreadFromBackgroundQueue() {
        let store = AppMetricsStore(defaults: defaults())
        var publishedOnMain = true
        let published = expectation(description: "isAuthenticated published")
        published.assertForOverFulfill = false
        let cancellable = store.$isAuthenticated
            .dropFirst()
            .sink { value in
                publishedOnMain = publishedOnMain && Thread.isMainThread
                if value { published.fulfill() }
            }

        DispatchQueue.global().async {
            store.setAuthenticated(true)
        }

        wait(for: [published], timeout: 5)
        cancellable.cancel()

        XCTAssertTrue(publishedOnMain, "every publish must happen on the main thread")
        XCTAssertTrue(store.isAuthenticated)
    }

    /// Concurrent increments from several queues must not lose counts.
    func testConcurrentIncrementsFromMultipleQueuesKeepEveryCount() {
        let suite = defaults()
        let store = AppMetricsStore(defaults: suite)
        let queues = 4
        let perQueue = 250

        DispatchQueue.concurrentPerform(iterations: queues) { _ in
            for _ in 0..<perQueue {
                store.incrementActionsLogged()
            }
        }

        let settled = expectation(description: "publishes settled")
        DispatchQueue.main.async { DispatchQueue.main.async { settled.fulfill() } }
        wait(for: [settled], timeout: 10)

        XCTAssertEqual(store.actionsLogged, queues * perQueue)
        XCTAssertEqual(suite.integer(forKey: "velvt.metrics.actions_logged"), queues * perQueue)
    }
}
