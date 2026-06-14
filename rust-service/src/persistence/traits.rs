use super::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, NewUploadBatch,
    PersistenceError, RawEventEntry, UploadBatch,
};
use chrono::{DateTime, Utc};

pub trait AbstractionMapRepo: Send + Sync {
    fn upsert(&self, mapping: &AbstractionMapping) -> Result<(), PersistenceError>;
    fn get(&self, stable_id: &str) -> Result<AbstractionMapping, PersistenceError>;
    fn exists(&self, key_hash: &str) -> Result<bool, PersistenceError>;
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
    fn add_event_to_batch(
        &self,
        batch_id: &str,
        event: &BatchEvent,
    ) -> Result<(), PersistenceError>;
}

pub trait HistoryCacheRepo: Send + Sync {
    fn upsert(&self, entry: &HistoryCacheEntry) -> Result<(), PersistenceError>;
    fn get(&self, date: &str) -> Result<Option<HistoryCacheEntry>, PersistenceError>;
    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError>;
}

pub trait InsightCacheRepo: Send + Sync {
    fn upsert(&self, entry: &InsightCacheEntry) -> Result<(), PersistenceError>;
    fn get(&self, date: &str) -> Result<Option<InsightCacheEntry>, PersistenceError>;
    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError>;
}

pub trait RawEventRepo: Send + Sync {
    fn insert(&self, event: &RawEventEntry) -> Result<(), PersistenceError>;
    fn events_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<RawEventEntry>, PersistenceError>;
    fn delete_before(&self, cutoff: DateTime<Utc>) -> Result<u64, PersistenceError>;
}
