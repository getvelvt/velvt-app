import Foundation

/// Messages sent from the macOS client to the Rust service.
public enum ClientMessage: Codable, Equatable, Sendable {
    case clientHello(ClientHello)
    case rawEvent(RawEventMessage)
    case errorResponse(ErrorResponse)

    public init(from decoder: Decoder) throws {
        let type = try MessageTypeProbe(from: decoder).type
        try validateMessageKeys(type: type, decoder: decoder, allowedKeysByType: Self.allowedKeysByType)
        switch type {
        case "client_hello":
            self = .clientHello(try ClientHello(from: decoder))
        case "raw_event":
            self = .rawEvent(try RawEventMessage(from: decoder))
        case "error_response":
            self = .errorResponse(try ErrorResponse(from: decoder))
        default:
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Unknown client message type"))
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case let .clientHello(value):
            try value.encode(to: encoder)
        case let .rawEvent(value):
            try value.encode(to: encoder)
        case let .errorResponse(value):
            try value.encode(to: encoder)
        }
    }

    private static let allowedKeysByType: [String: Set<String>] = [
        "client_hello": ["type", "protocol_version", "client_version"],
        "raw_event": ["type", "event_id", "occurred_at", "app_name", "window_title", "bundle_id"],
        "error_response": ["type", "code", "message", "related_event_id"]
    ]
}

/// Messages sent from the Rust service to the macOS client.
public enum ServerMessage: Codable, Equatable, Sendable {
    case serverHello(ServerHello)
    case acknowledged(Acknowledged)
    case versionMismatch(VersionMismatch)
    case rawEventAck(RawEventAcknowledgement)
    case insightPayload(InsightPayload)
    case historyPayload(HistoryPayload)
    case serviceStatus(ServiceStatus)
    case errorResponse(ErrorResponse)
    /// Extension point for a future server discriminator. Unknown payload fields
    /// are deliberately discarded so handlers do not require exhaustive updates.
    case unknown(type: String)

    public init(from decoder: Decoder) throws {
        let type = try MessageTypeProbe(from: decoder).type
        try validateMessageKeys(type: type, decoder: decoder, allowedKeysByType: Self.allowedKeysByType)
        switch type {
        case "server_hello":
            self = .serverHello(try ServerHello(from: decoder))
        case "acknowledged":
            self = .acknowledged(try Acknowledged(from: decoder))
        case "version_mismatch":
            self = .versionMismatch(try VersionMismatch(from: decoder))
        case "raw_event_ack":
            self = .rawEventAck(try RawEventAcknowledgement(from: decoder))
        case "insight_payload":
            self = .insightPayload(try InsightPayload(from: decoder))
        case "history_payload":
            self = .historyPayload(try HistoryPayload(from: decoder))
        case "service_status":
            self = .serviceStatus(try ServiceStatus(from: decoder))
        case "error_response":
            self = .errorResponse(try ErrorResponse(from: decoder))
        default:
            self = .unknown(type: type)
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case let .serverHello(value):
            try value.encode(to: encoder)
        case let .acknowledged(value):
            try value.encode(to: encoder)
        case let .versionMismatch(value):
            try value.encode(to: encoder)
        case let .rawEventAck(value):
            try value.encode(to: encoder)
        case let .insightPayload(value):
            try value.encode(to: encoder)
        case let .historyPayload(value):
            try value.encode(to: encoder)
        case let .serviceStatus(value):
            try value.encode(to: encoder)
        case let .errorResponse(value):
            try value.encode(to: encoder)
        case let .unknown(type):
            var container = encoder.container(keyedBy: CommonCodingKeys.self)
            try container.encode(type, forKey: .type)
        }
    }

    private static let allowedKeysByType: [String: Set<String>] = [
        "server_hello": ["type", "protocol_version"],
        "acknowledged": ["type"],
        "version_mismatch": ["type", "expected", "got"],
        "raw_event_ack": ["type", "event_id", "status", "drop_reason"],
        "insight_payload": ["type", "date", "text", "confidence_level", "low_confidence", "generated_at"],
        "history_payload": ["type", "days", "summaries"],
        "service_status": ["type", "state", "reason"],
        "error_response": ["type", "code", "message", "related_event_id"]
    ]
}

private struct MessageTypeProbe: Decodable {
    let type: String
}

private struct DynamicCodingKey: CodingKey {
    let stringValue: String
    let intValue: Int? = nil

    init?(stringValue: String) {
        self.stringValue = stringValue
    }

    init?(intValue: Int) {
        return nil
    }
}

private func validateMessageKeys(
    type: String,
    decoder: Decoder,
    allowedKeysByType: [String: Set<String>]
) throws {
    guard let allowedKeys = allowedKeysByType[type] else {
        return
    }
    let container = try decoder.container(keyedBy: DynamicCodingKey.self)
    let actualKeys = Set(container.allKeys.map(\.stringValue))
    guard actualKeys.isSubset(of: allowedKeys) else {
        throw DecodingError.dataCorrupted(
            .init(codingPath: decoder.codingPath, debugDescription: "Message contains undeclared fields")
        )
    }
}

private protocol TaggedMessage {
    static var messageType: String { get }
}

private extension TaggedMessage {
    func encodeType(to container: inout KeyedEncodingContainer<CommonCodingKeys>) throws {
        try container.encode(Self.messageType, forKey: .type)
    }
}

private enum CommonCodingKeys: String, CodingKey {
    case type
}

/// Announces the server protocol version after a socket connection opens.
public struct ServerHello: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "server_hello"
    public let protocolVersion: Int

    public init(protocolVersion: Int) {
        self.protocolVersion = protocolVersion
    }

    private enum CodingKeys: String, CodingKey { case type; case protocolVersion = "protocol_version" }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try container.decode(Int.self, forKey: .protocolVersion)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(Self.messageType, forKey: .type)
        try container.encode(protocolVersion, forKey: .protocolVersion)
    }
}

/// Declares the client protocol and application versions.
public struct ClientHello: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "client_hello"
    public let protocolVersion: Int
    public let clientVersion: String

    public init(protocolVersion: Int, clientVersion: String) {
        self.protocolVersion = protocolVersion
        self.clientVersion = clientVersion
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case protocolVersion = "protocol_version"
        case clientVersion = "client_version"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try container.decode(Int.self, forKey: .protocolVersion)
        clientVersion = try container.decode(String.self, forKey: .clientVersion)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(Self.messageType, forKey: .type)
        try container.encode(protocolVersion, forKey: .protocolVersion)
        try container.encode(clientVersion, forKey: .clientVersion)
    }
}

/// Confirms that the client and server protocol versions match.
public struct Acknowledged: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "acknowledged"

    public init() {}

    public init(from decoder: Decoder) throws {
        _ = try decoder.container(keyedBy: CommonCodingKeys.self)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CommonCodingKeys.self)
        try encodeType(to: &container)
    }
}

/// Reports incompatible client and server protocol versions.
public struct VersionMismatch: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "version_mismatch"
    public let expected: Int
    public let got: Int

    public init(expected: Int, got: Int) {
        self.expected = expected
        self.got = got
    }

    private enum CodingKeys: String, CodingKey { case type, expected, got }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        expected = try container.decode(Int.self, forKey: .expected)
        got = try container.decode(Int.self, forKey: .got)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(Self.messageType, forKey: .type)
        try container.encode(expected, forKey: .expected)
        try container.encode(got, forKey: .got)
    }
}

/// A local-only raw activity event sent to the Rust privacy boundary.
public struct RawEventMessage: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "raw_event"
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
        case type
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
        try container.encode(Self.messageType, forKey: .type)
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
public struct RawEventAcknowledgement: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "raw_event_ack"
    public let eventID: UUID
    public let status: RawEventAcknowledgementStatus
    public let dropReason: String?

    public init(eventID: UUID, status: RawEventAcknowledgementStatus, dropReason: String?) {
        self.eventID = eventID
        self.status = status
        self.dropReason = dropReason
    }

    private enum CodingKeys: String, CodingKey {
        case type
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
        try container.encode(Self.messageType, forKey: .type)
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
public struct InsightPayload: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "insight_payload"
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
        case type
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
        try container.encode(Self.messageType, forKey: .type)
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
public struct HistoryPayload: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "history_payload"
    public let days: Int
    public let summaries: [DailySummary]

    public init(days: Int, summaries: [DailySummary]) {
        self.days = days
        self.summaries = summaries
    }

    private enum CodingKeys: String, CodingKey { case type, days, summaries }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        days = try container.decode(Int.self, forKey: .days)
        summaries = try container.decode([DailySummary].self, forKey: .summaries)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(Self.messageType, forKey: .type)
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
public struct ServiceStatus: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "service_status"
    public let state: ServiceState
    public let reason: String?

    public init(state: ServiceState, reason: String?) {
        self.state = state
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey { case type, state, reason }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        state = try container.decode(ServiceState.self, forKey: .state)
        reason = try container.decodeIfPresent(String.self, forKey: .reason)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(Self.messageType, forKey: .type)
        try container.encode(state, forKey: .state)
        try container.encodeIfPresent(reason, forKey: .reason)
    }
}

/// A typed error envelope that must contain only privacy-safe text.
public struct ErrorResponse: Codable, Equatable, Sendable, TaggedMessage {
    static let messageType = "error_response"
    public let code: String
    public let message: String
    public let relatedEventID: UUID?

    public init(code: String, message: String, relatedEventID: UUID?) {
        self.code = code
        self.message = message
        self.relatedEventID = relatedEventID
    }

    private enum CodingKeys: String, CodingKey {
        case type
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
        try container.encode(Self.messageType, forKey: .type)
        try container.encode(code, forKey: .code)
        try container.encode(message, forKey: .message)
        try container.encodeIfPresent(relatedEventID, forKey: .relatedEventID)
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
