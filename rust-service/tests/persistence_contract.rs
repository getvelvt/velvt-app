use chrono::{Duration, TimeZone, Utc};
use std::fs;
use std::sync::Arc;
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::persistence::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, NewUploadBatch,
    PersistenceError, RawEventEntry, SqlitePersistence, UploadBatchStatus,
};
use velvt_shared_types::RawEvent;

fn database() -> SqlitePersistence {
    SqlitePersistence::open_in_memory().unwrap()
}

fn timestamp(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).unwrap()
}

#[test]
fn migrations_are_idempotent_and_create_required_indexes() {
    let database = database();
    let before = database.schema_snapshot().unwrap();

    database.run_migrations().unwrap();

    assert_eq!(before, database.schema_snapshot().unwrap());
    for table in [
        "abstraction_map",
        "raw_event_buffer",
        "upload_batch",
        "batch_event",
        "history_cache",
        "insight_cache",
    ] {
        assert!(before.iter().any(|entry| entry == table), "{table}");
    }
    for index in [
        "idx_abstraction_map_created_at",
        "idx_abstraction_map_updated_at",
        "idx_raw_event_buffer_occurred_at",
        "idx_raw_event_buffer_created_at",
        "idx_upload_batch_created_at",
        "idx_upload_batch_sent_at",
        "idx_batch_event_occurred_at",
        "idx_batch_event_created_at",
        "idx_history_cache_date",
        "idx_history_cache_ttl",
        "idx_history_cache_created_at",
        "idx_insight_cache_date",
        "idx_insight_cache_ttl",
        "idx_insight_cache_created_at",
        "idx_batch_event_batch_id",
    ] {
        assert!(before.iter().any(|entry| entry == index), "{index}");
    }
    assert!(before
        .iter()
        .any(|entry| entry == "persistence_migration_probe"));
    let schema = database.schema_sql().unwrap().join(" ");
    assert!(schema.matches("INTEGER PRIMARY KEY AUTOINCREMENT").count() >= 8);
    assert!(
        schema
            .matches("created_at INTEGER NOT NULL DEFAULT (unixepoch())")
            .count()
            >= 8
    );
}

#[test]
fn abstraction_map_repo_contract() {
    let database = database();
    let repository = database.abstraction_map_repo();
    let mapping = AbstractionMapping {
        key_hash: "a".repeat(64),
        stable_id: "abs_1".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
    };

    repository.upsert(&mapping).unwrap();
    assert!(repository.exists(&mapping.key_hash).unwrap());
    assert_eq!(repository.get(&mapping.stable_id).unwrap(), mapping);
}

#[test]
fn raw_event_repo_contract_and_timestamp_query_uses_index() {
    let database = database();
    let repository = database.raw_event_repo();
    let event = RawEventEntry {
        event_id: "event-1".into(),
        stable_id: "abs_1".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        occurred_at: timestamp(10),
        duration_seconds: 0,
    };

    repository.insert(&event).unwrap();
    assert_eq!(
        repository.events_before(timestamp(11)).unwrap(),
        vec![event.clone()]
    );
    assert!(database
        .raw_event_query_plan()
        .unwrap()
        .contains("idx_raw_event_buffer_occurred_at"));
    assert_eq!(repository.delete_before(timestamp(11)).unwrap(), 1);
    assert!(repository.events_before(timestamp(11)).unwrap().is_empty());
}

#[test]
fn upload_batch_repo_contract() {
    let database = database();
    let repository = database.upload_batch_repo();
    let batch = NewUploadBatch {
        batch_id: "batch-1".into(),
    };
    let event = BatchEvent {
        event_id: "event-1".into(),
        stable_id: "abs_1".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        occurred_at: timestamp(10),
        duration_seconds: 0,
    };

    repository.insert_batch(&batch).unwrap();
    repository
        .add_event_to_batch(&batch.batch_id, &event)
        .unwrap();
    let pending = repository.pending_batches().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].events, vec![event]);
    repository.mark_sent(&batch.batch_id).unwrap();
    assert!(repository.pending_batches().unwrap().is_empty());
}

#[test]
fn history_cache_repo_contract() {
    let database = database();
    let repository = database.history_cache_repo();
    let entry = HistoryCacheEntry {
        date: "2026-06-14".into(),
        payload: r#"{"event_count":4}"#.into(),
        expires_at: timestamp(2_000_000_000) + Duration::days(1),
    };

    repository.upsert(&entry).unwrap();
    assert_eq!(repository.get(&entry.date).unwrap(), Some(entry.clone()));
    assert_eq!(repository.invalidate(&entry.date).unwrap(), 1);
    assert_eq!(repository.get(&entry.date).unwrap(), None);
}

#[test]
fn insight_cache_repo_contract() {
    let database = database();
    let repository = database.insight_cache_repo();
    let entry = InsightCacheEntry {
        date: "2026-06-14".into(),
        payload: "Ready-to-display local insight".into(),
        expires_at: timestamp(2_000_000_000) + Duration::days(1),
    };

    repository.upsert(&entry).unwrap();
    assert_eq!(repository.get(&entry.date).unwrap(), Some(entry.clone()));
    assert_eq!(repository.invalidate(&entry.date).unwrap(), 1);
    assert_eq!(repository.get(&entry.date).unwrap(), None);
}

#[test]
fn multi_table_batch_write_rolls_back_on_failure() {
    let database = database();
    let repository = database.upload_batch_repo();
    let batch = NewUploadBatch {
        batch_id: "batch-rollback".into(),
    };
    let event = BatchEvent {
        event_id: "event-duplicate".into(),
        stable_id: "abs_1".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        occurred_at: timestamp(10),
        duration_seconds: 0,
    };

    assert!(repository
        .insert_batch_with_events(&batch, &[event.clone(), event])
        .is_err());
    assert!(repository.pending_batches().unwrap().is_empty());
}

#[test]
fn schema_has_no_forbidden_raw_content_columns() {
    let database = database();
    let schema = database.schema_sql().unwrap().join(" ").to_lowercase();

    for forbidden in ["app_name", "window_title", "bundle_id", "url", "file_path"] {
        assert!(!schema.contains(forbidden), "{forbidden}");
    }
    assert_eq!(UploadBatchStatus::Pending.as_str(), "pending");
}

#[test]
fn abstraction_engine_uses_sqlite_mapping_store_across_recreation() {
    let database = database();
    let repository = database.abstraction_map_repo();
    let event = RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: timestamp(10),
        app_name: "VS Code".into(),
        window_title: "private title".into(),
        bundle_id: None,
    };
    let first = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(event.clone())
        .unwrap();
    let second = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(event)
        .unwrap();

    assert_eq!(first.stable_id(), second.stable_id());
    let persisted = repository.get(first.stable_id()).unwrap();
    assert!(!format!("{persisted:?}").contains("private title"));
}

#[tokio::test]
async fn concurrent_writes_are_serialized_without_busy_errors_or_panics() {
    let database = Arc::new(database());
    let mut tasks = Vec::new();
    for task_id in 0..2 {
        let database = Arc::clone(&database);
        tasks.push(tokio::task::spawn_blocking(move || {
            let repository = database.abstraction_map_repo();
            for index in 0..100 {
                repository.upsert(&AbstractionMapping {
                    key_hash: format!("{:064x}", task_id * 100 + index),
                    stable_id: format!("abs-{task_id}-{index}"),
                    label: "document:edit".into(),
                    category: "FOCUS_WORK".into(),
                    taxonomy_version: "mvp-1".into(),
                })?;
            }
            Ok::<(), PersistenceError>(())
        }));
    }

    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert!(database
        .abstraction_map_repo()
        .exists(&format!("{:064x}", 199))
        .unwrap());
}

#[test]
fn opening_a_missing_database_file_creates_it_and_runs_migrations() {
    let directory = std::env::temp_dir().join(format!("velvt-r3-{}", Uuid::new_v4()));
    let path = directory.join("nested").join("velvt.sqlite3");
    assert!(!path.exists());

    let database = SqlitePersistence::open(&path).unwrap();

    assert!(path.exists());
    assert!(database
        .schema_snapshot()
        .unwrap()
        .iter()
        .any(|name| name == "abstraction_map"));
    drop(database);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn missing_row_update_returns_typed_not_found_error() {
    let database = database();

    let error = database
        .upload_batch_repo()
        .mark_sent("missing-batch")
        .unwrap_err();

    assert!(matches!(
        error,
        PersistenceError::NotFound {
            entity: "upload_batch"
        }
    ));
}

#[test]
fn missing_row_query_returns_typed_not_found_error() {
    let database = database();

    let error = database
        .abstraction_map_repo()
        .get("missing-id")
        .unwrap_err();

    assert!(matches!(
        error,
        PersistenceError::NotFound {
            entity: "abstraction_map"
        }
    ));
}

#[test]
fn failed_batches_are_resumed_and_rejected_batches_are_terminal() {
    let database = database();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch(&NewUploadBatch {
            batch_id: "batch-recovery".into(),
        })
        .unwrap();

    repository
        .mark_failed("batch-recovery", timestamp(50), "transport")
        .unwrap();
    let resumable = repository.resumable_batches(timestamp(50)).unwrap();
    assert_eq!(resumable.len(), 1);
    assert_eq!(resumable[0].status, UploadBatchStatus::Failed);
    assert_eq!(resumable[0].attempt_count, 1);

    repository
        .mark_rejected("batch-recovery", "raw_field_rejected")
        .unwrap();
    assert!(repository
        .resumable_batches(timestamp(2_000_000_000))
        .unwrap()
        .is_empty());
}

#[test]
fn host_backoff_attempt_survives_repository_recreation() {
    let database = database();
    let repository = database.upload_batch_repo();
    repository
        .set_host_backoff("api.velvt.test", 3, timestamp(120))
        .unwrap();

    assert_eq!(
        database
            .upload_batch_repo()
            .host_backoff_attempt("api.velvt.test")
            .unwrap(),
        3
    );
    repository.clear_host_backoff("api.velvt.test").unwrap();
    assert_eq!(
        repository.host_backoff_attempt("api.velvt.test").unwrap(),
        0
    );
}
