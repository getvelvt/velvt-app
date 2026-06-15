//! Integration tests for the R8 retention scheduler and batched-delete strategy.
//!
//! All tests use an in-memory SQLite database to avoid touching the filesystem.
//! The retention targets call real DAL methods so the SQL and the trait
//! implementations are both exercised.

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use velvt_service::persistence::{RawEventEntry, SqlitePersistence};
use velvt_service::retention::{RawEventRetentionTarget, RetentionTarget};

fn open_db() -> SqlitePersistence {
    SqlitePersistence::open_in_memory().unwrap()
}

fn make_event(n: u64) -> RawEventEntry {
    RawEventEntry {
        event_id: format!("evt-{n:04}"),
        stable_id: format!("abs_{n}"),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        occurred_at: Utc::now(),
        duration_seconds: 30,
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
        500,                             // batch_size
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
    assert_eq!(r3.deleted, 200, "cycle 3 must delete the remaining 200 rows");
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
