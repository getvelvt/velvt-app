use chrono::{TimeZone, Utc};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;
use velvt_service::abstraction::{
    AbstractionEngine, ClassificationPlugin, ClassificationResult, ClassificationTier,
    DefaultTitleAbstractor, EmbeddingError, EmbeddingMetrics, EmbeddingModel,
    EmbeddingSimilarityPlugin, InMemoryMappingStore, Taxonomy, TitleAbstractor,
};
use velvt_shared_types::RawEvent;

fn raw_event(app_name: &str, window_title: &str) -> RawEvent {
    RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: Utc.with_ymd_and_hms(2026, 6, 13, 12, 0, 0).unwrap(),
        app_name: app_name.to_owned(),
        window_title: window_title.to_owned(),
        bundle_id: None,
    }
}

fn engine() -> AbstractionEngine {
    AbstractionEngine::from_builtin_taxonomy(Arc::new(InMemoryMappingStore::default())).unwrap()
}

#[test]
fn tier1_is_deterministic_and_completes_under_one_millisecond() {
    let engine = engine();
    let first = engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();
    let second = engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();
    let mut samples = Vec::with_capacity(10_000);
    for _ in 0..10_000 {
        let started = Instant::now();
        let _ = engine
            .process(raw_event("VS Code", "private project"))
            .unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();
    let mean = samples.iter().sum::<Duration>() / samples.len() as u32;
    let p50 = samples[4_999];
    let p95 = samples[9_499];
    let p99 = samples[9_899];

    assert_eq!(first.stable_id(), second.stable_id());
    assert_eq!(first.label(), second.label());
    assert_eq!(first.category(), second.category());
    assert_eq!(first.category(), "FOCUS_WORK");
    assert_eq!(first.taxonomy_version(), "mvp-1");
    assert_eq!(first.classification_tier(), ClassificationTier::ExactMatch);
    eprintln!("Tier 1 mean={mean:?} p50={p50:?} p95={p95:?} p99={p99:?}");
    assert!(mean < Duration::from_millis(1), "Tier 1 mean was {mean:?}");
    assert!(p95.as_millis() < 1, "Tier 1 p95 was {p95:?}");
}

#[test]
fn stable_id_survives_engine_recreation_through_store_contract() {
    let store = Arc::new(InMemoryMappingStore::default());
    let first_engine = AbstractionEngine::from_builtin_taxonomy(store.clone()).unwrap();
    let first = first_engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();
    drop(first_engine);
    let second_engine = AbstractionEngine::from_builtin_taxonomy(store).unwrap();
    let second = second_engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();

    assert_eq!(first.stable_id(), second.stable_id());
}

#[test]
fn inputs_with_a_shared_hash_prefix_receive_distinct_stable_ids() {
    let (first_title, second_title) = find_shared_hash_prefix();
    let engine = engine();

    let first = engine
        .process(raw_event("Unknown App", &first_title))
        .unwrap();
    let second = engine
        .process(raw_event("Unknown App", &second_title))
        .unwrap();

    assert_ne!(first_title, second_title);
    assert_ne!(first.stable_id(), second.stable_id());
}

fn find_shared_hash_prefix() -> (String, String) {
    let mut prefixes = std::collections::HashMap::new();
    for index in 0..10_000 {
        let title = format!("collision-candidate-{index}");
        let digest = composite_digest("Unknown App", &title);
        let prefix = [digest[0], digest[1]];
        if let Some(previous) = prefixes.insert(prefix, title.clone()) {
            return (previous, title);
        }
    }
    panic!("failed to find a shared SHA-256 prefix");
}

fn composite_digest(app_name: &str, window_title: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"velvt:abstraction-key:v1");
    for value in [app_name, window_title] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().to_vec()
}

#[test]
fn every_seed_entry_routes_through_tier1() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let engine = engine();
    let seeds = taxonomy.seed_applications();
    let categories: std::collections::HashSet<_> =
        seeds.iter().map(|seed| seed.category()).collect();

    assert!(seeds.len() >= 30);
    for required in [
        "FOCUS_WORK",
        "PASSIVE_CONSUMPTION",
        "SOCIAL_FEED",
        "COMMUNICATION",
        "TASK_MANAGEMENT",
        "REFERENCE",
        "SYSTEM",
    ] {
        assert!(categories.contains(required), "{required}");
    }

    for seed in seeds {
        let result = engine
            .process(raw_event(seed.app_name_pattern(), "private title"))
            .unwrap();
        assert_eq!(result.label(), seed.label(), "{}", seed.app_name_pattern());
        assert_eq!(
            result.category(),
            seed.category(),
            "{}",
            seed.app_name_pattern()
        );
        assert_eq!(result.classification_tier(), ClassificationTier::ExactMatch);
    }
}

#[test]
fn unknown_app_uses_unlogged_fallback() {
    let started = Instant::now();
    let result = engine()
        .process(raw_event("Unknown App", "private title"))
        .unwrap();

    assert_eq!(result.label(), "unlogged");
    assert_eq!(result.category(), "UNLOGGED");
    assert_eq!(result.classification_tier(), ClassificationTier::Fallback);
    assert!(started.elapsed() < Duration::from_millis(5));
}

#[test]
fn local_purpose_heuristic_classifies_unknown_cad_app_family() {
    let result = engine()
        .process(raw_event("Autodesk Fusion", "private title"))
        .unwrap();

    assert_eq!(result.label(), "design:cad");
    assert_eq!(result.category(), "FOCUS_WORK");
    assert_eq!(
        result.classification_tier(),
        ClassificationTier::LocalPurposeHeuristic
    );
}

struct TestPlugin;

impl ClassificationPlugin for TestPlugin {
    fn classify(&self, app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        (app_name == "Test Target").then(|| {
            ClassificationResult::new(
                "test:target",
                "UNLOGGED",
                "mvp-1",
                ClassificationTier::ExactMatch,
            )
        })
    }
}

struct UnsafePlugin;

impl ClassificationPlugin for UnsafePlugin {
    fn classify(&self, _app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        Some(ClassificationResult::new(
            window_title,
            "UNLOGGED",
            "mvp-1",
            ClassificationTier::ExactMatch,
        ))
    }
}

#[test]
fn custom_plugin_fires_without_affecting_other_paths() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let engine = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_plugin(TestPlugin)
        .register_builtin_plugins()
        .build()
        .unwrap();

    assert_eq!(
        engine
            .process(raw_event("Test Target", "private"))
            .unwrap()
            .label(),
        "test:target"
    );
    assert_eq!(
        engine
            .process(raw_event("Slack", "private"))
            .unwrap()
            .category(),
        "COMMUNICATION"
    );
}

#[test]
fn plugin_cannot_return_a_raw_input_as_a_label() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let engine = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_plugin(UnsafePlugin)
        .build()
        .unwrap();

    assert!(engine
        .process(raw_event("private_app", "private:title"))
        .is_err());
}

#[test]
fn abstracted_event_serialization_excludes_raw_inputs_and_stable_key() {
    let result = engine()
        .process(raw_event("PRIVATE_APP", "PRIVATE_WINDOW_TITLE"))
        .unwrap();
    let json = serde_json::to_string(&result).unwrap();

    for forbidden in [
        "PRIVATE_APP",
        "PRIVATE_WINDOW_TITLE",
        "app_name",
        "window_title",
        "stable_key",
    ] {
        assert!(!json.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn no_op_title_abstractor_is_wired() {
    let abstractor = DefaultTitleAbstractor;
    assert_eq!(abstractor.abstract_title("private title"), "private title");
    assert_eq!(
        engine()
            .process(raw_event("VS Code", "private title"))
            .unwrap()
            .category(),
        "FOCUS_WORK"
    );
}

struct BelowThresholdModel;

impl EmbeddingModel for BelowThresholdModel {
    fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(vec![0.0, 1.0])
    }
}

struct HighSimilarityModel;

impl EmbeddingModel for HighSimilarityModel {
    fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(vec![1.0, 0.0])
    }
}

#[test]
fn all_three_tiers_fall_through_end_to_end() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let tier2 = EmbeddingSimilarityPlugin::new(
        Arc::new(BelowThresholdModel),
        std::collections::HashMap::from([("FOCUS_WORK".to_owned(), vec![1.0, 0.0])]),
        taxonomy.version(),
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();
    let engine = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_builtin_plugins_with_embedding(Some(tier2))
        .build()
        .unwrap();

    assert_eq!(
        engine
            .process(raw_event("VS Code", "private"))
            .unwrap()
            .classification_tier(),
        ClassificationTier::ExactMatch
    );
    assert_eq!(
        engine
            .process(raw_event("Unknown App", "private"))
            .unwrap()
            .classification_tier(),
        ClassificationTier::Fallback
    );
}

#[test]
fn tier2_path_is_deterministic_and_reports_embedding_tier() {
    let store = Arc::new(InMemoryMappingStore::default());
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let tier2 = EmbeddingSimilarityPlugin::new(
        Arc::new(HighSimilarityModel),
        std::collections::HashMap::from([("FOCUS_WORK".to_owned(), vec![1.0, 0.0])]),
        taxonomy.version(),
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();
    let engine = AbstractionEngine::builder(store, taxonomy)
        .register_builtin_plugins_with_embedding(Some(tier2))
        .build()
        .unwrap();

    let first = engine.process(raw_event("Unknown IDE", "")).unwrap();
    let second = engine.process(raw_event("Unknown IDE", "")).unwrap();

    assert_eq!(first.stable_id(), second.stable_id());
    assert_eq!(first.label(), second.label());
    assert_eq!(first.category(), second.category());
    assert_eq!(
        first.classification_tier(),
        ClassificationTier::EmbeddingSimilarity
    );
}

#[test]
fn invalid_taxonomy_returns_clear_error() {
    let error = Taxonomy::from_json(
        br#"{
            "category_taxonomy_version":"mvp-1",
            "default_category":"UNLOGGED",
            "categories":["UNLOGGED"],
            "seed_applications":[]
        }"#,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "taxonomy has no seed application entries"
    );
}
