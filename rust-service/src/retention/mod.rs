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

mod scheduler;
mod targets;

pub use scheduler::RetentionScheduler;
pub use targets::{
    CacheRetentionTarget, RawEventRetentionTarget, UploadBatchRetentionTarget,
    WorkBlockIntentionRetentionTarget,
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
