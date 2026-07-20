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
            explanation: "Changes between privacy-safe broad categories; system and unclassified activity are excluded."
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
                || coordinator.historyViewModel.baselineProgress.isComplete {
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
              signal.status == .ready else { return nil }
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
                Text("Updated \(signal.observedThrough.formatted(date: .omitted, time: .shortened)) · raw app names, titles, URLs, and files stay on this Mac")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text(errorMessage ?? "Waiting for the local privacy service to report this observation window.")
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
            return "Velvt needs about \(signal.requiredSeconds) more seconds of qualifying activity before it can show your first local pattern."
        }
        return "Velvt is checking that this activity can be summarized without exposing private details."
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
            Text("Computed only from abstracted categories on this Mac · Updated \(signal.observedThrough.formatted(date: .omitted, time: .shortened))")
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
        return "\(start.formatted(date: .omitted, time: .shortened))–\(signal.observedThrough.formatted(date: .omitted, time: .shortened))"
    }
}

/// The live Focus Fragmentation surface. All timing and switch-rate values
/// come from the local Rust service; this view only lays out safe segments.
public struct LocalDashboardView: View {
    @ObservedObject private var coordinator: LocalDashboardCoordinator

    public init(coordinator: LocalDashboardCoordinator) {
        self.coordinator = coordinator
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Activity")
                        .font(.headline)
                        .foregroundStyle(Color.velvtText)
                    Text("A rough view of the last hour")
                        .font(.caption2)
                        .foregroundStyle(Color.velvtMuted)
                }
                Spacer()
                metric
            }

            if let snapshot = coordinator.snapshot {
                timeline(snapshot)
                Text(coverageText(snapshot.coverage))
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                categoryGuide
            } else if let commandError = coordinator.commandError {
                Text(commandError)
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
            } else {
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(14)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Local activity timeline")
    }

    private var metric: some View {
        VStack(alignment: .trailing, spacing: 2) {
            if let snapshot = coordinator.snapshot {
                Text(String(format: "%.1f", snapshot.switchesPerHour))
                    .font(.headline.monospacedDigit())
                Text("switches / hour")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            } else {
                Text("—")
                    .font(.headline.monospacedDigit())
                Text("switches / hour")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Switches per hour")
        .accessibilityValue(coordinator.snapshot.map { String(format: "%.1f", $0.switchesPerHour) } ?? "No data")
    }

    private func timeline(_ snapshot: LocalDashboardSnapshot) -> some View {
        GeometryReader { proxy in
            HStack(spacing: 2) {
                if snapshot.segments.isEmpty {
                    RoundedRectangle(cornerRadius: 4)
                        .fill(Color.white.opacity(0.08))
                        .frame(maxWidth: .infinity)
                        .accessibilityLabel("No live activity data")
                } else {
                    ForEach(snapshot.segments) { segment in
                        let duration = max(1, segment.endedAt.timeIntervalSince(segment.startedAt))
                        let window = max(1, snapshot.windowEnd.timeIntervalSince(snapshot.windowStart))
                        RoundedRectangle(cornerRadius: 3)
                            .fill(color(for: segment.category))
                            .frame(width: max(4, proxy.size.width * duration / window))
                            .help("\(categoryLabel(segment.category)): \(categoryDescription(segment.category)); \(formatDuration(duration))")
                            .accessibilityLabel("\(categoryLabel(segment.category)): \(categoryDescription(segment.category)); \(formatDuration(duration))")
                    }
                }
            }
        }
        .frame(height: 18)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Last hour of broad, approximate activity groupings")
    }

    private var categoryGuide: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Color guide")
                .font(.caption2.bold())
                .foregroundStyle(Color.velvtText)
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    guideItem("FOCUS_WORK")
                    guideItem("REFERENCE")
                }
                VStack(alignment: .leading, spacing: 4) {
                    guideItem("COMMUNICATION")
                    guideItem("CREATIVE")
                }
            }
            Text("These are rough local groupings, not a productivity score or a judgment.")
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func guideItem(_ category: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            Circle()
                .fill(color(for: category))
                .frame(width: 6, height: 6)
            Text(categoryLabel(category))
                .font(.caption2)
                .foregroundStyle(Color.velvtText)
            Text("— \(categoryDescription(category))")
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
        }
        .help("\(categoryLabel(category)): \(categoryDescription(category))")
    }

    private func coverageText(_ coverage: LocalDashboardCoverage) -> String {
        switch coverage {
        case .noData: return "No live activity data yet."
        case .partial: return "Partial coverage; the timeline is still building."
        case .good: return "Observed category movement; labels are local and approximate."
        }
    }

    private func color(for category: String) -> Color {
        switch category {
        case "FOCUS_WORK": return .velvtGreen
        case "COMMUNICATION": return .velvtPink
        case "REFERENCE": return .velvtBlue
        case "CREATIVE": return .orange.opacity(0.85)
        default: return .velvtMuted.opacity(0.6)
        }
    }

    private func categoryLabel(_ category: String) -> String {
        switch category {
        case "FOCUS_WORK": return "Focus work"
        case "REFERENCE": return "Reading & research"
        case "COMMUNICATION": return "Messages & meetings"
        case "CREATIVE": return "Design & creative"
        default: return "Other activity"
        }
    }

    private func categoryDescription(_ category: String) -> String {
        switch category {
        case "FOCUS_WORK": return "writing, coding, or building"
        case "REFERENCE": return "reading, notes, or research"
        case "COMMUNICATION": return "email, chat, or calls"
        case "CREATIVE": return "visual or audio work"
        default: return "not enough information to tell"
        }
    }

    private func formatDuration(_ seconds: TimeInterval) -> String {
        let rounded = Int(seconds.rounded())
        let minutes = rounded / 60
        let remainder = rounded % 60
        return minutes > 0
            ? String(format: "%dm %ds", minutes, remainder)
            : String(format: "%ds", remainder)
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
        c.updateInsight(InsightPayload(
            date: "2026-06-15",
            text: "Your focus held steady across the morning block, with fewer context switches than the previous week.",
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
