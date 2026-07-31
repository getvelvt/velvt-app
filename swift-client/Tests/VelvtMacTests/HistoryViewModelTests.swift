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

  func testBaselineProgressCountsOnlyRealSummaryDays() {
    let sut = HistoryViewModel()
    sut.update(from: makeHistoryPayload(readyCount: 2))

    XCTAssertEqual(sut.baselineProgress.collectedDays, 2)
    XCTAssertEqual(sut.baselineProgress.label, "Learning from your recent sessions")
    XCTAssertFalse(sut.baselineProgress.isComplete)
  }

  func testSevenVisibleDaysDoNotClaimMatureBaselineEarly() {
    let sut = HistoryViewModel()
    sut.update(from: makeHistoryPayload(readyCount: 7))

    XCTAssertFalse(sut.baselineProgress.isComplete)
    XCTAssertEqual(
      sut.baselineProgress.label,
      "Your personal baseline is becoming more reliable")
  }

  func testBackendMaturityStatusCompletesBaseline() {
    let sut = HistoryViewModel()
    sut.update(from: makeHistoryPayload(readyCount: 7, baselineStatus: "mature"))

    XCTAssertTrue(sut.baselineProgress.isComplete)
    XCTAssertEqual(sut.baselineProgress.label, "Your personal baseline is ready")
  }

  func testTodayMetricsComeDirectlyFromSummaryFields() {
    let summary = DailySummary(
      date: "2026-06-15", status: .ready, eventCount: 50,
      focusScore: 65, fragmentationScore: 22,
      confidenceLevel: .medium, activeSeconds: 7200,
      focusedSeconds: 3900, meaningfulSwitchCount: 14,
      longestUninterruptedSeconds: 1500)

    let row = DaySummaryViewModel(summary)

    XCTAssertEqual(row.focusedTime, "1h 5m")
    XCTAssertEqual(row.meaningfulSwitchCount, 14)
    XCTAssertEqual(row.longestUninterrupted, "25m")
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
      XCTAssertNil(
        day.focusScore,
        "no_data day \(day.id) should have nil focusScore")
      XCTAssertNil(
        day.fragmentationScore,
        "no_data day \(day.id) should have nil fragmentationScore")
      XCTAssertEqual(
        day.activeTime, "—",
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
    let payload = HistoryPayload(
      days: 7,
      summaries: [
        DailySummary(
          date: "2026-06-14", status: .ready, eventCount: 20,
          focusScore: 70.0, fragmentationScore: 18.0,
          confidenceLevel: .medium, activeSeconds: 3600),
        DailySummary(
          date: "2026-06-15", status: .ready, eventCount: 25,
          focusScore: 72.0, fragmentationScore: 17.0,
          confidenceLevel: .medium, activeSeconds: 4200),
      ])
    sut.update(from: payload)
    XCTAssertEqual(
      sut.days.count, 7,
      "Must pad to payload.days even when Rust sends fewer summaries")
  }

  func testPaddedRowsAreNoData() {
    let sut = HistoryViewModel()
    let payload = HistoryPayload(
      days: 7,
      summaries: [
        DailySummary(
          date: "2026-06-14", status: .ready, eventCount: 20,
          focusScore: 70.0, fragmentationScore: 18.0,
          confidenceLevel: .medium, activeSeconds: 3600),
        DailySummary(
          date: "2026-06-15", status: .ready, eventCount: 25,
          focusScore: 72.0, fragmentationScore: 17.0,
          confidenceLevel: .medium, activeSeconds: 4200),
      ])
    sut.update(from: payload)
    // First 5 rows are synthetic stubs — all no_data.
    for row in sut.days.prefix(5) {
      XCTAssertTrue(
        row.isNoData,
        "Padded row \(row.id) should be no_data")
      XCTAssertNil(
        row.focusScore,
        "Padded row \(row.id) must have nil focusScore")
      XCTAssertEqual(
        row.activeTime, "—",
        "Padded row \(row.id) must show — for activeTime")
    }
  }

  func testPaddedRowsAreChronologicallyBeforeRealRows() {
    let sut = HistoryViewModel()
    let payload = HistoryPayload(
      days: 7,
      summaries: [
        DailySummary(
          date: "2026-06-14", status: .ready, eventCount: 20,
          focusScore: 70.0, fragmentationScore: 18.0,
          confidenceLevel: .medium, activeSeconds: 3600),
        DailySummary(
          date: "2026-06-15", status: .ready, eventCount: 25,
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
    XCTAssertEqual(
      sut.days.count, 0,
      "Empty payload with no anchor date cannot be padded")
  }

  // MARK: - Identifier stability

  func testDayIdMatchesDateString() {
    let summary = readySummary(date: "2026-06-10")
    let row = DaySummaryViewModel(summary)
    XCTAssertEqual(row.id, "2026-06-10")
  }

  func testTodaySelectsOnlyExactCurrentLocalDate() {
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 2, summaries: [
      readySummary(date: "2026-07-17"),
      readySummary(date: "2026-07-18"),
    ]))

    XCTAssertEqual(sut.readyDay(for: "2026-07-18")?.id, "2026-07-18")
  }

  func testOlderSummaryCannotMasqueradeAsToday() {
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 1, summaries: [
      readySummary(date: "2026-07-17")
    ]))

    XCTAssertNil(sut.readyDay(for: "2026-07-18"))
    XCTAssertEqual(sut.latestReadyDay?.id, "2026-07-17")
  }

  // MARK: - Week-over-week coaching

  func testWeekOverWeekInsightComparesTwoSevenDayWindows() throws {
    let sut = HistoryViewModel()
    sut.update(from: makeTwoWeekHistory())

    let insight = try XCTUnwrap(sut.progressiveInsight)
    XCTAssertEqual(insight.tier, .weekOverWeek)
    XCTAssertEqual(insight.recentObservedDays, 7)
    XCTAssertEqual(insight.priorObservedDays, 7)
    XCTAssertTrue(insight.observation.contains("60%"))
    XCTAssertTrue(insight.comparison.contains("27 points higher"))
    XCTAssertTrue(insight.comparison.contains("2.0 per hour lower"))
    XCTAssertEqual(insight.evidenceSummary, "7/7 recent days compared with 7/7 prior days")
  }

  func testPartialCoverageNeverMasqueradesAsWeekOverWeek() throws {
    let summaries = makeTwoWeekHistory().summaries.enumerated().map { index, summary in
      index < 5
        ? DailySummary(
            date: summary.date, status: .noData, eventCount: 0,
            focusScore: nil, fragmentationScore: nil,
            confidenceLevel: .none, activeSeconds: 0)
        : summary
    }
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 14, summaries: summaries))

    let insight = try XCTUnwrap(sut.progressiveInsight)
    XCTAssertEqual(insight.tier, .thisWeekSoFar)
    XCTAssertTrue(insight.comparison.contains("not week over week"))
  }

  func testOneObservedDayProducesTodaySoFarInsight() throws {
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 7, summaries: [
      DailySummary(
        date: "2026-07-26", status: .ready, eventCount: 12,
        focusScore: 60, fragmentationScore: 20,
        confidenceLevel: .low, activeSeconds: 3600, focusedSeconds: 1800,
        meaningfulSwitchCount: 3)
    ]))

    let insight = try XCTUnwrap(sut.progressiveInsight)
    XCTAssertEqual(insight.tier, .todaySoFar)
    XCTAssertTrue(insight.observation.contains("50%"))
    XCTAssertEqual(insight.confidenceSummary, "Early confidence")
    XCTAssertTrue(insight.evidenceSummary.contains("current day may be incomplete"))
  }

  func testTwoObservedDaysProducePartialWeekInsight() throws {
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 7, summaries: [
      DailySummary(
        date: "2026-07-25", status: .ready, eventCount: 12,
        focusScore: 60, fragmentationScore: 20,
        confidenceLevel: .medium, activeSeconds: 3600, focusedSeconds: 1800),
      DailySummary(
        date: "2026-07-26", status: .ready, eventCount: 16,
        focusScore: 70, fragmentationScore: 15,
        confidenceLevel: .medium, activeSeconds: 3600, focusedSeconds: 2700),
    ]))

    let insight = try XCTUnwrap(sut.progressiveInsight)
    XCTAssertEqual(insight.tier, .thisWeekSoFar)
    XCTAssertEqual(insight.recentObservedDays, 2)
    XCTAssertTrue(insight.evidenceSummary.contains("partial window"))
  }

  func testZeroActivityDayProducesGroundedNonComparativeInsight() throws {
    let sut = HistoryViewModel()
    sut.update(from: HistoryPayload(days: 7, summaries: [
      DailySummary(
        date: "2026-07-26", status: .ready, eventCount: 0,
        focusScore: nil, fragmentationScore: nil,
        confidenceLevel: .none, activeSeconds: 0)
    ]))

    let insight = try XCTUnwrap(sut.progressiveInsight)
    XCTAssertEqual(insight.tier, .todaySoFar)
    XCTAssertTrue(insight.observation.contains("No qualifying activity"))
    XCTAssertTrue(insight.comparison.contains("not enough active time"))
  }

  // MARK: - Helpers

  private func noDataSummary(date: String = "2026-06-09") -> DailySummary {
    DailySummary(
      date: date, status: .noData, eventCount: 0,
      focusScore: nil, fragmentationScore: nil,
      confidenceLevel: .low, activeSeconds: 0)
  }

  private func readySummary(
    date: String = "2026-06-15",
    focusScore: Double = 65.0,
    fragScore: Double = 22.0,
    activeSeconds: Int = 7200
  ) -> DailySummary {
    DailySummary(
      date: date, status: .ready, eventCount: 50,
      focusScore: focusScore, fragmentationScore: fragScore,
      confidenceLevel: .medium, activeSeconds: activeSeconds)
  }

  private func makeHistoryPayload(
    readyCount: Int = 4,
    baselineStatus: String? = nil
  ) -> HistoryPayload {
    let dates = [
      "2026-06-09", "2026-06-10", "2026-06-11", "2026-06-12",
      "2026-06-13", "2026-06-14", "2026-06-15",
    ]
    let summaries: [DailySummary] = dates.enumerated().map { i, date in
      let isReady = i < readyCount
      if isReady {
        return DailySummary(
          date: date, status: .ready, eventCount: 40 + i * 8,
          focusScore: 55.0 + Double(i * 5),
          fragmentationScore: 30.0 - Double(i * 2),
          confidenceLevel: .medium,
          activeSeconds: 3600 + i * 900,
          baselineStatus: baselineStatus ?? (readyCount >= 7 ? "emerging" : "provisional"))
      }
      return DailySummary(
        date: date, status: .noData, eventCount: 0,
        focusScore: nil, fragmentationScore: nil,
        confidenceLevel: .low, activeSeconds: 0)
    }
    return HistoryPayload(days: 7, summaries: summaries)
  }

  private func makeTwoWeekHistory() -> HistoryPayload {
    let dates = (1 ... 14).map { String(format: "2026-07-%02d", $0) }
    let summaries = dates.enumerated().map { index, date in
      let isRecent = index >= 7
      return DailySummary(
        date: date,
        status: .ready,
        eventCount: 40,
        focusScore: isRecent ? 72 : 58,
        fragmentationScore: isRecent ? 18 : 30,
        confidenceLevel: .high,
        activeSeconds: 3_600,
        focusedSeconds: isRecent ? 2_160 : 1_200,
        meaningfulSwitchCount: isRecent ? 2 : 4,
        longestUninterruptedSeconds: isRecent ? 1_800 : 900
      )
    }
    return HistoryPayload(days: 14, summaries: summaries)
  }
}
