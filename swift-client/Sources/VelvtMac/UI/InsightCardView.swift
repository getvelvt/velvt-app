import SwiftUI

// MARK: - Brand color tokens

extension Color {
    /// Dark ambient card surface.
    static let velvtSurface = Color(red: 0.09, green: 0.08, blue: 0.10)
    /// Off-white primary text (#F2EDE7).
    static let velvtText = Color(red: 0.949, green: 0.929, blue: 0.906)
    /// Muted secondary / metadata text.
    static let velvtMuted = Color(red: 0.949, green: 0.929, blue: 0.906).opacity(0.45)
}

// MARK: - Shimmer modifier

/// Pulses content behind a `.redacted` placeholder to produce a shimmer effect.
struct ShimmerModifier: ViewModifier {
    @State private var opacity: Double = 0.3

    func body(content: Content) -> some View {
        content
            .redacted(reason: .placeholder)
            .opacity(opacity)
            .onAppear {
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

    public init(viewModel: InsightViewModel) {
        self.viewModel = viewModel
    }

    public var body: some View {
        if viewModel.isLoading {
            InsightCardSkeletonView()
        } else {
            InsightCardContentView(viewModel: viewModel)
        }
    }
}

// MARK: - InsightCardContentView

private struct InsightCardContentView: View {
    @ObservedObject var viewModel: InsightViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center) {
                Text(viewModel.date)
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                Spacer()
                ConfidenceDotView(label: viewModel.confidenceLabel)
            }

            Text(viewModel.text)
                .font(.body)
                .foregroundStyle(Color.velvtText)
                .fixedSize(horizontal: false, vertical: true)

            Text(viewModel.generatedAt)
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(Color.velvtMuted.opacity(0.7))
        }
        .padding(14)
        .background(Color.velvtSurface)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .focusable()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Insight for \(viewModel.date)")
        .accessibilityValue("\(viewModel.text). Confidence: \(viewModel.confidenceLabel). \(viewModel.generatedAt).")
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
