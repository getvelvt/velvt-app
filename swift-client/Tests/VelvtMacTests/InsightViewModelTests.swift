import XCTest
@testable import VelvtMac

@MainActor
final class InsightViewModelTests: XCTestCase {

    // MARK: - Initial state

    func testInitialStateIsLoading() {
        let sut = InsightViewModel()
        XCTAssertTrue(sut.isLoading)
        XCTAssertTrue(sut.text.isEmpty)
        XCTAssertTrue(sut.date.isEmpty)
        XCTAssertTrue(sut.confidenceLabel.isEmpty)
        XCTAssertTrue(sut.generatedAt.isEmpty)
    }

    // MARK: - update(from:) transitions

    func testUpdateFromPayloadClearsLoadingFlag() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight())
        XCTAssertFalse(sut.isLoading)
    }

    func testUpdateFromPayloadSetsText() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(text: "Deep work sustained across the morning."))
        XCTAssertEqual(sut.text, "Deep work sustained across the morning.")
    }

    func testUpdateFromPayloadSetsGeneratedAt() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight())
        XCTAssertFalse(sut.generatedAt.isEmpty)
        XCTAssertTrue(sut.generatedAt.hasPrefix("Generated "))
    }

    func testUpdateTwiceReflectsLatestPayload() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(text: "First insight."))
        sut.update(from: makeInsight(text: "Second insight."))
        XCTAssertEqual(sut.text, "Second insight.")
    }

    // MARK: - Date formatting

    func testTodayDateString() {
        let today = todayDateString()
        XCTAssertEqual(InsightViewModel.formatDate(today), "Today")
    }

    func testYesterdayDateString() {
        let yesterday = dateString(daysAgo: 1)
        XCTAssertEqual(InsightViewModel.formatDate(yesterday), "Yesterday")
    }

    func testOlderDateFormatsLong() {
        let result = InsightViewModel.formatDate("2026-01-05")
        // Verify it is neither "Today" nor "Yesterday" and contains a month name.
        XCTAssertNotEqual(result, "Today")
        XCTAssertNotEqual(result, "Yesterday")
        XCTAssertTrue(result.contains("January") || result.contains("Jan"),
                      "Expected a formatted month name; got: \(result)")
    }

    func testMalformedDateStringPassedThrough() {
        let result = InsightViewModel.formatDate("not-a-date")
        XCTAssertEqual(result, "not-a-date")
    }

    // MARK: - Confidence label

    func testConfidenceLabelHigh() {
        XCTAssertEqual(InsightViewModel.confidenceLabel(for: .high, isLow: false), "high")
    }

    func testConfidenceLabelMedium() {
        XCTAssertEqual(InsightViewModel.confidenceLabel(for: .medium, isLow: false), "moderate")
    }

    func testConfidenceLabelLow() {
        XCTAssertEqual(InsightViewModel.confidenceLabel(for: .low, isLow: false), "early data")
    }

    func testLowConfidenceFlagOverridesHighLevel() {
        XCTAssertEqual(InsightViewModel.confidenceLabel(for: .high, isLow: true), "early data")
    }

    func testLowConfidenceFlagOverridesMediumLevel() {
        XCTAssertEqual(InsightViewModel.confidenceLabel(for: .medium, isLow: true), "early data")
    }

    // MARK: - generatedAt formatting

    func testGeneratedAtPrefixedCorrectly() {
        let result = InsightViewModel.formatGeneratedAt(Date())
        XCTAssertTrue(result.hasPrefix("Generated "))
    }

    func testGeneratedAtContainsColon() {
        let result = InsightViewModel.formatGeneratedAt(Date())
        XCTAssertTrue(result.contains(":"))
    }

    func testGeneratedAtHasTimeComponent() {
        // "Generated HH:mm" — 8 chars minimum: "Generated 00:00"
        let result = InsightViewModel.formatGeneratedAt(Date())
        XCTAssertGreaterThanOrEqual(result.count, 14)
    }

    // MARK: - Confidence label propagates through update

    func testUpdateSetsConfidenceLabelHigh() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(confidenceLevel: .high, lowConfidence: false))
        XCTAssertEqual(sut.confidenceLabel, "high")
    }

    func testUpdateSetsConfidenceLabelMedium() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(confidenceLevel: .medium, lowConfidence: false))
        XCTAssertEqual(sut.confidenceLabel, "moderate")
    }

    func testUpdateSetsEarlyDataWhenLowConfidence() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(confidenceLevel: .high, lowConfidence: true))
        XCTAssertEqual(sut.confidenceLabel, "early data")
    }

    // MARK: - Insight text is never mutated

    func testInsightTextMatchesPayloadExactly() {
        let raw = "Steady focus, no exclamation mark injected by the UI!"
        let sut = InsightViewModel()
        sut.update(from: makeInsight(text: raw))
        XCTAssertEqual(sut.text, raw)
    }

    func testLongInsightTextStoredVerbatimWithoutTruncation() {
        // 500+ characters: view model must not shorten, clip, or append ellipsis.
        let long = String(repeating: "Focus held steady. ", count: 30) // 570 chars
        let sut = InsightViewModel()
        sut.update(from: makeInsight(text: long))
        XCTAssertEqual(sut.text, long,
                       "Insight text must pass through verbatim regardless of length")
        XCTAssertEqual(sut.text.count, long.count,
                       "Character count must be unchanged after update")
    }

    // MARK: - Rapid successive pushes

    func testTwoRapidPushesShowOnlyLatest() {
        // Simulates Rust resending an updated insight without an intermediate render cycle.
        let sut = InsightViewModel()
        let first  = makeInsight(text: "Initial baseline insight from earlier processing.")
        let second = makeInsight(text: "Revised insight reflecting afternoon context shift.")
        sut.update(from: first)
        sut.update(from: second)
        XCTAssertEqual(sut.text, second.text,
                       "Latest push must win; earlier text must not persist")
        XCTAssertNotEqual(sut.text, first.text)
    }

    func testRapidPushDoesNotLeaveStaleIsLoading() {
        let sut = InsightViewModel()
        sut.update(from: makeInsight(text: "First."))
        sut.update(from: makeInsight(text: "Second."))
        XCTAssertFalse(sut.isLoading)
    }

    // MARK: - Helpers

    private func makeInsight(
        date: String? = nil,
        text: String = "Default insight.",
        confidenceLevel: ConfidenceLevel = .high,
        lowConfidence: Bool = false
    ) -> InsightPayload {
        InsightPayload(
            date: date ?? todayDateString(),
            text: text,
            confidenceLevel: confidenceLevel,
            lowConfidence: lowConfidence,
            generatedAt: Date(timeIntervalSince1970: 1_750_000_000)
        )
    }

    private func todayDateString() -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f.string(from: Date())
    }

    private func dateString(daysAgo: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd"
        f.locale = Locale(identifier: "en_US_POSIX")
        let date = Calendar.current.date(byAdding: .day, value: -daysAgo, to: Date()) ?? Date()
        return f.string(from: date)
    }
}
