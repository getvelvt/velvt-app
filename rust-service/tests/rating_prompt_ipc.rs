//! The question after a block over protocol 38, through the real router,
//! work-block manager and store, wired as `main.rs` wires them. Every frame
//! the service answers with is validated against `proto/schema/`.
//!
//! The router stamps each message with the wall clock, so the blocks are
//! placed in the past relative to now: the Mac's first block 20 days ago,
//! and the block asked about ending ten minutes ago.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{SelfRatingAnswer, SelfRatingRepo, SqlitePersistence};
use velvt_service::self_rating::{RatingPromptManager, QUESTION, SETTINGS_TITLE};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{RatingPromptSource, WorkBlockManager};
use velvt_shared_types::{
    ClientMessage, RateWorkBlock, RatingPromptPresentation, RatingPromptSettings,
    RequestRatingPromptSettings, RequestWorkBlockState, ServerMessage, SetRatingPromptEnabled,
    StartWorkBlock, WorkBlockIntensity, WorkBlockRatingAnswer, WorkBlockRatingNotNow,
    WorkBlockRatingShown, WorkBlockSnapshot,
};

#[path = "../shared-types/tests/support/json_schema.rs"]
mod json_schema;

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
        _now: DateTime<Utc>,
    ) -> IngestorFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn flush_due<'a>(&'a self, _now: DateTime<Utc>) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_shutdown<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_now<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
}

struct Service {
    router: R7Router,
    work_blocks: Arc<WorkBlockManager>,
    rating: Arc<RatingPromptManager>,
    ratings: Arc<dyn SelfRatingRepo>,
}

fn service(with_question: bool) -> Service {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let rating = Arc::new(RatingPromptManager::new(persistence.self_rating_repo()));
    let mut work_blocks = WorkBlockManager::new(persistence.work_block_repo());
    if with_question {
        work_blocks =
            work_blocks.with_rating_prompt(Arc::clone(&rating) as Arc<dyn RatingPromptSource>);
    }
    let work_blocks = Arc::new(work_blocks);
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    let mut router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        Arc::new(
            AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap(),
        ),
        persistence.raw_event_repo(),
        Arc::new(NullIngestor) as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(
        Arc::clone(&work_blocks),
        PushAdapter::new(PushQueue::new(50)),
    );
    if with_question {
        router = router.with_rating_prompt(Arc::clone(&rating));
    }
    Service {
        router,
        work_blocks,
        rating,
        ratings: persistence.self_rating_repo(),
    }
}

fn start(planned_seconds: u32) -> StartWorkBlock {
    StartWorkBlock {
        intention: None,
        planned_duration_seconds: planned_seconds,
        purpose: None,
        intensity: WorkBlockIntensity::Medium,
        invitation_id: None,
    }
}

/// The Mac's first block `first_days_ago`, and a 30-minute block that ended
/// ten minutes ago. Returns that block's id.
fn history(service: &Service, first_days_ago: i64, turn_on: bool) -> Uuid {
    let now = Utc::now();
    let first = now - Duration::days(first_days_ago);
    let id = service
        .work_blocks
        .start(start(1_500), first)
        .unwrap()
        .block_id
        .unwrap();
    service
        .work_blocks
        .end(id, first + Duration::minutes(20))
        .unwrap();
    if turn_on {
        assert!(
            service
                .rating
                .set_enabled(true, now - Duration::days(1))
                .unwrap()
                .enabled
        );
    }
    let block = service
        .work_blocks
        .start(start(1_800), now - Duration::minutes(40))
        .unwrap()
        .block_id
        .unwrap();
    service
        .work_blocks
        .request_state(now - Duration::minutes(10))
        .unwrap();
    block
}

fn assert_valid(file: &str, value: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../proto/schema")
        .join(file);
    let schema: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    if let Err(problem) = json_schema::validate(&schema, &schema, value, "$") {
        panic!("the service emitted a frame proto/schema/{file} refuses: {problem}\n{value}");
    }
}

async fn send(service: &Service, message: ClientMessage) -> Option<ServerMessage> {
    service.router.route(message).await.unwrap()
}

async fn snapshot_from(service: &Service, message: ClientMessage) -> WorkBlockSnapshot {
    let reply = send(service, message).await;
    let Some(reply @ ServerMessage::WorkBlockState(_)) = reply else {
        panic!("expected work_block_state, got {reply:?}");
    };
    assert_valid(
        "work_block_state.json",
        &serde_json::to_value(&reply).unwrap(),
    );
    let ServerMessage::WorkBlockState(snapshot) = reply else {
        unreachable!()
    };
    snapshot
}

async fn state(service: &Service) -> WorkBlockSnapshot {
    snapshot_from(
        service,
        ClientMessage::RequestWorkBlockState(RequestWorkBlockState {}),
    )
    .await
}

async fn settings_from(service: &Service, message: ClientMessage) -> RatingPromptSettings {
    let reply = send(service, message).await;
    let Some(reply @ ServerMessage::RatingPromptSettings(_)) = reply else {
        panic!("expected rating_prompt_settings, got {reply:?}");
    };
    assert_valid(
        "rating_prompt_settings.json",
        &serde_json::to_value(&reply).unwrap(),
    );
    let ServerMessage::RatingPromptSettings(settings) = reply else {
        unreachable!()
    };
    settings
}

/// The whole path: asked, seen, Not now, answered through the link, then
/// the setting read and turned off.
#[tokio::test]
async fn a_question_is_asked_seen_put_off_and_answered_over_the_socket() {
    let service = service(true);
    let block = history(&service, 20, true);

    let asked = state(&service).await;
    let prompt = asked.rating_prompt.expect("the ended block is asked about");
    assert_eq!(prompt.block_id, block);
    assert_eq!(prompt.question, QUESTION);
    assert_eq!(prompt.presentation, RatingPromptPresentation::Question);
    assert_eq!(asked.rating_opt_in, None);

    assert_eq!(
        send(
            &service,
            ClientMessage::WorkBlockRatingShown(WorkBlockRatingShown { block_id: block }),
        )
        .await,
        None,
        "a sighting is answered with nothing"
    );
    let put_off = snapshot_from(
        &service,
        ClientMessage::WorkBlockRatingNotNow(WorkBlockRatingNotNow { block_id: block }),
    )
    .await;
    assert_eq!(
        put_off.rating_prompt.unwrap().presentation,
        RatingPromptPresentation::Link
    );

    let answered = snapshot_from(
        &service,
        ClientMessage::RateWorkBlock(RateWorkBlock {
            block_id: block,
            answer: WorkBlockRatingAnswer::Most,
            via_link: true,
        }),
    )
    .await;
    assert_eq!(answered.rating_prompt, None);
    let row = service.ratings.rating(&block.to_string()).unwrap().unwrap();
    assert!(row.shown_at.is_some() && row.not_now_at.is_some());
    assert!(row.shown_at <= row.not_now_at && row.not_now_at <= row.answered_at);
    assert_eq!(row.answer, Some(SelfRatingAnswer::Most));
    assert!(row.card_seen_before_answer);

    let read = settings_from(
        &service,
        ClientMessage::RequestRatingPromptSettings(RequestRatingPromptSettings {}),
    )
    .await;
    assert!(read.available && read.enabled);
    assert_eq!(read.title, SETTINGS_TITLE);
    let off = settings_from(
        &service,
        ClientMessage::SetRatingPromptEnabled(SetRatingPromptEnabled { enabled: false }),
    )
    .await;
    assert!(off.available && !off.enabled);
}

/// The invitation follows the first block after day 14 until Not now, which
/// the service records so it never returns.
#[tokio::test]
async fn the_invitation_is_offered_and_its_not_now_is_kept() {
    let service = service(true);
    history(&service, 20, false);

    let offered = state(&service).await;
    assert!(offered.rating_opt_in.is_some());
    assert_eq!(offered.rating_prompt, None);

    let declined = settings_from(
        &service,
        ClientMessage::SetRatingPromptEnabled(SetRatingPromptEnabled { enabled: false }),
    )
    .await;
    assert!(declined.available && !declined.enabled);
    assert_eq!(state(&service).await.rating_opt_in, None);
    assert!(service.ratings.settings().unwrap().opted_out_at.is_some());
}

/// Inside the Mac's first 14 days the toggle is not offered, turning the
/// question on is refused, and no invitation follows a block.
#[tokio::test]
async fn inside_the_first_14_days_it_cannot_be_turned_on() {
    let service = service(true);
    history(&service, 3, false);

    let refused = settings_from(
        &service,
        ClientMessage::SetRatingPromptEnabled(SetRatingPromptEnabled { enabled: true }),
    )
    .await;
    assert!(!refused.available && !refused.enabled);
    let snapshot = state(&service).await;
    assert_eq!(snapshot.rating_opt_in, None);
    assert_eq!(snapshot.rating_prompt, None);
    assert_eq!(service.ratings.settings().unwrap().first_opted_in_at, None);
}

/// A service built without the question drops its reports and answers its
/// settings messages with nothing, and its snapshots never carry it.
#[tokio::test]
async fn without_the_question_its_messages_change_nothing() {
    let service = service(false);
    let block = history(&service, 20, true);
    for message in [
        ClientMessage::WorkBlockRatingShown(WorkBlockRatingShown { block_id: block }),
        ClientMessage::WorkBlockRatingNotNow(WorkBlockRatingNotNow { block_id: block }),
        ClientMessage::RateWorkBlock(RateWorkBlock {
            block_id: block,
            answer: WorkBlockRatingAnswer::All,
            via_link: false,
        }),
        ClientMessage::SetRatingPromptEnabled(SetRatingPromptEnabled { enabled: false }),
        ClientMessage::RequestRatingPromptSettings(RequestRatingPromptSettings {}),
    ] {
        assert_eq!(send(&service, message).await, None);
    }
    let snapshot = state(&service).await;
    assert_eq!(snapshot.rating_prompt, None);
    assert_eq!(snapshot.rating_opt_in, None);
    assert!(service
        .ratings
        .rating(&block.to_string())
        .unwrap()
        .is_none());
}
