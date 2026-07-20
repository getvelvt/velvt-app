import SwiftUI

extension View {
    func tourHighlight(_ isHighlighted: Bool) -> some View {
        overlay {
            if isHighlighted {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color.velvtPink, lineWidth: 2)
                    .padding(2)
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

    private func progressText(_ signal: LocalEarlySignal) -> String {
        if signal.requiredSeconds > 0 {
      return
        "Velvt needs about \(signal.requiredSeconds) more seconds of qualifying activity before it can show your first local pattern."
        }
    return
      "Velvt is checking that this activity can be summarized without exposing private details."
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

public enum MinimalDashboardSurface: String, CaseIterable, Identifiable {
  case focus
  case activity

  public var id: String { rawValue }
  public var title: String { rawValue.capitalized }
}

/// The popover's single analytical workspace. The insight remains above the
/// persisted two-item control, so switching surfaces never hides the action.
public struct MinimalDashboardWorkspaceView: View {
  @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
  @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
  @ObservedObject private var localDashboardCoordinator: LocalDashboardCoordinator
  private let onStartWorkBlock: () -> Void
  @AppStorage("velvt.minimalDashboard.surface") private var selectedRawValue =
    MinimalDashboardSurface.focus.rawValue

  public init(
    coordinator: ConcreteDisplayDataCoordinator,
    workBlockCoordinator: WorkBlockCoordinator,
    localDashboardCoordinator: LocalDashboardCoordinator,
    onStartWorkBlock: @escaping () -> Void
  ) {
    self.coordinator = coordinator
    self.workBlockCoordinator = workBlockCoordinator
    self.localDashboardCoordinator = localDashboardCoordinator
    self.onStartWorkBlock = onStartWorkBlock
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if let snapshot = workBlockCoordinator.snapshot,
        snapshot.phase == .active || snapshot.phase == .paused
      {
        CompactWorkBlockControl(snapshot: snapshot, coordinator: workBlockCoordinator)
      }

      latestInsight

      Picker("Analysis", selection: $selectedRawValue) {
        ForEach(MinimalDashboardSurface.allCases) { surface in
          Text(surface.title).tag(surface.rawValue)
        }
      }
      .pickerStyle(.segmented)
      .accessibilityLabel("Choose Focus Fragmentation or Daily Activity")
      .help(
        "Focus explains one explicit work block. Activity shows exactly seven local calendar days.")

      if selectedRawValue == MinimalDashboardSurface.activity.rawValue {
        DailyActivityView(snapshot: localDashboardCoordinator.snapshot)
      } else {
        FocusFragmentationView(
          focus: localDashboardCoordinator.snapshot?.focusFragmentation,
          errorMessage: localDashboardCoordinator.commandError,
          onStartWorkBlock: onStartWorkBlock
        )
      }
    }
    .padding(12)
    .onAppear { localDashboardCoordinator.refresh() }
  }

  @ViewBuilder
  private var latestInsight: some View {
    if coordinator.insightAvailability == .available {
      InsightCardView(
        viewModel: coordinator.insightViewModel,
        onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
          ? startSuggestedWorkBlock
          : nil
      )
    } else if let signal = localDashboardCoordinator.snapshot?.earlySignal,
      signal.status == .ready
    {
      EarlyLocalSignalView(
        signal: signal,
        onSuggestedAction: workBlockCoordinator.snapshot?.phase == .idle
          ? { startLocalSignalBlock(signal) }
          : nil
      )
    } else {
      EarlySignalProgressView(
        signal: localDashboardCoordinator.snapshot?.earlySignal,
        errorMessage: localDashboardCoordinator.commandError
      )
    }
  }

  private func startSuggestedWorkBlock() {
    workBlockCoordinator.startBlock(
      intention: nil,
      durationSeconds: max(5, coordinator.insightViewModel.suggestedActionMinutes) * 60,
      purpose: nil,
      intensity: .medium
    )
  }

  private func startLocalSignalBlock(_ signal: LocalEarlySignal) {
    workBlockCoordinator.startBlock(
      intention: nil,
      durationSeconds: max(5, signal.actionMinutes) * 60,
      purpose: nil,
      intensity: .medium
    )
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
  @State private var hoveredDetail: String?
  @FocusState private var focusedEvidenceID: String?

  public var body: some View {
    VStack(alignment: .leading, spacing: 9) {
      Text("Focus Fragmentation")
        .font(.headline)
      if let focus {
        Text(focus.windowLabel)
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
          .accessibilityLabel("Timeline window: \(focus.windowLabel)")
          .help(
            "This window contains at most the most recent 60 minutes of one explicit work block.")
        focusTimeline(focus)
        if let hoveredDetail {
          Text(hoveredDetail)
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel(hoveredDetail)
        }
        metrics(focus)
        if let comparison = focus.comparison {
          Text("\(comparison.label): \(comparison.explanation)")
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .help(
              "Like-for-like comparison of covered 60-minute active windows. Partial or low-confidence windows are never compared."
            )
            .accessibilityLabel("Comparison \(comparison.label). \(comparison.explanation)")
        } else {
          Text("Not enough comparable activity")
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .help(
              "No delta is shown unless both 60-minute windows have sufficient classified coverage."
            )
        }
        Text(focus.observation)
          .font(.caption)
          .fixedSize(horizontal: false, vertical: true)
        Text(focus.nextAction)
          .font(.caption.bold())
          .fixedSize(horizontal: false, vertical: true)
      } else {
        VStack(alignment: .leading, spacing: 8) {
          Text(
            errorMessage
              ?? "Start a work block to see its attention timeline. Velvt does not infer your intent from general activity."
          )
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
    .accessibilityLabel("Focus Fragmentation timeline, \(focus.windowLabel)")
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
    .accessibilityHint("Observed category movement is not proof of distraction")
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
    .accessibilityHint("A switch is observed category movement, not proof of distraction")
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
      columns: [GridItem(.adaptive(minimum: 82), spacing: 8)], alignment: .leading, spacing: 6
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
      focusMetric(
        "Recoveries", "\(focus.recoveryCount)",
        "A recovery is a return to the block's dominant covered category after moving away, using the documented session rule."
      )
      focusMetric(
        "Clusters", "\(focus.clusters.count)",
        "A cluster is at least three category transitions inside five minutes under rule version 1."
      )
      focusMetric(
        "Coverage", coverageLabel(focus),
        "The share and confidence of this window that could be classified without guessing.")
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

public struct DailyActivityView: View {
  let snapshot: LocalDashboardSnapshot?
  @State private var hoveredDetail: String?
  @FocusState private var focusedSegmentID: String?

  public var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text("Daily Activity").font(.headline)
        Spacer()
        Text("7 days").font(.caption2).foregroundStyle(Color.velvtMuted)
      }
      if let days = snapshot?.dailyActivity, days.count == 7 {
        ForEach(days) { day in
          dayRow(day)
        }
      } else {
        ForEach(0..<7, id: \.self) { _ in
          RoundedRectangle(cornerRadius: 3)
            .fill(Color.white.opacity(0.06))
            .frame(height: 28)
        }
        Text("Still building the seven local day rows.")
          .font(.caption2).foregroundStyle(Color.velvtMuted)
      }
      if let hoveredDetail {
        Text(hoveredDetail)
          .font(.caption2)
          .foregroundStyle(Color.velvtMuted)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityLabel(hoveredDetail)
      }
    }
    .padding(12)
    .background(Color.velvtPanel)
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .onChange(of: focusedSegmentID) { _ in updateFocusedDetail() }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Daily Activity, exactly seven days")
  }

  private func dayRow(_ day: LocalDailyActivityDay) -> some View {
    HStack(spacing: 8) {
      VStack(alignment: .leading, spacing: 1) {
        Text(dayLabel(day.date)).font(.caption.bold())
        Text(stateLabel(day)).font(.caption2).foregroundStyle(Color.velvtMuted)
      }
      .frame(width: 104, alignment: .leading)
      GeometryReader { proxy in
        HStack(spacing: 1) {
          if day.segments.isEmpty {
            RoundedRectangle(cornerRadius: 3)
              .fill(Color.white.opacity(0.07))
              .help(stateLabel(day))
              .accessibilityLabel("\(dayLabel(day.date)), \(stateLabel(day))")
          } else {
            ForEach(Array(day.segments.enumerated()), id: \.element.id) { index, segment in
              let detail = segmentDetail(segment, day: day)
              Button {
                hoveredDetail = detail
              } label: {
                let width = max(6, proxy.size.width * CGFloat(segment.percentage) / 100)
                ZStack {
                  RoundedRectangle(cornerRadius: 3)
                    .fill(palette(index, category: segment.category))
                  Text(segment.label)
                    .font(.system(size: 7, weight: .semibold))
                    .foregroundStyle(Color.black.opacity(0.72))
                    .lineLimit(1)
                    .minimumScaleFactor(0.55)
                    .padding(.horizontal, 2)
                }
                .frame(width: width)
              }
              .buttonStyle(.plain)
              .help(detail)
              .focused($focusedSegmentID, equals: segment.id)
              .onHover { hoveredDetail = $0 ? detail : nil }
              .accessibilityLabel(detail)
              .accessibilityHint(
                segment.explanation
                  ?? "No additional grounded evidence is available for this segment.")
            }
          }
        }
      }
      .frame(height: 14)
      Text(duration(day.activeSeconds))
        .font(.caption2.monospacedDigit())
        .foregroundStyle(Color.velvtMuted)
        .frame(width: 46, alignment: .trailing)
    }
    .frame(height: 31)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(
      "\(dayLabel(day.date)), \(stateLabel(day)), \(duration(day.activeSeconds)) recorded")
  }

  private func updateFocusedDetail() {
    guard let id = focusedSegmentID,
      let day = snapshot?.dailyActivity.first(where: {
        $0.segments.contains(where: { $0.id == id })
      }),
      let segment = day.segments.first(where: { $0.id == id })
    else { return }
    hoveredDetail = segmentDetail(segment, day: day)
  }

  private func segmentDetail(_ segment: LocalDailyActivitySegment, day: LocalDailyActivityDay)
    -> String
  {
    let base =
      "\(dayLabel(day.date)), \(segment.label), \(duration(segment.durationSeconds)), \(segment.percentage)%, \(segment.confidence.rawValue) confidence."
    return [base, segment.explanation].compactMap { $0 }.joined(separator: " ")
  }

  private func stateLabel(_ day: LocalDailyActivityDay) -> String {
    switch day.state {
    case .noData: return "No data"
    case .lowConfidence: return "Low confidence"
    case .stillBuilding: return "Still building"
    case .ready:
      return day.segments.contains(where: { $0.label == "Unclassified" })
        ? "Includes Unclassified" : "Recorded"
    }
  }

  private func dayLabel(_ value: String) -> String {
    let parser = DateFormatter()
    parser.dateFormat = "yyyy-MM-dd"
    parser.locale = Locale(identifier: "en_US_POSIX")
    guard let date = parser.date(from: value) else { return value }
    let formatter = DateFormatter()
    formatter.dateFormat = "EEE d"
    return formatter.string(from: date)
  }

  private func palette(_ index: Int, category: String) -> Color {
    if category == "UNCLASSIFIED" { return Color.gray.opacity(0.55) }
    if category == "OTHER" { return Color.velvtMuted.opacity(0.5) }
    return [
      Color.velvtGreen, .velvtPink, .velvtBlue, .orange.opacity(0.85), .purple.opacity(0.85),
    ][index % 5]
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
