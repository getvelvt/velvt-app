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
    InterventionResponse, StartWorkBlock, WorkBlockCoverage, WorkBlockIntensity,
    WorkBlockLifecycleEvent, WorkBlockNextAction, WorkBlockPhase, WorkBlockPurpose,
    WorkBlockResult, WorkBlockSnapshot, WORK_BLOCK_STATE_VERSION,
};

use crate::{
    delivery::PushAdapter,
    persistence::{
        PersistenceError, WorkBlockCategoryCorrection, WorkBlockCompletion, WorkBlockIntervention,
        WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord, WorkBlockRepo,
        WrongInterventionCounts,
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
/// Versioned backoff policy (`plan/05-unified-roadmap.md` invariant 2).
/// A re-offer inside the same block waits out a cooldown that every negative
/// reply multiplies, and delivery salience only ever decreases after a
/// negative reply. Escalation in salience, frequency, or emotional charge in
/// response to non-compliance is a policy violation, not a tuning option.
const DRIFT_BACKOFF_POLICY_VERSION: u32 = 1;
const DRIFT_REOFFER_BASE_COOLDOWN_SECONDS: i64 = 15 * 60;
const DRIFT_BACKOFF_COOLDOWN_MULTIPLIER: u32 = 2;
const DRIFT_MAX_OFFERS_PER_BLOCK: usize = 3;
/// Rolling window for the local wrong-intervention counter
/// (`dismissed_was_focused` replies / interventions delivered). 0.1.6
/// attaches auto-demotion (roadmap invariant 4) to this stream; 0.1.5 only
/// keeps the number honest.
const WRONG_INTERVENTION_ROLLING_DAYS: i64 = 14;
/// The only action in the registry today. Closed by construction: the schema
/// constrains `action_id`, so an unregistered action cannot be persisted.
const DRIFT_ACTION_ID: &str = "protect_next_10";
const DRIFT_PROTECT_MINUTES: u32 = 10;
const DRIFT_TITLE: &str = "Your work block is running";

/// How prominently an offer may be delivered. `Standard` permits the optional
/// OS notification; `Reduced` is the in-app card only. Salience never
/// increases in response to non-compliance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftSalience {
    Standard,
    Reduced,
}

/// A single approved, device-local intervention offer. Copy is authored here,
/// beside the evidence that justifies it; Swift renders it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftIntervention {
    pub block_id: Uuid,
    pub action_id: &'static str,
    pub title: String,
    pub body: String,
    pub salience: DriftSalience,
}

/// Result of a safe category observation: the state Swift renders, plus at most
/// one intervention to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationOutcome {
    pub snapshot: WorkBlockSnapshot,
    pub intervention: Option<DriftIntervention>,
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
    deadline: watch::Sender<Option<DateTime<Utc>>>,
}

impl WorkBlockManager {
    pub fn new(repo: Arc<dyn WorkBlockRepo>) -> Self {
        let (deadline, _) = watch::channel(None);
        Self { repo, deadline }
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
        if response == InterventionResponse::WrongClassification {
            self.record_block_category_correction(&record, &existing, now)?;
        }
        self.snapshot_for(record, now)
    }

    /// Believes a wrong-classification reply instantly (roadmap invariant 3):
    /// the most recent confidently observed non-anchor category counts as the
    /// block's focus work for this block, and the snapshot acknowledges it in
    /// copy until the block ends. Durable per-activity training stays in the
    /// existing personal-override correction path.
    fn record_block_category_correction(
        &self,
        record: &WorkBlockRecord,
        intervention: &WorkBlockIntervention,
        now: DateTime<Utc>,
    ) -> Result<(), WorkBlockError> {
        let observations = self.repo.observations(&record.block_id)?;
        let Some(disputed) = observations
            .iter()
            .filter(|observation| is_confident_evidence(observation))
            .rfind(|observation| {
                !observation
                    .category
                    .eq_ignore_ascii_case(&intervention.anchor_category)
            })
        else {
            // Nothing confidently observed to reclassify; the reply itself is
            // already recorded.
            return Ok(());
        };
        self.repo.record_category_correction(
            &record.block_id,
            &WorkBlockCategoryCorrection {
                category: disputed.category.clone(),
                counts_as_category: intervention.anchor_category.clone(),
                corrected_at: now,
            },
        )?;
        Ok(())
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
        let corrections = self.repo.category_corrections(&record.block_id)?;
        let effective = corrections
            .iter()
            .find(|correction| correction.category.eq_ignore_ascii_case(category))
            .map(|correction| correction.counts_as_category.as_str())
            .unwrap_or(category);
        if !pending.anchor_category.eq_ignore_ascii_case(effective) {
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
        let prior = self.repo.interventions(&record.block_id)?;
        // A live offer must be answered, observed, or outlived by the block
        // before anything else is considered; offers never overlap.
        if prior.iter().any(|offer| !offer.outcome.is_terminal()) {
            return Ok(None);
        }
        if prior.len() >= DRIFT_MAX_OFFERS_PER_BLOCK {
            return Ok(None);
        }
        let negative_replies = count_negative_replies(&prior);
        if let Some(last) = prior.last() {
            // Backoff, never escalation: each negative reply multiplies the
            // remaining cooldown by the versioned constant. Nothing in this
            // policy can shorten a wait in response to non-compliance.
            let cooldown = DRIFT_REOFFER_BASE_COOLDOWN_SECONDS.saturating_mul(i64::from(
                DRIFT_BACKOFF_COOLDOWN_MULTIPLIER.saturating_pow(negative_replies),
            ));
            if now < last.offered_at + Duration::seconds(cooldown) {
                return Ok(None);
            }
        }
        let observations = self.repo.observations(&record.block_id)?;
        let corrections = self.repo.category_corrections(&record.block_id)?;
        let Some(anchor) = dominant_category(&observations, &corrections) else {
            return Ok(None);
        };
        let mut window_start = now - Duration::seconds(DRIFT_WINDOW_SECONDS);
        if let Some(last) = prior.last() {
            // Materially new evidence only: switches that predate the previous
            // offer were already spent on it.
            window_start = window_start.max(last.offered_at);
        }
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
            .map(|observation| {
                effective_category(observation, &corrections).eq_ignore_ascii_case(&anchor)
            });
        let mut switch_count = 0_u32;
        for observation in observations
            .iter()
            .filter(|observation| observation.occurred_at >= window_start)
            .filter(|observation| is_confident_evidence(observation))
        {
            let is_anchor =
                effective_category(observation, &corrections).eq_ignore_ascii_case(&anchor);
            if !is_anchor && previous_was_anchor == Some(true) {
                switch_count = switch_count.saturating_add(1);
            }
            previous_was_anchor = Some(is_anchor);
        }
        if switch_count < DRIFT_MIN_SWITCHES {
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
                backoff_policy_version: DRIFT_BACKOFF_POLICY_VERSION,
                outcome: WorkBlockInterventionOutcome::Offered,
                outcome_at: None,
            },
        )?;
        // Reduced salience after any negative reply in this block, and only
        // ever in that direction. Copy is untouched: the same registered
        // template renders every offer, however the previous one was received.
        let salience = if negative_replies > 0 {
            DriftSalience::Reduced
        } else {
            DriftSalience::Standard
        };
        Ok(Some(DriftIntervention {
            block_id: Uuid::parse_str(&record.block_id).unwrap_or_default(),
            action_id: DRIFT_ACTION_ID,
            title: DRIFT_TITLE.to_owned(),
            body: drift_body(switch_count, &anchor),
            salience,
        }))
    }

    pub fn accept_recovery(
        &self,
        block_id: Uuid,
        action_id: &str,
        now: DateTime<Utc>,
    ) -> Result<WorkBlockSnapshot, WorkBlockError> {
        if action_id != "protect_next_10" {
            return Err(WorkBlockError::InvalidRequest);
        }
        let source = self.repo.get(&block_id.to_string())?;
        if !matches!(
            source.phase,
            WorkBlockPhase::Completed | WorkBlockPhase::Abandoned | WorkBlockPhase::Expired
        ) || self.repo.result(&source.block_id)?.is_none()
        {
            return Err(WorkBlockError::InvalidTransition);
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
            intention_expires_at: now + Duration::hours(INTENTION_RETENTION_HOURS),
            updated_at: now,
        };
        self.repo.create(&record)?;
        self.publish_deadline(Some(planned_deadline(&record)));
        self.snapshot_for(record, now)
    }

    /// Local, content-free wrong-intervention counter over the rolling
    /// window. Two bounded integers, computed on demand from stored outcome
    /// enums; never uploaded and absent from every IPC payload.
    pub fn wrong_intervention_counts(
        &self,
        now: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, WorkBlockError> {
        Ok(self
            .repo
            .wrong_intervention_counts(now - Duration::days(WRONG_INTERVENTION_ROLLING_DAYS))?)
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
        let corrections = self.repo.category_corrections(&record.block_id)?;
        let elapsed = elapsed_seconds(record, ended_at);
        let result = aggregate_result(record, elapsed, &observations, &corrections);
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
        let correction_acknowledgment = self.correction_acknowledgment(&record)?;
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
            active_intervention: self.active_intervention(&record.block_id)?,
            correction_acknowledgment,
        })
    }

    /// A believed correction is acknowledged immediately and for the rest of
    /// the block, then the acknowledgment ends with the block.
    fn correction_acknowledgment(
        &self,
        record: &WorkBlockRecord,
    ) -> Result<Option<String>, WorkBlockError> {
        if !matches!(
            record.phase,
            WorkBlockPhase::Active | WorkBlockPhase::Paused
        ) {
            return Ok(None);
        }
        Ok(self
            .repo
            .category_corrections(&record.block_id)?
            .last()
            .map(correction_acknowledgment_copy))
    }

    /// The live offer, if one is still awaiting a response. Answered offers are
    /// not surfaced: the card disappears once the user has replied.
    fn active_intervention(
        &self,
        block_id: &str,
    ) -> Result<Option<ActiveIntervention>, WorkBlockError> {
        let Some(intervention) = self.repo.intervention(block_id)? else {
            return Ok(None);
        };
        if intervention.outcome.is_terminal() {
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

/// The category an observation counts as after block-scoped corrections. A
/// believed correction covers the whole block: the user said the category
/// *is* their focus work for this block, not that it became so mid-way.
fn effective_category<'a>(
    observation: &'a WorkBlockObservation,
    corrections: &'a [WorkBlockCategoryCorrection],
) -> &'a str {
    corrections
        .iter()
        .find(|correction| correction.category.eq_ignore_ascii_case(&observation.category))
        .map(|correction| correction.counts_as_category.as_str())
        .unwrap_or(&observation.category)
}

/// Immediate, visible acknowledgment of a believed correction. Analyst voice:
/// it states the new rule and references no history.
fn correction_acknowledgment_copy(correction: &WorkBlockCategoryCorrection) -> String {
    let category = correction.category.replace('_', " ").to_ascii_lowercase();
    format!("Got it — {category} counts as focus work for this block.")
}

/// The category holding the most confidently observed time so far. Ties break
/// on category name so the anchor cannot oscillate between equal candidates.
fn dominant_category(
    observations: &[WorkBlockObservation],
    corrections: &[WorkBlockCategoryCorrection],
) -> Option<String> {
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
            .entry(effective_category(observation, corrections).to_owned())
            .or_default();
        *entry = entry.saturating_add(seconds);
    }
    category_seconds
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
        .map(|(category, _)| category)
}

/// Negative replies drive backoff. A return, an acceptance, silence, or a
/// category dispute is not "leave me alone" evidence and does not multiply
/// the cooldown.
fn count_negative_replies(prior: &[WorkBlockIntervention]) -> u32 {
    prior
        .iter()
        .filter(|offer| {
            matches!(
                offer.outcome,
                WorkBlockInterventionOutcome::Dismissed
                    | WorkBlockInterventionOutcome::DismissedWasFocused
                    | WorkBlockInterventionOutcome::NotHelpful
            )
        })
        .count() as u32
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
        InterventionResponse::Dismissed => WorkBlockInterventionOutcome::Dismissed,
        InterventionResponse::DismissedWasFocused => {
            WorkBlockInterventionOutcome::DismissedWasFocused
        }
    }
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
    elapsed: u32,
    observations: &[WorkBlockObservation],
    corrections: &[WorkBlockCategoryCorrection],
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
            (classified && seconds > 0)
                .then(|| (effective_category(observation, corrections).to_owned(), seconds))
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
        next_action: WorkBlockNextAction {
            action_id: "protect_next_10".into(),
            label: recovery_label(record.purpose),
            duration_seconds: RECOVERY_DURATION_SECONDS,
        },
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
        correction_acknowledgment: None,
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
        }
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
        assert_eq!(
            recorded.backoff_policy_version,
            DRIFT_BACKOFF_POLICY_VERSION
        );
        assert_eq!(intervention.salience, DriftSalience::Standard);
    }

    #[test]
    fn a_recent_offer_gates_reoffers_for_the_base_cooldown() {
        let (manager, _repo) = manager_with_repo();
        manager.start(request(3600), at(0)).unwrap();
        assert!(drift_into_offer(&manager).unwrap().intervention.is_some());

        // Keep drifting right after the offer; the cooldown holds.
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

    /// Roadmap invariant 2, half one: a dismissal multiplies the remaining
    /// cooldown by the versioned constant. Gate-clearing evidence after the
    /// base cooldown but inside the doubled one must stay silent.
    #[test]
    fn a_dismissal_doubles_the_reoffer_cooldown_and_reduces_salience() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager).unwrap().intervention.unwrap();
        manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(540))
            .unwrap();

        // Anchor holds, then fresh gate-clearing switching resumes. The base
        // cooldown (900s from t=520) has long passed by t=1920; only the
        // doubled cooldown (until t=2320) explains continued silence.
        observe(&manager, "DEEP_WORK", 560);
        for (category, seconds) in [
            ("COMMUNICATION", 1920),
            ("DEEP_WORK", 1940),
            ("COMMUNICATION", 1960),
            ("DEEP_WORK", 1980),
            ("COMMUNICATION", 2000),
            ("DEEP_WORK", 2020),
            ("COMMUNICATION", 2040),
            ("DEEP_WORK", 2260),
            ("COMMUNICATION", 2300),
        ] {
            let outcome = observe(&manager, category, seconds).unwrap();
            assert!(
                outcome.intervention.is_none(),
                "an offer inside the doubled cooldown at t={seconds}"
            );
        }

        // Past 520 + 2 * 900 the same evidence shape is offer-worthy again
        // at the very next evaluation, but only at reduced salience: the
        // in-app card without the notification.
        let reoffer = observe(&manager, "DEEP_WORK", 2330)
            .unwrap()
            .intervention
            .expect("fresh evidence past the doubled cooldown offers again");
        assert_eq!(reoffer.salience, DriftSalience::Reduced);
        assert_eq!(repo.interventions(&block_id.to_string()).unwrap().len(), 2);
    }

    /// Two negative replies quadruple the cooldown: silence between the
    /// doubled and quadrupled marks is what distinguishes 2^2 from 2^1.
    #[test]
    fn each_negative_reply_multiplies_the_cooldown_again() {
        let (manager, _repo) = manager_with_repo();
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(540))
            .unwrap();
        observe(&manager, "DEEP_WORK", 560);
        observe(&manager, "DEEP_WORK", 2330);
        observe(&manager, "COMMUNICATION", 2340);
        observe(&manager, "DEEP_WORK", 2350);
        observe(&manager, "COMMUNICATION", 2355);
        observe(&manager, "DEEP_WORK", 2358);
        for (category, seconds) in
            [("COMMUNICATION", 2359), ("DEEP_WORK", 2360), ("COMMUNICATION", 2361)]
        {
            observe(&manager, category, seconds);
        }
        // Second offer exists by now; answer it not-helpful.
        manager
            .report_intervention_outcome(block_id, InterventionResponse::NotHelpful, at(2380))
            .unwrap();

        // Fresh switching after the doubled cooldown from the second offer
        // would have cleared a 2x policy, but two negatives mean 4x.
        observe(&manager, "DEEP_WORK", 2400);
        for (category, seconds) in [
            ("COMMUNICATION", 5620),
            ("DEEP_WORK", 5640),
            ("COMMUNICATION", 5660),
            ("DEEP_WORK", 5680),
            ("COMMUNICATION", 5700),
            ("DEEP_WORK", 5720),
            ("COMMUNICATION", 5740),
        ] {
            let outcome = observe(&manager, category, seconds).unwrap();
            assert!(
                outcome.intervention.is_none(),
                "an offer inside the quadrupled cooldown at t={seconds}"
            );
        }
        observe(&manager, "DEEP_WORK", 5960);
        let third = observe(&manager, "COMMUNICATION", 6000)
            .unwrap()
            .intervention
            .expect("fresh evidence past the quadrupled cooldown offers again");
        assert_eq!(third.salience, DriftSalience::Reduced);
    }

    /// Roadmap invariant 2, half two: nothing about how an offer renders may
    /// escalate after a dismissal. Same registered template, same title, no
    /// reference to the dismissal.
    #[test]
    fn copy_does_not_escalate_after_a_dismissal() {
        let (manager, _repo) = manager_with_repo();
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        let first = drift_into_offer(&manager).unwrap().intervention.unwrap();
        manager
            .report_intervention_outcome(block_id, InterventionResponse::Dismissed, at(540))
            .unwrap();
        observe(&manager, "DEEP_WORK", 560);
        for (category, seconds) in [
            ("COMMUNICATION", 2340),
            ("DEEP_WORK", 2350),
            ("COMMUNICATION", 2355),
            ("DEEP_WORK", 2358),
            ("COMMUNICATION", 2359),
            ("DEEP_WORK", 2360),
        ] {
            observe(&manager, category, seconds);
        }
        let second = observe(&manager, "COMMUNICATION", 2361)
            .unwrap()
            .intervention
            .expect("post-dismissal reoffer");

        assert_eq!(second.title, first.title);
        assert_eq!(second.action_id, first.action_id);
        assert!(second.body.contains("switches away from deep work"));
        assert!(second.body.contains("Protect the next 10 minutes"));
        for copy in [&second.title, &second.body] {
            let lowered = copy.to_ascii_lowercase();
            for forbidden in ["dismiss", "again", "ignored", "last time", "failed"] {
                assert!(
                    !lowered.contains(forbidden),
                    "escalating or history-referencing copy {forbidden:?} in {copy:?}"
                );
            }
        }
    }

    /// Backoff is driven by negative replies only. A user who returned is not
    /// told off with a quieter card: the next offer keeps standard salience
    /// and the base cooldown.
    #[test]
    fn a_positive_outcome_keeps_standard_salience_and_base_cooldown() {
        let (manager, _repo) = manager_with_repo();
        manager.start(request(3600), at(0)).unwrap();
        let first = drift_into_offer(&manager).unwrap().intervention.unwrap();
        assert_eq!(first.salience, DriftSalience::Standard);
        // Observed return resolves the offer without a negative reply.
        observe(&manager, "DEEP_WORK", 560);

        for (category, seconds) in [
            ("COMMUNICATION", 1430),
            ("DEEP_WORK", 1450),
            ("COMMUNICATION", 1470),
            ("DEEP_WORK", 1490),
            ("COMMUNICATION", 1510),
            ("DEEP_WORK", 1530),
        ] {
            observe(&manager, category, seconds);
        }
        let second = observe(&manager, "COMMUNICATION", 1550)
            .unwrap()
            .intervention
            .expect("base cooldown plus fresh evidence offers again");
        // t=1550 is far inside the doubled window (520 + 1800), so a negative
        // multiplier was not applied; and salience stays standard.
        assert_eq!(second.salience, DriftSalience::Standard);
    }

    #[test]
    fn no_more_than_three_offers_are_made_per_block() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap().to_string();
        drift_into_offer(&manager);
        observe(&manager, "DEEP_WORK", 560);

        let mut base = 1_430_i64;
        for _ in 0..2 {
            for (category, offset) in [
                ("COMMUNICATION", 0),
                ("DEEP_WORK", 20),
                ("COMMUNICATION", 40),
                ("DEEP_WORK", 60),
                ("COMMUNICATION", 80),
                ("DEEP_WORK", 100),
                ("COMMUNICATION", 120),
            ] {
                observe(&manager, category, base + offset);
            }
            observe(&manager, "DEEP_WORK", base + 140);
            base += 1_000;
        }
        assert_eq!(repo.interventions(&block_id).unwrap().len(), 3);

        // The cap holds against any further gate-clearing evidence.
        for (category, offset) in [
            ("COMMUNICATION", 0),
            ("DEEP_WORK", 20),
            ("COMMUNICATION", 40),
            ("DEEP_WORK", 60),
            ("COMMUNICATION", 80),
            ("DEEP_WORK", 100),
            ("COMMUNICATION", 120),
        ] {
            let outcome = observe(&manager, category, base + offset);
            assert!(outcome.map_or(true, |o| o.intervention.is_none()));
        }
        assert_eq!(repo.interventions(&block_id).unwrap().len(), 3);
    }

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
                InterventionResponse::Dismissed,
                WorkBlockInterventionOutcome::Dismissed,
            ),
            (
                InterventionResponse::DismissedWasFocused,
                WorkBlockInterventionOutcome::DismissedWasFocused,
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

    /// Requirement 14: the counter is `dismissed_was_focused` over delivered,
    /// rolls across blocks, ages out, and dies with clear-all-data. It feeds
    /// 0.1.6 auto-demotion; nothing in 0.1.5 reads it into IPC or uploads.
    #[test]
    fn wrong_intervention_counter_rolls_across_blocks_and_clears() {
        let (manager, _repo) = manager_with_repo();

        // Block one: the interruption itself was wrong.
        let first = manager.start(request(3600), at(0)).unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(
                first.block_id.unwrap(),
                InterventionResponse::DismissedWasFocused,
                at(540),
            )
            .unwrap();
        manager.end(first.block_id.unwrap(), at(900)).unwrap();

        // Block two: a plain dismissal, which must not count as wrong.
        let second = manager.start(request(3600), at(2_000)).unwrap();
        observe(&manager, "DEEP_WORK", 2_010);
        observe(&manager, "COMMUNICATION", 2_400);
        observe(&manager, "DEEP_WORK", 2_420);
        observe(&manager, "COMMUNICATION", 2_440);
        observe(&manager, "DEEP_WORK", 2_460);
        observe(&manager, "COMMUNICATION", 2_480);
        observe(&manager, "DEEP_WORK", 2_500);
        assert!(observe(&manager, "COMMUNICATION", 2_520)
            .unwrap()
            .intervention
            .is_some());
        manager
            .report_intervention_outcome(
                second.block_id.unwrap(),
                InterventionResponse::Dismissed,
                at(2_540),
            )
            .unwrap();
        manager.end(second.block_id.unwrap(), at(3_000)).unwrap();

        let counts = manager.wrong_intervention_counts(at(3_100)).unwrap();
        assert_eq!(counts.delivered, 2);
        assert_eq!(counts.was_focused, 1);

        // Rolling: offers age out of the window.
        let fifteen_days = 15 * 24 * 60 * 60;
        let aged = manager.wrong_intervention_counts(at(fifteen_days)).unwrap();
        assert_eq!(aged.delivered, 0);
        assert_eq!(aged.was_focused, 0);

        // Clear-all-data removes the stream entirely.
        manager.clear_data().unwrap();
        let cleared = manager.wrong_intervention_counts(at(3_100)).unwrap();
        assert_eq!(cleared.delivered, 0);
        assert_eq!(cleared.was_focused, 0);
    }

    /// Roadmap invariant 3: a correction is believed instantly, acknowledged
    /// visibly, and applied for the remainder of the block.
    #[test]
    fn wrong_classification_is_believed_instantly_and_acknowledged() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);

        let acknowledged = manager
            .report_intervention_outcome(block_id, InterventionResponse::WrongClassification, at(540))
            .unwrap();

        // Acknowledged immediately and visibly, in analyst voice.
        assert_eq!(
            acknowledged.correction_acknowledgment.as_deref(),
            Some("Got it — communication counts as focus work for this block.")
        );
        assert!(acknowledged.active_intervention.is_none());
        let recorded = repo
            .category_corrections(&block_id.to_string())
            .unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].category, "COMMUNICATION");
        assert_eq!(recorded[0].counts_as_category, "DEEP_WORK");

        // Still acknowledged later in the same block.
        let later = manager.request_state(at(600)).unwrap();
        assert!(later.correction_acknowledgment.is_some());

        // The result honors the correction: the disputed category counts as
        // the block's focus work, so no switch-away is claimed against it.
        let ended = manager.end(block_id, at(900)).unwrap();
        assert!(ended.correction_acknowledgment.is_none());
        let result = ended.result.unwrap();
        assert_eq!(result.switch_away_count, 0);
        assert_eq!(result.safe_evidence_category.as_deref(), Some("DEEP_WORK"));
        assert!(result.observation.contains("one sustained category pattern"));
    }

    #[test]
    fn a_corrected_category_cannot_retrigger_the_gate() {
        let (manager, repo) = manager_with_repo();
        let active = manager.start(request(10_800), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(block_id, InterventionResponse::WrongClassification, at(540))
            .unwrap();

        // Heavy switching into the corrected category, well past every
        // cooldown. Corrected observations count as the anchor, so there is
        // no departure evidence to offer against.
        for (index, seconds) in
            (0..40).map(|step| (step, 2_500 + i64::from(step) * 30))
        {
            let category = if index % 2 == 0 {
                "COMMUNICATION"
            } else {
                "DEEP_WORK"
            };
            let outcome = observe(&manager, category, seconds);
            assert!(
                outcome.map_or(true, |o| o.intervention.is_none()),
                "an offer against the corrected category at t={seconds}"
            );
        }
        assert_eq!(repo.interventions(&block_id.to_string()).unwrap().len(), 1);
    }

    /// The correction is scoped to its block: a new block starts from the
    /// device's ordinary classification.
    #[test]
    fn a_correction_ends_with_its_block() {
        let (manager, _repo) = manager_with_repo();
        let active = manager.start(request(3600), at(0)).unwrap();
        let block_id = active.block_id.unwrap();
        drift_into_offer(&manager);
        manager
            .report_intervention_outcome(block_id, InterventionResponse::WrongClassification, at(540))
            .unwrap();
        manager.end(block_id, at(900)).unwrap();

        let next = manager.start(request(3600), at(1_000)).unwrap();
        assert!(next.correction_acknowledgment.is_none());
        // The same drift shape in the new block clears the gate again.
        observe(&manager, "DEEP_WORK", 1_010);
        observe(&manager, "COMMUNICATION", 1_400);
        observe(&manager, "DEEP_WORK", 1_420);
        observe(&manager, "COMMUNICATION", 1_440);
        observe(&manager, "DEEP_WORK", 1_460);
        observe(&manager, "COMMUNICATION", 1_480);
        observe(&manager, "DEEP_WORK", 1_500);
        let outcome = observe(&manager, "COMMUNICATION", 1_520).unwrap();
        assert!(outcome.intervention.is_some());
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
            assert!(outcome.map_or(true, |o| o.intervention.is_none()));
        }
    }

    /// Analyst voice (roadmap invariant 7): registered copy reports evidence.
    /// No "still", no moralizing, and no reference to the user's dismissal or
    /// failure history anywhere in the registry.
    #[test]
    fn registered_copy_is_analyst_voice_with_no_history_references() {
        let mut registry: Vec<String> = vec![
            DRIFT_TITLE.to_owned(),
            drift_body(4, "DEEP_WORK"),
            correction_acknowledgment_copy(&WorkBlockCategoryCorrection {
                category: "COMMUNICATION".into(),
                counts_as_category: "DEEP_WORK".into(),
                corrected_at: at(0),
            }),
        ];
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
            for forbidden in [
                "still",
                "dismiss",
                "failed",
                "failure",
                "ignored",
                "last time",
                "again",
            ] {
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
