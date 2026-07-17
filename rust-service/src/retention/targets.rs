use std::{sync::Arc, time::Duration};

use chrono::Utc;

use crate::persistence::{
    HistoryCacheRepo, InsightCacheRepo, RawEventRepo, UploadBatchRepo, WorkBlockRepo,
};

use super::{CleanupReport, RetentionError, RetentionTarget};

// ---------------------------------------------------------------------------
// RawEventRetentionTarget
// ---------------------------------------------------------------------------

/// Expires rows from `raw_event_buffer` whose `created_at` is older than
/// the configured TTL.
pub struct RawEventRetentionTarget {
    repo: Arc<dyn RawEventRepo>,
    ttl: Duration,
    batch_size: usize,
}

impl RawEventRetentionTarget {
    pub fn new(repo: Arc<dyn RawEventRepo>, ttl: Duration, batch_size: usize) -> Self {
        Self {
            repo,
            ttl,
            batch_size,
        }
    }
}

impl RetentionTarget for RawEventRetentionTarget {
    fn name(&self) -> &'static str {
        "raw_event_buffer"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.ttl.as_secs() as i64);
        let deleted = self.repo.delete_expired_batch(cutoff, self.batch_size)?;
        Ok(CleanupReport { deleted })
    }
}

// ---------------------------------------------------------------------------
// UploadBatchRetentionTarget
// ---------------------------------------------------------------------------

/// Expires sent and rejected rows from `upload_batch` (and their associated
/// `batch_event` rows via cascade delete).
///
/// Pending and in-flight batches (`status = 'pending'` or `'failed'`) are
/// never touched.
pub struct UploadBatchRetentionTarget {
    repo: Arc<dyn UploadBatchRepo>,
    sent_retention: Duration,
    rejected_audit_period: Duration,
    batch_size: usize,
}

impl UploadBatchRetentionTarget {
    pub fn new(
        repo: Arc<dyn UploadBatchRepo>,
        sent_retention: Duration,
        rejected_audit_period: Duration,
        batch_size: usize,
    ) -> Self {
        Self {
            repo,
            sent_retention,
            rejected_audit_period,
            batch_size,
        }
    }
}

impl RetentionTarget for UploadBatchRetentionTarget {
    fn name(&self) -> &'static str {
        "upload_batch"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let sent_cutoff =
            Utc::now() - chrono::Duration::seconds(self.sent_retention.as_secs() as i64);
        let rejected_cutoff =
            Utc::now() - chrono::Duration::seconds(self.rejected_audit_period.as_secs() as i64);

        let sent_deleted = self.repo.delete_sent_batch(sent_cutoff, self.batch_size)?;
        let rejected_deleted = self
            .repo
            .delete_rejected_batch(rejected_cutoff, self.batch_size)?;

        Ok(CleanupReport {
            deleted: sent_deleted + rejected_deleted,
        })
    }
}

// ---------------------------------------------------------------------------
// CacheRetentionTarget
// ---------------------------------------------------------------------------

/// Expires entries from `history_cache` and `insight_cache` that have been
/// expired (past their TTL) for at least the configured grace period.
pub struct CacheRetentionTarget {
    history_repo: Arc<dyn HistoryCacheRepo>,
    insight_repo: Arc<dyn InsightCacheRepo>,
    grace: Duration,
    batch_size: usize,
}

impl CacheRetentionTarget {
    pub fn new(
        history_repo: Arc<dyn HistoryCacheRepo>,
        insight_repo: Arc<dyn InsightCacheRepo>,
        grace: Duration,
        batch_size: usize,
    ) -> Self {
        Self {
            history_repo,
            insight_repo,
            grace,
            batch_size,
        }
    }
}

impl RetentionTarget for CacheRetentionTarget {
    fn name(&self) -> &'static str {
        "cache"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        // grace_cutoff: entries whose ttl (expires_at) is before this have
        // been expired for at least `grace` seconds.
        let grace_cutoff = Utc::now() - chrono::Duration::seconds(self.grace.as_secs() as i64);

        let history_deleted = self
            .history_repo
            .delete_expired_batch(grace_cutoff, self.batch_size)?;
        let insight_deleted = self
            .insight_repo
            .delete_expired_batch(grace_cutoff, self.batch_size)?;

        Ok(CleanupReport {
            deleted: history_deleted + insight_deleted,
        })
    }
}

/// Clears expired free-form intention text while preserving safe block/result
/// evidence. The intention deadline is stored per block and never extended by
/// ordinary reads.
pub struct WorkBlockIntentionRetentionTarget {
    repo: Arc<dyn WorkBlockRepo>,
}

impl WorkBlockIntentionRetentionTarget {
    pub fn new(repo: Arc<dyn WorkBlockRepo>) -> Self {
        Self { repo }
    }
}

impl RetentionTarget for WorkBlockIntentionRetentionTarget {
    fn name(&self) -> &'static str {
        "work_block_intention"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        Ok(CleanupReport {
            deleted: self.repo.expire_intentions(Utc::now())?,
        })
    }
}
