//! `work_block_state.anchor_category` (protocol 31) through the real router,
//! abstraction engine, work-block manager, and persistence.
//!
//! The field exists so a local IPC client can apply the rule "defer while
//! `current_category` is the anchor" exactly instead of approximating it. It
//! must carry the broad category label and nothing else: the raw app names and
//! window titles that produced it never reach the payload.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    ClientMessage, RawEvent, RequestWorkBlockState, ServerMessage, StartWorkBlock,
    WorkBlockIntensity, WorkBlockPurpose, WorkBlockSnapshot,
};

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

fn raw_event(at: chrono::DateTime<Utc>, app_name: &str, window_title: &str) -> ClientMessage {
    ClientMessage::RawEvent(RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: at,
        duration_seconds: 0,
        app_name: app_name.into(),
        window_title: window_title.into(),
        bundle_id: None,
        declared_app_category: None,
        document_type_ids: Vec::new(),
        focused_document_url: None,
    })
}

async fn request_state(router: &R7Router) -> WorkBlockSnapshot {
    let response = router
        .route(ClientMessage::RequestWorkBlockState(
            RequestWorkBlockState {},
        ))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(snapshot)) = response else {
        panic!("request_work_block_state answers with work_block_state");
    };
    snapshot
}

const TITLE_SENTINEL: &str = "PRIVATE_TITLE_SENTINEL.swift";
const CHANNEL_SENTINEL: &str = "PRIVATE_CHANNEL_SENTINEL";

#[tokio::test]
async fn request_work_block_state_carries_the_anchor_as_a_category_label_only() {
    let router = router();
    let now = Utc::now();
    let at = |seconds: i64| now + ChronoDuration::seconds(seconds);

    let started = router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: Some("Write the anchor tests".into()),
            planned_duration_seconds: 3_600,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(started)) = started else {
        panic!("start returns work-block state");
    };
    assert_eq!(started.anchor_category, None, "no evidence, no anchor");

    router
        .route(raw_event(at(10), "Xcode", TITLE_SENTINEL))
        .await
        .unwrap();
    let focused = request_state(&router).await;
    assert_eq!(
        focused.anchor_category, None,
        "an observation that is still open is not evidence yet"
    );
    let focus_category = focused
        .current_category
        .clone()
        .expect("Xcode classifies confidently");

    router
        .route(raw_event(at(400), "Slack", CHANNEL_SENTINEL))
        .await
        .unwrap();
    let away = request_state(&router).await;
    assert_eq!(
        away.anchor_category.as_deref(),
        Some(focus_category.as_str())
    );
    assert_ne!(away.current_category, away.anchor_category);

    router
        .route(raw_event(at(420), "Xcode", TITLE_SENTINEL))
        .await
        .unwrap();
    let back = request_state(&router).await;
    assert_eq!(
        back.anchor_category.as_deref(),
        Some(focus_category.as_str())
    );
    assert_eq!(back.current_category, back.anchor_category);

    let encoded = serde_json::to_value(ServerMessage::WorkBlockState(back)).unwrap();
    assert_eq!(
        encoded["payload"]["anchor_category"].as_str(),
        Some(focus_category.as_str())
    );
    let wire = encoded.to_string();
    for forbidden in ["Xcode", "Slack", TITLE_SENTINEL, CHANNEL_SENTINEL] {
        assert!(!wire.contains(forbidden), "{forbidden} reached the payload");
    }
}
