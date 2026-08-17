use chrono::{DateTime, Duration, TimeZone, Utc};
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
    WorkBlockCategoryCorrection, WorkBlockIntervention, WorkBlockInterventionOutcome,
    WorkBlockRecord,
};
use velvt_service::upload::BatchEventPayload;
use velvt_shared_types::RawEvent;
use velvt_shared_types::{InterventionSalience, WorkBlockIntensity, WorkBlockPhase};

fn database() -> SqlitePersistence {
    SqlitePersistence::open_in_memory().unwrap()
}

fn work_block_record(block_id: &str, started_at: DateTime<Utc>) -> WorkBlockRecord {
    WorkBlockRecord {
        block_id: block_id.to_owned(),
        phase: WorkBlockPhase::Active,
        intention: None,
        purpose: None,
        intensity: WorkBlockIntensity::Medium,
        planned_duration_seconds: 1_500,
        started_at,
        paused_at: None,
        total_paused_seconds: 0,
        ended_at: None,
        recovered_after_restart: false,
        recovery_of: None,
        intention_expires_at: started_at + Duration::hours(24),
        updated_at: started_at,
    }
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
                local_name_suggestion: None,
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "exact_match".into(),
                classification_status: "classified".into(),
                classification_confidence: "high".into(),
                classification_source: "seed".into(),
                occurred_at: now - Duration::seconds(index * 300),
                duration_seconds: 240,
                upload_eligible: true,
                app_stable_id: None,
                app_scope_eligible: true,
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
        local_name_suggestion: None,
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "exact_match".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "seed".into(),
        occurred_at: timestamp(10),
        duration_seconds: 0,
        upload_eligible: true,
        app_stable_id: None,
        app_scope_eligible: true,
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
                local_name_suggestion: None,
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "exact_match".into(),
                classification_status: "classified".into(),
                classification_confidence: "high".into(),
                classification_source: "seed".into(),
                occurred_at: timestamp(10 + index as i64),
                duration_seconds: 70 - (index as u64 * 10),
                upload_eligible: true,
                app_stable_id: None,
                app_scope_eligible: true,
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
fn every_local_display_persistence_debug_surface_is_redacted() {
    use velvt_service::persistence::{LocalDisplayAggregate, LocalEventMetadata};

    let sentinel = "PRIVATE_LOCAL_NAME_SENTINEL";
    let metadata = LocalEventMetadata {
        local_display_label: Some(sentinel.into()),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "user_rule".into(),
    };
    let aggregate = LocalDisplayAggregate {
        label: sentinel.into(),
        duration_seconds: 60,
    };

    assert!(!format!("{metadata:?}").contains(sentinel));
    assert!(!format!("{aggregate:?}").contains(sentinel));
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
        .save_personal_override(
            initial.stable_id(),
            "COMMUNICATION",
            Some("Client messages"),
        )
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
    assert_eq!(corrected.local_display_label(), Some("Client messages"));
    assert_eq!(corrected.taxonomy_version(), "mvp-2");
    assert_eq!(corrected.classification_source().as_str(), "user_rule");
    let correction_history = repository.personal_overrides(25).unwrap();
    assert_eq!(correction_history.len(), 1);
    assert_eq!(correction_history[0].stable_id, corrected.stable_id());
    assert_eq!(
        correction_history[0].local_activity_name.as_deref(),
        Some("Client messages")
    );
    assert_eq!(correction_history[0].category, "COMMUNICATION");
    let correction_debug = format!("{:?}", correction_history[0]);
    assert!(!correction_debug.contains("Client messages"));
    assert!(!correction_debug.contains("private title"));
    let uploads = database.upload_batch_repo();
    uploads
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "uploaded-correction".into(),
            },
            &[BatchEvent {
                event_id: Uuid::new_v4().to_string(),
                stable_id: corrected.stable_id().to_owned(),
                label: corrected.label().to_owned(),
                category: corrected.category().to_owned(),
                taxonomy_version: corrected.taxonomy_version().to_owned(),
                classification_tier: corrected.classification_tier().as_str().to_owned(),
                occurred_at: timestamp(10),
                duration_seconds: 60,
            }],
        )
        .unwrap();
    uploads.mark_sent("uploaded-correction").unwrap();
    assert_eq!(
        uploads
            .delete_sent_batch(Utc::now() + Duration::minutes(1), 25)
            .unwrap(),
        1
    );
    assert_eq!(repository.personal_overrides(25).unwrap().len(), 1);

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
        .save_personal_override(reverted.stable_id(), "COMMUNICATION", None)
        .unwrap();
    assert_eq!(repository.personal_override_count().unwrap(), 1);
    assert_eq!(repository.reset_personal_overrides().unwrap(), 1);
    assert_eq!(repository.personal_override_count().unwrap(), 0);
}

#[test]
fn correction_history_search_paginates_beyond_twenty_five_and_survives_upload_restart_edit_and_undo(
) {
    let path = std::env::temp_dir().join(format!(
        "velvt-correction-history-{}.sqlite3",
        Uuid::new_v4()
    ));
    {
        let database = SqlitePersistence::open(&path).unwrap();
        let repository = database.abstraction_map_repo();
        for index in 0..45 {
            let stable_id = format!("abs_{index:03}");
            repository
                .upsert(&AbstractionMapping {
                    key_hash: format!("{index:064x}"),
                    stable_id: stable_id.clone(),
                    label: "reference:inferred".into(),
                    category: "REFERENCE".into(),
                    taxonomy_version: "mvp-1".into(),
                    classification_tier: "exact_match".into(),
                    classification_status: "classified".into(),
                    classification_confidence: "high".into(),
                    classification_source: "user_rule".into(),
                    display_name: None,
                })
                .unwrap();
            repository
                .save_personal_override(&stable_id, "REFERENCE", Some(&format!("Research {index}")))
                .unwrap();
        }

        let (first, total) = repository.search_personal_overrides(None, 0, 20).unwrap();
        let (second, _) = repository.search_personal_overrides(None, 20, 20).unwrap();
        let (third, _) = repository.search_personal_overrides(None, 40, 20).unwrap();
        assert_eq!(
            (first.len(), second.len(), third.len(), total),
            (20, 20, 5, 45)
        );
        let (matches, match_count) = repository
            .search_personal_overrides(Some("Research 3"), 0, 20)
            .unwrap();
        assert_eq!(matches.len(), 11);
        assert_eq!(match_count, 11);

        let uploads = database.upload_batch_repo();
        uploads
            .insert_batch_with_events(
                &NewUploadBatch {
                    batch_id: "correction-history-upload".into(),
                },
                &[BatchEvent {
                    event_id: Uuid::new_v4().to_string(),
                    stable_id: "abs_030".into(),
                    label: "reference:inferred".into(),
                    category: "REFERENCE".into(),
                    taxonomy_version: "mvp-1".into(),
                    classification_tier: "exact_match".into(),
                    occurred_at: timestamp(10),
                    duration_seconds: 60,
                }],
            )
            .unwrap();
        uploads.mark_sent("correction-history-upload").unwrap();
        uploads
            .delete_sent_batch(Utc::now() + Duration::minutes(1), 20)
            .unwrap();
    }

    {
        let database = SqlitePersistence::open(&path).unwrap();
        let repository = database.abstraction_map_repo();
        assert_eq!(
            repository.search_personal_overrides(None, 0, 20).unwrap().1,
            45
        );
        repository
            .save_personal_override("abs_030", "COMMUNICATION", Some("Edited alias"))
            .unwrap();
        let (edited, total) = repository
            .search_personal_overrides(Some("edited alias"), 0, 20)
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(edited[0].category, "COMMUNICATION");
        assert!(repository.remove_personal_override("abs_030").unwrap());
    }

    let reopened = SqlitePersistence::open(&path).unwrap();
    assert_eq!(
        reopened
            .abstraction_map_repo()
            .personal_override_count()
            .unwrap(),
        44
    );
    drop(reopened);
    fs::remove_file(path).unwrap();
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
        .save_personal_override(original.stable_id(), "COMMUNICATION", None)
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
            .save_personal_override(&stable_id, "COMMUNICATION", None)
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

fn event_for_app(event_id: &str, app_stable_id: Option<&str>, eligible: bool) -> RawEventEntry {
    RawEventEntry {
        event_id: event_id.into(),
        stable_id: format!("stable-{event_id}"),
        label: "document:edit".into(),
        local_display_label: None,
        local_name_suggestion: None,
        category: "UNLOGGED".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "fallback".into(),
        classification_status: "unclassified".into(),
        classification_confidence: "low".into(),
        classification_source: "fallback".into(),
        occurred_at: Utc::now(),
        duration_seconds: 120,
        upload_eligible: true,
        app_stable_id: app_stable_id.map(str::to_owned),
        app_scope_eligible: eligible,
    }
}

/// A correction has to generalize to the app, so the next window of the same
/// app is already classified rather than unclassified again.
#[test]
fn a_correction_generalizes_to_every_window_of_the_app() {
    let database = database();
    let events = database.raw_event_repo();
    let app_key = "a".repeat(64);
    events
        .insert(&event_for_app("evt-1", Some(&app_key), true))
        .unwrap();

    let generalized = database
        .abstraction_map_repo()
        .save_personal_app_override("evt-1", "FOCUS_WORK", Some("Editing"))
        .unwrap();

    assert!(generalized, "an eligible event must generalize");
    let stored = database
        .abstraction_mapping_store()
        .personal_app_override(&app_key)
        .unwrap()
        .expect("the app-scoped override is readable by the engine");
    assert_eq!(stored.category, "FOCUS_WORK");
    assert_eq!(stored.local_activity_name.as_deref(), Some("Editing"));
}

/// One browser tab being focus work says nothing about the next, so a
/// correction there stays bound to the window it was made on.
#[test]
fn a_browser_window_correction_is_not_generalized_to_the_whole_browser() {
    let database = database();
    let app_key = "b".repeat(64);
    database
        .raw_event_repo()
        .insert(&event_for_app("evt-2", Some(&app_key), false))
        .unwrap();

    let generalized = database
        .abstraction_map_repo()
        .save_personal_app_override("evt-2", "FOCUS_WORK", None)
        .unwrap();

    assert!(
        !generalized,
        "a site-scoped window must not recolour the app"
    );
    assert!(database
        .abstraction_mapping_store()
        .personal_app_override(&app_key)
        .unwrap()
        .is_none());
}

/// Events recorded before app-scoped corrections existed carry no app
/// identity and cannot be generalized retroactively.
#[test]
fn an_event_without_an_app_identity_cannot_be_generalized() {
    let database = database();
    database
        .raw_event_repo()
        .insert(&event_for_app("evt-3", None, true))
        .unwrap();

    assert!(!database
        .abstraction_map_repo()
        .save_personal_app_override("evt-3", "FOCUS_WORK", None)
        .unwrap());
}

/// Correcting the same app twice sharpens the existing rule rather than
/// failing on the primary key, and counts the corrections.
#[test]
fn correcting_the_same_app_twice_updates_the_rule() {
    let database = database();
    let app_key = "c".repeat(64);
    let events = database.raw_event_repo();
    events
        .insert(&event_for_app("evt-4", Some(&app_key), true))
        .unwrap();
    events
        .insert(&event_for_app("evt-5", Some(&app_key), true))
        .unwrap();
    let maps = database.abstraction_map_repo();

    assert!(maps
        .save_personal_app_override("evt-4", "COMMUNICATION", None)
        .unwrap());
    assert!(maps
        .save_personal_app_override("evt-5", "FOCUS_WORK", None)
        .unwrap());

    assert_eq!(
        database
            .abstraction_mapping_store()
            .personal_app_override(&app_key)
            .unwrap()
            .unwrap()
            .category,
        "FOCUS_WORK",
        "the most recent correction wins"
    );
}

/// Invariant 4's input: the rolling counter the auto-demotion rule reads.
/// Counts what the product actually records — `was_focused` is main's
/// vocabulary for "this offer should never have fired".
#[test]
fn the_wrong_intervention_counter_counts_delivered_and_was_focused() {
    let database = database();
    let blocks = database.work_block_repo();
    let start = Utc::now() - Duration::seconds(600);

    for (index, outcome) in [
        WorkBlockInterventionOutcome::Returned,
        WorkBlockInterventionOutcome::WasFocused,
        WorkBlockInterventionOutcome::WasFocused,
        WorkBlockInterventionOutcome::Dismissed,
    ]
    .into_iter()
    .enumerate()
    {
        let block_id = format!("wic-{index}");
        blocks
            .create(&work_block_record(&block_id, start))
            .expect("block starts");
        blocks
            .record_intervention(
                &block_id,
                &WorkBlockIntervention {
                    offered_at: start + Duration::seconds(index as i64),
                    action_id: "protect_next_10".into(),
                    anchor_category: "FOCUS_WORK".into(),
                    switch_count: 4,
                    window_seconds: 600,
                    outcome,
                    outcome_at: Some(start + Duration::seconds(index as i64 + 5)),
                    salience: InterventionSalience::Normal,
                },
            )
            .expect("intervention records");
    }

    let counts = blocks
        .wrong_intervention_counts(start - Duration::seconds(60))
        .expect("counts read");
    assert_eq!(
        counts.delivered, 4,
        "every delivered offer is the denominator"
    );
    assert_eq!(
        counts.was_focused, 2,
        "only was_focused disputes the judgment"
    );

    // Offers before the window are outside the rolling rate entirely.
    let later = blocks
        .wrong_intervention_counts(Utc::now() + Duration::seconds(60))
        .expect("counts read");
    assert_eq!(later.delivered, 0);
}

/// Invariant 3: a correction is believed instantly, and restating it does not
/// rewrite when it was first believed.
#[test]
fn a_block_correction_is_recorded_once_and_keeps_its_first_timestamp() {
    let database = database();
    let blocks = database.work_block_repo();
    // Whole seconds: corrected_at persists as a Unix integer, so a fixture
    // carrying microseconds would fail on the round-trip for a reason that has
    // nothing to do with the behaviour under test.
    let start = Utc.timestamp_opt(Utc::now().timestamp() - 300, 0).unwrap();
    blocks
        .create(&work_block_record("corr-1", start))
        .expect("block starts");

    let first = WorkBlockCategoryCorrection {
        category: "PASSIVE_CONSUMPTION".into(),
        counts_as_category: "FOCUS_WORK".into(),
        corrected_at: start + Duration::seconds(30),
    };
    let restated = WorkBlockCategoryCorrection {
        corrected_at: start + Duration::seconds(120),
        ..first.clone()
    };
    blocks
        .record_category_correction("corr-1", &first)
        .expect("first correction");
    blocks
        .record_category_correction("corr-1", &restated)
        .expect("restating is a no-op, not an error");

    let stored = blocks.category_corrections("corr-1").expect("read back");
    assert_eq!(stored.len(), 1, "restating does not duplicate");
    assert_eq!(
        stored[0].corrected_at, first.corrected_at,
        "the first correction's timestamp is when the user was believed"
    );
    assert_eq!(stored[0].counts_as_category, "FOCUS_WORK");
}
