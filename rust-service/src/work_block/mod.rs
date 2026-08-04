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
        PersistenceError, WorkBlockCompletion, WorkBlockIntervention, WorkBlockInterventionOutcome,
        WorkBlockObservation, WorkBlockRecord, WorkBlockRepo,
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
/// The only action in the registry today. Closed by construction: the schema
/// constrains `action_id`, so an unregistered action cannot be persisted.
const DRIFT_ACTION_ID: &str = "protect_next_10";
const DRIFT_PROTECT_MINUTES: u32 = 10;
const DRIFT_TITLE: &str = "Your work block is still running";

/// A single approved, device-local intervention offer. Copy is authored here,
/// beside the evidence that justifies it; Swift renders it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftIntervention {
    pub block_id: Uuid,
    pub action_id: &'static str,
    pub title: String,
    pub body: String,
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
            },
        )?;
        Ok(Some(DriftIntervention {
            block_id: Uuid::parse_str(&record.block_id).unwrap_or_default(),
            action_id: DRIFT_ACTION_ID,
            title: DRIFT_TITLE.to_owned(),
            body: drift_body(switch_count, &anchor),
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
        let result = aggregate_result(record, elapsed, &observations);
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
            active_intervention: self.active_intervention(&record.block_id)?,
        })
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
        InterventionResponse::Dismissed => WorkBlockInterventionOutcome::Dismissed,
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
            assert!(outcome.map_or(true, |o| o.intervention.is_none()));
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
