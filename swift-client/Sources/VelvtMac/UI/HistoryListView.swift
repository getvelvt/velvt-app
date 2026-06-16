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
        } else {
            VStack(spacing: 0) {
                ForEach(viewModel.days) { day in
                    HistoryDayRowView(day: day)
                        .id(day.id)
                    if day.id != viewModel.days.last?.id {
                        Divider()
                            .opacity(0.12)
                            .padding(.horizontal, 14)
                    }
                }
            }
        }
    }
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
                                activeSeconds: 3600 + i * 900)
        }
    }()
}
#endif
