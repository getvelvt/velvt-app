use super::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, LocalDisplayAggregate,
    LocalEventMetadata, NewUploadBatch, PersistenceError, PersonalOverrideRecord, RawEventEntry,
    UploadBatch, UploadQueueDiagnostics, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord,
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
    fn resolve_intervention(
        &self,
        block_id: &str,
        outcome: WorkBlockInterventionOutcome,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError>;
    fn expire_intentions(&self, now: DateTime<Utc>) -> Result<u64, PersistenceError>;
    fn clear_all(&self) -> Result<u64, PersistenceError>;
}
