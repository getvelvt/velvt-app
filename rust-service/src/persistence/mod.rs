//! SQLite repository interfaces.
//!
//! This module owns durable local records, retention queries, and cached
//! delivery payloads. It does not own abstraction decisions, cloud HTTP,
//! credential storage, or UI behavior.

#![allow(async_fn_in_trait)]

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::abstraction::AbstractedEvent;
use crate::ipc::{HistoryPayload, InsightPayload, RawEvent};
use crate::upload::UploadBatch;

/// Persists and expires local-only raw events.
pub trait EventRepository {
    /// Stores one local-only raw event.
    async fn insert_raw_event(&self, event: &RawEvent) -> Result<(), PersistenceError>;

    /// Removes raw events whose indexed retention expiry has passed.
    async fn delete_expired_raw_events(&self, now: DateTime<Utc>) -> Result<u64, PersistenceError>;

    /// Stores one privacy-safe abstracted event.
    async fn insert_abstracted_event(
        &self,
        event: &AbstractedEvent,
    ) -> Result<(), PersistenceError>;
}

/// Persists local-only raw-to-abstract mappings.
pub trait AbstractionMappingRepository {
    /// Looks up an abstract label by stable local identifier.
    async fn find_label(&self, stable_local_id: &str) -> Result<Option<String>, PersistenceError>;

    /// Stores a local-only mapping.
    async fn save_label(
        &self,
        stable_local_id: &str,
        abstract_label: &str,
    ) -> Result<(), PersistenceError>;
}

/// Persists privacy-safe upload batches and retry state.
pub trait UploadBatchRepository {
    /// Stores an assembled upload batch.
    async fn insert_batch(&self, batch: &UploadBatch) -> Result<(), PersistenceError>;

    /// Marks a batch as successfully uploaded.
    async fn mark_uploaded(&self, batch_id: Uuid) -> Result<(), PersistenceError>;

    /// Marks a batch as permanently failed.
    async fn mark_permanently_failed(&self, batch_id: Uuid) -> Result<(), PersistenceError>;
}

/// Persists non-secret device state.
pub trait DeviceStateRepository {
    /// Reads a non-secret device-state value.
    async fn get_value(&self, key: &str) -> Result<Option<String>, PersistenceError>;

    /// Writes a non-secret device-state value.
    async fn set_value(&self, key: &str, value: &str) -> Result<(), PersistenceError>;
}

/// Persists ready-to-display summaries and insights.
pub trait InsightCacheRepository {
    /// Stores a ready-to-display daily insight.
    async fn cache_insight(&self, insight: &InsightPayload) -> Result<(), PersistenceError>;

    /// Stores a ready-to-display history payload.
    async fn cache_history(&self, history: &HistoryPayload) -> Result<(), PersistenceError>;

    /// Reads a cached insight by date.
    async fn insight_for_date(
        &self,
        date: NaiveDate,
    ) -> Result<Option<InsightPayload>, PersistenceError>;
}

/// Errors produced by local persistence.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// SQLite operation failed.
    #[error("database operation failed")]
    Database,
}
