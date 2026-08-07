use chrono::{DateTime, Utc};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, WorkBlockIntensity, WorkBlockPhase,
    WorkBlockPurpose, WorkBlockResult,
};

#[derive(Clone, PartialEq, Eq)]
pub struct AbstractionMapping {
    pub key_hash: String,
    pub stable_id: String,
    pub label: String,
    pub category: String,
    pub taxonomy_version: String,
    pub classification_tier: String,
    pub classification_status: String,
    pub classification_confidence: String,
    pub classification_source: String,
    /// Curated local-only display label. Never serialized into cloud DTOs.
    pub display_name: Option<String>,
}

impl std::fmt::Debug for AbstractionMapping {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AbstractionMapping")
            .field("key_hash", &"[local_identifier]")
            .field("stable_id", &"[local_identifier]")
            .field("label", &self.label)
            .field("category", &self.category)
            .field("taxonomy_version", &self.taxonomy_version)
            .field("classification_tier", &self.classification_tier)
            .field("classification_status", &self.classification_status)
            .field("classification_confidence", &self.classification_confidence)
            .field("classification_source", &self.classification_source)
            .field(
                "display_name",
                &self.display_name.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RawEventEntry {
    pub event_id: String,
    pub stable_id: String,
    pub label: String,
    pub local_display_label: Option<String>,
    pub local_name_suggestion: Option<String>,
    pub category: String,
    pub taxonomy_version: String,
    pub classification_tier: String,
    pub classification_status: String,
    pub classification_confidence: String,
    pub classification_source: String,
    pub occurred_at: DateTime<Utc>,
    pub duration_seconds: u64,
    /// Whether this locally retained event may enter the cloud upload queue.
    /// Events collected before authentication remain permanently local-only.
    pub upload_eligible: bool,
}

impl std::fmt::Debug for RawEventEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawEventEntry")
            .field("event_id", &self.event_id)
            .field("stable_id", &"[local_identifier]")
            .field("label", &self.label)
            .field(
                "local_display_label",
                &self.local_display_label.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "local_name_suggestion",
                &self.local_name_suggestion.as_ref().map(|_| "[redacted]"),
            )
            .field("category", &self.category)
            .field("taxonomy_version", &self.taxonomy_version)
            .field("classification_tier", &self.classification_tier)
            .field("classification_status", &self.classification_status)
            .field("classification_confidence", &self.classification_confidence)
            .field("classification_source", &self.classification_source)
            .field("occurred_at", &self.occurred_at)
            .field("duration_seconds", &self.duration_seconds)
            .field("upload_eligible", &self.upload_eligible)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalEventMetadata {
    pub local_display_label: Option<String>,
    pub classification_status: String,
    pub classification_confidence: String,
    pub classification_source: String,
}

impl std::fmt::Debug for LocalEventMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalEventMetadata")
            .field(
                "local_display_label",
                &self.local_display_label.as_ref().map(|_| "[redacted]"),
            )
            .field("classification_status", &self.classification_status)
            .field("classification_confidence", &self.classification_confidence)
            .field("classification_source", &self.classification_source)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalDisplayAggregate {
    pub label: String,
    pub duration_seconds: u64,
}

impl std::fmt::Debug for LocalDisplayAggregate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalDisplayAggregate")
            .field("label", &"[redacted]")
            .field("duration_seconds", &self.duration_seconds)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PersonalOverrideRecord {
    pub stable_id: String,
    pub label: String,
    pub local_activity_name: Option<String>,
    pub category: String,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for PersonalOverrideRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersonalOverrideRecord")
            .field("stable_id", &"[local_identifier]")
            .field("label", &self.label)
            .field(
                "local_activity_name",
                &self.local_activity_name.as_ref().map(|_| "[redacted]"),
            )
            .field("category", &self.category)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUploadBatch {
    pub batch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEvent {
    pub event_id: String,
    pub stable_id: String,
    pub label: String,
    pub category: String,
    pub taxonomy_version: String,
    pub classification_tier: String,
    pub occurred_at: DateTime<Utc>,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadBatchStatus {
    Pending,
    Sent,
    Failed,
    Rejected,
}

impl UploadBatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadBatch {
    pub batch_id: String,
    pub status: UploadBatchStatus,
    pub attempt_count: u32,
    pub next_attempt_at: DateTime<Utc>,
    pub events: Vec<BatchEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadQueueDiagnostics {
    pub pending_batch_count: u64,
    pub failed_batch_count: u64,
    pub rejected_batch_count: u64,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub last_successful_sync_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCacheEntry {
    pub date: String,
    pub payload: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsightCacheEntry {
    pub date: String,
    pub payload: String,
    pub expires_at: DateTime<Utc>,
    /// True when this entry records a 404 (no approved insight for the date).
    pub is_negative: bool,
}

/// How a block came to be declared. A closed, content-free two-value enum
/// (R2): it records only that the start followed an invitation, never when
/// invitations happen, so it cannot reconstruct a schedule. Local records
/// only — the marker is absent from every IPC, upload, log, and telemetry
/// payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBlockOrigin {
    /// The user declared the block themselves (including recovery starts,
    /// which stay separately identifiable through `recovery_of`).
    Manual,
    /// One tap on an initiation invitation declared the block.
    Invitation,
}

impl WorkBlockOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Invitation => "invitation",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            "invitation" => Some(Self::Invitation),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct WorkBlockRecord {
    pub block_id: String,
    pub phase: WorkBlockPhase,
    pub intention: Option<String>,
    pub purpose: Option<WorkBlockPurpose>,
    pub intensity: WorkBlockIntensity,
    pub planned_duration_seconds: u32,
    pub started_at: DateTime<Utc>,
    pub paused_at: Option<DateTime<Utc>>,
    pub total_paused_seconds: u32,
    pub ended_at: Option<DateTime<Utc>>,
    pub recovered_after_restart: bool,
    pub recovery_of: Option<String>,
    pub origin: WorkBlockOrigin,
    pub intention_expires_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for WorkBlockRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkBlockRecord")
            .field("block_id", &self.block_id)
            .field("phase", &self.phase)
            .field("intention", &self.intention.as_ref().map(|_| "[redacted]"))
            .field("purpose", &self.purpose)
            .field("intensity", &self.intensity)
            .field("planned_duration_seconds", &self.planned_duration_seconds)
            .field("started_at", &self.started_at)
            .field("paused_at", &self.paused_at)
            .field("total_paused_seconds", &self.total_paused_seconds)
            .field("ended_at", &self.ended_at)
            .field("recovered_after_restart", &self.recovered_after_restart)
            .field("recovery_of", &self.recovery_of)
            .field("origin", &self.origin)
            .field("intention_expires_at", &self.intention_expires_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBlockObservation {
    pub occurred_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub category: String,
    pub classification_status: ClassificationStatus,
    pub classification_confidence: ClassificationConfidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkBlockCompletion {
    pub phase: WorkBlockPhase,
    pub ended_at: DateTime<Utc>,
    pub result: WorkBlockResult,
}

/// Outcome of an offered in-session drift intervention. `Offered` becomes
/// terminal only when the block ends without a return, at which point it is
/// rewritten as `Expired`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBlockInterventionOutcome {
    /// The only non-terminal state.
    Offered,
    /// The user took the offered action.
    AcceptedAction,
    /// Observed return to the anchor category, whether or not the action was
    /// explicitly accepted.
    Returned,
    /// The user said the offer did not help. Distinct from disagreeing that
    /// drift occurred.
    NotHelpful,
    /// The user said the underlying classification was wrong. This is evidence
    /// against the detector, not against the user.
    WrongClassification,
    /// The user explicitly dismissed the offer.
    Dismissed,
    /// The user reports the interruption itself was wrong, regardless of
    /// classification: they were focused. Ground-truth false-positive signal,
    /// kept distinct from both `Dismissed` and `WrongClassification`.
    DismissedWasFocused,
    /// The delivery path would have fired while system Focus/DND was active,
    /// so the decision was recorded, held, and delivered by no channel.
    /// Terminal at creation — a nudge that was never shown cannot be
    /// answered — and reconciled after the block as a count only. Excluded
    /// from delivered-intervention metrics.
    DeliverySuppressedDnd,
    /// The block ended with no response of any kind. Never inferred from a
    /// notification disappearing.
    NoResponse,
}

impl WorkBlockInterventionOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::AcceptedAction => "accepted_action",
            Self::Returned => "returned",
            Self::NotHelpful => "not_helpful",
            Self::WrongClassification => "wrong_classification",
            Self::Dismissed => "dismissed",
            Self::DismissedWasFocused => "dismissed_was_focused",
            Self::DeliverySuppressedDnd => "delivery_suppressed_dnd",
            Self::NoResponse => "no_response",
        }
    }

    /// Not `FromStr`: the input is a closed database enum, not user text, and
    /// an unknown value is a schema mismatch rather than a parse failure.
    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "offered" => Some(Self::Offered),
            "accepted_action" => Some(Self::AcceptedAction),
            "returned" => Some(Self::Returned),
            "not_helpful" => Some(Self::NotHelpful),
            "wrong_classification" => Some(Self::WrongClassification),
            "dismissed" => Some(Self::Dismissed),
            "dismissed_was_focused" => Some(Self::DismissedWasFocused),
            "delivery_suppressed_dnd" => Some(Self::DeliverySuppressedDnd),
            "no_response" => Some(Self::NoResponse),
            _ => None,
        }
    }

    /// True once the outcome can no longer change. An explicit user response
    /// outranks the block later ending.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Offered)
    }
}

/// Content-free rolling precision counts for the intervention detector:
/// `dismissed_was_focused` replies over interventions delivered. Two bounded
/// integers — no categories, copy, identifiers, or raw timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WrongInterventionCounts {
    pub delivered: u32,
    pub was_focused: u32,
}

/// A block-scoped classification correction: for this block, `category`
/// counts as `counts_as_category`. Broad taxonomy categories only; the
/// correction dies with the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBlockCategoryCorrection {
    pub category: String,
    pub counts_as_category: String,
    pub corrected_at: DateTime<Utc>,
}

/// One coarse system Focus/DND transition, as stored. Deliberately coarse:
/// active/inactive, the transition time floored to the five-minute bucket,
/// and local hour/date buckets for the deterministic pattern rule. No field
/// can hold a Focus mode's name, configuration, or schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusTransition {
    pub active: bool,
    /// Transition time floored to the coarse five-minute bucket.
    pub changed_at_bucket: DateTime<Utc>,
    /// Local hour bucket (0-23) at the transition, from the client's offset.
    pub local_hour: u32,
    /// Local calendar date (`YYYY-MM-DD`) at the transition.
    pub local_date: String,
    pub recorded_at: DateTime<Utc>,
}

/// The user's remembered reply to a quiet-hours offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietHoursOfferResponse {
    Accepted,
    Declined,
}

impl QuietHoursOfferResponse {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Declined => "declined",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "accepted" => Some(Self::Accepted),
            "declined" => Some(Self::Declined),
            _ => None,
        }
    }
}

/// Singleton lifecycle record for the quiet-hours offer: when the pattern
/// rule triggered, when the offer surfaced, and what the user replied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietHoursOfferState {
    pub rule_version: u32,
    pub triggered_at: Option<DateTime<Utc>>,
    pub offered_at: Option<DateTime<Utc>>,
    pub response: Option<QuietHoursOfferResponse>,
    pub responded_at: Option<DateTime<Utc>>,
}

/// Velvt's own quiet-hours window, configured only by explicit user
/// acceptance of an offer. Only ever reduces delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VelvtQuietHours {
    pub start_local_minutes: u32,
    pub end_local_minutes: u32,
    pub rule_version: u32,
    pub configured_at: DateTime<Utc>,
}

/// Outcome of an extended initiation invitation. A separate closed enum
/// from [`WorkBlockInterventionOutcome`]: invitations and interventions
/// answer different questions and their counts must never mix. Content-free
/// by construction — no copy, category, or schedule detail is representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiationInvitationOutcome {
    /// The only non-terminal state.
    Offered,
    /// One tap declared a block; the block carries the origin marker.
    Accepted,
    /// The user explicitly dismissed the invitation. Feeds backoff.
    Dismissed,
    /// The response window lapsed with no reply of any kind. Silence is not
    /// "leave me alone" evidence and does not feed backoff.
    NoResponse,
    /// State invalidated the invitation before an answer (quiet hours began,
    /// a block started, opt-out, logout/account switch, clear-all-data, or
    /// an incompatible policy version). Not backoff evidence.
    Expired,
}

impl InitiationInvitationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Accepted => "accepted",
            Self::Dismissed => "dismissed",
            Self::NoResponse => "no_response",
            Self::Expired => "expired",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "offered" => Some(Self::Offered),
            "accepted" => Some(Self::Accepted),
            "dismissed" => Some(Self::Dismissed),
            "no_response" => Some(Self::NoResponse),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Offered)
    }
}

/// One extended initiation invitation, as stored. Bounded and content-free:
/// an id, when it was extended, the local date for the daily cap, the
/// registered action, the policy versions that produced it, and the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiationInvitationRecord {
    pub invitation_id: String,
    pub offered_at: DateTime<Utc>,
    /// Local calendar date (`YYYY-MM-DD`) at the moment the invitation was
    /// extended; exists solely to enforce the daily cap deterministically.
    pub local_date: String,
    pub action_id: String,
    pub policy_version: u32,
    pub backoff_policy_version: u32,
    pub outcome: InitiationInvitationOutcome,
    pub outcome_at: Option<DateTime<Utc>>,
}

/// One confident, closed observation span inside a completed block — the
/// safe local dwell evidence the good-hours policy aggregates. Broad
/// category evidence only; the category itself is not even carried here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedBlockDwellSpan {
    pub block_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

/// A device-local intervention offer and its observed outcome. `anchor_category`
/// is a broad taxonomy category and carries no raw context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBlockIntervention {
    pub offered_at: DateTime<Utc>,
    pub action_id: String,
    pub anchor_category: String,
    pub switch_count: u32,
    pub window_seconds: u32,
    /// The versioned backoff policy that produced this offer. 0 marks offers
    /// recorded before the policy existed.
    pub backoff_policy_version: u32,
    pub outcome: WorkBlockInterventionOutcome,
    pub outcome_at: Option<DateTime<Utc>>,
}
