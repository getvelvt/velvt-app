//! SQLite-backed persistence hidden behind consumer-specific DAL traits.

mod egress_ledger;
mod models;
mod sqlite;
mod traits;

pub use egress_ledger::EgressLedgerRepo;
pub use models::{out_of_block_run_bucket, OUT_OF_BLOCK_RUN_BUCKET_SECONDS};
pub use models::{
    AbstractionMapping, AntecedentFinding, AntecedentFindingState, AntecedentRetractionReason,
    AppScopeOverride, BatchEvent, BlockAntecedent, CompletedBlockDwellSpan, DayType,
    DeclaredAppMetadata, DemotionStateRecord, FocusTransition, GateVerdict, HistoryCacheEntry,
    InitiationInvitationOutcome, InitiationInvitationRecord, InsightCacheEntry,
    InterventionDecision, InterventionDemotionState, LocalDisplayAggregate, LocalEventMetadata,
    NewUploadBatch, OutOfBlockRun, PersonalOverrideRecord, QuietHoursOfferResponse,
    QuietHoursOfferState, RawEventEntry, UnclassifiedAppEntry, UploadBatch, UploadBatchStatus,
    UploadQueueDiagnostics, VelvtQuietHours, WeeklyDigestRecord, WorkBlockCategoryCorrection,
    WorkBlockCompletion, WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockObservation,
    WorkBlockOrigin, WorkBlockRecord, WrongInterventionCounts,
};
pub use models::{ReportedDwell, MAX_REPORTED_DWELL_SECONDS};
pub use sqlite::{MigrationChecksumMismatch, MigrationReport, PersistenceError, SqlitePersistence};
pub use traits::{
    AbstractionMapRepo, AntecedentFindingRepo, BehaviorRepo, FocusRepo, HistoryCacheRepo,
    InitiationRepo, InsightCacheRepo, RawEventRepo, ReceiptsRepo, UploadBatchRepo, WorkBlockRepo,
    TRIAGE_MAX_ENTRIES, TRIAGE_MAX_LOOKBACK_DAYS, TRIAGE_MIN_SECONDS,
};
