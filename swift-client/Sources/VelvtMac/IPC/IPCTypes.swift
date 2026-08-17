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
    case updateClassificationOverride(UpdateClassificationOverride)
    case requestCorrectionHistory(RequestCorrectionHistory)
    case removeClassificationOverride(RemoveClassificationOverride)
    case resetClassificationOverrides
    case startWorkBlock(StartWorkBlock)
    case pauseWorkBlock(WorkBlockIdentifier)
    case resumeWorkBlock(WorkBlockIdentifier)
    case endWorkBlock(WorkBlockIdentifier)
    case requestWorkBlockState
    case requestLocalDashboard(RequestLocalDashboard)
    case acceptWorkBlockRecovery(AcceptWorkBlockRecovery)
    case reportInterventionOutcome(ReportInterventionOutcome)
    case workBlockLifecycle(WorkBlockLifecycle)
    case clearWorkBlockData
    case focusStateChanged(FocusStateChanged)
    case respondQuietHoursOffer(RespondQuietHoursOffer)

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
        case "update_classification_override":
            self = .updateClassificationOverride(try UpdateClassificationOverride(from: payload))
        case "request_correction_history":
            self = .requestCorrectionHistory(try RequestCorrectionHistory(from: payload))
        case "remove_classification_override":
            self = .removeClassificationOverride(try RemoveClassificationOverride(from: payload))
        case "reset_classification_overrides":
            self = .resetClassificationOverrides
        case "start_work_block":
            self = .startWorkBlock(try StartWorkBlock(from: payload))
        case "pause_work_block":
            self = .pauseWorkBlock(try WorkBlockIdentifier(from: payload))
        case "resume_work_block":
            self = .resumeWorkBlock(try WorkBlockIdentifier(from: payload))
        case "end_work_block":
            self = .endWorkBlock(try WorkBlockIdentifier(from: payload))
        case "request_work_block_state":
            self = .requestWorkBlockState
        case "request_local_dashboard":
            self = .requestLocalDashboard(try RequestLocalDashboard(from: payload))
        case "accept_work_block_recovery":
            self = .acceptWorkBlockRecovery(try AcceptWorkBlockRecovery(from: payload))
        case "report_intervention_outcome":
            self = .reportInterventionOutcome(try ReportInterventionOutcome(from: payload))
        case "work_block_lifecycle":
            self = .workBlockLifecycle(try WorkBlockLifecycle(from: payload))
        case "clear_work_block_data":
            self = .clearWorkBlockData
        case "focus_state_changed":
            self = .focusStateChanged(try FocusStateChanged(from: payload))
        case "respond_quiet_hours_offer":
            self = .respondQuietHoursOffer(try RespondQuietHoursOffer(from: payload))
        default:
      throw DecodingError.dataCorrupted(
        .init(codingPath: decoder.codingPath, debugDescription: "Unknown client message type"))
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
        case let .updateClassificationOverride(value):
            try envelope.encode("update_classification_override", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .requestCorrectionHistory(value):
            try envelope.encode("request_correction_history", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .removeClassificationOverride(value):
            try envelope.encode("remove_classification_override", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case .resetClassificationOverrides:
            try envelope.encode("reset_classification_overrides", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case let .startWorkBlock(value):
            try envelope.encode("start_work_block", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .pauseWorkBlock(value):
            try envelope.encode("pause_work_block", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .resumeWorkBlock(value):
            try envelope.encode("resume_work_block", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .endWorkBlock(value):
            try envelope.encode("end_work_block", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case .requestWorkBlockState:
            try envelope.encode("request_work_block_state", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case let .requestLocalDashboard(value):
            try envelope.encode("request_local_dashboard", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .acceptWorkBlockRecovery(value):
            try envelope.encode("accept_work_block_recovery", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .reportInterventionOutcome(value):
            try envelope.encode("report_intervention_outcome", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .workBlockLifecycle(value):
            try envelope.encode("work_block_lifecycle", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case .clearWorkBlockData:
            try envelope.encode("clear_work_block_data", forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        case let .focusStateChanged(value):
            try envelope.encode("focus_state_changed", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .respondQuietHoursOffer(value):
            try envelope.encode("respond_quiet_hours_offer", forKey: .type)
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
    case correctionHistoryPage(CorrectionHistoryPage)
    case workBlockState(WorkBlockSnapshot)
    case localDashboard(LocalDashboardSnapshot)
    /// A deterministic next-morning quiet-hours offer (rule-versioned).
    case quietHoursOffer(QuietHoursOffer)
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
        case "correction_history_page":
            self = .correctionHistoryPage(try CorrectionHistoryPage(from: payload))
        case "work_block_state":
            self = .workBlockState(try WorkBlockSnapshot(from: payload))
        case "local_dashboard":
            self = .localDashboard(try LocalDashboardSnapshot(from: payload))
        case "quiet_hours_offer":
            self = .quietHoursOffer(try QuietHoursOffer(from: payload))
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
        case let .correctionHistoryPage(value):
            try envelope.encode("correction_history_page", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .workBlockState(value):
            try envelope.encode("work_block_state", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .localDashboard(value):
            try envelope.encode("local_dashboard", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .quietHoursOffer(value):
            try envelope.encode("quiet_hours_offer", forKey: .type)
            try value.encode(to: envelope.superEncoder(forKey: .payload))
        case let .unknown(type):
            try envelope.encode(type, forKey: .type)
            try EmptyPayload().encode(to: envelope.superEncoder(forKey: .payload))
        }
    }

    /// Payload-free discriminator suitable for unified logs and crash-safe
    /// diagnostics. Never use `String(describing:)` on an IPC envelope.
    var safeLogDescription: String {
        switch self {
        case .serverHello: "server_hello"
        case .acknowledged: "acknowledged"
        case .versionMismatch: "version_mismatch"
        case .malformedMessage: "malformed_message"
        case .rawEventAck: "raw_event_ack"
        case .insightPayload: "insight_payload"
        case .historyPayload: "history_payload"
        case .serviceStatus: "service_status"
        case .privacyViolationAlert: "privacy_violation_alert"
        case .errorResponse: "error_response"
        case .cacheEmpty: "cache_empty"
        case .shuttingDown: "shutting_down"
        case .authSuccess: "auth_success"
        case .authSessionUpdated: "auth_session_updated"
        case .authFailure: "auth_failure"
        case .accountDeletionAccepted: "account_deletion_accepted"
        case .needsReauth: "needs_reauth"
        case .deviceRevoked: "device_revoked"
        case .notificationPayload: "notification_payload"
        case .menuStatus: "menu_status"
        case .correctionHistoryPage: "correction_history_page"
        case .workBlockState: "work_block_state"
        case .localDashboard: "local_dashboard"
        case .quietHoursOffer: "quiet_hours_offer"
        case .unknown: "unknown"
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
    public let reason: String?

    public init(payloadType: String, reason: String? = nil) {
        self.payloadType = payloadType
        self.reason = reason
    }

    private enum CodingKeys: String, CodingKey {
        case payloadType = "payload_type"
        case reason
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
    public let focusedDocumentURL: String?

    public init(
        eventID: UUID,
        occurredAt: Date,
        durationSeconds: Int = 0,
        appName: String,
        windowTitle: String,
        bundleID: String?,
        focusedDocumentURL: String? = nil
    ) {
        self.eventID = eventID
        self.occurredAt = occurredAt
        self.durationSeconds = durationSeconds
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleID = bundleID
        self.focusedDocumentURL = focusedDocumentURL
    }

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case occurredAt = "occurred_at"
        case durationSeconds = "duration_seconds"
        case appName = "app_name"
        case windowTitle = "window_title"
        case bundleID = "bundle_id"
        case focusedDocumentURL = "focused_document_url"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        eventID = try container.decode(UUID.self, forKey: .eventID)
        occurredAt = try container.decode(Date.self, forKey: .occurredAt)
        durationSeconds = try container.decode(Int.self, forKey: .durationSeconds)
        appName = try container.decode(String.self, forKey: .appName)
        windowTitle = try container.decode(String.self, forKey: .windowTitle)
        bundleID = try container.decodeIfPresent(String.self, forKey: .bundleID)
        focusedDocumentURL = try container.decodeIfPresent(String.self, forKey: .focusedDocumentURL)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(eventID, forKey: .eventID)
        try container.encode(occurredAt, forKey: .occurredAt)
        try container.encode(durationSeconds, forKey: .durationSeconds)
        try container.encode(appName, forKey: .appName)
        try container.encode(windowTitle, forKey: .windowTitle)
        try container.encode(bundleID, forKey: .bundleID)
        try container.encodeIfPresent(focusedDocumentURL, forKey: .focusedDocumentURL)
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

public enum EmotionalStage: String, Codable, Equatable, Sendable, CaseIterable {
    case early
    case stable
    case positiveDeviation = "positive_deviation"
    case sustainedPositiveTrend = "sustained_positive_trend"
    case negativeDeviation = "negative_deviation"
    case repeatedNegativeTrend = "repeated_negative_trend"
    case sustainedHighConfidenceDecline = "sustained_high_confidence_decline"
    case recovery
}

public struct InsightEvidence: Codable, Equatable, Sendable {
    public let observation: String
    public let comparison: String
    public let suggestedAction: String
    public let toneStage: EmotionalStage
    public let observationType: String
    public let templateID: String
    public let metricValue: Int
    public let metricUnit: String
    public let timeWindow: [String: String]
    public let safeCategories: [String]
    public let confidence: String
    public let coverage: Double
    public let baselineStatus: String
    public let baselineComparison: BaselineComparison?
    public let actionMinutes: Int
    public let repetitionDays: Int
    public let nextActionID: String
    public let direction: String
    public let magnitude: Double

    public init(
        observation: String,
        comparison: String,
        suggestedAction: String,
        toneStage: EmotionalStage,
        observationType: String,
        templateID: String = "unavailable",
        metricValue: Int,
        metricUnit: String,
        timeWindow: [String: String],
        safeCategories: [String],
        confidence: String,
        coverage: Double,
        baselineStatus: String,
        baselineComparison: BaselineComparison? = nil,
        actionMinutes: Int,
        repetitionDays: Int,
        nextActionID: String = "unavailable",
        direction: String = "stable",
        magnitude: Double = 0
    ) {
        self.observation = observation
        self.comparison = comparison
        self.suggestedAction = suggestedAction
        self.toneStage = toneStage
        self.observationType = observationType
        self.templateID = templateID
        self.metricValue = metricValue
        self.metricUnit = metricUnit
        self.timeWindow = timeWindow
        self.safeCategories = safeCategories
        self.confidence = confidence
        self.coverage = coverage
        self.baselineStatus = baselineStatus
        self.baselineComparison = baselineComparison
        self.actionMinutes = actionMinutes
        self.repetitionDays = repetitionDays
        self.nextActionID = nextActionID
        self.direction = direction
        self.magnitude = magnitude
    }

    public static let unavailable = InsightEvidence(
        observation: "Insight evidence is unavailable.",
        comparison: "No baseline comparison is available.",
        suggestedAction: "Protect one realistic work block.",
        toneStage: .early,
        observationType: "unavailable",
        templateID: "unavailable",
        metricValue: 0,
        metricUnit: "none",
        timeWindow: [:],
        safeCategories: [],
        confidence: "none",
        coverage: 0,
        baselineStatus: "unknown",
        actionMinutes: 0,
        repetitionDays: 0,
        nextActionID: "unavailable",
        direction: "stable",
        magnitude: 0
    )

    private enum CodingKeys: String, CodingKey {
        case observation, comparison, confidence, coverage
        case suggestedAction = "suggested_action"
        case toneStage = "tone_stage"
        case observationType = "observation_type"
        case templateID = "template_id"
        case metricValue = "metric_value"
        case metricUnit = "metric_unit"
        case timeWindow = "time_window"
        case safeCategories = "safe_categories"
        case baselineStatus = "baseline_status"
        case baselineComparison = "baseline_comparison"
        case actionMinutes = "action_minutes"
        case repetitionDays = "repetition_days"
        case nextActionID = "next_action_id"
        case direction, magnitude
    }
}

/// A ready-to-display daily insight.
public struct InsightPayload: Codable, Equatable, Sendable {
    public let date: String
    public let text: String
    public let evidence: InsightEvidence
    public let confidenceLevel: ConfidenceLevel
    public let lowConfidence: Bool
    public let generatedAt: Date

    public init(
        date: String,
        text: String,
        evidence: InsightEvidence = .unavailable,
        confidenceLevel: ConfidenceLevel,
        lowConfidence: Bool,
        generatedAt: Date
    ) {
        self.date = date
        self.text = text
        self.evidence = evidence
        self.confidenceLevel = confidenceLevel
        self.lowConfidence = lowConfidence
        self.generatedAt = generatedAt
    }

    private enum CodingKeys: String, CodingKey {
        case date
        case text
        case evidence
        case confidenceLevel = "confidence_level"
        case lowConfidence = "low_confidence"
        case generatedAt = "generated_at"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        date = try container.decode(String.self, forKey: .date)
        text = try container.decode(String.self, forKey: .text)
    evidence =
      try container.decodeIfPresent(InsightEvidence.self, forKey: .evidence) ?? .unavailable
        confidenceLevel = try container.decode(ConfidenceLevel.self, forKey: .confidenceLevel)
        lowConfidence = try container.decode(Bool.self, forKey: .lowConfidence)
        generatedAt = try container.decode(Date.self, forKey: .generatedAt)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(date, forKey: .date)
        try container.encode(text, forKey: .text)
        try container.encode(evidence, forKey: .evidence)
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
    public let focusedSeconds: Int
    public let meaningfulSwitchCount: Int
    public let longestUninterruptedSeconds: Int
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
        case focusedSeconds = "focused_seconds"
        case meaningfulSwitchCount = "meaningful_switch_count"
        case longestUninterruptedSeconds = "focus_seconds"
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
        focusedSeconds: Int = 0,
        meaningfulSwitchCount: Int = 0,
        longestUninterruptedSeconds: Int = 0,
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
        self.focusedSeconds = focusedSeconds
        self.meaningfulSwitchCount = meaningfulSwitchCount
        self.longestUninterruptedSeconds = longestUninterruptedSeconds
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
        focusedSeconds = try container.decodeIfPresent(Int.self, forKey: .focusedSeconds) ?? 0
    meaningfulSwitchCount =
      try container.decodeIfPresent(Int.self, forKey: .meaningfulSwitchCount) ?? 0
    longestUninterruptedSeconds =
      try container.decodeIfPresent(Int.self, forKey: .longestUninterruptedSeconds) ?? 0
    baselineStatus =
      try container.decodeIfPresent(String.self, forKey: .baselineStatus) ?? "unknown"
    baselineComparison = try container.decodeIfPresent(
      BaselineComparison.self, forKey: .baselineComparison)
    typeProportions =
      try container.decodeIfPresent([ActivityProportion].self, forKey: .typeProportions) ?? []
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
        try container.encode(focusedSeconds, forKey: .focusedSeconds)
        try container.encode(meaningfulSwitchCount, forKey: .meaningfulSwitchCount)
        try container.encode(longestUninterruptedSeconds, forKey: .longestUninterruptedSeconds)
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
public enum ClassificationStatus: String, Codable, Equatable, Sendable {
    case classified
    case ambiguous
    case unclassified
}

public enum ClassificationConfidence: String, Codable, Equatable, Sendable {
    case high
    case medium
    case low
    case none
}

public enum ClassificationSource: String, Codable, Equatable, Sendable {
    case seed
    case heuristic
    case embedding
    case userRule = "user_rule"
    case fallback
}

public struct QueuedEventSummary: Codable, Equatable, Sendable, Identifiable {
    public let eventID: UUID
    public let stableID: String
    public let label: String
    public let localLabel: String?
    public let category: String
    public let classificationTier: String
    public let classificationStatus: ClassificationStatus
    public let classificationConfidence: ClassificationConfidence
    public let classificationSource: ClassificationSource
    public let occurredAt: Date

    public var id: UUID { eventID }

    private enum CodingKeys: String, CodingKey {
        case label, category
        case eventID = "event_id"
        case stableID = "stable_id"
        case localLabel = "local_label"
        case classificationTier = "classification_tier"
        case classificationStatus = "classification_status"
        case classificationConfidence = "classification_confidence"
        case classificationSource = "classification_source"
        case occurredAt = "occurred_at"
    }
}

public struct CorrectEventClassification: Codable, Equatable, Sendable {
    public let eventID: UUID
    public let stableID: String
    public let category: String
    public let localActivityName: String?

    private enum CodingKeys: String, CodingKey {
        case eventID = "event_id"
        case stableID = "stable_id"
        case category
        case localActivityName = "local_activity_name"
    }

    public init(
        eventID: UUID,
        stableID: String,
        category: String,
        localActivityName: String? = nil
    ) {
        self.eventID = eventID
        self.stableID = stableID
        self.category = category
        self.localActivityName = localActivityName
    }
}

public struct UpdateClassificationOverride: Codable, Equatable, Sendable {
    public let stableID: String
    public let category: String
    public let localActivityName: String?

    public init(stableID: String, category: String, localActivityName: String?) {
        self.stableID = stableID
        self.category = category
        self.localActivityName = localActivityName
    }

    private enum CodingKeys: String, CodingKey {
        case category
        case stableID = "stable_id"
        case localActivityName = "local_activity_name"
    }
}

public struct RequestCorrectionHistory: Codable, Equatable, Sendable {
    public let query: String?
    public let offset: Int
    public let pageSize: Int

    public init(query: String?, offset: Int, pageSize: Int = 20) {
        self.query = query
        self.offset = max(0, offset)
        self.pageSize = min(max(1, pageSize), 20)
    }

    private enum CodingKeys: String, CodingKey {
        case query, offset
        case pageSize = "page_size"
    }
}

public struct RemoveClassificationOverride: Codable, Equatable, Sendable {
    public let stableID: String

    private enum CodingKeys: String, CodingKey {
        case stableID = "stable_id"
    }

    public init(stableID: String) {
        self.stableID = stableID
    }
}

public struct ClassificationCorrectionSummary: Codable, Equatable, Sendable, Identifiable {
    public let stableID: String
    public let label: String
    public let localLabel: String?
    public let category: String
    public let updatedAt: Date

    public var id: String { stableID }

    private enum CodingKeys: String, CodingKey {
        case label, category
        case stableID = "stable_id"
        case localLabel = "local_label"
        case updatedAt = "updated_at"
    }
}

public struct CorrectionHistoryPage: Codable, Equatable, Sendable {
    public let items: [ClassificationCorrectionSummary]
    public let offset: Int
    public let pageSize: Int
    public let totalCount: Int
    public let hasMore: Bool

    public init(
        items: [ClassificationCorrectionSummary],
        offset: Int,
        pageSize: Int,
        totalCount: Int,
        hasMore: Bool
    ) {
        self.items = items
        self.offset = offset
        self.pageSize = pageSize
        self.totalCount = totalCount
        self.hasMore = hasMore
    }

    private enum CodingKeys: String, CodingKey {
        case items, offset
        case pageSize = "page_size"
        case totalCount = "total_count"
        case hasMore = "has_more"
    }
}

/// Snapshot returned by the Rust privacy boundary for the settings popover.
public struct MenuStatus: Codable, Equatable, Sendable {
    public let deviceID: String?
    public let cloudReady: Bool
    public let uploadStatus: String
    public let lastUploadErrorCode: String?
    public let nextUploadAttemptAt: Date?
    public let lastSuccessfulSyncAt: Date?
    public let pendingUploadBatchCount: Int
    public let failedUploadBatchCount: Int
    public let rejectedUploadBatchCount: Int
    public let queuedEventCount: Int
    public let queuedEvents: [QueuedEventSummary]
    public let correctionHistory: [ClassificationCorrectionSummary]
    /// One-shot confirmation authored by the service, present only on the
    /// status returned by a correction command. A polled status carries `nil`,
    /// so the confirmation cannot linger or reappear.
    public let correctionAcknowledgment: String?

    private enum CodingKeys: String, CodingKey {
        case deviceID = "device_id"
        case cloudReady = "cloud_ready"
        case uploadStatus = "upload_status"
        case lastUploadErrorCode = "last_upload_error_code"
        case nextUploadAttemptAt = "next_upload_attempt_at"
        case lastSuccessfulSyncAt = "last_successful_sync_at"
        case pendingUploadBatchCount = "pending_upload_batch_count"
        case failedUploadBatchCount = "failed_upload_batch_count"
        case rejectedUploadBatchCount = "rejected_upload_batch_count"
        case queuedEventCount = "queued_event_count"
        case queuedEvents = "queued_events"
        case correctionHistory = "correction_history"
        case correctionAcknowledgment = "correction_acknowledgment"
    }

    public init(
        deviceID: String?,
        cloudReady: Bool,
        uploadStatus: String,
        lastUploadErrorCode: String?,
        nextUploadAttemptAt: Date?,
        lastSuccessfulSyncAt: Date? = nil,
        pendingUploadBatchCount: Int,
        failedUploadBatchCount: Int,
        rejectedUploadBatchCount: Int,
        queuedEventCount: Int,
        queuedEvents: [QueuedEventSummary],
        correctionHistory: [ClassificationCorrectionSummary] = [],
        correctionAcknowledgment: String? = nil
    ) {
        self.deviceID = deviceID
        self.cloudReady = cloudReady
        self.uploadStatus = uploadStatus
        self.lastUploadErrorCode = lastUploadErrorCode
        self.nextUploadAttemptAt = nextUploadAttemptAt
        self.lastSuccessfulSyncAt = lastSuccessfulSyncAt
        self.pendingUploadBatchCount = pendingUploadBatchCount
        self.failedUploadBatchCount = failedUploadBatchCount
        self.rejectedUploadBatchCount = rejectedUploadBatchCount
        self.queuedEventCount = queuedEventCount
        self.queuedEvents = queuedEvents
        self.correctionHistory = correctionHistory
        self.correctionAcknowledgment = correctionAcknowledgment
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        deviceID = try container.decodeIfPresent(String.self, forKey: .deviceID)
        cloudReady = try container.decode(Bool.self, forKey: .cloudReady)
    uploadStatus =
      try container.decodeIfPresent(String.self, forKey: .uploadStatus)
      ?? (cloudReady ? "ready" : "network_unavailable")
        lastUploadErrorCode = try container.decodeIfPresent(String.self, forKey: .lastUploadErrorCode)
        nextUploadAttemptAt = try container.decodeIfPresent(Date.self, forKey: .nextUploadAttemptAt)
        lastSuccessfulSyncAt = try container.decodeIfPresent(Date.self, forKey: .lastSuccessfulSyncAt)
    pendingUploadBatchCount =
      try container.decodeIfPresent(Int.self, forKey: .pendingUploadBatchCount) ?? 0
    failedUploadBatchCount =
      try container.decodeIfPresent(Int.self, forKey: .failedUploadBatchCount) ?? 0
    rejectedUploadBatchCount =
      try container.decodeIfPresent(Int.self, forKey: .rejectedUploadBatchCount) ?? 0
        queuedEventCount = try container.decode(Int.self, forKey: .queuedEventCount)
        queuedEvents = try container.decode([QueuedEventSummary].self, forKey: .queuedEvents)
        correctionHistory =
            try container.decodeIfPresent(
                [ClassificationCorrectionSummary].self,
                forKey: .correctionHistory
            ) ?? []
        correctionAcknowledgment =
            try container.decodeIfPresent(String.self, forKey: .correctionAcknowledgment)
    }
}

// MARK: - Device-local Focus/Activity and meaningful-work DTOs (proto v22)

public struct RequestLocalDashboard: Codable, Equatable, Sendable {
    public let windowSeconds: Int
  public let utcOffsetSeconds: Int

  public init(windowSeconds: Int = 3600, utcOffsetSeconds: Int = TimeZone.current.secondsFromGMT())
  {
        self.windowSeconds = windowSeconds
    self.utcOffsetSeconds = utcOffsetSeconds
    }

    private enum CodingKeys: String, CodingKey {
        case windowSeconds = "window_seconds"
    case utcOffsetSeconds = "utc_offset_seconds"
    }
}

public enum LocalDashboardCoverage: String, Codable, Equatable, Sendable {
    case noData = "no_data"
    case partial
    case good
}

public struct LocalTimelineSegment: Codable, Equatable, Sendable, Identifiable {
  public let id: String
    public let startedAt: Date
    public let endedAt: Date
    public let category: String
    public let confidence: ClassificationConfidence

  private enum CodingKeys: String, CodingKey {
    case id, category, confidence
    case startedAt = "started_at"
    case endedAt = "ended_at"
  }
}

public struct LocalTransitionMarker: Codable, Equatable, Sendable, Identifiable {
  public let id: String
  public let occurredAt: Date
  public let fromCategory: String
  public let toCategory: String
  public let confidence: ClassificationConfidence

  private enum CodingKeys: String, CodingKey {
    case id, confidence
    case occurredAt = "occurred_at"
    case fromCategory = "from_category"
    case toCategory = "to_category"
  }
}

public struct LocalSwitchingCluster: Codable, Equatable, Sendable, Identifiable {
  public let id: String
  public let ruleVersion: Int
  public let startedAt: Date
  public let endedAt: Date
  public let transitionCount: Int
  public let categories: [String]
  public let confidence: ClassificationConfidence
  public let explanation: String

    private enum CodingKeys: String, CodingKey {
    case id, categories, confidence, explanation
    case ruleVersion = "rule_version"
        case startedAt = "started_at"
        case endedAt = "ended_at"
    case transitionCount = "transition_count"
  }
}

public enum LocalComparisonKind: String, Codable, Equatable, Sendable {
  case earlierToday = "earlier_today"
  case sevenDayPattern = "seven_day_pattern"
}

public struct LocalFocusComparison: Codable, Equatable, Sendable {
  public let kind: LocalComparisonKind
  public let label: String
  public let switchDelta: Int
  public let explanation: String

  private enum CodingKeys: String, CodingKey {
    case kind, label, explanation
    case switchDelta = "switch_delta"
  }
}

public struct LocalFocusFragmentation: Codable, Equatable, Sendable {
  public let blockID: UUID
  public let phase: WorkBlockPhase
  public let windowLabel: String
  public let windowStartedAt: Date
  public let windowEndedAt: Date
  public let plannedDurationSeconds: Int
  public let elapsedDurationSeconds: Int
  public let longestUninterruptedSeconds: Int
  public let observedSwitchCount: Int
  public let recoveryCount: Int
  public let coverage: LocalDashboardCoverage
  public let coverageRatio: Double
  public let comparison: LocalFocusComparison?
  public let observation: String
  public let nextAction: String
  public let segments: [LocalTimelineSegment]
  public let transitions: [LocalTransitionMarker]
  public let clusters: [LocalSwitchingCluster]

  private enum CodingKeys: String, CodingKey {
    case phase, coverage, comparison, observation, segments, transitions, clusters
    case blockID = "block_id"
    case windowLabel = "window_label"
    case windowStartedAt = "window_started_at"
    case windowEndedAt = "window_ended_at"
    case plannedDurationSeconds = "planned_duration_seconds"
    case elapsedDurationSeconds = "elapsed_duration_seconds"
    case longestUninterruptedSeconds = "longest_uninterrupted_seconds"
    case observedSwitchCount = "observed_switch_count"
    case recoveryCount = "recovery_count"
    case coverageRatio = "coverage_ratio"
    case nextAction = "next_action"
  }
}

public enum LocalDailyActivityState: String, Codable, Equatable, Sendable {
  case noData = "no_data"
  case lowConfidence = "low_confidence"
  case ready
  case stillBuilding = "still_building"
}

public struct LocalDailyActivitySegment: Codable, Equatable, Sendable, Identifiable {
  public let id: String
  public let label: String
  public let representativeEventID: UUID?
  public let stableID: String?
  public let suggestedName: String?
  public let aliasConfirmed: Bool
  public let category: String
  public let durationSeconds: Int
  public let percentage: Int
  public let confidence: ClassificationConfidence
  public let explanation: String?

  public init(
    id: String,
    label: String,
    representativeEventID: UUID? = nil,
    stableID: String? = nil,
    suggestedName: String? = nil,
    aliasConfirmed: Bool = false,
    category: String,
    durationSeconds: Int,
    percentage: Int,
    confidence: ClassificationConfidence,
    explanation: String?
  ) {
    self.id = id
    self.label = label
    self.representativeEventID = representativeEventID
    self.stableID = stableID
    self.suggestedName = suggestedName
    self.aliasConfirmed = aliasConfirmed
    self.category = category
    self.durationSeconds = durationSeconds
    self.percentage = percentage
    self.confidence = confidence
    self.explanation = explanation
  }

  private enum CodingKeys: String, CodingKey {
    case id, label, category, percentage, confidence, explanation
    case representativeEventID = "representative_event_id"
    case stableID = "stable_id"
    case suggestedName = "suggested_name"
    case aliasConfirmed = "alias_confirmed"
    case durationSeconds = "duration_seconds"
  }
}

public struct LocalDailyActivityDay: Codable, Equatable, Sendable, Identifiable {
  public let id: String
  public let date: String
  public let state: LocalDailyActivityState
  public let activeSeconds: Int
  public let coverage: LocalDashboardCoverage
  public let segments: [LocalDailyActivitySegment]

  private enum CodingKeys: String, CodingKey {
    case id, date, state, coverage, segments
    case activeSeconds = "active_seconds"
    }
}

public enum LocalEarlySignalStatus: String, Codable, Equatable, Sendable {
    case insufficientEvidence = "insufficient_evidence"
    case ready
}

public struct LocalEarlySignal: Codable, Equatable, Sendable {
    public let status: LocalEarlySignalStatus
    public let observedFrom: Date?
    public let observedThrough: Date
    public let observedSeconds: Int
    public let requiredSeconds: Int
    public let evidenceEventCount: Int
    public let focusedSeconds: Int
    public let meaningfulSwitchCount: Int
    public let longestUninterruptedSeconds: Int
    public let observation: String?
    public let suggestedAction: String?
    public let actionMinutes: Int

    private enum CodingKeys: String, CodingKey {
        case status, observation
        case observedFrom = "observed_from"
        case observedThrough = "observed_through"
        case observedSeconds = "observed_seconds"
        case requiredSeconds = "required_seconds"
        case evidenceEventCount = "evidence_event_count"
        case focusedSeconds = "focused_seconds"
        case meaningfulSwitchCount = "meaningful_switch_count"
        case longestUninterruptedSeconds = "longest_uninterrupted_seconds"
        case suggestedAction = "suggested_action"
        case actionMinutes = "action_minutes"
    }

    public init(
        status: LocalEarlySignalStatus,
        observedFrom: Date?,
        observedThrough: Date,
        observedSeconds: Int,
        requiredSeconds: Int,
        evidenceEventCount: Int,
        focusedSeconds: Int,
        meaningfulSwitchCount: Int,
        longestUninterruptedSeconds: Int,
        observation: String?,
        suggestedAction: String?,
        actionMinutes: Int
    ) {
        self.status = status
        self.observedFrom = observedFrom
        self.observedThrough = observedThrough
        self.observedSeconds = observedSeconds
        self.requiredSeconds = requiredSeconds
        self.evidenceEventCount = evidenceEventCount
        self.focusedSeconds = focusedSeconds
        self.meaningfulSwitchCount = meaningfulSwitchCount
        self.longestUninterruptedSeconds = longestUninterruptedSeconds
        self.observation = observation
        self.suggestedAction = suggestedAction
        self.actionMinutes = actionMinutes
    }
}

public struct LocalDashboardSnapshot: Codable, Equatable, Sendable {
    public let generatedAt: Date
    public let windowStart: Date
    public let windowEnd: Date
    public let switchCount: Int
    public let switchesPerHour: Double
    public let coverage: LocalDashboardCoverage
    public let earlySignal: LocalEarlySignal
    public let segments: [LocalTimelineSegment]
  public let focusFragmentation: LocalFocusFragmentation?
  public let dailyActivity: [LocalDailyActivityDay]

    private enum CodingKeys: String, CodingKey {
        case coverage, segments
        case earlySignal = "early_signal"
    case focusFragmentation = "focus_fragmentation"
    case dailyActivity = "daily_activity"
        case generatedAt = "generated_at"
        case windowStart = "window_start"
        case windowEnd = "window_end"
        case switchCount = "switch_count"
        case switchesPerHour = "switches_per_hour"
    }
}

public enum WorkBlockPurpose: String, Codable, Equatable, Sendable, CaseIterable, Identifiable {
    case deepWork = "deep_work"
    case study
    case creativePractice = "creative_practice"
    case healthyTechUse = "healthy_tech_use"
    case workLifeBoundary = "work_life_boundary"

    public var id: String { rawValue }
}

public enum WorkBlockIntensity: String, Codable, Equatable, Sendable, CaseIterable, Identifiable {
    case light
    case medium
    case intense

    public var id: String { rawValue }
}

public enum WorkBlockPhase: String, Codable, Equatable, Sendable {
    case idle
    case active
    case paused
    case completed
    case abandoned
    case expired
}

public struct StartWorkBlock: Codable, Equatable, Sendable {
    public let intention: String?
    public let plannedDurationSeconds: Int
    public let purpose: WorkBlockPurpose?
    public let intensity: WorkBlockIntensity

    public init(
        intention: String?,
        plannedDurationSeconds: Int,
        purpose: WorkBlockPurpose?,
        intensity: WorkBlockIntensity
    ) {
        self.intention = intention
        self.plannedDurationSeconds = plannedDurationSeconds
        self.purpose = purpose
        self.intensity = intensity
    }

    private enum CodingKeys: String, CodingKey {
        case intention, purpose, intensity
        case plannedDurationSeconds = "planned_duration_seconds"
    }
}

public struct WorkBlockIdentifier: Codable, Equatable, Sendable {
    public let blockID: UUID

    public init(blockID: UUID) { self.blockID = blockID }

    private enum CodingKeys: String, CodingKey { case blockID = "block_id" }
}

public struct AcceptWorkBlockRecovery: Codable, Equatable, Sendable {
    public let blockID: UUID
    public let actionID: String

    public init(blockID: UUID, actionID: String) {
        self.blockID = blockID
        self.actionID = actionID
    }

    private enum CodingKeys: String, CodingKey {
        case blockID = "block_id"
        case actionID = "action_id"
    }
}

/// The user's explicit reply to an in-session drift offer.
///
/// Only replies a person can actually give are representable. Silence is not in
/// this set: the service records it when the block ends, and it is never
/// inferred from a card or notification disappearing.
public enum InterventionResponse: String, Codable, Equatable, Sendable, CaseIterable {
    case acceptedAction = "accepted_action"
    case notHelpful = "not_helpful"
    case wrongClassification = "wrong_classification"
    /// The offer itself was wrong: the user was working the whole time.
    ///
    /// Kept distinct from every other reply. `dismissed` says "not now",
    /// `notHelpful` concedes the drift, and `wrongClassification` disputes a
    /// label. Only this one says the intervention should never have fired,
    /// which is what makes it measurable as a false positive.
    case wasFocused = "was_focused"
    case dismissed
}

/// How loudly an offer was delivered.
///
/// Salience only ever decreases. `quiet` means the in-app card rendered and no
/// notification was sent, because the user pushed a recent offer away.
public enum InterventionSalience: String, Codable, Equatable, Sendable {
    case normal
    case quiet
}

public struct ReportInterventionOutcome: Codable, Equatable, Sendable {
    public let blockID: UUID
    public let response: InterventionResponse

    public init(blockID: UUID, response: InterventionResponse) {
        self.blockID = blockID
        self.response = response
    }

    private enum CodingKeys: String, CodingKey {
        case blockID = "block_id"
        case response
    }
}

/// A live drift offer awaiting a reply. Rust authors the copy; Swift renders it
/// verbatim and never reinterprets the evidence.
public struct ActiveIntervention: Codable, Equatable, Sendable {
    public let actionID: String
    public let title: String
    public let body: String
    public let anchorCategory: String
    public let switchCount: Int
    public let windowSeconds: Int
    public let offeredAt: Date
    public let salience: InterventionSalience

    public init(
        actionID: String,
        title: String,
        body: String,
        anchorCategory: String,
        switchCount: Int,
        windowSeconds: Int,
        offeredAt: Date,
        salience: InterventionSalience = .normal
    ) {
        self.actionID = actionID
        self.title = title
        self.body = body
        self.anchorCategory = anchorCategory
        self.switchCount = switchCount
        self.windowSeconds = windowSeconds
        self.offeredAt = offeredAt
        self.salience = salience
    }

    private enum CodingKeys: String, CodingKey {
        case title, body, salience
        case actionID = "action_id"
        case anchorCategory = "anchor_category"
        case switchCount = "switch_count"
        case windowSeconds = "window_seconds"
        case offeredAt = "offered_at"
    }
}

/// A coarse system Focus/DND transition observed by the client.
///
/// PRIVACY: only whether some Focus mode is active, when the transition was
/// observed, and the client's UTC offset are representable here. The Focus
/// mode's name, configuration, and schedule must never be added. Swift only
/// observes; the Rust service owns the evidence record and every decision
/// derived from it.
public struct FocusStateChanged: Codable, Equatable, Sendable {
    public let active: Bool
    public let occurredAt: Date
    public let utcOffsetSeconds: Int

    public init(active: Bool, occurredAt: Date, utcOffsetSeconds: Int) {
        self.active = active
        self.occurredAt = occurredAt
        self.utcOffsetSeconds = utcOffsetSeconds
    }

    private enum CodingKeys: String, CodingKey {
        case active
        case occurredAt = "occurred_at"
        case utcOffsetSeconds = "utc_offset_seconds"
    }
}

/// The user's one-tap reply to a quiet-hours offer. Accepting configures
/// Velvt's own quiet hours; declining is remembered by the service for a
/// versioned interval and changes nothing else.
public struct RespondQuietHoursOffer: Codable, Equatable, Sendable {
    public let accepted: Bool

    public init(accepted: Bool) { self.accepted = accepted }
}

/// A next-morning quiet-hours offer produced by the Rust service's
/// deterministic, versioned late-night DND pattern rule. Rust authors the
/// copy; Swift renders it verbatim. An offer, never a workaround.
public struct QuietHoursOffer: Codable, Equatable, Sendable {
    public let ruleVersion: Int
    public let lateNightDays: Int
    public let startLocalMinutes: Int
    public let endLocalMinutes: Int
    public let body: String

    public init(
        ruleVersion: Int,
        lateNightDays: Int,
        startLocalMinutes: Int,
        endLocalMinutes: Int,
        body: String
    ) {
        self.ruleVersion = ruleVersion
        self.lateNightDays = lateNightDays
        self.startLocalMinutes = startLocalMinutes
        self.endLocalMinutes = endLocalMinutes
        self.body = body
    }

    private enum CodingKeys: String, CodingKey {
        case body
        case ruleVersion = "rule_version"
        case lateNightDays = "late_night_days"
        case startLocalMinutes = "start_local_minutes"
        case endLocalMinutes = "end_local_minutes"
    }
}

/// Focus/DND-derived outcome recorded by the Rust service for one block.
///
/// `completedUnderDnd` marks a success: the block completed while DND was
/// active. Each `deliverySuppressedDnd` entry is one mid-block nudge the
/// service held because DND was active — delivered by no channel and
/// reconciled after the block as a count only.
public enum WorkBlockDndOutcome: String, Codable, Equatable, Sendable {
    case completedUnderDnd = "completed_under_dnd"
    case deliverySuppressedDnd = "delivery_suppressed_dnd"
}

public enum WorkBlockLifecycleEvent: String, Codable, Equatable, Sendable {
    case sleep
    case wake
    case clockChanged = "clock_changed"
    case timeZoneChanged = "time_zone_changed"
}

public struct WorkBlockLifecycle: Codable, Equatable, Sendable {
    public let event: WorkBlockLifecycleEvent
    public init(event: WorkBlockLifecycleEvent) { self.event = event }
}

public enum WorkBlockCoverage: String, Codable, Equatable, Sendable {
    case insufficient
    case partial
    case good
}

public struct WorkBlockNextAction: Codable, Equatable, Sendable {
    public let actionID: String
    public let label: String
    public let durationSeconds: Int

    private enum CodingKeys: String, CodingKey {
        case label
        case actionID = "action_id"
        case durationSeconds = "duration_seconds"
    }
}

public struct WorkBlockResult: Codable, Equatable, Sendable {
    public let plannedDurationSeconds: Int
    public let elapsedDurationSeconds: Int
    public let longestUninterruptedSeconds: Int
    public let switchAwayCount: Int
    public let recoveryCount: Int
    public let confidence: ConfidenceLevel
    public let coverage: WorkBlockCoverage
    public let coverageRatio: Double
    public let safeEvidenceCategory: String?
    public let observation: String
    public let nextAction: WorkBlockNextAction
    /// Focus/DND outcomes recorded by the service, in recorded order. Empty
    /// on pre-v26 payloads and blocks without DND evidence.
    public let dndOutcomes: [WorkBlockDndOutcome]
    /// At most one Rust-authored calm post-block line noting what was held
    /// under DND. Rendered verbatim; never a late nudge.
    public let reconciliation: String?

    public init(
        plannedDurationSeconds: Int,
        elapsedDurationSeconds: Int,
        longestUninterruptedSeconds: Int,
        switchAwayCount: Int,
        recoveryCount: Int,
        confidence: ConfidenceLevel,
        coverage: WorkBlockCoverage,
        coverageRatio: Double,
        safeEvidenceCategory: String?,
        observation: String,
        nextAction: WorkBlockNextAction,
        dndOutcomes: [WorkBlockDndOutcome] = [],
        reconciliation: String? = nil
    ) {
        self.plannedDurationSeconds = plannedDurationSeconds
        self.elapsedDurationSeconds = elapsedDurationSeconds
        self.longestUninterruptedSeconds = longestUninterruptedSeconds
        self.switchAwayCount = switchAwayCount
        self.recoveryCount = recoveryCount
        self.confidence = confidence
        self.coverage = coverage
        self.coverageRatio = coverageRatio
        self.safeEvidenceCategory = safeEvidenceCategory
        self.observation = observation
        self.nextAction = nextAction
        self.dndOutcomes = dndOutcomes
        self.reconciliation = reconciliation
    }

    private enum CodingKeys: String, CodingKey {
        case confidence, coverage, observation, reconciliation
        case plannedDurationSeconds = "planned_duration_seconds"
        case elapsedDurationSeconds = "elapsed_duration_seconds"
        case longestUninterruptedSeconds = "longest_uninterrupted_seconds"
        case switchAwayCount = "switch_away_count"
        case recoveryCount = "recovery_count"
        case coverageRatio = "coverage_ratio"
        case safeEvidenceCategory = "safe_evidence_category"
        case nextAction = "next_action"
        case dndOutcomes = "dnd_outcomes"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        plannedDurationSeconds = try container.decode(Int.self, forKey: .plannedDurationSeconds)
        elapsedDurationSeconds = try container.decode(Int.self, forKey: .elapsedDurationSeconds)
        longestUninterruptedSeconds = try container.decode(
            Int.self, forKey: .longestUninterruptedSeconds)
        switchAwayCount = try container.decode(Int.self, forKey: .switchAwayCount)
        recoveryCount = try container.decode(Int.self, forKey: .recoveryCount)
        confidence = try container.decode(ConfidenceLevel.self, forKey: .confidence)
        coverage = try container.decode(WorkBlockCoverage.self, forKey: .coverage)
        coverageRatio = try container.decode(Double.self, forKey: .coverageRatio)
        safeEvidenceCategory = try container.decodeIfPresent(
            String.self, forKey: .safeEvidenceCategory)
        observation = try container.decode(String.self, forKey: .observation)
        nextAction = try container.decode(WorkBlockNextAction.self, forKey: .nextAction)
        // Optional on the wire so pre-v26 results decode unchanged.
        dndOutcomes =
            try container.decodeIfPresent([WorkBlockDndOutcome].self, forKey: .dndOutcomes) ?? []
        reconciliation = try container.decodeIfPresent(String.self, forKey: .reconciliation)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(plannedDurationSeconds, forKey: .plannedDurationSeconds)
        try container.encode(elapsedDurationSeconds, forKey: .elapsedDurationSeconds)
        try container.encode(longestUninterruptedSeconds, forKey: .longestUninterruptedSeconds)
        try container.encode(switchAwayCount, forKey: .switchAwayCount)
        try container.encode(recoveryCount, forKey: .recoveryCount)
        try container.encode(confidence, forKey: .confidence)
        try container.encode(coverage, forKey: .coverage)
        try container.encode(coverageRatio, forKey: .coverageRatio)
        try container.encode(safeEvidenceCategory, forKey: .safeEvidenceCategory)
        try container.encode(observation, forKey: .observation)
        try container.encode(nextAction, forKey: .nextAction)
        if !dndOutcomes.isEmpty {
            try container.encode(dndOutcomes, forKey: .dndOutcomes)
        }
        try container.encodeIfPresent(reconciliation, forKey: .reconciliation)
    }
}

public struct WorkBlockSnapshot: Codable, Equatable, Sendable {
    public let stateVersion: Int
    public let phase: WorkBlockPhase
    public let blockID: UUID?
    public let intention: String?
    public let purpose: WorkBlockPurpose?
    public let intensity: WorkBlockIntensity?
    public let plannedDurationSeconds: Int
    public let elapsedDurationSeconds: Int
    public let remainingDurationSeconds: Int
    public let startedAt: Date?
    public let endsAt: Date?
    public let pausedAt: Date?
    public let recoveredAfterRestart: Bool
    public let currentCategory: String?
    public let classificationStatus: ClassificationStatus
    public let confidence: ClassificationConfidence
    public let statusLine: String
    public let result: WorkBlockResult?
    /// Present only while a drift offer is unanswered.
    public let activeIntervention: ActiveIntervention?

    private enum CodingKeys: String, CodingKey {
        case phase, intention, purpose, intensity, confidence, result
        case stateVersion = "state_version"
        case blockID = "block_id"
        case plannedDurationSeconds = "planned_duration_seconds"
        case elapsedDurationSeconds = "elapsed_duration_seconds"
        case remainingDurationSeconds = "remaining_duration_seconds"
        case startedAt = "started_at"
        case endsAt = "ends_at"
        case pausedAt = "paused_at"
        case recoveredAfterRestart = "recovered_after_restart"
        case currentCategory = "current_category"
        case classificationStatus = "classification_status"
        case statusLine = "status_line"
        case activeIntervention = "active_intervention"
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
