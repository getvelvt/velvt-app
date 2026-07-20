import SwiftUI

// MARK: - HistoryListView

/// Renders seven day-rows or a skeleton while history is loading.
public struct HistoryListView: View {
    @ObservedObject private var viewModel: HistoryViewModel

    public init(viewModel: HistoryViewModel) {
        self.viewModel = viewModel
    }

    public var body: some View {
        if viewModel.isLoading {
            HistorySkeletonView()
        } else if viewModel.days.isEmpty {
            EmptyDeliveryState(text: "No daily history generated yet", systemImage: "calendar")
        } else {
            HistoryDashboardView(days: viewModel.days)
        }
    }
}

// MARK: - HistoryDashboardView

private struct HistoryDashboardView: View {
    let days: [DaySummaryViewModel]

    private var readyDays: [DaySummaryViewModel] {
        days.filter { !$0.isNoData }
    }

    private var latestDay: DaySummaryViewModel? {
        readyDays.last
    }

    private var movingAverageText: String {
        guard let latestDay else { return "No personal baseline yet" }
        guard latestDay.baselineStatus == "mature" || latestDay.baselineComparison?.status == "compared" else {
            return "Collecting a seven-day baseline"
        }
        if let delta = latestDay.baselineComparison?.fragmentationDelta {
            if delta > 0 {
                return "+\(formatDelta(delta)) vs your usual range"
            }
            return "\(formatDelta(delta)) vs your usual range"
        }
        return "Compared with your usual range"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            dailyActivity
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Daily insight history")
    }

    private var dailyActivity: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Your week")
                        .font(.headline)
                        .foregroundStyle(Color.velvtText)
                    Text(movingAverageText)
                        .font(.caption2)
                        .foregroundStyle(Color.velvtMuted)
                }
                Spacer()
                Text("7 days")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            }

            VStack(spacing: 3) {
                ForEach(days) { day in
                    DailyActivityRow(day: day)
                }
            }
        }
        .padding(10)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private func formatDelta(_ value: Double) -> String {
        String(format: "%.1f", value)
    }
}

private struct DailyActivityRow: View {
    let day: DaySummaryViewModel
    @State private var hoveredCategoryDetail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                HStack(spacing: 5) {
                    Text(day.date)
                        .font(.caption.bold())
                        .foregroundStyle(day.isNoData ? Color.velvtMuted.opacity(0.5) : Color.velvtText)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Text(day.isNoData ? "No data" : day.activeTime)
                        .font(.caption2)
                        .foregroundStyle(Color.velvtMuted)
                }
                Spacer(minLength: 8)
                metric("Context changes", day.isNoData ? "—" : "\(day.meaningfulSwitchCount)")
                metric("Switching level", switchingLevel)
            }

            SplitActivityBar(day: day) { detail in
                hoveredCategoryDetail = detail
            }
                .frame(height: 10)

            if let hoveredCategoryDetail {
                Text(hoveredCategoryDetail)
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
                    .transition(.opacity)
            }
        }
        .padding(.vertical, 5)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(day.date)
        .accessibilityValue(
            "\(day.isNoData ? "No activity" : day.activeTime), \(day.meaningfulSwitchCount) context changes, switching level \(switchingLevel)"
        )
    }

    private var switchingLevel: String {
        guard let score = day.fragmentationScore else { return "—" }
        switch score {
        case ..<34: return "Low"
        case ..<67: return "Moderate"
        default: return "High"
        }
    }

    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(title)
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(Color.velvtMuted)
            Text(value)
                .font(.system(.caption2, design: .monospaced).weight(.semibold))
                .foregroundStyle(value == "—" ? Color.velvtMuted.opacity(0.45) : Color.velvtPink)
        }
        .frame(minWidth: title == "Switching level" ? 68 : 64, alignment: .trailing)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title): \(value)")
    }
}

private struct SplitActivityBar: View {
    let day: DaySummaryViewModel
    let onHover: (String?) -> Void

    private var segments: [ActivityProportion] {
        day.typeProportions.filter { $0.proportion > 0 }.prefix(5).map { $0 }
    }

    var body: some View {
        GeometryReader { proxy in
            HStack(spacing: 2) {
                if segments.isEmpty {
                        RoundedRectangle(cornerRadius: 3)
                            .fill(Color.white.opacity(day.isNoData ? 0.06 : 0.12))
                            .help(emptyHelpText)
                            .onHover { hovering in
                                onHover(hovering ? emptyHelpText : nil)
                            }
                } else {
                    ForEach(Array(segments.enumerated()), id: \.element.category) { index, segment in
                        let text = helpText(for: segment)
                        RoundedRectangle(cornerRadius: 3)
                            .fill(color(for: segment.category, index: index))
                            .frame(width: max(5, proxy.size.width * CGFloat(segment.proportion)))
                            .help(text)
                            .onHover { hovering in
                                onHover(hovering ? text : nil)
                            }
                    }
                }
            }
        }
        .onHover { hovering in
            if !hovering {
                onHover(nil)
            }
        }
        .frame(height: 10)
    }

    private var emptyHelpText: String {
        day.isNoData ? "No daily summary for this day." : "No activity mix available for this summary yet."
    }

    private func helpText(for segment: ActivityProportion) -> String {
        "\(categoryLabel(segment.category)): \(formatSeconds(segment.seconds)), \(Int((segment.proportion * 100).rounded()))% of active time."
    }

    private func color(for category: String, index: Int) -> Color {
        let palette: [Color] = [.velvtPink, .velvtGreen, .velvtBlue, .purple.opacity(0.85), .orange.opacity(0.85)]
        return palette[index % palette.count]
    }
}

private func categoryLabel(_ category: String) -> String {
    category
        .replacingOccurrences(of: "_", with: " ")
        .split(separator: " ")
        .map { $0.prefix(1).uppercased() + $0.dropFirst() }
        .joined(separator: " ")
}

private func formatSeconds(_ seconds: Int) -> String {
    let hours = seconds / 3600
    let minutes = (seconds % 3600) / 60
    if hours > 0 {
        return "\(hours)h \(minutes)m"
    }
    return "\(minutes)m"
}

// MARK: - HistoryDayRowView

struct HistoryDayRowView: View {
    let day: DaySummaryViewModel

    var body: some View {
        HStack(spacing: 0) {
            Text(day.date)
                .font(.caption)
                .foregroundStyle(day.isNoData ? Color.velvtMuted.opacity(0.4) : Color.velvtMuted)
                .frame(width: 52, alignment: .leading)

            Spacer()

            Text(day.activeTime)
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(day.isNoData ? Color.velvtMuted.opacity(0.35) : Color.velvtMuted)
                .frame(width: 52, alignment: .trailing)

            scoreCell(day.focusScore)
            scoreCell(day.fragmentationScore)
        }
        .padding(.vertical, 5)
        .padding(.horizontal, 14)
        .focusable()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(day.date), \(day.statusLabel)")
        .accessibilityValue(accessibilityValue)
    }

    private var accessibilityValue: String {
        guard !day.isNoData else { return "No data" }
        var parts = ["Active \(day.activeTime)"]
        if let focusScore = day.focusScore { parts.append("Focus \(focusScore)") }
        if let fragmentationScore = day.fragmentationScore { parts.append("Fragmentation \(fragmentationScore)") }
        return parts.joined(separator: ", ")
    }

    @ViewBuilder
    private func scoreCell(_ score: Int?) -> some View {
        Group {
            if let score {
                Text("\(score)")
                    .foregroundStyle(Color.velvtMuted)
            } else {
                Text("—")
                    .foregroundStyle(Color.velvtMuted.opacity(0.35))
            }
        }
        .font(.system(.caption2, design: .monospaced))
        .frame(width: 34, alignment: .trailing)
    }
}

// MARK: - HistorySkeletonView

struct HistorySkeletonView: View {
    var body: some View {
        VStack(spacing: 0) {
            ForEach(0 ..< 7, id: \.self) { _ in
                HStack(spacing: 0) {
                    Text("Mon 9")
                        .font(.caption)
                        .frame(width: 52, alignment: .leading)
                    Spacer()
                    Text("2h 15m")
                        .font(.system(.caption2, design: .monospaced))
                        .frame(width: 52, alignment: .trailing)
                    Text("72")
                        .font(.system(.caption2, design: .monospaced))
                        .frame(width: 34, alignment: .trailing)
                    Text("18")
                        .font(.system(.caption2, design: .monospaced))
                        .frame(width: 34, alignment: .trailing)
                }
                .padding(.vertical, 5)
                .padding(.horizontal, 14)
                .shimmering()
            }
        }
    }
}

// MARK: - Preview

#if DEBUG
@MainActor
struct HistoryListView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            HistoryListView(viewModel: populatedViewModel)
                .previewDisplayName("Populated")
            HistoryListView(viewModel: HistoryViewModel())
                .previewDisplayName("Skeleton")
        }
        .preferredColorScheme(.dark)
    }

    static var populatedViewModel: HistoryViewModel {
        let vm = HistoryViewModel()
        vm.update(from: HistoryPayload(days: 7, summaries: previewSummaries))
        return vm
    }

    static let previewSummaries: [DailySummary] = {
        let dates = ["2026-06-09", "2026-06-10", "2026-06-11", "2026-06-12",
                     "2026-06-13", "2026-06-14", "2026-06-15"]
        return dates.enumerated().map { i, date in
            if i % 3 == 0 {
                return DailySummary(date: date, status: .noData, eventCount: 0,
                                    focusScore: nil, fragmentationScore: nil,
                                    confidenceLevel: .low, activeSeconds: 0)
            }
            return DailySummary(date: date, status: .ready, eventCount: 40 + i * 8,
                                focusScore: 55.0 + Double(i * 5),
                                fragmentationScore: 30.0 - Double(i * 3),
                                confidenceLevel: .medium,
                                activeSeconds: 3600 + i * 900,
                                baselineStatus: i > 4 ? "mature" : "early_stage",
                                baselineComparison: BaselineComparison(
                                    status: i > 4 ? "compared" : "early_stage",
                                    fragmentationDelta: i > 4 ? -4.5 : nil,
                                    focusDelta: i > 4 ? 3.2 : nil
                                ),
                                typeProportions: [
                                    ActivityProportion(category: "document", seconds: 1600 + i * 120, proportion: 0.48),
                                    ActivityProportion(category: "messaging", seconds: 900, proportion: 0.27),
                                    ActivityProportion(category: "browser", seconds: 820, proportion: 0.25),
                                ])
        }
    }()
}
#endif
