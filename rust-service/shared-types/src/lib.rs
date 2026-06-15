//! Typed data-transfer objects for the Velvt local IPC contract.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current breaking-change version of the local IPC contract.
pub const PROTOCOL_VERSION: u32 = 4;

/// Client-to-server messages accepted by the Rust service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientMessage {
    /// Client response to the server's initial hello.
    ClientHello(ClientHello),
    /// Raw local activity event.
    RawEvent(RawEvent),
    /// Typed error envelope.
    ErrorResponse(ErrorResponse),
    /// Swift requests the cached insight for a specific date.
    RequestLatestInsight(RequestLatestInsight),
    /// Swift requests the cached history summary for the last N days.
    RequestLatestHistory(RequestLatestHistory),
    /// Test-only proof that adding a client DTO does not change existing handlers.
    #[cfg(any(test, feature = "extensibility-proof"))]
    DummyExtension(DummyExtension),
}

/// Test-only payload used to prove tagged-enum extensibility.
#[cfg(any(test, feature = "extensibility-proof"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DummyExtension {
    /// Arbitrary proof sequence.
    pub sequence: u32,
}

/// Server-to-client messages emitted by the Rust service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Server's initial protocol declaration.
    ServerHello(ServerHello),
    /// Successful handshake acknowledgement.
    Acknowledged(Acknowledged),
    /// Protocol versions are incompatible.
    VersionMismatch(VersionMismatch),
    /// A client frame could not be decoded as a valid typed message.
    MalformedMessage(MalformedMessage),
    /// Raw event receipt acknowledgement.
    RawEventAck(RawEventAck),
    /// Ready-to-display insight.
    InsightPayload(InsightPayload),
    /// Ready-to-display history.
    HistoryPayload(HistoryPayload),
    /// Service health update.
    ServiceStatus(ServiceStatus),
    /// Terminal cloud privacy rejection alert.
    PrivacyViolationAlert(PrivacyViolationAlert),
    /// Typed error envelope.
    ErrorResponse(ErrorResponse),
    /// The requested payload has no cached entry; `payload_type` names what was requested.
    CacheEmpty(CacheEmpty),
}

/// Server's first message on every connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerHello {
    /// Protocol version supported by the server.
    pub protocol_version: u32,
}

/// Swift client's response to a server hello.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHello {
    /// Protocol version expected by the client.
    pub expected_protocol_version: u32,
    /// Semantic version of the client.
    pub client_version: String,
}

/// Confirms that protocol negotiation succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledged;

/// Reports incompatible client and server protocol versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionMismatch {
    /// Protocol version supported by the server.
    pub server_protocol_version: u32,
    /// Protocol version declared by the client.
    pub client_protocol_version: u32,
}

/// Reports a rejected frame without echoing client-supplied content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MalformedMessage {
    /// Stable, privacy-safe malformed-message classification.
    pub code: MalformedMessageCode,
}

/// Privacy-safe malformed-message classifications.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MalformedMessageCode {
    /// The frame was not a valid declared client message.
    InvalidMessage,
}

/// Local-only raw activity event accepted from Swift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEvent {
    /// Stable event identifier.
    pub event_id: Uuid,
    /// UTC time at which the event occurred.
    pub occurred_at: DateTime<Utc>,
    /// Raw application name; local-only.
    pub app_name: String,
    /// Raw focused-window title; local-only.
    pub window_title: String,
    /// Optional raw application bundle identifier; local-only.
    pub bundle_id: Option<String>,
}

/// Acknowledgement for one raw event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEventAck {
    /// Event identifier being acknowledged.
    pub event_id: Uuid,
    /// Receipt outcome.
    pub status: RawEventStatus,
    /// Safe reason supplied only when the event is dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_reason: Option<String>,
}

/// Raw event receipt outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawEventStatus {
    /// The event was accepted.
    Accepted,
    /// The event was dropped.
    Dropped,
}

/// Ready-to-display daily insight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightPayload {
    /// Calendar date covered by the insight.
    pub date: NaiveDate,
    /// Ready-to-display insight copy.
    pub text: String,
    /// Confidence classification.
    pub confidence_level: ConfidenceLevel,
    /// Whether low-confidence treatment is required.
    pub low_confidence: bool,
    /// UTC generation timestamp.
    pub generated_at: DateTime<Utc>,
}

/// Ready-to-display history summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryPayload {
    /// Number of days requested.
    pub days: u32,
    /// Daily summary records.
    pub summaries: Vec<DailySummary>,
}

/// One privacy-safe daily summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailySummary {
    /// Calendar date covered by the summary.
    pub date: NaiveDate,
    /// Availability status.
    pub status: HistoryStatus,
    /// Number of abstracted events.
    pub event_count: u64,
    /// Optional derived focus score.
    pub focus_score: Option<f64>,
    /// Optional derived fragmentation score.
    pub fragmentation_score: Option<f64>,
    /// Confidence classification.
    pub confidence_level: ConfidenceLevel,
    /// Total active seconds.
    pub active_seconds: u64,
}

/// Insight confidence classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    /// Low confidence.
    Low,
    /// Medium confidence.
    Medium,
    /// High confidence.
    High,
}

/// History availability status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    /// Summary is available.
    Ready,
    /// No data is available.
    NoData,
}

/// Service health notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatus {
    /// Current service state.
    pub state: ServiceState,
    /// Optional safe diagnostic reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Terminal privacy rejection notification containing safe diagnostics only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyViolationAlert {
    pub code: String,
    pub message: String,
}

/// Service health state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Service is operating normally.
    Ready,
    /// Service has reduced functionality.
    Degraded,
    /// Cloud upload is paused.
    UploadPaused,
    /// User authentication is required.
    AuthRequired,
}

/// Swift client request for the cached insight for a specific date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestLatestInsight {
    /// Calendar date whose insight is being requested.
    pub date: NaiveDate,
}

/// Swift client request for the cached history summary for the last N days.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestLatestHistory {
    /// Number of days of history to return (1–30).
    pub days: u8,
}

/// Sent when Swift requested a payload that is not yet in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEmpty {
    /// Which payload type was requested (`"insight_payload"` or `"history_payload"`).
    pub payload_type: String,
}

/// Typed IPC error envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// Machine-readable snake_case error code.
    pub code: String,
    /// Human-readable safe error message.
    pub message: String,
    /// Optional related raw event identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_event_id: Option<Uuid>,
}

#[cfg(test)]
mod extensibility_proof {
    use super::{ClientMessage, DummyExtension};

    #[test]
    fn dummy_variant_serializes_without_service_handler_changes() {
        // The variant exists directly on the shared DTO enum; the non-exhaustive
        // service router and transports compile unchanged.
        let message = ClientMessage::DummyExtension(DummyExtension { sequence: 1 });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"dummy_extension","payload":{"sequence":1}}"#
        );
    }
}
