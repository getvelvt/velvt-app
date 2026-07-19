import SwiftUI

// MARK: - Brand color tokens

extension Color {
    /// Dark ambient card surface.
    static let velvtSurface = Color(red: 0.09, green: 0.08, blue: 0.10)
    /// Off-white primary text (#F2EDE7).
    static let velvtText = Color(red: 0.949, green: 0.929, blue: 0.906)
    /// Muted secondary / metadata text.
    static let velvtMuted = Color(red: 0.949, green: 0.929, blue: 0.906).opacity(0.45)
    static let velvtPanel = Color(red: 0.16, green: 0.11, blue: 0.13)
    static let velvtPanelHighlight = Color(red: 0.21, green: 0.15, blue: 0.18)
    static let velvtPink = Color(red: 0.84, green: 0.38, blue: 0.78)
    static let velvtGreen = Color(red: 0.45, green: 0.86, blue: 0.52)
    static let velvtBlue = Color(red: 0.43, green: 0.78, blue: 0.91)
}

// MARK: - Shimmer modifier

/// Pulses content behind a `.redacted` placeholder to produce a shimmer effect.
struct ShimmerModifier: ViewModifier {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var opacity: Double = 0.3

    func body(content: Content) -> some View {
        content
            .redacted(reason: .placeholder)
            .opacity(reduceMotion ? 0.5 : opacity)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) {
                    opacity = 0.65
                }
            }
    }
}

extension View {
    func shimmering() -> some View {
        modifier(ShimmerModifier())
    }
}

// MARK: - InsightCardView

/// Displays the latest insight or a skeleton while data is loading.
public struct InsightCardView: View {
    @ObservedObject private var viewModel: InsightViewModel
    private let onSuggestedAction: (() -> Void)?

    public init(
        viewModel: InsightViewModel,
        onSuggestedAction: (() -> Void)? = nil
    ) {
        self.viewModel = viewModel
        self.onSuggestedAction = onSuggestedAction
    }

    public var body: some View {
        if viewModel.isLoading {
            InsightCardSkeletonView()
        } else {
            InsightCardContentView(
                viewModel: viewModel,
                onSuggestedAction: onSuggestedAction
            )
        }
    }
}

// MARK: - InsightCardContentView

private struct InsightCardContentView: View {
    @ObservedObject var viewModel: InsightViewModel
    let onSuggestedAction: (() -> Void)?
    @State private var showsEvidence = false

    private var primaryObservation: String {
        viewModel.observation == InsightEvidence.unavailable.observation
            ? viewModel.text
            : viewModel.observation
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center) {
                Text(viewModel.date)
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                Spacer()
                Text("Daily observation")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            }

            Text(primaryObservation)
                .font(.body.weight(.medium))
                .foregroundStyle(Color.velvtText)
                .fixedSize(horizontal: false, vertical: true)

            Text(viewModel.baselineComparison)
                .font(.caption)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 4) {
                Text("A realistic next step")
                    .font(.caption2.bold())
                    .foregroundStyle(Color.velvtMuted)
                Text(viewModel.suggestedAction)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color.velvtText)
                    .fixedSize(horizontal: false, vertical: true)
                if let onSuggestedAction,
                   !viewModel.suggestedActionButtonLabel.isEmpty {
                    Button(viewModel.suggestedActionButtonLabel, action: onSuggestedAction)
                        .buttonStyle(.borderedProminent)
                        .controlSize(.large)
                        .keyboardShortcut(.defaultAction)
                        .padding(.top, 4)
                        .accessibilityHint("Starts this private work block on the local service")
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.velvtPanelHighlight.opacity(0.75))
            .clipShape(RoundedRectangle(cornerRadius: 7))

            DisclosureGroup("Why am I seeing this?", isExpanded: $showsEvidence) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(viewModel.evidenceSummary)
                    Text("Confidence: \(viewModel.confidenceLabel). \(viewModel.generatedAt).")
                }
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 5)
            }
            .font(.caption)
            .tint(Color.velvtText)
            .accessibilityHint("Shows the privacy-safe numbers behind this observation")
        }
        .padding(14)
        .background(Color.velvtSurface)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Insight for \(viewModel.date)")
        .accessibilityValue("\(primaryObservation). \(viewModel.baselineComparison). \(viewModel.suggestedAction).")
    }
}

// MARK: - InsightCardSkeletonView

struct InsightCardSkeletonView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Monday, 16 June")
                    .font(.caption)
                Spacer()
                Text("moderate")
                    .font(.caption2)
            }
            Text("Your attention stayed on a single context for the longest stretch in several weeks.")
                .font(.body)
            Text("Generated 14:32")
                .font(.system(.caption2, design: .monospaced))
        }
        .padding(14)
        .background(Color.velvtSurface)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .shimmering()
    }
}

// MARK: - ConfidenceDotView

struct ConfidenceDotView: View {
    let label: String

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(dotColor)
                .frame(width: 5, height: 5)
            Text(label)
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
        }
    }

    private var dotColor: Color {
        switch label {
        case "high":       return Color.velvtMuted
        case "moderate":   return Color.velvtMuted.opacity(0.65)
        default:           return Color.velvtMuted.opacity(0.40) // "early data"
        }
    }
}

// MARK: - Preview

#if DEBUG
@MainActor
struct InsightCardView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            InsightCardView(viewModel: populatedViewModel)
                .previewDisplayName("Populated")
            InsightCardView(viewModel: InsightViewModel())
                .previewDisplayName("Skeleton")
        }
        .padding()
        .preferredColorScheme(.dark)
    }

    static var populatedViewModel: InsightViewModel {
        let vm = InsightViewModel()
        vm.update(from: InsightPayload(
            date: "2026-06-15",
            text: "Your attention stayed on a single context for the longest stretch in several weeks. The late afternoon stood out.",
            confidenceLevel: .high,
            lowConfidence: false,
            generatedAt: Date()
        ))
        return vm
    }
}
#endif
