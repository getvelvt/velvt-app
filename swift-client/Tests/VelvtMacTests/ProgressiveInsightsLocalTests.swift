import AppKit
import SwiftUI
import XCTest

@testable import VelvtMac

/// What the Patterns card says about a history built on this Mac (protocol
/// 33): it draws an insight from it, says where it came from, and promises
/// nothing about when the synced summaries return.
@MainActor
final class ProgressiveInsightsLocalTests: XCTestCase {

    // MARK: - The card

    /// Fourteen days built on this Mac, four or more observed in each week,
    /// reach the tier the cloud's seven never could, and the card draws the
    /// insight rather than a placeholder, with its caption under it.
    func testTheCardRendersAnInsightFromAHistoryBuiltOnThisMac() throws {
        let history = HistoryViewModel()
        history.update(from: localHistoryWeeks(readyRecent: 5, readyPrior: 4))

        XCTAssertEqual(history.source, .thisMac)
        XCTAssertEqual(history.days.count, 14)
        let insight = try XCTUnwrap(history.progressiveInsight)
        XCTAssertEqual(insight.tier, .weekOverWeek)
        XCTAssertEqual(insight.evidenceSummary, "5/7 recent days compared with 4/7 prior days")
        XCTAssertTrue(insight.observation.hasPrefix("Focus-oriented work represented"))
        XCTAssertNil(WeekOverWeekCoachingView.placeholder(availability: .available, viewModel: history))

        let withInsight = fittingHeight(
            WeekOverWeekCoachingView(availability: .available, isSignedIn: true, viewModel: history))
        let loading = fittingHeight(
            WeekOverWeekCoachingView(availability: .loading, isSignedIn: true, viewModel: HistoryViewModel()))
        XCTAssertGreaterThan(withInsight, loading + 40, "the card drew a placeholder, not the insight")
    }

    /// The cloud's seven days are seven days: no prior week is padded in
    /// front of them, so week over week is never claimed from them.
    func testASyncedWeekIsNotPaddedIntoAFortnight() throws {
        let history = HistoryViewModel()
        let week = localHistoryWeeks(readyRecent: 7, readyPrior: 0).summaries.suffix(7)
        history.update(from: HistoryPayload(days: 7, summaries: Array(week), source: .cloud))

        XCTAssertEqual(history.days.count, 7)
        XCTAssertEqual(try XCTUnwrap(history.progressiveInsight).tier, .thisWeekSoFar)
    }

    /// Days built on this Mac feed the Patterns card only. The Today tab's
    /// baseline label counts days toward the cloud's baseline, and these are
    /// not days the cloud holds.
    func testAHistoryBuiltOnThisMacDoesNotCountTowardTheCloudsBaseline() {
        let history = HistoryViewModel()
        history.update(from: localHistoryWeeks(readyRecent: 7, readyPrior: 7))

        XCTAssertEqual(history.baselineProgress.collectedDays, 0)
        XCTAssertNil(history.todayReadyDay)
    }

    func testTheCaptionSaysWhereTheHistoryCameFromAndPromisesNothing() {
        XCTAssertEqual(
            WeekOverWeekCoachingView.caption(for: .thisMac, isSignedIn: true),
            "From this Mac. Synced daily summaries are unavailable right now.")
        XCTAssertEqual(
            WeekOverWeekCoachingView.caption(for: .thisMac, isSignedIn: false),
            "From this Mac. Synced daily summaries need you to be signed in.")
        XCTAssertNil(WeekOverWeekCoachingView.caption(for: .cloud, isSignedIn: true))
        XCTAssertNil(WeekOverWeekCoachingView.caption(for: nil, isSignedIn: false))
    }

    func testEveryPlaceholderIsOneTheCardCanKeep() {
        let loading = HistoryViewModel()
        XCTAssertEqual(
            WeekOverWeekCoachingView.placeholder(availability: .loading, viewModel: loading),
            "Loading daily summaries.")
        XCTAssertEqual(
            WeekOverWeekCoachingView.placeholder(availability: .notGenerated, viewModel: loading),
            "Daily summaries could not be read on this Mac. Velvt asks for them each time Patterns opens.")

        let quietMac = HistoryViewModel()
        quietMac.update(from: localHistoryWeeks(readyRecent: 0, readyPrior: 0))
        XCTAssertEqual(
            WeekOverWeekCoachingView.placeholder(availability: .available, viewModel: quietMac),
            "No activity was recorded on this Mac in the last 7 days.")

        let quietCloud = HistoryViewModel()
        quietCloud.update(
            from: HistoryPayload(
                days: 7,
                summaries: Array(localHistoryWeeks(readyRecent: 0, readyPrior: 0).summaries.suffix(7)),
                source: .cloud))
        XCTAssertEqual(
            WeekOverWeekCoachingView.placeholder(availability: .available, viewModel: quietCloud),
            "No qualifying activity is available yet.")

        let every = [
            WeekOverWeekCoachingView.loadingCopy,
            WeekOverWeekCoachingView.unavailableCopy,
            WeekOverWeekCoachingView.noLocalActivityCopy,
            WeekOverWeekCoachingView.noSyncedActivityCopy,
            WeekOverWeekCoachingView.thisMacSignedInCaption,
            WeekOverWeekCoachingView.thisMacSignedOutCaption,
        ]
        for copy in every {
            let lowered = copy.lowercased()
            for promise in ["catch up", "on its own", "just now", "will be back", "again"] {
                XCTAssertFalse(lowered.contains(promise), "\(copy) promises what nothing keeps")
            }
        }
    }

    // MARK: - Helpers

    private func fittingHeight(_ view: some View) -> CGFloat {
        let host = NSHostingView(rootView: view.frame(width: 420))
        host.layoutSubtreeIfNeeded()
        return host.fittingSize.height
    }
}
