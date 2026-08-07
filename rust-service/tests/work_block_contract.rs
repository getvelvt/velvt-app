use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use uuid::Uuid;
use velvt_service::{
    persistence::SqlitePersistence,
    retention::{RetentionTarget, WorkBlockIntentionRetentionTarget},
    upload::{BatchEventPayload, BatchPayload},
    work_block::WorkBlockManager,
};
use velvt_shared_types::{StartWorkBlock, WorkBlockIntensity, WorkBlockPurpose};

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn start_with_sentinel(manager: &WorkBlockManager, sentinel: &str) {
    manager
        .start(
            StartWorkBlock {
                intention: Some(sentinel.into()),
                planned_duration_seconds: 1_500,
                purpose: Some(WorkBlockPurpose::DeepWork),
                intensity: WorkBlockIntensity::Medium,
                invitation_id: None,
            },
            now(),
        )
        .unwrap();
}

#[test]
fn intention_is_isolated_from_upload_cache_telemetry_and_debug_surfaces() {
    let sentinel = "PRIVATE_INTENTION_NEVER_LEAVES_MAC";
    let database = SqlitePersistence::open_in_memory().unwrap();
    let manager = WorkBlockManager::new(database.work_block_repo());
    start_with_sentinel(&manager, sentinel);

    let event = BatchEventPayload {
        event_id: "event-safe".into(),
        stable_id: "stable-local".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "exact_match".into(),
        occurred_at: now(),
        duration_seconds: 60,
    };
    let upload = BatchPayload::new(
        "batch-safe",
        "1",
        "0.1.5",
        vec!["document:edit".into()],
        "mvp-1",
        vec![event],
    );
    assert!(!serde_json::to_string(&upload).unwrap().contains(sentinel));
    assert!(database
        .upload_batch_repo()
        .pending_batches()
        .unwrap()
        .is_empty());
    assert!(database
        .history_cache_repo()
        .get("2027-01-15")
        .unwrap()
        .is_none());
    assert!(database
        .insight_cache_repo()
        .get("2027-01-15")
        .unwrap()
        .is_none());

    let snapshot = manager.request_state(now() + Duration::seconds(1)).unwrap();
    assert!(!format!("{snapshot:?}").contains(sentinel));
    assert_eq!(snapshot.intention.as_deref(), Some(sentinel));
}

#[test]
fn intention_expires_and_clear_all_removes_every_work_block_row() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let manager = WorkBlockManager::new(database.work_block_repo());
    start_with_sentinel(&manager, "short lived local text");

    let expired = manager.request_state(now() + Duration::hours(25)).unwrap();
    assert!(expired.intention.is_none());
    assert_eq!(manager.clear_data().unwrap().phase.as_str(), "idle");
    assert!(database.work_block_repo().latest().unwrap().is_none());
}

#[test]
fn retention_target_clears_only_expired_intention_text() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.work_block_repo();
    let manager = WorkBlockManager::new(repo.clone());
    manager
        .start(
            StartWorkBlock {
                intention: Some("expired local text".into()),
                planned_duration_seconds: 1_500,
                purpose: None,
                intensity: WorkBlockIntensity::Light,
                invitation_id: None,
            },
            Utc::now() - Duration::hours(25),
        )
        .unwrap();

    let report = WorkBlockIntentionRetentionTarget::new(repo.clone())
        .run_cleanup()
        .unwrap();
    assert_eq!(report.deleted, 1);
    let retained = repo.latest().unwrap().unwrap();
    assert!(retained.intention.is_none());
    assert_eq!(retained.phase.as_str(), "active");
}

#[test]
fn migration_upgrades_a_current_v8_database_without_losing_existing_rows() {
    let path = std::env::temp_dir().join(format!("velvt-work-block-{}.sqlite3", Uuid::new_v4()));
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migration (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    version INTEGER NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                );",
            )
            .unwrap();
        let migrations = [
            (
                1,
                include_str!("../migrations/0001_initial_persistence.sql"),
            ),
            (
                2,
                include_str!("../migrations/0002_harden_indexes_and_probe.sql"),
            ),
            (3, include_str!("../migrations/0003_upload_retry_state.sql")),
            (
                4,
                include_str!("../migrations/0004_insight_cache_negative.sql"),
            ),
            (
                5,
                include_str!("../migrations/0005_local_queue_display_label.sql"),
            ),
            (
                6,
                include_str!("../migrations/0006_classification_provenance.sql"),
            ),
            (7, include_str!("../migrations/0007_personal_overrides.sql")),
            (
                8,
                include_str!("../migrations/0008_classification_contract.sql"),
            ),
        ];
        for (version, sql) in migrations {
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    (version, format!("migration-{version}")),
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO raw_event_buffer(
                    event_id, stable_id, label, category, taxonomy_version,
                    occurred_at, classification_tier, classification_status,
                    classification_confidence, classification_source
                 ) VALUES ('existing-event', 'existing-stable', 'document:edit',
                           'FOCUS_WORK', 'mvp-1', 1800000000, 'exact_match',
                           'classified', 'high', 'seed')",
                [],
            )
            .unwrap();
    }

    let upgraded = SqlitePersistence::open(&path).unwrap();
    assert_eq!(
        upgraded
            .raw_event_repo()
            .events_before(now() + Duration::seconds(1))
            .unwrap()
            .len(),
        1
    );
    assert!(upgraded
        .schema_snapshot()
        .unwrap()
        .iter()
        .any(|name| name == "work_block"));
    drop(upgraded);
    std::fs::remove_file(path).unwrap();
}
