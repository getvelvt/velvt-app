import Foundation

/// Messages sent from the macOS client to the Rust service.
public enum ClientMessage: Codable, Equatable, Sendable {
    case clientHello(ClientHello)
    case rawEvent(RawEventMessage)
    case errorResponse(ErrorResponse)

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
    /// Sent by the Rust service before a graceful shutdown.
    case shuttingDown(ShuttingDown)
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
        case "shutting_down":
            self = .shuttingDown(try ShuttingDown(from: payload))
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
        case let .shuttingDown(value):
            try envelope.encode("shutting_down", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .unknown(type):
            try envelope.encode(type, forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        }
    }
}

private enum EnvelopeCodingKeys: String, CodingKey { case type, payload }
private struct EmptyPayload: Codable {}

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
    public let appName: String
    public let windowTitle: String
    public let bundleID: String?

    public init(eventID: UUID, occurredAt: Date, appName: String, windowTitle: String, bundleID: String?) {
        self.eventID = eventID
        self.occurredAt = occurredAt
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleID = bundleID
    }

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case occurredAt = "occurred_at"
        case appName = "app_name"
        case windowTitle = "window_title"
        case bundleID = "bundle_id"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        eventID = try container.decode(UUID.self, forKey: .eventID)
        occurredAt = try container.decode(Date.self, forKey: .occurredAt)
        appName = try container.decode(String.self, forKey: .appName)
        windowTitle = try container.decode(String.self, forKey: .windowTitle)
        bundleID = try container.decodeIfPresent(String.self, forKey: .bundleID)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(eventID, forKey: .eventID)
        try container.encode(occurredAt, forKey: .occurredAt)
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

    private enum CodingKeys: String, CodingKey {
        case date
        case status
        case eventCount = "event_count"
        case focusScore = "focus_score"
        case fragmentationScore = "fragmentation_score"
        case confidenceLevel = "confidence_level"
        case activeSeconds = "active_seconds"
    }

    public init(
        date: String,
        status: HistoryStatus,
        eventCount: Int,
        focusScore: Double?,
        fragmentationScore: Double?,
        confidenceLevel: ConfidenceLevel,
        activeSeconds: Int
    ) {
        self.date = date
        self.status = status
        self.eventCount = eventCount
        self.focusScore = focusScore
        self.fragmentationScore = fragmentationScore
        self.confidenceLevel = confidenceLevel
        self.activeSeconds = activeSeconds
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
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }
}
