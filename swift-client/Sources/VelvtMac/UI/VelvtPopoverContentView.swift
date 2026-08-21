import AppKit
import SwiftUI

extension View {
    func tourHighlight(_ isHighlighted: Bool) -> some View {
        overlay {
            if isHighlighted {
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(Color.velvtPink, lineWidth: 2)
                    .allowsHitTesting(false)
            }
        }
    }
}
enum TodayObservationKind: Equatable {
    case cloud
    case earlyLocal
    case progress
}

enum TodayObservationResolver {
    static func resolve(
        cloudAvailable: Bool,
        cloudSourceDate: String,
        currentLocalDate: String,
        earlySignalStatus: LocalEarlySignalStatus?
    ) -> TodayObservationKind {
        if cloudAvailable && cloudSourceDate == currentLocalDate {
            return .cloud
        }
        if earlySignalStatus == .ready {
            return .earlyLocal
        }
        return .progress
    }
}

// MARK: - VelvtPopoverContentView

/// Root content for the menu bar popover.
///
/// Switches between skeleton, populated insight/history, and error states.
/// The error state renders skeleton content plus a muted inline status banner —
/// no alert, no modal, no dismissal required.
public struct VelvtPopoverContentView: View {
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator

    public init(coordinator: ConcreteDisplayDataCoordinator) {
        self.coordinator = coordinator
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            switch coordinator.state {
            case .loading:
                HistorySkeletonView()
                    .padding(.bottom, 8)

            case .populated(_, let historyVM):
                historySection(viewModel: historyVM)

            case .error(let message):
                HistorySkeletonView()
                IPCStatusBanner(message: message)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 10)
            }
        }
        .preferredColorScheme(.dark)
    }

    @ViewBuilder
    private func historySection(viewModel: HistoryViewModel) -> some View {
        switch coordinator.historyAvailability {
        case .available:
            HistoryListView(viewModel: viewModel)
                .padding(.bottom, 8)
        case .notGenerated:
            EmptyDeliveryState(text: "No daily history generated yet", systemImage: "calendar")
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
        case .loading:
            HistorySkeletonView()
                .padding(.bottom, 8)
        }
    }
}

public struct TodayWorkspaceView: View {
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
    @ObservedObject private var localDashboardCoordinator: LocalDashboardCoordinator
    private let highlightsEarlySignal: Bool

    public init(
        coordinator: ConcreteDisplayDataCoordinator,
        workBlockCoordinator: WorkBlockCoordinator,
        localDashboardCoordinator: LocalDashboardCoordinator,
        highlightsEarlySignal: Bool = false
    ) {
        self.coordinator = coordinator
        self.workBlockCoordinator = workBlockCoordinator
        self.localDashboardCoordinator = localDashboardCoordinator
        self.highlightsEarlySignal = highlightsEarlySignal
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            baselineStatus
            dailyMetrics
            observation
        }
        .padding(.vertical, 12)
    }

    private var baselineStatus: some View {
        Label(
            coordinator.historyViewModel.baselineProgress.label,
            systemImage: coordinator.historyViewModel.baselineProgress.isComplete
                ? "checkmark.circle"
                : "circle.dotted"
        )
        .font(.caption)
        .foregroundStyle(Color.velvtMuted)
        .padding(.horizontal, 16)
        .accessibilityHint("Built only from days with real privacy-safe summaries")
    }

    @ViewBuilder
    private var dailyMetrics: some View {
        if let day = coordinator.historyViewModel.todayReadyDay {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    metricViews(for: day)
                }
                VStack(spacing: 8) {
                    metricViews(for: day)
                }
            }
            .padding(.horizontal, 16)
        } else if let signal = readyLocalSignal {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { localMetricViews(for: signal) }
                VStack(spacing: 8) { localMetricViews(for: signal) }
            }
            .padding(.horizontal, 16)
        } else {
            EarlySignalProgressView(
                signal: localDashboardCoordinator.snapshot?.earlySignal,
                errorMessage: localDashboardCoordinator.commandError
            )
            .padding(.horizontal, 16)
            .tourHighlight(highlightsEarlySignal)
        }
    }

    @ViewBuilder
    private func localMetricViews(for signal: LocalEarlySignal) -> some View {
        todayMetric(
            title: "Focused time",
            value: DaySummaryViewModel.formatActiveTime(signal.focusedSeconds),
            explanation: "Observed time in the broad focus-work category during this local window."
        )
        todayMetric(
            title: "Meaningful switches",
            value: "\(signal.meaningfulSwitchCount)",
      explanation:
        "Changes between privacy-safe broad categories; system and unclassified activity are excluded."
        )
        todayMetric(
            title: "Longest stretch",
            value: DaySummaryViewModel.formatActiveTime(signal.longestUninterruptedSeconds),
            explanation: "The longest observed privacy-safe category stretch in this local window."
        )
    }

    @ViewBuilder
    private func metricViews(for day: DaySummaryViewModel) -> some View {
        todayMetric(
            title: "Focused time",
            value: day.focusedTime,
            explanation: "Time in broad focus-oriented work categories."
        )
        todayMetric(
            title: "Meaningful switches",
            value: "\(day.meaningfulSwitchCount)",
            explanation: "Changes between broad work categories; brief system activity is excluded."
        )
        todayMetric(
            title: "Longest block",
            value: day.longestUninterrupted,
            explanation: "Your longest recorded work block without a broad-category switch."
        )
    }

    private func todayMetric(title: String, value: String, explanation: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(value)
                .font(.title3.bold().monospacedDigit())
                .foregroundStyle(Color.velvtText)
            Text(title)
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .frame(maxWidth: .infinity, minHeight: 68, alignment: .leading)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .help(explanation)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(title)
        .accessibilityValue(value)
        .accessibilityHint(explanation)
    }

    @ViewBuilder
    private var observation: some View {
        switch observationKind {
        case .cloud:
            InsightCardView(
                viewModel: coordinator.insightViewModel,
                onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
                    ? startSuggestedWorkBlock
                    : nil
            )
                .padding(.horizontal, 16)
        case .earlyLocal:
            if let signal = readyLocalSignal {
                EarlyLocalSignalView(
                    signal: signal,
                    onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
                        ? { startEarlySignalWorkBlock(signal) }
                        : nil
                )
                .padding(.horizontal, 16)
                .tourHighlight(highlightsEarlySignal)
            }
        case .progress:
            if coordinator.insightNotReadyReason != "insufficient_evidence"
        || coordinator.historyViewModel.baselineProgress.isComplete
      {
                EmptyDeliveryState(
                    text: todayProgressExplanation,
                    systemImage: "sparkles"
                )
                .padding(.horizontal, 16)
            }
        }
    }

    private var todayProgressExplanation: String {
        switch coordinator.insightNotReadyReason {
        case "backend_unavailable":
            "Working offline. Your local progress remains available while cloud synchronization retries."
        case "insufficient_evidence":
            "No cloud observation was generated because evidence is still limited; the local signal will appear first."
        default:
            "A local observation will replace this progress state once enough evidence is available."
        }
    }

    private var observationKind: TodayObservationKind {
        TodayObservationResolver.resolve(
            cloudAvailable: coordinator.insightAvailability == .available,
            cloudSourceDate: coordinator.insightViewModel.sourceDate,
            currentLocalDate: HistoryViewModel.localDateString(),
            earlySignalStatus: localDashboardCoordinator.snapshot?.earlySignal.status
        )
    }

    private var readyLocalSignal: LocalEarlySignal? {
        guard let signal = localDashboardCoordinator.snapshot?.earlySignal,
      signal.status == .ready
    else { return nil }
        return signal
    }

    private func startSuggestedWorkBlock() {
        workBlockCoordinator.startBlock(
            intention: nil,
            durationSeconds: coordinator.insightViewModel.suggestedActionMinutes * 60,
            purpose: nil,
            intensity: .medium
        )
    }

    private func startEarlySignalWorkBlock(_ signal: LocalEarlySignal) {
        workBlockCoordinator.startBlock(
            intention: nil,
            durationSeconds: signal.actionMinutes * 60,
            purpose: nil,
            intensity: .medium
        )
    }
}

private struct EarlySignalProgressView: View {
    let signal: LocalEarlySignal?
    let errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Building an early local signal", systemImage: "waveform.path")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Color.velvtText)
            if let signal {
                ProgressView(
                    value: Double(signal.observedSeconds),
                    total: Double(max(1, signal.observedSeconds + signal.requiredSeconds))
                )
                    .tint(Color.velvtPink)
                Text(progressText(signal))
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
        Text(
          "Updated \(signal.observedThrough.formatted(date: .omitted, time: .shortened)) · raw app names, titles, URLs, and files stay on this Mac"
        )
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
                EarlySignalBasisDisclosure(signal: signal)
            } else {
        Text(
          errorMessage ?? "Waiting for the local privacy service to report this observation window."
        )
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    /// Only "qualifying" activity counts toward the signal — an app Velvt
    /// cannot categorize contributes nothing. So a person using apps outside
    /// the dictionary sees a countdown that never moves, and waiting is the
    /// one thing that will not fix it. Distinguishing the two cases turns a
    /// stuck progress bar into something the user can act on.
    private func progressText(_ signal: LocalEarlySignal) -> String {
        let sawActivityButCouldNotUseIt =
            signal.evidenceEventCount > 0 && signal.observedSeconds == 0
        if sawActivityButCouldNotUseIt {
            return
                "Velvt has seen activity but cannot categorize the apps you are using, so none of it counts yet. "
                + "Set a category for an app in Settings and every window of it will be recognized from then on."
        }
        if signal.requiredSeconds > 0 {
            return
                "Velvt needs about \(signal.requiredSeconds) more seconds of activity it can categorize before showing your first local pattern."
        }
        return
            "Velvt is checking that this activity can be summarized without exposing private details."
    }
}

/// The bare label "Early local signal" names a mechanism the reader cannot
/// interrogate: it appears before any baseline exists, computed from evidence
/// they never see. This is the same seam as "Why am I seeing this?" on a cloud
/// insight — the claim, the numbers behind it, and the boundary that produced
/// them, available on demand rather than crowding the card.
///
/// Every value shown here is already on the client, so this costs no protocol
/// change and asserts nothing that is not stored.
private struct EarlySignalBasisDisclosure: View {
    let signal: LocalEarlySignal?
    @State private var isExpanded = false

    var body: some View {
        DisclosureGroup("What is an early local signal?", isExpanded: $isExpanded) {
            VStack(alignment: .leading, spacing: 6) {
                Text(
                    "A first read on how today is going, computed on this Mac from abstracted "
                        + "categories — so it can appear before a seven-day baseline exists."
                )
                if let basisText {
                    Text(basisText)
                }
                // Deliberately narrower than "nothing leaves this Mac":
                // abstracted category events do sync in upload batches
                // (`BatchEvent`). What is claimed here is what the code
                // enforces — raw context is never an input, and the signal
                // itself has no upload path.
                Text(
                    "App names, window titles, URLs, and file paths are never read into it. "
                        + "The signal itself is computed and kept on this Mac."
                )
                // The one place the caveat belongs. It used to be bolted onto
                // four separate claims — two sentences from Rust and two
                // accessibility hints on the timeline — and a hedge attached
                // to a claim retracts the claim. Said once, here, where the
                // reader came looking for it, it is a description of the
                // method instead.
                Text(
                    "Velvt can see what you moved between, never why. It counts the moves and "
                        + "leaves the meaning to you."
                )
            }
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 5)
        }
        .font(.caption)
        .tint(Color.velvtText)
        .accessibilityHint(
            "Explains what an early local signal is and the privacy-safe numbers behind this one"
        )
    }

    private var basisText: String? {
        guard let signal, signal.evidenceEventCount > 0 else { return nil }
        var parts = [
            "\(signal.evidenceEventCount) categorized "
                + (signal.evidenceEventCount == 1 ? "observation" : "observations")
        ]
        if signal.focusedSeconds > 0 {
            parts.append("\(durationText(signal.focusedSeconds)) focused")
        }
        if signal.longestUninterruptedSeconds > 0 {
            parts.append(
                "longest uninterrupted stretch \(durationText(signal.longestUninterruptedSeconds))"
            )
        }
        if signal.meaningfulSwitchCount > 0 {
            parts.append(
                "\(signal.meaningfulSwitchCount) meaningful "
                    + (signal.meaningfulSwitchCount == 1 ? "switch" : "switches")
            )
        }
        let lead = signal.status == .ready ? "Behind this one: " : "Observed so far: "
        return lead + parts.joined(separator: ", ") + "."
    }

    private func durationText(_ seconds: Int) -> String {
        seconds < 60 ? "\(seconds)s" : "\(seconds / 60) min"
    }
}

private struct EarlyLocalSignalView: View {
    let signal: LocalEarlySignal
    let onSuggestedAction: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                Text("Early local signal")
                    .font(.caption.bold())
                    .foregroundStyle(Color.velvtPink)
                Spacer()
                Text(windowText)
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            }
            Text(signal.observation ?? "Your activity is still settling.")
                .font(.body.weight(.medium))
                .foregroundStyle(Color.velvtText)
                .fixedSize(horizontal: false, vertical: true)
            if let suggestion = signal.suggestedAction {
                Text(suggestion)
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let onSuggestedAction, signal.actionMinutes > 0 {
                Button("Protect \(signal.actionMinutes) minutes", action: onSuggestedAction)
                    .buttonStyle(.borderedProminent)
            }
            EarlySignalBasisDisclosure(signal: signal)
      Text(
        "Computed only from abstracted categories on this Mac · Updated \(signal.observedThrough.formatted(date: .omitted, time: .shortened))"
      )
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(14)
        .background(Color.velvtSurface)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private var windowText: String {
        guard let start = signal.observedFrom else { return "Current window" }
    return
      "\(start.formatted(date: .omitted, time: .shortened))–\(signal.observedThrough.formatted(date: .omitted, time: .shortened))"
    }
}

struct EmptyDeliveryState: View {
    let text: String
    let systemImage: String

    var body: some View {
        Label(text, systemImage: systemImage)
            .font(.caption)
            .foregroundStyle(.secondary)
    }
}

/// The Now workspace. Seven-day activity belongs in Patterns so the same
/// monitor is not repeated in two tabs.
public struct MinimalDashboardWorkspaceView: View {
  @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
  @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
  @ObservedObject private var localDashboardCoordinator: LocalDashboardCoordinator
  private let onStartWorkBlock: () -> Void
  private let highlightsInsight: Bool
  private let highlightsFocus: Bool

  public init(
    coordinator: ConcreteDisplayDataCoordinator,
    workBlockCoordinator: WorkBlockCoordinator,
    localDashboardCoordinator: LocalDashboardCoordinator,
    onStartWorkBlock: @escaping () -> Void,
    highlightsInsight: Bool = false,
    highlightsFocus: Bool = false
  ) {
    self.coordinator = coordinator
    self.workBlockCoordinator = workBlockCoordinator
    self.localDashboardCoordinator = localDashboardCoordinator
    self.onStartWorkBlock = onStartWorkBlock
    self.highlightsInsight = highlightsInsight
    self.highlightsFocus = highlightsFocus
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if let snapshot = workBlockCoordinator.snapshot,
        snapshot.phase == .active || snapshot.phase == .paused
      {
        CompactWorkBlockControl(snapshot: snapshot, coordinator: workBlockCoordinator)
      }

      latestInsight
        .tourHighlight(highlightsInsight)

      FocusFragmentationView(
        focus: localDashboardCoordinator.snapshot?.focusFragmentation,
        errorMessage: localDashboardCoordinator.commandError,
        onStartWorkBlock: onStartWorkBlock,
        header: workBlockCardHeader
      )
      .tourHighlight(highlightsFocus)
    }
    .padding(12)
    .onAppear { localDashboardCoordinator.refresh() }
  }

  /// The intention the user typed when they declared this block, else the
  /// anchor category Rust derived for it. Both are values already on the
  /// snapshot; this chooses between two given strings and derives neither.
  private var workBlockCardHeader: String? {
    guard let snapshot = workBlockCoordinator.snapshot else { return nil }
    if let intention = snapshot.intention,
      !intention.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    {
      return intention
    }
    guard let anchor = snapshot.result?.safeEvidenceCategory ?? snapshot.currentCategory else {
      return nil
    }
    return friendlyCategory(anchor)
  }

  @ViewBuilder
  private var latestInsight: some View {
    if coordinator.insightAvailability == .available {
      InsightCardView(
        viewModel: coordinator.insightViewModel,
        onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
          ? onStartWorkBlock
          : nil,
        compact: true
      )
    } else if let signal = localDashboardCoordinator.snapshot?.earlySignal,
      signal.status == .ready
    {
      EarlyLocalSignalView(
        signal: signal,
        onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
          ? onStartWorkBlock
          : nil
      )
    } else {
      EarlySignalProgressView(
        signal: localDashboardCoordinator.snapshot?.earlySignal,
        errorMessage: localDashboardCoordinator.commandError
      )
    }
  }

}

private struct CompactWorkBlockControl: View {
  let snapshot: WorkBlockSnapshot
  @ObservedObject var coordinator: WorkBlockCoordinator

  var body: some View {
    HStack(spacing: 10) {
      VStack(alignment: .leading, spacing: 2) {
        Text(snapshot.phase == .paused ? "Work block paused" : "Work block active")
          .font(.caption.bold())
        Text(
          "\(duration(snapshot.elapsedDurationSeconds)) elapsed of \(duration(snapshot.plannedDurationSeconds)) planned"
        )
        .font(.caption2.monospacedDigit())
        .foregroundStyle(Color.velvtMuted)
      }
      Spacer(minLength: 8)
      if snapshot.phase == .paused {
        Button("Resume") { coordinator.resume() }
      } else {
        Button("Pause") { coordinator.pause() }
      }
      Button("End", role: .destructive) { coordinator.end() }
    }
    .buttonStyle(.bordered)
    .controlSize(.small)
    .padding(10)
    .background(Color.velvtPanel)
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Active work block")
    .accessibilityValue(
      "\(duration(snapshot.elapsedDurationSeconds)) elapsed, \(duration(snapshot.plannedDurationSeconds)) planned"
    )
  }
}

public struct FocusFragmentationView: View {
  let focus: LocalFocusFragmentation?
  let errorMessage: String?
  let onStartWorkBlock: () -> Void
  /// What this block is about, in the user's own words if they gave any, and
  /// otherwise the anchor category Rust already derived for it. Passed in
  /// rather than computed: Swift renders derivations, it does not perform
  /// them, and neither the intention nor the anchor is on this DTO.
  var header: String?
  @State private var hoveredDetail: String?
  @FocusState private var focusedEvidenceID: String?

  public var body: some View {
    VStack(alignment: .leading, spacing: 7) {
      if let focus {
        // Observation first, then the one action, then the evidence behind
        // them. A chart cannot tell someone what just happened to their
        // attention.
        // Leading with the chart put the only two sentences that carry meaning
        // at the bottom of the card in caption text, truncated, with the real
        // wording reachable only by hovering — which is the roadmap's
        // "one observation, one bounded action; scores never lead" inverted.
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          Text(focus.observation)
            .font(.subheadline)
            .fixedSize(horizontal: false, vertical: true)
          Spacer(minLength: 4)
          Image(systemName: "info.circle")
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .help(focusHelp(focus))
        }
        Text(focus.nextAction)
          .font(.callout.weight(.semibold))
          .fixedSize(horizontal: false, vertical: true)

        // Roadmap invariant 6: recoveries are the headline personal stat,
        // never streaks. Every other tool can say where the time went; only
        // Velvt knows the person came back. It is also a number that cannot be
        // lost — it only ever goes up, so it cannot be used against them.
        // Stated as a fact, not praise: the analyst voice does not congratulate.
        if focus.recoveryCount > 0 {
          Label(
            focus.recoveryCount == 1
              ? "You came back once." : "You came back \(focus.recoveryCount) times.",
            systemImage: "arrow.uturn.backward"
          )
          .font(.callout.weight(.semibold))
          .foregroundStyle(Color.velvtPink)
          .fixedSize(horizontal: false, vertical: true)
        }

        Divider().opacity(0.15)

        // The card is named after the work, not after the metric. The
        // comment above this card used to say "Focus Fragmentation names a
        // metric, not a meaning" — and the view then printed it anyway,
        // twice, as the label over the chart and as the empty-state title.
        // With no intention and no anchor yet there is nothing honest to put
        // here, so the row carries the window alone rather than falling back
        // to the metric name.
        HStack(spacing: 6) {
          if let header, !header.isEmpty {
            Text(header)
              .font(.caption2)
              .foregroundStyle(Color.velvtMuted)
              .lineLimit(1)
              .truncationMode(.tail)
          }
          Spacer()
          Text(focus.windowLabel)
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
        }
        focusTimeline(focus)
        metrics(focus)
      } else {
        VStack(alignment: .leading, spacing: 8) {
          Text("Velvt only watches a block you started on purpose.")
            .font(.headline)
            .fixedSize(horizontal: false, vertical: true)
          Text(errorMessage ?? "Start one and it'll tell you how it went.")
            .font(.caption)
            .foregroundStyle(Color.velvtMuted)
            .fixedSize(horizontal: false, vertical: true)
          Button("Start a work block", action: onStartWorkBlock)
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
        }
      }
    }
    .padding(12)
    .background(Color.velvtPanel)
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .onChange(of: focusedEvidenceID) { _ in updateFocusedDetail() }
    .accessibilityElement(children: .contain)
  }

  private func focusTimeline(_ focus: LocalFocusFragmentation) -> some View {
    GeometryReader { proxy in
      ZStack(alignment: .leading) {
        RoundedRectangle(cornerRadius: 4)
          .fill(Color.white.opacity(0.08))
          .accessibilityHidden(true)
        timelineSegments(focus, width: proxy.size.width)
        transitionMarkers(focus, width: proxy.size.width)
        clusterMarkers(focus, width: proxy.size.width)
      }
    }
    .frame(height: 24)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Attention timeline, \(focus.windowLabel)")
  }

  private func timelineSegments(_ focus: LocalFocusFragmentation, width: CGFloat) -> some View {
    ZStack(alignment: .leading) {
      ForEach(focus.segments) { segment in
        timelineSegment(segment, focus: focus, width: width)
      }
    }
  }

  private func timelineSegment(
    _ segment: LocalTimelineSegment,
    focus: LocalFocusFragmentation,
    width: CGFloat
  ) -> some View {
    let detail = segmentDetail(segment)
    let renderedWidth = max(CGFloat(5), width * segmentWidth(segment, focus: focus))
    return Button {
      hoveredDetail = detail
    } label: {
      ZStack {
        RoundedRectangle(cornerRadius: 3)
          .fill(categoryColor(segment.category))
        if renderedWidth >= 34 {
          Text(shortCategory(segment.category))
            .font(.system(size: 8, weight: .semibold))
            .foregroundStyle(Color.black.opacity(0.72))
            .lineLimit(1)
        }
      }
      .frame(width: renderedWidth, height: 20)
    }
    .buttonStyle(.plain)
    .offset(x: segmentOffset(segment, focus: focus, width: width))
    .help(detail)
    .focused($focusedEvidenceID, equals: segment.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
  }

  private func transitionMarkers(_ focus: LocalFocusFragmentation, width: CGFloat) -> some View {
    ForEach(focus.transitions) { transition in
      transitionMarker(transition, focus: focus, width: width)
    }
  }

  private func transitionMarker(
    _ transition: LocalTransitionMarker,
    focus: LocalFocusFragmentation,
    width: CGFloat
  ) -> some View {
    let detail =
      "Observed category switch from \(friendlyCategory(transition.fromCategory)) to \(friendlyCategory(transition.toCategory)), \(transition.confidence.rawValue) confidence."
    return Button {
      hoveredDetail = detail
    } label: {
      Rectangle()
        .fill(Color.velvtText.opacity(0.78))
        .frame(width: 2, height: 22)
        .padding(.horizontal, 2)
    }
    .buttonStyle(.plain)
    .offset(x: timeOffset(transition.occurredAt, focus: focus, width: width) - 3)
    .help(detail)
    .focused($focusedEvidenceID, equals: transition.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
  }

  private func clusterMarkers(_ focus: LocalFocusFragmentation, width: CGFloat) -> some View {
    ForEach(focus.clusters) { cluster in
      clusterMarker(cluster, focus: focus, width: width)
    }
  }

  private func clusterMarker(
    _ cluster: LocalSwitchingCluster,
    focus: LocalFocusFragmentation,
    width: CGFloat
  ) -> some View {
    let detail =
      "Switching cluster. \(cluster.explanation) Confidence \(cluster.confidence.rawValue)."
    let xOffset = clusterOffset(cluster, focus: focus, width: width) - 6
    return Button {
      hoveredDetail = detail
    } label: {
      Image(systemName: "circle.grid.cross")
        .font(.system(size: 9, weight: .bold))
        .foregroundStyle(Color.velvtText)
        .padding(2)
        .background(Color.black.opacity(0.65), in: Circle())
    }
    .buttonStyle(.plain)
    .offset(x: xOffset)
    .help(detail)
    .focused($focusedEvidenceID, equals: cluster.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
    .accessibilityHint(
      "A cluster means at least three category transitions inside five minutes; it does not imply harm"
    )
  }

  private func metrics(_ focus: LocalFocusFragmentation) -> some View {
    LazyVGrid(
      columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 3),
      alignment: .leading,
      spacing: 4
    ) {
      focusMetric(
        "Planned / elapsed",
        "\(duration(focus.plannedDurationSeconds)) / \(duration(focus.elapsedDurationSeconds))",
        "Planned duration and recorded elapsed duration for this explicit work block.")
      focusMetric(
        "Longest stretch", duration(focus.longestUninterruptedSeconds),
        "Longest uninterrupted classified category stretch in this window.")
      focusMetric(
        "Switches", "\(focus.observedSwitchCount)",
        "Observed movement between classified categories; idle, system, duplicates, and unclassified movement are excluded."
      )
    }
    .help(
      "\(focus.recoveryCount) recoveries · \(focus.clusters.count) switching clusters · \(coverageLabel(focus)) coverage"
    )
  }

  private func focusMetric(_ title: String, _ value: String, _ help: String) -> some View {
    VStack(alignment: .leading, spacing: 1) {
      Text(value).font(.caption.bold().monospacedDigit())
      Text(title).font(.system(size: 9)).foregroundStyle(Color.velvtMuted).lineLimit(1)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .help(help)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(title), \(value)")
    .accessibilityHint(help)
  }

  private func updateFocusedDetail() {
    guard let focus, let id = focusedEvidenceID else { return }
    if let segment = focus.segments.first(where: { $0.id == id }) {
      hoveredDetail = segmentDetail(segment)
    } else if let transition = focus.transitions.first(where: { $0.id == id }) {
      hoveredDetail =
        "Observed category switch from \(friendlyCategory(transition.fromCategory)) to \(friendlyCategory(transition.toCategory)), \(transition.confidence.rawValue) confidence."
    } else if let cluster = focus.clusters.first(where: { $0.id == id }) {
      hoveredDetail =
        "Switching cluster. \(cluster.explanation) Confidence \(cluster.confidence.rawValue)."
    }
  }

  private func focusHelp(_ focus: LocalFocusFragmentation) -> String {
    let comparison = focus.comparison.map { "\($0.label): \($0.explanation)" }
      ?? "Not enough comparable activity."
    return "Most recent explicit work-block window. \(comparison) \(focus.recoveryCount) recoveries, \(focus.clusters.count) clusters, \(coverageLabel(focus)) coverage."
  }

  private func segmentDetail(_ segment: LocalTimelineSegment) -> String {
    "\(friendlyCategory(segment.category)), \(duration(Int(segment.endedAt.timeIntervalSince(segment.startedAt)))), \(segment.confidence.rawValue) confidence."
  }

  private func segmentWidth(_ segment: LocalTimelineSegment, focus: LocalFocusFragmentation)
    -> CGFloat
  {
    let total = max(1, focus.windowEndedAt.timeIntervalSince(focus.windowStartedAt))
    return CGFloat(max(1, segment.endedAt.timeIntervalSince(segment.startedAt)) / total)
  }

  private func segmentOffset(
    _ segment: LocalTimelineSegment, focus: LocalFocusFragmentation, width: CGFloat
  ) -> CGFloat {
    timeOffset(segment.startedAt, focus: focus, width: width)
  }

  private func clusterOffset(
    _ cluster: LocalSwitchingCluster, focus: LocalFocusFragmentation, width: CGFloat
  ) -> CGFloat {
    timeOffset(cluster.startedAt, focus: focus, width: width)
  }

  private func timeOffset(
    _ date: Date, focus: LocalFocusFragmentation, width: CGFloat
  ) -> CGFloat {
    let total = max(1, focus.windowEndedAt.timeIntervalSince(focus.windowStartedAt))
    let ratio = date.timeIntervalSince(focus.windowStartedAt) / total
    return width * CGFloat(min(1, max(0, ratio)))
  }

  private func coverageLabel(_ focus: LocalFocusFragmentation) -> String {
    "\(Int((focus.coverageRatio * 100).rounded()))% \(focus.coverage.rawValue.replacingOccurrences(of: "_", with: " "))"
  }

  private func categoryColor(_ category: String) -> Color {
    switch category {
    case "FOCUS_WORK": return .velvtGreen
    case "COMMUNICATION": return .velvtPink
    case "REFERENCE": return .velvtBlue
    case "CREATIVE": return .orange.opacity(0.85)
    default: return .gray.opacity(0.55)
    }
  }

  private func shortCategory(_ category: String) -> String {
    switch category {
    case "FOCUS_WORK": return "Focus"
    case "COMMUNICATION": return "Comms"
    case "REFERENCE": return "Reference"
    case "CREATIVE": return "Creative"
    default: return "Unclassified"
    }
  }
}

/// The correction workbench: one row per activity the local service has
/// classified, each a duration next to a label you can change.
///
/// This replaces the seven-day stacked activity chart. The chart was the only
/// place a misclassification could be corrected — corrections are load-bearing,
/// they write `personal_app_override`, `personal_override` and
/// `personal_semantic_prototype`, so a correction genuinely changes future
/// classification — but it was also the literal Screen Time artifact: seven
/// dated rows of stacked colour with a percentage-of-your-week column. The
/// affordance is kept and the report around it is not. A percentage of your
/// week is a report; a duration next to a correctable label is a workbench.
///
/// Nothing here is derived in Swift. The rows are the segments the Rust
/// service already sent for one day, rendered in the order it sent them, with
/// the bar widths it supplied. Aggregating durations across the seven days
/// would mean computing a displayed number on this side of the IPC boundary,
/// which is Rust's job; doing it properly needs a payload that does not exist
/// at protocol v28.
public struct LocalActivityCorrectionList: View {
  let snapshot: LocalDashboardSnapshot?
  var onCorrectActivity: (LocalDailyActivitySegment, String, String?) -> Void = { _, _, _ in }
  var onUndoActivity: (LocalDailyActivitySegment) -> Void = { _ in }

  /// Keyed on `stableID`, never on `LocalDailyActivitySegment.id`.
  ///
  /// The service builds that id as `{date}-segment-{index}-{category}`, so it
  /// encodes both the category and the segment's rank by duration. A
  /// correction changes the category, which re-buckets the activity and
  /// re-sorts the day — so the id of the thing the user just corrected does
  /// not exist in the next snapshot. Keyed on the id, the selection silently
  /// evaporated at the exact moment the user acted on it: the row they were
  /// working in disappeared and the evidence line kept asserting the
  /// classification they had just replaced. `stableID` is the abstraction
  /// identity the correction itself is written against, so it survives.
  @State private var selectedStableID: String?
  @State private var isEditing = false
  @FocusState private var focusedRowID: String?

  public init(
    snapshot: LocalDashboardSnapshot?,
    onCorrectActivity: @escaping (LocalDailyActivitySegment, String, String?) -> Void = {
      _, _, _ in
    },
    onUndoActivity: @escaping (LocalDailyActivitySegment) -> Void = { _ in }
  ) {
    self.snapshot = snapshot
    self.onCorrectActivity = onCorrectActivity
    self.onUndoActivity = onUndoActivity
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      if let day = Self.correctableDay(in: snapshot), !day.segments.isEmpty {
        VStack(alignment: .leading, spacing: 3) {
          ForEach(day.segments) { segment in
            activityRow(segment)
          }
        }
        selectionDetail
      } else {
        Text(Self.emptyStateCopy)
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
          .fixedSize(horizontal: false, vertical: true)
          .frame(maxWidth: .infinity, alignment: .leading)
      }
    }
    .onChange(of: focusedRowID) { id in
      guard let id else { return }
      selectedStableID = id
      isEditing = false
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Activities Velvt has categorized on this Mac")
  }

  static let emptyStateCopy =
    "Velvt has not categorized anything on this Mac yet. Once it has, the activities show up here and you can fix any it got wrong."

  /// The most recent day the service sent that actually has activity in it.
  ///
  /// A filter over the delivered payload, not a computation on it. Picking the
  /// most recent non-empty day rather than "today" means the workbench is
  /// never empty at nine in the morning, and it costs nothing: a correction is
  /// written against the activity's `stableID`, not against a date, so fixing
  /// yesterday's label fixes the label everywhere including today.
  static func correctableDay(in snapshot: LocalDashboardSnapshot?) -> LocalDailyActivityDay? {
    snapshot?.dailyActivity.last(where: { !$0.segments.isEmpty })
  }

  /// Resolves the selected row against the current snapshot.
  ///
  /// Called on every render rather than captured, so a correction that changes
  /// the label, the category or the confidence is on screen the moment the
  /// next snapshot lands.
  static func selectedSegment(
    stableID: String?,
    in snapshot: LocalDashboardSnapshot?
  ) -> LocalDailyActivitySegment? {
    guard let stableID else { return nil }
    guard let day = correctableDay(in: snapshot) else { return nil }
    return day.segments.first(where: { $0.stableID == stableID })
  }

  /// The evidence sentence for a row, derived from the segment in hand.
  ///
  /// No percentage: a share of a period is a report about the period. A
  /// duration and a confidence are facts about the thing being corrected.
  static func detail(for segment: LocalDailyActivitySegment) -> String {
    let base =
      "\(segment.label), \(plainDuration(segment.durationSeconds)), \(segment.confidence.rawValue) confidence."
    return [base, segment.explanation].compactMap { $0 }.joined(separator: " ")
  }

  static func plainDuration(_ seconds: Int) -> String {
    let minutes = max(0, seconds) / 60
    if minutes < 60 { return "\(minutes)m" }
    return "\(minutes / 60)h \(minutes % 60)m"
  }

  private var resolvedSegment: LocalDailyActivitySegment? {
    Self.selectedSegment(stableID: selectedStableID, in: snapshot)
  }

  private func activityRow(_ segment: LocalDailyActivitySegment) -> some View {
    let isSelected = segment.stableID != nil && segment.stableID == selectedStableID
    return Button {
      guard let stableID = segment.stableID else { return }
      if selectedStableID == stableID {
        selectedStableID = nil
        isEditing = false
      } else {
        selectedStableID = stableID
        isEditing = false
      }
    } label: {
      HStack(spacing: 8) {
        Text(segment.suggestedName ?? segment.label)
          .font(.caption)
          .lineLimit(1)
          .truncationMode(.tail)
          .frame(width: 120, alignment: .leading)
        GeometryReader { proxy in
          // Width comes from the share the service computed. It is used as a
          // bar length and never printed, so the workbench shows a magnitude
          // without making a claim about a proportion of anyone's week.
          RoundedRectangle(cornerRadius: 3)
            .fill(barColor(segment))
            .frame(
              width: max(4, proxy.size.width * CGFloat(min(100, max(0, segment.percentage))) / 100),
              height: 12
            )
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(height: 12)
        Text(Self.plainDuration(segment.durationSeconds))
          .font(.caption2.monospacedDigit())
          .foregroundStyle(Color.velvtMuted)
          .frame(width: 48, alignment: .trailing)
      }
      .contentShape(Rectangle())
      .padding(.vertical, 4)
      .padding(.horizontal, 6)
      .background(isSelected ? Color.velvtPanelHighlight : Color.clear)
      .clipShape(RoundedRectangle(cornerRadius: 5))
    }
    .buttonStyle(.plain)
    .disabled(segment.stableID == nil)
    .focused($focusedRowID, equals: segment.stableID ?? segment.id)
    .help(Self.detail(for: segment))
    .accessibilityLabel(Self.detail(for: segment))
    .accessibilityAddTraits(isSelected ? .isSelected : [])
    .accessibilityHint("Select to rename or recategorize this activity")
  }

  @ViewBuilder
  private var selectionDetail: some View {
    if let segment = resolvedSegment {
      Divider().opacity(0.18)
      Text(Self.detail(for: segment))
        .font(.caption2)
        .foregroundStyle(Color.velvtMuted)
        .fixedSize(horizontal: false, vertical: true)
        .lineLimit(3)
        .accessibilityLabel(Self.detail(for: segment))
      HStack(spacing: 7) {
        ActivityContextIcon(name: segment.suggestedName ?? segment.label)
        VStack(alignment: .leading, spacing: 1) {
          Text(segment.suggestedName ?? segment.label)
            .font(.caption.bold())
            .lineLimit(1)
          Label(
            segment.suggestedName == nil ? "Local only" : "Local-only suggestion",
            systemImage: "lock.fill"
          )
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
        }
        Spacer(minLength: 4)
        if let suggestion = segment.suggestedName, !segment.aliasConfirmed {
          Button("Use suggestion") {
            onCorrectActivity(segment, Self.correctionCategory(segment.category), suggestion)
          }
          .controlSize(.small)
          .accessibilityHint("Confirms this device-local name for future matching activity")
        }
        Button(segment.label == "Unclassified" ? "Name & categorize" : "Rename / Categorize") {
          isEditing.toggle()
        }
        .controlSize(.small)
        .buttonStyle(.borderedProminent)
        .accessibilityHint("Opens local-only activity naming and category controls")
      }
      if isEditing {
        InlineActivityCorrectionEditor(
          segment: segment,
          onSave: { category, name in
            onCorrectActivity(segment, category, name)
            isEditing = false
          },
          onCancel: { isEditing = false },
          onUndo: segment.aliasConfirmed
            ? {
              onUndoActivity(segment)
              isEditing = false
            }
            : nil
        )
        // Keyed on the abstraction, so the editor is not torn down and rebuilt
        // when a correction changes the segment's category.
        .id(segment.stableID)
      }
    }
  }

  static func correctionCategory(_ value: String) -> String {
    InlineActivityCorrectionEditor.categories.contains(value) ? value : "UNLOGGED"
  }

  private func barColor(_ segment: LocalDailyActivitySegment) -> Color {
    switch segment.category {
    case "UNCLASSIFIED": return Color.gray.opacity(0.55)
    case "OTHER": return Color.velvtMuted.opacity(0.5)
    case "FOCUS_WORK": return .velvtGreen
    case "COMMUNICATION": return .velvtPink
    case "REFERENCE": return .velvtBlue
    case "CREATIVE": return .orange.opacity(0.85)
    default: return .purple.opacity(0.85)
    }
  }
}

private struct ActivityContextIcon: View {
  let name: String

  var body: some View {
    Group {
      if let image = appImage {
        Image(nsImage: image)
          .resizable()
          .scaledToFit()
      } else {
        Image(systemName: "app.dashed")
          .resizable()
          .scaledToFit()
          .padding(3)
          .foregroundStyle(Color.velvtMuted)
      }
    }
    .frame(width: 22, height: 22)
    .accessibilityHidden(true)
  }

  private var appImage: NSImage? {
    guard
      let url = NSWorkspace.shared.runningApplications.first(where: {
        $0.localizedName?.localizedCaseInsensitiveCompare(name) == .orderedSame
      })?.bundleURL
    else { return nil }
    return NSWorkspace.shared.icon(forFile: url.path)
  }
}

struct InlineActivityCorrectionEditor: View {
  static let categories = [
    "FOCUS_WORK", "PASSIVE_CONSUMPTION", "SOCIAL_FEED", "COMMUNICATION",
    "TASK_MANAGEMENT", "REFERENCE", "SYSTEM", "UNLOGGED",
  ]

  let segment: LocalDailyActivitySegment
  let onSave: (String, String?) -> Void
  let onCancel: () -> Void
  let onUndo: (() -> Void)?
  @State private var name: String
  @State private var category: String

  init(
    segment: LocalDailyActivitySegment,
    onSave: @escaping (String, String?) -> Void,
    onCancel: @escaping () -> Void,
    onUndo: (() -> Void)?
  ) {
    self.segment = segment
    self.onSave = onSave
    self.onCancel = onCancel
    self.onUndo = onUndo
    _name = State(initialValue: segment.suggestedName ?? (segment.label == "Unclassified" ? "" : segment.label))
    _category = State(
      initialValue: Self.categories.contains(segment.category) ? segment.category : "UNLOGGED")
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      TextField("Local activity name", text: $name)
        .textFieldStyle(.roundedBorder)
        .onChange(of: name) { value in
          if value.count > 48 { name = String(value.prefix(48)) }
        }
        .accessibilityLabel("Local-only activity name")
        .accessibilityHint("This name stays on this Mac")
      HStack(spacing: 6) {
        Picker("Category", selection: $category) {
          ForEach(Self.categories, id: \.self) { value in
            Text(categoryLabel(value)).tag(value)
          }
        }
        .pickerStyle(.menu)
        .controlSize(.small)
        Button("Save") { onSave(category, normalizedName) }
          .keyboardShortcut(.return, modifiers: .command)
          .disabled(normalizedName == nil)
        Button("Cancel", action: onCancel)
          .keyboardShortcut(.cancelAction)
        if let onUndo {
          Button("Undo saved correction", role: .destructive, action: onUndo)
        }
      }
      Label("Names, suggestions, and icons stay on this Mac.", systemImage: "lock.fill")
        .font(.caption2)
        .foregroundStyle(Color.velvtMuted)
    }
    .padding(8)
    .background(Color.velvtSurface.opacity(0.75))
    .clipShape(RoundedRectangle(cornerRadius: 7))
  }

  private var normalizedName: String? {
    let value = name.trimmingCharacters(in: .whitespacesAndNewlines)
    return value.isEmpty ? nil : value
  }

  private func categoryLabel(_ value: String) -> String {
    value
      .replacingOccurrences(of: "_", with: " ")
      .lowercased()
      .capitalized
  }
}

private func duration(_ seconds: Int) -> String {
  let value = max(0, seconds)
  let hours = value / 3600
  let minutes = (value % 3600) / 60
  if hours > 0 { return "\(hours)h \(minutes)m" }
  if minutes > 0 { return "\(minutes)m" }
  return "\(value)s"
}

private func friendlyCategory(_ category: String) -> String {
  category.replacingOccurrences(of: "_", with: " ").lowercased().capitalized
}

// MARK: - IPCStatusBanner

/// Muted inline indicator for IPC unavailability.
///
/// Rendered at the bottom of the popover without blocking content or requiring
/// dismissal. Never shown as an alert.
struct IPCStatusBanner: View {
    let message: String

    var body: some View {
        Label(message, systemImage: "dot.radiowaves.left.and.right")
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted.opacity(0.55))
    }
}

// MARK: - Preview

#if DEBUG
@MainActor
struct VelvtPopoverContentView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            VelvtPopoverContentView(coordinator: loadingCoordinator)
                .frame(width: 280)
                .previewDisplayName("Loading")
            VelvtPopoverContentView(coordinator: populatedCoordinator)
                .frame(width: 280)
                .previewDisplayName("Populated")
            VelvtPopoverContentView(coordinator: errorCoordinator)
                .frame(width: 280)
                .previewDisplayName("Error")
        }
        .preferredColorScheme(.dark)
    }

    static var loadingCoordinator: ConcreteDisplayDataCoordinator {
        ConcreteDisplayDataCoordinator()
    }

    static var populatedCoordinator: ConcreteDisplayDataCoordinator {
        let c = ConcreteDisplayDataCoordinator()
      c.updateInsight(
        InsightPayload(
            date: "2026-06-15",
          text:
            "Your focus held steady across the morning block, with fewer context switches than the previous week.",
            confidenceLevel: .high,
            lowConfidence: false,
            generatedAt: Date()
        ))
        c.updateHistory(HistoryPayload(days: 7, summaries: HistoryListView_Previews.previewSummaries))
        return c
    }

    static var errorCoordinator: ConcreteDisplayDataCoordinator {
        let c = ConcreteDisplayDataCoordinator()
        // Simulate the error state the coordinator would reach after disconnect.
        return c
    }
}
#endif
