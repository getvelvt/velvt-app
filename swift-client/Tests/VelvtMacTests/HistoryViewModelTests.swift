import XCTest
@testable import VelvtMac

@MainActor
final class HistoryViewModelTests: XCTestCase {

    // MARK: - Initial state

    func testInitialStateIsLoading() {
        let sut = HistoryViewModel()
        XCTAssertTrue(sut.isLoading)
        XCTAssertTrue(sut.days.isEmpty)
    }

    // MARK: - Scroll-to-date

    func testScrollTargetStartsNil() {
        let sut = HistoryViewModel()
        XCTAssertNil(sut.scrollTarget)
    }

    func testScrollToDateSetsScrollTarget() {
        let sut = HistoryViewModel()
        sut.scrollToDate("2026-06-10")
        XCTAssertEqual(sut.scrollTarget, "2026-06-10")
    }

    func testScrollToDateActionDrivesScrollTarget() {
        let sut = HistoryViewModel()
        sut.scrollToDateAction("2026-06-11")
        XCTAssertEqual(sut.scrollTarget, "2026-06-11")
    }

    // MARK: - update(from:) transitions

    func testUpdateClearsLoadingFlag() {
        let sut = HistoryViewModel()
        sut.update(from: makeHistoryPayload())
        XCTAssertFalse(sut.isLoading)
    }

    func testUpdatePopulatesAllSevenDays() {
        let sut = HistoryViewModel()
        sut.update(from: makeHistoryPayload())
        XCTAssertEqual(sut.days.count, 7)
    }

    func testUpdateTwiceReflectsLatestPayload() {
        let sut = HistoryViewModel()
        let first = makeHistoryPayload(readyCount: 1)
        let second = makeHistoryPayload(readyCount: 7)
        sut.update(from: first)
        sut.update(from: second)
        XCTAssertFalse(sut.days.contains(where: { $0.isNoData }))
    }

    func testDayOrderMatchesSummaryOrder() {
        let sut = HistoryViewModel()
        let payload = makeHistoryPayload()
        sut.update(from: payload)
        let expected = payload.summaries.map(\.date)
        let actual = sut.days.map(\.id)
        XCTAssertEqual(actual, expected)
    }

    // MARK: - no_data day rendering

    func testNoDataDayHasNilFocusScore() {
        let row = DaySummaryViewModel(noDataSummary())
        XCTAssertNil(row.focusScore)
    }

    func testNoDataDayHasNilFragmentationScore() {
        let row = DaySummaryViewModel(noDataSummary())
        XCTAssertNil(row.fragmentationScore)
    }

    func testNoDataDayActiveTimeIsDash() {
        let row = DaySummaryViewModel(noDataSummary())
        XCTAssertEqual(row.activeTime, "—")
    }

    func testNoDataDayIsNoDataTrue() {
        let row = DaySummaryViewModel(noDataSummary())
        XCTAssertTrue(row.isNoData)
    }

    func testNoDataDayStatusLabel() {
        let row = DaySummaryViewModel(noDataSummary())
        XCTAssertEqual(row.statusLabel, "no data")
    }

    // MARK: - ready day rendering

    func testReadyDayHasFocusScore() {
        let row = DaySummaryViewModel(readySummary(focusScore: 70.0, fragScore: 20.0))
        XCTAssertNotNil(row.focusScore)
    }

    func testReadyDayHasFragmentationScore() {
        let row = DaySummaryViewModel(readySummary(focusScore: 70.0, fragScore: 20.0))
        XCTAssertNotNil(row.fragmentationScore)
    }

    func testReadyDayStatusLabel() {
        let row = DaySummaryViewModel(readySummary())
        XCTAssertEqual(row.statusLabel, "ready")
    }

    func testReadyDayIsNoDataFalse() {
        let row = DaySummaryViewModel(readySummary())
        XCTAssertFalse(row.isNoData)
    }

    // MARK: - Score rounding

    func testFocusScoreRoundsUp() {
        let row = DaySummaryViewModel(readySummary(focusScore: 72.7))
        XCTAssertEqual(row.focusScore, 73)
    }

    func testFocusScoreRoundsDown() {
        let row = DaySummaryViewModel(readySummary(focusScore: 72.3))
        XCTAssertEqual(row.focusScore, 72)
    }

    func testFragmentationScoreRoundsHalfUp() {
        let row = DaySummaryViewModel(readySummary(focusScore: 60.0, fragScore: 18.5))
        XCTAssertEqual(row.fragmentationScore, 19)
    }

    func testZeroFocusScoreRendersAsZeroNotNil() {
        let row = DaySummaryViewModel(readySummary(focusScore: 0.0))
        XCTAssertEqual(row.focusScore, 0)
    }

    // MARK: - activeTime formatting

    func testActiveTimeHoursAndMinutes() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(7530), "2h 5m")
    }

    func testActiveTimeHoursAndZeroMinutes() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(3600), "1h 0m")
    }

    func testActiveTimeMinutesOnly() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(1950), "32m")
    }

    func testActiveTimeExactMinutes() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(2700), "45m")
    }

    func testActiveTimeZeroSeconds() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(0), "0m")
    }

    func testActiveTimeLessThanOneMinute() {
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(45), "0m")
    }

    func testActiveTimeLargeValue() {
        // 8h 0m
        XCTAssertEqual(DaySummaryViewModel.formatActiveTime(28800), "8h 0m")
    }

    // MARK: - no_data days inside a mixed payload

    func testMixedPayloadNoDataDaysHaveNilScores() {
        let sut = HistoryViewModel()
        sut.update(from: makeHistoryPayload())
        let noDataDays = sut.days.filter { $0.isNoData }
        XCTAssertFalse(noDataDays.isEmpty, "Expected some no_data days in fixture payload")
        for day in noDataDays {
            XCTAssertNil(day.focusScore,
                         "no_data day \(day.id) should have nil focusScore")
            XCTAssertNil(day.fragmentationScore,
                         "no_data day \(day.id) should have nil fragmentationScore")
            XCTAssertEqual(day.activeTime, "—",
                           "no_data day \(day.id) activeTime should be —")
        }
    }

    func testMixedPayloadReadyDaysHaveScores() {
        let sut = HistoryViewModel()
        sut.update(from: makeHistoryPayload())
        let readyDays = sut.days.filter { !$0.isNoData }
        XCTAssertFalse(readyDays.isEmpty)
        for day in readyDays {
            XCTAssertNotNil(day.focusScore)
            XCTAssertNotNil(day.fragmentationScore)
            XCTAssertNotEqual(day.activeTime, "—")
        }
    }

    // MARK: - New-user padding (<7 days from Rust)

    func testFewerThanSevenSummariesPaddedToRequestedDayCount() {
        // Simulates a user on their second day: Rust sends 2 summaries for a 7-day window.
        let sut = HistoryViewModel()
        let payload = HistoryPayload(days: 7, summaries: [
            DailySummary(date: "2026-06-14", status: .ready, eventCount: 20,
                         focusScore: 70.0, fragmentationScore: 18.0,
                         confidenceLevel: .medium, activeSeconds: 3600),
            DailySummary(date: "2026-06-15", status: .ready, eventCount: 25,
                         focusScore: 72.0, fragmentationScore: 17.0,
                         confidenceLevel: .medium, activeSeconds: 4200),
        ])
        sut.update(from: payload)
        XCTAssertEqual(sut.days.count, 7,
                       "Must pad to payload.days even when Rust sends fewer summaries")
    }

    func testPaddedRowsAreNoData() {
        let sut = HistoryViewModel()
        let payload = HistoryPayload(days: 7, summaries: [
            DailySummary(date: "2026-06-14", status: .ready, eventCount: 20,
                         focusScore: 70.0, fragmentationScore: 18.0,
                         confidenceLevel: .medium, activeSeconds: 3600),
            DailySummary(date: "2026-06-15", status: .ready, eventCount: 25,
                         focusScore: 72.0, fragmentationScore: 17.0,
                         confidenceLevel: .medium, activeSeconds: 4200),
        ])
        sut.update(from: payload)
        // First 5 rows are synthetic stubs — all no_data.
        for row in sut.days.prefix(5) {
            XCTAssertTrue(row.isNoData,
                          "Padded row \(row.id) should be no_data")
            XCTAssertNil(row.focusScore,
                         "Padded row \(row.id) must have nil focusScore")
            XCTAssertEqual(row.activeTime, "—",
                           "Padded row \(row.id) must show — for activeTime")
        }
    }

    func testPaddedRowsAreChronologicallyBeforeRealRows() {
        let sut = HistoryViewModel()
        let payload = HistoryPayload(days: 7, summaries: [
            DailySummary(date: "2026-06-14", status: .ready, eventCount: 20,
                         focusScore: 70.0, fragmentationScore: 18.0,
                         confidenceLevel: .medium, activeSeconds: 3600),
            DailySummary(date: "2026-06-15", status: .ready, eventCount: 25,
                         focusScore: 72.0, fragmentationScore: 17.0,
                         confidenceLevel: .medium, activeSeconds: 4200),
        ])
        sut.update(from: payload)
        XCTAssertEqual(sut.days[5].id, "2026-06-14")
        XCTAssertEqual(sut.days[6].id, "2026-06-15")
    }

    func testPaddingDoesNotOccurWhenSummaryCountMatchesDays() {
        // Ensure padding is not applied when Rust sends exactly `days` summaries.
        let sut = HistoryViewModel()
        sut.update(from: makeHistoryPayload())
        XCTAssertEqual(sut.days.count, 7)
    }

    func testPaddingDoesNotOccurForEmptyPayload() {
        let sut = HistoryViewModel()
        let payload = HistoryPayload(days: 7, summaries: [])
        sut.update(from: payload)
        XCTAssertEqual(sut.days.count, 0,
                       "Empty payload with no anchor date cannot be padded")
    }

    // MARK: - Identifier stability

    func testDayIdMatchesDateString() {
        let summary = readySummary(date: "2026-06-10")
        let row = DaySummaryViewModel(summary)
        XCTAssertEqual(row.id, "2026-06-10")
    }

    // MARK: - Helpers

    private func noDataSummary(date: String = "2026-06-09") -> DailySummary {
        DailySummary(date: date, status: .noData, eventCount: 0,
                     focusScore: nil, fragmentationScore: nil,
                     confidenceLevel: .low, activeSeconds: 0)
    }

    private func readySummary(
        date: String = "2026-06-15",
        focusScore: Double = 65.0,
        fragScore: Double = 22.0,
        activeSeconds: Int = 7200
    ) -> DailySummary {
        DailySummary(date: date, status: .ready, eventCount: 50,
                     focusScore: focusScore, fragmentationScore: fragScore,
                     confidenceLevel: .medium, activeSeconds: activeSeconds)
    }

    private func makeHistoryPayload(readyCount: Int = 4) -> HistoryPayload {
        let dates = ["2026-06-09", "2026-06-10", "2026-06-11", "2026-06-12",
                     "2026-06-13", "2026-06-14", "2026-06-15"]
        let summaries: [DailySummary] = dates.enumerated().map { i, date in
            let isReady = i < readyCount
            if isReady {
                return DailySummary(date: date, status: .ready, eventCount: 40 + i * 8,
                                    focusScore: 55.0 + Double(i * 5),
                                    fragmentationScore: 30.0 - Double(i * 2),
                                    confidenceLevel: .medium,
                                    activeSeconds: 3600 + i * 900)
            }
            return DailySummary(date: date, status: .noData, eventCount: 0,
                                focusScore: nil, fragmentationScore: nil,
                                confidenceLevel: .low, activeSeconds: 0)
        }
        return HistoryPayload(days: 7, summaries: summaries)
    }
}
