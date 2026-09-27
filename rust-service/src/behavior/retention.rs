//! Retention for the durable behavioural substrate.
//!
//! `out_of_block_run` is durable, not infinite. Ninety days is long enough for
//! the antecedent miner's discovery and held-out confirmation windows to be
//! genuinely disjoint, and short enough that the store never becomes a
//! multi-year behavioural record nobody asked for.

use std::{sync::Arc, time::Duration};

use chrono::Utc;

use velvt_service::persistence::BehaviorRepo;
use velvt_service::retention::{CleanupReport, RetentionError, RetentionTarget};

/// Default retention for `out_of_block_run`.
///
/// A constant, not a configurable: widening it silently is exactly the change
/// that should require a code review and a `PRIVACY.md` edit. It may be narrowed
/// by a future constant; it may not be widened without both.
pub const OUT_OF_BLOCK_RUN_RETENTION_DAYS: u64 = 90;

/// Expires rows from `out_of_block_run` whose start bucket is older than the
/// retention window.
///
/// Unlike the raw-event target this has no fold watermark to respect: nothing
/// downstream folds `out_of_block_run` into a second tier yet. When the
/// longitudinal rollup lands, it gains the same floor `RawEventRetentionTarget`
/// will — delete only up to `min(cutoff, folded_through)` — so a broken fold job
/// costs disk rather than history.
pub struct OutOfBlockRunRetentionTarget {
    repo: Arc<dyn BehaviorRepo>,
    retention: Duration,
    batch_size: usize,
}

impl OutOfBlockRunRetentionTarget {
    pub fn new(repo: Arc<dyn BehaviorRepo>, retention: Duration, batch_size: usize) -> Self {
        Self {
            repo,
            retention,
            batch_size,
        }
    }

    /// The registered default: 90 days.
    pub fn with_default_retention(repo: Arc<dyn BehaviorRepo>, batch_size: usize) -> Self {
        Self::new(
            repo,
            Duration::from_secs(OUT_OF_BLOCK_RUN_RETENTION_DAYS * 24 * 60 * 60),
            batch_size,
        )
    }
}

impl RetentionTarget for OutOfBlockRunRetentionTarget {
    fn name(&self) -> &'static str {
        "out_of_block_run"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let cutoff = Utc::now().timestamp() - self.retention.as_secs() as i64;
        let deleted = self
            .repo
            .delete_out_of_block_runs_before(cutoff, self.batch_size)?;
        Ok(CleanupReport { deleted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velvt_service::persistence::{OutOfBlockRun, SqlitePersistence};
    use velvt_shared_types::{ClassificationConfidence, ClassificationStatus};

    fn run(bucket: i64) -> OutOfBlockRun {
        OutOfBlockRun {
            started_at_bucket: bucket,
            duration_seconds: 300,
            category: "COMMUNICATION".into(),
            classification_status: ClassificationStatus::Classified,
            classification_confidence: ClassificationConfidence::High,
            local_hour: 8,
            local_date: "2026-08-21".into(),
        }
    }

    /// The window is the whole guarantee: a run inside it survives, a run
    /// outside it does not, and the boundary is not off by a day.
    #[test]
    fn ninety_day_window_expires_only_what_is_older_than_it() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.behavior_repo();
        let day = 24 * 60 * 60;
        let now = Utc::now().timestamp();
        // Comfortably inside, just inside, and clearly outside the window.
        repo.record_out_of_block_run(&run(now - day)).unwrap();
        repo.record_out_of_block_run(&run(now - 89 * day)).unwrap();
        repo.record_out_of_block_run(&run(now - 91 * day)).unwrap();

        let report = OutOfBlockRunRetentionTarget::with_default_retention(repo.clone(), 500)
            .run_cleanup()
            .unwrap();

        assert_eq!(report.deleted, 1, "only the 91-day-old run is expired");
        let survivors = repo.out_of_block_runs(0).unwrap();
        assert_eq!(survivors.len(), 2);
        assert!(survivors
            .iter()
            .all(|run| run.started_at_bucket >= now - 90 * day));
    }

    /// Retention deletes in bounded batches, so a large backlog can never hold
    /// the single write lock for an unbounded time. The scheduler calls again
    /// on the next cycle.
    #[test]
    fn deletion_is_batched_and_resumes_on_the_next_pass() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.behavior_repo();
        let old = Utc::now().timestamp() - 120 * 24 * 60 * 60;
        for index in 0..5 {
            repo.record_out_of_block_run(&run(old + index * 300))
                .unwrap();
        }

        let target = OutOfBlockRunRetentionTarget::with_default_retention(repo.clone(), 2);
        assert_eq!(target.run_cleanup().unwrap().deleted, 2);
        assert_eq!(target.run_cleanup().unwrap().deleted, 2);
        assert_eq!(target.run_cleanup().unwrap().deleted, 1);
        assert_eq!(target.run_cleanup().unwrap().deleted, 0);
        assert!(repo.out_of_block_runs(0).unwrap().is_empty());
    }
}
