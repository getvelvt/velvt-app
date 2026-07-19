import Foundation

// MARK: - InsightViewModel

/// Holds the formatted, ready-to-display properties of the latest insight push.
///
/// Starts in a loading state; transitions to populated on the first call to
/// `update(from:)`. Has no knowledge of IPC internals.
@MainActor
public final class InsightViewModel: ObservableObject {

    @Published public private(set) var text: String = ""
    @Published public private(set) var observation: String = ""
    @Published public private(set) var baselineComparison: String = ""
    @Published public private(set) var suggestedAction: String = ""
    @Published public private(set) var suggestedActionButtonLabel: String = ""
    @Published public private(set) var suggestedActionMinutes: Int = 0
    @Published public private(set) var evidenceSummary: String = ""
    @Published public private(set) var emotionalStage: EmotionalStage = .early
    /// "Today", "Yesterday", or a long-form weekday date.
    @Published public private(set) var date: String = ""
    /// "early data" | "moderate" | "high"
    @Published public private(set) var confidenceLabel: String = ""
    /// Muted monospace timestamp: "Generated HH:mm"
    @Published public private(set) var generatedAt: String = ""
    @Published public private(set) var isLoading: Bool = true
    @Published public private(set) var sourceDate: String = ""

    public init() {}

    public func isForLocalDate(_ localDate: String) -> Bool {
        !isLoading && sourceDate == localDate
    }

    public func update(from payload: InsightPayload) {
        sourceDate = payload.date
        text = payload.text
        observation = payload.evidence.observation
        baselineComparison = payload.evidence.comparison
        suggestedAction = payload.evidence.suggestedAction
        suggestedActionMinutes = payload.evidence.actionMinutes
        suggestedActionButtonLabel = payload.evidence.actionMinutes > 0
            ? "Protect my next \(payload.evidence.actionMinutes) minutes"
            : ""
        evidenceSummary = Self.evidenceSummary(payload.evidence)
        emotionalStage = payload.evidence.toneStage
        date = Self.formatDate(payload.date)
        confidenceLabel = Self.confidenceLabel(for: payload.confidenceLevel, isLow: payload.lowConfidence)
        generatedAt = Self.formatGeneratedAt(payload.generatedAt)
        isLoading = false
    }

    public func reset() {
        sourceDate = ""
        text = ""
        observation = ""
        baselineComparison = ""
        suggestedAction = ""
        suggestedActionButtonLabel = ""
        suggestedActionMinutes = 0
        evidenceSummary = ""
        emotionalStage = .early
        date = ""
        confidenceLabel = ""
        generatedAt = ""
        isLoading = true
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

    static func evidenceSummary(_ evidence: InsightEvidence) -> String {
        guard evidence.observationType != "unavailable" else {
            return "Evidence is unavailable for this older insight."
        }
        let categories = evidence.safeCategories
            .map { $0.replacingOccurrences(of: "_", with: " ") }
            .joined(separator: ", ")
        let measured = "Measured \(evidence.metricValue) \(evidence.metricUnit)"
        let scope = categories.isEmpty ? "" : " across \(categories)"
        let confidence = "\(Int((evidence.coverage * 100).rounded()))% classified coverage"
        return "\(measured)\(scope); \(confidence). No app names, titles, URLs, or local labels were used."
    }
}
