//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, NewUploadBatch,
    RawEventEntry, UploadBatch, UploadBatchStatus, UploadQueueDiagnostics,
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, HistoryCacheRepo, InsightCacheRepo, RawEventRepo, UploadBatchRepo,
};
