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

use chrono::{Duration as ChronoDuration, Timelike, Utc};
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{QuietHoursOfferResponse, SqlitePersistence, VelvtQuietHours};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{FocusStateSource, WorkBlockManager};
use velvt_shared_types::{
    ClientMessage, EndWorkBlock, FocusStateChanged, InterventionSalience, RawEvent,
    RespondQuietHoursOffer, ServerMessage, StartWorkBlock, WorkBlockDndOutcome, WorkBlockIntensity,
    WorkBlockPurpose,
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
            invitation_id: None,
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

/// Velvt's own quiet hours reduce delivery on the surface that can actually
/// ring, not on a side channel.
///
/// Salience is the entire delivery instruction the client receives: `Normal`
/// rings and renders, `Quiet` renders only. So the window has to be applied
/// to the offer on the snapshot. Enforcing it by skipping a parallel
/// notification push instead left the client's real notification path — which
/// reads the snapshot and nothing else — with no knowledge of the window at
/// all, and it would have rung straight through it.
#[tokio::test]
async fn velvt_quiet_hours_mark_the_offer_quiet_on_the_snapshot_the_client_reads() {
    let h = harness();
    let now = Utc::now();

    // System Focus is off: the only thing in force is Velvt's own window,
    // configured here to cover the whole local day so the test does not
    // depend on when it runs.
    h.router
        .route(ClientMessage::FocusStateChanged(FocusStateChanged {
            active: false,
            occurred_at: now,
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap();
    // A one-hour window opening at this instant. Derived from `now` rather
    // than hard-coded so the test does not depend on the hour it runs, and
    // wide enough to still be open when the last drift event lands.
    let opens_at = now.hour() * 60 + now.minute();
    h.persistence
        .focus_repo()
        .set_quiet_hours(&VelvtQuietHours {
            start_local_minutes: opens_at,
            end_local_minutes: (opens_at + 60) % 1_440,
            rule_version: 1,
            configured_at: now,
        })
        .unwrap();

    h.router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: Some("Ship the quiet-hours path".into()),
            planned_duration_seconds: 3_600,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    drain(&h.queue).await;

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

    let mut cards = 0;
    for message in drain(&h.queue).await {
        match message {
            ServerMessage::NotificationPayload(_) => {
                panic!("a notification was delivered inside Velvt's own quiet hours")
            }
            ServerMessage::WorkBlockState(state) => {
                if let Some(offer) = state.active_intervention {
                    cards += 1;
                    assert_eq!(
                        offer.salience,
                        InterventionSalience::Quiet,
                        "quiet hours have to reach the client as reduced salience, \
                         because that is the only thing that stops it ringing"
                    );
                }
            }
            _ => {}
        }
    }
    assert!(
        cards > 0,
        "quiet hours reduce delivery, they do not cancel the offer: the in-app card still renders"
    );
}

/// The control for the case above: with no window configured, the same
/// evidence produces an offer the client is told to ring.
#[tokio::test]
async fn an_offer_outside_quiet_hours_reaches_the_client_as_normal_salience() {
    let h = harness();
    let now = Utc::now();

    h.router
        .route(ClientMessage::FocusStateChanged(FocusStateChanged {
            active: false,
            occurred_at: now,
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap();

    h.router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: Some("Ship the quiet-hours path".into()),
            planned_duration_seconds: 3_600,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    drain(&h.queue).await;

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

    let mut cards = 0;
    for message in drain(&h.queue).await {
        if let ServerMessage::WorkBlockState(state) = message {
            if let Some(offer) = state.active_intervention {
                cards += 1;
                assert_eq!(offer.salience, InterventionSalience::Normal);
            }
        }
    }
    assert!(
        cards > 0,
        "the drift gate has to clear for this to mean anything"
    );
}
