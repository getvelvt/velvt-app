//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{
    AbstractionMapping, BatchEvent, CompletedBlockDwellSpan, DemotionStateRecord, FocusTransition,
    HistoryCacheEntry, InitiationInvitationOutcome, InitiationInvitationRecord, InsightCacheEntry,
    InterventionDemotionState, LocalDisplayAggregate, LocalEventMetadata, NewUploadBatch,
    PersonalOverrideRecord, QuietHoursOfferResponse, QuietHoursOfferState, RawEventEntry,
    UploadBatch, UploadBatchStatus, UploadQueueDiagnostics, VelvtQuietHours, WeeklyDigestRecord,
    WorkBlockCategoryCorrection, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockOrigin, WorkBlockRecord,
    WrongInterventionCounts,
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, FocusRepo, HistoryCacheRepo, InitiationRepo, InsightCacheRepo,
    RawEventRepo, ReceiptsRepo, UploadBatchRepo, WorkBlockRepo,
};
