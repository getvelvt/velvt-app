//! Typed data-transfer objects for the Velvt local IPC contract.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current breaking-change version of the local IPC contract.
pub const PROTOCOL_VERSION: u32 = 11;

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
    /// Swift requests account creation with email/password credentials.
    SignUp(SignUp),
    /// Swift requests login with email/password credentials.
    LogIn(LogIn),
    /// Host client provides locally persisted auth for this service process.
    AuthSession(AuthSession),
    /// Fire-and-forget notification that the client cleared its local session.
    LogOut(LogOut),
    /// Swift requests permanent account deletion.
    DeleteAccount(DeleteAccount),
    /// Swift requests privacy-safe local/cloud status for the menu popover.
    RequestMenuStatus(RequestMenuStatus),
    /// Swift requests that the service flush its upload queue.
    FlushUploadQueue(FlushUploadQueue),
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
    /// Sent to all connected clients during graceful service shutdown.
    ShuttingDown(ShuttingDown),
    /// Account creation or login succeeded.
    AuthSuccess(AuthSuccess),
    /// Auth tokens changed; host client must persist this session locally.
    AuthSessionUpdated(AuthSession),
    /// Account creation or login failed.
    AuthFailure(AuthFailure),
    /// Confirms permanent account deletion was accepted and processed.
    AccountDeletionAccepted(AccountDeletionAccepted),
    /// Pushed when the session expires and cannot be refreshed.
    NeedsReauth(NeedsReauth),
    /// Pushed when the device registration is permanently revoked.
    DeviceRevoked(DeviceRevoked),
    /// A ready-to-schedule notification pushed after a fresh daily insight fetch.
    NotificationPayload(NotificationPayload),
    /// Privacy-safe menu-bar settings data.
    MenuStatus(MenuStatus),
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
    /// Personalized baseline state from the backend, e.g. `early_stage`,
    /// `mature`, or `no_data`.
    pub baseline_status: String,
    /// Comparison against the user's rolling baseline. Mature summaries include
    /// score deltas; early/no-data summaries include an explanatory status.
    pub baseline_comparison: serde_json::Value,
    /// Privacy-safe activity mix for the day. Categories are abstracted types,
    /// not raw app names.
    pub type_proportions: Vec<ActivityProportion>,
}

/// One privacy-safe category segment in a daily summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityProportion {
    pub category: String,
    pub seconds: u64,
    pub proportion: f64,
}

/// Insight confidence classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    /// No confidence value because no summary exists.
    None,
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

/// Swift requests a fresh menu-bar status snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestMenuStatus {}

/// Swift requests an explicit upload queue flush.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlushUploadQueue {}

/// One event waiting in the upload queue. `local_label` is display-only data
/// sent over the device-local Unix socket and never appears in cloud DTOs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedEventSummary {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_label: Option<String>,
    pub category: String,
    pub occurred_at: DateTime<Utc>,
}

/// Settings snapshot for the menu popover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuStatus {
    pub device_id: Option<String>,
    pub cloud_ready: bool,
    pub queued_event_count: u64,
    pub queued_events: Vec<QueuedEventSummary>,
}

/// Sent when Swift requested a payload that is not yet in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEmpty {
    /// Which payload type was requested (`"insight_payload"` or `"history_payload"`).
    pub payload_type: String,
}

/// Sent to all connected clients during a graceful service shutdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShuttingDown {
    /// Machine-readable shutdown reason (`"sigterm"` or `"sigint"`).
    pub reason: String,
}

/// Swift's request to create a new account. Direction: Swift to Rust.
///
/// PRIVACY: `password` is an opaque auth-protocol value. It must never be
/// logged, stored in SQLite, or included in any upload payload. `Debug` is
/// implemented manually below to redact it from any incidental log output.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignUp {
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for SignUp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignUp")
            .field("email", &"[redacted]")
            .field("password", &"[redacted]")
            .finish()
    }
}

/// Swift's request to authenticate an existing account. Direction: Swift to Rust.
///
/// PRIVACY: see [`SignUp`]; the same redaction rules apply.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogIn {
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for LogIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogIn")
            .field("email", &"[redacted]")
            .field("password", &"[redacted]")
            .finish()
    }
}

/// Portable auth session exchanged over IPC. The host client owns durable
/// platform storage; Rust keeps this only in memory.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSession {
    pub device_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for AuthSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthSession")
            .field("device_id", &self.device_id)
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Fire-and-forget notification that the client cleared its local session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogOut {}

/// Swift's request for permanent account deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteAccount {}

/// Successful signup/login. Direction: Rust to Swift.
///
/// PRIVACY: `access_token` and `refresh_token` are opaque auth-protocol
/// values. They must never be logged, stored in SQLite, or included in any
/// upload payload; Swift stores them in Keychain only. `Debug` is
/// implemented manually below to redact them from any incidental log output.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSuccess {
    pub user_id: String,
    pub device_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for AuthSuccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthSuccess")
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Privacy-safe signup/login failure classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthFailureCode {
    InvalidCredentials,
    NetworkError,
    ServerError,
}

/// Signup or login failed. Direction: Rust to Swift.
///
/// PRIVACY: `message` must never contain raw identifying user data, echoed
/// credentials, or tokens — it is a safe display string only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthFailure {
    pub code: AuthFailureCode,
    pub message: String,
}

/// Confirms the Rust service accepted and processed account deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountDeletionAccepted {}

/// Pushed when the session expires or the access token cannot be refreshed.
///
/// PRIVACY: `reason` is a safe diagnostic code, not user content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeedsReauth {
    pub reason: String,
}

/// Pushed when the device registration is permanently revoked.
///
/// PRIVACY: `message` is a safe display string supplied by Rust, never raw
/// identifying user data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRevoked {
    pub message: String,
}

/// A ready-to-schedule notification. Direction: Rust to Swift.
///
/// `title` and `body` are Rust-authored display copy; Swift schedules
/// exactly this content and never generates notification text itself (see
/// CONTRIBUTING.md "Notification text comes from the Rust service payload").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPayload {
    pub notification_id: Uuid,
    pub title: String,
    pub body: String,
    pub insight_date: NaiveDate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub do_not_disturb_until: Option<DateTime<Utc>>,
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

#[cfg(test)]
mod v6_auth_contract {
    use super::*;

    #[test]
    fn protocol_version_is_at_least_six() {
        let version = PROTOCOL_VERSION;
        assert!(version >= 6);
    }

    #[test]
    fn sign_up_round_trips_and_matches_schema_shape() {
        let message = ClientMessage::SignUp(SignUp {
            email: "user@example.com".into(),
            password: "hunter2".into(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"sign_up","payload":{"email":"user@example.com","password":"hunter2"}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn log_in_round_trips_and_matches_schema_shape() {
        let message = ClientMessage::LogIn(LogIn {
            email: "user@example.com".into(),
            password: "hunter2".into(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"log_in","payload":{"email":"user@example.com","password":"hunter2"}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn log_out_and_delete_account_have_empty_payloads() {
        let log_out = serde_json::to_string(&ClientMessage::LogOut(LogOut {})).unwrap();
        assert_eq!(log_out, r#"{"type":"log_out","payload":{}}"#);
        let delete_account =
            serde_json::to_string(&ClientMessage::DeleteAccount(DeleteAccount {})).unwrap();
        assert_eq!(delete_account, r#"{"type":"delete_account","payload":{}}"#);
    }

    #[test]
    fn auth_success_debug_redacts_tokens_but_keeps_user_id() {
        let success = AuthSuccess {
            user_id: "user-123".into(),
            device_id: "device-1".into(),
            access_token: "secret-access".into(),
            refresh_token: "secret-refresh".into(),
            expires_at: Utc::now(),
        };
        let output = format!("{success:?}");
        assert!(output.contains("user-123"));
        assert!(!output.contains("secret-access"));
        assert!(!output.contains("secret-refresh"));
    }

    #[test]
    fn sign_up_and_log_in_debug_redact_credentials() {
        let sign_up = SignUp {
            email: "user@example.com".into(),
            password: "hunter2".into(),
        };
        let log_in = LogIn {
            email: "user@example.com".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{sign_up:?}").contains("hunter2"));
        assert!(!format!("{log_in:?}").contains("hunter2"));
    }

    #[test]
    fn auth_failure_round_trips() {
        let message = ServerMessage::AuthFailure(AuthFailure {
            code: AuthFailureCode::InvalidCredentials,
            message: "invalid email or password".into(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"auth_failure","payload":{"code":"invalid_credentials","message":"invalid email or password"}}"#
        );
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn device_revoked_and_needs_reauth_round_trip() {
        let revoked = ServerMessage::DeviceRevoked(DeviceRevoked {
            message: "This device was removed from your account.".into(),
        });
        let decoded: ServerMessage =
            serde_json::from_str(&serde_json::to_string(&revoked).unwrap()).unwrap();
        assert_eq!(decoded, revoked);

        let needs_reauth = ServerMessage::NeedsReauth(NeedsReauth {
            reason: "refresh_token_expired".into(),
        });
        let decoded: ServerMessage =
            serde_json::from_str(&serde_json::to_string(&needs_reauth).unwrap()).unwrap();
        assert_eq!(decoded, needs_reauth);
    }

    #[test]
    fn notification_payload_round_trips_and_matches_schema_shape() {
        let message = ServerMessage::NotificationPayload(NotificationPayload {
            notification_id: Uuid::nil(),
            title: "Your Velvt insight is ready".into(),
            body: "Today was a focused day.".into(),
            insight_date: NaiveDate::from_ymd_opt(2026, 6, 16).unwrap(),
            do_not_disturb_until: None,
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"notification_payload","payload":{"notification_id":"00000000-0000-0000-0000-000000000000","title":"Your Velvt insight is ready","body":"Today was a focused day.","insight_date":"2026-06-16"}}"#
        );
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }
}
