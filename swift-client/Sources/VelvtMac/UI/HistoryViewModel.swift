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
    public let focusedTime: String
    public let activeSeconds: Int
    public let focusedSeconds: Int
    public let meaningfulSwitchCount: Int
    public let longestUninterrupted: String
    public let focusScore: Int?        // nil when isNoData
    public let fragmentationScore: Int?
    public let eventCount: Int
    public let confidenceLabel: String
    public let baselineStatus: String
    public let baselineComparison: BaselineComparison?
    public let typeProportions: [ActivityProportion]
    public let isNoData: Bool

    public init(_ summary: DailySummary) {
        id = summary.date
        isNoData = summary.status == .noData
        date = DaySummaryViewModel.formatDate(summary.date)
        statusLabel = isNoData ? "no data" : "ready"
        activeTime = isNoData ? "—" : DaySummaryViewModel.formatActiveTime(summary.activeSeconds)
        focusedTime = isNoData ? "—" : DaySummaryViewModel.formatActiveTime(summary.focusedSeconds)
        activeSeconds = isNoData ? 0 : summary.activeSeconds
        focusedSeconds = isNoData ? 0 : summary.focusedSeconds
        meaningfulSwitchCount = isNoData ? 0 : summary.meaningfulSwitchCount
        longestUninterrupted = isNoData
            ? "—"
            : DaySummaryViewModel.formatActiveTime(summary.longestUninterruptedSeconds)
        focusScore = isNoData ? nil : summary.focusScore.map { Int($0.rounded()) }
        fragmentationScore = isNoData ? nil : summary.fragmentationScore.map { Int($0.rounded()) }
        eventCount = summary.eventCount
        confidenceLabel = switch summary.confidenceLevel {
        case .high: "high"
        case .medium: "medium"
        case .low: "early"
        case .none: "none"
        }
        baselineStatus = summary.baselineStatus
        baselineComparison = summary.baselineComparison
        typeProportions = summary.typeProportions
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

    public var latestReadyDay: DaySummaryViewModel? {
        days.last { !$0.isNoData }
    }

    public var todayReadyDay: DaySummaryViewModel? {
        readyDay(for: Self.localDateString())
    }

    public func readyDay(for localDate: String) -> DaySummaryViewModel? {
        days.first { $0.id == localDate && !$0.isNoData }
    }

    public var baselineProgress: BaselineProgress {
        let collected = days.filter { !$0.isNoData }
        return BaselineProgress(
            collectedDays: collected.count,
            maturityStatus: collected.last?.baselineStatus
        )
    }

    public var progressiveInsight: ProgressiveInsight? {
        ProgressiveInsight.make(from: days)
    }

    public func update(from payload: HistoryPayload) {
        let mapped = payload.summaries.map(DaySummaryViewModel.init)
        days = HistoryViewModel.padded(mapped, toCount: payload.days)
        isLoading = false
    }

    public func reset() {
        days = []
        isLoading = true
        scrollTarget = nil
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

    nonisolated static func localDateString(
        now: Date = Date(),
        timeZone: TimeZone = .current
    ) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let components = calendar.dateComponents([.year, .month, .day], from: now)
        return String(
            format: "%04d-%02d-%02d",
            components.year ?? 0,
            components.month ?? 0,
            components.day ?? 0
        )
    }
}

public enum ProgressiveInsightTier: Equatable, Sendable {
    case todaySoFar
    case thisWeekSoFar
    case weekOverWeek

    public var label: String {
        switch self {
        case .todaySoFar: "Today so far"
        case .thisWeekSoFar: "This week so far"
        case .weekOverWeek: "Week-over-week coaching"
        }
    }
}

public struct ProgressiveInsight: Equatable, Sendable {
    public let tier: ProgressiveInsightTier
    public let observation: String
    public let comparison: String
    public let suggestedAction: String
    public let evidenceSummary: String
    public let confidenceSummary: String
    public let recentObservedDays: Int
    public let priorObservedDays: Int

    static func make(from days: [DaySummaryViewModel]) -> ProgressiveInsight? {
        let recentWindow = Array(days.suffix(7))
        let recent = recentWindow.filter { !$0.isNoData }
        guard !recent.isEmpty else { return nil }
        let prior = days.count >= 14
            ? Array(days.suffix(14).prefix(7).filter { !$0.isNoData })
            : []

        if recent.count >= 4, prior.count >= 4,
            recent.reduce(0, { $0 + $1.activeSeconds }) > 0,
            prior.reduce(0, { $0 + $1.activeSeconds }) > 0
        {
            return weekOverWeek(recent: recent, prior: prior)
        }
        if recent.count == 1 {
            return todaySoFar(day: recent[0], priorObservedDays: prior.count)
        }
        return thisWeekSoFar(days: recent, priorObservedDays: prior.count)
    }

    private static func weekOverWeek(
        recent: [DaySummaryViewModel],
        prior: [DaySummaryViewModel]
    ) -> ProgressiveInsight {
        let priorActive = prior.reduce(0) { $0 + $1.activeSeconds }
        let recentActive = recent.reduce(0) { $0 + $1.activeSeconds }
        let priorFocusShare =
            Double(prior.reduce(0) { $0 + $1.focusedSeconds }) / Double(priorActive)
        let recentFocusShare =
            Double(recent.reduce(0) { $0 + $1.focusedSeconds }) / Double(recentActive)
        let focusDeltaPoints = Int(((recentFocusShare - priorFocusShare) * 100).rounded())
        let priorSwitchRate =
            Double(prior.reduce(0) { $0 + $1.meaningfulSwitchCount })
            / (Double(priorActive) / 3_600)
        let recentSwitchRate =
            Double(recent.reduce(0) { $0 + $1.meaningfulSwitchCount })
            / (Double(recentActive) / 3_600)
        let focusDirection =
            focusDeltaPoints == 0
            ? "unchanged"
            : "\(abs(focusDeltaPoints)) points \(focusDeltaPoints > 0 ? "higher" : "lower")"
        let switchDirection: String
        let switchDelta = recentSwitchRate - priorSwitchRate
        if abs(switchDelta) < 0.05 {
            switchDirection = "about the same"
        } else {
            switchDirection =
                String(format: "%.1f per hour %@", abs(switchDelta), switchDelta < 0 ? "lower" : "higher")
        }
        let suggestedAction =
            focusDeltaPoints >= 0 && switchDelta <= 0
            ? "Repeat one work block from this week that felt sustainable."
            : "Protect one 20-minute lane next week, then check whether switching settles."

        return ProgressiveInsight(
            tier: .weekOverWeek,
            observation:
                "Uninterrupted focus represented \(Int((recentFocusShare * 100).rounded()))% of observed active time this week.",
            comparison:
                "That share was \(focusDirection) than last week; meaningful switching was \(switchDirection).",
            suggestedAction: suggestedAction,
            evidenceSummary:
                "\(recent.count)/7 recent days compared with \(prior.count)/7 prior days",
            confidenceSummary: confidenceSummary(for: recent + prior),
            recentObservedDays: recent.count,
            priorObservedDays: prior.count
        )
    }

    private static func todaySoFar(
        day: DaySummaryViewModel,
        priorObservedDays: Int
    ) -> ProgressiveInsight {
        let focusShare = share(focused: day.focusedSeconds, active: day.activeSeconds)
        let observation: String
        let comparison: String
        let suggestedAction: String
        if day.activeSeconds == 0 {
            observation = "No qualifying activity has been recorded in this observed day yet."
            comparison = "There is not enough active time for a within-day comparison."
            suggestedAction = "Keep Velvt running during your next work block and check again afterward."
        } else {
            observation =
                "\(Int((focusShare * 100).rounded()))% of \(day.activeTime) observed active time was in focus-oriented work."
            comparison =
                "\(day.meaningfulSwitchCount) meaningful switches were observed; no complete weekly comparison is available yet."
            suggestedAction = day.meaningfulSwitchCount > 4
                ? "Protect one 20-minute lane and see whether switching settles."
                : "Repeat one steady block from today while the context is still fresh."
        }
        return ProgressiveInsight(
            tier: .todaySoFar,
            observation: observation,
            comparison: comparison,
            suggestedAction: suggestedAction,
            evidenceSummary: "1/7 observed days · current day may be incomplete",
            confidenceSummary: confidenceSummary(for: [day]),
            recentObservedDays: 1,
            priorObservedDays: priorObservedDays
        )
    }

    private static func thisWeekSoFar(
        days: [DaySummaryViewModel],
        priorObservedDays: Int
    ) -> ProgressiveInsight {
        let activeDays = days.filter { $0.activeSeconds > 0 }
        let active = activeDays.reduce(0) { $0 + $1.activeSeconds }
        let focused = activeDays.reduce(0) { $0 + $1.focusedSeconds }
        let observation: String
        let comparison: String
        let suggestedAction: String
        if active == 0 {
            observation = "No qualifying activity was recorded across the available days."
            comparison =
                "These \(days.count) observed days are not being treated as a complete week."
            suggestedAction = "Keep Velvt running during one normal work block to build useful coverage."
        } else {
            let shares = activeDays.map { share(focused: $0.focusedSeconds, active: $0.activeSeconds) }
            let averageShare = share(focused: focused, active: active)
            let range = ((shares.max() ?? 0) - (shares.min() ?? 0)) * 100
            observation =
                "Focus-oriented work represented \(Int((averageShare * 100).rounded()))% of observed active time across \(days.count) days."
            comparison = shares.count > 1
                ? "Available days varied by \(Int(range.rounded())) focus-share points; this is a partial-window comparison, not week over week."
                : "Only one active day is available inside this partial window."
            suggestedAction = range >= 15
                ? "Repeat one condition from the steadier day during your next 20-minute block."
                : "Protect one 20-minute lane and keep building comparable days."
        }
        return ProgressiveInsight(
            tier: .thisWeekSoFar,
            observation: observation,
            comparison: comparison,
            suggestedAction: suggestedAction,
            evidenceSummary:
                "\(days.count)/7 current days observed · \(priorObservedDays)/7 prior days · partial window",
            confidenceSummary: confidenceSummary(for: days),
            recentObservedDays: days.count,
            priorObservedDays: priorObservedDays
        )
    }

    private static func share(focused: Int, active: Int) -> Double {
        guard active > 0 else { return 0 }
        return Double(focused) / Double(active)
    }

    private static func confidenceSummary(for days: [DaySummaryViewModel]) -> String {
        let labels = Set(days.map(\.confidenceLabel))
        if labels == ["high"] { return "High confidence" }
        if labels.contains("none") || labels.contains("early") { return "Early confidence" }
        return "Moderate confidence"
    }
}

public struct BaselineProgress: Equatable, Sendable {
    public static let targetDays = 14
    public let collectedDays: Int
    public let maturityStatus: String?

    public init(collectedDays: Int, maturityStatus: String? = nil) {
        self.collectedDays = min(max(collectedDays, 0), Self.targetDays)
        self.maturityStatus = maturityStatus
    }

    public var isComplete: Bool { maturityStatus == "mature" }
    public var label: String {
        switch maturityStatus {
        case "mature":
            "Your personal baseline is ready"
        case "emerging":
            "Your personal baseline is becoming more reliable"
        case "provisional":
            "Learning from your recent sessions"
        default:
            "Collecting a neutral baseline — \(collectedDays) observed day\(collectedDays == 1 ? "" : "s")"
        }
    }
}
