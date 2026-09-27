//! Payloads as the service actually emits them, validated against
//! `proto/schema/`.
//!
//! `shared-types/tests/schema_conformance.rs` checks every schema against the
//! Rust types, but with instances it generates from the schema itself, so it
//! cannot see a rule the schema states and the code does not keep. Two such
//! drifts survived it until this file existed:
//!
//! - `local_dashboard.daily_activity` was declared with `minItems`/`maxItems`
//!   7 from protocol 20, while the service has sent `DAILY_ACTIVITY_DAYS` = 14
//!   rows since 2026-08-27. Every generated instance had 7 rows.
//! - Fields Rust omits when they are `None` (`skip_serializing_if`) were listed
//!   as `required`: a daily-activity segment's `representative_event_id`,
//!   `stable_id` and `suggested_name`, and `work_block_state.active_intervention`,
//!   which is absent whenever no offer is pending. Every generated instance
//!   filled them in.
//!
//! A third survived until 2026-09-27: `menu_status.queued_events[]
//! .classification_source` did not list `declared_document_types` or
//! `declared_app_category`, which Rust has emitted since protocol 30.
//!
//! This file drives the real router, abstraction engine and SQLite store,
//! sends requests exactly as the Swift client does, and validates what would
//! cross the socket.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, NaiveTime, Utc};
use serde_json::Value;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::dashboard::DAILY_ACTIVITY_DAYS;
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{MenuStatusProvider, MessageRouter, R7Router};
use velvt_service::persistence::{RawEventEntry, SqlitePersistence};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    AcknowledgeCategoryPrompt, CategoryPromptResponse, ClientMessage, RawEvent,
    RequestCategoryPrompt, RequestCorrectionHistory, RequestLocalDashboard, RequestMenuStatus,
    RequestUnclassifiedTriage, RequestWorkBlockState, ServerMessage, SetSiteCategory,
    StartWorkBlock, WorkBlockIntensity, WorkBlockPurpose,
};

#[path = "../shared-types/tests/support/json_schema.rs"]
mod json_schema;

fn schema(file: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../proto/schema")
        .join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{file} is not JSON: {error}"))
}

fn assert_valid(file: &str, value: &Value) {
    let schema = schema(file);
    if let Err(problem) = json_schema::validate(&schema, &schema, value, "$") {
        panic!(
            "what the service emits does not match proto/schema/{file}: {problem}\n    \
             emitted: {value}"
        );
    }
}

async fn work_block_state(router: &R7Router) -> Value {
    let response = router
        .route(ClientMessage::RequestWorkBlockState(
            RequestWorkBlockState {},
        ))
        .await
        .unwrap();
    let Some(message @ ServerMessage::WorkBlockState(_)) = response else {
        panic!("request_work_block_state answers with work_block_state, got {response:?}");
    };
    serde_json::to_value(message).unwrap()
}

struct OfflineHttp;

impl HttpClient for OfflineHttp {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>>
    {
        Box::pin(async { Err(AuthError::Transport) })
    }
}

struct NullIngestor;

type IngestorFuture<'a, T> = Pin<
    Box<
        dyn std::future::Future<Output = Result<T, velvt_service::upload::CoordinatorError>>
            + Send
            + 'a,
    >,
>;

impl EventIngestor for NullIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a velvt_service::abstraction::AbstractedEvent,
        _duration_seconds: u64,
        _now: chrono::DateTime<Utc>,
    ) -> IngestorFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn flush_due<'a>(&'a self, _now: chrono::DateTime<Utc>) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_shutdown<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_now<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
}

fn router() -> R7Router {
    router_over(&SqlitePersistence::open_in_memory().unwrap())
}

/// The router over `persistence`, with the menu status the service wires in
/// production and the correction store, so a test can plant rows and read
/// them back through the real provider.
fn router_over(persistence: &SqlitePersistence) -> R7Router {
    let push = PushAdapter::new(PushQueue::new(50));
    let work_blocks = Arc::new(WorkBlockManager::new(persistence.work_block_repo()));
    let abstraction_engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    R7Router::new(
        Arc::new(FakeCacheManager::new()),
        abstraction_engine,
        persistence.raw_event_repo(),
        Arc::new(NullIngestor) as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(work_blocks, push)
    .with_classification_corrections(
        persistence.abstraction_map_repo(),
        persistence.upload_batch_repo(),
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
    )
    .with_menu_status(Arc::new(MenuStatusProvider::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        persistence.upload_batch_repo(),
        persistence.raw_event_repo(),
        persistence.abstraction_map_repo(),
    )))
}

fn raw_event(
    at: chrono::DateTime<Utc>,
    duration_seconds: u64,
    app_name: &str,
    window_title: &str,
) -> ClientMessage {
    ClientMessage::RawEvent(RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: at,
        duration_seconds,
        app_name: app_name.into(),
        window_title: window_title.into(),
        bundle_id: None,
        declared_app_category: None,
        document_type_ids: Vec::new(),
        focused_document_url: None,
        in_progress: false,
    })
}

#[tokio::test]
async fn the_emitted_local_dashboard_validates_against_its_schema() {
    let router = router();
    let now = Utc::now();

    // Evidence in the last hour, and at 02:00 UTC on yesterday and on the
    // oldest day of the window, so the rows carry real segments, labels and
    // percentages rather than only the empty shape. The request below uses a
    // zero UTC offset, so those two days are exactly rows 12 and 0.
    let early_on = |days_ago: i64| {
        (now.date_naive() - ChronoDuration::days(days_ago))
            .and_time(NaiveTime::from_hms_opt(2, 0, 0).unwrap())
            .and_utc()
    };
    for base in [
        now - ChronoDuration::minutes(50),
        early_on(1),
        early_on(DAILY_ACTIVITY_DAYS - 1),
    ] {
        for (offset_minutes, duration, app, title) in [
            (0, 600, "Xcode", "LocalDashboardSchema.swift"),
            (10, 120, "Slack", "general"),
            (12, 300, "Safari", "Rust reference"),
            (17, 900, "Xcode", "LocalDashboardSchema.swift"),
        ] {
            router
                .route(raw_event(
                    base + ChronoDuration::minutes(offset_minutes),
                    duration,
                    app,
                    title,
                ))
                .await
                .unwrap();
        }
    }

    // An active block, so `focus_fragmentation` is an object and not null.
    let started = router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: 3_600,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    assert!(
        matches!(started, Some(ServerMessage::WorkBlockState(_))),
        "the block starts"
    );

    let response = router
        .route(ClientMessage::RequestLocalDashboard(
            RequestLocalDashboard {
                window_seconds: 3_600,
                utc_offset_seconds: 0,
            },
        ))
        .await
        .unwrap();
    let Some(ServerMessage::LocalDashboard(snapshot)) = response else {
        panic!("request_local_dashboard answers with local_dashboard, got {response:?}");
    };
    let encoded = serde_json::to_value(ServerMessage::LocalDashboard(snapshot)).unwrap();
    assert_eq!(encoded["type"], "local_dashboard");
    let payload = &encoded["payload"];

    let days = payload["daily_activity"]
        .as_array()
        .expect("daily_activity is an array");
    assert_eq!(days.len() as i64, DAILY_ACTIVITY_DAYS);
    for row in [0, days.len() - 2] {
        assert!(
            !days[row]["segments"].as_array().unwrap().is_empty(),
            "the fixture should put segments on row {row}: {payload}"
        );
    }
    assert!(
        payload["focus_fragmentation"].is_object(),
        "an active block should produce focus_fragmentation: {payload}"
    );

    // `local_dashboard.json` describes the payload only.
    assert_valid("local_dashboard.json", payload);
}

/// `work_block_state` with no block and with an active one. Neither has an
/// unanswered offer, so `active_intervention` is absent from both, which the
/// schema must allow.
#[tokio::test]
async fn the_emitted_work_block_state_validates_against_its_schema() {
    let router = router();

    let idle = work_block_state(&router).await;
    assert_eq!(idle["payload"]["phase"], "idle");
    assert!(idle["payload"].get("active_intervention").is_none());
    assert_valid("work_block_state.json", &idle);

    router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: Some("Check the schema against the wire".into()),
            planned_duration_seconds: 1_800,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    router
        .route(raw_event(
            Utc::now(),
            0,
            "Xcode",
            "EmittedPayloadSchema.swift",
        ))
        .await
        .unwrap();
    let active = work_block_state(&router).await;
    assert_eq!(active["payload"]["phase"], "active");
    assert_valid("work_block_state.json", &active);
}

/// The schema's row count and the constant that decides it are one number.
/// Changing `DAILY_ACTIVITY_DAYS` without the schema, or the schema without
/// the constant, fails here with both values named.
#[test]
fn the_schema_row_count_is_daily_activity_days() {
    let schema = schema("local_dashboard.json");
    let daily_activity = &schema["properties"]["daily_activity"];
    for bound in ["minItems", "maxItems"] {
        assert_eq!(
            daily_activity[bound].as_i64(),
            Some(DAILY_ACTIVITY_DAYS),
            "proto/schema/local_dashboard.json daily_activity.{bound} must equal \
             dashboard::DAILY_ACTIVITY_DAYS ({DAILY_ACTIVITY_DAYS}); the service sends exactly \
             that many rows and the shaper rejects any other count"
        );
    }
}

/// One queued event per classification source the service can store, read
/// back through the real menu-status provider. The two declared-metadata
/// sources are the ones the schema was missing; an app rule puts a row in
/// `correction_history` too.
#[tokio::test]
async fn the_emitted_menu_status_validates_against_its_schema() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let router = router_over(&persistence);
    let events = persistence.raw_event_repo();
    let sources = [
        "seed",
        "heuristic",
        "embedding",
        "user_rule",
        "declared_document_types",
        "declared_app_category",
        "fallback",
    ];
    let now = Utc::now();
    for (index, source) in sources.iter().enumerate() {
        events
            .insert(&RawEventEntry {
                event_id: uuid::Uuid::new_v4().to_string(),
                stable_id: format!("abs_{index}"),
                label: "document:inferred".into(),
                local_display_label: None,
                local_name_suggestion: None,
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-2".into(),
                classification_tier: "local_purpose_heuristic".into(),
                classification_status: "classified".into(),
                classification_confidence: "medium".into(),
                classification_source: (*source).into(),
                occurred_at: now - ChronoDuration::minutes(index as i64),
                duration_seconds: 60,
                upload_eligible: true,
                app_stable_id: None,
                app_scope_eligible: true,
                site_stable_id: None,
            })
            .unwrap();
    }
    persistence
        .abstraction_map_repo()
        .save_app_scope_override(&"a".repeat(64), None, "REFERENCE", Some("Qwybex"))
        .unwrap();
    // A site rule too (protocol 33), which the history lists as scope `site`.
    persistence
        .abstraction_map_repo()
        .save_site_scope_override(&"b".repeat(64), "FOCUS_WORK", None)
        .unwrap();

    let response = router
        .route(ClientMessage::RequestMenuStatus(RequestMenuStatus {}))
        .await
        .unwrap();
    let Some(message @ ServerMessage::MenuStatus(_)) = response else {
        panic!("request_menu_status answers with menu_status, got {response:?}");
    };
    let encoded = serde_json::to_value(message).unwrap();
    let emitted: Vec<&str> = encoded["payload"]["queued_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["classification_source"].as_str().unwrap())
        .collect();
    for source in sources {
        assert!(
            emitted.contains(&source),
            "{source} was not emitted: {encoded}"
        );
    }
    let scopes: Vec<&str> = encoded["payload"]["correction_history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["scope"].as_str().unwrap())
        .collect();
    assert!(scopes.contains(&"app"), "{encoded}");
    assert!(scopes.contains(&"site"), "{encoded}");
    assert_valid("menu_status.json", &encoded);
}

fn unlogged_event(
    stable_id: &str,
    seconds: u64,
    app_key: &str,
    name: Option<&str>,
) -> RawEventEntry {
    RawEventEntry {
        event_id: uuid::Uuid::new_v4().to_string(),
        stable_id: stable_id.into(),
        label: "unlogged".into(),
        local_display_label: None,
        local_name_suggestion: name.map(str::to_owned),
        category: "UNLOGGED".into(),
        taxonomy_version: "mvp-2".into(),
        classification_tier: "fallback".into(),
        classification_status: "unclassified".into(),
        classification_confidence: "none".into(),
        classification_source: "fallback".into(),
        occurred_at: Utc::now() - ChronoDuration::minutes(5),
        duration_seconds: seconds,
        upload_eligible: false,
        app_stable_id: Some(app_key.into()),
        app_scope_eligible: true,
        site_stable_id: None,
    }
}

/// The needs-a-category list as protocol 33 sends it: an application with a
/// name, one without (`display_name: null`), and a site, on one list; and the
/// history page with a rule of each scope.
#[tokio::test]
async fn the_emitted_needs_a_category_list_and_history_validate_against_their_schemas() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let router = router_over(&persistence);
    let events = persistence.raw_event_repo();
    events
        .insert(&unlogged_event(
            "abs_named",
            1_200,
            &"1".repeat(64),
            Some("Qwybex"),
        ))
        .unwrap();
    events
        .insert(&unlogged_event("abs_nameless", 900, &"2".repeat(64), None))
        .unwrap();
    router
        .route(ClientMessage::RawEvent(RawEvent {
            event_id: uuid::Uuid::new_v4(),
            occurred_at: Utc::now() - ChronoDuration::minutes(3),
            duration_seconds: 600,
            app_name: "Safari".into(),
            window_title: "Zarniwoop".into(),
            bundle_id: None,
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: Some("https://qwybex-forum.example/t/1".into()),
            in_progress: false,
        }))
        .await
        .unwrap();

    let response = router
        .route(ClientMessage::RequestUnclassifiedTriage(
            RequestUnclassifiedTriage { lookback_days: 7 },
        ))
        .await
        .unwrap();
    let Some(message @ ServerMessage::UnclassifiedTriage(_)) = response else {
        panic!("request_unclassified_triage answers with unclassified_triage, got {response:?}");
    };
    let encoded = serde_json::to_value(message).unwrap();
    let entries = encoded["payload"]["entries"].as_array().unwrap();
    let kinds: Vec<(&str, &Value)> = entries
        .iter()
        .map(|entry| (entry["kind"].as_str().unwrap(), &entry["display_name"]))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("application", &Value::from("Qwybex")),
            ("application", &Value::Null),
            ("site", &Value::from("qwybex-forum.example")),
        ],
        "{encoded}"
    );
    assert_valid("unclassified_triage.json", &encoded);

    let rules = persistence.abstraction_map_repo();
    rules
        .save_app_scope_override(&"1".repeat(64), None, "REFERENCE", None)
        .unwrap();
    let site = entries[2]["stable_id"].as_str().unwrap().to_owned();
    router
        .route(ClientMessage::SetSiteCategory(SetSiteCategory {
            site_stable_id: site,
            category: "FOCUS_WORK".into(),
            activity_name: None,
        }))
        .await
        .unwrap();
    let response = router
        .route(ClientMessage::RequestCorrectionHistory(
            RequestCorrectionHistory {
                query: None,
                offset: 0,
                page_size: 20,
            },
        ))
        .await
        .unwrap();
    let Some(ServerMessage::CorrectionHistoryPage(page)) = response else {
        panic!("request_correction_history answers with a page, got {response:?}");
    };
    // `correction_history_page.json` describes the payload alone.
    let encoded = serde_json::to_value(page).unwrap();
    let scopes: Vec<&str> = encoded["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["scope"].as_str().unwrap())
        .collect();
    assert!(
        scopes.contains(&"app") && scopes.contains(&"site"),
        "{encoded}"
    );
    assert_valid("correction_history_page.json", &encoded);
}

/// The confirmation a site teach answers with stays inside `menu_status`'s
/// 200 characters whatever the site: it names the site by the name typed with
/// it, at most 48 characters, or as "this site", and never by its hostname,
/// which can be 253.
#[tokio::test]
async fn the_menu_status_a_site_teach_emits_validates_against_its_schema() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let router = router_over(&persistence);
    let host = format!(
        "{}.{}.{}.{}.example",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(53)
    );
    assert_eq!(host.len(), 253);
    router
        .route(ClientMessage::RawEvent(RawEvent {
            event_id: uuid::Uuid::new_v4(),
            occurred_at: Utc::now() - ChronoDuration::minutes(3),
            duration_seconds: 600,
            app_name: "Safari".into(),
            window_title: "Zarniwoop".into(),
            bundle_id: None,
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: Some(format!("https://{host}/t/1")),
            in_progress: false,
        }))
        .await
        .unwrap();
    let response = router
        .route(ClientMessage::RequestUnclassifiedTriage(
            RequestUnclassifiedTriage { lookback_days: 7 },
        ))
        .await
        .unwrap();
    let Some(ServerMessage::UnclassifiedTriage(triage)) = response else {
        panic!("request_unclassified_triage answers with unclassified_triage, got {response:?}");
    };
    assert_eq!(
        triage.entries[0].display_name.as_deref(),
        Some(host.as_str()),
        "the long host is on the list, so the teach below is of it"
    );
    let site = triage.entries[0].stable_id.clone();

    for (category, name) in [
        ("PASSIVE_CONSUMPTION", None),
        ("PASSIVE_CONSUMPTION", Some("x".repeat(48))),
        ("TASK_MANAGEMENT", Some("y".repeat(48))),
    ] {
        let response = router
            .route(ClientMessage::SetSiteCategory(SetSiteCategory {
                site_stable_id: site.clone(),
                category: category.into(),
                activity_name: name.clone(),
            }))
            .await
            .unwrap();
        let Some(message @ ServerMessage::MenuStatus(_)) = response else {
            panic!("set_site_category answers with menu_status, got {response:?}");
        };
        let encoded = serde_json::to_value(message).unwrap();
        let acknowledgment = encoded["payload"]["correction_acknowledgment"]
            .as_str()
            .unwrap();
        assert!(acknowledgment.chars().count() <= 200, "{acknowledgment:?}");
        assert!(!acknowledgment.contains("aaaa"), "{acknowledgment:?}");
        assert!(acknowledgment.contains(name.as_deref().unwrap_or("this site")));
        assert_valid("menu_status.json", &encoded);
    }
}

/// Delivery gates that never suppress, so the emitted prompt carries a
/// reminder as well as a card.
struct OpenGates;

impl velvt_service::initiation::InvitationGates for OpenGates {
    fn live_block_exists(&self) -> Result<bool, velvt_service::persistence::PersistenceError> {
        Ok(false)
    }

    fn in_quiet_hours(&self, _at: chrono::DateTime<Utc>) -> bool {
        false
    }

    fn in_quiet_hours_at(&self, _at: chrono::DateTime<Utc>, _utc_offset_seconds: i32) -> bool {
        false
    }

    fn focus_active(&self, _at: chrono::DateTime<Utc>) -> bool {
        false
    }
}

/// The needs-a-category prompt as the service emits it: empty with nothing on
/// the list, then a card and a reminder, then the reply to an answer.
#[tokio::test]
async fn the_emitted_category_prompt_validates_against_its_schema() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let router = router_over(&persistence).with_category_prompt(
        velvt_service::category_prompt::CategoryPromptManager::new(
            persistence.category_prompt_repo(),
            velvt_service::category_prompt::ListedCandidates::new(persistence.raw_event_repo()),
            Arc::new(OpenGates),
        ),
    );
    let request = || {
        ClientMessage::RequestCategoryPrompt(RequestCategoryPrompt {
            utc_offset_seconds: 3_600,
        })
    };
    let emitted = |response: Option<ServerMessage>| {
        let Some(message @ ServerMessage::CategoryPrompt(_)) = response else {
            panic!("the prompt messages answer with category_prompt, got {response:?}");
        };
        serde_json::to_value(message).unwrap()
    };

    let empty = emitted(router.route(request()).await.unwrap());
    assert_eq!(empty["payload"], serde_json::json!({}));
    assert_valid("category_prompt.json", &empty);

    persistence
        .raw_event_repo()
        .insert(&unlogged_event(
            "abs_named",
            1_200,
            &"1".repeat(64),
            Some("Qwybex"),
        ))
        .unwrap();
    let full = emitted(router.route(request()).await.unwrap());
    assert!(full["payload"]["card"].is_object(), "{full}");
    assert!(full["payload"]["notification"].is_object(), "{full}");
    assert_valid("category_prompt.json", &full);

    let answered = emitted(
        router
            .route(ClientMessage::AcknowledgeCategoryPrompt(
                AcknowledgeCategoryPrompt {
                    prompt_id: full["payload"]["prompt_id"].as_str().unwrap().to_owned(),
                    response: CategoryPromptResponse::Opened,
                },
            ))
            .await
            .unwrap(),
    );
    assert_eq!(answered["payload"], serde_json::json!({}));
    assert_valid("category_prompt.json", &answered);
}
