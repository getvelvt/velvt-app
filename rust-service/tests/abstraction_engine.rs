use chrono::{TimeZone, Utc};
use std::sync::Arc;
use uuid::Uuid;
use velvt_service::abstraction::{
    AbstractionEngine, AbstractionPlugin, InMemoryMappingStore, PluginMatch, RawKey, Taxonomy,
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
fn same_input_twice_produces_identical_stable_id_and_label() {
    let engine = engine();

    let first = engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();
    let second = engine
        .process(raw_event("VS Code", "private project"))
        .unwrap();

    assert_eq!(first.stable_id, second.stable_id);
    assert_eq!(first.label, second.label);
    assert_eq!(first.category, "FOCUS_WORK");
    assert_eq!(first.taxonomy_version, "mvp-1");
}

#[test]
fn seed_dictionary_assigns_expected_categories() {
    let cases = [
        ("Docs", "FOCUS_WORK"),
        ("VS Code", "FOCUS_WORK"),
        ("Xcode", "FOCUS_WORK"),
        ("Notion", "FOCUS_WORK"),
        ("Overleaf", "FOCUS_WORK"),
        ("Linear", "FOCUS_WORK"),
        ("YouTube", "PASSIVE_CONSUMPTION"),
        ("Netflix", "PASSIVE_CONSUMPTION"),
        ("TikTok", "PASSIVE_CONSUMPTION"),
        ("Twitch", "PASSIVE_CONSUMPTION"),
        ("Reddit", "SOCIAL_FEED"),
        ("Twitter", "SOCIAL_FEED"),
        ("X", "SOCIAL_FEED"),
        ("Instagram", "SOCIAL_FEED"),
        ("Facebook", "SOCIAL_FEED"),
        ("Slack", "COMMUNICATION"),
        ("Discord", "COMMUNICATION"),
        ("iMessage", "COMMUNICATION"),
        ("Gmail", "COMMUNICATION"),
        ("Outlook", "COMMUNICATION"),
        ("Jira", "TASK_MANAGEMENT"),
        ("Todoist", "TASK_MANAGEMENT"),
        ("Asana", "TASK_MANAGEMENT"),
        ("Wikipedia", "REFERENCE"),
        ("MDN", "REFERENCE"),
        ("Stack Overflow", "REFERENCE"),
        ("GitHub", "REFERENCE"),
        ("Finder", "SYSTEM"),
        ("System Preferences", "SYSTEM"),
        ("Terminal", "SYSTEM"),
        ("unknown application", "UNLOGGED"),
    ];
    let engine = engine();

    for (app_name, expected_category) in cases {
        let result = engine
            .process(raw_event(app_name, "private title"))
            .unwrap();
        assert_eq!(result.category, expected_category, "{app_name}");
    }
}

struct TestPlugin;

impl AbstractionPlugin for TestPlugin {
    fn priority(&self) -> u32 {
        1
    }

    fn classify(&self, raw_key: &RawKey) -> Option<PluginMatch> {
        (raw_key.app_name() == "Test Target").then(|| ("test:target".to_owned(), "test-v1"))
    }
}

struct NoOpPlugin;

impl AbstractionPlugin for NoOpPlugin {
    fn priority(&self) -> u32 {
        0
    }

    fn classify(&self, _raw_key: &RawKey) -> Option<PluginMatch> {
        None
    }
}

struct UnsafePlugin;

impl AbstractionPlugin for UnsafePlugin {
    fn priority(&self) -> u32 {
        1
    }

    fn classify(&self, raw_key: &RawKey) -> Option<PluginMatch> {
        Some((raw_key.window_title().to_owned(), "unsafe-v1"))
    }
}

#[test]
fn custom_plugin_fires_without_affecting_other_patterns() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let engine = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_plugin(NoOpPlugin)
        .register_plugin(TestPlugin)
        .register_builtin_plugins()
        .build()
        .unwrap();

    let target = engine.process(raw_event("Test Target", "private")).unwrap();
    let other = engine.process(raw_event("Slack", "private")).unwrap();

    assert_eq!(target.label, "test:target");
    assert_eq!(target.category, "UNLOGGED");
    assert_eq!(other.category, "COMMUNICATION");
}

#[test]
fn abstracted_event_serialization_excludes_raw_inputs() {
    let result = engine()
        .process(raw_event("PRIVATE_APP", "PRIVATE_WINDOW_TITLE"))
        .unwrap();

    let json = serde_json::to_string(&result).unwrap();

    assert!(!json.contains("PRIVATE_APP"));
    assert!(!json.contains("PRIVATE_WINDOW_TITLE"));
    assert!(!json.contains("app_name"));
    assert!(!json.contains("window_title"));
}

#[test]
fn unsafe_plugin_label_is_rejected_before_output() {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let engine = AbstractionEngine::builder(Arc::new(InMemoryMappingStore::default()), taxonomy)
        .register_plugin(UnsafePlugin)
        .build()
        .unwrap();

    let error = engine
        .process(raw_event("PRIVATE_APP", "PRIVATE WINDOW TITLE"))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "abstraction plugin returned an invalid label"
    );
}

#[test]
fn invalid_taxonomy_returns_clear_error() {
    let error = Taxonomy::from_json(
        br#"{
            "version":"mvp-1",
            "default_category":"UNLOGGED",
            "label_categories":{"unlogged:":"UNLOGGED"},
            "seed_applications":[]
        }"#,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "taxonomy has no seed application entries"
    );
}
