use chrono::{TimeZone, Utc};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;
use velvt_service::abstraction::{
    AbstractionEngine, ClassificationConfidence, ClassificationPlugin, ClassificationResult,
    ClassificationSource, ClassificationStatus, ClassificationTier, DefaultTitleAbstractor,
    EmbeddingError, EmbeddingMetrics, EmbeddingModel, EmbeddingSimilarityPlugin,
    InMemoryMappingStore, Taxonomy, TitleAbstractor,
};
use velvt_shared_types::RawEvent;

fn raw_event(app_name: &str, window_title: &str) -> RawEvent {
    RawEvent {
        event_id: Uuid::new_v4(),
        occurred_at: Utc.with_ymd_and_hms(2026, 6, 13, 12, 0, 0).unwrap(),
        app_name: app_name.to_owned(),
        window_title: window_title.to_owned(),
        bundle_id: None,
        focused_document_url: None,
        duration_seconds: 0,
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
    assert_eq!(
        first.classification_status(),
        ClassificationStatus::Classified
    );
    assert_eq!(
        first.classification_confidence(),
        ClassificationConfidence::High
    );
    assert_eq!(first.classification_source(), ClassificationSource::Seed);
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
        let expected_tier = if seed.label() == "reference:browser" {
            ClassificationTier::Fallback
        } else {
            ClassificationTier::ExactMatch
        };
        assert_eq!(result.classification_tier(), expected_tier);
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
    assert_eq!(
        result.classification_status(),
        ClassificationStatus::Unclassified
    );
    assert_eq!(
        result.classification_confidence(),
        ClassificationConfidence::None
    );
    assert_eq!(
        result.classification_source(),
        ClassificationSource::Fallback
    );
    assert!(started.elapsed() < Duration::from_millis(20));
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
    assert_eq!(
        result.classification_status(),
        ClassificationStatus::Classified
    );
    assert_eq!(
        result.classification_confidence(),
        ClassificationConfidence::Medium
    );
    assert_eq!(
        result.classification_source(),
        ClassificationSource::Heuristic
    );
}

#[test]
fn local_purpose_heuristic_routes_common_unknown_families_before_unlogged() {
    let cases = [
        ("PrusaSlicer", "private title", "design:3d", "FOCUS_WORK"),
        (
            "Unknown App",
            "Invoice reconciliation - Stripe",
            "task:finance",
            "TASK_MANAGEMENT",
        ),
        (
            "Unknown App",
            "Pull request review",
            "document:code",
            "FOCUS_WORK",
        ),
    ];

    for (app_name, window_title, expected_label, expected_category) in cases {
        let result = engine().process(raw_event(app_name, window_title)).unwrap();

        assert_eq!(result.label(), expected_label);
        assert_eq!(result.category(), expected_category);
        assert_eq!(
            result.classification_tier(),
            ClassificationTier::LocalPurposeHeuristic
        );
    }
}

#[test]
fn browser_context_routes_specific_tabs_before_generic_browser_seed() {
    let cases = [
        (
            "Google Chrome",
            "docs.google.com/document/d/private",
            "document:docs",
            "FOCUS_WORK",
        ),
        (
            "Safari",
            "sheets.google.com/spreadsheets/d/private",
            "document:sheets",
            "FOCUS_WORK",
        ),
        (
            "Arc",
            "mail.google.com/mail/u/0/#inbox",
            "communication:gmail",
            "COMMUNICATION",
        ),
        (
            "Firefox",
            "youtube.com/watch?v=private",
            "video:youtube",
            "PASSIVE_CONSUMPTION",
        ),
        (
            "Google Chrome",
            "chatgpt.com/c/private",
            "reference:ai_assistant",
            "REFERENCE",
        ),
    ];

    for (app_name, window_title, expected_label, expected_category) in cases {
        let result = engine().process(raw_event(app_name, window_title)).unwrap();

        assert_eq!(result.label(), expected_label);
        assert_eq!(result.category(), expected_category);
        assert_eq!(
            result.classification_tier(),
            ClassificationTier::LocalPurposeHeuristic
        );
    }
}

#[test]
fn focused_document_url_classifies_generic_titles_and_keys_identity_by_site() {
    let engine = engine();
    let mut first = raw_event("Safari", "Inbox");
    first.focused_document_url = Some("https://mail.google.com/mail/u/0/#inbox".into());
    let mut second = raw_event("Safari", "A different private subject");
    second.focused_document_url = Some("https://mail.google.com/mail/u/1/thread/private".into());

    let first = engine.process(first).unwrap();
    let second = engine.process(second).unwrap();

    assert_eq!(first.label(), "communication:gmail");
    assert_eq!(first.category(), "COMMUNICATION");
    assert_eq!(first.stable_id(), second.stable_id());
}

#[test]
fn different_focused_websites_receive_distinct_local_identities() {
    let engine = engine();
    let mut github = raw_event("Google Chrome", "Work");
    github.focused_document_url = Some("https://github.com/velvt/private".into());
    let mut youtube = raw_event("Google Chrome", "Work");
    youtube.focused_document_url = Some("https://youtube.com/watch?v=private".into());

    let github = engine.process(github).unwrap();
    let youtube = engine.process(youtube).unwrap();

    assert_eq!(github.label(), "reference:github");
    assert_eq!(youtube.label(), "video:youtube");
    assert_ne!(github.stable_id(), youtube.stable_id());
}

#[test]
fn generic_browser_without_specific_tab_context_still_uses_browser_seed() {
    let result = engine()
        .process(raw_event("Google Chrome", "private title"))
        .unwrap();

    assert_eq!(result.label(), "reference:browser");
    assert_eq!(result.category(), "REFERENCE");
    assert_eq!(result.classification_tier(), ClassificationTier::Fallback);
    assert_eq!(
        result.classification_status(),
        ClassificationStatus::Ambiguous
    );
    assert_eq!(
        result.classification_confidence(),
        ClassificationConfidence::Low
    );
    assert_eq!(
        result.classification_source(),
        ClassificationSource::Fallback
    );
}

#[test]
fn conflicting_browser_cues_abstain_instead_of_using_rule_order() {
    let result = engine()
        .process(raw_event(
            "Google Chrome",
            "GitHub discussion about youtube.com/watch/private",
        ))
        .unwrap();

    assert_eq!(result.label(), "unlogged");
    assert_eq!(result.category(), "UNLOGGED");
    assert_eq!(
        result.classification_status(),
        ClassificationStatus::Ambiguous
    );
    assert_eq!(
        result.classification_confidence(),
        ClassificationConfidence::Low
    );
    assert_eq!(
        result.classification_source(),
        ClassificationSource::Heuristic
    );
}

#[test]
fn classification_precedence_is_explicit() {
    let taxonomy = "mvp-1";
    let make = |source, status| {
        ClassificationResult::with_quality(
            "document:inferred",
            "FOCUS_WORK",
            taxonomy,
            ClassificationTier::ExactMatch,
            status,
            ClassificationConfidence::High,
            source,
        )
    };
    let ranks = [
        make(
            ClassificationSource::UserRule,
            ClassificationStatus::Classified,
        )
        .precedence(),
        make(ClassificationSource::Seed, ClassificationStatus::Classified).precedence(),
        make(
            ClassificationSource::Heuristic,
            ClassificationStatus::Classified,
        )
        .precedence(),
        make(
            ClassificationSource::Embedding,
            ClassificationStatus::Classified,
        )
        .precedence(),
        make(
            ClassificationSource::Fallback,
            ClassificationStatus::Ambiguous,
        )
        .precedence(),
        make(
            ClassificationSource::Fallback,
            ClassificationStatus::Unclassified,
        )
        .precedence(),
    ];

    assert!(ranks.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn developer_terminal_sessions_are_focus_work_not_system_noise() {
    let result = engine()
        .process(raw_event("Terminal", "cargo test"))
        .unwrap();

    assert_eq!(result.label(), "document:code");
    assert_eq!(result.category(), "FOCUS_WORK");
    assert_eq!(result.classification_tier(), ClassificationTier::ExactMatch);
}

#[test]
fn common_developer_apps_are_not_unlogged() {
    let cases = [
        ("Windsurf", "private title"),
        ("Warp", "cargo test"),
        ("Ghostty", "npm run dev"),
        ("Docker Desktop", "containers"),
        ("Postman", "request builder"),
    ];

    for (app_name, window_title) in cases {
        let result = engine().process(raw_event(app_name, window_title)).unwrap();

        assert_eq!(result.label(), "document:code", "{app_name}");
        assert_eq!(result.category(), "FOCUS_WORK", "{app_name}");
        assert_ne!(
            result.classification_tier(),
            ClassificationTier::Fallback,
            "{app_name}"
        );
    }
}

#[test]
fn browser_work_contexts_route_to_specific_safe_labels() {
    let cases = [
        (
            "Orion",
            "developer.apple.com/documentation/private",
            "reference:read",
            "REFERENCE",
        ),
        (
            "Google Chrome",
            "linear.app/acme/issue/private",
            "task:manage",
            "TASK_MANAGEMENT",
        ),
        (
            "Safari",
            "notion.so/private-page",
            "document:write",
            "FOCUS_WORK",
        ),
    ];

    for (app_name, window_title, expected_label, expected_category) in cases {
        let result = engine().process(raw_event(app_name, window_title)).unwrap();

        assert_eq!(result.label(), expected_label);
        assert_eq!(result.category(), expected_category);
        assert_eq!(
            result.classification_tier(),
            ClassificationTier::LocalPurposeHeuristic
        );
    }
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

struct ConflictingEmbeddingModel;

impl EmbeddingModel for ConflictingEmbeddingModel {
    fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
        Ok(vec![1.0, 1.0])
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
    assert_eq!(
        first.classification_status(),
        ClassificationStatus::Classified
    );
    assert_eq!(
        first.classification_confidence(),
        ClassificationConfidence::High
    );
    assert_eq!(
        first.classification_source(),
        ClassificationSource::Embedding
    );
}

#[test]
fn embedding_with_no_winning_margin_abstains() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let tier2 = EmbeddingSimilarityPlugin::new(
        Arc::new(ConflictingEmbeddingModel),
        std::collections::HashMap::from([
            ("FOCUS_WORK".to_owned(), vec![1.0, 0.0]),
            ("REFERENCE".to_owned(), vec![0.0, 1.0]),
        ]),
        taxonomy.version(),
        0.7,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();
    let result = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_builtin_plugins_with_embedding(Some(tier2))
        .build()
        .unwrap()
        .process(raw_event("Unknown App", "private"))
        .unwrap();

    assert_eq!(result.category(), "UNLOGGED");
    assert_eq!(
        result.classification_status(),
        ClassificationStatus::Ambiguous
    );
    assert_eq!(
        result.classification_confidence(),
        ClassificationConfidence::Low
    );
    assert_eq!(
        result.classification_source(),
        ClassificationSource::Embedding
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
