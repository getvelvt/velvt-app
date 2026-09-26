import XCTest

@testable import VelvtMac

/// The early-signal card chooses between two explanations for not having
/// cleared the gate: "keep going" (a countdown) and "Velvt cannot read the apps
/// you are using" (go teach it a category). The second is only ever true of
/// time spent in an app classification failed on — `UNLOGGED` or
/// `UNCLASSIFIED`.
///
/// `SYSTEM` and `IDLE` are also excluded from the signal's numerator, but they
/// are not apps anyone can teach: they are the observer working exactly as
/// intended. Counting them as unreadable time told a user whose day contained
/// an ordinary break that Velvt could not categorize their apps, and sent them
/// to Settings to fix nothing. Advice that points at a thing which is not
/// broken is worse than the stuck countdown it replaced.
final class EarlySignalUnreadableTimeTests: XCTestCase {

    private let base = Date(timeIntervalSince1970: 1_800_000_000)

    // MARK: The two explanations

    /// The bug. A day of nothing but idle time has no teachable seconds in it, so
    /// the honest answer is the countdown.
    func testADayOfPureIdleGetsTheCountdownNotTheAccusation() {
        let snapshot = snapshot(
            segments: [
                segment(id: "idle", category: "IDLE", start: 0, end: 3_600)
            ],
            observedSeconds: 0,
            evidenceEventCount: 40
        )
        XCTAssertEqual(
            observedActivitySeconds(snapshot), 0,
            "idle time is not activity in an app the user can teach")

        let text = EarlySignalProgressView.progressText(
            snapshot.earlySignal,
            totalObservedSeconds: observedActivitySeconds(snapshot)
        )
        XCTAssertTrue(text.contains("more seconds"), text)
        XCTAssertFalse(
            text.contains("cannot categorize"),
            "a day of idle time must not be reported as a classification failure: \(text)")
    }

    /// The other side of the same rule: an hour genuinely spent in apps Velvt
    /// could not classify still gets the explanation, because waiting will not
    /// fix that one.
    func testADayOfUnclassifiedAppTimeGetsTheExplanation() {
        let snapshot = snapshot(
            segments: [
                segment(id: "readable", category: "FOCUS_WORK", start: 0, end: 1),
                segment(id: "unlogged", category: "UNLOGGED", start: 1, end: 1_801),
                segment(id: "unclassified", category: "UNCLASSIFIED", start: 1_801, end: 3_601),
            ],
            observedSeconds: 1,
            evidenceEventCount: 40
        )
        XCTAssertEqual(observedActivitySeconds(snapshot), 3_601)

        let text = EarlySignalProgressView.progressText(
            snapshot.earlySignal,
            totalObservedSeconds: observedActivitySeconds(snapshot)
        )
        XCTAssertTrue(
            text.contains("cannot categorize"),
            "an hour of unclassified app time is exactly what teaching a category fixes: \(text)")
    }

    /// The mixed day that produced the report: a short readable stretch plus a
    /// long break. Before the fix the break alone cleared the 60-second bar and
    /// turned this into the accusation.
    func testIdleAndSystemTimeCannotPushAShortSessionIntoTheAccusation() {
        let snapshot = snapshot(
            segments: [
                segment(id: "readable", category: "FOCUS_WORK", start: 0, end: 20),
                segment(id: "lunch", category: "IDLE", start: 20, end: 3_620),
                segment(id: "login", category: "SYSTEM", start: 3_620, end: 3_920),
            ],
            observedSeconds: 20,
            evidenceEventCount: 6
        )
        XCTAssertEqual(observedActivitySeconds(snapshot), 20)

        let text = EarlySignalProgressView.progressText(
            snapshot.earlySignal,
            totalObservedSeconds: observedActivitySeconds(snapshot)
        )
        XCTAssertTrue(text.contains("more seconds"), text)
        XCTAssertFalse(text.contains("cannot categorize"), text)
    }

    /// Teachable time still wins when it is mixed in with a break, so the fix
    /// does not swing the other way and suppress advice that is warranted.
    func testTeachableTimeStillCountsOnADayThatAlsoContainsABreak() {
        let snapshot = snapshot(
            segments: [
                segment(id: "lunch", category: "IDLE", start: 0, end: 3_600),
                segment(id: "unlogged", category: "UNLOGGED", start: 3_600, end: 4_200),
            ],
            observedSeconds: 0,
            evidenceEventCount: 12
        )
        XCTAssertEqual(observedActivitySeconds(snapshot), 600)

        let text = EarlySignalProgressView.progressText(
            snapshot.earlySignal,
            totalObservedSeconds: observedActivitySeconds(snapshot)
        )
        XCTAssertTrue(text.contains("cannot categorize"), text)
    }

    // MARK: The filter itself

    func testIdleAndSystemAreTheOnlyExcludedCategories() {
        XCTAssertFalse(isTeachableActivityCategory("IDLE"))
        XCTAssertFalse(isTeachableActivityCategory("SYSTEM"))
        XCTAssertTrue(isTeachableActivityCategory("UNLOGGED"))
        XCTAssertTrue(isTeachableActivityCategory("UNCLASSIFIED"))
        XCTAssertTrue(isTeachableActivityCategory("FOCUS_WORK"))
    }

    /// Categories arrive as strings over IPC; the judgement must not hinge on
    /// their casing or on stray whitespace.
    func testTheFilterIgnoresCasingAndWhitespace() {
        XCTAssertFalse(isTeachableActivityCategory("idle"))
        XCTAssertFalse(isTeachableActivityCategory(" System "))
        XCTAssertTrue(isTeachableActivityCategory("focus_work"))
    }

    /// A category nobody here has heard of is activity in some app, and it is
    /// already inside the signal's `observedSeconds`. Counting it in both places
    /// makes it cancel out of the gap rather than invent unreadable time.
    func testAnUnrecognizedCategoryCancelsOutOfTheGap() {
        let snapshot = snapshot(
            segments: [
                segment(id: "future", category: "CREATIVE_PRACTICE", start: 0, end: 3_600)
            ],
            observedSeconds: 3_600,
            evidenceEventCount: 30
        )
        XCTAssertEqual(observedActivitySeconds(snapshot), 3_600)

        let text = EarlySignalProgressView.progressText(
            snapshot.earlySignal,
            totalObservedSeconds: observedActivitySeconds(snapshot)
        )
        XCTAssertFalse(text.contains("cannot categorize"), text)
    }

    /// Segments are half-open and clock changes happen; a negative span must
    /// contribute nothing rather than eat into the total.
    func testAnInvertedSegmentContributesNothing() {
        let snapshot = snapshot(
            segments: [
                segment(id: "inverted", category: "UNLOGGED", start: 600, end: 0),
                segment(id: "real", category: "UNLOGGED", start: 0, end: 30),
            ],
            observedSeconds: 0,
            evidenceEventCount: 4
        )
        XCTAssertEqual(observedActivitySeconds(snapshot), 30)
    }

    // MARK: Fixtures

    private func segment(
        id: String, category: String, start: TimeInterval, end: TimeInterval
    ) -> LocalTimelineSegment {
        LocalTimelineSegment(
            id: id,
            startedAt: base.addingTimeInterval(start),
            endedAt: base.addingTimeInterval(end),
            category: category,
            confidence: isReadable(category)
                ? ClassificationConfidence.high
                : ClassificationConfidence.none
        )
    }

    private func isReadable(_ category: String) -> Bool {
        !["UNLOGGED", "UNCLASSIFIED"].contains(category)
    }

    private func snapshot(
        segments: [LocalTimelineSegment],
        observedSeconds: Int,
        evidenceEventCount: Int
    ) -> LocalDashboardSnapshot {
        LocalDashboardSnapshot(
            generatedAt: base.addingTimeInterval(3_600),
            windowStart: base,
            windowEnd: base.addingTimeInterval(3_600),
            switchCount: 0,
            switchesPerHour: 0,
            coverage: .partial,
            earlySignal: LocalEarlySignal(
                status: .insufficientEvidence,
                observedFrom: segments.first?.startedAt,
                observedThrough: base.addingTimeInterval(3_600),
                observedSeconds: observedSeconds,
                requiredSeconds: max(0, EarlySignalProgressView.readableSecondsRequired - observedSeconds),
                evidenceEventCount: evidenceEventCount,
                focusedSeconds: 0,
                meaningfulSwitchCount: 0,
                longestUninterruptedSeconds: 0,
                observation: nil,
                suggestedAction: nil,
                actionMinutes: 10
            ),
            segments: segments,
            focusFragmentation: nil,
            dailyActivity: []
        )
    }
}
