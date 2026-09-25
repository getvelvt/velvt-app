//! Periodic retention scheduler for expired data.
//!
//! The `RetentionScheduler` drives a list of `RetentionTarget` objects on a
//! configurable interval.  Each target owns its own DAL calls; the scheduler
//! has zero table-level knowledge.
//!
//! # Extending retention
//!
//! To add a new retention target:
//! 1. Add a DAL method on the relevant trait in `persistence::traits`.
//! 2. Implement it in `persistence::sqlite`.
//! 3. Create a struct implementing `RetentionTarget` in `retention::targets`.
//! 4. Register it in `main.rs` with `scheduler.add_target(...)`.
//!
//! The scheduler core (`RetentionScheduler`) is never modified.
//!
//! One registered target writes rather than deletes:
//! `InterventionDecisionOutcomeTarget` resolves outcomes whose horizon has
//! closed. It is here because it needs exactly what this scheduler already
//! provides — a bounded pass on a periodic tick — and it reports the rows it
//! touched in the same `CleanupReport` field the deleting targets use.

mod scheduler;
mod targets;

pub use scheduler::RetentionScheduler;
pub use targets::{
    AbstractionMapRetentionTarget, CacheRetentionTarget, InterventionDecisionOutcomeTarget,
    RawEventRetentionTarget, SemanticEmbeddingCacheRetentionTarget, UploadBatchRetentionTarget,
    WorkBlockIntentionRetentionTarget, ABSTRACTION_MAP_RETENTION_DAYS,
    DECISION_OUTCOME_HORIZON_SECONDS, SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS,
};

use crate::persistence::PersistenceError;

/// A single data source eligible for periodic retention cleanup.
pub trait RetentionTarget: Send + Sync {
    fn name(&self) -> &'static str;

    /// Deletes at most `batch_size` expired rows and returns how many were
    /// removed.  If the returned count equals `batch_size`, more rows may
    /// remain; the scheduler will call this again on the next cycle.
    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError>;
}

/// Summary produced by one target after a single cleanup pass.
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub deleted: u64,
}

/// Errors returned by a retention target.
#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}
