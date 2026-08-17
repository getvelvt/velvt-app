//! Synthetic end-to-end demonstration of Focus/DND citizenship (0.1.6
//! Scope 1) through the real router, abstraction engine, work-block
//! manager, focus manager, persistence, and push queue.
//!
//! The scenario: DND turns on, a declared block drifts hard enough to clear
//! the drift gate, and the decision is held — nothing reaches the push
//! queue on any channel — then the block ends and exactly one calm
//! reconciliation line surfaces the held count. This is the automated
//! equivalent of the packaged-app manual demonstration.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{QuietHoursOfferResponse, SqlitePersistence};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{FocusStateSource, WorkBlockManager};
use velvt_shared_types::{
    ClientMessage, EndWorkBlock, FocusStateChanged, RawEvent, RespondQuietHoursOffer,
    ServerMessage, StartWorkBlock, WorkBlockDndOutcome, WorkBlockIntensity, WorkBlockPurpose,
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

struct Harness {
    router: R7Router,
    queue: Arc<PushQueue>,
    persistence: SqlitePersistence,
    focus: Arc<FocusManager>,
}

fn harness() -> Harness {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let queue = PushQueue::new(50);
    let push = PushAdapter::new(Arc::clone(&queue));
    let focus = FocusManager::new(persistence.focus_repo());
    let work_blocks = Arc::new(
        WorkBlockManager::new(persistence.work_block_repo())
            .with_focus_source(Arc::clone(&focus) as Arc<dyn FocusStateSource>),
    );
    let abstraction_engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    let router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        abstraction_engine,
        persistence.raw_event_repo(),
        Arc::new(NullIngestor) as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(Arc::clone(&work_blocks), push)
    .with_focus(Arc::clone(&focus));
    Harness {
        router,
        queue,
        persistence,
        focus,
    }
}

fn raw_event(at: chrono::DateTime<Utc>, app_name: &str, window_title: &str) -> ClientMessage {
    ClientMessage::RawEvent(RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: at,
        duration_seconds: 0,
        app_name: app_name.into(),
        window_title: window_title.into(),
        bundle_id: None,
        focused_document_url: None,
    })
}

async fn drain(queue: &Arc<PushQueue>) -> Vec<ServerMessage> {
    let mut messages = Vec::new();
    while let Some(message) = queue.try_pop().await {
        messages.push(message);
    }
    messages
}

/// Suppression then reconciliation, end to end: DND on, drift clears the
/// gate, nothing is delivered on any channel, and the ended block carries
/// the held count in one calm line.
#[tokio::test]
async fn dnd_suppression_delivers_nothing_and_reconciles_after_the_block() {
    let h = harness();
    let now = Utc::now();

    // DND turns on before the block starts.
    let response = h
        .router
        .route(ClientMessage::FocusStateChanged(FocusStateChanged {
            active: true,
            occurred_at: now,
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap();
    assert!(response.is_none(), "transitions are acknowledged silently");

    let started = h
        .router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: Some("Ship the focus citizenship scope".into()),
            planned_duration_seconds: 3_600,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(snapshot)) = started else {
        panic!("start returns work-block state");
    };
    let block_id = snapshot.block_id.expect("active block id");
    drain(&h.queue).await;

    // Anchor in focus work, then four confident departures inside the
    // ten-minute drift window while DND is active.
    let at = |seconds: i64| now + ChronoDuration::seconds(seconds);
    h.router
        .route(raw_event(at(10), "Xcode", "FocusManager.swift"))
        .await
        .unwrap();
    for (seconds, app, title) in [
        (400, "Slack", "team updates"),
        (420, "Xcode", "FocusManager.swift"),
        (440, "Slack", "team updates"),
        (460, "Xcode", "FocusManager.swift"),
        (480, "Slack", "team updates"),
        (500, "Xcode", "FocusManager.swift"),
        (520, "Slack", "team updates"),
    ] {
        h.router
            .route(raw_event(at(seconds), app, title))
            .await
            .unwrap();
    }

    // Zero alternate-channel delivery: the push queue carries no
    // notification and no snapshot with a live in-app offer.
    for message in drain(&h.queue).await {
        match message {
            ServerMessage::NotificationPayload(_) => {
                panic!("a notification was delivered against active DND")
            }
            ServerMessage::WorkBlockState(state) => assert!(
                state.active_intervention.is_none(),
                "an in-app card surfaced against active DND"
            ),
            _ => {}
        }
    }

    // The block ends; the held decision reconciles as one calm line.
    let ended = h
        .router
        .route(ClientMessage::EndWorkBlock(EndWorkBlock { block_id }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(ended)) = ended else {
        panic!("end returns work-block state");
    };
    let result = ended.result.expect("ended block has a result");
    assert_eq!(
        result.dnd_outcomes,
        vec![WorkBlockDndOutcome::DeliverySuppressedDnd]
    );
    let line = result.reconciliation.expect("one calm reconciliation line");
    assert!(line.contains("held 1 nudge"), "unexpected line: {line}");
    assert!(line.contains("delivered nothing mid-block"));
}

/// One tap on the quiet-hours offer configures Velvt's own quiet hours
/// through the router; the reply is recorded once and cannot be rewritten.
#[tokio::test]
async fn accepting_the_quiet_hours_offer_configures_velvt_quiet_hours() {
    let h = harness();
    let repo = h.persistence.focus_repo();
    repo.record_quiet_hours_trigger(1, Utc::now()).unwrap();

    let response = h
        .router
        .route(ClientMessage::RespondQuietHoursOffer(
            RespondQuietHoursOffer { accepted: true },
        ))
        .await
        .unwrap();
    assert!(response.is_none());

    let quiet_hours = repo.quiet_hours().unwrap().expect("quiet hours configured");
    assert_eq!(quiet_hours.start_local_minutes, 22 * 60);
    assert_eq!(quiet_hours.end_local_minutes, 7 * 60);
    let state = repo.quiet_hours_offer_state().unwrap().unwrap();
    assert_eq!(state.response, Some(QuietHoursOfferResponse::Accepted));

    // A later contradictory reply does not rewrite the recorded one.
    h.router
        .route(ClientMessage::RespondQuietHoursOffer(
            RespondQuietHoursOffer { accepted: false },
        ))
        .await
        .unwrap();
    let state = repo.quiet_hours_offer_state().unwrap().unwrap();
    assert_eq!(state.response, Some(QuietHoursOfferResponse::Accepted));
    let _ = &h.focus;
}
