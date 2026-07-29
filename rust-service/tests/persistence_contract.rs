use chrono::{Duration, TimeZone, Utc};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use uuid::Uuid;
use velvt_service::abstraction::{
    AbstractionEngine, EmbeddingError, EmbeddingMetrics, EmbeddingModel, EmbeddingSimilarityPlugin,
    Taxonomy,
};
use velvt_service::dashboard;
use velvt_service::delivery::{parse_insight_with_rehydrator, LocalInsightRehydrator};
use velvt_service::persistence::{
    AbstractionMapping, BatchEvent, HistoryCacheEntry, InsightCacheEntry, NewUploadBatch,
    PersistenceError, RawEventEntry, SqlitePersistence, UploadBatchStatus,
};
use velvt_service::upload::BatchEventPayload;
use velvt_shared_types::RawEvent;

fn database() -> SqlitePersistence {
    SqlitePersistence::open_in_memory().unwrap()
}

#[test]
fn bounded_seven_day_dashboard_query_is_indexed_and_measured() {
    let database = database();
    let repository = database.raw_event_repo();
    let now = Utc::now();
    for index in 0..1_400_i64 {
        repository
            .insert(&RawEventEntry {
                event_id: format!("measure-{index}"),
                stable_id: format!("stable-{index}"),
                label: "document:edit".into(),
                local_display_label: Some(if index % 2 == 0 { "Docs" } else { "VS Code" }.into()),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "exact_match".into(),
                classification_status: "classified".into(),
                classification_confidence: "high".into(),
                classification_source: "seed".into(),
                occurred_at: now - Duration::seconds(index * 300),
                duration_seconds: 240,
                upload_eligible: true,
            })
            .unwrap();
    }

    let started = std::time::Instant::now();
    let result = dashboard::snapshot(&*repository, None, now, 3_600, 0).unwrap();
    let elapsed = started.elapsed();

    assert_eq!(result.daily_activity.len(), 7);
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "bounded query took {elapsed:?}"
    );
    eprintln!(
        "bounded_scope3_dashboard_query_ms={:.3}",
        elapsed.as_secs_f64() * 1_000.0
    );
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
        classification_tier: "exact_match".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "seed".into(),
        display_name: Some("Code Editor".into()),
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
        local_display_label: Some("Docs".into()),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "exact_match".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "seed".into(),
        occurred_at: timestamp(10),
        duration_seconds: 0,
        upload_eligible: true,
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
fn local_display_aggregation_is_bounded_to_five_labels_plus_other() {
    let database = database();
    let repository = database.raw_event_repo();
    let labels = [
        Some("VS Code"),
        Some("Slack"),
        Some("GitHub"),
        Some("Docs"),
        Some("Browser"),
        Some("AI Assistant"),
        None,
    ];
    for (index, label) in labels.into_iter().enumerate() {
        repository
            .insert(&RawEventEntry {
                event_id: format!("event-{index}"),
                stable_id: format!("abs-{index}"),
                label: "document:edit".into(),
                local_display_label: label.map(str::to_owned),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "exact_match".into(),
                classification_status: "classified".into(),
                classification_confidence: "high".into(),
                classification_source: "seed".into(),
                occurred_at: timestamp(10 + index as i64),
                duration_seconds: 70 - (index as u64 * 10),
                upload_eligible: true,
            })
            .unwrap();
    }

    let aggregates = repository
        .local_display_aggregates(timestamp(0), timestamp(100), 5)
        .unwrap();

    assert_eq!(aggregates.len(), 6);
    assert_eq!(aggregates[0].label, "VS Code");
    assert_eq!(aggregates[4].label, "Browser");
    assert_eq!(aggregates[5].label, "Other");
    assert_eq!(aggregates[5].duration_seconds, 30);
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
        classification_tier: "exact_match".into(),
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
fn sent_upload_batches_do_not_report_stale_retry_errors() {
    let database = database();
    let repository = database.upload_batch_repo();
    let batch = NewUploadBatch {
        batch_id: "batch-with-recovered-error".into(),
    };

    repository.insert_batch(&batch).unwrap();
    repository
        .mark_failed(&batch.batch_id, timestamp(2_000), "authentication_required")
        .unwrap();
    assert_eq!(
        repository
            .queue_diagnostics()
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("authentication_required")
    );

    repository.mark_sent(&batch.batch_id).unwrap();

    let diagnostics = repository.queue_diagnostics().unwrap();
    assert_eq!(diagnostics.failed_batch_count, 0);
    assert_eq!(diagnostics.last_error_code, None);
    assert!(diagnostics.last_successful_sync_at.is_some());
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
        is_negative: false,
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
        classification_tier: "exact_match".into(),
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
        focused_document_url: None,
        duration_seconds: 0,
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

#[test]
fn personal_override_runs_before_plugins_and_is_not_taxonomy_version_scoped() {
    let database = database();
    let repository = database.abstraction_map_repo();
    let event = RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: timestamp(10),
        app_name: "Unknown Local App".into(),
        window_title: "private title".into(),
        bundle_id: None,
        focused_document_url: None,
        duration_seconds: 0,
    };
    let initial = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(event.clone())
        .unwrap();
    assert_eq!(initial.category(), "UNLOGGED");
    repository
        .save_personal_override(initial.stable_id(), "COMMUNICATION")
        .unwrap();
    let updated_taxonomy = Taxonomy::from_json(
        br#"{
            "category_taxonomy_version":"mvp-2",
            "default_category":"UNLOGGED",
            "categories":["COMMUNICATION","UNLOGGED"],
            "seed_applications":[
                {"app_name_pattern":"Known","label":"communication:chat","category":"COMMUNICATION"}
            ]
        }"#,
    )
    .unwrap();
    let corrected =
        AbstractionEngine::builder(database.abstraction_mapping_store(), updated_taxonomy)
            .register_builtin_plugins()
            .build()
            .unwrap()
            .process(event)
            .unwrap();

    assert_eq!(corrected.category(), "COMMUNICATION");
    assert_eq!(corrected.label(), "communication:inferred");
    assert_eq!(corrected.taxonomy_version(), "mvp-2");
    assert_eq!(corrected.classification_source().as_str(), "user_rule");

    let different_title =
        AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
            .unwrap()
            .process(RawEvent {
                event_id: Uuid::new_v4(),
                occurred_at: timestamp(11),
                duration_seconds: 0,
                app_name: "Unknown Local App".into(),
                window_title: "different private title".into(),
                bundle_id: None,
                focused_document_url: None,
            })
            .unwrap();
    assert_eq!(different_title.classification_source().as_str(), "fallback");

    assert!(repository
        .remove_personal_override(corrected.stable_id())
        .unwrap());
    let reverted = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: timestamp(11),
            duration_seconds: 0,
            app_name: "Unknown Local App".into(),
            window_title: "private title".into(),
            bundle_id: None,
            focused_document_url: None,
        })
        .unwrap();
    assert_eq!(reverted.classification_source().as_str(), "fallback");

    repository
        .save_personal_override(reverted.stable_id(), "COMMUNICATION")
        .unwrap();
    assert_eq!(repository.personal_override_count().unwrap(), 1);
    assert_eq!(repository.reset_personal_overrides().unwrap(), 1);
    assert_eq!(repository.personal_override_count().unwrap(), 0);
}

#[test]
fn explicit_correction_generalizes_locally_and_remove_forgets_semantic_prototype() {
    struct ConstantModel;
    impl EmbeddingModel for ConstantModel {
        fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
            Ok(vec![0.0, 1.0])
        }
    }

    let database = database();
    let repository = database.abstraction_map_repo();
    let build_engine = || {
        let plugin = EmbeddingSimilarityPlugin::new(
            Arc::new(ConstantModel),
            HashMap::from([("FOCUS_WORK".to_owned(), vec![1.0, 0.0])]),
            "mvp-1",
            0.72,
            std::time::Duration::from_millis(20),
            Arc::new(EmbeddingMetrics::default()),
        )
        .unwrap()
        .with_learning_store(database.semantic_learning_store());
        AbstractionEngine::builder(
            database.abstraction_mapping_store(),
            Taxonomy::from_builtin().unwrap(),
        )
        .register_builtin_plugins_with_embedding(Some(plugin))
        .build()
        .unwrap()
    };
    let event = |title: &str| RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: timestamp(10),
        app_name: "Novel Local Tool".into(),
        window_title: title.into(),
        bundle_id: None,
        focused_document_url: None,
        duration_seconds: 0,
    };

    let original = build_engine()
        .process(event("first private context"))
        .unwrap();
    repository
        .save_personal_override(original.stable_id(), "COMMUNICATION")
        .unwrap();
    assert_eq!(repository.personal_semantic_prototype_count().unwrap(), 1);

    let generalized = build_engine()
        .process(event("unseen related context"))
        .unwrap();
    assert_eq!(generalized.category(), "COMMUNICATION");
    assert_eq!(generalized.classification_source().as_str(), "user_rule");

    assert!(repository
        .remove_personal_override(original.stable_id())
        .unwrap());
    assert_eq!(repository.personal_semantic_prototype_count().unwrap(), 0);
    let forgotten = build_engine()
        .process(event("third related context"))
        .unwrap();
    assert_ne!(forgotten.category(), "COMMUNICATION");
}

#[test]
fn personal_semantic_memory_is_bounded_and_reset_with_exact_overrides() {
    let database = database();
    let repository = database.abstraction_map_repo();
    let semantic = database.semantic_learning_store();
    for index in 0..20 {
        let key_hash = format!("{index:064x}");
        let stable_id = format!("semantic-id-{index}");
        repository
            .upsert(&AbstractionMapping {
                key_hash: key_hash.clone(),
                stable_id: stable_id.clone(),
                label: "unlogged".into(),
                category: "UNLOGGED".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "fallback".into(),
                classification_status: "ambiguous".into(),
                classification_confidence: "low".into(),
                classification_source: "fallback".into(),
                display_name: None,
            })
            .unwrap();
        semantic
            .record_embedding(&key_hash, &[1.0, index as f32 / 100.0])
            .unwrap();
        repository
            .save_personal_override(&stable_id, "COMMUNICATION")
            .unwrap();
    }
    assert_eq!(repository.personal_override_count().unwrap(), 20);
    assert_eq!(repository.personal_semantic_prototype_count().unwrap(), 12);
    assert_eq!(repository.reset_personal_overrides().unwrap(), 20);
    assert_eq!(repository.personal_semantic_prototype_count().unwrap(), 0);
}

#[test]
fn classifier_artifact_version_is_counted_only_in_local_telemetry() {
    let database = database();
    let semantic = database.semantic_learning_store();
    semantic.record_classifier_use("builtin-hash-v1").unwrap();
    semantic.record_classifier_use("builtin-hash-v1").unwrap();
    assert_eq!(
        database
            .abstraction_map_repo()
            .classifier_artifact_count("builtin-hash-v1")
            .unwrap(),
        2
    );
}

#[test]
fn event_upload_and_structured_insight_round_trip_rehydrates_real_app_name_locally() {
    let database = database();
    let raw = RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: timestamp(10),
        app_name: "Slack".into(),
        window_title: "Private team conversation".into(),
        bundle_id: None,
        focused_document_url: None,
        duration_seconds: 0,
    };
    let abstracted = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(raw.clone())
        .unwrap();
    assert_eq!(abstracted.local_display_label(), Some("Slack"));
    let outbound = serde_json::to_value(BatchEventPayload::from_abstracted(
        raw.event_id.to_string(),
        &abstracted,
        60,
    ))
    .unwrap();
    assert_eq!(outbound["abstraction_type"], "communication:inferred");
    assert!(!outbound.to_string().contains("Private team conversation"));
    assert!(!outbound.to_string().contains("Slack"));

    let rehydrator = LocalInsightRehydrator::new(database.abstraction_map_repo());
    let delivered = parse_insight_with_rehydrator(
        serde_json::json!({
            "date": "2026-05-23",
            "text": "You switched to a communication app 23 times.",
            "template": "You switched to {label_0} 23 times.",
            "label_references": [
                {"token": "label_0", "label": "communication:slack"}
            ],
            "confidence_level": "high",
            "low_confidence": false,
            "generated_at": "2026-05-24T00:00:00Z"
        }),
        Some(&rehydrator),
    )
    .unwrap();

    assert_eq!(delivered.text, "You switched to Slack 23 times.");
    assert!(!delivered.text.contains("communication:slack"));
}

#[test]
fn raw_title_never_becomes_a_local_display_label_or_ready_insight() {
    let database = database();
    let raw_title = "Secret acquisition plan";
    let abstracted = AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store())
        .unwrap()
        .process(RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: timestamp(10),
            duration_seconds: 0,
            app_name: "Unknown Local App".into(),
            window_title: raw_title.into(),
            bundle_id: None,
            focused_document_url: None,
        })
        .unwrap();

    assert_eq!(abstracted.local_display_label(), None);
    let rehydrator = LocalInsightRehydrator::new(database.abstraction_map_repo());
    let delivered = parse_insight_with_rehydrator(
        serde_json::json!({
            "date": "2026-05-23",
            "text": "You switched activities.",
            "template": "You switched to {label_0}.",
            "label_references": [{"token": "label_0", "label": "unlogged"}],
            "confidence_level": "low",
            "low_confidence": true,
            "generated_at": "2026-05-24T00:00:00Z"
        }),
        Some(&rehydrator),
    )
    .unwrap();

    assert!(!delivered.text.contains(raw_title));
    assert_eq!(delivered.text, "You switched to an activity.");
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
                    classification_tier: "exact_match".into(),
                    classification_status: "classified".into(),
                    classification_confidence: "high".into(),
                    classification_source: "seed".into(),
                    display_name: None,
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
        .set_host_backoff("dev-api.getvelvt.com", 3, timestamp(120))
        .unwrap();

    assert_eq!(
        database
            .upload_batch_repo()
            .host_backoff_attempt("dev-api.getvelvt.com")
            .unwrap(),
        3
    );
    repository
        .clear_host_backoff("dev-api.getvelvt.com")
        .unwrap();
    assert_eq!(
        repository
            .host_backoff_attempt("dev-api.getvelvt.com")
            .unwrap(),
        0
    );
}
