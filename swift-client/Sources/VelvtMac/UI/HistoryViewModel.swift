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

// MARK: - ScrollToDateAction

/// Callable wrapper around a request to scroll the history list to a given
/// date. `@MainActor`-isolated because its sole concrete use
/// (`HistoryViewModel.scrollToDate`) mutates main-actor-isolated state.
@MainActor
public struct ScrollToDateAction {
    private let action: (String) -> Void

    public init(_ action: @escaping (String) -> Void) {
        self.action = action
    }

    public func callAsFunction(_ date: String) {
        action(date)
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
    /// The date most recently requested for scroll-into-view, e.g. by a
    /// tapped notification. Views observe this to drive a `ScrollViewReader`.
    @Published public private(set) var scrollTarget: String?

    public init() {}

    public func update(from payload: HistoryPayload) {
        let mapped = payload.summaries.map(DaySummaryViewModel.init)
        days = HistoryViewModel.padded(mapped, toCount: payload.days)
        isLoading = false
    }

    public func scrollToDate(_ date: String) {
        scrollTarget = date
    }

    /// Bound action passed to notification-tap routing so it stays decoupled
    /// from this concrete view model type.
    public var scrollToDateAction: ScrollToDateAction {
        ScrollToDateAction { [weak self] date in self?.scrollToDate(date) }
    }

    /// Prepends synthetic no_data stubs for dates before the earliest known day
    /// when the server sends fewer summaries than the requested window.
    ///
    /// New-user invariant: a user on day 2 with a 7-day window sees 5 no_data
    /// rows followed by 2 real rows — never a shorter-than-expected list.
    static func padded(_ existing: [DaySummaryViewModel], toCount target: Int) -> [DaySummaryViewModel] {
        guard target > existing.count, let earliest = existing.first else {
            return existing
        }
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd"
        formatter.locale = Locale(identifier: "en_US_POSIX")
        guard let earliestDate = formatter.date(from: earliest.id) else { return existing }
        let missing = target - existing.count
        // (1...missing).reversed() → offsets [missing, ..., 1] → chronological ascending order.
        let stubs: [DaySummaryViewModel] = (1 ... missing).reversed().compactMap { offset in
            guard let date = Calendar.current.date(byAdding: .day, value: -offset, to: earliestDate)
            else { return nil }
            let stub = DailySummary(
                date: formatter.string(from: date),
                status: .noData, eventCount: 0,
                focusScore: nil, fragmentationScore: nil,
                confidenceLevel: .low, activeSeconds: 0
            )
            return DaySummaryViewModel(stub)
        }
        return stubs + existing
    }
}
