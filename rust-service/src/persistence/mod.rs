//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{
<<<<<<< ours
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, LocalDisplayAggregate,
    LocalEventMetadata, NewUploadBatch, PersonalOverrideRecord, RawEventEntry, UploadBatch,
    UploadBatchStatus, UploadQueueDiagnostics, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord,
=======
    AbstractionMapping, BatchEvent, FocusTransition, HistoryCacheEntry, InsightCacheEntry,
    LocalDisplayAggregate, LocalEventMetadata, NewUploadBatch, PersonalOverrideRecord,
    QuietHoursOfferResponse, QuietHoursOfferState, RawEventEntry, UploadBatch, UploadBatchStatus,
    UploadQueueDiagnostics, VelvtQuietHours, WorkBlockCategoryCorrection, WorkBlockCompletion,
    WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord,
    WrongInterventionCounts,
>>>>>>> theirs
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, FocusRepo, HistoryCacheRepo, InsightCacheRepo, RawEventRepo,
    UploadBatchRepo, WorkBlockRepo,
};
