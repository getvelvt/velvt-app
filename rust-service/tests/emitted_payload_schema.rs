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
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    ClientMessage, RawEvent, RequestLocalDashboard, RequestWorkBlockState, ServerMessage,
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
    let persistence = SqlitePersistence::open_in_memory().unwrap();
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
