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

/// A device-local intervention offer and its observed outcome. `anchor_category`
/// is a broad taxonomy category and carries no raw context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkBlockIntervention {
    pub offered_at: DateTime<Utc>,
    pub action_id: String,
    pub anchor_category: String,
    pub switch_count: u32,
    pub window_seconds: u32,
    pub outcome: WorkBlockInterventionOutcome,
    pub outcome_at: Option<DateTime<Utc>>,
}
