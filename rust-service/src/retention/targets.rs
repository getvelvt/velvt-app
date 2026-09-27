use std::{sync::Arc, time::Duration};

use chrono::Utc;

use crate::persistence::{
    AbstractionMapRepo, EgressLedgerRepo, HistoryCacheRepo, InsightCacheRepo, RawEventRepo,
    UploadBatchRepo, WorkBlockRepo,
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

/// Expires rows from `upload_batch` (and their associated `batch_event` rows
/// via cascade delete): sent batches on the sent horizon, rejected batches on
/// the audit horizon, and anything at all that has outlived the sent horizon
/// since it was created.
///
/// That last sweep is the one that bounds the queue. Sweeping only the two
/// terminal statuses left every batch that never reached one of them — the
/// whole queue, when the host is unreachable — on disk for as long as the
/// service ran. A batch still inside the horizon is still owed to the backend
/// and is never touched.
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
        // The queued backstop runs on the sent horizon: a batch that has been
        // waiting longer than a delivered one is kept has stopped being a
        // delivery and started being a record of activity nobody asked to keep.
        let stale_deleted = self
            .repo
            .delete_stale_queued_batch(sent_cutoff, self.batch_size)?;

        Ok(CleanupReport {
            deleted: sent_deleted + rejected_deleted + stale_deleted,
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

// ---------------------------------------------------------------------------
// SemanticEmbeddingCacheRetentionTarget
// ---------------------------------------------------------------------------

/// Default retention for `semantic_embedding_cache`, in days.
///
/// The cache holds a hashed sketch derived from the application name and the
/// window title, so it is a derivation of the same raw input `raw_event_buffer`
/// holds and it expires on the same horizon. A constant rather than a config
/// value, for the reason `OUT_OF_BLOCK_RUN_RETENTION_DAYS` is: widening a
/// privacy horizon should cost a code review and a `PRIVACY.md` edit, not an
/// environment variable.
pub const SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS: u64 = 14;

/// Expires rows from `semantic_embedding_cache` that have not been re-observed
/// within the retention window.
///
/// Until this existed the table's only bound was its 512-row cap, and
/// `record_embedding` refreshes `updated_at` on every re-observation — so a
/// window visited often enough to stay inside the cap never aged out at all.
/// `personal_semantic_prototype` is deliberately not swept here: it holds
/// corrections the user made on purpose, which is learned state rather than a
/// cache, and clearing it is what "Reset Corrections" is for.
pub struct SemanticEmbeddingCacheRetentionTarget {
    repo: Arc<dyn AbstractionMapRepo>,
    retention: Duration,
    batch_size: usize,
}

impl SemanticEmbeddingCacheRetentionTarget {
    pub fn new(repo: Arc<dyn AbstractionMapRepo>, retention: Duration, batch_size: usize) -> Self {
        Self {
            repo,
            retention,
            batch_size,
        }
    }

    /// The registered default: the raw-event horizon.
    pub fn with_default_retention(repo: Arc<dyn AbstractionMapRepo>, batch_size: usize) -> Self {
        Self::new(
            repo,
            Duration::from_secs(SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS * 24 * 60 * 60),
            batch_size,
        )
    }
}

impl RetentionTarget for SemanticEmbeddingCacheRetentionTarget {
    fn name(&self) -> &'static str {
        "semantic_embedding_cache"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.retention.as_secs() as i64);
        let deleted = self
            .repo
            .delete_expired_semantic_embeddings(cutoff, self.batch_size)?;
        Ok(CleanupReport { deleted })
    }
}

// ---------------------------------------------------------------------------
// AbstractionMapRetentionTarget
// ---------------------------------------------------------------------------

/// Default retention for `abstraction_map`, in days, counted from the last time
/// the window was observed.
///
/// A mapping holds a window's (application, title) key, so it is evidence with
/// the same shape as a raw event and expires on the same horizon as the buffer
/// and the embedding cache. A constant rather than a config value, for the reason
/// `SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS` is one: widening a privacy horizon
/// should cost a code review and a `PRIVACY.md` edit, not an environment
/// variable.
pub const ABSTRACTION_MAP_RETENTION_DAYS: u64 = 14;

/// Expires `abstraction_map` rows not observed within the retention window that
/// no correction and no buffered event still points at.
///
/// Until this existed the table kept one row per distinct window ever seen, with
/// no sweep at all, so the file answered "was this title ever open?" for as long
/// as Velvt had been installed. Keys are salted per install since migration 0037,
/// which stops that answer being computed offline; this is what stops it being
/// kept. A window the user corrected keeps its row for as long as the correction
/// exists, because the correction history lists and removes a window rule
/// through it (`AbstractionMapRepo::delete_expired_mappings`).
pub struct AbstractionMapRetentionTarget {
    repo: Arc<dyn AbstractionMapRepo>,
    retention: Duration,
    batch_size: usize,
}

impl AbstractionMapRetentionTarget {
    pub fn new(repo: Arc<dyn AbstractionMapRepo>, retention: Duration, batch_size: usize) -> Self {
        Self {
            repo,
            retention,
            batch_size,
        }
    }

    /// The registered default: the raw-event horizon.
    pub fn with_default_retention(repo: Arc<dyn AbstractionMapRepo>, batch_size: usize) -> Self {
        Self::new(
            repo,
            Duration::from_secs(ABSTRACTION_MAP_RETENTION_DAYS * 24 * 60 * 60),
            batch_size,
        )
    }
}

impl RetentionTarget for AbstractionMapRetentionTarget {
    fn name(&self) -> &'static str {
        "abstraction_map"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let cutoff = Utc::now() - chrono::Duration::seconds(self.retention.as_secs() as i64);
        let deleted = self.repo.delete_expired_mappings(cutoff, self.batch_size)?;
        Ok(CleanupReport { deleted })
    }
}

// ---------------------------------------------------------------------------
// EgressLedgerRetentionTarget
// ---------------------------------------------------------------------------

/// Prunes `egress_ledger` to `EGRESS_LEDGER_RETENTION_DAYS` and
/// `EGRESS_LEDGER_MAX_ENTRIES`, whichever is tighter, oldest entries only.
///
/// Every pass that removes anything first records a checkpoint naming the last
/// entry it removes (`EgressLedgerRepo::prune`), so the first surviving entry
/// still links to a hash the verifier can read.
pub struct EgressLedgerRetentionTarget {
    repo: Arc<dyn EgressLedgerRepo>,
    retention_days: i64,
    max_entries: u64,
    batch_size: usize,
}

impl EgressLedgerRetentionTarget {
    pub fn new(
        repo: Arc<dyn EgressLedgerRepo>,
        retention_days: i64,
        max_entries: u64,
        batch_size: usize,
    ) -> Self {
        Self {
            repo,
            retention_days,
            max_entries,
            batch_size,
        }
    }

    pub fn with_default_retention(repo: Arc<dyn EgressLedgerRepo>, batch_size: usize) -> Self {
        Self::new(
            repo,
            crate::egress::EGRESS_LEDGER_RETENTION_DAYS,
            crate::egress::EGRESS_LEDGER_MAX_ENTRIES,
            batch_size,
        )
    }
}

impl RetentionTarget for EgressLedgerRetentionTarget {
    fn name(&self) -> &'static str {
        "egress_ledger"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let now = Utc::now();
        let cutoff = now - chrono::Duration::days(self.retention_days);
        let deleted = self
            .repo
            .prune(cutoff, self.max_entries, self.batch_size, now)?;
        Ok(CleanupReport { deleted })
    }
}

// ---------------------------------------------------------------------------
// InterventionDecisionOutcomeTarget
// ---------------------------------------------------------------------------

/// The horizon `intervention_decision_log.anchor_seen_within_600s` is named
/// after. The column name pins the number, so it cannot drift without a
/// migration that renames the column.
pub const DECISION_OUTCOME_HORIZON_SECONDS: i64 = 600;

/// Resolves the proximal outcome of logged drift decisions once their horizon
/// has closed.
///
/// The write site records `anchor_seen_within_600s: None` and says a later pass
/// resolves it. This is that pass. It answers each decision from
/// `work_block_observation`, which shares the decision's `ON DELETE CASCADE`
/// parent and therefore still holds the evidence for every decision still on
/// disk — so the first run backfills all of history rather than only what
/// happens next.
///
/// It reports the count in `CleanupReport::deleted` because that is the field
/// the scheduler has: the report is shared with four deleting targets, and
/// widening it to describe this one would change a type they all construct.
pub struct InterventionDecisionOutcomeTarget {
    repo: Arc<dyn WorkBlockRepo>,
    batch_size: usize,
}

impl InterventionDecisionOutcomeTarget {
    pub fn new(repo: Arc<dyn WorkBlockRepo>, batch_size: usize) -> Self {
        Self { repo, batch_size }
    }
}

impl RetentionTarget for InterventionDecisionOutcomeTarget {
    fn name(&self) -> &'static str {
        "intervention_decision_log"
    }

    fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
        let horizon = chrono::Duration::seconds(DECISION_OUTCOME_HORIZON_SECONDS);
        let unresolved = self
            .repo
            .unresolved_decisions(Utc::now() - horizon, self.batch_size)?;
        let mut resolved = 0;
        for decision in unresolved {
            let (Some(block_id), Some(anchor)) = (
                decision.block_id.as_deref(),
                decision.anchor_category.as_deref(),
            ) else {
                continue;
            };
            let horizon_closed_at = decision.occurred_at + horizon;
            let anchor_seen = self.repo.observed_category_between(
                block_id,
                anchor,
                decision.occurred_at,
                horizon_closed_at,
            )?;
            // `outcome_at` is when the horizon closed, not when this pass ran.
            // The answer is a function of the evidence alone, so a decision
            // resolved three weeks late records exactly what it would have
            // recorded on time, and a backfilled row is indistinguishable from
            // a promptly resolved one.
            if self
                .repo
                .resolve_decision(&decision.decision_id, anchor_seen, horizon_closed_at)?
            {
                resolved += 1;
            }
        }
        Ok(CleanupReport { deleted: resolved })
    }
}
