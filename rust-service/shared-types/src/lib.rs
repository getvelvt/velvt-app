//! Typed data-transfer objects for the Velvt local IPC contract.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current breaking-change version of the local IPC contract.
pub const PROTOCOL_VERSION: u32 = 24;

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
    /// Saves a local personal override and syncs an uploaded historical event.
    CorrectEventClassification(CorrectEventClassification),
    /// Edits a persisted device-local rule after its source event leaves the queue.
    UpdateClassificationOverride(UpdateClassificationOverride),
    /// Requests one bounded page of persisted device-local rules.
    RequestCorrectionHistory(RequestCorrectionHistory),
    /// Removes one device-local personal rule.
    RemoveClassificationOverride(RemoveClassificationOverride),
    /// Removes every device-local personal rule.
    ResetClassificationOverrides(ResetClassificationOverrides),
    /// Starts one bounded, device-local meaningful-work block.
    StartWorkBlock(StartWorkBlock),
    /// Pauses the current work block.
    PauseWorkBlock(PauseWorkBlock),
    /// Resumes the current paused work block.
    ResumeWorkBlock(ResumeWorkBlock),
    /// Ends the current work block before its planned deadline.
    EndWorkBlock(EndWorkBlock),
    /// Requests the current or most recent local work-block state.
    RequestWorkBlockState(RequestWorkBlockState),
    /// Requests the bounded, local-only live dashboard window.
    RequestLocalDashboard(RequestLocalDashboard),
    /// Accepts the one bounded recovery action offered by a terminal result.
    AcceptWorkBlockRecovery(AcceptWorkBlockRecovery),
    /// Reports the user's explicit response to an in-session drift offer.
    ReportInterventionOutcome(ReportInterventionOutcome),
    /// Reports an OS lifecycle boundary relevant to honest elapsed time.
    WorkBlockLifecycle(WorkBlockLifecycle),
    /// Clears local work-block state, observations, results, and intention text.
    ClearWorkBlockData(ClearWorkBlockData),
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
    /// One bounded page of persisted device-local rules.
    CorrectionHistoryPage(CorrectionHistoryPage),
    /// Current or most recent device-local work-block state.
    WorkBlockState(WorkBlockSnapshot),
    /// Bounded, local-only live dashboard data.
    LocalDashboard(LocalDashboardSnapshot),
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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawEvent {
    /// Stable event identifier.
    pub event_id: Uuid,
    /// UTC time at which the event occurred.
    pub occurred_at: DateTime<Utc>,
    /// Locally measured dwell interval, bounded to avoid treating an
    /// unattended machine as active.
    pub duration_seconds: u64,
    /// Raw application name; local-only.
    pub app_name: String,
    /// Raw focused-window title; local-only.
    pub window_title: String,
    /// Optional raw application bundle identifier; local-only.
    pub bundle_id: Option<String>,
    /// Optional raw focused browser URL; local-only and consumed at the Rust
    /// privacy boundary before any event persistence or upload construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_document_url: Option<String>,
}

impl std::fmt::Debug for RawEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawEvent")
            .field("event_id", &self.event_id)
            .field("occurred_at", &self.occurred_at)
            .field("duration_seconds", &self.duration_seconds)
            .field("app_name", &"[redacted]")
            .field("window_title", &"[redacted]")
            .field("bundle_id", &self.bundle_id.as_ref().map(|_| "[redacted]"))
            .finish()
    }
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
    /// Exact reviewed aggregate evidence used to render the insight.
    pub evidence: InsightEvidence,
    /// Confidence classification.
    pub confidence_level: ConfidenceLevel,
    /// Whether low-confidence treatment is required.
    pub low_confidence: bool,
    /// UTC generation timestamp.
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightEvidence {
    pub observation: String,
    pub comparison: String,
    pub suggested_action: String,
    pub tone_stage: EmotionalStage,
    pub observation_type: String,
    pub template_id: String,
    pub metric_value: i64,
    pub metric_unit: String,
    pub time_window: serde_json::Value,
    pub safe_categories: Vec<String>,
    pub confidence: String,
    pub coverage: f64,
    pub baseline_status: String,
    pub baseline_comparison: serde_json::Value,
    pub action_minutes: u32,
    pub repetition_days: u32,
    pub next_action_id: String,
    pub direction: String,
    pub magnitude: f64,
}

impl Default for InsightEvidence {
    fn default() -> Self {
        Self {
            observation: "Evidence unavailable".into(),
            comparison: "Baseline comparison unavailable".into(),
            suggested_action: "Protect one realistic work block".into(),
            tone_stage: EmotionalStage::Early,
            observation_type: "unavailable".into(),
            template_id: "unavailable".into(),
            metric_value: 0,
            metric_unit: "none".into(),
            time_window: serde_json::json!({}),
            safe_categories: vec![],
            confidence: "none".into(),
            coverage: 0.0,
            baseline_status: "unknown".into(),
            baseline_comparison: serde_json::json!({}),
            action_minutes: 0,
            repetition_days: 0,
            next_action_id: "unavailable".into(),
            direction: "stable".into(),
            magnitude: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmotionalStage {
    Early,
    Stable,
    PositiveDeviation,
    SustainedPositiveTrend,
    NegativeDeviation,
    RepeatedNegativeTrend,
    SustainedHighConfidenceDecline,
    Recovery,
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
    /// Aggregate time in broad focus-oriented work lanes.
    pub focused_seconds: u64,
    /// Changes between broad work lanes, excluding same-lane activity.
    pub meaningful_switch_count: u64,
    /// Longest recorded work session without a broad-lane switch.
    pub longest_uninterrupted_seconds: u64,
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

/// Quality status for one captured event's local classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationStatus {
    Classified,
    Ambiguous,
    Unclassified,
}

impl ClassificationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classified => "classified",
            Self::Ambiguous => "ambiguous",
            Self::Unclassified => "unclassified",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationConfidence {
    High,
    Medium,
    Low,
    None,
}

impl ClassificationConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationSource {
    Seed,
    Heuristic,
    Embedding,
    UserRule,
    Fallback,
}

impl ClassificationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Heuristic => "heuristic",
            Self::Embedding => "embedding",
            Self::UserRule => "user_rule",
            Self::Fallback => "fallback",
        }
    }
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

/// User-selected correction for one locally known event.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectEventClassification {
    pub event_id: Uuid,
    pub stable_id: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_activity_name: Option<String>,
}

/// Device-local edit of a persisted rule. No cloud request is made.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateClassificationOverride {
    pub stable_id: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_activity_name: Option<String>,
}

impl std::fmt::Debug for UpdateClassificationOverride {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UpdateClassificationOverride")
            .field("stable_id", &"[local_identifier]")
            .field("category", &self.category)
            .field(
                "local_activity_name",
                &self.local_activity_name.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// Search text is local-only because it may contain an activity alias.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestCorrectionHistory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub offset: u32,
    pub page_size: u32,
}

impl std::fmt::Debug for RequestCorrectionHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestCorrectionHistory")
            .field("query", &self.query.as_ref().map(|_| "[redacted]"))
            .field("offset", &self.offset)
            .field("page_size", &self.page_size)
            .finish()
    }
}

impl std::fmt::Debug for CorrectEventClassification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CorrectEventClassification")
            .field("event_id", &self.event_id)
            .field("stable_id", &"[local_identifier]")
            .field("category", &self.category)
            .field(
                "local_activity_name",
                &self.local_activity_name.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveClassificationOverride {
    pub stable_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetClassificationOverrides {}

/// Version of the persisted and wire-visible work-block state machine.
pub const WORK_BLOCK_STATE_VERSION: u32 = 1;

/// Optional coarse purpose selected directly by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockPurpose {
    DeepWork,
    Study,
    CreativePractice,
    HealthyTechUse,
    WorkLifeBoundary,
}

impl WorkBlockPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepWork => "deep_work",
            Self::Study => "study",
            Self::CreativePractice => "creative_practice",
            Self::HealthyTechUse => "healthy_tech_use",
            Self::WorkLifeBoundary => "work_life_boundary",
        }
    }
}

/// User-selected feedback intensity. Every mode uses the same non-shaming
/// evidence rules; this value only selects bounded reviewed copy/cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockIntensity {
    Light,
    Medium,
    Intense,
}

impl WorkBlockIntensity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Intense => "intense",
        }
    }
}

/// Persisted state-machine phases. `Expired` is used only when a restart or
/// clock discontinuity makes an on-time completion impossible to establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockPhase {
    Idle,
    Active,
    Paused,
    Completed,
    Abandoned,
    Expired,
}

impl WorkBlockPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
            Self::Expired => "expired",
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartWorkBlock {
    /// Free-form intention. Device-local only; all Debug output is redacted.
    pub intention: Option<String>,
    pub planned_duration_seconds: u32,
    pub purpose: Option<WorkBlockPurpose>,
    pub intensity: WorkBlockIntensity,
}

impl std::fmt::Debug for StartWorkBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartWorkBlock")
            .field("intention", &self.intention.as_ref().map(|_| "[redacted]"))
            .field("planned_duration_seconds", &self.planned_duration_seconds)
            .field("purpose", &self.purpose)
            .field("intensity", &self.intensity)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseWorkBlock {
    pub block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeWorkBlock {
    pub block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndWorkBlock {
    pub block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestWorkBlockState {}

/// Requests one bounded local dashboard window. The service caps this value
/// so the UI cannot accidentally turn a live view into a history scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestLocalDashboard {
    pub window_seconds: u32,
    /// Current local UTC offset supplied by Swift so Rust can produce exactly
    /// seven bounded local-calendar rows without receiving locale or identity.
    pub utc_offset_seconds: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptWorkBlockRecovery {
    pub block_id: Uuid,
    pub action_id: String,
}

/// The user's explicit response to an in-session drift offer.
///
/// Only responses a person can actually give are representable. Silence is not
/// in this set: it is recorded by the service when the block ends, never
/// inferred from a notification disappearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionResponse {
    /// Took the offered action.
    AcceptedAction,
    /// Did not help. The user is not disputing that drift occurred.
    NotHelpful,
    /// The underlying classification was wrong. Evidence against the detector.
    WrongClassification,
    /// Explicitly dismissed.
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportInterventionOutcome {
    pub block_id: Uuid,
    pub response: InterventionResponse,
}

/// A live drift offer, rendered in-app. Present only while unanswered.
///
/// The in-app surface is the primary path: it always renders, whereas an OS
/// notification depends on authorization and is suppressed by Focus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveIntervention {
    pub action_id: String,
    /// Rust-authored display copy. Swift renders it verbatim.
    pub title: String,
    pub body: String,
    /// Broad taxonomy category. Never app identity, a title, or a URL.
    pub anchor_category: String,
    pub switch_count: u32,
    pub window_seconds: u32,
    pub offered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockLifecycleEvent {
    Sleep,
    Wake,
    ClockChanged,
    TimeZoneChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBlockLifecycle {
    pub event: WorkBlockLifecycleEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearWorkBlockData {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockCoverage {
    Insufficient,
    Partial,
    Good,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBlockNextAction {
    pub action_id: String,
    pub label: String,
    pub duration_seconds: u32,
}

/// Safe local result consumed by Scope 3. It deliberately has no intention,
/// app identity, title, URL, local label, notification ID, or cloud ID field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBlockResult {
    pub planned_duration_seconds: u32,
    pub elapsed_duration_seconds: u32,
    pub longest_uninterrupted_seconds: u32,
    pub switch_away_count: u32,
    pub recovery_count: u32,
    pub confidence: ConfidenceLevel,
    pub coverage: WorkBlockCoverage,
    pub coverage_ratio: f64,
    pub safe_evidence_category: Option<String>,
    pub observation: String,
    /// Singular by construction: every result offers exactly one action.
    pub next_action: WorkBlockNextAction,
}

/// Rust-authored, ready-to-render state. Swift may render and issue direct
/// commands but must not recompute evidence or author behavioral claims.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBlockSnapshot {
    pub state_version: u32,
    pub phase: WorkBlockPhase,
    pub block_id: Option<Uuid>,
    pub intention: Option<String>,
    pub purpose: Option<WorkBlockPurpose>,
    pub intensity: Option<WorkBlockIntensity>,
    pub planned_duration_seconds: u32,
    pub elapsed_duration_seconds: u32,
    pub remaining_duration_seconds: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub analysis_ended_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub paused_at: Option<DateTime<Utc>>,
    pub recovered_after_restart: bool,
    pub current_category: Option<String>,
    pub classification_status: ClassificationStatus,
    pub confidence: ClassificationConfidence,
    pub status_line: String,
    pub result: Option<WorkBlockResult>,
    /// Set only while a drift offer is awaiting a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_intervention: Option<ActiveIntervention>,
}

/// Coverage state for the local live dashboard window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDashboardCoverage {
    NoData,
    Partial,
    Good,
}

/// One safe category segment in the bounded live timeline. It contains no
/// application, window, URL, local label, or intention text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTimelineSegment {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub category: String,
    pub confidence: ClassificationConfidence,
}

/// One deduplicated movement between classified, non-system categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTransitionMarker {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub from_category: String,
    pub to_category: String,
    pub confidence: ClassificationConfidence,
}

/// A deterministic group of overlapping transition windows. Rule version 1
/// means at least three category transitions inside an inclusive five minutes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSwitchingCluster {
    pub id: String,
    pub rule_version: u32,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub transition_count: u32,
    pub categories: Vec<String>,
    pub confidence: ClassificationConfidence,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalComparisonKind {
    EarlierToday,
    SevenDayPattern,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalFocusComparison {
    pub kind: LocalComparisonKind,
    pub label: String,
    pub switch_delta: i32,
    pub explanation: String,
}

/// The only session-analysis surface. It exists only for an explicit current
/// or most-recent work block and is fully derived inside Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalFocusFragmentation {
    pub block_id: Uuid,
    pub phase: WorkBlockPhase,
    pub window_label: String,
    pub window_started_at: DateTime<Utc>,
    pub window_ended_at: DateTime<Utc>,
    pub planned_duration_seconds: u32,
    pub elapsed_duration_seconds: u32,
    pub longest_uninterrupted_seconds: u64,
    pub observed_switch_count: u32,
    pub recovery_count: u32,
    pub coverage: LocalDashboardCoverage,
    pub coverage_ratio: f64,
    pub comparison: Option<LocalFocusComparison>,
    pub observation: String,
    pub next_action: String,
    pub segments: Vec<LocalTimelineSegment>,
    pub transitions: Vec<LocalTransitionMarker>,
    pub clusters: Vec<LocalSwitchingCluster>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDailyActivityState {
    NoData,
    LowConfidence,
    Ready,
    StillBuilding,
}

/// One locally aggregated display-label bucket. `label` may cross only local
/// IPC and is structurally absent from every cloud/upload DTO.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDailyActivitySegment {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representative_event_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
    pub alias_confirmed: bool,
    pub category: String,
    pub duration_seconds: u64,
    pub percentage: u32,
    pub confidence: ClassificationConfidence,
    pub explanation: Option<String>,
}

impl std::fmt::Debug for LocalDailyActivitySegment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalDailyActivitySegment")
            .field("id", &self.id)
            .field("label", &"[local_display_label]")
            .field(
                "representative_event_id",
                &self
                    .representative_event_id
                    .as_ref()
                    .map(|_| "[local_identifier]"),
            )
            .field(
                "stable_id",
                &self.stable_id.as_ref().map(|_| "[local_identifier]"),
            )
            .field(
                "suggested_name",
                &self.suggested_name.as_ref().map(|_| "[redacted]"),
            )
            .field("alias_confirmed", &self.alias_confirmed)
            .field("category", &self.category)
            .field("duration_seconds", &self.duration_seconds)
            .field("percentage", &self.percentage)
            .field("confidence", &self.confidence)
            .field(
                "explanation",
                &self.explanation.as_ref().map(|_| "[reviewed_copy]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDailyActivityDay {
    pub id: String,
    pub date: NaiveDate,
    pub state: LocalDailyActivityState,
    pub active_seconds: u64,
    pub coverage: LocalDashboardCoverage,
    pub segments: Vec<LocalDailyActivitySegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEarlySignalStatus {
    InsufficientEvidence,
    Ready,
}

/// A bounded, Rust-authored summary of privacy-safe local evidence. Copy is
/// deliberately observational: no productivity score, causal claim, raw
/// activity, or inferred intention can be represented by this contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalEarlySignal {
    pub status: LocalEarlySignalStatus,
    pub observed_from: Option<DateTime<Utc>>,
    pub observed_through: DateTime<Utc>,
    pub observed_seconds: u64,
    pub required_seconds: u64,
    pub evidence_event_count: u32,
    pub focused_seconds: u64,
    pub meaningful_switch_count: u32,
    pub longest_uninterrupted_seconds: u64,
    pub observation: Option<String>,
    pub suggested_action: Option<String>,
    pub action_minutes: u32,
}

/// Rust-authored local dashboard data. Swift renders this payload and does not
/// scan event history or calculate competing metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDashboardSnapshot {
    pub generated_at: DateTime<Utc>,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub switch_count: u32,
    pub switches_per_hour: f64,
    pub coverage: LocalDashboardCoverage,
    pub early_signal: LocalEarlySignal,
    pub segments: Vec<LocalTimelineSegment>,
    pub focus_fragmentation: Option<LocalFocusFragmentation>,
    pub daily_activity: Vec<LocalDailyActivityDay>,
}

impl std::fmt::Debug for WorkBlockSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkBlockSnapshot")
            .field("state_version", &self.state_version)
            .field("phase", &self.phase)
            .field("block_id", &self.block_id)
            .field("intention", &self.intention.as_ref().map(|_| "[redacted]"))
            .field("purpose", &self.purpose)
            .field("intensity", &self.intensity)
            .field("planned_duration_seconds", &self.planned_duration_seconds)
            .field("elapsed_duration_seconds", &self.elapsed_duration_seconds)
            .field(
                "remaining_duration_seconds",
                &self.remaining_duration_seconds,
            )
            .field("started_at", &self.started_at)
            .field("analysis_ended_at", &self.analysis_ended_at)
            .field("ends_at", &self.ends_at)
            .field("paused_at", &self.paused_at)
            .field("recovered_after_restart", &self.recovered_after_restart)
            .field("current_category", &self.current_category)
            .field("classification_status", &self.classification_status)
            .field("confidence", &self.confidence)
            .field("status_line", &"[reviewed_copy]")
            .field("result", &self.result)
            .finish()
    }
}

/// One event waiting in the upload queue. `local_label` is display-only data
/// sent over the device-local Unix socket and never appears in cloud DTOs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedEventSummary {
    pub event_id: Uuid,
    pub stable_id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_label: Option<String>,
    pub category: String,
    pub classification_tier: String,
    pub classification_status: ClassificationStatus,
    pub classification_confidence: ClassificationConfidence,
    pub classification_source: ClassificationSource,
    pub occurred_at: DateTime<Utc>,
}

/// One persisted personal rule. Local labels are redacted from Debug output
/// and never enter any cloud DTO.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassificationCorrectionSummary {
    pub stable_id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_label: Option<String>,
    pub category: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionHistoryPage {
    pub items: Vec<ClassificationCorrectionSummary>,
    pub offset: u32,
    pub page_size: u32,
    pub total_count: u64,
    pub has_more: bool,
}

impl std::fmt::Debug for CorrectionHistoryPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CorrectionHistoryPage")
            .field("item_count", &self.items.len())
            .field("offset", &self.offset)
            .field("page_size", &self.page_size)
            .field("total_count", &self.total_count)
            .field("has_more", &self.has_more)
            .finish()
    }
}

impl std::fmt::Debug for ClassificationCorrectionSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClassificationCorrectionSummary")
            .field("stable_id", &"[local_identifier]")
            .field("label", &self.label)
            .field(
                "local_label",
                &self.local_label.as_ref().map(|_| "[redacted]"),
            )
            .field("category", &self.category)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Settings snapshot for the menu popover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MenuStatus {
    pub device_id: Option<String>,
    pub cloud_ready: bool,
    pub upload_status: String,
    pub last_upload_error_code: Option<String>,
    pub next_upload_attempt_at: Option<DateTime<Utc>>,
    pub last_successful_sync_at: Option<DateTime<Utc>>,
    pub pending_upload_batch_count: u64,
    pub failed_upload_batch_count: u64,
    pub rejected_upload_batch_count: u64,
    pub queued_event_count: u64,
    pub queued_events: Vec<QueuedEventSummary>,
    pub correction_history: Vec<ClassificationCorrectionSummary>,
}

/// Sent when Swift requested a payload that is not yet in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEmpty {
    /// Which payload type was requested (`"insight_payload"` or `"history_payload"`).
    pub payload_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for AuthSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthSession")
            .field("device_id", &self.device_id)
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field(
                "user_access_token",
                &self.user_access_token.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "user_refresh_token",
                &self.user_refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("user_expires_at", &self.user_expires_at)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_expires_at: Option<DateTime<Utc>>,
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
            .field(
                "user_access_token",
                &self.user_access_token.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "user_refresh_token",
                &self.user_refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("user_expires_at", &self.user_expires_at)
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
            user_access_token: Some("secret-user-access".into()),
            user_refresh_token: Some("secret-user-refresh".into()),
            user_expires_at: Some(Utc::now()),
        };
        let output = format!("{success:?}");
        assert!(output.contains("user-123"));
        assert!(!output.contains("secret-access"));
        assert!(!output.contains("secret-refresh"));
        assert!(!output.contains("secret-user-access"));
        assert!(!output.contains("secret-user-refresh"));
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
