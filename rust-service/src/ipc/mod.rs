//! Unix domain socket IPC contract and server interfaces.
//!
//! This module owns newline-delimited JSON framing, version negotiation, and
//! message dispatch. It does not own event capture, abstraction, persistence,
//! cloud upload, or UI rendering.

#![allow(async_fn_in_trait)]

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// Starts and stops the local IPC server.
///
/// Implementors bind the configured Unix domain socket, require a successful
/// handshake before accepting raw events, dispatch only [`InboundMessage`]
/// variants, and emit only [`OutboundMessage`] variants. They must remove only
/// a confirmed stale socket file and must never log decoded message content.
pub trait IpcServer {
    /// Runs the server at `socket_path` until shutdown is requested.
    async fn run(&self, socket_path: &Path, protocol_version: u32) -> Result<(), IpcError>;

    /// Requests a clean server shutdown.
    async fn shutdown(&self) -> Result<(), IpcError>;
}

/// Encodes and decodes one newline-delimited JSON frame.
///
/// Implementors must preserve the exact snake_case field names defined in
/// `proto/schema/`, append exactly one `\n` when encoding, reject undeclared
/// fields, and reject frames that are not valid UTF-8 JSON objects.
pub trait IpcMessageCodec {
    /// Decodes one complete client-to-server JSON line without the delimiter.
    fn decode_inbound(&self, frame: &[u8]) -> Result<InboundMessage, IpcError>;

    /// Encodes one server-to-client message and appends one newline delimiter.
    fn encode_outbound(&self, message: &OutboundMessage) -> Result<Vec<u8>, IpcError>;
}

/// Negotiates the protocol before any other inbound message is dispatched.
pub trait HandshakeNegotiator {
    /// Accepts an exact version match or returns a rejected response.
    fn negotiate(
        &self,
        request: &HandshakeRequest,
        server_protocol_version: u32,
    ) -> HandshakeResponse;
}

/// Client-to-server messages accepted by the Rust service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboundMessage {
    /// Client protocol negotiation request.
    HandshakeRequest(HandshakeRequest),
    /// Raw local activity event.
    RawEvent(RawEvent),
    /// Typed error envelope.
    ErrorResponse(ErrorResponse),
}

/// Server-to-client messages emitted by the Rust service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboundMessage {
    /// Server protocol negotiation response.
    HandshakeResponse(HandshakeResponse),
    /// Raw event receipt acknowledgement.
    RawEventAck(RawEventAck),
    /// Ready-to-display insight.
    InsightPayload(InsightPayload),
    /// Ready-to-display history.
    HistoryPayload(HistoryPayload),
    /// Service health update.
    ServiceStatus(ServiceStatus),
    /// Typed error envelope.
    ErrorResponse(ErrorResponse),
}

/// Swift client's first message on a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Protocol version supported by the client.
    pub protocol_version: u32,
    /// Semantic version of the client.
    pub client_version: String,
}

/// Server response to protocol negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Whether the requested protocol version is accepted.
    pub accepted: bool,
    /// Protocol version supported by the server.
    pub server_protocol_version: u32,
    /// Safe reason supplied only when the request is rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

/// Local-only raw activity event accepted from Swift.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawEventStatus {
    /// The event was accepted.
    Accepted,
    /// The event was dropped.
    Dropped,
}

/// Ready-to-display daily insight.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPayload {
    /// Number of days requested.
    pub days: u32,
    /// Daily summary records.
    pub summaries: Vec<DailySummary>,
}

/// One privacy-safe daily summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    /// Summary is available.
    Ready,
    /// No data is available.
    NoData,
}

/// Service health notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Current service state.
    pub state: ServiceState,
    /// Optional safe diagnostic reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Service health state.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Typed IPC error envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Machine-readable snake_case error code.
    pub code: String,
    /// Human-readable safe error message.
    pub message: String,
    /// Optional related raw event identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_event_id: Option<Uuid>,
}

/// Errors produced by IPC transport or protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Socket transport failed.
    #[error("IPC transport failed")]
    Transport,
    /// Message framing or decoding failed.
    #[error("IPC message is malformed")]
    MalformedMessage,
    /// Client and server protocol versions do not match.
    #[error("unsupported IPC protocol version")]
    UnsupportedProtocolVersion,
}
