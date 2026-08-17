//! Typed data-transfer objects for the Velvt local IPC contract.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current breaking-change version of the local IPC contract.
pub const PROTOCOL_VERSION: u32 = 28;

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
    /// A coarse system Focus/DND transition observed by the client. Swift
    /// observes only; Rust owns the evidence record and every decision.
    FocusStateChanged(FocusStateChanged),
    /// The user's one-tap reply to a quiet-hours offer.
    RespondQuietHoursOffer(RespondQuietHoursOffer),
    /// Asks the deterministic initiation policy whether one invitation is
    /// pending right now. Every gate is owned and enforced in Rust.
    RequestInitiationInvitation(RequestInitiationInvitation),
    /// The one-tap dismissal of a live initiation invitation.
    DismissInitiationInvitation(DismissInitiationInvitation),
    /// Sets the single Rust-owned opt-out for initiation invitations.
    SetInitiationSettings(SetInitiationSettings),
    /// Reads the current Rust-owned initiation-invitation setting.
    RequestInitiationSettings(RequestInitiationSettings),
    /// Reads the current auto-demotion state of the intervention detector.
    /// The state machine and its versioned criteria are owned in Rust.
    RequestDemotionState(RequestDemotionState),
    /// The user's explicit one-tap resume from the demoted state.
    ResetInterventionDemotion(ResetInterventionDemotion),
    /// Asks whether the weekly receipts digest for the most recent completed
    /// local week is ready to show. Held during quiet hours and Focus/DND.
    RequestWeeklyDigest(RequestWeeklyDigest),
    /// The one-tap acknowledgment that closes a shown weekly digest.
    AcknowledgeWeeklyDigest(AcknowledgeWeeklyDigest),
    /// The one-tap "explain this nudge" request. Accepts no user text; there
    /// is no reply, follow-up, or thread anywhere on this surface (D7).
    RequestInterventionExplanation(RequestInterventionExplanation),
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
    /// A next-morning quiet-hours offer from the deterministic pattern rule.
    QuietHoursOffer(QuietHoursOffer),
    /// At most one daily invitation to a soft start from the deterministic,
    /// versioned good-hours policy. Schedule-free by construction.
    InitiationInvitation(InitiationInvitation),
    /// The current Rust-owned initiation-invitation setting.
    InitiationSettings(InitiationSettings),
    /// The current state of the deterministic, versioned auto-demotion
    /// policy, disclosed as a feature and inspectable on demand.
    DemotionState(DemotionState),
    /// The weekly receipts digest for one completed local week.
    WeeklyDigest(WeeklyDigest),
    /// Exactly one grounded sentence explaining a shown intervention.
    InterventionExplanation(InterventionExplanation),
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
    /// When present, claims a live initiation invitation so the accepted
    /// start is recorded with a content-free origin marker. The service
    /// validates it; a stale or unknown id is a plain manual start.
    #[serde(default)]
    pub invitation_id: Option<Uuid>,
}

impl std::fmt::Debug for StartWorkBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartWorkBlock")
            .field("intention", &self.intention.as_ref().map(|_| "[redacted]"))
            .field("planned_duration_seconds", &self.planned_duration_seconds)
            .field("purpose", &self.purpose)
            .field("intensity", &self.intensity)
            .field("invitation_id", &self.invitation_id)
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
    /// The offer itself was wrong: the user was working the whole time.
    ///
    /// Distinct from every other reply. `Dismissed` says "not now",
    /// `NotHelpful` concedes the drift, and `WrongClassification` disputes a
    /// label. Only this one says the intervention should never have fired, so
    /// it is the ground-truth false-positive stream behind the
    /// wrong-intervention rate.
    WasFocused,
    /// Explicitly dismissed.
    Dismissed,
}

/// How loudly an offer is delivered.
///
/// Salience only ever decreases. A user who has waved Velvt off recently gets
/// the in-app card and no notification; salience returns to normal only after
/// an offer lands well, never in response to continued drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionSalience {
    /// In-app card plus a local notification.
    Normal,
    /// In-app card only.
    Quiet,
}

impl InterventionSalience {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Quiet => "quiet",
        }
    }
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
    pub salience: InterventionSalience,
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

/// A coarse system Focus/DND transition observed by Swift.
///
/// PRIVACY: this message can carry only whether some Focus mode is active,
/// when the transition happened, and the client's UTC offset. The Focus
/// mode's name, configuration, and schedule are structurally
/// unrepresentable here and must never be added. Rust buckets the
/// transition time to the coarse local granularity before storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusStateChanged {
    /// Whether a system Focus/DND mode is active after the transition.
    pub active: bool,
    /// UTC time of the observed transition, as sampled by the client.
    pub occurred_at: DateTime<Utc>,
    /// Client's current UTC offset so the pattern rule can bucket to local
    /// hours without receiving locale or identity.
    pub utc_offset_seconds: i32,
}

/// The user's one-tap reply to a quiet-hours offer. Accepting configures
/// Velvt's own quiet hours; declining is remembered locally for a versioned
/// interval and changes nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RespondQuietHoursOffer {
    pub accepted: bool,
}

/// A next-morning quiet-hours offer produced by the deterministic,
/// versioned late-night DND pattern rule. An offer, never a workaround.
///
/// PRIVACY: carries only the rule version, a bounded distinct-day count,
/// the proposed local window, and Rust-authored copy. No Focus mode name,
/// schedule, or configuration is representable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHoursOffer {
    /// Version of the deterministic pattern rule that produced this offer.
    pub rule_version: u32,
    /// Distinct local days with late-night DND evidence inside the rule's
    /// lookback window.
    pub late_night_days: u32,
    /// Proposed quiet-hours start, minutes after local midnight.
    pub start_local_minutes: u32,
    /// Proposed quiet-hours end, minutes after local midnight.
    pub end_local_minutes: u32,
    /// Rust-authored display copy. Swift renders it verbatim.
    pub body: String,
}

/// Asks the deterministic initiation policy whether one invitation is
/// pending right now. Carries only the client's UTC offset so Rust can
/// evaluate local-time gates; every gate is owned and enforced in Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestInitiationInvitation {
    /// Client's current UTC offset so the good-hours policy can evaluate
    /// local-time gates without receiving locale or identity.
    pub utc_offset_seconds: i32,
}

/// At most one daily invitation to a soft start, produced by the
/// deterministic, versioned good-hours policy.
///
/// PRIVACY: schedule-free by construction. No good-hours window, weekday,
/// hour bucket, or timing evidence is representable in this payload, and
/// the registered copy carries no numbers derived from the user's schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiationInvitation {
    /// Opaque id used to accept or dismiss this invitation.
    pub invitation_id: Uuid,
    /// The registered action. Only `soft_start_25` exists.
    pub action_id: String,
    /// Rust-authored display copy. Swift renders it verbatim.
    pub body: String,
    /// Planned duration of the block one tap declares.
    pub duration_seconds: u32,
    /// Version of the deterministic good-hours policy that produced this.
    pub policy_version: u32,
}

/// The one-tap dismissal of a live initiation invitation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DismissInitiationInvitation {
    pub invitation_id: Uuid,
}

/// Sets the single Rust-owned opt-out for initiation invitations. Opting
/// out silences invitations entirely and changes nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetInitiationSettings {
    pub invitations_enabled: bool,
}

/// Reads the current Rust-owned initiation-invitation setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestInitiationSettings {}

/// The current Rust-owned initiation-invitation setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiationSettings {
    pub invitations_enabled: bool,
}

/// Reads the current auto-demotion state of the intervention detector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDemotionState {}

/// The user's explicit one-tap resume from the demoted (observe-only)
/// state. Resetting restarts the demotion evaluation window from the reset
/// instant; it never edits or discards the underlying outcome record, and
/// the wrong-intervention counter itself is untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetInterventionDemotion {}

/// The two states of the deterministic auto-demotion policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemotionStateKind {
    /// Interventions may fire through the normal gates.
    Active,
    /// Interventions are paused; Velvt observes quietly. Disclosed, never
    /// hidden.
    Demoted,
}

/// The current state of the deterministic, versioned auto-demotion policy
/// over the rolling wrong-intervention counter (roadmap invariant 4; D5).
///
/// PRIVACY: carries only the two bounded counts the rate is computed from,
/// the versioned policy constants, the current state, and Rust-authored
/// disclosure copy. No transition history, timeline, category, or copy
/// evidence is representable. Local IPC surface only — never uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemotionState {
    pub state: DemotionStateKind,
    /// `dismissed_was_focused` replies inside the evaluation window.
    pub wrong_count: u32,
    /// Interventions delivered inside the evaluation window. Suppressed and
    /// withheld decisions were never shown and are excluded.
    pub delivered_count: u32,
    /// Versioned demotion threshold, in whole percent. Demotion requires the
    /// rate to exceed this value strictly.
    pub threshold_percent: u32,
    /// Versioned minimum delivered sample below which demotion never
    /// triggers.
    pub minimum_sample: u32,
    /// Rolling evaluation window, in days.
    pub window_days: u32,
    pub threshold_policy_version: u32,
    pub repromotion_policy_version: u32,
    /// When the current demotion began. Present only while demoted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demoted_at: Option<DateTime<Utc>>,
    /// Rust-authored disclosure copy. Present only while demoted; Swift
    /// renders it verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disclosure: Option<String>,
}

/// Asks whether the weekly receipts digest for the most recent completed
/// local week is ready to show. Carries only the client's UTC offset so
/// Rust can bound the local week; generation, count sourcing, quiet-hours
/// and Focus holds, and the acknowledged flag are all owned in Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestWeeklyDigest {
    pub utc_offset_seconds: i32,
}

/// The one-tap acknowledgment that closes a shown weekly digest.
/// Bookkeeping only: it stops the card re-rendering and changes nothing
/// else. There is no reply, follow-up, or thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgeWeeklyDigest {
    /// Local Monday (`YYYY-MM-DD`) of the digest week being acknowledged.
    pub week_start_local_date: String,
}

/// The weekly receipts digest for one completed local week (D6).
///
/// Every count is read from the same stored aggregates the local metrics
/// use, exactly. One digest, not a dashboard: recoveries and completions
/// lead, the wrong-intervention count appears exactly once, and no streak,
/// chain, or failure tally is representable (D8; roadmap invariant 6).
/// Local IPC surface only — never uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyDigest {
    /// Local Monday (`YYYY-MM-DD`) of the covered week.
    pub week_start_local_date: String,
    pub blocks_declared: u32,
    pub blocks_completed: u32,
    /// Returns to the block's dominant work, summed from the stored session
    /// results of the week. The accumulating positive stat; it leads.
    pub recoveries: u32,
    /// `dismissed_was_focused` replies inside the week. Stated once,
    /// plainly.
    pub wrong_interventions: u32,
    pub invitations_accepted: u32,
    /// What Velvt chose not to send: decisions held under DND plus
    /// decisions withheld while demoted.
    pub withheld: u32,
    /// Rust-authored recovery-led headline. Swift renders it verbatim.
    pub headline: String,
    /// Version of the deterministic digest policy that produced this.
    pub digest_version: u32,
}

/// The one-tap "explain this nudge" request for the block's most recent
/// shown intervention. Accepts no user text anywhere: there is no input
/// field, reply, follow-up, or thread, and adding one is a stop condition
/// (D7). The UTC offset is used solely to bucket the coarse local weekly
/// tap count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestInterventionExplanation {
    pub block_id: Uuid,
    pub utc_offset_seconds: i32,
}

/// Exactly one grounded sentence explaining the block's most recent shown
/// intervention. Deterministic code selects the claim, evidence, and tone
/// from the stored intervention record; the sentence cannot exceed that
/// evidence, and Swift renders it verbatim with no reply affordance (D7).
/// Local IPC surface only — never uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterventionExplanation {
    pub block_id: Uuid,
    pub sentence: String,
}

/// Focus/DND-derived outcome recorded by the service for one work block.
///
/// `CompletedUnderDnd` marks a success: the block completed while DND was
/// active and counts as a completed block everywhere a completed block
/// counts. Each `DeliverySuppressedDnd` entry is one mid-block nudge held
/// because DND was active — delivered by no channel, retried against no
/// setting, and reconciled after the block as a count only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkBlockDndOutcome {
    CompletedUnderDnd,
    DeliverySuppressedDnd,
}

impl WorkBlockDndOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompletedUnderDnd => "completed_under_dnd",
            Self::DeliverySuppressedDnd => "delivery_suppressed_dnd",
        }
    }
}

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
    /// Focus/DND outcomes recorded for this block, in the order recorded.
    /// Omitted when empty so pre-v26 payloads decode unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dnd_outcomes: Vec<WorkBlockDndOutcome>,
    /// At most one Rust-authored calm post-block line noting what was held
    /// under DND. Analyst voice; never a late nudge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<String>,
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
    /// One-shot confirmation that a correction was taken, authored by Rust
    /// beside the change it describes.
    ///
    /// Present only on the status returned by a correction command; a polled
    /// status always carries `None`, so the message cannot linger or reappear.
    /// A correction the user cannot see land is indistinguishable from one that
    /// was ignored, which is what makes people stop correcting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction_acknowledgment: Option<String>,
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
mod v28_demotion_receipts_probe_contract {
    use super::*;

    #[test]
    fn protocol_version_is_twenty_eight() {
        assert_eq!(PROTOCOL_VERSION, 28);
    }

    #[test]
    fn demotion_state_messages_round_trip_and_match_schema_shape() {
        let request = ClientMessage::RequestDemotionState(RequestDemotionState {});
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(encoded, r#"{"type":"request_demotion_state","payload":{}}"#);
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            request
        );

        let reset = ClientMessage::ResetInterventionDemotion(ResetInterventionDemotion {});
        let encoded = serde_json::to_string(&reset).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"reset_intervention_demotion","payload":{}}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            reset
        );

        let active = ServerMessage::DemotionState(DemotionState {
            state: DemotionStateKind::Active,
            wrong_count: 1,
            delivered_count: 12,
            threshold_percent: 15,
            minimum_sample: 10,
            window_days: 14,
            threshold_policy_version: 1,
            repromotion_policy_version: 1,
            demoted_at: None,
            disclosure: None,
        });
        let encoded = serde_json::to_string(&active).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"demotion_state","payload":{"state":"active","wrong_count":1,"delivered_count":12,"threshold_percent":15,"minimum_sample":10,"window_days":14,"threshold_policy_version":1,"repromotion_policy_version":1}}"#
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&encoded).unwrap(),
            active
        );
    }

    /// The demotion payload carries the current state and its versioned
    /// constants only. A transition history, timeline, or per-transition
    /// record is rejected at decode time.
    #[test]
    fn demotion_state_rejects_history_fields() {
        let smuggled = r#"{"type":"demotion_state","payload":{"state":"demoted","wrong_count":3,"delivered_count":12,"threshold_percent":15,"minimum_sample":10,"window_days":14,"threshold_policy_version":1,"repromotion_policy_version":1,"transitions":[]}}"#;
        assert!(serde_json::from_str::<ServerMessage>(smuggled).is_err());
    }

    #[test]
    fn weekly_digest_messages_round_trip_and_match_schema_shape() {
        let request = ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: -28_800,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"request_weekly_digest","payload":{"utc_offset_seconds":-28800}}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            request
        );

        let digest = ServerMessage::WeeklyDigest(WeeklyDigest {
            week_start_local_date: "2026-07-27".into(),
            blocks_declared: 5,
            blocks_completed: 3,
            recoveries: 4,
            wrong_interventions: 1,
            invitations_accepted: 2,
            withheld: 1,
            headline: "You returned 4 times and completed 3 of 5 blocks.".into(),
            digest_version: 1,
        });
        let encoded = serde_json::to_string(&digest).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"weekly_digest","payload":{"week_start_local_date":"2026-07-27","blocks_declared":5,"blocks_completed":3,"recoveries":4,"wrong_interventions":1,"invitations_accepted":2,"withheld":1,"headline":"You returned 4 times and completed 3 of 5 blocks.","digest_version":1}}"#
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&encoded).unwrap(),
            digest
        );

        let acknowledge = ClientMessage::AcknowledgeWeeklyDigest(AcknowledgeWeeklyDigest {
            week_start_local_date: "2026-07-27".into(),
        });
        let encoded = serde_json::to_string(&acknowledge).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"acknowledge_weekly_digest","payload":{"week_start_local_date":"2026-07-27"}}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            acknowledge
        );
    }

    /// The digest is a closed set of bounded counts plus one registered
    /// headline. A streak, chain, or per-day field is rejected at decode
    /// time.
    #[test]
    fn weekly_digest_rejects_streak_shaped_fields() {
        for field in ["\"streak\":3", "\"days\":[1,2]", "\"failure_count\":1"] {
            let smuggled = format!(
                r#"{{"type":"weekly_digest","payload":{{"week_start_local_date":"2026-07-27","blocks_declared":5,"blocks_completed":3,"recoveries":4,"wrong_interventions":1,"invitations_accepted":2,"withheld":1,"headline":"x","digest_version":1,{field}}}}}"#
            );
            assert!(
                serde_json::from_str::<ServerMessage>(&smuggled).is_err(),
                "digest accepted smuggled field {field:?}"
            );
        }
    }

    #[test]
    fn intervention_explanation_messages_round_trip_and_match_schema_shape() {
        let request =
            ClientMessage::RequestInterventionExplanation(RequestInterventionExplanation {
                block_id: Uuid::nil(),
                utc_offset_seconds: 3_600,
            });
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"request_intervention_explanation","payload":{"block_id":"00000000-0000-0000-0000-000000000000","utc_offset_seconds":3600}}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            request
        );

        let explanation = ServerMessage::InterventionExplanation(InterventionExplanation {
            block_id: Uuid::nil(),
            sentence: "Velvt offered this nudge because it observed 5 switches away from focus \
                       work in the 10 minutes before the offer."
                .into(),
        });
        let encoded = serde_json::to_string(&explanation).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, explanation);
    }

    /// The explanation request is structurally content-free: any user-text
    /// field is rejected at decode time, so a chat surface cannot grow out
    /// of this affordance without a protocol change (D7).
    #[test]
    fn explanation_request_rejects_user_text_fields() {
        for field in ["\"text\":\"why\"", "\"message\":\"hi\"", "\"reply\":\"ok\""] {
            let smuggled = format!(
                r#"{{"type":"request_intervention_explanation","payload":{{"block_id":"00000000-0000-0000-0000-000000000000","utc_offset_seconds":0,{field}}}}}"#
            );
            assert!(
                serde_json::from_str::<ClientMessage>(&smuggled).is_err(),
                "explanation request accepted smuggled field {field:?}"
            );
        }
    }
}

#[cfg(test)]
mod v27_initiation_contract {
    use super::*;

    #[test]
    fn protocol_version_is_at_least_twenty_seven() {
        let version = PROTOCOL_VERSION;
        assert!(version >= 27);
    }

    #[test]
    fn request_initiation_invitation_round_trips_and_matches_schema_shape() {
        let message = ClientMessage::RequestInitiationInvitation(RequestInitiationInvitation {
            utc_offset_seconds: -28_800,
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"request_initiation_invitation","payload":{"utc_offset_seconds":-28800}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn initiation_invitation_round_trips_and_matches_schema_shape() {
        let message = ServerMessage::InitiationInvitation(InitiationInvitation {
            invitation_id: Uuid::nil(),
            action_id: "soft_start_25".into(),
            body: "You usually focus well around now — want a 25-minute soft start?".into(),
            duration_seconds: 1_500,
            policy_version: 1,
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"initiation_invitation","payload":{"invitation_id":"00000000-0000-0000-0000-000000000000","action_id":"soft_start_25","body":"You usually focus well around now — want a 25-minute soft start?","duration_seconds":1500,"policy_version":1}}"#
        );
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    /// The invitation payload is schedule-free by construction: any attempt
    /// to smuggle a window, weekday, or hour-bucket field is rejected at
    /// decode time, and the encoded payload never contains one.
    #[test]
    fn initiation_invitation_rejects_schedule_fields() {
        let smuggled = r#"{"type":"initiation_invitation","payload":{"invitation_id":"00000000-0000-0000-0000-000000000000","action_id":"soft_start_25","body":"x","duration_seconds":1500,"policy_version":1,"good_hour":9}}"#;
        assert!(serde_json::from_str::<ServerMessage>(smuggled).is_err());
        let encoded =
            serde_json::to_string(&ServerMessage::InitiationInvitation(InitiationInvitation {
                invitation_id: Uuid::nil(),
                action_id: "soft_start_25".into(),
                body: "You usually focus well around now — want a 25-minute soft start?".into(),
                duration_seconds: 1_500,
                policy_version: 1,
            }))
            .unwrap();
        for forbidden in ["hour", "weekday", "bucket", "window", "local_"] {
            assert!(
                !encoded.contains(forbidden),
                "schedule-shaped field {forbidden:?} in invitation payload {encoded}"
            );
        }
    }

    #[test]
    fn dismiss_initiation_invitation_round_trips() {
        let message = ClientMessage::DismissInitiationInvitation(DismissInitiationInvitation {
            invitation_id: Uuid::nil(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"dismiss_initiation_invitation","payload":{"invitation_id":"00000000-0000-0000-0000-000000000000"}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn initiation_settings_messages_round_trip() {
        let set = ClientMessage::SetInitiationSettings(SetInitiationSettings {
            invitations_enabled: false,
        });
        let encoded = serde_json::to_string(&set).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"set_initiation_settings","payload":{"invitations_enabled":false}}"#
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            set
        );

        let request = ClientMessage::RequestInitiationSettings(RequestInitiationSettings {});
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"request_initiation_settings","payload":{}}"#
        );

        let state = ServerMessage::InitiationSettings(InitiationSettings {
            invitations_enabled: true,
        });
        let encoded = serde_json::to_string(&state).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"initiation_settings","payload":{"invitations_enabled":true}}"#
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&encoded).unwrap(),
            state
        );
    }

    /// A v26 start command without `invitation_id` still decodes (a plain
    /// manual start), and an accepted invitation travels on the existing
    /// start command rather than a second declaration path.
    #[test]
    fn start_work_block_invitation_id_is_optional_on_the_wire() {
        let v26 = r#"{"type":"start_work_block","payload":{"intention":null,"planned_duration_seconds":1500,"purpose":null,"intensity":"medium"}}"#;
        let decoded: ClientMessage = serde_json::from_str(v26).unwrap();
        let ClientMessage::StartWorkBlock(request) = &decoded else {
            panic!("expected start_work_block");
        };
        assert_eq!(request.invitation_id, None);

        let accepted = ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: 1_500,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            invitation_id: Some(Uuid::nil()),
        });
        let encoded = serde_json::to_string(&accepted).unwrap();
        assert!(encoded.contains(r#""invitation_id":"00000000-0000-0000-0000-000000000000""#));
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&encoded).unwrap(),
            accepted
        );
    }

    /// The registry stays closed on the wire: recovery actions admit exactly
    /// the two registered ids.
    #[test]
    fn recovery_action_vocabulary_is_the_closed_registry() {
        for action_id in ["protect_next_10", "soft_restart_10"] {
            let message = ClientMessage::AcceptWorkBlockRecovery(AcceptWorkBlockRecovery {
                block_id: Uuid::nil(),
                action_id: action_id.into(),
            });
            let encoded = serde_json::to_string(&message).unwrap();
            let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, message);
        }
    }
}

#[cfg(test)]
mod v26_focus_contract {
    use super::*;

    #[test]
    fn protocol_version_is_at_least_twenty_six() {
        let version = PROTOCOL_VERSION;
        assert!(version >= 26);
    }

    #[test]
    fn focus_state_changed_round_trips_and_matches_schema_shape() {
        let message = ClientMessage::FocusStateChanged(FocusStateChanged {
            active: true,
            occurred_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            utc_offset_seconds: -28_800,
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"focus_state_changed","payload":{"active":true,"occurred_at":"2027-01-15T08:00:00Z","utc_offset_seconds":-28800}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    /// The Focus/DND transition message is structurally coarse: any attempt
    /// to smuggle a mode name, schedule, or configuration field is rejected
    /// at decode time.
    #[test]
    fn focus_state_changed_rejects_mode_identity_fields() {
        let smuggled = r#"{"type":"focus_state_changed","payload":{"active":true,"occurred_at":"2027-01-15T08:00:00Z","utc_offset_seconds":0,"mode_name":"Work"}}"#;
        assert!(serde_json::from_str::<ClientMessage>(smuggled).is_err());
    }

    #[test]
    fn respond_quiet_hours_offer_round_trips() {
        let message =
            ClientMessage::RespondQuietHoursOffer(RespondQuietHoursOffer { accepted: false });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"respond_quiet_hours_offer","payload":{"accepted":false}}"#
        );
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn quiet_hours_offer_round_trips_and_matches_schema_shape() {
        let message = ServerMessage::QuietHoursOffer(QuietHoursOffer {
            rule_version: 1,
            late_night_days: 3,
            start_local_minutes: 1_320,
            end_local_minutes: 420,
            body: "Velvt can hold its notifications overnight.".into(),
        });
        let encoded = serde_json::to_string(&message).unwrap();
        assert_eq!(
            encoded,
            r#"{"type":"quiet_hours_offer","payload":{"rule_version":1,"late_night_days":3,"start_local_minutes":1320,"end_local_minutes":420,"body":"Velvt can hold its notifications overnight."}}"#
        );
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn dnd_outcome_enum_uses_exact_wire_vocabulary() {
        assert_eq!(
            serde_json::to_string(&WorkBlockDndOutcome::CompletedUnderDnd).unwrap(),
            r#""completed_under_dnd""#
        );
        assert_eq!(
            serde_json::to_string(&WorkBlockDndOutcome::DeliverySuppressedDnd).unwrap(),
            r#""delivery_suppressed_dnd""#
        );
    }

    /// Backward compatibility: a v25 result payload without the new fields
    /// decodes to an empty outcome list and no reconciliation line, and a
    /// result without DND evidence serializes without the new fields.
    #[test]
    fn work_block_result_dnd_fields_are_optional_on_the_wire() {
        let v25 = r#"{
            "planned_duration_seconds": 1500,
            "elapsed_duration_seconds": 1500,
            "longest_uninterrupted_seconds": 900,
            "switch_away_count": 1,
            "recovery_count": 1,
            "confidence": "high",
            "coverage": "good",
            "coverage_ratio": 0.9,
            "safe_evidence_category": "focus_work",
            "observation": "Velvt observed one sustained category pattern.",
            "next_action": {
                "action_id": "protect_next_10",
                "label": "Protect the next 10 minutes.",
                "duration_seconds": 600
            }
        }"#;
        let decoded: WorkBlockResult = serde_json::from_str(v25).unwrap();
        assert!(decoded.dnd_outcomes.is_empty());
        assert!(decoded.reconciliation.is_none());
        let encoded = serde_json::to_string(&decoded).unwrap();
        assert!(!encoded.contains("dnd_outcomes"));
        assert!(!encoded.contains("reconciliation"));

        let with_outcomes = WorkBlockResult {
            dnd_outcomes: vec![
                WorkBlockDndOutcome::CompletedUnderDnd,
                WorkBlockDndOutcome::DeliverySuppressedDnd,
            ],
            reconciliation: Some("Velvt held 1 nudge while Do Not Disturb was on.".into()),
            ..decoded
        };
        let encoded = serde_json::to_string(&with_outcomes).unwrap();
        assert!(
            encoded.contains(r#""dnd_outcomes":["completed_under_dnd","delivery_suppressed_dnd"]"#)
        );
        let round_tripped: WorkBlockResult = serde_json::from_str(&encoded).unwrap();
        assert_eq!(round_tripped, with_outcomes);
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
