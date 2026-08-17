//! Device-local meaningful-work state, evidence aggregation, and reviewed copy.
//!
//! Swift sends direct commands and renders [`WorkBlockSnapshot`]. This module
//! owns every transition, category observation, derived result, and behavioral
//! sentence. No type in this module is used by the cloud upload path.

use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Duration, Utc};
use tokio::sync::watch;
use uuid::Uuid;
use velvt_shared_types::{
    ActiveIntervention, ClassificationConfidence, ClassificationStatus, ConfidenceLevel,
    InterventionResponse, InterventionSalience, StartWorkBlock, WorkBlockCoverage,
    WorkBlockDndOutcome, WorkBlockIntensity, WorkBlockLifecycleEvent, WorkBlockNextAction,
    WorkBlockPhase, WorkBlockPurpose, WorkBlockResult, WorkBlockSnapshot, WORK_BLOCK_STATE_VERSION,
};

use velvt_shared_types::{DemotionState, DemotionStateKind};

use crate::{
    delivery::PushAdapter,
    persistence::{
        DemotionStateRecord, InterventionDemotionState, PersistenceError, WorkBlockCompletion,
        WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockOrigin,
        WorkBlockRecord, WorkBlockRepo, WrongInterventionCounts,
    },
};

const MIN_DURATION_SECONDS: u32 = 5 * 60;
const MAX_DURATION_SECONDS: u32 = 180 * 60;
const RECOVERY_DURATION_SECONDS: u32 = 10 * 60;
const INTENTION_RETENTION_HOURS: i64 = 24;

/// In-session drift gates. These are deterministic evidence thresholds, not a
/// learned policy: an offer is made only when the observed switching is
/// unambiguous, the block has run long enough to have an anchor, and there is
/// still enough time left for a return to mean anything.
const DRIFT_WINDOW_SECONDS: i64 = 10 * 60;
const DRIFT_MIN_SWITCHES: u32 = 4;
const DRIFT_MIN_ELAPSED_SECONDS: u32 = 5 * 60;
const DRIFT_MIN_REMAINING_SECONDS: u32 = 2 * 60;
/// The registered banned vocabulary for every Rust-authored copy surface
/// (roadmap invariants 2, 6, and 7). Matched case-insensitively as
/// substrings against rendered copy. Absence framing, failure tallies, and
/// streak language are banned everywhere, not only in intervention copy.
pub const BANNED_COPY_TOKENS: &[&str] = &[
    "still",
    "dismiss",
    "failed",
    "failure",
    "ignored",
    "last time",
    "again",
    "learned",
    "adaptive",
    "missed",
    "skipped",
    "declined",
    "you didn't",
    "you haven't",
    "you never",
    "last invitation",
    "last offer",
    "streak",
    "broken chain",
];

/// Rolling window for the local wrong-intervention counter: `was_focused`
/// replies over interventions delivered. The auto-demotion policy below
/// evaluates over this same window.
const WRONG_INTERVENTION_ROLLING_DAYS: i64 = 14;
/// Versioned auto-demotion policy (roadmap invariant 4). A deterministic
/// rule over the wrong-intervention counter, never a learned or adaptive
/// value. Bump the threshold version when the threshold, window, or minimum
/// sample below changes meaning.
pub const DEMOTION_THRESHOLD_POLICY_VERSION: u32 = 1;
/// Demotion triggers when the wrong-intervention rate strictly exceeds this
/// whole-percent threshold. Exactly at the threshold is not demotion.
pub const DEMOTION_THRESHOLD_PERCENT: u32 = 15;
/// Minimum delivered interventions inside the evaluation window before the
/// rate is meaningful. Below this floor demotion never triggers: one wrong
/// nudge out of three delivered is thin evidence, not a 33% detector.
pub const DEMOTION_MIN_DELIVERED_SAMPLE: u32 = 10;
/// Versioned re-promotion policy (v1): the demoted state ends the moment the
/// same windowed evaluation stops exceeding the threshold — because wrong
/// replies aged out of the rolling window, because the delivered sample fell
/// back below the minimum floor, or because the user manually reset (which
/// restarts the evaluation window at the reset instant). Deterministic: the
/// same stored outcome stream and clock always produce the same transitions.
pub const DEMOTION_REPROMOTION_POLICY_VERSION: u32 = 1;
/// The closed action registry. Two actions exist: the in-block drift
/// recovery and the post-block gentle re-entry. Closed by construction: the
/// schema constrains `action_id`, so an unregistered action cannot be
/// persisted, and `accept_recovery` only honors the action the terminal
/// result actually offered.
const DRIFT_ACTION_ID: &str = "protect_next_10";
/// Gentle re-entry after an invited block ended early (0.1.6 Scope 3; D4).
/// Forward-looking analyst voice: no reference to what went wrong.
const SOFT_RESTART_ACTION_ID: &str = "soft_restart_10";
const SOFT_RESTART_LABEL: &str = "Want back in? 10-minute soft restart.";
const DRIFT_PROTECT_MINUTES: u32 = 10;
const DRIFT_TITLE: &str = "Your work block is running";

/// Backoff after a pushed-away offer.
///
/// A dismissal is negative training signal, so the only permitted response is
/// to ask less often and more quietly. Each additional consecutive negative
/// outcome doubles the cooldown; a helpful outcome — accepting the action or
/// returning to the anchor — clears the streak in one step. Emotional charge
/// never rises, and nothing here reads how *often* the user drifted: only how
/// they answered.
const BACKOFF_BASE_SECONDS: i64 = 2 * 60 * 60;
const BACKOFF_MAX_SECONDS: i64 = 24 * 60 * 60;
/// How far back the streak is counted. Larger than any plausible streak, so the
/// bound is a query limit rather than a policy.
const BACKOFF_HISTORY_LIMIT: usize = 20;

/// Read-only view of the coarse, device-local Focus/DND evidence record
/// (roadmap invariant 5; D2). Swift only observes and reports transitions;
/// Rust owns the record, and this seam is how the work-block manager
/// consults it for delivery and completion decisions. The answer is a
/// single coarse boolean — no mode name, schedule, or configuration exists
/// behind it.
pub trait FocusStateSource: Send + Sync {
    fn is_focus_active(&self, at: DateTime<Utc>) -> bool;
}

/// A single approved, device-local intervention offer. Copy is authored here,
/// beside the evidence that justifies it; Swift renders it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftIntervention {
    pub block_id: Uuid,
    pub action_id: &'static str,
    pub title: String,
    pub body: String,
    /// `Quiet` suppresses the OS notification and leaves only the in-app card.
    pub salience: InterventionSalience,
}

/// How the last offers were answered, expressed as the two levers backoff is
/// allowed to pull: ask later, and ask more quietly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackoffState {
    suppressed: bool,
    salience: InterventionSalience,
}

/// Result of a safe category observation: the state Swift renders, plus at most
/// one intervention to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationOutcome {
    pub snapshot: WorkBlockSnapshot,
    pub intervention: Option<DriftIntervention>,
}

/// One evaluation of the deterministic demotion policy: the resulting
/// state, the windowed counts it was computed from, and the disclosure
/// instant while demoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemotionEvaluation {
    pub state: InterventionDemotionState,
    pub counts: WrongInterventionCounts,
    pub demoted_at: Option<DateTime<Utc>>,
}

/// The one registered explanation claim: the drift offer fired on observed
/// switching evidence. Closed registry — an explanation for evidence that
/// was not stored cannot be selected.
const DRIFT_EXPLANATION_CLAIM_ID: &str = "drift_switches_observed";

/// The code-selected claim, evidence, and tone for one explanation (D7).
/// Deterministic code builds this from the stored intervention row; any
/// phrasing layer receives exactly these values and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplanationSelection {
    pub claim_id: &'static str,
    pub anchor_category: String,
    pub switch_count: u32,
    pub window_minutes: u32,
}

/// Optional phrasing seam for the selected explanation. A provider may only
/// rephrase the already-selected claim and values; it cannot decide what
/// happened. No provider is wired in this release — the deterministic
/// template below is the v1 explanation — and any future provider output
/// must pass `validate_explanation` or the deterministic template is used.
pub trait ExplanationPhraser: Send + Sync {
    fn phrase(&self, selection: &ExplanationSelection) -> Option<String>;
}

#[derive(Debug, thiserror::Error)]
pub enum WorkBlockError {
    #[error("work-block persistence unavailable")]
    Persistence(#[from] PersistenceError),
    #[error("invalid work-block transition")]
    InvalidTransition,
    #[error("invalid work-block request")]
    InvalidRequest,
}

#[derive(Clone)]
pub struct WorkBlockManager {
    repo: Arc<dyn WorkBlockRepo>,
    focus: Option<Arc<dyn FocusStateSource>>,
    deadline: watch::Sender<Option<DateTime<Utc>>>,
}

impl WorkBlockManager {
    pub fn new(repo: Arc<dyn WorkBlockRepo>) -> Self {
        let (deadline, _) = watch::channel(None);
        Self {
            repo,
            focus: None,
            deadline,
        }
    }

    /// Attaches the Focus/DND evidence source. Without one the manager
    /// behaves exactly as before: no suppression, no DND outcomes.
    pub fn with_focus_source(mut self, focus: Arc<dyn FocusStateSource>) -> Self {
        self.focus = Some(focus);
        self
    }

    fn focus_active(&self, at: DateTime<Utc>) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|focus| focus.is_focus_active(at))
    }

    pub fn deadline_receiver(&self) -> watch::Receiver<Option<DateTime<Utc>>> {
        self.deadline.subscribe()
    }

    pub fn recover_after_restart(
        &self,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        self.repo.expire_intentions(now)?;
        let Some(record) = self.repo.latest()? else {
            return Ok(idle_snapshot());
        };
        match record.phase {
            WorkBlockPhase::Active => {
                if elapsed_seconds(&record, now) >= record.planned_duration_seconds {
                    self.finish(&record, WorkBlockPhase::Expired, planned_deadline(&record))
                } else {
                    self.repo.mark_recovered(&record.block_id, now)?;
                    let recovered = self.repo.get(&record.block_id)?;
                    self.publish_deadline(Some(planned_deadline(&recovered)));
                    self.snapshot_for(recovered, now)
                }
            }
            WorkBlockPhase::Paused => {
                self.repo.mark_recovered(&record.block_id, now)?;
                self.publish_deadline(None);
                self.snapshot_for(self.repo.get(&record.block_id)?, now)
            }
            _ => self.snapshot_for(record, now),
        }
    }

    pub fn start(
        &self,
        request: StartWorkBlock,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        self.start_with_origin(request, WorkBlockOrigin::Manual, now)
    }

    /// Starts a block carrying an explicit origin marker. The marker is
    /// decided by the caller after validating an invitation claim; this
    /// module records it and derives nothing else from invitations, so an
    /// invited block flows through exactly the machinery a manual one does.
    pub fn start_with_origin(
        &self,
        request: StartWorkBlock,
        origin: WorkBlockOrigin,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        if let Some(current) = self.repo.latest()? {
            if matches!(
                current.phase,
                WorkBlockPhase::Active | WorkBlockPhase::Paused
            ) {
                return Err(WorkBlockError::InvalidTransition);
            }
        }
        let intention = normalize_intention(request.intention)?;
        if !(MIN_DURATION_SECONDS..=MAX_DURATION_SECONDS)
            .contains(&request.planned_duration_seconds)
        {
            return Err(WorkBlockError::InvalidRequest);
        }
        let record = WorkBlockRecord {
            block_id: Uuid::new_v4().to_string(),
            phase: WorkBlockPhase::Active,
            intention,
            purpose: request.purpose,
            intensity: request.intensity,
            planned_duration_seconds: request.planned_duration_seconds,
            started_at: now,
            paused_at: None,
            total_paused_seconds: 0,
            ended_at: None,
            recovered_after_restart: false,
            recovery_of: None,
            origin,
            intention_expires_at: now + Duration::hours(INTENTION_RETENTION_HOURS),
            updated_at: now,
        };
        self.repo.create(&record)?;
        self.publish_deadline(Some(planned_deadline(&record)));
        self.snapshot_for(record, now)
    }

    pub fn pause(
        &self,
        block_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let record = self.require(block_id, WorkBlockPhase::Active)?;
        if elapsed_seconds(&record, now) >= record.planned_duration_seconds {
            return self.finish(
                &record,
                WorkBlockPhase::Completed,
                planned_deadline(&record),
            );
        }
        let effective_now = effective_now(&record, now).min(planned_deadline(&record));
        self.repo
            .close_open_observation(&record.block_id, effective_now)?;
        self.repo.set_paused(&record.block_id, effective_now)?;
        self.publish_deadline(None);
        self.snapshot_for(self.repo.get(&record.block_id)?, effective_now)
    }

    pub fn resume(
        &self,
        block_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let record = self.require(block_id, WorkBlockPhase::Paused)?;
        let effective_now = effective_now(&record, now);
        let paused_at = record.paused_at.ok_or(WorkBlockError::InvalidTransition)?;
        let added = positive_seconds(effective_now - paused_at);
        let total_paused = record.total_paused_seconds.saturating_add(added);
        self.repo
            .set_active(&record.block_id, effective_now, total_paused)?;
        let active = self.repo.get(&record.block_id)?;
        self.publish_deadline(Some(planned_deadline(&active)));
        self.snapshot_for(active, effective_now)
    }

    pub fn end(
        &self,
        block_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let record = self.repo.get(&block_id.to_string())?;
        if !matches!(
            record.phase,
            WorkBlockPhase::Active | WorkBlockPhase::Paused
        ) {
            if let Some(result) = self.repo.result(&record.block_id)? {
                return self.snapshot_with_result(record, now, Some(result));
            }
            return Err(WorkBlockError::InvalidTransition);
        }
        if record.phase == WorkBlockPhase::Active
            && elapsed_seconds(&record, now) >= record.planned_duration_seconds
        {
            return self.finish(
                &record,
                WorkBlockPhase::Completed,
                planned_deadline(&record),
            );
        }
        // A paused block's work logically ended when the pause began, the
        // same way a completed block ends at its planned deadline rather
        // than at the wall-clock moment the finish was observed. Using the
        // command's wall time here would fold the final pause span into
        // every later terminal `elapsed_seconds` read, contradicting the
        // frozen elapsed value captured in the persisted result.
        let ended_at = match record.phase {
            WorkBlockPhase::Paused => record
                .paused_at
                .unwrap_or_else(|| effective_now(&record, now)),
            _ => effective_now(&record, now),
        };
        self.finish(&record, WorkBlockPhase::Abandoned, ended_at)
    }

    pub fn request_state(&self, now: DateTime<Utc>) -> Result<WorkBlockSnapshot, WorkBlockError> {
        self.repo.expire_intentions(now)?;
        let Some(record) = self.repo.latest()? else {
            return Ok(idle_snapshot());
        };
        if record.phase == WorkBlockPhase::Active
            && elapsed_seconds(&record, now) >= record.planned_duration_seconds
        {
            return self.finish(
                &record,
                WorkBlockPhase::Completed,
                planned_deadline(&record),
            );
        }
        self.snapshot_for(record, now)
    }

    pub fn lifecycle(
        &self,
        event: WorkBlockLifecycleEvent,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        match event {
            WorkBlockLifecycleEvent::Sleep => {
                if let Some(record) = self.repo.latest()? {
                    if record.phase == WorkBlockPhase::Active {
                        let block_id = Uuid::parse_str(&record.block_id)
                            .map_err(|_| WorkBlockError::InvalidRequest)?;
                        return self.pause(block_id, now);
                    }
                }
                self.request_state(now)
            }
            WorkBlockLifecycleEvent::ClockChanged => {
                if let Some(record) = self.repo.latest()? {
                    if record.phase == WorkBlockPhase::Active
                        && elapsed_seconds(&record, now) >= record.planned_duration_seconds
                    {
                        return self.finish(
                            &record,
                            WorkBlockPhase::Expired,
                            planned_deadline(&record),
                        );
                    }
                }
                self.request_state(now)
            }
            WorkBlockLifecycleEvent::Wake | WorkBlockLifecycleEvent::TimeZoneChanged => {
                self.request_state(now)
            }
        }
    }

    pub fn observe_safe_category(
        &self,
        category: &str,
        status: ClassificationStatus,
        confidence: ClassificationConfidence,
        occurred_at: DateTime<Utc>,
    ) -> Result<Option<ObservationOutcome>, WorkBlockError> {
        let Some(record) = self.repo.latest()? else {
            return Ok(None);
        };
        if record.phase != WorkBlockPhase::Active {
            return Ok(None);
        }
        if elapsed_seconds(&record, occurred_at) >= record.planned_duration_seconds {
            return self
                .finish(
                    &record,
                    WorkBlockPhase::Completed,
                    planned_deadline(&record),
                )
                .map(|snapshot| {
                    Some(ObservationOutcome {
                        snapshot,
                        intervention: None,
                    })
                });
        }
        let at = effective_now(&record, occurred_at).min(planned_deadline(&record));
        if self
            .repo
            .latest_observation(&record.block_id)?
            .is_some_and(|latest| {
                latest.ended_at.is_none()
                    && latest.category == category
                    && latest.classification_status == status
                    && latest.classification_confidence == confidence
            })
        {
            return Ok(None);
        }
        self.repo.close_open_observation(&record.block_id, at)?;
        self.repo.append_observation(
            &record.block_id,
            &WorkBlockObservation {
                occurred_at: at,
                ended_at: None,
                category: category.to_owned(),
                classification_status: status,
                classification_confidence: confidence,
            },
        )?;
        // Observing the return closes the loop: an offer is only worth making
        // if its outcome is recorded.
        self.record_return_if_pending(&record, category, at)?;
        let intervention = self.evaluate_drift(&record, at)?;
        let snapshot = self.snapshot_for(record, at)?;
        Ok(Some(ObservationOutcome {
            snapshot,
            intervention,
        }))
    }

    /// Records the user's explicit response to a live offer.
    ///
    /// An explicit response is the strongest evidence available about whether
    /// the detector was right, so it is recorded even if the block has already
    /// ended. Only an unanswered offer transitions: a response cannot be
    /// overwritten, and a second tap is a no-op rather than an error.
    pub fn report_intervention_outcome(
        &self,
        block_id: Uuid,
        response: InterventionResponse,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let record = self.repo.get(&block_id.to_string())?;
        let Some(existing) = self.repo.intervention(&record.block_id)? else {
            return Err(WorkBlockError::InvalidRequest);
        };
        if existing.outcome.is_terminal() {
            // Already answered. Report current state rather than failing.
            return self.snapshot_for(record, now);
        }
        self.repo
            .resolve_intervention(&record.block_id, outcome_for(response), now)?;
        self.snapshot_for(record, now)
    }

    /// Marks a pending offer as returned once the anchor category is observed
    /// again. Only an `offered` row transitions, so this is idempotent.
    fn record_return_if_pending(
        &self,
        record: &WorkBlockRecord,
        category: &str,
        at: DateTime<Utc>,
    ) -> Result<(), WorkBlockError> {
        let Some(pending) = self.repo.intervention(&record.block_id)? else {
            return Ok(());
        };
        if pending.outcome != WorkBlockInterventionOutcome::Offered {
            return Ok(());
        }
        if !pending.anchor_category.eq_ignore_ascii_case(category) {
            return Ok(());
        }
        self.repo.resolve_intervention(
            &record.block_id,
            WorkBlockInterventionOutcome::Returned,
            at,
        )?;
        Ok(())
    }

    /// Deterministic drift gate. Returns an offer at most once per block, and
    /// abstains whenever the evidence is thin rather than guessing.
    fn evaluate_drift(
        &self,
        record: &WorkBlockRecord,
        now: DateTime<Utc>,
    ) -> Result<Option<DriftIntervention>, WorkBlockError> {
        let elapsed = elapsed_seconds(record, now);
        if elapsed < DRIFT_MIN_ELAPSED_SECONDS {
            return Ok(None);
        }
        if record.planned_duration_seconds.saturating_sub(elapsed) < DRIFT_MIN_REMAINING_SECONDS {
            return Ok(None);
        }
        // Hard cap. One offer per block, enforced by the row's existence
        // regardless of how it was resolved.
        if self.repo.intervention(&record.block_id)?.is_some() {
            return Ok(None);
        }
        let backoff = self.backoff_state(now)?;
        if backoff.suppressed {
            return Ok(None);
        }
        let observations = self.repo.observations(&record.block_id)?;
        let Some(anchor) = dominant_category(&observations) else {
            return Ok(None);
        };
        let window_start = now - Duration::seconds(DRIFT_WINDOW_SECONDS);
        // A "switch" is a departure: a confident non-anchor observation whose
        // previous confident observation was the anchor. Counting rows
        // instead would let classifier noise clear the gate — confidence or
        // status flapping on one non-anchor app appends a new row per flap
        // while the user switched away once — and would disagree with the
        // switch_away_count the end-of-block result reports for the same
        // evidence. Seed from the last confident observation before the
        // window so an away period that merely straddles the boundary is not
        // recounted as a fresh departure.
        let mut previous_was_anchor = observations
            .iter()
            .filter(|observation| observation.occurred_at < window_start)
            .rfind(|observation| is_confident_evidence(observation))
            .map(|observation| observation.category.eq_ignore_ascii_case(&anchor));
        let mut switch_count = 0_u32;
        for observation in observations
            .iter()
            .filter(|observation| observation.occurred_at >= window_start)
            .filter(|observation| is_confident_evidence(observation))
        {
            let is_anchor = observation.category.eq_ignore_ascii_case(&anchor);
            if !is_anchor && previous_was_anchor == Some(true) {
                switch_count = switch_count.saturating_add(1);
            }
            previous_was_anchor = Some(is_anchor);
        }
        if switch_count < DRIFT_MIN_SWITCHES {
            return Ok(None);
        }
        // Never offer while the latest confident evidence is the anchor: the
        // user is back at the block, so an offer at this instant would be
        // untruthful, would immediately self-resolve as `returned` without a
        // fresh departure, and would invite an honest `was_focused` reply that
        // pollutes the wrong-intervention rate with a policy-caused false
        // positive. The accumulated departure evidence is not discarded — the
        // offer fires on the next confident non-anchor observation instead.
        // Offer frequency can only decrease under this rule, which is the
        // direction roadmap invariant 2 requires.
        if previous_was_anchor == Some(true) {
            return Ok(None);
        }
        // Auto-demotion (roadmap invariant 4; D5): while the versioned
        // demotion policy is in `demoted`, no intervention fires through
        // any path. The decision the gate would have made is recorded and
        // withheld — no channel, no retry, no catch-up after re-promotion —
        // and, like DND suppression, it starts the same cooldown and counts
        // toward the per-block cap so re-promotion can never produce a
        // burst. Excluded from delivered metrics: a nudge that was never
        // shown cannot be wrong. Evidence collection, blocks, session
        // results, and corrections are untouched by this branch.
        if self.evaluate_demotion(now)?.state == InterventionDemotionState::Demoted {
            self.repo.record_intervention(
                &record.block_id,
                &WorkBlockIntervention {
                    offered_at: now,
                    action_id: DRIFT_ACTION_ID.to_owned(),
                    anchor_category: anchor,
                    switch_count,
                    window_seconds: DRIFT_WINDOW_SECONDS.try_into().unwrap_or(u32::MAX),
                    outcome: WorkBlockInterventionOutcome::WithheldDemotion,
                    outcome_at: Some(now),
                    salience: InterventionSalience::Normal,
                },
            )?;
            return Ok(None);
        }
        // DND is data, not defiance (D2; roadmap invariants 1 and 5). When
        // the gate clears while system Focus/DND is active, the decision is
        // recorded and held: no OS notification, no in-app takeover, no
        // fallback channel, no mid-block retry against the user's setting.
        // The row is terminal at creation — a nudge that was never shown
        // cannot be answered — and it composes with the backoff policy in
        // one direction only: it starts the same re-offer cooldown a
        // delivered offer does and counts toward the per-block cap, so
        // suppression can never shorten a wait, raise salience, or increase
        // future frequency. It is excluded from delivered-intervention
        // metrics and reconciles after the block as a count, never as a
        // late nudge.
        if self.focus_active(now) {
            self.repo.record_intervention(
                &record.block_id,
                &WorkBlockIntervention {
                    offered_at: now,
                    action_id: DRIFT_ACTION_ID.to_owned(),
                    anchor_category: anchor,
                    switch_count,
                    window_seconds: DRIFT_WINDOW_SECONDS.try_into().unwrap_or(u32::MAX),
                    outcome: WorkBlockInterventionOutcome::DeliverySuppressedDnd,
                    outcome_at: Some(now),
                    // A held decision was never shown, so it carries the
                    // salience it would have had. It is excluded from the
                    // delivered count either way.
                    salience: InterventionSalience::Normal,
                },
            )?;
            return Ok(None);
        }
        self.repo.record_intervention(
            &record.block_id,
            &WorkBlockIntervention {
                offered_at: now,
                action_id: DRIFT_ACTION_ID.to_owned(),
                anchor_category: anchor.clone(),
                switch_count,
                window_seconds: DRIFT_WINDOW_SECONDS.try_into().unwrap_or(u32::MAX),
                outcome: WorkBlockInterventionOutcome::Offered,
                outcome_at: None,
                salience: backoff.salience,
            },
        )?;
        Ok(Some(DriftIntervention {
            block_id: Uuid::parse_str(&record.block_id).unwrap_or_default(),
            action_id: DRIFT_ACTION_ID,
            title: DRIFT_TITLE.to_owned(),
            body: drift_body(switch_count, &anchor),
            salience: backoff.salience,
        }))
    }

    /// Rolling wrong-intervention counts since `since`, for invariant 4.
    pub fn wrong_intervention_counts(
        &self,
        since: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, WorkBlockError> {
        Ok(self.repo.wrong_intervention_counts(since)?)
    }

    /// Derives the current backoff from how the last offers were answered.
    ///
    /// Derived rather than stored: the intervention rows already are the
    /// evidence, and a separate counter could disagree with them after a
    /// crash, a clear, or a manual edit.
    fn backoff_state(&self, now: DateTime<Utc>) -> Result<BackoffState, WorkBlockError> {
        let recent = self.repo.recent_interventions(BACKOFF_HISTORY_LIMIT)?;
        let mut streak: u32 = 0;
        let mut last_negative_at: Option<DateTime<Utc>> = None;
        for intervention in recent.iter().filter(|row| row.outcome.is_terminal()) {
            if !intervention.outcome.is_negative() {
                break;
            }
            if streak == 0 {
                last_negative_at = intervention.outcome_at.or(Some(intervention.offered_at));
            }
            streak = streak.saturating_add(1);
        }
        let Some(since) = last_negative_at else {
            return Ok(BackoffState {
                suppressed: false,
                salience: InterventionSalience::Normal,
            });
        };
        let cooldown = Duration::seconds(
            BACKOFF_BASE_SECONDS
                .saturating_mul(1_i64 << streak.saturating_sub(1).min(16))
                .min(BACKOFF_MAX_SECONDS),
        );
        Ok(BackoffState {
            suppressed: now < since + cooldown,
            // The first offer after a cooldown returns quietly. Full salience
            // is earned back by an offer that lands, never by time alone.
            salience: InterventionSalience::Quiet,
        })
    }

    pub fn accept_recovery(
        &self,
        block_id: Uuid,
        action_id: &str,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        // Closed registry: only a registered action can be accepted, and only
        // the one the terminal result actually offered.
        if !matches!(action_id, DRIFT_ACTION_ID | SOFT_RESTART_ACTION_ID) {
            return Err(WorkBlockError::InvalidRequest);
        }
        let source = self.repo.get(&block_id.to_string())?;
        let Some(result) = self.repo.result(&source.block_id)? else {
            return Err(WorkBlockError::InvalidTransition);
        };
        if !matches!(
            source.phase,
            WorkBlockPhase::Completed | WorkBlockPhase::Abandoned | WorkBlockPhase::Expired
        ) {
            return Err(WorkBlockError::InvalidTransition);
        }
        if result.next_action.action_id != action_id {
            return Err(WorkBlockError::InvalidRequest);
        }
        if let Some(current) = self.repo.latest()? {
            if matches!(
                current.phase,
                WorkBlockPhase::Active | WorkBlockPhase::Paused
            ) {
                return Err(WorkBlockError::InvalidTransition);
            }
        }
        let record = WorkBlockRecord {
            block_id: Uuid::new_v4().to_string(),
            phase: WorkBlockPhase::Active,
            intention: source.intention,
            purpose: source.purpose,
            intensity: source.intensity,
            planned_duration_seconds: RECOVERY_DURATION_SECONDS,
            started_at: now,
            paused_at: None,
            total_paused_seconds: 0,
            ended_at: None,
            recovered_after_restart: false,
            recovery_of: Some(source.block_id),
            // A recovery start is the user's own tap; `recovery_of` keeps it
            // separately identifiable so the R2 comparison of invited versus
            // self-declared blocks can exclude recovery follow-ons.
            origin: WorkBlockOrigin::Manual,
            intention_expires_at: now + Duration::hours(INTENTION_RETENTION_HOURS),
            updated_at: now,
        };
        self.repo.create(&record)?;
        self.publish_deadline(Some(planned_deadline(&record)));
        self.snapshot_for(record, now)
    }

    /// Whether a block is running right now.
    ///
    /// Read-only on purpose: a correction asks this to phrase its confirmation,
    /// and must never end or transition a block as a side effect of asking.
    pub fn has_active_block(&self) -> Result<bool, WorkBlockError> {
        Ok(self
            .repo
            .latest()?
            .is_some_and(|record| record.phase == WorkBlockPhase::Active))
    }

    /// Evaluates the deterministic auto-demotion policy (roadmap invariant
    /// 4; D5) and persists the transition when the state changed.
    ///
    /// The state is a pure function of the stored outcome stream, the last
    /// manual reset, and the clock: the rolling window starts at
    /// `max(now - window, manual_reset_at)`, and the state is `Demoted`
    /// exactly while `delivered >= minimum sample` and
    /// `wrong / delivered > threshold` (strictly). Re-promotion is the same
    /// evaluation ceasing to hold — versioned, deterministic, and never
    /// learned. The persisted singleton only remembers the entered-at
    /// instant for disclosure and the reset marker; it is never a history.
    pub fn evaluate_demotion(
        &self,
        now: DateTime<Utc>,
    ) -> Result<DemotionEvaluation, WorkBlockError> {
        let stored = self.repo.demotion_state()?;
        let manual_reset_at = stored.as_ref().and_then(|state| state.manual_reset_at);
        let mut since = now - Duration::days(WRONG_INTERVENTION_ROLLING_DAYS);
        if let Some(reset_at) = manual_reset_at {
            since = since.max(reset_at);
        }
        let counts = self.repo.wrong_intervention_counts(since)?;
        let over_threshold = counts.delivered >= DEMOTION_MIN_DELIVERED_SAMPLE
            && counts.was_focused.saturating_mul(100)
                > counts.delivered.saturating_mul(DEMOTION_THRESHOLD_PERCENT);
        let previous = stored
            .as_ref()
            .map(|state| state.state)
            .unwrap_or(InterventionDemotionState::Active);
        let state = if over_threshold {
            InterventionDemotionState::Demoted
        } else {
            InterventionDemotionState::Active
        };
        let demoted_at = match (previous, state) {
            (InterventionDemotionState::Demoted, InterventionDemotionState::Demoted) => stored
                .as_ref()
                .and_then(|record| record.demoted_at)
                .or(Some(now)),
            (_, InterventionDemotionState::Demoted) => Some(now),
            _ => None,
        };
        if stored.as_ref().map(|record| record.state) != Some(state)
            || stored.as_ref().and_then(|record| record.demoted_at) != demoted_at
        {
            self.repo.set_demotion_state(&DemotionStateRecord {
                state,
                demoted_at,
                manual_reset_at,
                threshold_policy_version: DEMOTION_THRESHOLD_POLICY_VERSION,
                repromotion_policy_version: DEMOTION_REPROMOTION_POLICY_VERSION,
                updated_at: now,
            })?;
        }
        Ok(DemotionEvaluation {
            state,
            counts,
            demoted_at,
        })
    }

    /// The inspectable demotion state for the disclosure surface. Counts,
    /// versioned constants, current state, and registered copy — nothing
    /// else is representable.
    pub fn demotion_state_payload(
        &self,
        now: DateTime<Utc>,
    ) -> Result<DemotionState, WorkBlockError> {
        let evaluation = self.evaluate_demotion(now)?;
        let demoted = evaluation.state == InterventionDemotionState::Demoted;
        Ok(DemotionState {
            state: match evaluation.state {
                InterventionDemotionState::Active => DemotionStateKind::Active,
                InterventionDemotionState::Demoted => DemotionStateKind::Demoted,
            },
            wrong_count: evaluation.counts.was_focused,
            delivered_count: evaluation.counts.delivered,
            threshold_percent: DEMOTION_THRESHOLD_PERCENT,
            minimum_sample: DEMOTION_MIN_DELIVERED_SAMPLE,
            window_days: WRONG_INTERVENTION_ROLLING_DAYS.unsigned_abs() as u32,
            threshold_policy_version: DEMOTION_THRESHOLD_POLICY_VERSION,
            repromotion_policy_version: DEMOTION_REPROMOTION_POLICY_VERSION,
            demoted_at: demoted.then_some(evaluation.demoted_at).flatten(),
            disclosure: demoted.then(demotion_disclosure_copy),
        })
    }

    /// The user's explicit one-tap resume from the demoted state.
    ///
    /// Restarts the demotion evaluation window at the reset instant and
    /// returns to `Active`. The underlying outcome record and the rolling
    /// wrong-intervention counter are untouched: a reset changes what the
    /// demotion rule looks at, never what happened.
    pub fn reset_demotion(&self, now: DateTime<Utc>) -> Result<DemotionState, WorkBlockError> {
        self.repo.set_demotion_state(&DemotionStateRecord {
            state: InterventionDemotionState::Active,
            demoted_at: None,
            manual_reset_at: Some(now),
            threshold_policy_version: DEMOTION_THRESHOLD_POLICY_VERSION,
            repromotion_policy_version: DEMOTION_REPROMOTION_POLICY_VERSION,
            updated_at: now,
        })?;
        self.demotion_state_payload(now)
    }

    /// One grounded sentence for the block's most recent shown intervention
    /// (D7). Code selects the claim and evidence from the stored row; the
    /// deterministic template phrases it. `phrase_explanation` is the seam
    /// where an optional provider could rephrase the same selection later —
    /// no provider is wired in this release, and the deterministic sentence
    /// is the v1 explanation.
    pub fn explain_intervention(&self, block_id: Uuid) -> Result<Option<String>, WorkBlockError> {
        // One offer per block, so "the offer that was shown" is simply this
        // block's offer — if it was shown at all. A held or withheld decision
        // was never displayed and has nothing to explain.
        let Some(shown) = self
            .repo
            .intervention(&block_id.to_string())?
            .filter(|offer| {
                !matches!(
                    offer.outcome,
                    WorkBlockInterventionOutcome::DeliverySuppressedDnd
                        | WorkBlockInterventionOutcome::WithheldDemotion
                )
            })
        else {
            return Ok(None);
        };
        let selection = ExplanationSelection {
            claim_id: DRIFT_EXPLANATION_CLAIM_ID,
            anchor_category: shown.anchor_category.clone(),
            switch_count: shown.switch_count,
            window_minutes: (shown.window_seconds / 60).max(1),
        };
        Ok(Some(phrase_explanation(&selection, None)))
    }

    pub fn clear_data(&self) -> Result<WorkBlockSnapshot, WorkBlockError> {
        self.repo.clear_all()?;
        self.publish_deadline(None);
        Ok(idle_snapshot())
    }

    fn require(
        &self,
        block_id: Uuid,
        phase: WorkBlockPhase,
    ) -> Result<WorkBlockRecord, WorkBlockError> {
        let record = self.repo.get(&block_id.to_string())?;
        if record.phase != phase {
            return Err(WorkBlockError::InvalidTransition);
        }
        Ok(record)
    }

    fn finish(
        &self,
        record: &WorkBlockRecord,
        phase: WorkBlockPhase,
        ended_at: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        if let Some(result) = self.repo.result(&record.block_id)? {
            self.publish_deadline(None);
            return self.snapshot_with_result(
                self.repo.get(&record.block_id)?,
                ended_at,
                Some(result),
            );
        }
        self.repo
            .close_open_observation(&record.block_id, ended_at)?;
        // Silence is a real outcome, not a gap. `resolve_intervention` only
        // moves an unanswered offer, so an explicit response already given
        // survives the block ending.
        self.repo.resolve_intervention(
            &record.block_id,
            WorkBlockInterventionOutcome::NoResponse,
            ended_at,
        )?;
        let observations = self.repo.observations(&record.block_id)?;
        let elapsed = elapsed_seconds(record, ended_at);
        let result = aggregate_result(record, phase, elapsed, &observations);
        // DND evidence is decided once, at finalization, and persists with the
        // result: a block that completes while DND is active is a success, and
        // a held decision reconciles here as a count — the one calm line below
        // is the only surface a suppressed nudge ever gets.
        //
        // At most one offer exists per block, enforced by the intervention
        // table's primary key, so the held count is 0 or 1 by construction.
        let held_count = u32::from(self.repo.intervention(&record.block_id)?.is_some_and(
            |offer| offer.outcome == WorkBlockInterventionOutcome::DeliverySuppressedDnd,
        ));
        let completed_under_dnd = phase == WorkBlockPhase::Completed && self.focus_active(ended_at);
        let result = with_dnd_evidence(result, completed_under_dnd, held_count);
        let result = self.repo.finalize(
            &record.block_id,
            &WorkBlockCompletion {
                phase,
                ended_at,
                result,
            },
        )?;
        self.publish_deadline(None);
        self.snapshot_with_result(self.repo.get(&record.block_id)?, ended_at, Some(result))
    }

    fn snapshot_for(
        &self,
        record: WorkBlockRecord,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let result = self.repo.result(&record.block_id)?;
        self.snapshot_with_result(record, now, result)
    }

    fn snapshot_with_result(
        &self,
        record: WorkBlockRecord,
        now: DateTime<Utc>,
        result: Option<WorkBlockResult>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        let elapsed = elapsed_seconds(&record, now);
        let remaining = record.planned_duration_seconds.saturating_sub(elapsed);
        let latest = self.repo.latest_observation(&record.block_id)?;
        let (category, status, confidence) = current_evidence(latest.as_ref());
        let ends_at = (record.phase == WorkBlockPhase::Active).then(|| planned_deadline(&record));
        Ok(WorkBlockSnapshot {
            state_version: WORK_BLOCK_STATE_VERSION,
            phase: record.phase,
            block_id: Uuid::parse_str(&record.block_id).ok(),
            intention: record.intention,
            purpose: record.purpose,
            intensity: Some(record.intensity),
            planned_duration_seconds: record.planned_duration_seconds,
            elapsed_duration_seconds: elapsed,
            remaining_duration_seconds: remaining,
            started_at: Some(record.started_at),
            analysis_ended_at: record.ended_at,
            ends_at,
            paused_at: record.paused_at,
            recovered_after_restart: record.recovered_after_restart,
            current_category: category.clone(),
            classification_status: status,
            confidence,
            status_line: status_line(record.phase, record.intensity, category.as_deref(), status),
            result,
            active_intervention: self.active_intervention(&record.block_id, now)?,
        })
    }

    /// The live offer, if one is still awaiting a response. Answered offers are
    /// not surfaced: the card disappears once the user has replied.
    fn active_intervention(
        &self,
        block_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ActiveIntervention>, WorkBlockError> {
        let Some(intervention) = self.repo.intervention(block_id)? else {
            return Ok(None);
        };
        if intervention.outcome.is_terminal() {
            return Ok(None);
        }
        // While demoted, a still-unanswered offer is not re-rendered either:
        // demotion silences every intervention surface, not just new offers.
        // The row stays and resolves normally (`no_response` at block end,
        // or the user's earlier reply), so the record is never rewritten.
        if self.evaluate_demotion(now)?.state == InterventionDemotionState::Demoted {
            return Ok(None);
        }
        Ok(Some(ActiveIntervention {
            action_id: intervention.action_id.clone(),
            title: DRIFT_TITLE.to_owned(),
            body: drift_body(intervention.switch_count, &intervention.anchor_category),
            anchor_category: intervention.anchor_category,
            switch_count: intervention.switch_count,
            window_seconds: intervention.window_seconds,
            offered_at: intervention.offered_at,
            salience: intervention.salience,
        }))
    }

    fn publish_deadline(&self, deadline: Option<DateTime<Utc>>) {
        self.deadline.send_replace(deadline);
    }
}

/// Runs one-shot deadline sleeps. A new command replaces the pending deadline;
/// there is no periodic timer or state polling.
pub async fn run_deadline_scheduler(
    manager: Arc<WorkBlockManager>,
    push: Arc<PushAdapter>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut deadlines = manager.deadline_receiver();
    loop {
        let deadline = *deadlines.borrow_and_update();
        match deadline {
            Some(deadline) => {
                let wait = (deadline - Utc::now()).to_std().unwrap_or_default();
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {
                        match manager.request_state(Utc::now()) {
                            Ok(snapshot) => push.push_work_block_state(snapshot).await,
                            // Only a successful finish clears the deadline
                            // from the watch channel, so after an error the
                            // next pass re-reads the same past deadline with
                            // a zero wait — a 100%-CPU spin against whatever
                            // made the store fail. Back off before retrying;
                            // a deadline change still interrupts immediately
                            // on the next loop pass.
                            Err(_) => {
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                        }
                    }
                    changed = deadlines.changed() => {
                        if changed.is_err() { return; }
                    }
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return; }
                    }
                }
            }
            None => {
                tokio::select! {
                    changed = deadlines.changed() => {
                        if changed.is_err() { return; }
                    }
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return; }
                    }
                }
            }
        }
    }
}

fn normalize_intention(intention: Option<String>) -> Result<Option<String>, WorkBlockError> {
    let Some(value) = intention else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 120 || trimmed.contains(['\n', '\r']) {
        return Err(WorkBlockError::InvalidRequest);
    }
    Ok(Some(trimmed.to_owned()))
}

fn effective_now(record: &WorkBlockRecord, now: DateTime<Utc>) -> DateTime<Utc> {
    now.max(record.updated_at)
}

fn planned_deadline(record: &WorkBlockRecord) -> DateTime<Utc> {
    record.started_at
        + Duration::seconds(
            i64::from(record.planned_duration_seconds) + i64::from(record.total_paused_seconds),
        )
}

fn elapsed_seconds(record: &WorkBlockRecord, now: DateTime<Utc>) -> u32 {
    let end = match record.phase {
        WorkBlockPhase::Paused => record.paused_at.unwrap_or(now),
        WorkBlockPhase::Completed | WorkBlockPhase::Abandoned | WorkBlockPhase::Expired => {
            record.ended_at.unwrap_or(now)
        }
        _ => effective_now(record, now),
    };
    positive_seconds(end - record.started_at).saturating_sub(record.total_paused_seconds)
}

fn positive_seconds(duration: Duration) -> u32 {
    duration.num_seconds().max(0).min(i64::from(u32::MAX)) as u32
}

fn current_evidence(
    observation: Option<&WorkBlockObservation>,
) -> (
    Option<String>,
    ClassificationStatus,
    ClassificationConfidence,
) {
    let Some(observation) = observation else {
        return (
            None,
            ClassificationStatus::Unclassified,
            ClassificationConfidence::None,
        );
    };
    let is_safe = observation.classification_status == ClassificationStatus::Classified
        && matches!(
            observation.classification_confidence,
            ClassificationConfidence::High | ClassificationConfidence::Medium
        );
    (
        is_safe.then(|| observation.category.clone()),
        observation.classification_status,
        observation.classification_confidence,
    )
}

/// True when an observation is strong enough to count as evidence. Mirrors the
/// filter `aggregate_result` applies, so a switch that would not appear in the
/// end-of-block result cannot trigger an offer either.
fn is_confident_evidence(observation: &WorkBlockObservation) -> bool {
    observation.classification_status == ClassificationStatus::Classified
        && matches!(
            observation.classification_confidence,
            ClassificationConfidence::High | ClassificationConfidence::Medium
        )
        && !matches!(
            observation.category.to_ascii_lowercase().as_str(),
            "system" | "unclassified" | "unlogged"
        )
}

/// The category holding the most confidently observed time so far. Ties break
/// on category name so the anchor cannot oscillate between equal candidates.
fn dominant_category(observations: &[WorkBlockObservation]) -> Option<String> {
    let mut category_seconds = HashMap::<String, u32>::new();
    for observation in observations.iter().filter(|o| is_confident_evidence(o)) {
        let Some(ended_at) = observation.ended_at else {
            continue;
        };
        let seconds = positive_seconds(ended_at - observation.occurred_at);
        if seconds == 0 {
            continue;
        }
        let entry = category_seconds
            .entry(observation.category.clone())
            .or_default();
        *entry = entry.saturating_add(seconds);
    }
    category_seconds
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(category, _)| category)
}

/// Maps a user's reply onto the stored vocabulary. Total by construction, so a
/// new reply cannot silently fall through to a default.
fn outcome_for(response: InterventionResponse) -> WorkBlockInterventionOutcome {
    match response {
        InterventionResponse::AcceptedAction => WorkBlockInterventionOutcome::AcceptedAction,
        InterventionResponse::NotHelpful => WorkBlockInterventionOutcome::NotHelpful,
        InterventionResponse::WrongClassification => {
            WorkBlockInterventionOutcome::WrongClassification
        }
        InterventionResponse::WasFocused => WorkBlockInterventionOutcome::WasFocused,
        InterventionResponse::Dismissed => WorkBlockInterventionOutcome::Dismissed,
    }
}

/// Registered analyst-voice demotion disclosure (D5; roadmap invariants 4
/// and 7). States Velvt's own error rate and the pause as respect — no
/// apology spiral, no reference to the user's history, and it never
/// describes the deterministic rule as learned.
fn demotion_disclosure_copy() -> String {
    "Velvt is getting these nudges wrong too often, so it has gone quiet: no nudges will be \
     sent for now, and you can resume them at any time."
        .to_owned()
}

/// Phrases one explanation from a code-selected claim. The provider, when
/// one exists, may only rephrase the same selection; anything it returns
/// that fails validation falls back to the deterministic template for the
/// same selection. With no provider (this release), the deterministic
/// template is the explanation.
fn phrase_explanation(
    selection: &ExplanationSelection,
    provider: Option<&dyn ExplanationPhraser>,
) -> String {
    if let Some(candidate) = provider.and_then(|phraser| phraser.phrase(selection)) {
        if validate_explanation(&candidate, selection) {
            return candidate;
        }
    }
    deterministic_explanation(selection)
}

/// The registered deterministic template for the drift claim. Exactly one
/// sentence, grounded only in the stored row's values, analyst voice.
fn deterministic_explanation(selection: &ExplanationSelection) -> String {
    let anchor = selection
        .anchor_category
        .replace('_', " ")
        .to_ascii_lowercase();
    format!(
        "Velvt offered this nudge because it observed {} switches away from {anchor} in the \
         {} minutes before the offer.",
        selection.switch_count, selection.window_minutes
    )
}

/// The copy gate for a phrased explanation: exactly one sentence, no
/// question or reply hook, no number beyond the selected evidence, the
/// claim's own evidence present, and no banned vocabulary. Anything that
/// fails is discarded in favor of the deterministic template.
fn validate_explanation(sentence: &str, selection: &ExplanationSelection) -> bool {
    let trimmed = sentence.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 240 {
        return false;
    }
    // One sentence: a single terminal period and no other sentence break,
    // question, or exclamation anywhere.
    if !trimmed.ends_with('.') {
        return false;
    }
    let body = &trimmed[..trimmed.len() - 1];
    if body.contains(['.', '?', '!']) {
        return false;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if BANNED_COPY_TOKENS
        .iter()
        .any(|token| lowered.contains(token))
    {
        return false;
    }
    // Grounding: every number in the sentence must be one of the selected
    // values, and the selected evidence must actually appear.
    let allowed = [
        selection.switch_count.to_string(),
        selection.window_minutes.to_string(),
    ];
    let mut digits = String::new();
    for character in trimmed.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() {
            digits.push(character);
        } else if !digits.is_empty() {
            if !allowed.contains(&digits) {
                return false;
            }
            digits.clear();
        }
    }
    let anchor = selection
        .anchor_category
        .replace('_', " ")
        .to_ascii_lowercase();
    lowered.contains(&selection.switch_count.to_string()) && lowered.contains(&anchor)
}

/// Describes only what was observed. No intent, cause, diagnosis, or judgement.
fn drift_body(switch_count: u32, anchor: &str) -> String {
    let minutes = DRIFT_WINDOW_SECONDS / 60;
    // `friendly_category` capitalises for sentence-initial use; this category
    // sits mid-sentence.
    let anchor = anchor.replace('_', " ").to_ascii_lowercase();
    format!(
        "Velvt observed {switch_count} switches away from {anchor} in the last {minutes} minutes. \
         Protect the next {DRIFT_PROTECT_MINUTES} minutes for the work you chose."
    )
}

fn status_line(
    phase: WorkBlockPhase,
    intensity: WorkBlockIntensity,
    category: Option<&str>,
    status: ClassificationStatus,
) -> String {
    match phase {
        WorkBlockPhase::Paused => "Paused. Resume when you are ready.".into(),
        WorkBlockPhase::Completed => "The planned work block is complete.".into(),
        WorkBlockPhase::Abandoned => {
            "This block ended early; the result uses only observed activity.".into()
        }
        WorkBlockPhase::Expired => {
            "The block expired while timing was uncertain; the result is marked accordingly.".into()
        }
        WorkBlockPhase::Idle => "Choose one bounded block to begin.".into(),
        WorkBlockPhase::Active => {
            if status != ClassificationStatus::Classified || category.is_none() {
                return "The current activity is unclear; Velvt is not guessing.".into();
            }
            let category = friendly_category(category.unwrap_or_default());
            match intensity {
                WorkBlockIntensity::Light => format!("Current category: {category}."),
                WorkBlockIntensity::Medium => format!("Current safe category: {category}."),
                WorkBlockIntensity::Intense => format!(
                    "Current category: {category}. Intense mode uses the same calm evidence rules."
                ),
            }
        }
    }
}

fn aggregate_result(
    record: &WorkBlockRecord,
    phase: WorkBlockPhase,
    elapsed: u32,
    observations: &[WorkBlockObservation],
) -> WorkBlockResult {
    let valid = observations
        .iter()
        .filter_map(|observation| {
            let ended_at = observation.ended_at?;
            let seconds = positive_seconds(ended_at - observation.occurred_at);
            let classified = observation.classification_status == ClassificationStatus::Classified
                && matches!(
                    observation.classification_confidence,
                    ClassificationConfidence::High | ClassificationConfidence::Medium
                )
                && !matches!(
                    observation.category.to_ascii_lowercase().as_str(),
                    "system" | "unclassified" | "unlogged"
                );
            (classified && seconds > 0).then(|| (observation.category.clone(), seconds))
        })
        .collect::<Vec<_>>();
    let observed_seconds = valid
        .iter()
        .fold(0_u32, |total, (_, seconds)| total.saturating_add(*seconds));
    let coverage_ratio = if elapsed == 0 {
        0.0
    } else {
        (f64::from(observed_seconds) / f64::from(elapsed)).clamp(0.0, 1.0)
    };
    let coverage = if coverage_ratio < 0.25 {
        WorkBlockCoverage::Insufficient
    } else if coverage_ratio < 0.75 {
        WorkBlockCoverage::Partial
    } else {
        WorkBlockCoverage::Good
    };
    let confidence = match coverage {
        WorkBlockCoverage::Insufficient => ConfidenceLevel::None,
        WorkBlockCoverage::Partial => ConfidenceLevel::Low,
        WorkBlockCoverage::Good if valid.len() >= 2 => ConfidenceLevel::High,
        WorkBlockCoverage::Good => ConfidenceLevel::Medium,
    };

    let mut category_seconds = HashMap::<String, u32>::new();
    for (category, seconds) in &valid {
        *category_seconds.entry(category.clone()).or_default() = category_seconds
            .get(category)
            .copied()
            .unwrap_or_default()
            .saturating_add(*seconds);
    }
    let dominant = category_seconds
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(category, _)| category);

    let mut longest = 0_u32;
    let mut current_category: Option<&str> = None;
    let mut current_stretch = 0_u32;
    let mut switch_aways = 0_u32;
    let mut recoveries = 0_u32;
    let mut was_away_from_dominant = false;
    let mut has_seen_dominant = false;
    for (category, seconds) in &valid {
        if current_category == Some(category.as_str()) {
            current_stretch = current_stretch.saturating_add(*seconds);
        } else {
            current_category = Some(category);
            current_stretch = *seconds;
        }
        longest = longest.max(current_stretch);
        if let Some(dominant) = dominant.as_deref() {
            if category == dominant {
                if has_seen_dominant && was_away_from_dominant {
                    recoveries = recoveries.saturating_add(1);
                }
                has_seen_dominant = true;
                was_away_from_dominant = false;
            } else if has_seen_dominant && !was_away_from_dominant {
                switch_aways = switch_aways.saturating_add(1);
                was_away_from_dominant = true;
            }
        }
    }

    let safe_evidence_category = (coverage != WorkBlockCoverage::Insufficient)
        .then_some(dominant)
        .flatten();
    let observation = if coverage == WorkBlockCoverage::Insufficient {
        "Coverage was incomplete, so Velvt cannot make a confident observation about this block."
            .into()
    } else if switch_aways == 0 {
        format!(
            "Velvt observed one sustained category pattern across {} minutes of covered activity.",
            rounded_minutes(observed_seconds)
        )
    } else {
        format!(
            "Velvt observed {switch_aways} switch-away transitions across {} minutes of covered activity; switching alone does not show distraction.",
            rounded_minutes(observed_seconds)
        )
    };
    WorkBlockResult {
        planned_duration_seconds: record.planned_duration_seconds,
        elapsed_duration_seconds: elapsed,
        longest_uninterrupted_seconds: longest,
        switch_away_count: switch_aways,
        recovery_count: recoveries,
        confidence,
        coverage,
        coverage_ratio,
        safe_evidence_category,
        observation,
        next_action: next_action_for(record, phase),
        dnd_outcomes: Vec::new(),
        reconciliation: None,
    }
}

/// Selects the one bounded next action from the closed registry. An invited
/// block that ended early gets the gentle re-entry (`soft_restart_10`, D4);
/// everything else keeps the 0.1.5 `protect_next_10` behavior unchanged.
fn next_action_for(record: &WorkBlockRecord, phase: WorkBlockPhase) -> WorkBlockNextAction {
    if record.origin == WorkBlockOrigin::Invitation
        && matches!(phase, WorkBlockPhase::Abandoned | WorkBlockPhase::Expired)
    {
        return WorkBlockNextAction {
            action_id: SOFT_RESTART_ACTION_ID.into(),
            label: SOFT_RESTART_LABEL.into(),
            duration_seconds: RECOVERY_DURATION_SECONDS,
        };
    }
    WorkBlockNextAction {
        action_id: DRIFT_ACTION_ID.into(),
        label: recovery_label(record.purpose),
        duration_seconds: RECOVERY_DURATION_SECONDS,
    }
}

/// Applies the Focus/DND evidence to a finalized result. `completed_under_dnd`
/// appears at most once and first; each held decision appears once.
fn with_dnd_evidence(
    mut result: WorkBlockResult,
    completed_under_dnd: bool,
    held_count: u32,
) -> WorkBlockResult {
    if completed_under_dnd {
        result
            .dnd_outcomes
            .push(WorkBlockDndOutcome::CompletedUnderDnd);
    }
    result.dnd_outcomes.extend(std::iter::repeat_n(
        WorkBlockDndOutcome::DeliverySuppressedDnd,
        held_count as usize,
    ));
    result.reconciliation = dnd_reconciliation_copy(completed_under_dnd, held_count);
    result
}

/// The single calm post-block reconciliation line (roadmap invariants 5 and
/// 7; D2, D8). Analyst voice, evidence only: what completed and what was
/// held. It never blames the channel, never references what the user
/// missed, and never turns a held decision into a late nudge.
fn dnd_reconciliation_copy(completed_under_dnd: bool, held_count: u32) -> Option<String> {
    let held = match held_count {
        0 => None,
        1 => Some("Velvt held 1 nudge and delivered nothing mid-block".to_owned()),
        count => Some(format!(
            "Velvt held {count} nudges and delivered nothing mid-block"
        )),
    };
    match (completed_under_dnd, held) {
        (true, Some(held)) => Some(format!(
            "Do Not Disturb was on and the block completed as planned; {held}."
        )),
        (true, None) => Some("Do Not Disturb was on and the block completed as planned.".into()),
        (false, Some(held)) => Some(format!(
            "Do Not Disturb was on for part of this block; {held}."
        )),
        (false, None) => None,
    }
}

fn rounded_minutes(seconds: u32) -> u32 {
    seconds.saturating_add(30) / 60
}

fn recovery_label(purpose: Option<WorkBlockPurpose>) -> String {
    match purpose {
        Some(WorkBlockPurpose::DeepWork) => "Protect the next 10 minutes for deep work.".into(),
        Some(WorkBlockPurpose::Study) => "Protect the next 10 minutes for study.".into(),
        Some(WorkBlockPurpose::CreativePractice) => {
            "Protect the next 10 minutes for creative practice.".into()
        }
        Some(WorkBlockPurpose::HealthyTechUse) => {
            "Protect the next 10 minutes for healthy tech use.".into()
        }
        Some(WorkBlockPurpose::WorkLifeBoundary) => {
            "Protect the next 10 minutes for your work-life boundary.".into()
        }
        None => "Protect the next 10 minutes.".into(),
    }
}

fn friendly_category(category: &str) -> String {
    category
        .replace('_', " ")
        .to_ascii_lowercase()
        .split_whitespace()
        .enumerate()
        .map(|(index, word)| {
            if index == 0 {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn idle_snapshot() -> WorkBlockSnapshot {
    WorkBlockSnapshot {
        state_version: WORK_BLOCK_STATE_VERSION,
        phase: WorkBlockPhase::Idle,
        block_id: None,
        intention: None,
        purpose: None,
        intensity: None,
        planned_duration_seconds: 0,
        elapsed_duration_seconds: 0,
        remaining_duration_seconds: 0,
        started_at: None,
        analysis_ended_at: None,
        ends_at: None,
        paused_at: None,
        recovered_after_restart: false,
        current_category: None,
        classification_status: ClassificationStatus::Unclassified,
        confidence: ClassificationConfidence::None,
        status_line: "Choose one bounded block to begin.".into(),
        result: None,
        active_intervention: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SqlitePersistence;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn manager() -> WorkBlockManager {
        let db = SqlitePersistence::open_in_memory().unwrap();
        WorkBlockManager::new(db.work_block_repo())
    }

    fn request(seconds: u32) -> StartWorkBlock {
        StartWorkBlock {
            intention: Some("Write the local state tests".into()),
            planned_duration_seconds: seconds,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }
    }

    /// Origin marker (R2): an invited start records `invitation`, a manual
    /// start records `manual`, and nothing else about the block differs.
    #[test]
    fn origin_marker_distinguishes_invited_from_manual_declarations() {
        let (manager, repo) = manager_with_repo();
        let manual = manager.start(request(1_500), at(0)).unwrap();
        assert_eq!(
            repo.get(&manual.block_id.unwrap().to_string())
                .unwrap()
                .origin,
            WorkBlockOrigin::Manual
        );
        manager.end(manual.block_id.unwrap(), at(60)).unwrap();

        let invited = manager
            .start_with_origin(request(1_500), WorkBlockOrigin::Invitation, at(120))
            .unwrap();
        let invited_record = repo.get(&invited.block_id.unwrap().to_string()).unwrap();
        assert_eq!(invited_record.origin, WorkBlockOrigin::Invitation);
        // The marker is the only difference: same state machine, same
        // snapshot shape, no invitation detail on the record.
        assert_eq!(invited.phase, WorkBlockPhase::Active);
        assert_eq!(invited.planned_duration_seconds, 1_500);
    }

    /// The origin marker never crosses IPC: the snapshot for an invited
    /// block serializes without any origin or invitation field.
    #[test]
    fn snapshot_carries_no_origin_or_invitation_field() {
        let (manager, _repo) = manager_with_repo();
        let invited = manager
            .start_with_origin(request(1_500), WorkBlockOrigin::Invitation, at(0))
            .unwrap();
        let encoded = serde_json::to_string(&invited).unwrap();
        assert!(
            !encoded.contains("origin"),
            "origin leaked into IPC: {encoded}"
        );
        assert!(
            !encoded.contains("invitation"),
            "invitation detail leaked into IPC: {encoded}"
        );
    }

    /// Gentle re-entry (D4): an invited block that ends early offers the
    /// registered `soft_restart_10`; accepting it starts a ten-minute block
    /// through the same recovery path.
    #[test]
    fn invited_block_ending_early_offers_soft_restart_and_accepting_starts_it() {
        let (manager, repo) = manager_with_repo();
        let invited = manager
            .start_with_origin(request(1_500), WorkBlockOrigin::Invitation, at(0))
            .unwrap();
        let block_id = invited.block_id.unwrap();
        let ended = manager.end(block_id, at(300)).unwrap();
        let result = ended.result.expect("terminal result");
        assert_eq!(result.next_action.action_id, "soft_restart_10");
        assert_eq!(result.next_action.label, SOFT_RESTART_LABEL);
        assert_eq!(result.next_action.duration_seconds, 600);

        // The registered action starts a ten-minute block; the mismatched
        // one is refused even though it is registered.
        assert!(matches!(
            manager.accept_recovery(block_id, "protect_next_10", at(400)),
            Err(WorkBlockError::InvalidRequest)
        ));
        let restarted = manager
            .accept_recovery(block_id, "soft_restart_10", at(400))
            .unwrap();
        assert_eq!(restarted.phase, WorkBlockPhase::Active);
        assert_eq!(restarted.planned_duration_seconds, 600);
        let restarted_record = repo.get(&restarted.block_id.unwrap().to_string()).unwrap();
        assert_eq!(
            restarted_record.recovery_of.as_deref(),
            Some(block_id.to_string().as_str())
        );
        assert_eq!(
            restarted_record.origin,
            WorkBlockOrigin::Manual,
            "a recovery start is the user's own tap; recovery_of keeps it identifiable"
        );
    }

    /// A completed invited block and every manual terminal block keep the
    /// 0.1.5 `protect_next_10` behavior unchanged.
    #[test]
    fn completed_invited_and_manual_blocks_keep_protect_next_10() {
        let (manager, _repo) = manager_with_repo();
        let invited = manager
            .start_with_origin(request(300), WorkBlockOrigin::Invitation, at(0))
            .unwrap();
        let block_id = invited.block_id.unwrap();
        let completed = manager.request_state(at(301)).unwrap();
        assert_eq!(completed.phase, WorkBlockPhase::Completed);
        assert_eq!(
            completed.result.unwrap().next_action.action_id,
            "protect_next_10"
        );
        let _ = block_id;

        let (manager, _repo) = manager_with_repo();
        let manual = manager.start(request(1_500), at(0)).unwrap();
        let ended = manager.end(manual.block_id.unwrap(), at(300)).unwrap();
        assert_eq!(
            ended.result.unwrap().next_action.action_id,
            "protect_next_10"
        );

        // An unregistered action stays refused.
        assert!(matches!(
            manager.accept_recovery(manual.block_id.unwrap(), "escalate_now", at(400)),
            Err(WorkBlockError::InvalidRequest)
        ));
    }

    /// An invited block inherits the 0.1.5 intervention machinery
    /// unchanged: the same drift gate fires the same registered offer.
    #[test]
    fn invited_block_flows_the_unchanged_drift_machinery() {
        let (manager, repo) = manager_with_repo();
        manager
            .start_with_origin(request(3_600), WorkBlockOrigin::Invitation, at(0))
            .unwrap();
        let outcome = drift_into_offer(&manager).unwrap();
        let intervention = outcome.intervention.expect("the drift gate is unchanged");
        assert_eq!(intervention.action_id, "protect_next_10");
        // One offer per block, so the assertion is that the block has exactly
        // one — read through the singular accessor the shipped gate uses.
        let recorded = repo
            .intervention(&repo.latest().unwrap().unwrap().block_id)
            .unwrap();
        assert!(recorded.is_some());
    }

    /// Manager plus the repo behind it, so a test can assert the recorded
    /// outcome and not just the returned offer.
    fn manager_with_repo() -> (WorkBlockManager, Arc<dyn WorkBlockRepo>) {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let repo = db.work_block_repo();
        (WorkBlockManager::new(repo.clone()), repo)
    }

    fn observe(
        manager: &WorkBlockManager,
        category: &str,
        seconds: i64,
    ) -> Option<ObservationOutcome> {
        manager
            .observe_safe_category(
                category,
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                at(seconds),
            )
            .unwrap()
    }

    /// Establishes DEEP_WORK as the anchor, then switches away four times
    /// inside the ten-minute window.
    fn drift_into_offer(manager: &WorkBlockManager) -> Option<ObservationOutcome> {
        observe(manager, "DEEP_WORK", 10);
        observe(manager, "COMMUNICATION", 400);
        observe(manager, "DEEP_WORK", 420);
        observe(manager, "COMMUNICATION", 440);
        observe(manager, "DEEP_WORK", 460);
        observe(manager, "COMMUNICATION", 480);
        observe(manager, "DEEP_WORK", 500);
        observe(manager, "COMMUNICATION", 520)
    }

    /// Runs one complete block that drifts, optionally answers the offer, and
    /// ends. Returns the offer if one was delivered.
    ///
    /// Backoff is only observable across blocks — the gate allows one offer per
    /// block — so every backoff test needs a prior block with a real outcome.
    fn drift_block(
        manager: &WorkBlockManager,
        start: i64,
        response: Option<InterventionResponse>,
    ) -> Option<DriftIntervention> {
        let active = manager.start(request(3600), at(start)).unwrap();
        let block_id = active.block_id.unwrap();
        let mut offer = None;
        for (offset, category) in [
            (10, "DEEP_WORK"),
            (400, "COMMUNICATION"),
            (420, "DEEP_WORK"),
            (440, "COMMUNICATION"),
            (460, "DEEP_WORK"),
            (480, "COMMUNICATION"),
            (500, "DEEP_WORK"),
            (520, "COMMUNICATION"),
        ] {
            if let Some(outcome) = manager
                .observe_safe_category(
                    category,
                    ClassificationStatus::Classified,
                    ClassificationConfidence::High,
                    at(start + offset),
                )
                .unwrap()
            {
                offer = offer.or(outcome.intervention);
            }
        }
        if let Some(response) = response {
            manager
                .report_intervention_outcome(block_id, response, at(start + 540))
                .unwrap();
        }
        manager.end(block_id, at(start + 560)).unwrap();
        offer
    }

    /// An offer never fires on the observation that returns to the anchor.
    ///
    /// The switch threshold can be crossed while the warm-up still holds the
    /// gate shut. When the warm-up then expires on a return to the anchor, the
    /// accumulated evidence is spent at the exact moment the user is back at
    /// work: the offer would be untruthful, would self-resolve as `returned`
    /// without a fresh departure, and would invite an honest `was_focused`
    /// reply that pollutes the wrong-intervention rate with a false positive
    /// the policy caused. The evidence is deferred, not discarded.
    #[test]
    fn no_offer_fires_on_the_observation_that_returns_to_the_anchor() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3_600), at(0)).unwrap();
        let block_id = active.block_id.unwrap().to_string();

        // Four departures, all inside the five-minute warm-up, so the gate
        // returns early every time and no offer is possible yet. DEEP_WORK
        // holds the longer dwell throughout and stays the anchor.
        for (category, seconds) in [
            ("DEEP_WORK", 10),
            ("COMMUNICATION", 60),
            ("DEEP_WORK", 70),
            ("COMMUNICATION", 120),
            ("DEEP_WORK", 130),
            ("COMMUNICATION", 180),
            ("DEEP_WORK", 190),
            ("COMMUNICATION", 240),
        ] {
            let outcome = observe(&manager, category, seconds);
            assert!(
                outcome.is_none_or(|o| o.intervention.is_none()),
                "an offer cleared the warm-up gate at t={seconds}"
            );
        }

        // Warm-up expired and the four switches are still inside the window,
        // but this observation is the return to the anchor: no offer.
        let returned = observe(&manager, "DEEP_WORK", 310).unwrap();
        assert!(
            returned.intervention.is_none(),
            "an offer fired on the observation that returned to the anchor"
        );
        assert!(
            repo.intervention(&block_id).unwrap().is_none(),
            "an offer was recorded for the return to the anchor"
        );

        // The next confident departure spends the deferred evidence instead.
        let departed = observe(&manager, "COMMUNICATION", 330).unwrap();
        let offer = departed
            .intervention
            .expect("the deferred offer fires on the next confident departure");
        assert_eq!(offer.salience, InterventionSalience::Normal);
        let recorded = repo
            .intervention(&block_id)
            .unwrap()
            .expect("the departure offer is recorded");
        assert_eq!(recorded.offered_at, at(330));
    }

    // Removed with scope 4's integration: four tests of an in-block
    // re-offer cooldown multiplier —
    //   a_dismissal_doubles_the_reoffer_cooldown_and_reduces_salience
    //   a_positive_outcome_keeps_standard_salience_and_base_cooldown
    //   copy_does_not_escalate_after_a_dismissal
    //   each_negative_reply_multiplies_the_cooldown_again
    // That mechanism never shipped. This build makes one offer per block and
    // backs off across blocks by reducing salience, which invariant 2 is
    // already tested for here by `a_pushed_away_offer_suppresses_the_next_one`,
    // `each_further_dismissal_doubles_the_cooldown`, and
    // `the_offer_after_a_cooldown_returns_quietly`. Keeping tests for a
    // mechanism the product does not have would assert a fiction.

    #[test]
    fn a_pushed_away_offer_suppresses_the_next_one() {
        let manager = manager();
        assert!(drift_block(&manager, 0, Some(InterventionResponse::Dismissed)).is_some());

        // One hour later, inside the two-hour cooldown.
        let offer = drift_block(&manager, 3_600, None);

        assert!(
            offer.is_none(),
            "a dismissal must buy quiet, not a second attempt"
        );
    }

    /// Invariant 2: backoff, never escalation. The offer that eventually
    /// returns is quieter than the one that was pushed away — never louder,
    /// and never more emotionally charged.
    #[test]
    fn the_offer_after_a_cooldown_returns_quietly() {
        let (manager, repo) = manager_with_repo();
        drift_block(&manager, 0, Some(InterventionResponse::WasFocused));

        let offer = drift_block(&manager, 8_000, None).expect("the cooldown has elapsed");

        assert_eq!(offer.salience, InterventionSalience::Quiet);
        assert_eq!(offer.title, DRIFT_TITLE, "copy is unchanged by backoff");
        let recorded = repo.recent_interventions(1).unwrap();
        assert_eq!(
            recorded[0].salience,
            InterventionSalience::Quiet,
            "how the offer was delivered is part of its record"
        );
    }

    #[test]
    fn each_further_dismissal_doubles_the_cooldown() {
        let manager = manager();
        drift_block(&manager, 0, Some(InterventionResponse::Dismissed));
        drift_block(&manager, 8_000, Some(InterventionResponse::Dismissed));

        // Two dismissals: the cooldown is now four hours, so an attempt three
        // hours later is still too soon.
        assert!(drift_block(&manager, 18_000, None).is_none());
        assert!(drift_block(&manager, 24_000, None).is_some());
    }

    /// The streak resets on evidence the offer helped, so a user who returns to
    /// work is not permanently down-weighted for one bad day.
    #[test]
    fn an_accepted_offer_restores_normal_salience() {
        let manager = manager();
        drift_block(&manager, 0, Some(InterventionResponse::Dismissed));
        drift_block(&manager, 8_000, Some(InterventionResponse::AcceptedAction));

        let offer = drift_block(&manager, 16_000, None).expect("the streak was cleared");

        assert_eq!(offer.salience, InterventionSalience::Normal);
    }

    /// Silence is not refusal. An offer delivered while the Mac was untouched
    /// must not suppress the next one — otherwise a user who never saw the
    /// first offer is quietly opted out of the feature.
    #[test]
    fn an_unanswered_offer_does_not_trigger_backoff() {
        let manager = manager();
        assert!(drift_block(&manager, 0, None).is_some());

        let offer = drift_block(&manager, 3_600, None).expect("silence is not a refusal");

        assert_eq!(offer.salience, InterventionSalience::Normal);
    }

    #[test]
    fn sustained_switching_offers_one_grounded_recovery_action() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();

        let outcome = drift_into_offer(&manager).expect("observation returns state");
        let intervention = outcome
            .intervention
            .expect("four confident switches should clear the gate");

        assert_eq!(intervention.action_id, DRIFT_ACTION_ID);
        assert_eq!(intervention.block_id, block_id);
        // Copy reports observation only: no intent, cause, or judgement.
        assert!(intervention.body.contains("4 switches away from deep work"));
        assert!(intervention.body.contains("last 10 minutes"));

        let recorded = repo
            .intervention(&block_id.to_string())
            .unwrap()
            .expect("the offer is persisted so its outcome can be observed");
        assert_eq!(recorded.anchor_category, "DEEP_WORK");
        assert_eq!(recorded.switch_count, 4);
        assert_eq!(recorded.outcome, WorkBlockInterventionOutcome::Offered);
    }

    #[test]
    fn at_most_one_offer_is_made_per_block() {
        let (manager, _repo) = manager_with_repo();
        manager.start(request(3600), at(0)).unwrap();
        assert!(drift_into_offer(&manager).unwrap().intervention.is_some());

        // Keep drifting well past the gate; the cap holds.
        for (index, seconds) in [560, 580, 600, 620, 640].iter().enumerate() {
            let category = if index % 2 == 0 {
                "DEEP_WORK"
            } else {
                "COMMUNICATION"
            };
            let outcome = observe(&manager, category, *seconds).unwrap();
            assert!(
                outcome.intervention.is_none(),
                "a second offer was made at t={seconds}"
            );
        }
    }

    // Removed: `no_more_than_three_offers_are_made_per_block`.
    // That test asserted a cap of three offers per block. This build makes
    // one offer per block, enforced by the intervention table's primary key
    // and relied on by the pre-registered primary metric's denominator. A
    // cap of three is a different product decision, not a weaker form of
    // ours, so the test is dropped rather than loosened.

    #[test]
    fn returning_to_the_anchor_records_the_outcome() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap().to_string();
        drift_into_offer(&manager);

        observe(&manager, "DEEP_WORK", 560);

        let recorded = repo.intervention(&block_id).unwrap().unwrap();
        assert_eq!(recorded.outcome, WorkBlockInterventionOutcome::Returned);
        assert_eq!(recorded.outcome_at, Some(at(560)));
    }

    #[test]
    fn an_offer_without_a_response_records_silence_when_the_block_ends() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);

        manager.end(block_id, at(900)).unwrap();

        let recorded = repo.intervention(&block_id.to_string()).unwrap().unwrap();
        assert_eq!(recorded.outcome, WorkBlockInterventionOutcome::NoResponse);
    }

    #[test]
    fn a_recorded_return_is_not_overwritten_by_block_expiry() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        observe(&manager, "DEEP_WORK", 560);

        manager.end(block_id, at(900)).unwrap();

        let recorded = repo.intervention(&block_id.to_string()).unwrap().unwrap();
        assert_eq!(recorded.outcome, WorkBlockInterventionOutcome::Returned);
        assert_eq!(recorded.outcome_at, Some(at(560)));
    }

    /// The measurement this whole slice exists for: silence and disagreement
    /// must not land in the same bucket.
    #[test]
    fn each_user_response_is_recorded_distinctly() {
        for (response, expected) in [
            (
                InterventionResponse::AcceptedAction,
                WorkBlockInterventionOutcome::AcceptedAction,
            ),
            (
                InterventionResponse::NotHelpful,
                WorkBlockInterventionOutcome::NotHelpful,
            ),
            (
                InterventionResponse::WrongClassification,
                WorkBlockInterventionOutcome::WrongClassification,
            ),
            (
                InterventionResponse::WasFocused,
                WorkBlockInterventionOutcome::WasFocused,
            ),
            (
                InterventionResponse::Dismissed,
                WorkBlockInterventionOutcome::Dismissed,
            ),
        ] {
            let (manager, repo) = manager_with_repo();
            let active = manager.start(request(3600), at(0)).unwrap();
            let block_id = active.block_id.unwrap();
            drift_into_offer(&manager);

            manager
                .report_intervention_outcome(block_id, response, at(540))
                .unwrap();

            let recorded = repo.intervention(&block_id.to_string()).unwrap().unwrap();
            assert_eq!(recorded.outcome, expected, "for response {response:?}");
            assert_eq!(recorded.outcome_at, Some(at(540)));
        }
    }

    #[test]
    fn an_explicit_response_survives_the_block_ending() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(block_id, InterventionResponse::NotHelpful, at(540))
            .unwrap();

        manager.end(block_id, at(900)).unwrap();

        let recorded = repo.intervention(&block_id.to_string()).unwrap().unwrap();
        assert_eq!(recorded.outcome, WorkBlockInterventionOutcome::NotHelpful);
        assert_eq!(recorded.outcome_at, Some(at(540)));
    }

    #[test]
    fn a_second_response_does_not_overwrite_the_first() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(block_id, InterventionResponse::AcceptedAction, at(540))
            .unwrap();

        // A double tap is a no-op, not an error.
        manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(560))
            .unwrap();

        let recorded = repo.intervention(&block_id.to_string()).unwrap().unwrap();
        assert_eq!(
            recorded.outcome,
            WorkBlockInterventionOutcome::AcceptedAction
        );
        assert_eq!(recorded.outcome_at, Some(at(540)));
    }

    #[test]
    fn reporting_without_an_offer_is_rejected() {
        let (manager, _repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();

        assert!(manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(60))
            .is_err());
    }

    /// The in-app card is the primary surface, so the snapshot must carry the
    /// offer while it is unanswered and drop it once answered.
    #[test]
    fn the_snapshot_carries_the_offer_only_while_it_is_unanswered() {
        let (manager, _repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();

        let offered = drift_into_offer(&manager).unwrap().snapshot;
        let card = offered
            .active_intervention
            .expect("an unanswered offer renders in-app");
        assert_eq!(card.action_id, DRIFT_ACTION_ID);
        assert_eq!(card.anchor_category, "DEEP_WORK");
        assert_eq!(card.switch_count, 4);
        assert!(card.body.contains("4 switches away from deep work"));

        let answered = manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(540))
            .unwrap();
        assert!(answered.active_intervention.is_none());
    }

    #[test]
    fn no_offer_is_made_before_the_block_has_an_anchor() {
        let (manager, _repo) = manager_with_repo();
        manager.start(request(3600), at(0)).unwrap();
        // Same switching shape, but inside the first five minutes.
        observe(&manager, "DEEP_WORK", 10);
        for (index, seconds) in [40, 60, 80, 100, 120, 140, 160].iter().enumerate() {
            let category = if index % 2 == 0 {
                "COMMUNICATION"
            } else {
                "DEEP_WORK"
            };
            let outcome = observe(&manager, category, *seconds).unwrap();
            assert!(outcome.intervention.is_none());
        }
    }

    #[test]
    fn no_offer_is_made_when_too_little_of_the_block_remains() {
        let (manager, _repo) = manager_with_repo();
        // 600s block: by t=520 only 80s remain, under the two-minute floor.
        manager.start(request(600), at(0)).unwrap();
        assert!(drift_into_offer(&manager).unwrap().intervention.is_none());
    }

    #[test]
    fn weak_evidence_abstains_rather_than_guessing() {
        let (manager, _repo) = manager_with_repo();
        manager.start(request(3600), at(0)).unwrap();
        observe(&manager, "DEEP_WORK", 10);
        // Ambiguous, low-confidence switches are not evidence of anything.
        for seconds in [400, 440, 480, 520] {
            let outcome = manager
                .observe_safe_category(
                    "COMMUNICATION",
                    ClassificationStatus::Ambiguous,
                    ClassificationConfidence::Low,
                    at(seconds),
                )
                .unwrap();
            assert!(outcome.is_none_or(|o| o.intervention.is_none()));
        }
    }

    /// Analyst voice (roadmap invariant 7): registered copy reports evidence.
    /// No "still", no moralizing, and no reference to the user's dismissal or
    /// failure history anywhere in the registry.
    #[test]
    fn registered_copy_is_analyst_voice_with_no_history_references() {
        let mut registry: Vec<String> = vec![
            DRIFT_TITLE.to_owned(),
            SOFT_RESTART_LABEL.to_owned(),
            drift_body(4, "DEEP_WORK"),
            demotion_disclosure_copy(),
            deterministic_explanation(&ExplanationSelection {
                claim_id: DRIFT_EXPLANATION_CLAIM_ID,
                anchor_category: "DEEP_WORK".into(),
                switch_count: 5,
                window_minutes: 10,
            }),
        ];
        for (completed_under_dnd, held_count) in
            [(true, 0), (true, 1), (true, 3), (false, 1), (false, 3)]
        {
            registry.extend(dnd_reconciliation_copy(completed_under_dnd, held_count));
        }
        for purpose in [
            None,
            Some(WorkBlockPurpose::DeepWork),
            Some(WorkBlockPurpose::Study),
            Some(WorkBlockPurpose::CreativePractice),
            Some(WorkBlockPurpose::HealthyTechUse),
            Some(WorkBlockPurpose::WorkLifeBoundary),
        ] {
            registry.push(recovery_label(purpose));
        }
        for phase in [
            WorkBlockPhase::Idle,
            WorkBlockPhase::Active,
            WorkBlockPhase::Paused,
            WorkBlockPhase::Completed,
            WorkBlockPhase::Abandoned,
            WorkBlockPhase::Expired,
        ] {
            for intensity in [
                WorkBlockIntensity::Light,
                WorkBlockIntensity::Medium,
                WorkBlockIntensity::Intense,
            ] {
                registry.push(status_line(
                    phase,
                    intensity,
                    Some("DEEP_WORK"),
                    ClassificationStatus::Classified,
                ));
                registry.push(status_line(
                    phase,
                    intensity,
                    None,
                    ClassificationStatus::Ambiguous,
                ));
            }
        }

        for copy in &registry {
            let lowered = copy.to_ascii_lowercase();
            for forbidden in BANNED_COPY_TOKENS {
                assert!(
                    !lowered.contains(forbidden),
                    "{forbidden:?} in registered copy {copy:?}"
                );
            }
        }
    }

    #[test]
    fn state_transitions_are_bounded_and_terminal_completion_is_idempotent() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        let id = active.block_id.unwrap();
        assert_eq!(active.phase, WorkBlockPhase::Active);
        assert_eq!(
            manager.pause(id, at(60)).unwrap().phase,
            WorkBlockPhase::Paused
        );
        assert_eq!(
            manager.resume(id, at(120)).unwrap().phase,
            WorkBlockPhase::Active
        );
        let completed = manager.request_state(at(360)).unwrap();
        assert_eq!(completed.phase, WorkBlockPhase::Completed);
        let again = manager.request_state(at(500)).unwrap();
        assert_eq!(completed.result, again.result);
    }

    #[test]
    fn restart_recovers_unexpired_and_expires_overdue_blocks_once() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let first = WorkBlockManager::new(db.work_block_repo());
        let active = first.start(request(300), at(0)).unwrap();
        let recovered = WorkBlockManager::new(db.work_block_repo())
            .recover_after_restart(at(120))
            .unwrap();
        assert!(recovered.recovered_after_restart);
        assert_eq!(recovered.phase, WorkBlockPhase::Active);
        let expired_manager = WorkBlockManager::new(db.work_block_repo());
        let expired = expired_manager.recover_after_restart(at(600)).unwrap();
        assert_eq!(expired.phase, WorkBlockPhase::Expired);
        assert_eq!(
            expired.result,
            expired_manager
                .recover_after_restart(at(700))
                .unwrap()
                .result
        );
        assert_eq!(expired.block_id, active.block_id);
    }

    #[test]
    fn insufficient_and_ambiguous_coverage_never_make_confident_claims() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        manager
            .observe_safe_category(
                "COMMUNICATION",
                ClassificationStatus::Ambiguous,
                ClassificationConfidence::Low,
                at(10),
            )
            .unwrap();
        let result = manager
            .end(active.block_id.unwrap(), at(120))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.coverage, WorkBlockCoverage::Insufficient);
        assert_eq!(result.confidence, ConfidenceLevel::None);
        assert!(result.safe_evidence_category.is_none());
        assert!(result
            .observation
            .contains("cannot make a confident observation"));
    }

    #[test]
    fn result_has_exactly_one_singular_bounded_action() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        let result = manager
            .end(active.block_id.unwrap(), at(120))
            .unwrap()
            .result
            .unwrap();
        assert_eq!(result.next_action.action_id, "protect_next_10");
        assert_eq!(result.next_action.duration_seconds, 600);
        let json = serde_json::to_value(result).unwrap();
        assert!(json.get("next_action").unwrap().is_object());
        assert!(json.get("next_actions").is_none());
    }

    #[test]
    fn sleep_pauses_and_wake_does_not_invent_elapsed_time() {
        let manager = manager();
        manager.start(request(300), at(0)).unwrap();
        let paused = manager
            .lifecycle(WorkBlockLifecycleEvent::Sleep, at(60))
            .unwrap();
        assert_eq!(paused.phase, WorkBlockPhase::Paused);
        let wake = manager
            .lifecycle(WorkBlockLifecycleEvent::Wake, at(3600))
            .unwrap();
        assert_eq!(wake.phase, WorkBlockPhase::Paused);
        assert_eq!(wake.elapsed_duration_seconds, 60);
    }

    #[test]
    fn rust_derives_stretches_switches_recoveries_and_non_accusatory_copy() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        let id = active.block_id.unwrap();
        for (category, second) in [
            ("FOCUS_WORK", 0),
            ("COMMUNICATION", 60),
            ("FOCUS_WORK", 120),
        ] {
            manager
                .observe_safe_category(
                    category,
                    ClassificationStatus::Classified,
                    ClassificationConfidence::High,
                    at(second),
                )
                .unwrap();
        }

        let result = manager.end(id, at(300)).unwrap().result.unwrap();
        assert_eq!(result.coverage, WorkBlockCoverage::Good);
        assert_eq!(result.longest_uninterrupted_seconds, 180);
        assert_eq!(result.switch_away_count, 1);
        assert_eq!(result.recovery_count, 1);
        assert_eq!(result.safe_evidence_category.as_deref(), Some("FOCUS_WORK"));
        assert!(result
            .observation
            .contains("switching alone does not show distraction"));
        assert!(!result.observation.contains("failed"));
    }

    #[test]
    fn backward_clock_and_timezone_change_preserve_the_block() {
        let manager = manager();
        let active = manager.start(request(300), at(100)).unwrap();
        let after_clock = manager
            .lifecycle(WorkBlockLifecycleEvent::ClockChanged, at(50))
            .unwrap();
        assert_eq!(after_clock.phase, WorkBlockPhase::Active);
        assert_eq!(after_clock.elapsed_duration_seconds, 0);
        let after_zone = manager
            .lifecycle(WorkBlockLifecycleEvent::TimeZoneChanged, at(150))
            .unwrap();
        assert_eq!(after_zone.phase, WorkBlockPhase::Active);
        assert_eq!(after_zone.block_id, active.block_id);
    }

    #[test]
    fn accepting_the_only_recovery_action_starts_one_bounded_block() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        let completed = manager.end(active.block_id.unwrap(), at(120)).unwrap();
        let recovery = manager
            .accept_recovery(
                completed.block_id.unwrap(),
                &completed.result.unwrap().next_action.action_id,
                at(121),
            )
            .unwrap();
        assert_eq!(recovery.phase, WorkBlockPhase::Active);
        assert_eq!(recovery.planned_duration_seconds, 600);
        assert_eq!(
            recovery.intention.as_deref(),
            Some("Write the local state tests")
        );
        assert!(manager
            .accept_recovery(completed.block_id.unwrap(), "another_action", at(122))
            .is_err());
    }

    #[test]
    fn commands_arriving_after_the_deadline_complete_instead_of_extending() {
        let manager = manager();
        let active = manager.start(request(300), at(0)).unwrap();
        assert_eq!(
            manager
                .pause(active.block_id.unwrap(), at(301))
                .unwrap()
                .phase,
            WorkBlockPhase::Completed
        );

        let next = manager.start(request(300), at(400)).unwrap();
        assert_eq!(
            manager.end(next.block_id.unwrap(), at(701)).unwrap().phase,
            WorkBlockPhase::Completed
        );
    }

    #[test]
    fn backward_clock_after_a_terminal_block_does_not_resurrect_it() {
        let manager = manager();
        let first = manager.start(request(300), at(100)).unwrap();
        manager.end(first.block_id.unwrap(), at(200)).unwrap();
        let second = manager.start(request(300), at(50)).unwrap();

        let current = manager.request_state(at(60)).unwrap();
        assert_eq!(current.block_id, second.block_id);
        assert_eq!(current.phase, WorkBlockPhase::Active);
    }

    #[test]
    fn invalid_edges_are_rejected_without_mutating_the_state_machine() {
        let manager = manager();
        assert!(manager.pause(Uuid::new_v4(), at(0)).is_err());
        let active = manager.start(request(300), at(0)).unwrap();
        let id = active.block_id.unwrap();
        assert_eq!(active.state_version, WORK_BLOCK_STATE_VERSION);
        assert!(manager.start(request(300), at(1)).is_err());
        assert!(manager.resume(id, at(1)).is_err());

        let paused = manager.pause(id, at(2)).unwrap();
        assert_eq!(paused.phase, WorkBlockPhase::Paused);
        assert!(manager.pause(id, at(3)).is_err());
        assert!(manager.start(request(300), at(3)).is_err());

        let abandoned = manager.end(id, at(4)).unwrap();
        assert_eq!(abandoned.phase, WorkBlockPhase::Abandoned);
        assert!(manager.pause(id, at(5)).is_err());
        assert!(manager.resume(id, at(5)).is_err());
        assert_eq!(manager.end(id, at(5)).unwrap().result, abandoned.result);
    }

    #[test]
    fn confidence_flapping_on_one_departure_is_not_four_switches() {
        let manager = manager();
        manager.start(request(3_600), at(0)).unwrap();
        observe(&manager, "DEEP_WORK", 10);

        // One real departure after the warm-up gate. The classifier then
        // flaps confidence on the same category; every flap appends a new
        // confident observation row, but the user switched away once.
        let confidences = [
            (ClassificationConfidence::High, 400),
            (ClassificationConfidence::Medium, 420),
            (ClassificationConfidence::High, 440),
            (ClassificationConfidence::Medium, 460),
            (ClassificationConfidence::High, 480),
        ];
        for (confidence, seconds) in confidences {
            let outcome = manager
                .observe_safe_category(
                    "COMMUNICATION",
                    ClassificationStatus::Classified,
                    confidence,
                    at(seconds),
                )
                .unwrap()
                .expect("changed evidence returns state");
            assert!(
                outcome.intervention.is_none(),
                "one departure must not clear the four-switch gate"
            );
        }
    }

    #[test]
    fn ending_a_paused_block_does_not_count_the_final_pause_as_elapsed_work() {
        let manager = manager();
        let active = manager.start(request(3_600), at(0)).unwrap();
        let id = active.block_id.unwrap();
        manager.pause(id, at(60)).unwrap();

        // The user walks away paused and only ends the block hours later.
        // The pause span is not work time: the terminal snapshot, its
        // result, and every later re-read must agree on 60 seconds.
        let ended = manager.end(id, at(28_800)).unwrap();
        let result = ended.result.clone().unwrap();
        assert_eq!(result.elapsed_duration_seconds, 60);
        assert_eq!(ended.elapsed_duration_seconds, 60);
        assert_eq!(ended.remaining_duration_seconds, 3_540);

        let reread = manager.end(id, at(28_900)).unwrap();
        assert_eq!(reread.elapsed_duration_seconds, 60);
        assert_eq!(reread.result.unwrap().elapsed_duration_seconds, 60);
    }

    /// Mutable fake Focus/DND source answering from explicit active ranges,
    /// so decisions keyed to specific instants (like the planned deadline)
    /// stay deterministic.
    struct FakeFocus(std::sync::Mutex<Vec<(DateTime<Utc>, DateTime<Utc>)>>);

    impl FakeFocus {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(Vec::new())))
        }

        fn set_active(&self, from: DateTime<Utc>, until: DateTime<Utc>) {
            self.0.lock().unwrap().push((from, until));
        }
    }

    impl FocusStateSource for FakeFocus {
        fn is_focus_active(&self, at: DateTime<Utc>) -> bool {
            self.0
                .lock()
                .unwrap()
                .iter()
                .any(|(from, until)| (*from..*until).contains(&at))
        }
    }

    fn manager_with_focus() -> (WorkBlockManager, Arc<dyn WorkBlockRepo>, Arc<FakeFocus>) {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let repo = db.work_block_repo();
        let focus = FakeFocus::new();
        let manager = WorkBlockManager::new(repo.clone())
            .with_focus_source(focus.clone() as Arc<dyn FocusStateSource>);
        (manager, repo, focus)
    }

    /// Roadmap invariants 1 and 5, D2: when the drift gate clears while DND
    /// is active, the decision is recorded and held. Nothing is delivered on
    /// any channel — no notification, no in-app card — and the row is
    /// terminal at creation.
    #[test]
    fn dnd_suppresses_delivery_and_records_the_held_decision() {
        let (manager, repo, focus) = manager_with_focus();
        focus.set_active(at(0), at(3_600));
        let active = manager.start(request(3_600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();

        let outcome = drift_into_offer(&manager).expect("observation returns state");
        assert!(
            outcome.intervention.is_none(),
            "a nudge was delivered against active DND"
        );
        assert!(
            outcome.snapshot.active_intervention.is_none(),
            "a held decision must not surface as an in-app card"
        );

        let recorded = repo
            .intervention(&block_id.to_string())
            .unwrap()
            .expect("the held decision is recorded");
        assert_eq!(
            recorded.outcome,
            WorkBlockInterventionOutcome::DeliverySuppressedDnd
        );
        assert!(recorded.outcome.is_terminal());
        assert!(recorded.outcome_at.is_some());

        // Never delivered means never counted as delivered: the
        // wrong-intervention precision metric excludes held decisions.
        let counts = manager.wrong_intervention_counts(at(600)).unwrap();
        assert_eq!(counts.delivered, 0);
        assert_eq!(counts.was_focused, 0);
    }

    /// D2 and D8: a block that completes while DND is active is a success.
    /// It stays a completed block everywhere, records `completed_under_dnd`,
    /// and the result carries one positive, non-channel-blaming line.
    #[test]
    fn a_block_completing_under_dnd_records_success_with_positive_framing() {
        let (manager, _repo, focus) = manager_with_focus();
        focus.set_active(at(0), at(400));
        manager.start(request(300), at(0)).unwrap();
        observe(&manager, "DEEP_WORK", 10);

        let completed = manager.request_state(at(301)).unwrap();
        assert_eq!(completed.phase, WorkBlockPhase::Completed);
        let result = completed.result.unwrap();
        assert_eq!(
            result.dnd_outcomes,
            vec![WorkBlockDndOutcome::CompletedUnderDnd]
        );
        let line = result.reconciliation.expect("one calm line");
        assert!(line.contains("completed as planned"));
        assert!(!line.to_ascii_lowercase().contains("missed"));
    }

    /// Held decisions reconcile after the block as counts inside one calm
    /// line — never as a late nudge on any surface.
    #[test]
    fn held_nudges_reconcile_after_the_block_as_a_count_only() {
        let (manager, repo, focus) = manager_with_focus();
        focus.set_active(at(0), at(3_600));
        let active = manager.start(request(3_600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);

        let ended = manager.end(block_id, at(700)).unwrap();
        assert!(
            ended.active_intervention.is_none(),
            "reconciliation must never resurrect a held nudge"
        );
        let result = ended.result.unwrap();
        assert_eq!(
            result.dnd_outcomes,
            vec![WorkBlockDndOutcome::DeliverySuppressedDnd]
        );
        let line = result.reconciliation.expect("one calm line");
        assert!(line.contains("held 1 nudge"));
        assert!(line.contains("delivered nothing mid-block"));
        // The held row stays terminal after the block resolves silence.
        let rows = repo
            .intervention(&block_id.to_string())
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].outcome,
            WorkBlockInterventionOutcome::DeliverySuppressedDnd
        );
    }

    /// Backoff composition (roadmap invariant 2, requirement 9): a
    /// suppressed decision starts the same base cooldown a delivered offer
    /// does — DND lifting mid-block never releases the held nudge early —
    /// and it is not a negative reply, so it neither multiplies the cooldown
    /// nor reduces the salience of a later offer.
    #[test]
    fn dnd_toggled_mid_block_holds_cooldown_without_touching_backoff() {
        let (manager, repo, focus) = manager_with_focus();
        focus.set_active(at(0), at(600));
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        assert_eq!(
            repo.intervention(&block_id.to_string())
                .unwrap()
                .unwrap()
                .outcome,
            WorkBlockInterventionOutcome::DeliverySuppressedDnd
        );

        // DND turns off at t=600. Gate-clearing switching resumes inside the
        // base cooldown (until 520 + 900 = 1420): still nothing delivered —
        // the held decision is not released late when DND lifts.
        observe(&manager, "DEEP_WORK", 540);
        for (category, seconds) in [
            ("COMMUNICATION", 700),
            ("DEEP_WORK", 720),
            ("COMMUNICATION", 740),
            ("DEEP_WORK", 760),
            ("COMMUNICATION", 780),
            ("DEEP_WORK", 800),
            ("COMMUNICATION", 820),
            ("DEEP_WORK", 840),
        ] {
            let outcome = observe(&manager, category, seconds).unwrap();
            assert!(
                outcome.intervention.is_none(),
                "delivery inside the post-suppression cooldown at t={seconds}"
            );
        }

        // Past the base cooldown, fresh evidence still earns nothing further in
        // this block: the gate allows one offer per block, and the held
        // decision already occupied it.
        //
        // The branch this test came from allowed several offers per block and
        // asserted a second row here. That is not the shipped behaviour — one
        // offer per block is enforced by the intervention table's primary key,
        // and the pre-registered primary metric's denominator counts one row
        // per block. What the test still proves, and what matters, is that
        // suppression neither shortened a wait nor raised salience, and that
        // the held decision is never rewritten into a delivery.
        observe(&manager, "COMMUNICATION", 1_430);
        observe(&manager, "DEEP_WORK", 1_440);
        observe(&manager, "COMMUNICATION", 1_450);
        observe(&manager, "DEEP_WORK", 1_460);
        observe(&manager, "COMMUNICATION", 1_470);
        observe(&manager, "DEEP_WORK", 1_480);
        observe(&manager, "COMMUNICATION", 1_490);

        let held = repo
            .intervention(&block_id.to_string())
            .unwrap()
            .expect("the held decision is the block's one intervention row");
        assert_eq!(
            held.outcome,
            WorkBlockInterventionOutcome::DeliverySuppressedDnd,
            "the held decision is never rewritten into a delivery"
        );
    }

    /// Restart while a suppressed decision is on record: the held row and
    /// its reconciliation survive service restarts unchanged.
    #[test]
    fn restart_during_suppression_preserves_the_held_record() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let focus = FakeFocus::new();
        focus.set_active(at(0), at(600));
        let first = WorkBlockManager::new(db.work_block_repo())
            .with_focus_source(focus.clone() as Arc<dyn FocusStateSource>);
        let active = first.start(request(3_600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&first);
        drop(first);

        let second = WorkBlockManager::new(db.work_block_repo());
        let recovered = second.recover_after_restart(at(900)).unwrap();
        assert_eq!(recovered.phase, WorkBlockPhase::Active);
        assert!(recovered.active_intervention.is_none());

        let result = second.end(block_id, at(1_000)).unwrap().result.unwrap();
        assert_eq!(
            result.dnd_outcomes,
            vec![WorkBlockDndOutcome::DeliverySuppressedDnd]
        );
        assert!(result.reconciliation.unwrap().contains("held 1 nudge"));
    }

    /// Clear-all-data removes held decisions with everything else.
    #[test]
    fn clear_data_removes_held_dnd_decisions() {
        let (manager, repo, focus) = manager_with_focus();
        focus.set_active(at(0), at(3_600));
        let active = manager.start(request(3_600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        assert!(repo.intervention(&block_id.to_string()).unwrap().is_some());

        let cleared = manager.clear_data().unwrap();
        assert_eq!(cleared.phase, WorkBlockPhase::Idle);
        assert!(repo.intervention(&block_id.to_string()).unwrap().is_none());
    }

    #[test]
    fn identical_evidence_does_not_create_repeated_status_pushes() {
        let manager = manager();
        manager.start(request(300), at(0)).unwrap();
        assert!(manager
            .observe_safe_category(
                "FOCUS_WORK",
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                at(1),
            )
            .unwrap()
            .is_some());
        assert!(manager
            .observe_safe_category(
                "FOCUS_WORK",
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                at(60),
            )
            .unwrap()
            .is_none());
    }
}
