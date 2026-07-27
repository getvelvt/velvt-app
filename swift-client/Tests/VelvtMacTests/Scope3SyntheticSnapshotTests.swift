import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

@MainActor
final class Scope3SyntheticSnapshotTests: XCTestCase {
  func testRenderSyntheticScope3SurfacesWhenRequested() async throws {
    guard let output = ProcessInfo.processInfo.environment["VELVT_SCOPE3_SCREENSHOT_DIR"] else {
      throw XCTSkip("Set VELVT_SCOPE3_SCREENSHOT_DIR to render synthetic handoff screenshots")
    }
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    let focus = LocalFocusFragmentation(
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
      phase: .active,
      windowLabel: "Most recent 60 work-block minutes",
      windowStartedAt: base,
      windowEndedAt: base.addingTimeInterval(3_600),
      plannedDurationSeconds: 3_000,
      elapsedDurationSeconds: 2_340,
      longestUninterruptedSeconds: 1_080,
      observedSwitchCount: 5,
      recoveryCount: 2,
      coverage: .good,
      coverageRatio: 0.91,
      comparison: LocalFocusComparison(
        kind: .earlierToday,
        label: "versus earlier today",
        switchDelta: -2,
        explanation: "This comparable window had 2 fewer observed category switches."
      ),
      observation:
        "Velvt observed one switching cluster in this work-block window; clusters describe timing, not intent.",
      nextAction: "Protect the next 10 minutes for the work you chose.",
      segments: [
        segment("focus-1", base, 1_080, "FOCUS_WORK", .high),
        segment("reference-1", base.addingTimeInterval(1_080), 420, "REFERENCE", .medium),
        segment("focus-2", base.addingTimeInterval(1_500), 720, "FOCUS_WORK", .high),
        segment("communication-1", base.addingTimeInterval(2_220), 120, "COMMUNICATION", .medium),
      ],
      transitions: [
        transition("switch-1", base.addingTimeInterval(1_080), "FOCUS_WORK", "REFERENCE"),
        transition("switch-2", base.addingTimeInterval(1_500), "REFERENCE", "FOCUS_WORK"),
        transition("switch-3", base.addingTimeInterval(1_950), "FOCUS_WORK", "REFERENCE"),
        transition("switch-4", base.addingTimeInterval(2_080), "REFERENCE", "FOCUS_WORK"),
        transition("switch-5", base.addingTimeInterval(2_220), "FOCUS_WORK", "COMMUNICATION"),
      ],
      clusters: [
        LocalSwitchingCluster(
          id: "cluster-1",
          ruleVersion: 1,
          startedAt: base.addingTimeInterval(1_950),
          endedAt: base.addingTimeInterval(2_220),
          transitionCount: 3,
          categories: ["FOCUS_WORK", "REFERENCE", "COMMUNICATION"],
          confidence: .medium,
          explanation: "3 switches in 5 minutes between focus work, reference, and communication."
        )
      ]
    )

    let days = (0..<7).map { index in
      LocalDailyActivityDay(
        id: "2026-07-\(14 + index)",
        date: "2026-07-\(14 + index)",
        state: index == 0 ? .noData : (index == 1 ? .lowConfidence : .ready),
        activeSeconds: index == 0 ? 0 : 7_200 + index * 420,
        coverage: index == 0 ? .noData : (index == 1 ? .partial : .good),
        segments: index == 0 ? [] : syntheticDaySegments(dayIndex: index)
      )
    }
    let dashboard = LocalDashboardSnapshot(
      generatedAt: base.addingTimeInterval(3_600),
      windowStart: base,
      windowEnd: base.addingTimeInterval(3_600),
      switchCount: 5,
      switchesPerHour: 5,
      coverage: .good,
      earlySignal: LocalEarlySignal(
        status: .ready,
        observedFrom: base,
        observedThrough: base.addingTimeInterval(3_600),
        observedSeconds: 3_276,
        requiredSeconds: 0,
        evidenceEventCount: 14,
        focusedSeconds: 2_100,
        meaningfulSwitchCount: 5,
        longestUninterruptedSeconds: 1_080,
        observation: "A grounded local observation.",
        suggestedAction: "Protect a short block.",
        actionMinutes: 10
      ),
      segments: focus.segments,
      focusFragmentation: focus,
      dailyActivity: days
    )
    let history = HistoryViewModel()
    history.update(
      from: HistoryPayload(
        days: 14,
        summaries: (0..<14).map { index in
          let recent = index >= 7
          return DailySummary(
            date: String(format: "2026-07-%02d", 7 + index),
            status: .ready,
            eventCount: 48,
            focusScore: recent ? 74 : 61,
            fragmentationScore: recent ? 19 : 28,
            confidenceLevel: .high,
            activeSeconds: 7_200,
            focusedSeconds: recent ? 4_500 : 3_100,
            meaningfulSwitchCount: recent ? 5 : 8,
            longestUninterruptedSeconds: recent ? 1_800 : 1_080
          )
        }
      )
    )

    let renderStartedAt = Date.timeIntervalSinceReferenceDate
    let oneDayHistory = progressiveHistory(observedDays: 1)
    let partialHistory = progressiveHistory(observedDays: 3)
    try render(
      WeekOverWeekCoachingView(availability: .available, viewModel: oneDayHistory),
      named: "progressive-insight-one-day.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 235)
    )
    try render(
      WeekOverWeekCoachingView(availability: .available, viewModel: partialHistory),
      named: "progressive-insight-partial-week.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 235)
    )
    try render(
      WeekOverWeekCoachingView(availability: .available, viewModel: history),
      named: "progressive-insight-full-comparison.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 235)
    )
    let inlineSegment = LocalDailyActivitySegment(
      id: "unknown-inline",
      label: "Unclassified",
      representativeEventID: UUID(uuidString: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
      stableID: "abs_inline_local",
      suggestedName: "Sketch Companion",
      category: "UNCLASSIFIED",
      durationSeconds: 1_200,
      percentage: 100,
      confidence: .none,
      explanation: "Observed locally for 20 minutes."
    )
    try render(
      InlineActivityCorrectionEditor(
        segment: inlineSegment,
        onSave: { _, _ in },
        onCancel: {},
        onUndo: nil
      ),
      named: "inline-activity-naming.png",
      outputDirectory: output,
      size: NSSize(width: 520, height: 165)
    )
    let historyMessages = PassthroughSubject<ServerMessage, Never>()
    let historyModel = MenuStatusViewModel(
      ipcClient: FakeIPCClient(),
      messages: historyMessages
    )
    historyMessages.send(
      .correctionHistoryPage(
        CorrectionHistoryPage(
          items: (0..<20).map { index in
            ClassificationCorrectionSummary(
              stableID: "abs_snapshot_\(index)",
              label: "reference:inferred",
              localLabel: "Local correction \(index + 1)",
              category: index.isMultiple(of: 2) ? "REFERENCE" : "FOCUS_WORK",
              updatedAt: base
            )
          },
          offset: 20,
          pageSize: 20,
          totalCount: 63,
          hasMore: true
        )
      )
    )
    await Task.yield()
    try render(
      CorrectionHistoryBrowser(model: historyModel),
      named: "paginated-correction-history.png",
      outputDirectory: output,
      size: NSSize(width: 560, height: 300)
    )
    try render(
      FocusFragmentationView(focus: focus, errorMessage: nil, onStartWorkBlock: {}),
      named: "scope3-focus-fragmentation-synthetic.png",
      outputDirectory: output
    )
    try render(
      YourWeekContentView(
        snapshot: dashboard,
        historyAvailability: .available,
        historyViewModel: history
      ),
      named: "scope3-daily-activity-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 620, height: 650)
    )
    let renderMilliseconds =
      (Date.timeIntervalSinceReferenceDate - renderStartedAt) * 1_000
    print(String(format: "scope3_compact_popover_surfaces_render_ms=%.3f", renderMilliseconds))
    XCTAssertLessThan(renderMilliseconds, 1_500)
  }

  private func progressiveHistory(observedDays: Int) -> HistoryViewModel {
    let history = HistoryViewModel()
    var summaries: [DailySummary] = []
    for index in 0..<observedDays {
      let dayNumber = 26 - observedDays + index + 1
      let focusScore = 62.0 + Double(index * 3)
      let fragmentationScore = 20.0 - Double(index)
      summaries.append(
        DailySummary(
          date: String(format: "2026-07-%02d", dayNumber),
          status: .ready,
          eventCount: 12 + index,
          focusScore: focusScore,
          fragmentationScore: fragmentationScore,
          confidenceLevel: observedDays == 1 ? .low : .medium,
          activeSeconds: 3_600,
          focusedSeconds: 2_100 + index * 120,
          meaningfulSwitchCount: 3,
          longestUninterruptedSeconds: 1_200
        )
      )
    }
    history.update(from: HistoryPayload(days: 7, summaries: summaries))
    return history
  }

  private func render<V: View>(
    _ view: V,
    named name: String,
    outputDirectory: String,
    size: NSSize = NSSize(width: 620, height: 390)
  ) throws {
    let root = AnyView(
      view
        .padding(18)
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(Color.velvtSurface)
        .preferredColorScheme(.dark)
    )
    let hostingView = NSHostingView(rootView: root)
    hostingView.frame = NSRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      XCTFail("Unable to create snapshot bitmap")
      return
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("Unable to encode snapshot PNG")
      return
    }
    let directory = URL(fileURLWithPath: outputDirectory, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try data.write(to: directory.appendingPathComponent(name), options: .atomic)
  }

  private func segment(
    _ id: String,
    _ start: Date,
    _ duration: TimeInterval,
    _ category: String,
    _ confidence: ClassificationConfidence
  ) -> LocalTimelineSegment {
    LocalTimelineSegment(
      id: id,
      startedAt: start,
      endedAt: start.addingTimeInterval(duration),
      category: category,
      confidence: confidence
    )
  }

  private func transition(
    _ id: String,
    _ occurredAt: Date,
    _ fromCategory: String,
    _ toCategory: String
  ) -> LocalTransitionMarker {
    LocalTransitionMarker(
      id: id,
      occurredAt: occurredAt,
      fromCategory: fromCategory,
      toCategory: toCategory,
      confidence: .medium
    )
  }

  private func syntheticDaySegments(dayIndex: Int) -> [LocalDailyActivitySegment] {
    [
      LocalDailyActivitySegment(
        id: "day-\(dayIndex)-docs", label: "Docs", category: "FOCUS_WORK",
        durationSeconds: 3_600, percentage: 45, confidence: .high,
        explanation:
          "One sustained focus work block lasted 18 minutes; evidence window 14:10–14:28 UTC with high confidence."
      ),
      LocalDailyActivitySegment(
        id: "day-\(dayIndex)-slack", label: "Slack", category: "COMMUNICATION",
        durationSeconds: 2_000, percentage: 25, confidence: .medium,
        explanation:
          "3 switches in 4 minutes included communication; evidence window 15:02–15:06 UTC with medium confidence."
      ),
      LocalDailyActivitySegment(
        id: "day-\(dayIndex)-browser", label: "Browser", category: "REFERENCE",
        durationSeconds: 1_600, percentage: 20, confidence: .medium, explanation: nil
      ),
      LocalDailyActivitySegment(
        id: "day-\(dayIndex)-other", label: "Other", category: "OTHER",
        durationSeconds: 800, percentage: 10, confidence: .none, explanation: nil
      ),
    ]
  }
}
