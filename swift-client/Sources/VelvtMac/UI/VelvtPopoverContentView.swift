import SwiftUI

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
                InsightCardSkeletonView()
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                sectionDivider
                HistorySkeletonView()
                    .padding(.bottom, 8)

            case .populated(let insightVM, let historyVM):
                insightSection(viewModel: insightVM)
                sectionDivider
                historySection(viewModel: historyVM)

            case .error(let message):
                InsightCardSkeletonView()
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                sectionDivider
                HistorySkeletonView()
                IPCStatusBanner(message: message)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 10)
            }
        }
        .preferredColorScheme(.dark)
    }

    private var sectionDivider: some View {
        Divider()
            .opacity(0.15)
            .padding(.vertical, 8)
    }

    @ViewBuilder
    private func insightSection(viewModel: InsightViewModel) -> some View {
        switch coordinator.insightAvailability {
        case .available:
            InsightCardView(viewModel: viewModel)
                .padding(.horizontal, 16)
                .padding(.top, 12)
        case .notGenerated:
            EmptyDeliveryState(text: "No daily insight generated yet", systemImage: "sparkles")
                .padding(.horizontal, 16)
                .padding(.top, 16)
        case .loading:
            InsightCardSkeletonView()
                .padding(.horizontal, 16)
                .padding(.top, 12)
        }
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
                    Text("Live Focus")
                        .font(.headline)
                        .foregroundStyle(Color.velvtText)
                    Text("Last 60 minutes")
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
        .accessibilityLabel("Live focus timeline")
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
                            .help("\(categoryLabel(segment.category)), \(formatDuration(duration))")
                            .accessibilityLabel("\(categoryLabel(segment.category)), \(formatDuration(duration))")
                    }
                }
            }
        }
        .frame(height: 18)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Last 60 minutes of observed categories")
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
        category
            .replacingOccurrences(of: "_", with: " ")
            .split(separator: " ")
            .map { $0.prefix(1).uppercased() + $0.dropFirst().lowercased() }
            .joined(separator: " ")
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
