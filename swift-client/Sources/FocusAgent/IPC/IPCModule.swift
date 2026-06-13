import Foundation

/// IPC module - owns the Unix domain socket client, protocol version handshake,
/// newline-delimited JSON serialization, bounded reconnect buffering, and
/// reconnect behavior. Does NOT own event capture, abstraction logic, cloud API
/// calls, UI rendering, or notification scheduling.

/// Represents the connection state of the IPC client.
public enum IPCConnectionState: Equatable, Sendable {
    case disconnected
    case connecting
    case handshaking
    case connected
    case reconnecting(attempt: Int)
}

/// The IPC client interface. Implementors manage the socket lifecycle.
///
/// Implementors connect only to the configured Unix domain socket, send a
/// handshake request as the first frame on every connection, and do not report
/// `connected` until an accepted handshake response is received. Raw events
/// may be buffered in memory for at most 30 seconds while reconnecting.
public protocol IPCClient: AnyObject {
    var connectionState: IPCConnectionState { get }
    func connect() async throws
    func disconnect()
    func send(_ message: OutboundIPCMessage) async throws
    func setMessageHandler(_ handler: @escaping (InboundIPCMessage) -> Void)
}

/// Encodes one IPC message as newline-delimited JSON.
///
/// Implementors preserve the exact snake_case field names in `proto/schema/`,
/// encode UTC timestamps as ISO 8601 strings ending in `Z`, and append exactly
/// one newline delimiter. They omit absent `rejection_reason`, `drop_reason`,
/// `reason`, and `related_event_id` fields. `bundle_id` is the only optional
/// IPC field that may be encoded as JSON null.
public protocol IPCMessageEncoding {
    func encode(_ message: OutboundIPCMessage) throws -> Data
}

/// Decodes one newline-delimited JSON IPC message.
///
/// Implementors reject undeclared fields, invalid enum values, malformed JSON,
/// and server-to-client message types not represented by `InboundIPCMessage`.
public protocol IPCMessageDecoding {
    func decode(_ data: Data) throws -> InboundIPCMessage
}

/// Supplies bounded reconnect delays without polling for activity events.
public protocol IPCReconnectScheduling {
    /// Returns the delay before the given reconnect attempt.
    func delay(forAttempt attempt: Int) -> Duration
}

/// Messages sent by the Swift client.
public enum OutboundIPCMessage: Equatable, Sendable {
    case handshakeRequest(HandshakeRequest)
    case rawEvent(RawEventMessage)
    case errorResponse(ErrorResponse)
}

/// Messages received by the Swift client.
public enum InboundIPCMessage: Equatable, Sendable {
    case handshakeResponse(HandshakeResponse)
    case rawEventAck(RawEventAcknowledgement)
    case insightPayload(InsightPayload)
    case historyPayload(HistoryPayload)
    case serviceStatus(ServiceStatus)
    case errorResponse(ErrorResponse)
}

public struct HandshakeRequest: Equatable, Sendable {
    public let protocolVersion: Int
    public let clientVersion: String

    public init(protocolVersion: Int, clientVersion: String) {
        self.protocolVersion = protocolVersion
        self.clientVersion = clientVersion
    }
}

public struct HandshakeResponse: Equatable, Sendable {
    public let accepted: Bool
    public let serverProtocolVersion: Int
    public let rejectionReason: String?

    public init(accepted: Bool, serverProtocolVersion: Int, rejectionReason: String?) {
        self.accepted = accepted
        self.serverProtocolVersion = serverProtocolVersion
        self.rejectionReason = rejectionReason
    }
}

public struct RawEventMessage: Equatable, Sendable {
    public let eventID: UUID
    public let occurredAt: Date
    public let appName: String
    public let windowTitle: String
    public let bundleID: String?

    public init(
        eventID: UUID,
        occurredAt: Date,
        appName: String,
        windowTitle: String,
        bundleID: String?
    ) {
        self.eventID = eventID
        self.occurredAt = occurredAt
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleID = bundleID
    }
}

public enum RawEventAcknowledgementStatus: String, Equatable, Sendable {
    case accepted
    case dropped
}

public struct RawEventAcknowledgement: Equatable, Sendable {
    public let eventID: UUID
    public let status: RawEventAcknowledgementStatus
    public let dropReason: String?

    public init(eventID: UUID, status: RawEventAcknowledgementStatus, dropReason: String?) {
        self.eventID = eventID
        self.status = status
        self.dropReason = dropReason
    }
}

public enum ConfidenceLevel: String, Equatable, Sendable {
    case low
    case medium
    case high
}

public struct InsightPayload: Equatable, Sendable {
    public let date: String
    public let text: String
    public let confidenceLevel: ConfidenceLevel
    public let lowConfidence: Bool
    public let generatedAt: Date

    public init(
        date: String,
        text: String,
        confidenceLevel: ConfidenceLevel,
        lowConfidence: Bool,
        generatedAt: Date
    ) {
        self.date = date
        self.text = text
        self.confidenceLevel = confidenceLevel
        self.lowConfidence = lowConfidence
        self.generatedAt = generatedAt
    }
}

public enum HistoryStatus: String, Equatable, Sendable {
    case ready
    case noData = "no_data"
}

public struct DailySummary: Equatable, Sendable {
    public let date: String
    public let status: HistoryStatus
    public let eventCount: Int
    public let focusScore: Double?
    public let fragmentationScore: Double?
    public let confidenceLevel: ConfidenceLevel
    public let activeSeconds: Int

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

public struct HistoryPayload: Equatable, Sendable {
    public let days: Int
    public let summaries: [DailySummary]

    public init(days: Int, summaries: [DailySummary]) {
        self.days = days
        self.summaries = summaries
    }
}

public enum ServiceState: String, Equatable, Sendable {
    case ready
    case degraded
    case uploadPaused = "upload_paused"
    case authRequired = "auth_required"
}

public struct ServiceStatus: Equatable, Sendable {
    public let state: ServiceState
    public let reason: String?

    public init(state: ServiceState, reason: String?) {
        self.state = state
        self.reason = reason
    }
}

public struct ErrorResponse: Equatable, Sendable {
    public let code: String
    public let message: String
    public let relatedEventID: UUID?

    public init(code: String, message: String, relatedEventID: UUID?) {
        self.code = code
        self.message = message
        self.relatedEventID = relatedEventID
    }
}

/// Safe IPC errors that never include message content.
public enum IPCError: Error, Equatable {
    case socket(code: Int)
    case malformedMessage
    case unexpectedMessageType
    case handshakeRequired
    case handshakeRejected(reason: String)
    case unsupportedProtocolVersion(serverVersion: Int)
    case bufferExpired
}
