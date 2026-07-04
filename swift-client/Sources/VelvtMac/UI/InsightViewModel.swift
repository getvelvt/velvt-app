import Foundation

// MARK: - InsightViewModel

/// Holds the formatted, ready-to-display properties of the latest insight push.
///
/// Starts in a loading state; transitions to populated on the first call to
/// `update(from:)`. Has no knowledge of IPC internals.
@MainActor
public final class InsightViewModel: ObservableObject {

    @Published public private(set) var text: String = ""
    /// "Today", "Yesterday", or a long-form weekday date.
    @Published public private(set) var date: String = ""
    /// "early data" | "moderate" | "high"
    @Published public private(set) var confidenceLabel: String = ""
    /// Muted monospace timestamp: "Generated HH:mm"
    @Published public private(set) var generatedAt: String = ""
    @Published public private(set) var isLoading: Bool = true

    public init() {}

    public func update(from payload: InsightPayload) {
        text = payload.text
        date = Self.formatDate(payload.date)
        confidenceLabel = Self.confidenceLabel(for: payload.confidenceLevel, isLow: payload.lowConfidence)
        generatedAt = Self.formatGeneratedAt(payload.generatedAt)
        isLoading = false
    }

    // MARK: - Formatting (internal for testability)

    static func formatDate(_ dateString: String) -> String {
        let parser = DateFormatter()
        parser.dateFormat = "yyyy-MM-dd"
        parser.locale = Locale(identifier: "en_US_POSIX")
        guard let date = parser.date(from: dateString) else { return dateString }
        let cal = Calendar.current
        if cal.isDateInToday(date) { return "Today" }
        if cal.isDateInYesterday(date) { return "Yesterday" }
        let display = DateFormatter()
        display.dateFormat = "EEEE, d MMMM"
        return display.string(from: date)
    }

    static func confidenceLabel(for level: ConfidenceLevel, isLow: Bool) -> String {
        if isLow { return "early data" }
        switch level {
        case .none:   return "not available"
        case .low:    return "early data"
        case .medium: return "moderate"
        case .high:   return "high"
        }
    }

    static func formatGeneratedAt(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return "Generated \(formatter.string(from: date))"
    }
}
