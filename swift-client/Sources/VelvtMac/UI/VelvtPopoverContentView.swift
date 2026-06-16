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
                InsightCardView(viewModel: insightVM)
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                sectionDivider
                HistoryListView(viewModel: historyVM)
                    .padding(.bottom, 8)

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
