//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, LocalDisplayAggregate,
    LocalEventMetadata, NewUploadBatch, RawEventEntry, UploadBatch, UploadBatchStatus,
    UploadQueueDiagnostics, WorkBlockCompletion, WorkBlockObservation, WorkBlockRecord,
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, HistoryCacheRepo, InsightCacheRepo, RawEventRepo, UploadBatchRepo,
    WorkBlockRepo,
};
