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
use velvt_service::persistence::{
    BatchEvent, GateVerdict, InterventionDecision, NewUploadBatch, RawEventEntry,
    SqlitePersistence, UploadBatchStatus, WorkBlockObservation, WorkBlockOrigin, WorkBlockRecord,
};
use velvt_service::retention::{
    CleanupReport, InterventionDecisionOutcomeTarget, RawEventRetentionTarget, RetentionError,
    RetentionScheduler, RetentionTarget, SemanticEmbeddingCacheRetentionTarget,
    UploadBatchRetentionTarget, DECISION_OUTCOME_HORIZON_SECONDS,
};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, WorkBlockIntensity, WorkBlockPhase,
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
        app_stable_id: None,
        app_scope_eligible: true,
    }
}

/// Assembles `count` events into one persisted upload batch.
///
/// This is the normal state of an aged row. Retention deliberately spares rows
/// the upload pipeline still owes the backend — an upload-eligible row with no
/// `batch_event` was acked to Swift but never made it into a batch — so a TTL
/// test has to batch its fixtures to be testing the TTL at all.
fn batch_events(db: &SqlitePersistence, batch_id: &str, ids: std::ops::Range<u64>) {
    let repo = db.upload_batch_repo();
    let events: Vec<BatchEvent> = ids
        .map(|n| {
            let event = make_event(n);
            BatchEvent {
                event_id: event.event_id,
                stable_id: event.stable_id,
                label: event.label,
                category: event.category,
                taxonomy_version: event.taxonomy_version,
                classification_tier: event.classification_tier,
                occurred_at: event.occurred_at,
                duration_seconds: event.duration_seconds,
            }
        })
        .collect();
    repo.insert_batch_with_events(
        &NewUploadBatch {
            batch_id: batch_id.to_owned(),
        },
        &events,
    )
    .unwrap();
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
    batch_events(&db, "batch-aged", 0..3);
    let old_ts = (Utc::now() - chrono::Duration::hours(100)).timestamp();
    db.set_all_raw_event_created_at_for_test(old_ts).unwrap();

    // Insert 2 fresh rows (created_at = now, well within the 72h TTL).
    for n in 3..5u64 {
        repo.insert(&make_event(n)).unwrap();
    }
    batch_events(&db, "batch-fresh", 3..5);

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
    batch_events(&db, "batch-aged", 0..1200);
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
// Test 3b — Expiry never deletes an accepted event the backend has not seen
// ---------------------------------------------------------------------------

/// An upload-eligible row with no `batch_event` was acked to Swift as
/// `Accepted` but never persisted into a batch — the crash window between an
/// ack and the next flush. Nothing re-batches it except the startup recovery
/// pass, so expiring it at the TTL destroyed the only copy. Upload-batch
/// retention already spares pending and failed batches for exactly this reason.
#[test]
fn expiry_spares_eligible_events_that_never_reached_a_batch() {
    let db = open_db();
    let repo = db.raw_event_repo();

    for n in 0..4u64 {
        repo.insert(&make_event(n)).unwrap();
    }
    // Only the first two ever made it into a batch.
    batch_events(&db, "batch-aged", 0..2);
    let old_ts = (Utc::now() - chrono::Duration::hours(100)).timestamp();
    db.set_all_raw_event_created_at_for_test(old_ts).unwrap();

    let target =
        RawEventRetentionTarget::new(Arc::clone(&repo), Duration::from_secs(72 * 3600), 500);
    let report = target.run_cleanup().unwrap();

    assert_eq!(report.deleted, 2, "only the batched rows may be deleted");
    assert_eq!(
        repo.unbatched_events(10).unwrap().len(),
        2,
        "events still owed to the backend must survive the TTL"
    );
}

/// The spare applies only to rows the upload pipeline actually owes. Local-only
/// events — collected while signed out — are never uploaded, so nothing is
/// waiting on them and the TTL is their only bound.
#[test]
fn expiry_still_deletes_local_only_events_at_the_ttl() {
    let db = open_db();
    let repo = db.raw_event_repo();

    for n in 0..3u64 {
        let mut event = make_event(n);
        event.upload_eligible = false;
        repo.insert(&event).unwrap();
    }
    let old_ts = (Utc::now() - chrono::Duration::hours(100)).timestamp();
    db.set_all_raw_event_created_at_for_test(old_ts).unwrap();

    let target =
        RawEventRetentionTarget::new(Arc::clone(&repo), Duration::from_secs(72 * 3600), 500);
    let report = target.run_cleanup().unwrap();

    assert_eq!(report.deleted, 3);
    assert_eq!(db.count_raw_events_for_test().unwrap(), 0);
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
// Test 6 — Queued batches expire on the sent horizon, and only then
// ---------------------------------------------------------------------------

/// A batch the backend never accepted used to have no expiry at all: the sweep
/// named `sent` and `rejected`, and a queue filled by an unreachable host
/// reaches neither. Aged `pending` and `failed` rows are collected on the sent
/// horizon like everything else.
///
/// The other half of the assertion is the case the old test was protecting: a
/// batch inside the horizon is still owed to the backend, and retention running
/// while the batcher assembles one must not take it.
#[test]
fn upload_batch_retention_expires_queued_batches_past_the_sent_horizon() {
    let db = open_db();
    let repo = db.upload_batch_repo();

    for n in 0..3u64 {
        repo.insert_batch(&NewUploadBatch {
            batch_id: format!("batch-pend-{n}"),
        })
        .unwrap();
    }
    repo.mark_failed(
        "batch-pend-2",
        Utc::now() + chrono::Duration::minutes(15),
        "transport",
    )
    .unwrap();

    // Age the rows as aggressively as possible.
    let old_ts = (Utc::now() - chrono::Duration::days(365)).timestamp();
    db.set_all_upload_batch_created_at_for_test(old_ts).unwrap();
    // The batch the assembler is still filling, created now.
    repo.insert_batch(&NewUploadBatch {
        batch_id: "batch-in-progress".into(),
    })
    .unwrap();

    assert_eq!(db.count_upload_batches_for_test().unwrap(), 4);

    let target = UploadBatchRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(30 * 24 * 3600), // 30d sent retention
        Duration::from_secs(7 * 24 * 3600),  // 7d audit period
        500,
    );
    let report = target.run_cleanup().unwrap();

    assert_eq!(
        report.deleted, 3,
        "queued batches older than the sent horizon must expire whatever their status"
    );
    assert_eq!(db.count_upload_batches_for_test().unwrap(), 1);
    assert_eq!(
        repo.batch_status("batch-in-progress").unwrap(),
        UploadBatchStatus::Pending,
        "a batch inside the horizon is still owed to the backend"
    );
}

// ---------------------------------------------------------------------------
// Test 6b — A batch that has spent its attempts stops being retried
// ---------------------------------------------------------------------------

/// Sustained transport failure kept a batch resumable forever: every attempt
/// wrote `pending` or `failed` back, and nothing counted the attempts. A batch
/// that has failed enough times becomes terminal instead, so the queue stops
/// re-reading it and the menu bar stops describing it as retrying.
///
/// The ceiling is pinned against a measured recovery rather than a round
/// number. The first version of this test asserted `attempts >= 48` — half a
/// day at the backoff cap — which is a floor no plausible ceiling fails, and it
/// stayed green under a ceiling of 96 that sat *below* the longest outage the
/// development device has actually come back from. A floor cannot catch a
/// ceiling that is too low; only the observed maximum can.
#[test]
fn a_batch_retried_past_its_ceiling_becomes_terminal() {
    let db = open_db();
    let repo = db.upload_batch_repo();
    repo.insert_batch(&NewUploadBatch {
        batch_id: "batch-doomed".into(),
    })
    .unwrap();

    let mut attempts = 0;
    while repo.batch_status("batch-doomed").unwrap() != UploadBatchStatus::Abandoned {
        repo.mark_pending_retry(
            "batch-doomed",
            Utc::now() + chrono::Duration::minutes(15),
            "transport",
        )
        .unwrap();
        attempts += 1;
        assert!(
            attempts <= 1_000,
            "a retried batch must reach a terminal status"
        );
    }
    // 116 attempts is the most a batch on the development device ever
    // accumulated and still delivered: created 2026-08-21 14:42 UTC, sent
    // 2026-08-22 20:50 UTC, one ~30-hour outage it fully recovered from.
    // `mark_sent` does not reset `attempt_count`, so that is the cumulative
    // cost of the outage rather than a per-session count. Read out of
    // `~/.velvt/velvt-service.sqlite3` on 2026-08-31; `migrations/0030` quotes
    // the whole distribution.
    const OBSERVED_RECOVERED_OUTAGE_ATTEMPTS: u32 = 116;
    assert!(
        attempts >= 2 * OBSERVED_RECOVERED_OUTAGE_ATTEMPTS,
        "the ceiling abandons at {attempts} attempts, against a real outage of \
         {OBSERVED_RECOVERED_OUTAGE_ATTEMPTS} attempts that the device recovered from. \
         A ceiling at or below an observed recovery throws away events that would \
         have been delivered; the age sweep already bounds the queue, so this one \
         errs long"
    );

    assert!(
        repo.resumable_batches(Utc::now() + chrono::Duration::days(1))
            .unwrap()
            .is_empty(),
        "an abandoned batch is never resumed"
    );
    // Terminal, and collected by the sweep that already existed.
    db.set_all_upload_batch_created_at_for_test(
        (Utc::now() - chrono::Duration::days(365)).timestamp(),
    )
    .unwrap();
    let report = UploadBatchRetentionTarget::new(
        Arc::clone(&repo),
        Duration::from_secs(30 * 24 * 3600),
        Duration::from_secs(7 * 24 * 3600),
        500,
    )
    .run_cleanup()
    .unwrap();
    assert_eq!(report.deleted, 1);
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

// ---------------------------------------------------------------------------
// Test 8 — The decision log's proximal outcome is resolved on its horizon
// ---------------------------------------------------------------------------

fn decision_block(db: &SqlitePersistence, block_id: &str, started_at: chrono::DateTime<Utc>) {
    db.work_block_repo()
        .create(&WorkBlockRecord {
            block_id: block_id.to_owned(),
            phase: WorkBlockPhase::Active,
            intention: None,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            planned_duration_seconds: 3_600,
            started_at,
            paused_at: None,
            total_paused_seconds: 0,
            ended_at: None,
            recovered_after_restart: false,
            recovery_of: None,
            origin: WorkBlockOrigin::Manual,
            intention_expires_at: started_at + chrono::Duration::hours(24),
            updated_at: started_at,
        })
        .unwrap();
}

fn observe(db: &SqlitePersistence, block_id: &str, category: &str, at: chrono::DateTime<Utc>) {
    db.work_block_repo()
        .append_observation(
            block_id,
            &WorkBlockObservation {
                occurred_at: at,
                ended_at: Some(at + chrono::Duration::seconds(60)),
                category: category.to_owned(),
                classification_status: ClassificationStatus::Classified,
                classification_confidence: ClassificationConfidence::High,
            },
        )
        .unwrap();
}

fn log_decision(
    db: &SqlitePersistence,
    decision_id: &str,
    block_id: &str,
    anchor: Option<&str>,
    at: chrono::DateTime<Utc>,
) {
    db.work_block_repo()
        .record_decision(&InterventionDecision {
            decision_id: decision_id.to_owned(),
            occurred_at: at,
            block_id: Some(block_id.to_owned()),
            policy_version: 1,
            anchor_category: anchor.map(str::to_owned),
            switch_count: 4,
            elapsed_seconds: 600,
            remaining_seconds: 3_000,
            gate_verdict: GateVerdict::AbstainedBackoff,
            propensity: 1.0,
            anchor_seen_within_600s: None,
            outcome_at: None,
        })
        .unwrap();
}

fn logged(db: &SqlitePersistence, decision_id: &str) -> InterventionDecision {
    db.work_block_repo()
        .recent_decisions(32)
        .unwrap()
        .into_iter()
        .find(|decision| decision.decision_id == decision_id)
        .expect("decision is on disk")
}

/// The write site records the outcome as unresolved and says a later pass fills
/// it in. This is that pass, and the evidence it reads outlives the decision by
/// sharing its cascade parent — so it answers history, not only what happens
/// next.
///
/// Four cases, because each one is a different meaning of NULL: an anchor seen
/// inside the horizon, an anchor seen only after it, a horizon that has not
/// closed, and a gate that abstained before it had an anchor at all.
#[test]
fn decision_outcomes_resolve_on_the_horizon_and_only_once() {
    let db = open_db();
    let now = Utc::now();
    let closed = now - chrono::Duration::minutes(30);

    decision_block(&db, "block-seen", closed - chrono::Duration::minutes(1));
    log_decision(&db, "seen", "block-seen", Some("FOCUS_WORK"), closed);
    observe(
        &db,
        "block-seen",
        "COMMUNICATION",
        closed + chrono::Duration::seconds(60),
    );
    observe(
        &db,
        "block-seen",
        "FOCUS_WORK",
        closed + chrono::Duration::seconds(300),
    );
    // The gate abstained before it had an anchor. There is nothing to look for,
    // so this stays unresolved rather than being recorded as "did not return".
    log_decision(&db, "no-anchor", "block-seen", None, closed);

    decision_block(&db, "block-unseen", closed - chrono::Duration::minutes(1));
    log_decision(&db, "unseen", "block-unseen", Some("FOCUS_WORK"), closed);
    observe(
        &db,
        "block-unseen",
        "FOCUS_WORK",
        closed + chrono::Duration::seconds(900),
    );

    decision_block(&db, "block-early", now - chrono::Duration::minutes(2));
    log_decision(
        &db,
        "early",
        "block-early",
        Some("FOCUS_WORK"),
        now - chrono::Duration::minutes(1),
    );
    observe(
        &db,
        "block-early",
        "FOCUS_WORK",
        now - chrono::Duration::seconds(30),
    );

    let target = InterventionDecisionOutcomeTarget::new(db.work_block_repo(), 500);
    assert_eq!(target.run_cleanup().unwrap().deleted, 2);

    assert_eq!(logged(&db, "seen").anchor_seen_within_600s, Some(true));
    assert_eq!(
        logged(&db, "seen").outcome_at,
        Some(
            chrono::DateTime::from_timestamp(
                closed.timestamp() + DECISION_OUTCOME_HORIZON_SECONDS,
                0
            )
            .unwrap()
        ),
        "the outcome is dated when the horizon closed, so resolving it late \
         records what resolving it on time would have"
    );
    assert_eq!(
        logged(&db, "unseen").anchor_seen_within_600s,
        Some(false),
        "an anchor seen after the horizon is not an anchor seen within it"
    );
    assert_eq!(
        logged(&db, "early").anchor_seen_within_600s,
        None,
        "a horizon that has not closed is unresolved, not negative"
    );
    assert_eq!(
        logged(&db, "no-anchor").anchor_seen_within_600s,
        None,
        "a decision with no anchor has no horizon to answer"
    );

    assert_eq!(
        target.run_cleanup().unwrap().deleted,
        0,
        "nothing is left to resolve"
    );
    assert!(
        !db.work_block_repo()
            .resolve_decision("seen", false, Utc::now())
            .unwrap(),
        "an answered decision keeps the answer it was given, so a rerun over \
         history cannot move a number someone has already read"
    );
    assert_eq!(logged(&db, "seen").anchor_seen_within_600s, Some(true));
}

// ---------------------------------------------------------------------------
// Test 7b — The sparing rule's horizon ordering
// ---------------------------------------------------------------------------

/// `delete_expired_batch` spares upload-eligible rows that have no
/// `batch_event`, because those are exactly the rows `recover_unbatched`
/// re-queues at the next start. The rule is only safe while a batched row dies
/// before its batch does: a row whose `batch_event` cascades away re-enters the
/// spared set and is never collected again, and it is then also re-uploaded.
///
/// Two assertions, because the ordering has two halves that fail differently.
/// The first is the relationship itself, read from the config the service
/// actually loads, so an environment that inverts it fails here rather than
/// leaking rows in the field. The second is the mechanism, so the relationship
/// is not just a comparison of numbers whose consequence nobody checked.
#[test]
fn the_raw_event_horizon_stays_inside_the_batch_horizon() {
    let config = velvt_service::config::ServiceConfig::load().expect("service config loads");
    assert!(
        config.raw_event_ttl < config.sent_batch_retention,
        "raw events expire at {:?} and their batches at {:?}. With the TTL at or \
         past the batch horizon, a batched row outlives the sweep that deletes its \
         batch, the cascade removes its `batch_event`, and it is spared by \
         `delete_expired_batch` forever and re-queued by `recover_unbatched`",
        config.raw_event_ttl,
        config.sent_batch_retention
    );

    // The mechanism, with the horizons inverted on purpose: this is what the
    // assertion above is protecting against, demonstrated once so the ordering
    // is not an unexplained inequality.
    let db = open_db();
    let repo = db.raw_event_repo();
    repo.insert(&make_event(1)).unwrap();
    batch_events(&db, "batch-orphaning", 1..2);
    assert!(
        repo.unbatched_events(10).unwrap().is_empty(),
        "a batched row is not in the spared set"
    );

    db.set_all_upload_batch_created_at_for_test(
        (Utc::now() - chrono::Duration::days(365)).timestamp(),
    )
    .unwrap();
    let deleted = db
        .upload_batch_repo()
        .delete_stale_queued_batch(Utc::now(), 500)
        .unwrap();
    assert_eq!(deleted, 1);

    assert_eq!(
        repo.unbatched_events(10).unwrap().len(),
        1,
        "the cascade put the row back in the spared set"
    );
    let report = RawEventRetentionTarget::new(Arc::clone(&repo), Duration::from_secs(0), 500)
        .run_cleanup()
        .unwrap();
    assert_eq!(
        report.deleted, 0,
        "and it is now permanently spared: at a zero TTL it is still not collected. \
         The shipped ordering is what keeps this state unreachable"
    );
}

// ---------------------------------------------------------------------------
// Test 8b — The evidence a decision was made on is not evidence of a return
// ---------------------------------------------------------------------------

/// `observe_safe_category` appends the observation and then evaluates the gate,
/// so the observation that produced a decision is already on disk carrying the
/// decision's own timestamp. With an inclusive lower bound the resolver read
/// that row back as a return, which made the `AbstainedAtAnchor` verdict — the
/// one that fires exactly when the latest confident observation IS the anchor —
/// resolve to `true` for every row of it, definitionally. Zero rows of that
/// verdict existed when this was found, which is the only reason no published
/// number was wrong.
#[test]
fn an_observation_at_the_decision_instant_is_not_a_return() {
    let db = open_db();
    let now = Utc::now();
    let closed = now - chrono::Duration::minutes(30);

    // The anchor is observed at exactly the instant the gate decided. This is
    // the `AbstainedAtAnchor` shape, reproduced through the same repository the
    // gate writes through.
    decision_block(
        &db,
        "block-at-anchor",
        closed - chrono::Duration::minutes(1),
    );
    observe(&db, "block-at-anchor", "FOCUS_WORK", closed);
    log_decision(
        &db,
        "at-anchor",
        "block-at-anchor",
        Some("FOCUS_WORK"),
        closed,
    );

    // One second later is a return, and stays one. The bound moved by exactly
    // the row that cannot be evidence of anything.
    decision_block(&db, "block-after", closed - chrono::Duration::minutes(1));
    observe(&db, "block-after", "FOCUS_WORK", closed);
    log_decision(&db, "after", "block-after", Some("FOCUS_WORK"), closed);
    observe(
        &db,
        "block-after",
        "FOCUS_WORK",
        closed + chrono::Duration::seconds(1),
    );

    let target = InterventionDecisionOutcomeTarget::new(db.work_block_repo(), 500);
    assert_eq!(target.run_cleanup().unwrap().deleted, 2);

    assert_eq!(
        logged(&db, "at-anchor").anchor_seen_within_600s,
        Some(false),
        "the observation the decision was made on is not a return from it; \
         resolving it as one makes the verdict true by construction"
    );
    assert_eq!(
        logged(&db, "after").anchor_seen_within_600s,
        Some(true),
        "an anchor observed after the decision is still a return"
    );
}

// ---------------------------------------------------------------------------
// Test 9 — The embedding cache expires on the raw-event horizon
// ---------------------------------------------------------------------------

/// The cache holds a sketch derived from the window title, and its only bound
/// was a 512-row cap that a frequently revisited window never falls out of.
/// A row not re-observed inside the window expires like the raw event it was
/// derived from.
#[test]
fn semantic_embedding_cache_expires_on_the_raw_event_horizon() {
    let db = open_db();
    let learning = db.semantic_learning_store();
    for index in 0..3u8 {
        learning
            .record_embedding(&format!("{index:064x}"), &[1.0, index as f32])
            .unwrap();
    }
    db.set_semantic_embedding_updated_at_for_test(
        &[format!("{:064x}", 0), format!("{:064x}", 1)],
        (Utc::now() - chrono::Duration::days(15)).timestamp(),
    )
    .unwrap();

    let target = SemanticEmbeddingCacheRetentionTarget::with_default_retention(
        db.abstraction_map_repo(),
        500,
    );
    assert_eq!(
        target.run_cleanup().unwrap().deleted,
        2,
        "an embedding not re-observed inside the window expires"
    );
    assert!(
        learning
            .embedding(&format!("{:064x}", 2))
            .unwrap()
            .is_some(),
        "a row inside the window is still a live cache entry"
    );
    assert_eq!(target.run_cleanup().unwrap().deleted, 0);
}
