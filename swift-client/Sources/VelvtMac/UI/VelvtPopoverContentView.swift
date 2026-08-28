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
                onSuggestedAction: canStartSuggestedBlock ? startSuggestedWorkBlock : nil
            )
                .padding(.horizontal, 16)
        case .earlyLocal:
            if let signal = readyLocalSignal {
                EarlyLocalSignalView(
                    signal: signal,
                    onSuggestedAction: canStartEarlySignalBlock(signal)
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

    /// Both suggested-action buttons carry a duration authored elsewhere —
    /// the cloud insight payload's `action_minutes`, and the local signal's
    /// — and both fed it straight into a start command. The service accepts
    /// 300...10800 seconds and answers anything else with
    /// `invalid_work_block_request`, which reaches the person as "Unable to
    /// update this local work block. Try again." for a button they were
    /// invited to press.
    ///
    /// The gate withholds the button rather than clamping the number,
    /// because the button states its own duration: clamping four minutes up
    /// to five would start a block the label did not offer.
    private var canStartSuggestedBlock: Bool {
        workBlockCoordinator.snapshot?.phase == .idle
            && WorkBlockDurationLimits.acceptsMinutes(
                coordinator.insightViewModel.suggestedActionMinutes)
    }

    private func canStartEarlySignalBlock(_ signal: LocalEarlySignal) -> Bool {
        workBlockCoordinator.snapshot?.phase == .idle
            && WorkBlockDurationLimits.acceptsMinutes(signal.actionMinutes)
    }

    private func startSuggestedWorkBlock() {
        guard canStartSuggestedBlock else { return }
        workBlockCoordinator.startBlock(
            intention: nil,
            durationSeconds: coordinator.insightViewModel.suggestedActionMinutes * 60,
            purpose: nil,
            intensity: .medium
        )
    }

    private func startEarlySignalWorkBlock(_ signal: LocalEarlySignal) {
        guard canStartEarlySignalBlock(signal) else { return }
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

      TodaySoFarView(day: localDashboardCoordinator.snapshot?.dailyActivity.last)

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

/// Today, as a total.
///
/// Every other local surface answers a different question: the early signal
/// covers the last hour, the fragmentation card covers the current block, and
/// the Patterns chart covers seven days at a glance. None of them answered
/// "what have I done today", which is the question a person actually opens a
/// menu-bar app to ask, and the one the app could already answer — Rust builds
/// today as the last row of `daily_activity` on every snapshot.
///
/// It states observed time and where it went. It does not score the day,
/// compare it to another day, or say whether it was good, because none of those
/// are things this evidence supports.
struct TodaySoFarView: View {
  let day: LocalDailyActivityDay?

  private var slices: [(category: String, seconds: Int)] {
    guard let day else { return [] }
    return LocalWeekActivityView.slices(for: day)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack(alignment: .firstTextBaseline) {
        Label("Today so far", systemImage: "sun.max")
          .font(.caption.bold())
          .foregroundStyle(Color.velvtText)
        Spacer(minLength: 8)
        Text(observedText)
          .font(.caption.monospacedDigit())
          .foregroundStyle(Color.velvtMuted)
      }

      if slices.isEmpty {
        Text("Nothing observed yet today.")
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
      } else {
        VStack(alignment: .leading, spacing: 2) {
          ForEach(slices.prefix(3), id: \.category) { slice in
            HStack(spacing: 6) {
              Text(localCategoryLabel(slice.category))
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .lineLimit(1)
              Spacer(minLength: 8)
              Text(DaySummaryViewModel.formatActiveTime(slice.seconds))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(Color.velvtText)
            }
          }
        }
      }
    }
    .padding(10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(Color.velvtPanel)
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Self.spokenSummary(for: day))
  }

  private var observedText: String {
    guard let day, day.activeSeconds > 0 else { return "—" }
    return "\(DaySummaryViewModel.formatActiveTime(day.activeSeconds)) observed"
  }

  /// One sentence carrying the same three facts the card shows.
  static func spokenSummary(for day: LocalDailyActivityDay?) -> String {
    guard let day, day.activeSeconds > 0 else { return "Today so far, nothing observed yet" }
    let time = DaySummaryViewModel.formatActiveTime(day.activeSeconds)
    guard let top = LocalWeekActivityView.slices(for: day).first else {
      return "Today so far, \(time) observed"
    }
    return "Today so far, \(time) observed, mostly \(localCategoryLabel(top.category))"
  }
}

/// The one-line live control that sits above the evidence card while a block
/// is running. Internal rather than file-private so the env-gated snapshot
/// renderer can put the live row on screen on its own.
struct CompactWorkBlockControl: View {
  let snapshot: WorkBlockSnapshot
  @ObservedObject var coordinator: WorkBlockCoordinator

  var body: some View {
    HStack(spacing: 10) {
      VStack(alignment: .leading, spacing: 2) {
        Text(snapshot.phase == .paused ? "Work block paused" : "Work block active")
          .font(.caption.bold())
        elapsedLine
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
    .accessibilityLabel(
      snapshot.phase == .paused ? "Paused work block" : "Active work block")
  }

  /// The service publishes work-block state on commands and on one deadline
  /// sleep — `run_deadline_scheduler` says so in as many words: "there is no
  /// periodic timer or state polling." So `elapsed_duration_seconds` is true
  /// at the instant it was sent and at no instant after. Open the panel ten
  /// minutes into a block whose start command was the last push and this row
  /// read "0m elapsed of 25m planned".
  ///
  /// The live value is not re-derived here. The service defines
  /// `ends_at = started_at + planned + total_paused` and
  /// `remaining = planned - elapsed`, so `ends_at - planned` is the exact
  /// instant its own elapsed count starts from — including across pauses,
  /// which is why `ends_at` and not `started_at`. The ticking text is the
  /// service's own number, continued.
  @ViewBuilder
  private var elapsedLine: some View {
    if snapshot.phase == .active, let endsAt = snapshot.endsAt {
      HStack(spacing: 0) {
        Text(
          timerInterval: endsAt.addingTimeInterval(
            -TimeInterval(snapshot.plannedDurationSeconds))...Date.distantFuture,
          countsDown: false
        )
        Text(" elapsed of \(duration(snapshot.plannedDurationSeconds)) planned")
      }
      .accessibilityElement(children: .combine)
    } else {
      // Clock, not compact, so a pause cannot change the shape of the number
      // that was ticking a second ago. One rule across the minutes surfaces:
      // while a block is running or paused, a counted time is a clock; once
      // it is over, every number on the result card is a duration. A chosen
      // duration — the plan — is always a duration.
      Text(
        "\(DurationText.clock(snapshot.elapsedDurationSeconds)) elapsed of \(duration(snapshot.plannedDurationSeconds)) planned"
      )
    }
  }
}

/// Where every mark on the work-block evidence timeline goes, for a track of
/// a given width.
///
/// This exists because the timeline had no bounds check and no collision
/// handling, and a real block found both at once. Collection died three
/// minutes into a twenty-five minute block, so the window held five
/// transitions inside its first ~1% and nothing after. Each transition was
/// offset by `timeOffset(...) - 3`, so all five landed within a couple of
/// points of x=0 and stacked; the switching-cluster glyph was offset by
/// `clusterOffset(...) - 6`, so at x≈0 it rendered six points outside the
/// track's leading edge, on top of the pile. The topmost mark won and the
/// evidence read as one smudge.
///
/// Two rules fix that, and they are geometry rules, not judgement calls:
///
/// 1. **A mark is a box, not a point.** Every offset here is the leading edge
///    of a glyph of a known width, clamped into `0 ... width - glyphWidth`,
///    so no mark can render outside the track no matter where in the window
///    its timestamp falls — including exactly at the window start and exactly
///    at the window end.
/// 2. **Marks closer together than they are wide collapse, they do not
///    move.** Of the three ways to handle collision, nudging overlapping
///    ticks apart to a minimum spacing is the one that lies: separating five
///    marks by 7pt on a 300pt bar for a 25-minute window relocates the last
///    of them by roughly two and a half minutes of wall-clock time, which is
///    an invented timestamp on a surface whose entire job is evidence.
///    Dropping the ticks once density passes a threshold and showing only the
///    cluster mark loses the ticks in the common case where transitions are
///    dense but no cluster was reported, and shows nothing at all. Collapsing
///    keeps the mark inside the run's own time extent — a run is closed once
///    it would span more than `minimumTickSpacing` points, so a collapsed
///    mark never claims more of the timeline than one tick's width — and it
///    keeps the count, which the label then states. The only cost is that the
///    reader must hover or focus the mark to learn the count, and that cost
///    is paid in a string, not in a fabricated position.
///
/// Nothing here derives a number for display. It consumes the transitions and
/// clusters Rust already sent and decides only where they can be drawn.
struct TimelineMarkerLayout: Equatable {

  // MARK: Vertical metrics

  /// The bar itself: background, segments, and transition ticks.
  static let trackHeight: CGFloat = 22
  static let segmentHeight: CGFloat = 18
  static var segmentTopInset: CGFloat { (trackHeight - segmentHeight) / 2 }
  static let tickHeight: CGFloat = 22
  /// Clusters get their own lane below the bar so a cluster can never sit on
  /// top of the transitions it is made of.
  static let clusterLaneGap: CGFloat = 2
  static let clusterLaneHeight: CGFloat = 8
  static let clusterRailHeight: CGFloat = 3
  static var clusterLaneTop: CGFloat { trackHeight + clusterLaneGap }
  static var trackHeightWithClusterLane: CGFloat { clusterLaneTop + clusterLaneHeight }

  // MARK: Horizontal metrics

  /// Hit target and clamping box for one tick. The visible bar is narrower
  /// and centred inside it, so a tick clamped hard against either end of the
  /// track is still visibly inside the track rather than flush with its edge.
  static let tickGlyphWidth: CGFloat = 6
  static let singleTickBarWidth: CGFloat = 2
  /// A collapsed run is drawn wider than a single tick so density is legible
  /// without hovering, even though the exact count is not.
  static let collapsedTickBarWidth: CGFloat = 4
  /// Two tick centres closer than this cannot be read as two marks, so the
  /// run collapses into one. It is also the cap on how much of the timeline
  /// a single collapsed mark is allowed to stand for.
  static let minimumTickSpacing: CGFloat = 7
  /// A cluster whose start and end land on the same point still has to be
  /// visible and still has to be clickable.
  static let minimumClusterRailWidth: CGFloat = 8
  /// A short segment is widened to this so it is visible — but only into
  /// space no other segment wants. See `segmentBars`.
  static let minimumSegmentWidth: CGFloat = 5
  /// The floor below which a segment is not drawn at all, because it has no
  /// room left to be drawn in.
  static let hairlineSegmentWidth: CGFloat = 1
  /// The floor below which a stretch with no segment on it is left alone.
  ///
  /// Two adjacent segments always leave a sub-point hole between them, and
  /// hatching every one of those would turn a fully observed bar into a
  /// dotted line. Only a hole wide enough to read as a hole is marked as one.
  static let minimumUnobservedSpanWidth: CGFloat = 6

  struct Tick: Equatable, Identifiable {
    /// The first transition in the run, which is stable for a given width.
    let id: String
    /// Leading edge of a `tickGlyphWidth`-wide glyph, already clamped.
    let offset: CGFloat
    /// Every transition this one mark stands for, in time order. Never empty.
    let transitionIDs: [String]

    var transitionCount: Int { transitionIDs.count }
    var isCollapsed: Bool { transitionIDs.count > 1 }
    var center: CGFloat { offset + TimelineMarkerLayout.tickGlyphWidth / 2 }
  }

  struct ClusterRail: Equatable, Identifiable {
    let id: String
    /// Leading edge, already clamped so `offset + width <= trackWidth`.
    let offset: CGFloat
    let width: CGFloat
  }

  struct SegmentBar: Equatable, Identifiable {
    let id: String
    /// Leading edge, already clamped so `offset + width <= trackWidth`.
    let offset: CGFloat
    let width: CGFloat
  }

  /// A stretch of the track that no segment covers.
  ///
  /// This is the difference between "one category the whole time" and "the
  /// instrument was off", which on a bare track are the same pixels. At full
  /// coverage there are none of these; at partial coverage they are most of
  /// the bar, and drawing them is what keeps the axis from claiming the
  /// unobserved part was quiet.
  struct UnobservedSpan: Equatable, Identifiable {
    let id: String
    let offset: CGFloat
    let width: CGFloat
  }

  let ticks: [Tick]
  let clusterRails: [ClusterRail]
  let segmentBars: [SegmentBar]
  let unobservedSpans: [UnobservedSpan]

  static let empty = TimelineMarkerLayout(
    ticks: [], clusterRails: [], segmentBars: [], unobservedSpans: [])

  static func make(focus: LocalFocusFragmentation, width: CGFloat) -> TimelineMarkerLayout {
    make(
      transitions: focus.transitions,
      clusters: focus.clusters,
      segments: focus.segments,
      windowStartedAt: focus.windowStartedAt,
      windowEndedAt: focus.windowEndedAt,
      width: width
    )
  }

  static func make(
    transitions: [LocalTransitionMarker],
    clusters: [LocalSwitchingCluster],
    segments: [LocalTimelineSegment] = [],
    windowStartedAt: Date,
    windowEndedAt: Date,
    width: CGFloat
  ) -> TimelineMarkerLayout {
    guard width > 0 else { return .empty }

    // Runs are closed on extent, not on gap-to-previous. Chaining on the gap
    // would let a long, evenly-dense sequence collapse into one mark spanning
    // most of the bar, which is a worse lie than the overlap it fixed.
    var runs: [(first: CGFloat, last: CGFloat, ids: [String])] = []
    for transition in transitions.sorted(by: { $0.occurredAt < $1.occurredAt }) {
      let center = position(
        transition.occurredAt,
        windowStartedAt: windowStartedAt,
        windowEndedAt: windowEndedAt,
        width: width
      )
      if var run = runs.last, center - run.first < minimumTickSpacing {
        run.last = center
        run.ids.append(transition.id)
        runs[runs.count - 1] = run
      } else {
        runs.append((first: center, last: center, ids: [transition.id]))
      }
    }

    var ticks: [Tick] = []
    var previousCenter: CGFloat?
    for run in runs {
      // Sit in the middle of the run's own extent, then hold the minimum
      // spacing against the previous mark. Because a run spans less than
      // `minimumTickSpacing`, this pass can move a mark by at most half that
      // — under four points, a few seconds of a 25-minute window.
      var center = (run.first + run.last) / 2
      if let previous = previousCenter { center = max(center, previous + minimumTickSpacing) }
      previousCenter = center
      ticks.append(
        Tick(
          id: run.ids[0],
          offset: clampedCenter(center, glyphWidth: tickGlyphWidth, trackWidth: width),
          transitionIDs: run.ids
        ))
    }

    let rails = clusters.map { cluster -> ClusterRail in
      let start = position(
        cluster.startedAt,
        windowStartedAt: windowStartedAt,
        windowEndedAt: windowEndedAt,
        width: width
      )
      let end = position(
        cluster.endedAt,
        windowStartedAt: windowStartedAt,
        windowEndedAt: windowEndedAt,
        width: width
      )
      let railWidth = min(width, max(minimumClusterRailWidth, end - start))
      return ClusterRail(
        id: cluster.id,
        offset: clampedLeading(start, glyphWidth: railWidth, trackWidth: width),
        width: railWidth
      )
    }

    return TimelineMarkerLayout(
      ticks: ticks,
      clusterRails: rails,
      segmentBars: segmentBars(
        segments,
        windowStartedAt: windowStartedAt,
        windowEndedAt: windowEndedAt,
        width: width
      ),
      unobservedSpans: unobservedSpans(
        segments,
        windowStartedAt: windowStartedAt,
        windowEndedAt: windowEndedAt,
        width: width
      )
    )
  }

  /// The complement of the segments: every stretch of the window the service
  /// sent nothing for, in track coordinates.
  ///
  /// Derives no number for display. It walks the segments the service already
  /// sent, in time order, and reports the holes — the same thing `segmentBars`
  /// does with the segments themselves.
  static func unobservedSpans(
    _ segments: [LocalTimelineSegment],
    windowStartedAt: Date,
    windowEndedAt: Date,
    width: CGFloat
  ) -> [UnobservedSpan] {
    guard width > 0 else { return [] }
    var spans: [UnobservedSpan] = []
    var cursor: CGFloat = 0
    func close(_ upTo: CGFloat) {
      guard upTo - cursor >= minimumUnobservedSpanWidth else { return }
      spans.append(
        UnobservedSpan(id: "unobserved-\(spans.count)", offset: cursor, width: upTo - cursor))
    }
    for segment in segments.sorted(by: { $0.startedAt < $1.startedAt }) {
      let start = position(
        segment.startedAt, windowStartedAt: windowStartedAt, windowEndedAt: windowEndedAt,
        width: width)
      let end = position(
        segment.endedAt, windowStartedAt: windowStartedAt, windowEndedAt: windowEndedAt,
        width: width)
      close(start)
      cursor = max(cursor, end)
    }
    close(width)
    return spans
  }

  /// A segment is only widened into space the next segment does not want.
  ///
  /// The old rule was `max(5, width * proportion)` with each bar positioned
  /// independently at its own start, which is fine while segments are long
  /// and silently destructive once they are not: on a dead-collection block
  /// whose longest meaningful stretch was seventeen seconds, a dozen
  /// sub-five-point segments each inflated to five points and drew over
  /// their neighbours, so what looked like four blocks of colour was twelve
  /// segments with eight of them buried. Now the floor applies only where
  /// there is room for it, and where there is not, each bar gets exactly the
  /// space between its own start and the next one's. The transition ticks
  /// carry "a switch happened here" in the dense case, which is what the
  /// five-point floor was standing in for.
  static func segmentBars(
    _ segments: [LocalTimelineSegment],
    windowStartedAt: Date,
    windowEndedAt: Date,
    width: CGFloat
  ) -> [SegmentBar] {
    guard width > 0 else { return [] }
    let ordered = segments.sorted { $0.startedAt < $1.startedAt }
    let starts = ordered.map {
      position($0.startedAt, windowStartedAt: windowStartedAt, windowEndedAt: windowEndedAt,
        width: width)
    }
    return ordered.enumerated().compactMap { index, segment -> SegmentBar? in
      let start = starts[index]
      let end = position(
        segment.endedAt, windowStartedAt: windowStartedAt, windowEndedAt: windowEndedAt,
        width: width)
      let available = max(0, (index + 1 < starts.count ? starts[index + 1] : width) - start)
      guard available >= hairlineSegmentWidth else { return nil }
      let barWidth = min(max(minimumSegmentWidth, end - start), available)
      return SegmentBar(
        id: segment.id,
        offset: clampedLeading(start, glyphWidth: barWidth, trackWidth: width),
        width: barWidth
      )
    }
  }

  /// Centre point on the track for an instant in the window, clamped to the
  /// window so an out-of-window timestamp cannot escape the track.
  static func position(
    _ date: Date, windowStartedAt: Date, windowEndedAt: Date, width: CGFloat
  ) -> CGFloat {
    let total = max(1, windowEndedAt.timeIntervalSince(windowStartedAt))
    let ratio = date.timeIntervalSince(windowStartedAt) / total
    return width * CGFloat(min(1, max(0, ratio)))
  }

  /// Leading edge for a glyph of `glyphWidth` centred on `center`, clamped so
  /// the whole glyph is inside `0 ... trackWidth`.
  static func clampedCenter(
    _ center: CGFloat, glyphWidth: CGFloat, trackWidth: CGFloat
  ) -> CGFloat {
    clampedLeading(center - glyphWidth / 2, glyphWidth: glyphWidth, trackWidth: trackWidth)
  }

  static func clampedLeading(
    _ leading: CGFloat, glyphWidth: CGFloat, trackWidth: CGFloat
  ) -> CGFloat {
    min(max(0, trackWidth - glyphWidth), max(0, leading))
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
  /// The last laid-out track width, kept so keyboard focus can resolve which
  /// mark an id belongs to. A collapsed tick's id is its run's first
  /// transition, and which transitions share a run depends on the width, so
  /// `updateFocusedDetail` cannot answer "how many switches is this mark?"
  /// without it — and answering "one" would be the exact lie the collapse
  /// exists to avoid.
  @State private var trackWidth: CGFloat = 0
  @FocusState private var focusedEvidenceID: String?

  public var body: some View {
    VStack(alignment: .leading, spacing: 7) {
      if let focus {
        // Coverage is a layout variable here, not a sentence appended to one.
        // The card renders what it knows and stops; what it does not know is
        // not drawn faintly, it is not drawn.
        let state = FocusEvidenceState.resolve(
          coverage: focus.coverage, coverageRatio: focus.coverageRatio)

        // The hero line, in every state. When coverage is thin this is the
        // service's own low-coverage sentence, so the top of the card says
        // the same thing whether or not there is a chart under it.
        //
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

        // Directly under the hero when there is no evidence section, because
        // there it is the reason there is no evidence section. In the drawable
        // state it moves down to sit over the numbers it qualifies.
        if !state.showsObservedMetrics, let notice = coverageNotice(focus, state: state) {
          Text(notice)
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel(notice)
        }

        // Roadmap invariant 6: recoveries are the headline personal stat,
        // never streaks. Every other tool can say where the time went; only
        // Velvt knows the person came back. It is also a number that cannot be
        // lost — it only ever goes up, so it cannot be used against them.
        // Stated as a fact, not praise: the analyst voice does not congratulate.
        //
        // Gated with the rest of the observed numbers. "You came back once"
        // is counted over the observed part like everything else, and under a
        // sentence that has just said there is not enough here to say
        // anything it is not a headline stat, it is the contradiction. The
        // count only ever goes up, so withholding it costs nothing: it is
        // there the moment there is enough of the block behind it.
        if state.showsObservedMetrics, focus.recoveryCount > 0 {
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
        if state.showsTimeline {
          focusTimeline(focus)
        }
        metrics(focus, state: state)
        nextActionRow(focus)
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
    let hasClusters = !focus.clusters.isEmpty
    return GeometryReader { proxy in
      let layout = TimelineMarkerLayout.make(focus: focus, width: proxy.size.width)
      ZStack(alignment: .topLeading) {
        RoundedRectangle(cornerRadius: 4)
          .fill(Color.white.opacity(0.08))
          .frame(height: TimelineMarkerLayout.trackHeight)
          .accessibilityHidden(true)
        unobservedSpans(layout)
        timelineSegments(layout, focus: focus)
        transitionTicks(layout, focus: focus)
        clusterRails(layout, focus: focus)
      }
      .onAppear { trackWidth = proxy.size.width }
      .onChange(of: proxy.size.width) { newWidth in trackWidth = newWidth }
    }
    .frame(
      height: hasClusters
        ? TimelineMarkerLayout.trackHeightWithClusterLane
        : TimelineMarkerLayout.trackHeight
    )
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Attention timeline, \(focus.windowLabel)")
    // The dashed stretches are hidden from the accessibility tree
    // individually — a reader does not need eleven "not observed" nodes to
    // learn one fact about the window — so the encoding is named once here.
    .accessibilityHint("Dashed stretches are time no activity was observed in")
  }

  /// The stretches with no segment on them, drawn as unobserved rather than
  /// left as bare track.
  ///
  /// On a full-coverage bar an empty stretch means one category held the
  /// whole time. On a partial one it means the instrument was not looking.
  /// Those are opposite facts and they were the same pixels. Hatching them
  /// is the alternative to the other repair available here — rescaling the
  /// axis onto the observed extent — which was rejected for three reasons:
  /// horizontal position on this bar means elapsed time into the block, and
  /// re-basing changes that meaning silently; partial coverage is usually
  /// interior holes, so re-basing moves the emptiness inward instead of
  /// removing it; and saying what the new axis covered would mean deriving
  /// and printing a duration in Swift, which is the service's job. Marking
  /// the fiction is cheaper and more honest than shrinking it.
  private func unobservedSpans(_ layout: TimelineMarkerLayout) -> some View {
    ForEach(layout.unobservedSpans) { span in
      ZStack {
        RoundedRectangle(cornerRadius: 3)
          .fill(Color.black.opacity(0.22))
        Path { path in
          let midpoint = TimelineMarkerLayout.segmentHeight / 2
          path.move(to: CGPoint(x: 2, y: midpoint))
          path.addLine(to: CGPoint(x: span.width - 2, y: midpoint))
        }
        .stroke(
          Color.velvtMuted.opacity(0.7),
          style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
      }
      .frame(width: span.width, height: TimelineMarkerLayout.segmentHeight)
      .offset(x: span.offset, y: TimelineMarkerLayout.segmentTopInset)
      .help("No activity was observed in this part of the window.")
      .accessibilityHidden(true)
    }
  }

  private func timelineSegments(
    _ layout: TimelineMarkerLayout, focus: LocalFocusFragmentation
  ) -> some View {
    ZStack(alignment: .topLeading) {
      ForEach(layout.segmentBars) { bar in
        timelineSegment(bar, focus: focus)
      }
    }
  }

  private func timelineSegment(
    _ bar: TimelineMarkerLayout.SegmentBar,
    focus: LocalFocusFragmentation
  ) -> some View {
    let segment = focus.segments.first { $0.id == bar.id }
    let detail = segment.map(segmentDetail) ?? ""
    return Button {
      hoveredDetail = detail
    } label: {
      ZStack {
        RoundedRectangle(cornerRadius: 3)
          .fill(categoryColor(segment?.category ?? ""))
        if bar.width >= 34, let segment {
          Text(shortCategory(segment.category))
            .font(.system(size: 8, weight: .semibold))
            .foregroundStyle(Color.black.opacity(0.72))
            .lineLimit(1)
        }
      }
      .frame(width: bar.width, height: TimelineMarkerLayout.segmentHeight)
    }
    .buttonStyle(.plain)
    .offset(x: bar.offset, y: TimelineMarkerLayout.segmentTopInset)
    .help(detail)
    .focused($focusedEvidenceID, equals: bar.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
  }

  private func transitionTicks(
    _ layout: TimelineMarkerLayout, focus: LocalFocusFragmentation
  ) -> some View {
    ForEach(layout.ticks) { tick in
      transitionTick(tick, focus: focus)
    }
  }

  /// One tick stands for one *or more* transitions. `TimelineMarkerLayout`
  /// decides which, and the label says how many — a collapsed mark that
  /// claimed to be a single switch would be the same lie the stacked markers
  /// told visually.
  private func transitionTick(
    _ tick: TimelineMarkerLayout.Tick,
    focus: LocalFocusFragmentation
  ) -> some View {
    let detail = tickDetail(tick, focus: focus)
    return Button {
      hoveredDetail = detail
    } label: {
      ZStack {
        Color.clear
        RoundedRectangle(cornerRadius: tick.isCollapsed ? 1.5 : 0.5)
          .fill(Color.velvtText.opacity(tick.isCollapsed ? 0.95 : 0.78))
          .frame(
            width: tick.isCollapsed
              ? TimelineMarkerLayout.collapsedTickBarWidth
              : TimelineMarkerLayout.singleTickBarWidth,
            height: TimelineMarkerLayout.tickHeight
          )
      }
      .frame(
        width: TimelineMarkerLayout.tickGlyphWidth,
        height: TimelineMarkerLayout.tickHeight
      )
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .offset(x: tick.offset)
    .help(detail)
    .focused($focusedEvidenceID, equals: tick.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
  }

  private func clusterRails(
    _ layout: TimelineMarkerLayout, focus: LocalFocusFragmentation
  ) -> some View {
    ForEach(layout.clusterRails) { rail in
      clusterRail(rail, focus: focus)
    }
  }

  /// A cluster is a span with a start and an end, so it is drawn as one: a
  /// thin rail in its own lane under the bar, covering the time it actually
  /// covers.
  ///
  /// It used to be a 9pt bold `circle.grid.cross` on a filled circle sitting
  /// on the bar itself — a glyph heavier than the segment bar it annotated,
  /// pinned to the cluster's start instant as though a cluster happened at a
  /// moment, and offset by a hard-coded −6 that pushed it outside the track's
  /// leading edge whenever the cluster began near the top of the window. The
  /// rail is subordinate to the ticks by construction: it is 3pt tall, it is
  /// not on the bar, and it cannot cover a tick.
  private func clusterRail(
    _ rail: TimelineMarkerLayout.ClusterRail,
    focus: LocalFocusFragmentation
  ) -> some View {
    let cluster = focus.clusters.first { $0.id == rail.id }
    let detail = cluster.map(clusterDetail) ?? ""
    return Button {
      hoveredDetail = detail
    } label: {
      ZStack {
        Color.clear
        Capsule()
          .fill(Color.velvtPink.opacity(0.6))
          .frame(width: rail.width, height: TimelineMarkerLayout.clusterRailHeight)
      }
      .frame(width: rail.width, height: TimelineMarkerLayout.clusterLaneHeight)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .offset(x: rail.offset, y: TimelineMarkerLayout.clusterLaneTop)
    .help(detail)
    .focused($focusedEvidenceID, equals: rail.id)
    .onHover { hoveredDetail = $0 ? detail : nil }
    .accessibilityLabel(detail)
    .accessibilityHint(
      "A cluster means at least three category transitions inside five minutes; it does not imply harm"
    )
  }

  /// The numbers, and only the ones the coverage behind them supports.
  ///
  /// Planned and elapsed are on the card in every state: neither is measured
  /// by the classifier. The user chose the plan and the service timed the
  /// block, so a collection outage cannot make either of them wrong. Longest
  /// stretch and switches are the opposite — they exist only inside the
  /// observed fraction — so they appear only when that fraction is large
  /// enough to be worth reading, and when it is not, the row is one metric
  /// wide rather than three metrics wide with two of them describing an
  /// outage.
  private func metrics(_ focus: LocalFocusFragmentation, state: FocusEvidenceState) -> some View {
    VStack(alignment: .leading, spacing: 5) {
      // The qualifier goes above the numbers, not in a tooltip under them.
      // A block whose collection died three minutes into twenty-five reports
      // a 17-second longest stretch, and that number is only readable next
      // to how much of the window was actually observed — which Rust already
      // sends on this DTO as `coverage` and `coverage_ratio`, and which this
      // card used to spend only inside a `.help(...)` nobody opens.
      if state.showsObservedMetrics, let notice = coverageNotice(focus, state: state) {
        Text(notice)
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityLabel(notice)
      }
      LazyVGrid(
        columns: Array(
          repeating: GridItem(.flexible(), spacing: 8),
          count: state.showsObservedMetrics ? 3 : 1),
        alignment: .leading,
        spacing: 4
      ) {
        focusMetric(
          "Planned / elapsed",
          "\(duration(focus.plannedDurationSeconds)) / \(duration(focus.elapsedDurationSeconds))",
          "Planned duration and recorded elapsed duration for this explicit work block.")
        if state.showsObservedMetrics {
          focusMetric(
            "Longest stretch", duration(focus.longestUninterruptedSeconds),
            metricEvidenceHelp(
              "Longest uninterrupted classified category stretch in this window.", focus: focus))
          focusMetric(
            "Switches", "\(focus.observedSwitchCount)",
            metricEvidenceHelp(
              "Observed movement between classified categories; idle, system, duplicates, and unclassified movement are excluded.",
              focus: focus)
          )
        }
      }
      // A legend for an encoding that is on screen. When the timeline is
      // withheld the underlines are withheld with it, and a line naming them
      // would be a key to a chart that is not there.
      if state.showsTimeline, !focus.clusters.isEmpty {
        Text(
          focus.clusters.count == 1
            ? "Underline: switching cluster." : "Underlines: switching clusters."
        )
        .font(.caption2)
        .foregroundStyle(Color.velvtMuted)
      }
    }
    .help(
      "\(focus.recoveryCount) recoveries · \(focus.clusters.count) switching clusters · \(coverageLabel(focus)) coverage"
    )
  }

  /// The service's next-action label, rendered as whatever it currently is.
  /// See `FocusNextActionRole` for why it is not always bold body text.
  @ViewBuilder
  private func nextActionRow(_ focus: LocalFocusFragmentation) -> some View {
    switch FocusNextActionRole.resolve(phase: focus.phase) {
    case .underway:
      Text(focus.nextAction)
        .font(.caption)
        .foregroundStyle(Color.velvtMuted)
        .fixedSize(horizontal: false, vertical: true)
    case .offer:
      Button(focus.nextAction, action: onStartWorkBlock)
        .buttonStyle(.bordered)
        .controlSize(.small)
        .padding(.top, 1)
        .accessibilityHint("Opens the focus session planner on this Mac")
    }
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

  /// States the coverage Rust already reported. It reports a fraction and
  /// stops — the reading of that fraction is the person's, and a card that
  /// warned them about their own block would be inventing a finding out of a
  /// collection outage.
  private func coverageNotice(
    _ focus: LocalFocusFragmentation, state: FocusEvidenceState
  ) -> String? {
    CoverageNotice.sentence(
      isGood: focus.coverage == .good,
      isEmpty: state == .noObservation,
      coverageRatio: focus.coverageRatio,
      switchLabel: state.showsObservedMetrics ? "switches" : nil)
  }

  private func metricEvidenceHelp(_ base: String, focus: LocalFocusFragmentation) -> String {
    guard focus.coverage != .good else { return base }
    return "\(base) Measured over the \(coveragePercent(focus))% of this window with observed activity."
  }

  private func coveragePercent(_ focus: LocalFocusFragmentation) -> Int {
    Int((focus.coverageRatio * 100).rounded())
  }

  private func updateFocusedDetail() {
    guard let focus, let id = focusedEvidenceID else { return }
    if let segment = focus.segments.first(where: { $0.id == id }) {
      hoveredDetail = segmentDetail(segment)
    } else if let tick = TimelineMarkerLayout.make(focus: focus, width: trackWidth)
      .ticks.first(where: { $0.id == id })
    {
      hoveredDetail = tickDetail(tick, focus: focus)
    } else if let transition = focus.transitions.first(where: { $0.id == id }) {
      hoveredDetail = transitionDetail(transition)
    } else if let cluster = focus.clusters.first(where: { $0.id == id }) {
      hoveredDetail = clusterDetail(cluster)
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

  private func transitionDetail(_ transition: LocalTransitionMarker) -> String {
    transitionEvidenceLabel(transition)
  }

  private func clusterDetail(_ cluster: LocalSwitchingCluster) -> String {
    clusterEvidenceLabel(cluster)
  }

  private func tickDetail(
    _ tick: TimelineMarkerLayout.Tick, focus: LocalFocusFragmentation
  ) -> String {
    tickEvidenceLabel(
      tick, transitions: focus.transitions, windowStartedAt: focus.windowStartedAt)
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

  /// What this row calls the activity.
  ///
  /// The classified label, not `suggestedName`. `suggestedName` is the raw
  /// macOS application name, offered in the detail pane as something the user
  /// may adopt — the "Use suggestion" button exists precisely because it has
  /// not been adopted yet, and the detail pane labels it "Local-only
  /// suggestion" rather than a name. Preferring it here overwrote the point of
  /// the classifier: on a real machine 5,285 events the service had resolved to
  /// Coding, YouTube, Gmail and GitHub all rendered as "Google Chrome", which is
  /// both wrong and the single label this product exists not to show. Once the
  /// user confirms the alias it becomes the name, and then it is shown.
  static func rowLabel(for segment: LocalDailyActivitySegment) -> String {
    guard segment.aliasConfirmed, let confirmed = segment.suggestedName else {
      return segment.label
    }
    return confirmed
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
        // The classified label, not the suggestion. `suggestedName` is the raw
        // macOS application name, offered in the detail pane as something the
        // user may adopt — the "Use suggestion" button exists precisely because
        // it has not been adopted yet. Preferring it here overwrote the whole
        // point of the classifier: 5,285 events that the service had resolved
        // to Coding, YouTube, Gmail and GitHub all rendered as "Google Chrome",
        // which is the one label the product exists not to show.
        Text(Self.rowLabel(for: segment))
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

/// The one duration vocabulary for the work-block minutes surfaces.
///
/// There used to be four local rules and they disagreed with each other. A
/// 17-second longest stretch was published as `17s` on the dashboard card and
/// as `0:17` on the result card. A three-hour block's remaining time read
/// `179:00` while paused and `2:59:00` a second later while running. And the
/// largest-unit-only rule silently rounded a 1m59s longest stretch down to
/// `1m` — a 41% understatement of the single number that card exists to
/// report, on a card whose whole job is evidence.
///
/// Two shapes, because there are two jobs:
///
/// - `compact` is a measured duration standing next to other measured
///   durations. The two most significant non-zero units, largest first, with
///   a trailing zero unit dropped: `0s`, `17s`, `1m 30s`, `25m`, `59m 59s`,
///   `1h`, `2h 59m`, `3h`. One rule end to end, so `17s` next to `25m` is the
///   same rule reading a smaller number rather than a different formatter.
/// - `clock` is a countdown or a frozen countdown, and matches exactly what
///   SwiftUI's own `Text(timerInterval:)` draws beside it, so pausing a block
///   cannot change the shape of the number.
enum DurationText {
  static func compact(_ seconds: Int) -> String {
    let value = max(0, seconds)
    let hours = value / 3600
    let minutes = (value % 3600) / 60
    let remainder = value % 60
    if hours > 0 { return minutes > 0 ? "\(hours)h \(minutes)m" : "\(hours)h" }
    if minutes > 0 { return remainder > 0 ? "\(minutes)m \(remainder)s" : "\(minutes)m" }
    return "\(value)s"
  }

  static func clock(_ seconds: Int) -> String {
    let value = max(0, seconds)
    let hours = value / 3600
    let minutes = (value % 3600) / 60
    let remainder = value % 60
    if hours > 0 { return String(format: "%d:%02d:%02d", hours, minutes, remainder) }
    return String(format: "%d:%02d", minutes, remainder)
  }
}

/// The one sentence that states how much of a work-block window was actually
/// observed.
///
/// It lives here rather than inside either card because two cards show
/// elapsed — the local dashboard's work-block card and the end-of-block
/// result card — and a sentence written out twice is a sentence that drifts.
/// It states the fraction the service already reported and stops: reading it
/// is the person's job, and a card that warned them about their own block
/// would be manufacturing a finding out of a collection outage.
///
/// `switchLabel` is the name the calling card gives its own switch metric, so
/// the sentence points at a label the reader can see.
enum CoverageNotice {
  /// `switchLabel` is `nil` on a card that is not showing those numbers at
  /// all. The second clause is a pointer at two labels on screen; on a card
  /// that withheld them it would point at nothing, so the sentence stops
  /// after the fraction — the same claim, minus a reference that no longer
  /// resolves.
  static func sentence(
    isGood: Bool,
    isEmpty: Bool,
    coverageRatio: Double,
    switchLabel: String?
  ) -> String? {
    if isGood { return nil }
    if isEmpty { return "No activity was observed inside this window." }
    let percent = Int((coverageRatio * 100).rounded())
    let covered = "Observed activity covers \(percent)% of this window."
    guard let switchLabel else { return covered }
    return "\(covered) Longest stretch and \(switchLabel) count only that part."
  }
}

/// The one place a coverage number becomes a layout decision.
///
/// Rust owns the verdict about *claims*: it sends `coverage` and
/// `coverage_ratio`, and `dashboard.rs` already refuses to say anything about
/// a block it could not see. Swift owns one question Rust cannot answer,
/// because it is a question about a chart and Rust has never seen the card:
/// whether there is enough observed material for a drawing of it to carry
/// anything a sentence does not.
enum FocusCardCoverage {
  /// At or below this fraction the window holds no observed activity at all.
  ///
  /// The one coverage number Swift compares against anything. The service
  /// reports an empty window as `.noData`, and this is checked beside that
  /// label so a zero-ratio window arriving as `partial` is still treated as
  /// empty rather than drawn as a chart of nothing.
  static let empty: Double = 0

  // There is deliberately no second threshold beside it.
  //
  // The obvious design is a Swift-side "enough to draw" line — a quarter of
  // the window, say — so a 30%-covered block still gets a timeline. It was
  // built, rendered, and thrown away, because of what the render showed: at
  // 30% the service still sends `LOW_COVERAGE_BLOCK_COPY` as the observation,
  // so the card read "Velvt hasn't seen enough of this block yet to say
  // anything" above a chart, a recovery count and two metrics. That is the
  // reported bug at a larger number.
  //
  // The sufficiency line lives in one place, `SUFFICIENT_COVERAGE_RATIO` in
  // `rust-service/src/dashboard.rs`, and it already decides whether the
  // service is willing to speak about a block: at `.good` it sends a real
  // observation, otherwise it sends the low-coverage sentence. So the card
  // draws its evidence exactly when the service was willing to speak, and a
  // threshold on this side could only ever disagree with that one.
}

/// What a work-block card is entitled to put on screen, given how much of its
/// own window was actually observed.
///
/// `rust-service/src/work_block/mod.rs` states the rule this enum exists to
/// carry out: a hedge bolted onto a claim retracts the claim. That rule was
/// applied to sentences and never to charts. A block whose collection died a
/// minute into twenty-five printed "Velvt hasn't seen enough of this block yet
/// to say anything" and then, underneath it, a bold imperative, a recovery
/// count, a full-width timeline that was 96% empty, a longest stretch of 17
/// seconds beside a planned 25 minutes, a switch count and a legend — seven
/// claims under a retraction, every one of them computed over the 4% that
/// existed. Coverage decides which of those exist. It does not decide whether
/// a sentence is appended to them.
enum FocusEvidenceState: Equatable {
  /// Nothing was observed. There is no evidence, so there is no evidence
  /// section: the window it covers, the duration that was planned, and the
  /// way out.
  case noObservation
  /// Something was observed, but too little of the window to draw. Same
  /// shape as `.noObservation`, with the fraction stated instead of the
  /// absence.
  case tooLittleToDraw
  /// The service was willing to speak about this block, so the card is
  /// willing to draw it. The timeline still marks the part of the window it
  /// did not see rather than leaving it as bare track — `.good` is three
  /// quarters, not all of it.
  case drawable

  static func resolve(
    coverage: LocalDashboardCoverage,
    coverageRatio: Double
  ) -> FocusEvidenceState {
    if coverage == .noData || coverageRatio <= FocusCardCoverage.empty { return .noObservation }
    if coverage == .good { return .drawable }
    return .tooLittleToDraw
  }

  /// The timeline, and with it the cluster lane and the legend that explains
  /// the lane.
  var showsTimeline: Bool { self == .drawable }

  /// Every number measured over the observed part only — longest stretch,
  /// switch count, recoveries. They are on the card exactly when the observed
  /// part is large enough to stand behind them. A 17-second longest stretch
  /// inside a 25-minute block is a statement about missing data; printing it
  /// in the same row as "25m / 25m" makes it a statement about the person.
  var showsObservedMetrics: Bool { self == .drawable }
}

/// What the service's `next_action` label is, on this card, right now.
///
/// `LocalFocusFragmentation` carries the label and nothing else: the action
/// id and the duration stay on `WorkBlockResult`, where the end-of-block card
/// turns the same label into a button. Here it arrived alone and was rendered
/// as bold body text — an imperative with no control beside it, under a
/// sentence saying the card had nothing to say. Either it gets its action back
/// or it stops shouting, and which of those depends only on whether the block
/// it is talking about is still running.
enum FocusNextActionRole: Equatable {
  /// The block is over. The label is an offer, so it goes in the control that
  /// can accept it.
  case offer
  /// The block is running or paused. The offer is already being taken, and
  /// the controls for that block are in the live row above this card, so the
  /// label is a standing instruction stated quietly.
  case underway

  static func resolve(phase: WorkBlockPhase) -> FocusNextActionRole {
    phase == .active || phase == .paused ? .underway : .offer
  }
}

private func duration(_ seconds: Int) -> String { DurationText.compact(seconds) }

private func friendlyCategory(_ category: String) -> String {
  category.replacingOccurrences(of: "_", with: " ").lowercased().capitalized
}

// MARK: - Evidence-marker labels

/// The spoken and hovered text for one transition tick, one collapsed run of
/// them, or one switching cluster.
///
/// These live at file scope rather than inside `FocusFragmentationView`
/// because the count in a collapsed label is the load-bearing part and has to
/// be assertable in a test: a mark standing for five switches that announces
/// itself as one switch is the same defect as five marks stacked on one pixel,
/// moved from the pixels into the accessibility tree.

func transitionEvidenceLabel(_ transition: LocalTransitionMarker) -> String {
  "Observed category switch from \(friendlyCategory(transition.fromCategory)) to \(friendlyCategory(transition.toCategory)), \(transition.confidence.rawValue) confidence."
}

func clusterEvidenceLabel(_ cluster: LocalSwitchingCluster) -> String {
  "Switching cluster, \(cluster.transitionCount) transitions. \(cluster.explanation) Confidence \(cluster.confidence.rawValue)."
}

func tickEvidenceLabel(
  _ tick: TimelineMarkerLayout.Tick,
  transitions: [LocalTransitionMarker],
  windowStartedAt: Date
) -> String {
  let members = tick.transitionIDs.compactMap { id in
    transitions.first { $0.id == id }
  }
  guard let first = members.first, let last = members.last else { return "" }
  guard members.count > 1 else { return transitionEvidenceLabel(first) }
  let firstElapsed = duration(Int(first.occurredAt.timeIntervalSince(windowStartedAt).rounded()))
  let lastElapsed = duration(Int(last.occurredAt.timeIntervalSince(windowStartedAt).rounded()))
  return
    "\(members.count) observed category switches, too close together to draw apart: from \(friendlyCategory(first.fromCategory)) at \(firstElapsed) through \(friendlyCategory(last.toCategory)) at \(lastElapsed)."
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
