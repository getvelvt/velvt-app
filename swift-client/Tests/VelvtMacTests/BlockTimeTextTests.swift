import XCTest

@testable import VelvtMac

/// The founder read the dashboard's work-block card as "25m / 12m" and asked
/// for "12m / 25m": the moving number first, the plan it is read against
/// second. Every surface that shows a block's time now takes that order from
/// `BlockTimeText`, and each one hands it the payload it renders rather than
/// two loose integers it could swap.
///
/// Every fixture below uses an elapsed value that differs from its plan, so a
/// swap anywhere reads as a failure rather than as `25m / 25m`.
final class BlockTimeTextTests: XCTestCase {

    // MARK: The formatter

    func testTheLabelNamesElapsedFirst() {
        XCTAssertEqual(BlockTimeText.label, "Elapsed / planned")
        XCTAssertEqual(BlockTimeText.remainingLabel, "Remaining")
    }

    func testCompactPutsElapsedFirstAndThePlanSecond() {
        XCTAssertEqual(BlockTimeText.compact(Timing(elapsed: 720, planned: 1_500)), "12m / 25m")
    }

    func testCompactUsesTheOneDurationRuleOnBothSides() {
        let cases: [(Int, Int, String)] = [
            (0, 1_500, "0s / 25m"),
            (17, 1_500, "17s / 25m"),
            (1_487, 1_500, "24m 47s / 25m"),
            (1_500, 1_500, "25m / 25m"),
            (5_430, 10_800, "1h 30m / 3h"),
            (-5, 300, "0s / 5m"),
        ]
        for (elapsed, planned, expected) in cases {
            XCTAssertEqual(
                BlockTimeText.compact(Timing(elapsed: elapsed, planned: planned)), expected,
                "compact(\(elapsed), \(planned))")
        }
    }

    /// A running or paused block keeps the clock shape the timer draws, and
    /// the plan stays a duration: the one rule `DurationText` states.
    func testClockKeepsElapsedAsAClockAndThePlanAsADuration() {
        XCTAssertEqual(BlockTimeText.clock(Timing(elapsed: 754, planned: 1_500)), "12:34 / 25m")
        XCTAssertEqual(
            BlockTimeText.clock(Timing(elapsed: 5_400, planned: 10_800)), "1:30:00 / 3h")
        XCTAssertEqual(BlockTimeText.plannedSuffix(Timing(elapsed: 754, planned: 1_500)), " / 25m")
    }

    /// The live row has no label above its number, so it says the pair in
    /// words — and in the same order.
    func testSentenceReadsElapsedOfPlanned() {
        let block = Timing(elapsed: 754, planned: 1_500)
        XCTAssertEqual(BlockTimeText.sentence(block), "12:34 elapsed of 25m planned")
        XCTAssertEqual(BlockTimeText.sentenceSuffix(block), " elapsed of 25m planned")
    }

    func testSpokenFormIsASentenceWithoutSlashesOrUnitLetters() {
        let block = Timing(elapsed: 720, planned: 1_500)
        XCTAssertEqual(BlockTimeText.spoken(block), "12 minutes elapsed of 25 minutes planned")
        XCTAssertEqual(BlockTimeText.spokenSuffix(block), "elapsed of 25 minutes planned")
        XCTAssertFalse(BlockTimeText.spoken(block).contains("/"))
    }

    func testSpokenDurationFollowsTheCompactRuleInWords() {
        let cases: [(Int, String)] = [
            (0, "0 seconds"),
            (1, "1 second"),
            (17, "17 seconds"),
            (60, "1 minute"),
            (61, "1 minute 1 second"),
            (90, "1 minute 30 seconds"),
            (1_487, "24 minutes 47 seconds"),
            (1_500, "25 minutes"),
            (3_600, "1 hour"),
            (3_601, "1 hour"),
            (3_660, "1 hour 1 minute"),
            (10_740, "2 hours 59 minutes"),
            (10_800, "3 hours"),
            (-1, "0 seconds"),
        ]
        for (seconds, expected) in cases {
            XCTAssertEqual(DurationText.spoken(seconds), expected, "spoken(\(seconds))")
        }
    }

    // MARK: Each surface, from the payload it renders

    /// The dashboard's work-block card: the surface that printed `25m / 12m`.
    func testDashboardWorkBlockCardReadsElapsedOverPlanned() {
        let focus = Self.focus(elapsed: 720, planned: 1_500)
        XCTAssertEqual(BlockTimeText.compact(focus), "12m / 25m")
        XCTAssertEqual(BlockTimeText.spoken(focus), "12 minutes elapsed of 25 minutes planned")
    }

    /// The one-line live row above the dashboard card, paused or frozen, and
    /// the words that follow its ticking count while it runs.
    func testLiveRowReadsElapsedOfPlanned() {
        let paused = Self.snapshot(phase: .paused, elapsed: 610, planned: 1_500)
        XCTAssertEqual(BlockTimeText.sentence(paused), "10:10 elapsed of 25m planned")
        XCTAssertEqual(
            BlockTimeText.spoken(paused), "10 minutes 10 seconds elapsed of 25 minutes planned")

        let active = Self.snapshot(phase: .active, elapsed: 0, planned: 1_500)
        XCTAssertEqual(BlockTimeText.sentenceSuffix(active), " elapsed of 25m planned")
        XCTAssertEqual(BlockTimeText.spokenSuffix(active), "elapsed of 25 minutes planned")
    }

    /// The work-block card while a block runs: elapsed against the plan in one
    /// column, and the countdown in a separate, separately labelled one.
    func testActiveWorkBlockCardReadsElapsedOverPlannedBesideALabelledCountdown() {
        let paused = Self.snapshot(phase: .paused, elapsed: 610, planned: 1_500)
        XCTAssertEqual(BlockTimeText.clock(paused), "10:10 / 25m")
        XCTAssertEqual(DurationText.clock(paused.remainingDurationSeconds), "14:50")

        let active = Self.snapshot(phase: .active, elapsed: 0, planned: 10_800)
        XCTAssertEqual(BlockTimeText.plannedSuffix(active), " / 3h")
        XCTAssertNotEqual(
            BlockTimeText.label, BlockTimeText.remainingLabel,
            "the countdown must never sit under the elapsed / planned label")
    }

    /// The work-block card once a block is over, and the one label VoiceOver
    /// reads for its whole metric row.
    func testWorkBlockResultCardReadsElapsedOverPlanned() {
        let result = Self.result(elapsed: 1_487, planned: 1_500)
        XCTAssertEqual(BlockTimeText.compact(result), "24m 47s / 25m")
        XCTAssertEqual(
            WorkBlockView.resultMetricsAccessibilityLabel(result),
            "Came back 4 times after 3 switch-aways. 24 minutes 47 seconds elapsed of 25 minutes planned."
        )
    }

    /// An early end is where the order matters most: 12 of a planned 25.
    func testAnEarlyEndReadsTheSameOnEveryFinishedSurface() {
        let result = Self.result(elapsed: 720, planned: 1_500)
        let focus = Self.focus(elapsed: 720, planned: 1_500)
        XCTAssertEqual(BlockTimeText.compact(result), "12m / 25m")
        XCTAssertEqual(BlockTimeText.compact(result), BlockTimeText.compact(focus))
    }

    // MARK: Fixtures

    private struct Timing: BlockTiming {
        let elapsedDurationSeconds: Int
        let plannedDurationSeconds: Int

        init(elapsed: Int, planned: Int) {
            elapsedDurationSeconds = elapsed
            plannedDurationSeconds = planned
        }
    }

    private static func focus(elapsed: Int, planned: Int) -> LocalFocusFragmentation {
        let start = Date(timeIntervalSince1970: 1_800_000_000)
        return LocalFocusFragmentation(
            blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
            phase: .abandoned,
            windowLabel: "12 work-block minutes",
            windowStartedAt: start,
            windowEndedAt: start.addingTimeInterval(TimeInterval(elapsed)),
            plannedDurationSeconds: planned,
            elapsedDurationSeconds: elapsed,
            longestUninterruptedSeconds: 300,
            observedSwitchCount: 3,
            recoveryCount: 1,
            coverage: .good,
            coverageRatio: 0.95,
            comparison: nil,
            observation: "Velvt observed one switching cluster in this work-block window.",
            nextAction: "Protect the next 10 minutes for the work you chose.",
            segments: [],
            transitions: [],
            clusters: []
        )
    }

    private static func snapshot(phase: WorkBlockPhase, elapsed: Int, planned: Int)
        -> WorkBlockSnapshot
    {
        let startedAt = Date(timeIntervalSince1970: 1_800_000_000)
        return WorkBlockSnapshot(
            stateVersion: 1, phase: phase,
            blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            intention: "Draft the report", purpose: .deepWork, intensity: .medium,
            plannedDurationSeconds: planned, elapsedDurationSeconds: elapsed,
            remainingDurationSeconds: max(0, planned - elapsed),
            startedAt: startedAt,
            endsAt: phase == .active ? startedAt.addingTimeInterval(TimeInterval(planned)) : nil,
            pausedAt: phase == .paused ? startedAt.addingTimeInterval(TimeInterval(elapsed)) : nil,
            recoveredAfterRestart: false, currentCategory: "FOCUS_WORK", anchorCategory: nil,
            classificationStatus: .classified, confidence: .high,
            statusLine: "Current category: Focus work.", result: nil, activeIntervention: nil)
    }

    private static func result(elapsed: Int, planned: Int) -> WorkBlockResult {
        WorkBlockResult(
            plannedDurationSeconds: planned, elapsedDurationSeconds: elapsed,
            longestUninterruptedSeconds: 754, switchAwayCount: 3, recoveryCount: 4,
            confidence: .high, coverage: .good, coverageRatio: 0.96,
            safeEvidenceCategory: "FOCUS_WORK",
            observation: "Velvt observed one switching cluster in this work-block window.",
            nextAction: WorkBlockNextAction(
                actionID: "plan_next", label: "Plan another session", durationSeconds: 1_500))
    }
}
