//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod models;
mod sqlite;
mod traits;

pub use models::{out_of_block_run_bucket, OUT_OF_BLOCK_RUN_BUCKET_SECONDS};
pub use models::{
    AbstractionMapping, BatchEvent, BlockAntecedent, CompletedBlockDwellSpan, DayType,
    DemotionStateRecord, FocusTransition, GateVerdict, HistoryCacheEntry,
    InitiationInvitationOutcome, InitiationInvitationRecord, InsightCacheEntry,
    InterventionDecision, InterventionDemotionState, LocalDisplayAggregate, LocalEventMetadata,
    NewUploadBatch, OutOfBlockRun, PersonalOverrideRecord, QuietHoursOfferResponse,
    QuietHoursOfferState, RawEventEntry, UploadBatch, UploadBatchStatus, UploadQueueDiagnostics,
    VelvtQuietHours, WeeklyDigestRecord, WorkBlockCategoryCorrection, WorkBlockCompletion,
    WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockOrigin,
    WorkBlockRecord, WrongInterventionCounts,
};
pub use sqlite::{PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, BehaviorRepo, FocusRepo, HistoryCacheRepo, InitiationRepo,
    InsightCacheRepo, RawEventRepo, ReceiptsRepo, UploadBatchRepo, WorkBlockRepo,
};
