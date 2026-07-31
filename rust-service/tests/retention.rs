//! Integration tests for the R8 retention scheduler and batched-delete strategy.
//!
//! All tests use an in-memory SQLite database to avoid touching the filesystem.
//! The retention targets call real DAL methods so the SQL and the trait
//! implementations are both exercised.

use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use chrono::Utc;
use velvt_service::persistence::{NewUploadBatch, RawEventEntry, SqlitePersistence};
use velvt_service::retention::{
    CleanupReport, RawEventRetentionTarget, RetentionError, RetentionScheduler, RetentionTarget,
    UploadBatchRetentionTarget,
};

fn open_db() -> SqlitePersistence {
    SqlitePersistence::open_in_memory().unwrap()
}

fn make_event(n: u64) -> RawEventEntry {
    RawEventEntry {
        event_id: format!("evt-{n:04}"),
        stable_id: format!("abs_{n}"),
        label: "document:edit".into(),
        local_display_label: None,
        local_name_suggestion: None,
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "exact_match".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "seed".into(),
        occurred_at: Utc::now(),
        duration_seconds: 30,
        upload_eligible: true,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — Only expired rows are deleted; rows within TTL survive
// ---------------------------------------------------------------------------

/// Insert 3 "aged" rows (backdated to 100h ago) and 2 "fresh" rows.
/// Running `RawEventRetentionTarget` with a 72h TTL must delete only the 3
/// aged rows and leave the 2 fresh ones untouched.
#[test]
fn only_expired_rows_deleted_fresh_rows_survive() {
    let db = open_db();
    let repo = db.raw_event_repo();

    // Insert 3 rows then back-date them to 100 hours ago.
    for n in 0..3u64 {
        repo.insert(&make_event(n)).unwrap();
    }
    let old_ts = (Utc::now() - chrono::Duration::hours(100)).timestamp();
    db.set_all_raw_event_created_at_for_test(old_ts).unwrap();

    // Insert 2 fresh rows (created_at = now, well within the 72h TTL).
    for n in 3..5u64 {
        repo.insert(&make_event(n)).unwrap();
    }

    assert_eq!(db.count_raw_events_for_test().unwrap(), 5);

    let target = RawEventRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(72 * 3600), // 72h TTL → cutoff = now - 72h
        500,
    );
    let report = target.run_cleanup().unwrap();

    assert_eq!(report.deleted, 3, "exactly 3 aged rows must be deleted");
    assert_eq!(
        db.count_raw_events_for_test().unwrap(),
        2,
        "2 fresh rows must survive the TTL cutoff"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Batched delete: 1200 expired rows, batch_size 500 → 3 cycles
// ---------------------------------------------------------------------------

/// Inserts 1200 rows older than the TTL, then calls `run_cleanup()` three times
/// with `batch_size = 500`.  Each call must issue exactly one DELETE and return
/// without looping internally.
#[test]
fn batched_delete_requires_three_cycles_for_1200_rows_at_batch_size_500() {
    let db = open_db();
    let repo = db.raw_event_repo();

    for n in 0..1200u64 {
        repo.insert(&make_event(n)).unwrap();
    }
    // Back-date all 1200 rows to 100 hours ago so the 72h TTL marks them expired.
    let old_ts = (Utc::now() - chrono::Duration::hours(100)).timestamp();
    db.set_all_raw_event_created_at_for_test(old_ts).unwrap();

    assert_eq!(db.count_raw_events_for_test().unwrap(), 1200);

    let target = RawEventRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(72 * 3600), // 72h TTL
        500,                            // batch_size
    );

    // Cycle 1: deletes exactly 500.
    let r1 = target.run_cleanup().unwrap();
    assert_eq!(r1.deleted, 500, "cycle 1 must delete exactly 500 rows");
    assert_eq!(db.count_raw_events_for_test().unwrap(), 700);

    // Cycle 2: deletes exactly 500.
    let r2 = target.run_cleanup().unwrap();
    assert_eq!(r2.deleted, 500, "cycle 2 must delete exactly 500 rows");
    assert_eq!(db.count_raw_events_for_test().unwrap(), 200);

    // Cycle 3: deletes the remaining 200.
    let r3 = target.run_cleanup().unwrap();
    assert_eq!(
        r3.deleted, 200,
        "cycle 3 must delete the remaining 200 rows"
    );
    assert_eq!(db.count_raw_events_for_test().unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Test 3 — No expired rows: run_cleanup returns zero and is safe to call
// ---------------------------------------------------------------------------

/// When all rows are within the TTL, `run_cleanup()` must return `deleted = 0`
/// without error and leave the rows intact.
#[test]
fn run_cleanup_returns_zero_when_no_expired_rows_exist() {
    let db = open_db();
    let repo = db.raw_event_repo();

    for n in 0..5u64 {
        repo.insert(&make_event(n)).unwrap();
    }

    let target = RawEventRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(72 * 3600), // 72h TTL — rows just inserted, none expired
        500,
    );
    let report = target.run_cleanup().unwrap();
    assert_eq!(
        report.deleted, 0,
        "no rows should be deleted when all are within TTL"
    );
    assert_eq!(db.count_raw_events_for_test().unwrap(), 5);
}

// ---------------------------------------------------------------------------
// Test 4 — Sent batches are deleted after the retention window
// ---------------------------------------------------------------------------

/// Insert 3 sent batches, backdate `sent_at` to 35 days ago, then run
/// `UploadBatchRetentionTarget` with a 30-day sent-retention window.  All 3
/// must be deleted.  Associated `batch_event` rows cascade-delete automatically.
#[test]
fn upload_batch_retention_deletes_sent_batches_after_window() {
    let db = open_db();
    let repo = db.upload_batch_repo();

    for n in 0..3u64 {
        let batch = NewUploadBatch {
            batch_id: format!("batch-sent-{n}"),
        };
        repo.insert_batch(&batch).unwrap();
        repo.mark_sent(&batch.batch_id).unwrap();
    }

    // Backdate sent_at to 35 days ago — past the 30d retention window.
    let old_ts = (Utc::now() - chrono::Duration::days(35)).timestamp();
    db.set_all_upload_batch_sent_at_for_test(old_ts).unwrap();

    assert_eq!(db.count_upload_batches_for_test().unwrap(), 3);

    let target = UploadBatchRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(30 * 24 * 3600), // 30d sent retention
        Duration::from_secs(7 * 24 * 3600),  // 7d rejected audit
        500,
    );
    let report = target.run_cleanup().unwrap();

    assert_eq!(report.deleted, 3, "3 aged sent batches must be deleted");
    assert_eq!(db.count_upload_batches_for_test().unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Test 5 — Rejected batches are deleted after the audit period
// ---------------------------------------------------------------------------

/// Insert 3 rejected batches, backdate `created_at` to 10 days ago, then run
/// `UploadBatchRetentionTarget` with a 7-day rejected audit period.  All 3
/// must be deleted.
#[test]
fn upload_batch_retention_deletes_rejected_batches_after_audit_period() {
    let db = open_db();
    let repo = db.upload_batch_repo();

    for n in 0..3u64 {
        let batch = NewUploadBatch {
            batch_id: format!("batch-rej-{n}"),
        };
        repo.insert_batch(&batch).unwrap();
        repo.mark_rejected(&batch.batch_id, "server_rejected")
            .unwrap();
    }

    // Backdate created_at to 10 days ago — past the 7d audit period.
    let old_ts = (Utc::now() - chrono::Duration::days(10)).timestamp();
    db.set_all_upload_batch_created_at_for_test(old_ts).unwrap();

    assert_eq!(db.count_upload_batches_for_test().unwrap(), 3);

    let target = UploadBatchRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(30 * 24 * 3600), // 30d sent retention (no sent rows)
        Duration::from_secs(7 * 24 * 3600),  // 7d audit period → cutoff = now - 7d
        500,
    );
    let report = target.run_cleanup().unwrap();

    assert_eq!(report.deleted, 3, "3 aged rejected batches must be deleted");
    assert_eq!(db.count_upload_batches_for_test().unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Test 6 — Pending batches are never deleted (in-progress batch protection)
// ---------------------------------------------------------------------------

/// Even when `created_at` is far in the past and retention windows are zero,
/// batches with `status = 'pending'` must survive every cleanup pass.
/// This covers the "retention while batcher is assembling a batch" edge case.
#[test]
fn upload_batch_retention_never_deletes_pending_batches() {
    let db = open_db();
    let repo = db.upload_batch_repo();

    for n in 0..3u64 {
        repo.insert_batch(&NewUploadBatch {
            batch_id: format!("batch-pend-{n}"),
        })
        .unwrap();
    }

    // Age the rows as aggressively as possible.
    let old_ts = (Utc::now() - chrono::Duration::days(365)).timestamp();
    db.set_all_upload_batch_created_at_for_test(old_ts).unwrap();

    assert_eq!(db.count_upload_batches_for_test().unwrap(), 3);

    // Zero-second retention means "everything is past the cutoff" — but only
    // for the matching status columns.  Pending rows have no sent_at, and the
    // SQL filter requires status = 'sent' or status = 'rejected'.
    let target = UploadBatchRetentionTarget::new(
        Arc::clone(&repo),
        Duration::ZERO, // sent_retention: cutoff = now
        Duration::ZERO, // audit period: cutoff = now
        500,
    );
    let report = target.run_cleanup().unwrap();

    assert_eq!(
        report.deleted, 0,
        "pending batches must never be deleted by retention"
    );
    assert_eq!(db.count_upload_batches_for_test().unwrap(), 3);
}

// ---------------------------------------------------------------------------
// Test 7 — Slow DB during retention does not starve the async IPC path
// ---------------------------------------------------------------------------

/// `RetentionTarget::run_cleanup()` is synchronous.  When it blocks a tokio
/// worker thread, the multi-thread runtime must keep other tasks running on
/// remaining threads.  This verifies the architectural guarantee that the IPC
/// path is not starved by a slow retention target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_slow_during_retention_does_not_block_async_tasks() {
    struct BlockingTarget {
        started: Arc<tokio::sync::Notify>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl RetentionTarget for BlockingTarget {
        fn name(&self) -> &'static str {
            "blocking"
        }
        fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
            self.started.notify_one();

            let (lock, signal) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = signal.wait(released).unwrap();
            }

            Ok(CleanupReport { deleted: 0 })
        }
    }

    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let scheduler =
        RetentionScheduler::new(Duration::from_secs(60), shutdown_rx).add_target(BlockingTarget {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
    let scheduler_task = tokio::spawn(async move { scheduler.run().await });

    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("retention cleanup did not start");

    let fast_task = tokio::spawn(async { tokio::task::yield_now().await });
    let fast_task_result = tokio::time::timeout(Duration::from_secs(5), fast_task).await;

    let _ = shutdown_tx.send(true);

    let (lock, signal) = &*release;
    *lock.lock().unwrap() = true;
    signal.notify_all();

    tokio::time::timeout(Duration::from_secs(5), scheduler_task)
        .await
        .expect("retention scheduler did not stop")
        .expect("retention scheduler task must not panic");
    fast_task_result
        .expect("async task was deadlocked by blocking retention")
        .expect("async task must not panic");
}
