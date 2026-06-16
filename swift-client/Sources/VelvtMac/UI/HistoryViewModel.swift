import Foundation

// MARK: - DaySummaryViewModel

/// Formatted, ready-to-render representation of one day in the 7-day history.
///
/// Numeric fields are `nil` for `no_data` days; views must render "—" for nil.
public struct DaySummaryViewModel: Identifiable, Equatable {
    public let id: String              // YYYY-MM-DD — stable across renders
    public let date: String            // "Mon 9"
    public let statusLabel: String     // "ready" | "no data"
    public let activeTime: String      // "3h 12m" | "45m" | "—"
    public let focusScore: Int?        // nil when isNoData
    public let fragmentationScore: Int?
    public let isNoData: Bool

    public init(_ summary: DailySummary) {
        id = summary.date
        isNoData = summary.status == .noData
        date = DaySummaryViewModel.formatDate(summary.date)
        statusLabel = isNoData ? "no data" : "ready"
        activeTime = isNoData ? "—" : DaySummaryViewModel.formatActiveTime(summary.activeSeconds)
        focusScore = isNoData ? nil : summary.focusScore.map { Int($0.rounded()) }
        fragmentationScore = isNoData ? nil : summary.fragmentationScore.map { Int($0.rounded()) }
    }

    // MARK: - Formatting (internal for testability)

    static func formatDate(_ dateString: String) -> String {
        let parser = DateFormatter()
        parser.dateFormat = "yyyy-MM-dd"
        parser.locale = Locale(identifier: "en_US_POSIX")
        guard let date = parser.date(from: dateString) else { return dateString }
        let display = DateFormatter()
        display.dateFormat = "EEE d"
        return display.string(from: date)
    }

    /// "Xh Ym" when hours > 0; "Ym" when under an hour; "0m" for zero.
    static func formatActiveTime(_ seconds: Int) -> String {
        guard seconds > 0 else { return "0m" }
        let hours = seconds / 3600
        let minutes = (seconds % 3600) / 60
        if hours > 0 {
            return "\(hours)h \(minutes)m"
        }
        return "\(minutes)m"
    }
}

// MARK: - HistoryViewModel

/// Holds the formatted day rows for the 7-day history list.
///
/// Starts empty and loading; populated on the first call to `update(from:)`.
/// Has no knowledge of IPC internals.
@MainActor
public final class HistoryViewModel: ObservableObject {

    @Published public private(set) var days: [DaySummaryViewModel] = []
    @Published public private(set) var isLoading: Bool = true

    public init() {}

    public func update(from payload: HistoryPayload) {
        days = payload.summaries.map(DaySummaryViewModel.init)
        isLoading = false
    }
}
