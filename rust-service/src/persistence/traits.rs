use super::{
    AbstractionMapping, BatchEvent, CompletedBlockDwellSpan, DemotionStateRecord, FocusTransition,
    HistoryCacheEntry, InitiationInvitationOutcome, InitiationInvitationRecord, InsightCacheEntry,
    LocalDisplayAggregate, LocalEventMetadata, NewUploadBatch, PersistenceError,
    PersonalOverrideRecord, QuietHoursOfferResponse, QuietHoursOfferState, RawEventEntry,
    UploadBatch, UploadQueueDiagnostics, VelvtQuietHours, WeeklyDigestRecord,
    WorkBlockCategoryCorrection, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord, WrongInterventionCounts,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use velvt_shared_types::WorkBlockResult;

pub trait AbstractionMapRepo: Send + Sync {
    fn upsert(&self, mapping: &AbstractionMapping) -> Result<(), PersistenceError>;
    fn get(&self, stable_id: &str) -> Result<AbstractionMapping, PersistenceError>;
    fn exists(&self, key_hash: &str) -> Result<bool, PersistenceError>;
    fn save_personal_override(
        &self,
        stable_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError>;
    fn personal_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<PersonalOverrideRecord>, PersistenceError>;
    fn search_personal_overrides(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<PersonalOverrideRecord>, u64), PersistenceError>;
    /// Generalizes a correction to every window of the application the event
    /// was classified under.
    ///
    /// A no-op returning `Ok(false)` when the event predates app-scoped
    /// corrections (null `app_stable_id`) or is not eligible for them — a
    /// browser window whose identity came from the site, where one tab says
    /// nothing about the next.
    fn save_personal_app_override(
        &self,
        event_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<bool, PersistenceError>;

    fn remove_personal_override(&self, stable_id: &str) -> Result<bool, PersistenceError>;
    fn reset_personal_overrides(&self) -> Result<u64, PersistenceError>;
    fn personal_override_count(&self) -> Result<u64, PersistenceError>;
    fn personal_semantic_prototype_count(&self) -> Result<u64, PersistenceError>;
    fn classifier_artifact_count(&self, artifact_version: &str) -> Result<u64, PersistenceError>;
    fn display_name_for_label(&self, label: &str) -> Result<Option<String>, PersistenceError>;
}

pub trait UploadBatchRepo: Send + Sync {
    fn insert_batch(&self, batch: &NewUploadBatch) -> Result<(), PersistenceError>;
    fn insert_batch_with_events(
        &self,
        batch: &NewUploadBatch,
        events: &[BatchEvent],
    ) -> Result<(), PersistenceError>;
    fn mark_sent(&self, batch_id: &str) -> Result<(), PersistenceError>;
    fn pending_batches(&self) -> Result<Vec<UploadBatch>, PersistenceError>;
    fn resumable_batches(&self, now: DateTime<Utc>) -> Result<Vec<UploadBatch>, PersistenceError>;
    fn queue_diagnostics(&self) -> Result<UploadQueueDiagnostics, PersistenceError>;
    fn mark_failed(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError>;
    fn mark_pending_retry(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError>;
    fn mark_rejected(&self, batch_id: &str, error_code: &str) -> Result<(), PersistenceError>;
    fn discard_batch(&self, batch_id: &str) -> Result<(), PersistenceError>;
    fn batch_status(&self, batch_id: &str) -> Result<super::UploadBatchStatus, PersistenceError>;
    fn host_backoff_attempt(&self, host: &str) -> Result<u32, PersistenceError>;
    fn host_backoff_until(&self, host: &str) -> Result<Option<DateTime<Utc>>, PersistenceError>;
    fn set_host_backoff(
        &self,
        host: &str,
        attempt_count: u32,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    fn clear_host_backoff(&self, host: &str) -> Result<(), PersistenceError>;
    fn add_event_to_batch(
        &self,
        batch_id: &str,
        event: &BatchEvent,
    ) -> Result<(), PersistenceError>;
    fn update_event_classification(
        &self,
        event_id: &str,
        label: &str,
        category: &str,
    ) -> Result<(), PersistenceError>;
    /// Deletes at most `limit` sent batches whose `sent_at` is before `cutoff`.
    /// Cascade deletes the associated `batch_event` rows.
    fn delete_sent_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError>;
    /// Deletes at most `limit` rejected batches whose `created_at` is before `cutoff`.
    /// Cascade deletes the associated `batch_event` rows.
    fn delete_rejected_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError>;
}

pub trait HistoryCacheRepo: Send + Sync {
    fn upsert(&self, entry: &HistoryCacheEntry) -> Result<(), PersistenceError>;
    fn get(&self, date: &str) -> Result<Option<HistoryCacheEntry>, PersistenceError>;
    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError>;
    fn invalidate_all(&self) -> Result<u64, PersistenceError>;
    /// Deletes at most `limit` entries whose TTL (`expires_at`) is before
    /// `grace_cutoff`, meaning they have been expired for at least the
    /// configured grace period.
    fn delete_expired_batch(
        &self,
        grace_cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError>;
}

pub trait InsightCacheRepo: Send + Sync {
    fn upsert(&self, entry: &InsightCacheEntry) -> Result<(), PersistenceError>;
    /// Stores a negative cache entry (404) so the API is not re-queried until
    /// the short TTL expires.
    fn upsert_negative(
        &self,
        date: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    fn get(&self, date: &str) -> Result<Option<InsightCacheEntry>, PersistenceError>;
    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError>;
    fn invalidate_all(&self) -> Result<u64, PersistenceError>;
    /// Deletes at most `limit` entries whose TTL (`expires_at`) is before
    /// `grace_cutoff`, meaning they have been expired for at least the
    /// configured grace period.
    fn delete_expired_batch(
        &self,
        grace_cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError>;
}

pub trait RawEventRepo: Send + Sync {
    fn insert(&self, event: &RawEventEntry) -> Result<(), PersistenceError>;
    fn unbatched_events(&self, limit: usize) -> Result<Vec<RawEventEntry>, PersistenceError>;
    fn events_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<RawEventEntry>, PersistenceError>;
    /// Returns at most `limit` abstracted events in a bounded time window.
    fn events_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RawEventEntry>, PersistenceError>;
    fn local_event_metadata(
        &self,
        event_ids: &[String],
    ) -> Result<HashMap<String, LocalEventMetadata>, PersistenceError>;
    /// Returns at most `limit` curated labels plus an optional `Other` bucket.
    fn local_display_aggregates(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<LocalDisplayAggregate>, PersistenceError>;
    fn update_classification(
        &self,
        event_id: &str,
        label: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError>;
    fn delete_before(&self, cutoff: DateTime<Utc>) -> Result<u64, PersistenceError>;
    /// Deletes at most `limit` rows whose `created_at` is before `cutoff`.
    /// Returns the number of rows actually deleted.
    fn delete_expired_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError>;
}

/// Coarse Focus/DND evidence, quiet-hours offer memory, and Velvt's own
/// quiet-hours setting. Everything behind this trait is device-local and
/// structurally unable to hold a Focus mode's name, configuration, or
/// schedule.
pub trait FocusRepo: Send + Sync {
    /// Appends an edge transition. Returns `false` (and stores nothing) when
    /// the stored state already matches `transition.active`, so repeated
    /// samples of an unchanged state never accumulate rows.
    fn record_focus_transition(
        &self,
        transition: &FocusTransition,
    ) -> Result<bool, PersistenceError>;
    /// The most recently stored transition, if any.
    fn latest_focus_transition(&self) -> Result<Option<FocusTransition>, PersistenceError>;
    /// The coarse Focus state at the given bucket: the `active` value of the
    /// latest transition at or before it.
    fn focus_state_at_bucket(
        &self,
        bucket: DateTime<Utc>,
    ) -> Result<Option<bool>, PersistenceError>;
    /// Distinct local dates carrying an active transition in any of the given
    /// local hours, newest first. Bounded by evidence retention.
    fn focus_active_dates_in_hours(&self, hours: &[u32]) -> Result<Vec<String>, PersistenceError>;
    /// Deletes evidence rows whose bucket is before `cutoff`.
    fn prune_focus_evidence(&self, cutoff: DateTime<Utc>) -> Result<u64, PersistenceError>;
    /// Stores the client's latest UTC offset for local-hour decisions.
    fn set_utc_offset(&self, seconds: i32, at: DateTime<Utc>) -> Result<(), PersistenceError>;
    fn utc_offset_seconds(&self) -> Result<Option<i32>, PersistenceError>;
    fn quiet_hours_offer_state(&self) -> Result<Option<QuietHoursOfferState>, PersistenceError>;
    /// Records a fresh pattern-rule trigger, replacing any previous offer
    /// lifecycle. The caller owns the gate on when replacing is allowed.
    fn record_quiet_hours_trigger(
        &self,
        rule_version: u32,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    /// Marks the current offer as surfaced.
    fn record_quiet_hours_offered(&self, at: DateTime<Utc>) -> Result<(), PersistenceError>;
    /// Records the user's reply to the current offer.
    fn record_quiet_hours_response(
        &self,
        response: QuietHoursOfferResponse,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    fn quiet_hours(&self) -> Result<Option<VelvtQuietHours>, PersistenceError>;
    fn set_quiet_hours(&self, quiet_hours: &VelvtQuietHours) -> Result<(), PersistenceError>;
    /// Clears Focus evidence, the stored offset, and offer memory. Velvt's
    /// own quiet-hours setting is a user choice and is cleared separately.
    fn clear_focus_evidence(&self) -> Result<u64, PersistenceError>;
}

pub trait WorkBlockRepo: Send + Sync {
    fn create(&self, block: &WorkBlockRecord) -> Result<(), PersistenceError>;
    fn latest(&self) -> Result<Option<WorkBlockRecord>, PersistenceError>;
    fn get(&self, block_id: &str) -> Result<WorkBlockRecord, PersistenceError>;
    fn set_paused(&self, block_id: &str, at: DateTime<Utc>) -> Result<(), PersistenceError>;
    fn set_active(
        &self,
        block_id: &str,
        at: DateTime<Utc>,
        total_paused_seconds: u32,
    ) -> Result<(), PersistenceError>;
    fn mark_recovered(&self, block_id: &str, at: DateTime<Utc>) -> Result<(), PersistenceError>;
    fn close_open_observation(
        &self,
        block_id: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    fn append_observation(
        &self,
        block_id: &str,
        observation: &WorkBlockObservation,
    ) -> Result<(), PersistenceError>;
    fn observations(&self, block_id: &str) -> Result<Vec<WorkBlockObservation>, PersistenceError>;
    fn latest_observation(
        &self,
        block_id: &str,
    ) -> Result<Option<WorkBlockObservation>, PersistenceError>;
    fn finalize(
        &self,
        block_id: &str,
        completion: &WorkBlockCompletion,
    ) -> Result<WorkBlockResult, PersistenceError>;
    fn result(&self, block_id: &str) -> Result<Option<WorkBlockResult>, PersistenceError>;
    /// Records an intervention offer. The caller must treat a duplicate as a
    /// no-op: the table's primary key enforces one offer per block.
    fn record_intervention(
        &self,
        block_id: &str,
        intervention: &WorkBlockIntervention,
    ) -> Result<(), PersistenceError>;
    fn intervention(
        &self,
        block_id: &str,
    ) -> Result<Option<WorkBlockIntervention>, PersistenceError>;
    /// Returns the most recent offers across every block, newest first.
    ///
    /// Backoff spans blocks: the one-offer-per-block cap means a dismissal can
    /// only ever affect the *next* block, so the decision needs the offers that
    /// came before this one.
    fn recent_interventions(
        &self,
        limit: usize,
    ) -> Result<Vec<WorkBlockIntervention>, PersistenceError>;
    /// Transitions an offer to a terminal outcome. Only an `offered` row is
    /// updated, so a recorded return is never overwritten by block expiry.
    /// Records a block-scoped classification correction. The first correction
    /// for a category wins; recording it again is a no-op.
    fn record_category_correction(
        &self,
        block_id: &str,
        correction: &WorkBlockCategoryCorrection,
    ) -> Result<(), PersistenceError>;

    /// Every block-scoped correction, oldest first.
    fn category_corrections(
        &self,
        block_id: &str,
    ) -> Result<Vec<WorkBlockCategoryCorrection>, PersistenceError>;

    /// Rolling wrong-intervention counts across blocks: offers delivered since
    /// `since`, and how many were answered `was_focused` — the reply that says
    /// the offer should never have fired. Two integers, content-free.
    ///
    /// A decision held because Focus/DND was active, or withheld because the
    /// user is auto-demoted, was never delivered and is excluded from the
    /// delivered count. Counting a nudge nobody could see as an interruption
    /// they tolerated would flatter the precision metric.
    fn wrong_intervention_counts(
        &self,
        since: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, PersistenceError>;

    /// The persisted demotion state singleton, if one has been written.
    fn demotion_state(&self) -> Result<Option<DemotionStateRecord>, PersistenceError>;
    /// Writes the demotion state singleton (insert-or-update on the one
    /// row). Current state only; no history accumulates.
    fn set_demotion_state(&self, record: &DemotionStateRecord) -> Result<(), PersistenceError>;
    /// Transitions an offer to a terminal outcome. Only an `offered` row is
    /// updated, so a recorded return is never overwritten by block expiry.
    fn resolve_intervention(
        &self,
        block_id: &str,
        outcome: WorkBlockInterventionOutcome,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError>;
    fn expire_intentions(&self, now: DateTime<Utc>) -> Result<u64, PersistenceError>;
    fn clear_all(&self) -> Result<u64, PersistenceError>;
}

/// Storage seam for the deterministic initiation-invitation policy: the
/// bounded invitation store, the single opt-out, and the safe local
/// evidence the good-hours policy aggregates. Everything device-local.
pub trait InitiationRepo: Send + Sync {
    fn record_invitation(
        &self,
        invitation: &InitiationInvitationRecord,
    ) -> Result<(), PersistenceError>;
    fn invitation(
        &self,
        invitation_id: &str,
    ) -> Result<Option<InitiationInvitationRecord>, PersistenceError>;
    /// The invitation still awaiting a response, if any. At most one exists.
    fn open_invitation(&self) -> Result<Option<InitiationInvitationRecord>, PersistenceError>;
    /// Most recent invitations first, bounded by `limit`.
    fn recent_invitations(
        &self,
        limit: usize,
    ) -> Result<Vec<InitiationInvitationRecord>, PersistenceError>;
    /// Transitions an invitation to a terminal outcome. Only an `offered`
    /// row is updated, so a recorded answer is never overwritten.
    fn resolve_invitation(
        &self,
        invitation_id: &str,
        outcome: InitiationInvitationOutcome,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError>;
    /// Invitations extended on the given local calendar date (daily cap).
    fn invitations_on_local_date(&self, local_date: &str) -> Result<u64, PersistenceError>;
    fn invitations_enabled(&self) -> Result<bool, PersistenceError>;
    fn set_invitations_enabled(
        &self,
        enabled: bool,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    /// Completed blocks started at or after `since` (cold-start gate input).
    fn completed_block_count(&self, since: DateTime<Utc>) -> Result<u64, PersistenceError>;
    /// Confident, closed observation spans inside completed blocks started
    /// at or after `since`, ordered by span start (good-hours dwell input).
    fn completed_block_dwell_spans(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<CompletedBlockDwellSpan>, PersistenceError>;
    /// Deletes every invitation row. The opt-out setting is an explicit user
    /// choice and survives; the behavioral record does not.
    fn clear_invitations(&self) -> Result<u64, PersistenceError>;
}

/// Storage seam for the weekly receipts digest and the explain-tap probe
/// (0.1.6 Scope 4; D6, D7). Everything device-local.
///
/// Every count aggregate here reads the same stored rows the local metrics
/// read — `work_block`, `work_block_result`, `work_block_intervention`, and
/// `initiation_invitation` — through the same predicates. No parallel
/// counters exist.
pub trait ReceiptsRepo: Send + Sync {
    /// Blocks declared with `since <= started_at < until`.
    fn declared_block_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
    /// Completed blocks with `since <= started_at < until`. Shares the exact
    /// SQL predicate with `InitiationRepo::completed_block_count`.
    fn completed_block_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
    /// Stored session-result payloads for blocks with
    /// `since <= started_at < until`. The digest sums `recovery_count` from
    /// these — the same stored numbers each session result displayed.
    fn result_payloads_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<String>, PersistenceError>;
    /// Invitations accepted with `since <= outcome_at < until`, from the
    /// same stored rows the invitation-acceptance metric reads.
    fn accepted_invitation_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
    /// The wrong-intervention counts bounded to `since <= offered_at <
    /// until`. Shares the exact SQL body with
    /// `WorkBlockRepo::wrong_intervention_counts`, so the digest's weekly
    /// wrong-intervention count cannot drift from the rolling metric.
    fn wrong_intervention_counts_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, PersistenceError>;
    /// Decisions Velvt chose not to send with `since <= offered_at < until`:
    /// held under DND plus withheld while demoted, from the same stored
    /// intervention rows the reconciliation counts read.
    fn withheld_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
    /// Interventions delivered with `since <= offered_at < until`, through
    /// the same delivered predicate the wrong-intervention counter uses.
    /// This is the probe denominator; it is derived, never stored.
    fn delivered_intervention_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
    /// The stored digest for a local week, if generated.
    fn weekly_digest(
        &self,
        week_start_local_date: &str,
    ) -> Result<Option<WeeklyDigestRecord>, PersistenceError>;
    /// Stores a freshly generated digest row. Insert-only; a stored digest
    /// is frozen and never regenerated.
    fn store_weekly_digest(&self, record: &WeeklyDigestRecord) -> Result<(), PersistenceError>;
    /// Marks the first showing. Only a null `delivered_at` is written, so
    /// the first-delivery instant is never overwritten.
    fn mark_digest_delivered(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    /// Records the user's one-tap close. Idempotent.
    fn acknowledge_digest(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    /// Increments the coarse, content-free explain-tap count for a local
    /// week. One bounded integer per week; nothing else is representable.
    fn record_explain_tap(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError>;
    /// The stored tap count for a local week (0 when absent).
    fn explain_taps_for_week(&self, week_start_local_date: &str) -> Result<u64, PersistenceError>;
    /// Deletes digests and probe buckets keyed strictly before the given
    /// local Monday. Deterministic retention, called at generation time.
    fn prune_receipts_before(&self, week_start_local_date: &str) -> Result<u64, PersistenceError>;
    /// Deletes every digest row and probe bucket (clear-all-data).
    fn clear_receipts(&self) -> Result<u64, PersistenceError>;
}
