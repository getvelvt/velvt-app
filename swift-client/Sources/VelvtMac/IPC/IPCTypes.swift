import Foundation

/// Messages sent from the macOS client to the Rust service.
public enum ClientMessage: Codable, Equatable, Sendable {
    case clientHello(ClientHello)
    case rawEvent(RawEventMessage)
    case errorResponse(ErrorResponse)
    case requestLatestInsight(RequestLatestInsight)
    case requestLatestHistory(RequestLatestHistory)
    // Auth messages (proto v6)
    case signUp(SignUpRequest)
    case logIn(LogInRequest)
    case authSession(AuthSession)
    case logOut
    case deleteAccount
    case requestMenuStatus
    case flushUploadQueue
    case correctEventClassification(CorrectEventClassification)

    public init(from decoder: Decoder) throws {
        let envelope = try decoder.container(keyedBy: EnvelopeCodingKeys.self)
        let type = try envelope.decode(String.self, forKey: .type)
        let payload = try envelope.superDecoder(forKey: .payload)
        switch type {
        case "client_hello":
            self = .clientHello(try ClientHello(from: payload))
        case "raw_event":
            self = .rawEvent(try RawEventMessage(from: payload))
        case "error_response":
            self = .errorResponse(try ErrorResponse(from: payload))
        case "request_latest_insight":
            self = .requestLatestInsight(try RequestLatestInsight(from: payload))
        case "request_latest_history":
            self = .requestLatestHistory(try RequestLatestHistory(from: payload))
        case "sign_up":
            self = .signUp(try SignUpRequest(from: payload))
        case "log_in":
            self = .logIn(try LogInRequest(from: payload))
        case "auth_session":
            self = .authSession(try AuthSession(from: payload))
        case "log_out":
            self = .logOut
        case "delete_account":
            self = .deleteAccount
        case "request_menu_status":
            self = .requestMenuStatus
        case "flush_upload_queue":
            self = .flushUploadQueue
        case "correct_event_classification":
            self = .correctEventClassification(try CorrectEventClassification(from: payload))
        default:
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Unknown client message type"))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var envelope = encoder.container(keyedBy: EnvelopeCodingKeys.self)
        switch self {
        case let .clientHello(value):
            try envelope.encode("client_hello", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .rawEvent(value):
            try envelope.encode("raw_event", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .errorResponse(value):
            try envelope.encode("error_response", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .requestLatestInsight(value):
            try envelope.encode("request_latest_insight", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .requestLatestHistory(value):
            try envelope.encode("request_latest_history", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .signUp(value):
            try envelope.encode("sign_up", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .logIn(value):
            try envelope.encode("log_in", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .authSession(value):
            try envelope.encode("auth_session", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case .logOut:
            try envelope.encode("log_out", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case .deleteAccount:
            try envelope.encode("delete_account", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case .requestMenuStatus:
            try envelope.encode("request_menu_status", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case .flushUploadQueue:
            try envelope.encode("flush_upload_queue", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case let .correctEventClassification(value):
            try envelope.encode("correct_event_classification", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        }
    }
}

/// Messages sent from the Rust service to the macOS client.
public enum ServerMessage: Codable, Equatable, Sendable {
    case serverHello(ServerHello)
    case acknowledged(Acknowledged)
    case versionMismatch(VersionMismatch)
    case malformedMessage(MalformedMessage)
    case rawEventAck(RawEventAcknowledgement)
    case insightPayload(InsightPayload)
    case historyPayload(HistoryPayload)
    case serviceStatus(ServiceStatus)
    case privacyViolationAlert(PrivacyViolationAlert)
    case errorResponse(ErrorResponse)
    case cacheEmpty(CacheEmpty)
    /// Sent by the Rust service before a graceful shutdown.
    case shuttingDown(ShuttingDown)
    // Auth messages (proto v6)
    case authSuccess(AuthSuccess)
    case authSessionUpdated(AuthSession)
    case authFailure(AuthFailure)
    case accountDeletionAccepted
    case needsReauth(NeedsReauth)
    case deviceRevoked(DeviceRevoked)
    // Notification push (S7)
    case notificationPayload(NotificationPayload)
    case menuStatus(MenuStatus)
    /// Extension point for a future server discriminator. Unknown payload fields
    /// are deliberately discarded so handlers do not require exhaustive updates.
    case unknown(type: String)

    public init(from decoder: Decoder) throws {
        let envelope = try decoder.container(keyedBy: EnvelopeCodingKeys.self)
        let type = try envelope.decode(String.self, forKey: .type)
        let payload = try envelope.superDecoder(forKey: .payload)
        switch type {
        case "server_hello":
            self = .serverHello(try ServerHello(from: payload))
        case "acknowledged":
            self = .acknowledged(try Acknowledged(from: payload))
        case "version_mismatch":
            self = .versionMismatch(try VersionMismatch(from: payload))
        case "malformed_message":
            self = .malformedMessage(try MalformedMessage(from: payload))
        case "raw_event_ack":
            self = .rawEventAck(try RawEventAcknowledgement(from: payload))
        case "insight_payload":
            self = .insightPayload(try InsightPayload(from: payload))
        case "history_payload":
            self = .historyPayload(try HistoryPayload(from: payload))
        case "service_status":
            self = .serviceStatus(try ServiceStatus(from: payload))
        case "privacy_violation_alert":
            self = .privacyViolationAlert(try PrivacyViolationAlert(from: payload))
        case "error_response":
            self = .errorResponse(try ErrorResponse(from: payload))
        case "cache_empty":
            self = .cacheEmpty(try CacheEmpty(from: payload))
        case "shutting_down":
            self = .shuttingDown(try ShuttingDown(from: payload))
        case "auth_success":
            self = .authSuccess(try AuthSuccess(from: payload))
        case "auth_session_updated":
            self = .authSessionUpdated(try AuthSession(from: payload))
        case "auth_failure":
            self = .authFailure(try AuthFailure(from: payload))
        case "account_deletion_accepted":
            self = .accountDeletionAccepted
        case "needs_reauth":
            self = .needsReauth(try NeedsReauth(from: payload))
        case "device_revoked":
            self = .deviceRevoked(try DeviceRevoked(from: payload))
        case "notification_payload":
            self = .notificationPayload(try NotificationPayload(from: payload))
        case "menu_status":
            self = .menuStatus(try MenuStatus(from: payload))
        default:
            self = .unknown(type: type)
        }
    }

    public func encode(to encoder: Encoder) throws {
        var envelope = encoder.container(keyedBy: EnvelopeCodingKeys.self)
        switch self {
        case let .serverHello(value):
            try envelope.encode("server_hello", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .acknowledged(value):
            try envelope.encode("acknowledged", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .versionMismatch(value):
            try envelope.encode("version_mismatch", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .malformedMessage(value):
            try envelope.encode("malformed_message", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .rawEventAck(value):
            try envelope.encode("raw_event_ack", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .insightPayload(value):
            try envelope.encode("insight_payload", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .historyPayload(value):
            try envelope.encode("history_payload", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .serviceStatus(value):
            try envelope.encode("service_status", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .privacyViolationAlert(value):
            try envelope.encode("privacy_violation_alert", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .errorResponse(value):
            try envelope.encode("error_response", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .cacheEmpty(value):
            try envelope.encode("cache_empty", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .shuttingDown(value):
            try envelope.encode("shutting_down", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .authSuccess(value):
            try envelope.encode("auth_success", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .authSessionUpdated(value):
            try envelope.encode("auth_session_updated", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .authFailure(value):
            try envelope.encode("auth_failure", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case .accountDeletionAccepted:
            try envelope.encode("account_deletion_accepted", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case let .needsReauth(value):
            try envelope.encode("needs_reauth", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .deviceRevoked(value):
            try envelope.encode("device_revoked", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .notificationPayload(value):
            try envelope.encode("notification_payload", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .menuStatus(value):
            try envelope.encode("menu_status", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .unknown(type):
            try envelope.encode(type, forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        }
    }
}

private enum EnvelopeCodingKeys: String, CodingKey { case type, payload }
private struct EmptyPayload: Codable {}

public struct RequestLatestInsight: Codable, Equatable, Sendable {
    public let date: String

    public init(date: String) {
        self.date = date
    }
}

public struct RequestLatestHistory: Codable, Equatable, Sendable {
    public let days: Int

    public init(days: Int) {
        self.days = days
    }
}

public struct CacheEmpty: Codable, Equatable, Sendable {
    public let payloadType: String

    public init(payloadType: String) {
        self.payloadType = payloadType
    }

    private enum CodingKeys: String, CodingKey {
        case payloadType = "payload_type"
    }
}

/// Announces the server protocol version after a socket connection opens.
public struct ServerHello: Codable, Equatable, Sendable {
    public let protocolVersion: Int

    public init(protocolVersion: Int) {
        self.protocolVersion = protocolVersion
    }

    private enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version" }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try container.decode(Int.self, forKey: .protocolVersion)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(protocolVersion, forKey: .protocolVersion)
    }
}

/// Declares the client protocol and application versions.
public struct ClientHello: Codable, Equatable, Sendable {
    public let expectedProtocolVersion: Int
    public let clientVersion: String

    public init(expectedProtocolVersion: Int, clientVersion: String) {
        self.expectedProtocolVersion = expectedProtocolVersion
        self.clientVersion = clientVersion
    }

    private enum CodingKeys: String, CodingKey {
        case expectedProtocolVersion = "expected_protocol_version"
        case clientVersion = "client_version"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        expectedProtocolVersion = try container.decode(Int.self, forKey: .expectedProtocolVersion)
        clientVersion = try container.decode(String.self, forKey: .clientVersion)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(expectedProtocolVersion, forKey: .expectedProtocolVersion)
        try container.encode(clientVersion, forKey: .clientVersion)
    }
}

/// Confirms that the client and server protocol versions match.
public struct Acknowledged: Codable, Equatable, Sendable {
    public init() {}
}

/// Reports incompatible client and server protocol versions.
public struct VersionMismatch: Codable, Equatable, Sendable {
    public let serverProtocolVersion: Int
    public let clientProtocolVersion: Int

    public init(serverProtocolVersion: Int, clientProtocolVersion: Int) {
        self.serverProtocolVersion = serverProtocolVersion
        self.clientProtocolVersion = clientProtocolVersion
    }

    private enum CodingKeys: String, CodingKey {
        case serverProtocolVersion = "server_protocol_version"
        case clientProtocolVersion = "client_protocol_version"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        serverProtocolVersion = try container.decode(Int.self, forKey: .serverProtocolVersion)
        clientProtocolVersion = try container.decode(Int.self, forKey: .clientProtocolVersion)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(serverProtocolVersion, forKey: .serverProtocolVersion)
        try container.encode(clientProtocolVersion, forKey: .clientProtocolVersion)
    }
}

/// A privacy-safe reason code for rejecting an invalid IPC frame.
public enum MalformedMessageCode: String, Codable, Equatable, Sendable {
    case invalidMessage = "invalid_message"
}

/// Reports an invalid IPC frame without retaining or echoing its contents.
public struct MalformedMessage: Codable, Equatable, Sendable {
    public let code: MalformedMessageCode

    public init(code: MalformedMessageCode) {
        self.code = code
    }
}

/// A local-only raw activity event sent to the Rust privacy boundary.
public struct RawEventMessage: Codable, Equatable, Sendable {
    public let eventID: UUID
    public let occurredAt: Date
    public let durationSeconds: Int
    public let appName: String
    public let windowTitle: String
    public let bundleID: String?

    public init(
        eventID: UUID,
        occurredAt: Date,
        durationSeconds: Int = 0,
        appName: String,
        windowTitle: String,
        bundleID: String?
    ) {
        self.eventID = eventID
        self.occurredAt = occurredAt
        self.durationSeconds = durationSeconds
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleID = bundleID
    }

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case occurredAt = "occurred_at"
        case durationSeconds = "duration_seconds"
        case appName = "app_name"
        case windowTitle = "window_title"
        case bundleID = "bundle_id"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        eventID = try container.decode(UUID.self, forKey: .eventID)
        occurredAt = try container.decode(Date.self, forKey: .occurredAt)
        durationSeconds = try container.decode(Int.self, forKey: .durationSeconds)
        appName = try container.decode(String.self, forKey: .appName)
        windowTitle = try container.decode(String.self, forKey: .windowTitle)
        bundleID = try container.decodeIfPresent(String.self, forKey: .bundleID)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(eventID, forKey: .eventID)
        try container.encode(occurredAt, forKey: .occurredAt)
        try container.encode(durationSeconds, forKey: .durationSeconds)
        try container.encode(appName, forKey: .appName)
        try container.encode(windowTitle, forKey: .windowTitle)
        try container.encode(bundleID, forKey: .bundleID)
    }
}

/// The outcome of accepting a raw event.
public enum RawEventAcknowledgementStatus: String, Codable, Equatable, Sendable {
    case accepted
    case dropped
}

/// Acknowledges receipt of a raw event without echoing raw fields.
public struct RawEventAcknowledgement: Codable, Equatable, Sendable {
    public let eventID: UUID
    public let status: RawEventAcknowledgementStatus
    public let dropReason: String?

    public init(eventID: UUID, status: RawEventAcknowledgementStatus, dropReason: String?) {
        self.eventID = eventID
        self.status = status
        self.dropReason = dropReason
    }

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case status
        case dropReason = "drop_reason"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        eventID = try container.decode(UUID.self, forKey: .eventID)
        status = try container.decode(RawEventAcknowledgementStatus.self, forKey: .status)
        dropReason = try container.decodeIfPresent(String.self, forKey: .dropReason)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(eventID, forKey: .eventID)
        try container.encode(status, forKey: .status)
        try container.encodeIfPresent(dropReason, forKey: .dropReason)
    }
}

/// Confidence classification supplied by the Rust service.
public enum ConfidenceLevel: String, Codable, Equatable, Sendable {
    case none
    case low
    case medium
    case high
}

/// A ready-to-display daily insight.
public struct InsightPayload: Codable, Equatable, Sendable {
    public let date: String
    public let text: String
    public let confidenceLevel: ConfidenceLevel
    public let lowConfidence: Bool
    public let generatedAt: Date

    public init(date: String, text: String, confidenceLevel: ConfidenceLevel, lowConfidence: Bool, generatedAt: Date) {
        self.date = date
        self.text = text
        self.confidenceLevel = confidenceLevel
        self.lowConfidence = lowConfidence
        self.generatedAt = generatedAt
    }

    private enum CodingKeys: String, CodingKey {
        case date
        case text
        case confidenceLevel = "confidence_level"
        case lowConfidence = "low_confidence"
        case generatedAt = "generated_at"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        date = try container.decode(String.self, forKey: .date)
        text = try container.decode(String.self, forKey: .text)
        confidenceLevel = try container.decode(ConfidenceLevel.self, forKey: .confidenceLevel)
        lowConfidence = try container.decode(Bool.self, forKey: .lowConfidence)
        generatedAt = try container.decode(Date.self, forKey: .generatedAt)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(date, forKey: .date)
        try container.encode(text, forKey: .text)
        try container.encode(confidenceLevel, forKey: .confidenceLevel)
        try container.encode(lowConfidence, forKey: .lowConfidence)
        try container.encode(generatedAt, forKey: .generatedAt)
    }
}

/// Availability of a daily history summary.
public enum HistoryStatus: String, Codable, Equatable, Sendable {
    case ready
    case noData = "no_data"
}

/// One privacy-safe daily history summary.
public struct DailySummary: Codable, Equatable, Sendable {
    public let date: String
    public let status: HistoryStatus
    public let eventCount: Int
    public let focusScore: Double?
    public let fragmentationScore: Double?
    public let confidenceLevel: ConfidenceLevel
    public let activeSeconds: Int
    public let baselineStatus: String
    public let baselineComparison: BaselineComparison?
    public let typeProportions: [ActivityProportion]

    private enum CodingKeys: String, CodingKey {
        case date
        case status
        case eventCount = "event_count"
        case focusScore = "focus_score"
        case fragmentationScore = "fragmentation_score"
        case confidenceLevel = "confidence_level"
        case activeSeconds = "active_seconds"
        case baselineStatus = "baseline_status"
        case baselineComparison = "baseline_comparison"
        case typeProportions = "type_proportions"
    }

    public init(
        date: String,
        status: HistoryStatus,
        eventCount: Int,
        focusScore: Double?,
        fragmentationScore: Double?,
        confidenceLevel: ConfidenceLevel,
        activeSeconds: Int,
        baselineStatus: String = "unknown",
        baselineComparison: BaselineComparison? = nil,
        typeProportions: [ActivityProportion] = []
    ) {
        self.date = date
        self.status = status
        self.eventCount = eventCount
        self.focusScore = focusScore
        self.fragmentationScore = fragmentationScore
        self.confidenceLevel = confidenceLevel
        self.activeSeconds = activeSeconds
        self.baselineStatus = baselineStatus
        self.baselineComparison = baselineComparison
        self.typeProportions = typeProportions
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        date = try container.decode(String.self, forKey: .date)
        status = try container.decode(HistoryStatus.self, forKey: .status)
        eventCount = try container.decode(Int.self, forKey: .eventCount)
        focusScore = try container.decodeIfPresent(Double.self, forKey: .focusScore)
        fragmentationScore = try container.decodeIfPresent(Double.self, forKey: .fragmentationScore)
        confidenceLevel = try container.decode(ConfidenceLevel.self, forKey: .confidenceLevel)
        activeSeconds = try container.decode(Int.self, forKey: .activeSeconds)
        baselineStatus = try container.decodeIfPresent(String.self, forKey: .baselineStatus) ?? "unknown"
        baselineComparison = try container.decodeIfPresent(BaselineComparison.self, forKey: .baselineComparison)
        typeProportions = try container.decodeIfPresent([ActivityProportion].self, forKey: .typeProportions) ?? []
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(date, forKey: .date)
        try container.encode(status, forKey: .status)
        try container.encode(eventCount, forKey: .eventCount)
        try container.encodeIfPresent(focusScore, forKey: .focusScore)
        try container.encodeIfPresent(fragmentationScore, forKey: .fragmentationScore)
        try container.encode(confidenceLevel, forKey: .confidenceLevel)
        try container.encode(activeSeconds, forKey: .activeSeconds)
        try container.encode(baselineStatus, forKey: .baselineStatus)
        try container.encodeIfPresent(baselineComparison, forKey: .baselineComparison)
        try container.encode(typeProportions, forKey: .typeProportions)
    }
}

public struct ActivityProportion: Codable, Equatable, Sendable, Identifiable {
    public let category: String
    public let seconds: Int
    public let proportion: Double

    public var id: String { category }

    public init(category: String, seconds: Int, proportion: Double) {
        self.category = category
        self.seconds = seconds
        self.proportion = proportion
    }
}

public struct BaselineComparison: Codable, Equatable, Sendable {
    public let status: String?
    public let message: String?
    public let fragmentationDelta: Double?
    public let focusDelta: Double?
    public let activeSecondsDelta: Int?

    public init(
        status: String? = nil,
        message: String? = nil,
        fragmentationDelta: Double? = nil,
        focusDelta: Double? = nil,
        activeSecondsDelta: Int? = nil
    ) {
        self.status = status
        self.message = message
        self.fragmentationDelta = fragmentationDelta
        self.focusDelta = focusDelta
        self.activeSecondsDelta = activeSecondsDelta
    }

    private enum CodingKeys: String, CodingKey {
        case status
        case message
        case fragmentationDelta = "fragmentation_delta"
        case focusDelta = "focus_delta"
        case activeSecondsDelta = "active_seconds_delta"
    }
}

/// A ready-to-display multi-day history payload.
public struct HistoryPayload: Codable, Equatable, Sendable {
    public let days: Int
    public let summaries: [DailySummary]

    public init(days: Int, summaries: [DailySummary]) {
        self.days = days
        self.summaries = summaries
    }

    private enum CodingKeys: String, CodingKey { case days, summaries }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        days = try container.decode(Int.self, forKey: .days)
        summaries = try container.decode([DailySummary].self, forKey: .summaries)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(days, forKey: .days)
        try container.encode(summaries, forKey: .summaries)
    }
}

/// Health state reported by the Rust service.
public enum ServiceState: String, Codable, Equatable, Sendable {
    case ready
    case degraded
    case uploadPaused = "upload_paused"
    case authRequired = "auth_required"
}

/// A privacy-safe Rust service health update.
public struct ServiceStatus: Codable, Equatable, Sendable {
    public let state: ServiceState
    public let reason: String?

    public init(state: ServiceState, reason: String?) {
        self.state = state
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey { case state, reason }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        state = try container.decode(ServiceState.self, forKey: .state)
        reason = try container.decodeIfPresent(String.self, forKey: .reason)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(state, forKey: .state)
        try container.encodeIfPresent(reason, forKey: .reason)
    }
}

/// Privacy-safe queued event metadata for the menu-bar settings UI.
public struct QueuedEventSummary: Codable, Equatable, Sendable, Identifiable {
    public let eventID: UUID
    public let stableID: String
    public let label: String
    public let localLabel: String?
    public let category: String
    public let classificationTier: String
    public let occurredAt: Date

    public var id: UUID { eventID }

    private enum CodingKeys: String, CodingKey {
        case label, category
        case eventID = "event_id"
        case stableID = "stable_id"
        case localLabel = "local_label"
        case classificationTier = "classification_tier"
        case occurredAt = "occurred_at"
    }
}

public struct CorrectEventClassification: Codable, Equatable, Sendable {
    public let eventID: UUID
    public let stableID: String
    public let category: String

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case stableID = "stable_id"
        case category
    }

    public init(eventID: UUID, stableID: String, category: String) {
        self.eventID = eventID
        self.stableID = stableID
        self.category = category
    }
}

/// Snapshot returned by the Rust privacy boundary for the settings popover.
public struct MenuStatus: Codable, Equatable, Sendable {
    public let deviceID: String?
    public let cloudReady: Bool
    public let uploadStatus: String
    public let lastUploadErrorCode: String?
    public let nextUploadAttemptAt: Date?
    public let pendingUploadBatchCount: Int
    public let failedUploadBatchCount: Int
    public let rejectedUploadBatchCount: Int
    public let queuedEventCount: Int
    public let queuedEvents: [QueuedEventSummary]

    private enum CodingKeys: String, CodingKey {
        case deviceID = "device_id"
        case cloudReady = "cloud_ready"
        case uploadStatus = "upload_status"
        case lastUploadErrorCode = "last_upload_error_code"
        case nextUploadAttemptAt = "next_upload_attempt_at"
        case pendingUploadBatchCount = "pending_upload_batch_count"
        case failedUploadBatchCount = "failed_upload_batch_count"
        case rejectedUploadBatchCount = "rejected_upload_batch_count"
        case queuedEventCount = "queued_event_count"
        case queuedEvents = "queued_events"
    }

    public init(
        deviceID: String?,
        cloudReady: Bool,
        uploadStatus: String,
        lastUploadErrorCode: String?,
        nextUploadAttemptAt: Date?,
        pendingUploadBatchCount: Int,
        failedUploadBatchCount: Int,
        rejectedUploadBatchCount: Int,
        queuedEventCount: Int,
        queuedEvents: [QueuedEventSummary]
    ) {
        self.deviceID = deviceID
        self.cloudReady = cloudReady
        self.uploadStatus = uploadStatus
        self.lastUploadErrorCode = lastUploadErrorCode
        self.nextUploadAttemptAt = nextUploadAttemptAt
        self.pendingUploadBatchCount = pendingUploadBatchCount
        self.failedUploadBatchCount = failedUploadBatchCount
        self.rejectedUploadBatchCount = rejectedUploadBatchCount
        self.queuedEventCount = queuedEventCount
        self.queuedEvents = queuedEvents
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        deviceID = try container.decodeIfPresent(String.self, forKey: .deviceID)
        cloudReady = try container.decode(Bool.self, forKey: .cloudReady)
        uploadStatus = try container.decodeIfPresent(String.self, forKey: .uploadStatus) ?? (cloudReady ? "ready" : "network_unavailable")
        lastUploadErrorCode = try container.decodeIfPresent(String.self, forKey: .lastUploadErrorCode)
        nextUploadAttemptAt = try container.decodeIfPresent(Date.self, forKey: .nextUploadAttemptAt)
        pendingUploadBatchCount = try container.decodeIfPresent(Int.self, forKey: .pendingUploadBatchCount) ?? 0
        failedUploadBatchCount = try container.decodeIfPresent(Int.self, forKey: .failedUploadBatchCount) ?? 0
        rejectedUploadBatchCount = try container.decodeIfPresent(Int.self, forKey: .rejectedUploadBatchCount) ?? 0
        queuedEventCount = try container.decode(Int.self, forKey: .queuedEventCount)
        queuedEvents = try container.decode([QueuedEventSummary].self, forKey: .queuedEvents)
    }
}

/// A terminal privacy rejection alert containing safe diagnostics only.
public struct PrivacyViolationAlert: Codable, Equatable, Sendable {
    public let code: String
    public let message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }
}

/// A typed error envelope that must contain only privacy-safe text.
public struct ErrorResponse: Codable, Equatable, Sendable {
    public let code: String
    public let message: String
    public let relatedEventID: UUID?

    public init(code: String, message: String, relatedEventID: UUID?) {
        self.code = code
        self.message = message
        self.relatedEventID = relatedEventID
    }

    private enum CodingKeys: String, CodingKey {
        case code
        case message
        case relatedEventID = "related_event_id"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        code = try container.decode(String.self, forKey: .code)
        message = try container.decode(String.self, forKey: .message)
        relatedEventID = try container.decodeIfPresent(UUID.self, forKey: .relatedEventID)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(code, forKey: .code)
        try container.encode(message, forKey: .message)
        try container.encodeIfPresent(relatedEventID, forKey: .relatedEventID)
    }
}

/// Signals that the Rust service is about to shut down.
///
/// Clients should disconnect and reconnect after the service restarts.
/// The `reason` field is `"sigterm"` or `"sigint"`.
public struct ShuttingDown: Codable, Equatable, Sendable {
    public let reason: String

    public init(reason: String) {
        self.reason = reason
    }
}

// MARK: - Auth DTOs (proto v6)

/// Credentials for account creation. Never log, persist outside Keychain, or
/// include in upload payloads.
public struct SignUpRequest: Codable, Equatable, Sendable {
    public let email: String
    public let password: String

    public init(email: String, password: String) {
        self.email = email
        self.password = password
    }
}

/// Credentials for account login. Never log, persist outside Keychain, or
/// include in upload payloads.
public struct LogInRequest: Codable, Equatable, Sendable {
    public let email: String
    public let password: String

    public init(email: String, password: String) {
        self.email = email
        self.password = password
    }
}

/// Portable auth session persisted by the host client. On macOS this lives in
/// Keychain; Rust keeps it in memory only.
public struct AuthSession: Codable, Equatable, Sendable {
    public let deviceId: String
    public let accessToken: String
    public let refreshToken: String
    public let expiresAt: Date
    public let userAccessToken: String?
    public let userRefreshToken: String?
    public let userExpiresAt: Date?

    public init(
        deviceId: String,
        accessToken: String,
        refreshToken: String,
        expiresAt: Date,
        userAccessToken: String? = nil,
        userRefreshToken: String? = nil,
        userExpiresAt: Date? = nil
    ) {
        self.deviceId = deviceId
        self.accessToken = accessToken
        self.refreshToken = refreshToken
        self.expiresAt = expiresAt
        self.userAccessToken = userAccessToken
        self.userRefreshToken = userRefreshToken
        self.userExpiresAt = userExpiresAt
    }

    private enum CodingKeys: String, CodingKey {
        case deviceId = "device_id"
        case accessToken = "access_token"
        case refreshToken = "refresh_token"
        case expiresAt = "expires_at"
        case userAccessToken = "user_access_token"
        case userRefreshToken = "user_refresh_token"
        case userExpiresAt = "user_expires_at"
    }
}

/// Tokens returned on successful authentication. Swift stores these in Keychain
/// only — never in SQLite, logs, or string interpolation.
public struct AuthSuccess: Codable, Equatable, Sendable {
    public let userId: String
    public let deviceId: String
    public let accessToken: String
    public let refreshToken: String
    public let expiresAt: Date
    public let userAccessToken: String?
    public let userRefreshToken: String?
    public let userExpiresAt: Date?

    public init(
        userId: String,
        deviceId: String,
        accessToken: String,
        refreshToken: String,
        expiresAt: Date,
        userAccessToken: String? = nil,
        userRefreshToken: String? = nil,
        userExpiresAt: Date? = nil
    ) {
        self.userId = userId
        self.deviceId = deviceId
        self.accessToken = accessToken
        self.refreshToken = refreshToken
        self.expiresAt = expiresAt
        self.userAccessToken = userAccessToken
        self.userRefreshToken = userRefreshToken
        self.userExpiresAt = userExpiresAt
    }

    private enum CodingKeys: String, CodingKey {
        case userId = "user_id"
        case deviceId = "device_id"
        case accessToken = "access_token"
        case refreshToken = "refresh_token"
        case expiresAt = "expires_at"
        case userAccessToken = "user_access_token"
        case userRefreshToken = "user_refresh_token"
        case userExpiresAt = "user_expires_at"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        userId = try c.decode(String.self, forKey: .userId)
        deviceId = try c.decode(String.self, forKey: .deviceId)
        accessToken = try c.decode(String.self, forKey: .accessToken)
        refreshToken = try c.decode(String.self, forKey: .refreshToken)
        expiresAt = try c.decode(Date.self, forKey: .expiresAt)
        userAccessToken = try c.decodeIfPresent(String.self, forKey: .userAccessToken)
        userRefreshToken = try c.decodeIfPresent(String.self, forKey: .userRefreshToken)
        userExpiresAt = try c.decodeIfPresent(Date.self, forKey: .userExpiresAt)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(userId, forKey: .userId)
        try c.encode(deviceId, forKey: .deviceId)
        try c.encode(accessToken, forKey: .accessToken)
        try c.encode(refreshToken, forKey: .refreshToken)
        try c.encode(expiresAt, forKey: .expiresAt)
        try c.encodeIfPresent(userAccessToken, forKey: .userAccessToken)
        try c.encodeIfPresent(userRefreshToken, forKey: .userRefreshToken)
        try c.encodeIfPresent(userExpiresAt, forKey: .userExpiresAt)
    }
}

/// Machine-readable auth failure reason.
public enum AuthFailureCode: String, Codable, Equatable, Sendable {
    case invalidCredentials = "invalid_credentials"
    case networkError = "network_error"
    case serverError = "server_error"
}

/// Reports a failed authentication attempt. The message field is safe for
/// display and must never echo credentials or tokens.
public struct AuthFailure: Codable, Equatable, Sendable {
    public let code: AuthFailureCode
    public let message: String

    public init(code: AuthFailureCode, message: String) {
        self.code = code
        self.message = message
    }
}

/// Signals that the current session is no longer valid and login is required.
public struct NeedsReauth: Codable, Equatable, Sendable {
    public let reason: String

    public init(reason: String) {
        self.reason = reason
    }
}

/// Signals that this device registration has been permanently revoked.
public struct DeviceRevoked: Codable, Equatable, Sendable {
    public let message: String

    public init(message: String) {
        self.message = message
    }
}

// MARK: - Notification DTOs (proto v7, S7)

/// A ready-to-schedule notification pushed by the Rust service. The Swift
/// layer schedules exactly this content — it never generates notification
/// copy itself.
///
/// `doNotDisturbUntil`, when present, is a future timestamp before which the
/// notification must not be delivered to the user.
public struct NotificationPayload: Codable, Equatable, Sendable {
    public let notificationID: UUID
    public let title: String
    public let body: String
    public let insightDate: String
    public let doNotDisturbUntil: Date?

    public init(
        notificationID: UUID,
        title: String,
        body: String,
        insightDate: String,
        doNotDisturbUntil: Date?
    ) {
        self.notificationID = notificationID
        self.title = title
        self.body = body
        self.insightDate = insightDate
        self.doNotDisturbUntil = doNotDisturbUntil
    }

    private enum CodingKeys: String, CodingKey {
        case notificationID = "notification_id"
        case title
        case body
        case insightDate = "insight_date"
        case doNotDisturbUntil = "do_not_disturb_until"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        notificationID = try container.decode(UUID.self, forKey: .notificationID)
        title = try container.decode(String.self, forKey: .title)
        body = try container.decode(String.self, forKey: .body)
        insightDate = try container.decode(String.self, forKey: .insightDate)
        doNotDisturbUntil = try container.decodeIfPresent(Date.self, forKey: .doNotDisturbUntil)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(notificationID, forKey: .notificationID)
        try container.encode(title, forKey: .title)
        try container.encode(body, forKey: .body)
        try container.encode(insightDate, forKey: .insightDate)
        try container.encodeIfPresent(doNotDisturbUntil, forKey: .doNotDisturbUntil)
    }
}

/// Creates the canonical JSON encoder and decoder used by IPC.
public enum IPCMessageCodec {
    public static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }

    public static func makeDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let string = try container.decode(String.self)
            if let date = fractionalSecondsFormatter.date(from: string) {
                return date
            }
            if let date = standardFormatter.date(from: string) {
                return date
            }
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Expected date string to be ISO8601-formatted: \(string)"
            )
        }
        return decoder
    }

    /// The Rust service serializes `chrono::DateTime<Utc>` (e.g.
    /// `AuthSuccess.expires_at`) with fractional seconds, such as
    /// "2026-06-19T21:36:13.182093Z". Foundation's built-in `.iso8601`
    /// decoding strategy cannot parse that: `ISO8601DateFormatter` without
    /// `.withFractionalSeconds` returns nil, so the decode throws. Because
    /// the IPC receive loop treats any decode failure as a dropped
    /// connection and reconnects, this previously discarded `AuthSuccess`
    /// entirely — the login/signup spinner span forever even though the
    /// server had already responded 200/201. Try fractional seconds first,
    /// then fall back to whole seconds for any other ISO8601 producer.
    private static let fractionalSecondsFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static let standardFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()
}
