import SwiftUI

// Brand colour tokens live in `VelvtTheme.swift`.

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
    private let compact: Bool

    public init(
        viewModel: InsightViewModel,
        onSuggestedAction: (() -> Void)? = nil,
        compact: Bool = false
    ) {
        self.viewModel = viewModel
        self.onSuggestedAction = onSuggestedAction
        self.compact = compact
    }

    public var body: some View {
        if viewModel.isLoading {
            InsightCardSkeletonView()
        } else {
            InsightCardContentView(
                viewModel: viewModel,
                onSuggestedAction: onSuggestedAction,
                compact: compact
            )
        }
    }
}

// MARK: - InsightCardContentView

private struct InsightCardContentView: View {
    @ObservedObject var viewModel: InsightViewModel
    let onSuggestedAction: (() -> Void)?
    let compact: Bool
    @State private var showsEvidence = false

    private var primaryObservation: String {
        viewModel.observation == InsightEvidence.unavailable.observation
            ? viewModel.text
            : viewModel.observation
    }

    // The daily insight is a message addressed to the person, so the guide's
    // paper stock carries it and the proposed next step sits in the blush inset
    // reserved for an experiment the reader can decline.
    var body: some View {
        VelvtPaperCard(padding: compact ? VelvtMetrics.spaceMD : VelvtMetrics.cardPadding) {
            VStack(alignment: .leading, spacing: compact ? 7 : VelvtMetrics.spaceMD) {
                HStack(alignment: .center) {
                    Text(viewModel.date)
                        .font(VelvtType.caption())
                        .foregroundStyle(VelvtInk.tertiaryOnPaper)
                    Spacer()
                    Text("Daily observation")
                        .font(VelvtType.label())
                        .tracking(VelvtType.labelTracking)
                        .textCase(.uppercase)
                        .foregroundStyle(VelvtInk.labelOnPaper)
                    if compact {
                        Image(systemName: "info.circle")
                            .font(VelvtType.caption(10.5))
                            .foregroundStyle(VelvtInk.tertiaryOnPaper)
                            .help(
                                "\(viewModel.evidenceSummary) Confidence: \(viewModel.confidenceLabel). \(viewModel.generatedAt)."
                            )
                    }
                }

                Text(primaryObservation)
                    .font(compact ? VelvtType.heading(14) : VelvtType.display(20))
                    .lineSpacing(compact ? VelvtType.headingSpacing(14) : VelvtType.displaySpacing(20))
                    .foregroundStyle(VelvtInk.primaryOnPaper)
                    .fixedSize(horizontal: false, vertical: true)
                    .lineLimit(compact ? 2 : nil)
                    .help(compact ? primaryObservation : "")

                if !compact {
                    Text(viewModel.baselineComparison)
                        .velvtBody(12, onPaper: true)
                        .fixedSize(horizontal: false, vertical: true)
                }

                if compact {
                    HStack(spacing: VelvtMetrics.spaceSM) {
                        Text(viewModel.suggestedAction)
                            .font(VelvtType.caption())
                            .foregroundStyle(VelvtInk.secondaryOnPaper)
                            .lineLimit(1)
                            .help(viewModel.suggestedAction)
                        Spacer(minLength: VelvtMetrics.spaceXS)
                        if let onSuggestedAction,
                            !viewModel.suggestedActionButtonLabel.isEmpty
                        {
                            Button("Plan session", action: onSuggestedAction)
                                .buttonStyle(VelvtPrimaryButtonStyle(uppercase: true))
                        }
                    }
                } else {
                    VelvtInsetPanel {
                        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
                            Text("A realistic next step")
                                .font(VelvtType.label())
                                .tracking(VelvtType.labelTracking)
                                .textCase(.uppercase)
                                .foregroundStyle(VelvtInk.labelOnPaper)
                            Text(viewModel.suggestedAction)
                                .velvtHeading(14, onPaper: true)
                                .fixedSize(horizontal: false, vertical: true)
                            if let onSuggestedAction,
                                !viewModel.suggestedActionButtonLabel.isEmpty
                            {
                                Button(viewModel.suggestedActionButtonLabel, action: onSuggestedAction)
                                    .buttonStyle(VelvtPrimaryButtonStyle())
                                    .keyboardShortcut(.defaultAction)
                                    .padding(.top, VelvtMetrics.spaceXS)
                                    .accessibilityHint("Starts this private work block on the local service")
                            }
                        }
                    }
                }

                if !compact {
                    DisclosureGroup("Why am I seeing this?", isExpanded: $showsEvidence) {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(viewModel.evidenceSummary)
                            Text("Confidence: \(viewModel.confidenceLabel). \(viewModel.generatedAt).")
                        }
                        .font(VelvtType.caption(11))
                        .lineSpacing(VelvtType.bodySpacing(11))
                        .foregroundStyle(VelvtInk.tertiaryOnPaper)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, 5)
                    }
                    .font(VelvtType.body(12))
                    .foregroundStyle(VelvtInk.secondaryOnPaper)
                    .tint(VelvtInk.labelOnPaper)
                    .accessibilityHint("Shows the privacy-safe numbers behind this observation")
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Insight for \(viewModel.date)")
        .accessibilityValue("\(primaryObservation). \(viewModel.baselineComparison). \(viewModel.suggestedAction).")
    }
}

// MARK: - InsightCardSkeletonView

struct InsightCardSkeletonView: View {
    var body: some View {
        VelvtPaperCard {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text("Monday, 16 June")
                        .font(VelvtType.caption())
                        .foregroundStyle(VelvtInk.tertiaryOnPaper)
                    Spacer()
                    Text("moderate")
                        .font(VelvtType.label())
                        .tracking(VelvtType.labelTracking)
                        .textCase(.uppercase)
                        .foregroundStyle(VelvtInk.labelOnPaper)
                }
                Text("Your attention stayed on a single context for the longest stretch in several weeks.")
                    .velvtDisplay(20, onPaper: true)
                Text("Generated 14:32")
                    .font(VelvtType.measurement(11))
                    .foregroundStyle(VelvtInk.measurementOnPaper)
            }
        }
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
                .font(VelvtType.caption(10.5))
                .foregroundStyle(VelvtInk.tertiaryOnPaper)
        }
    }

    // Three steps of the same ink, never three hues: confidence is a quality of
    // the evidence, not a score to pass or fail.
    private var dotColor: Color {
        switch label {
        case "high": return VelvtInk.secondaryOnPaper
        case "moderate": return VelvtInk.tertiaryOnPaper
        default: return VelvtPalette.ink.opacity(0.28)  // "early data"
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
            .background(VelvtSurface.ground)
            .preferredColorScheme(.dark)
        }

        static var populatedViewModel: InsightViewModel {
            let vm = InsightViewModel()
            vm.update(
                from: InsightPayload(
                    date: "2026-06-15",
                    text:
                        "Your attention stayed on a single context for the longest stretch in several weeks. The late afternoon stood out.",
                    confidenceLevel: .high,
                    lowConfidence: false,
                    generatedAt: Date()
                ))
            return vm
        }
    }
#endif
