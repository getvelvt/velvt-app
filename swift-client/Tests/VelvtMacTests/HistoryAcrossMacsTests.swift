import XCTest

@testable import VelvtMac

/// Synced history rows are UTC calendar days that merge every Mac on the
/// account (velvt-core #42). What the app says about them (protocol 37):
/// they are labelled as UTC days, never taken for the local "today"
/// (cross-device-10), and a category's share is of the active time the row
/// shows.
@MainActor
final class HistoryAcrossMacsTests: XCTestCase {

    // MARK: - UTC days

    /// A synced row dated the local today is still a UTC day: west of UTC it
    /// holds yesterday evening, and from late afternoon the local day is
    /// already split across two rows. It is described as a partial week,
    /// never "Today so far".
    func testASyncedRowDatedTodayIsNotTodaySoFar() throws {
        let history = HistoryViewModel()
        history.today = { "2026-10-14" }
        history.update(from: HistoryPayload(days: 1, summaries: [readyDay("2026-10-14")], source: .cloud))

        let insight = try XCTUnwrap(history.progressiveInsight)
        XCTAssertEqual(insight.tier, .thisWeekSoFar)
        XCTAssertFalse(insight.evidenceSummary.contains("may be incomplete"), insight.evidenceSummary)
    }

    /// A history built on this Mac is per local day, so its today is today.
    func testALocalRowDatedTodayIsTodaySoFar() throws {
        let history = HistoryViewModel()
        history.today = { "2026-10-14" }
        history.update(from: HistoryPayload(days: 1, summaries: [readyDay("2026-10-14")], source: .thisMac))

        XCTAssertEqual(try XCTUnwrap(history.progressiveInsight).tier, .todaySoFar)
    }

    func testTheRowsSayWhichCalendarTheyFollow() {
        XCTAssertEqual(HistoryListView.dayBoundaryLabel(for: .cloud, dayCount: 7), "7 UTC days")
        XCTAssertEqual(HistoryListView.dayBoundaryLabel(for: .cloud, dayCount: 1), "1 UTC day")
        XCTAssertEqual(HistoryListView.dayBoundaryLabel(for: .thisMac, dayCount: 14), "14 local days")
        XCTAssertEqual(HistoryListView.caption(for: .cloud), "Privacy-safe cloud summaries")
        XCTAssertEqual(HistoryListView.caption(for: .thisMac), "Built on this Mac")
        XCTAssertEqual(HistoryListView.rowAccessibilityLabel(date: "Tue 14", source: .cloud), "Tue 14, UTC day")
        XCTAssertEqual(HistoryListView.rowAccessibilityLabel(date: "Tue 14", source: .thisMac), "Tue 14")
    }

    // MARK: - Share of active time

    /// The simulation's two-Mac hour: focus on one Mac, video on the other,
    /// both the whole hour. velvt-core's `proportion` is each category's
    /// share of the categories' total (50% each), which the tooltip called
    /// "of active time" beside a row showing one hour. Each was active the
    /// whole hour.
    func testTwoMacsAtOnceEachShowTheirShareOfTheActiveTimeTheRowShows() {
        XCTAssertEqual(
            HistoryListView.segmentHelpText(category: "focus_work", seconds: 3_600, activeSeconds: 3_600),
            "Focus Work: 1h 0m, 100% of active time.")
        XCTAssertEqual(
            HistoryListView.segmentHelpText(category: "passive_consumption", seconds: 3_600, activeSeconds: 3_600),
            "Passive Consumption: 1h 0m, 100% of active time.")
    }

    /// One Mac, no overlap: the share is what `proportion` said.
    func testOneMacsDayReadsAsBefore() {
        XCTAssertEqual(
            HistoryListView.segmentHelpText(category: "focus_work", seconds: 2_700, activeSeconds: 3_600),
            "Focus Work: 45m, 75% of active time.")
        XCTAssertEqual(DaySummaryViewModel.percentOfActiveTime(seconds: 1, activeSeconds: 3), 33)
    }

    func testAShareIsNeverOverAWholeDayOrOfNothing() {
        XCTAssertEqual(DaySummaryViewModel.percentOfActiveTime(seconds: 4_000, activeSeconds: 3_600), 100)
        XCTAssertNil(DaySummaryViewModel.percentOfActiveTime(seconds: 60, activeSeconds: 0))
        XCTAssertEqual(
            HistoryListView.segmentHelpText(category: "focus_work", seconds: 60, activeSeconds: 0),
            "Focus Work: 1m.")
    }

    // MARK: - Helpers

    private func readyDay(_ date: String) -> DailySummary {
        DailySummary(
            date: date, status: .ready, eventCount: 12,
            focusScore: 60, fragmentationScore: 20,
            confidenceLevel: .low, activeSeconds: 3_600, focusedSeconds: 1_800,
            meaningfulSwitchCount: 3)
    }
}
