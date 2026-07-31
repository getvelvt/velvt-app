//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, LocalDisplayAggregate,
    LocalEventMetadata, NewUploadBatch, PersonalOverrideRecord, RawEventEntry, UploadBatch,
    UploadBatchStatus, UploadQueueDiagnostics, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord,
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, HistoryCacheRepo, InsightCacheRepo, RawEventRepo, UploadBatchRepo,
    WorkBlockRepo,
};
