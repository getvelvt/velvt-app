use chrono::{DateTime, Utc};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, InterventionSalience, WorkBlockIntensity,
    WorkBlockPhase, WorkBlockPurpose, WorkBlockResult,
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
    /// Application identity this event was classified under, so a correction
    /// can be generalized to the app without retaining the raw application
    /// name. Null for rows written before app-scoped corrections existed;
    /// those events cannot be generalized retroactively.
    pub app_stable_id: Option<String>,
    /// Whether generalizing a correction to the whole app is meaningful.
    /// False for a browser window carrying a site context.
    pub app_scope_eligible: bool,
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
    /// The user said they were working the whole time: the offer should never
    /// have fired. The strongest evidence a false positive occurred.
    WasFocused,
    /// The user explicitly dismissed the offer.
    Dismissed,
    /// The delivery path would have fired while system Focus/DND was active,
    /// so the decision was recorded, held, and delivered by no channel.
    /// Terminal at creation — a nudge that was never shown cannot be
    /// answered — and reconciled after the block as a count only. Excluded
    /// from delivered-intervention metrics.
    DeliverySuppressedDnd,
    /// The drift gate cleared while the auto-demotion state machine was
    /// demoted, so the decision was recorded and withheld: no channel, no
    /// retry, no catch-up after re-promotion. Terminal at creation — a
    /// nudge that was never shown cannot be answered or be wrong — and
    /// excluded from delivered-intervention metrics. These rows are what
    /// the weekly digest counts as "what Velvt chose not to send".
    WithheldDemotion,
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
            Self::WasFocused => "was_focused",
            Self::Dismissed => "dismissed",
            Self::DeliverySuppressedDnd => "delivery_suppressed_dnd",
            Self::WithheldDemotion => "withheld_demotion",
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
            "was_focused" => Some(Self::WasFocused),
            "dismissed" => Some(Self::Dismissed),
            "delivery_suppressed_dnd" => Some(Self::DeliverySuppressedDnd),
            "withheld_demotion" => Some(Self::WithheldDemotion),
            "no_response" => Some(Self::NoResponse),
            _ => None,
        }
    }

    /// True once the outcome can no longer change. An explicit user response
    /// outranks the block later ending.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Offered)
    }

    /// True when the user pushed the offer away.
    ///
    /// This is negative training signal, and the only allowed response to it is
    /// to back off — a longer cooldown and a quieter offer next time. Nothing
    /// in the system may raise emotional charge because of it. `NoResponse` is
    /// excluded on purpose: silence is not a refusal, and treating an
    /// undelivered offer as one would suppress the next offer for a user who
    /// never saw the first.
    pub fn is_negative(self) -> bool {
        matches!(
            self,
            Self::NotHelpful | Self::WrongClassification | Self::WasFocused | Self::Dismissed
        )
    }
}

/// Rolling counts behind the auto-demotion rule (roadmap invariant 4).
///
/// `delivered` counts every offer that reached the user; `was_focused` counts
/// those answered with the reply that says the offer should never have fired.
/// Content-free by construction — two integers, no categories, no timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WrongInterventionCounts {
    pub delivered: u32,
    pub was_focused: u32,
}

/// The two states of the deterministic auto-demotion policy (roadmap
/// invariant 4; D5). A versioned rule over the wrong-intervention counter,
/// never a learned or adaptive value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionDemotionState {
    Active,
    Demoted,
}

impl InterventionDemotionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Demoted => "demoted",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "demoted" => Some(Self::Demoted),
            _ => None,
        }
    }
}

/// The persisted demotion state singleton. The state is derived
/// deterministically from the stored intervention outcomes plus
/// `manual_reset_at`; this record exists so the entered-at instant can be
/// disclosed and a manual reset is remembered. Current state only — never
/// a transition history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemotionStateRecord {
    pub state: InterventionDemotionState,
    pub demoted_at: Option<DateTime<Utc>>,
    pub manual_reset_at: Option<DateTime<Utc>>,
    pub threshold_policy_version: u32,
    pub repromotion_policy_version: u32,
    pub updated_at: DateTime<Utc>,
}

/// One stored weekly receipts digest, frozen at generation time from the
/// same stored aggregates the local metrics read (D6). Bounded counts only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyDigestRecord {
    /// Local Monday (`YYYY-MM-DD`) of the covered week.
    pub week_start_local_date: String,
    pub generated_at: DateTime<Utc>,
    pub blocks_declared: u32,
    pub blocks_completed: u32,
    pub recoveries: u32,
    pub wrong_interventions: u32,
    pub invitations_accepted: u32,
    pub withheld: u32,
    pub digest_version: u32,
    pub delivered_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
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
    pub outcome: WorkBlockInterventionOutcome,
    pub outcome_at: Option<DateTime<Utc>>,
    /// How the offer was delivered. Recorded because an outcome cannot be read
    /// without it: an ignored quiet offer never rang.
    pub salience: InterventionSalience,
}

/// The closed verdict vocabulary of the drift gate.
///
/// Every variant is a real branch of `evaluate_drift`, and there is a test that
/// constructs a scenario for each: a closed enum with unreachable variants is a
/// lie about what the gate does.
///
/// Ordering of the variants follows the order the gate evaluates them, so the
/// enum reads as the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GateVerdict {
    /// The block has not run long enough to have an anchor.
    AbstainedWarmup,
    /// Too little time remains for a return to mean anything.
    AbstainedRemaining,
    /// One offer per block, already spent.
    AbstainedBlockCap,
    /// Inside the re-offer cooldown earned by a negative reply.
    AbstainedBackoff,
    /// No confident dominant category yet, so there is nothing to drift from.
    AbstainedNoAnchor,
    /// Departures observed, but below the evidence threshold.
    AbstainedMinSwitches,
    /// The latest confident evidence is the anchor: the user is already back.
    AbstainedAtAnchor,
    /// The versioned demotion policy is in `demoted`; recorded, never shown.
    WithheldDemotion,
    /// System Focus/DND was active; recorded and held, never shown.
    SuppressedDnd,
    /// The gate cleared and an offer was delivered.
    Offered,
}

impl GateVerdict {
    /// Every variant, in policy-evaluation order. The reachability test walks
    /// this, so a variant added without a scenario fails the build's tests
    /// rather than silently becoming a dead enum arm.
    pub const ALL: [GateVerdict; 10] = [
        GateVerdict::AbstainedWarmup,
        GateVerdict::AbstainedRemaining,
        GateVerdict::AbstainedBlockCap,
        GateVerdict::AbstainedBackoff,
        GateVerdict::AbstainedNoAnchor,
        GateVerdict::AbstainedMinSwitches,
        GateVerdict::AbstainedAtAnchor,
        GateVerdict::WithheldDemotion,
        GateVerdict::SuppressedDnd,
        GateVerdict::Offered,
    ];

    /// The stored token. Must match the schema's CHECK vocabulary exactly;
    /// a mismatch is a constraint violation at the first write, not a silent
    /// downgrade.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AbstainedWarmup => "abstained_warmup",
            Self::AbstainedRemaining => "abstained_remaining",
            Self::AbstainedBlockCap => "abstained_block_cap",
            Self::AbstainedBackoff => "abstained_backoff",
            Self::AbstainedNoAnchor => "abstained_no_anchor",
            Self::AbstainedMinSwitches => "abstained_min_switches",
            Self::AbstainedAtAnchor => "abstained_at_anchor",
            Self::WithheldDemotion => "withheld_demotion",
            Self::SuppressedDnd => "suppressed_dnd",
            Self::Offered => "offered",
        }
    }

    /// Total by construction: an unrecognised token is `None`, never a
    /// defaulted verdict. A row written by a newer binary must not read back
    /// as an older meaning.
    pub fn from_stored(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == value)
    }

    /// Whether this verdict actually put something in front of the user.
    /// `withheld_demotion` and `suppressed_dnd` decided to offer and then held
    /// it; nothing rang, so nothing was delivered.
    pub fn was_delivered(self) -> bool {
        matches!(self, Self::Offered)
    }
}

/// One evaluation of the drift policy and the decision it produced, including
/// every abstention.
///
/// Deliberately not `WorkBlockIntervention`: that table's `PRIMARY KEY(block_id)`
/// is the denominator of the pre-registered primary outcome. This record never
/// touches it.
///
/// `anchor_category` is `None` when the gate abstained before it had computed an
/// anchor. That is evidence about the gate, not missing data — the log states
/// what the gate knew at the instant it decided.
#[derive(Debug, Clone, PartialEq)]
pub struct InterventionDecision {
    pub decision_id: String,
    pub occurred_at: DateTime<Utc>,
    pub block_id: Option<String>,
    pub policy_version: u32,
    pub anchor_category: Option<String>,
    pub switch_count: u32,
    pub elapsed_seconds: u32,
    pub remaining_seconds: u32,
    pub gate_verdict: GateVerdict,
    /// The realized probability of the arm that was taken. 1.0 while the policy
    /// is deterministic. Stored now so that off-policy evaluation is possible
    /// later; a decision made without one can never be corrected after the fact.
    pub propensity: f64,
    /// Proximal outcome on the same horizon regardless of verdict. `None` means
    /// unresolved, never "did not return".
    pub anchor_seen_within_600s: Option<bool>,
    pub outcome_at: Option<DateTime<Utc>>,
}

/// The bucket granularity for `out_of_block_run.started_at_bucket`, matching the
/// five-minute precision class `focus_state_evidence` (migration 0019) already
/// established. Defined once so no caller can introduce a finer one — a new
/// precision class is a privacy change, and it should require editing this line.
pub const OUT_OF_BLOCK_RUN_BUCKET_SECONDS: i64 = 300;

/// Floors a unix timestamp onto the five-minute bucket grid.
///
/// `div_euclid` rather than `/` so a pre-epoch timestamp floors downwards too,
/// instead of rounding towards zero into the following bucket.
pub fn out_of_block_run_bucket(at: DateTime<Utc>) -> i64 {
    at.timestamp()
        .div_euclid(OUT_OF_BLOCK_RUN_BUCKET_SECONDS)
        .saturating_mul(OUT_OF_BLOCK_RUN_BUCKET_SECONDS)
}

/// One closed run of activity that happened outside any declared work block.
///
/// Broad category and coarse time only. There is deliberately no field that
/// could hold a label, a stable id, an application name, a window title, a URL,
/// or intention text — the durable store knows less than the 7-day buffer it is
/// folded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutOfBlockRun {
    /// Unix seconds floored to the 300-second bucket.
    pub started_at_bucket: i64,
    pub duration_seconds: u32,
    pub category: String,
    /// Carried so that `is_confident_evidence` is reconstructible out of block.
    /// Without it the feature layer and the shipped gate could disagree about
    /// what counts as evidence, and every comparison between them would be
    /// meaningless.
    pub classification_status: ClassificationStatus,
    pub classification_confidence: ClassificationConfidence,
    pub local_hour: u8,
    pub local_date: String,
}

/// Whether a block started on a weekday or at the weekend. Closed vocabulary:
/// the schema constrains it, so an unrecognised day type cannot be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayType {
    Weekday,
    Weekend,
}

impl DayType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Weekday => "weekday",
            Self::Weekend => "weekend",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "weekday" => Some(Self::Weekday),
            "weekend" => Some(Self::Weekend),
            _ => None,
        }
    }
}

/// The bounded pre-block window, recorded once at block start and never updated.
///
/// `categories` is a *set*, not a sequence: a sequence would be both more
/// informative to the model and more identifying. `window_seconds` is bounded by
/// the schema at 30 minutes, so the amount of pre-block context recorded cannot
/// grow without a migration and a privacy review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAntecedent {
    pub block_id: String,
    pub window_seconds: u32,
    /// Distinct categories present in the window, sorted, no duplicates.
    /// Serialized as a JSON array; no ordering information, no per-item dwell.
    pub categories: Vec<String>,
    pub switch_count: u32,
    pub dominant_category: Option<String>,
    pub dominant_dwell_seconds: Option<u32>,
    pub day_type: DayType,
    pub hour_bucket: u8,
    pub is_first_block_of_day: bool,
    pub antecedent_version: u32,
}

/// The lifecycle of a discovered antecedent pattern (`0029`).
///
/// Closed vocabulary, constrained by the schema. `Surfaced` is unreachable
/// without `confirmed_at`, and that is enforced by the database rather than by
/// this enum — an invariant a caller can hold wrong is not an invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntecedentFindingState {
    /// Discovered on one window; not yet carried to a held-out window.
    Candidate,
    /// Replicated on a later, unseen window.
    Confirmed,
    /// Shown to the user. **Unreachable today**: nothing surfaces.
    Surfaced,
    Retracted,
    /// The user said this is wrong.
    Disputed,
}

impl AntecedentFindingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Confirmed => "confirmed",
            Self::Surfaced => "surfaced",
            Self::Retracted => "retracted",
            Self::Disputed => "disputed",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "candidate" => Some(Self::Candidate),
            "confirmed" => Some(Self::Confirmed),
            "surfaced" => Some(Self::Surfaced),
            "retracted" => Some(Self::Retracted),
            "disputed" => Some(Self::Disputed),
            _ => None,
        }
    }
}

/// Why a finding stopped being asserted. Closed vocabulary, constrained by the
/// schema.
///
/// `RegistryVersionChange` exists because a finding discovered under one
/// candidate registry is not comparable to one discovered under another: the
/// family size moved, so the correction that licensed it no longer applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntecedentRetractionReason {
    EffectDisappeared,
    SupportLost,
    UserDisputed,
    RegistryVersionChange,
}

impl AntecedentRetractionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EffectDisappeared => "effect_disappeared",
            Self::SupportLost => "support_lost",
            Self::UserDisputed => "user_disputed",
            Self::RegistryVersionChange => "registry_version_change",
        }
    }

    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "effect_disappeared" => Some(Self::EffectDisappeared),
            "support_lost" => Some(Self::SupportLost),
            "user_disputed" => Some(Self::UserDisputed),
            "registry_version_change" => Some(Self::RegistryVersionChange),
            _ => None,
        }
    }
}

/// One discovered antecedent pattern (`0029_antecedent_findings.sql`).
///
/// `effect_size` and `confirm_effect_size` are **risk differences**,
/// `P(Y=1|A) - P(Y=1|not A)`, on `[-1, 1]`. Not odds ratios, and there is no
/// field for one: the analysis computes only the quantity a surface could
/// state, so a surface cannot render a quantity the analysis did not compute.
///
/// `candidate_id` is a key from the closed compile-time registry in
/// `behavior/candidates.rs`. It cannot hold an application name, a label, a
/// stable id, a window title, a URL, or intention text, because the registry
/// that mints it has no constructor that could.
#[derive(Debug, Clone, PartialEq)]
pub struct AntecedentFinding {
    pub finding_id: String,
    pub candidate_id: String,
    pub candidate_registry_version: u32,
    pub discovered_at: i64,
    /// `YYYY-MM-DD`, enforced by the schema.
    pub discovery_window_start: String,
    pub discovery_window_end: String,
    pub support_episodes: u32,
    /// Risk difference on the discovery window.
    pub effect_size: f64,
    /// Benjamini-Hochberg q-value over the logged family size.
    pub q_value: f64,
    /// `None` means never confirmed, which means never shown.
    pub confirmed_at: Option<i64>,
    pub confirm_support_episodes: Option<u32>,
    /// Risk difference on the held-out window.
    pub confirm_effect_size: Option<f64>,
    pub state: AntecedentFindingState,
    pub surfaced_at: Option<i64>,
    pub retracted_at: Option<i64>,
    pub retraction_reason: Option<AntecedentRetractionReason>,
    pub user_disputed_at: Option<i64>,
}
