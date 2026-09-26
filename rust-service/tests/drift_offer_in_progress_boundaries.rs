//! Drift decisions across block boundaries, with and without the protocol-32
//! in-progress report.
//!
//! `drift_offer_in_progress.rs` shows that on a timeline with no boundary the
//! in-progress report moves only the wall-clock moment of the decision. These
//! timelines add the boundaries a real block has (a pause, a sleep, a service
//! restart, the block running out) and send each one twice: closed-only, as a
//! protocol-31 client did, and with in-progress reports. Both go through the
//! real router, abstraction engine, work-block manager and persistence.
//! Stamps are seconds after the block started.
//!
//! What they pin:
//! - The ledger, coverage and the upload queue never depend on the in-progress
//!   report, and neither do the spans except where noted.
//! - A boundary between a dwell's two reports never makes the gate decide on
//!   that dwell twice.
//! - The decisions that do change, which is why this is drift policy
//!   version 3: a departure is decided on when it begins, including one still
//!   in progress when the block ends or the Mac sleeps, which a closed-only
//!   client never reported in time.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use velvt_service::abstraction::{AbstractedEvent, AbstractionEngine};
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{RawEventRepo, SqlitePersistence};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    ClientMessage, RawEvent, ServerMessage, StartWorkBlock, WorkBlockIntensity, WorkBlockPurpose,
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

type IngestorFuture<'a, T> = Pin<
    Box<
        dyn std::future::Future<Output = Result<T, velvt_service::upload::CoordinatorError>>
            + Send
            + 'a,
    >,
>;

#[derive(Default)]
struct CountingIngestor(AtomicUsize);

impl EventIngestor for CountingIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a AbstractedEvent,
        _duration_seconds: u64,
        _now: DateTime<Utc>,
    ) -> IngestorFuture<'a, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
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

/// One run of the service over a shared database. A restart replaces it,
/// and with it the router's memory of in-progress reports.
struct Service {
    router: R7Router,
    manager: Arc<WorkBlockManager>,
    pushes: Arc<PushQueue>,
    _auth: tokio::sync::watch::Sender<AuthState>,
}

fn service(persistence: &SqlitePersistence, ingested: &Arc<CountingIngestor>) -> Service {
    let pushes = PushQueue::new(256);
    let manager = Arc::new(WorkBlockManager::new(persistence.work_block_repo()));
    let engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    // Signed in, so every closed report is upload eligible and the count of
    // what reached the queue means something.
    let (auth, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: "device".into(),
    });
    let router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        engine,
        persistence.raw_event_repo(),
        ingested.clone() as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(manager.clone(), PushAdapter::new(pushes.clone()))
    .with_auth_state(auth_state);
    Service {
        router,
        manager,
        pushes,
        _auth: auth,
    }
}

#[derive(Clone, Copy, Debug)]
enum Step {
    /// A dwell in `app` begins at this second, and the one before it ends.
    /// The client sends the closed report of the one before, then, with
    /// in-progress reports, this one in progress.
    Switch(&'static str, i64),
    Pause(i64),
    Resume(i64),
    /// The service restarts: a new router, and recovery runs at this second.
    Restart(i64),
    /// The service looks at the block at this second, as the deadline
    /// scheduler does; past the deadline this finishes it.
    Tick(i64),
    /// The client stops collecting, as it does when the Mac sleeps: the
    /// current dwell is sent closed, measured to here, and none begins.
    StopCollection(i64),
}

#[derive(Debug, PartialEq)]
struct Outcome {
    decisions: Vec<(i64, String, u32)>,
    offer: Option<(i64, String, Option<i64>)>,
    spans: Vec<(i64, i64, String)>,
    result: Option<(u32, u32, u32, String, Option<String>)>,
    ledger_rows: usize,
    ingested: usize,
}

/// Runs `steps` against a fresh database and returns what was decided and
/// measured, and every work-block push as (the step's second, whether it
/// carried a live offer).
async fn run(steps: &[Step], planned: u32, in_progress: bool) -> (Outcome, Vec<(i64, bool)>) {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ingested = Arc::new(CountingIngestor::default());
    let mut svc = service(&persistence, &ingested);
    let blocks = persistence.work_block_repo();
    let raw: Arc<dyn RawEventRepo> = persistence.raw_event_repo();
    let started = svc
        .router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: planned,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(started)) = started else {
        panic!()
    };
    let t0 = started.started_at.unwrap();
    let block_id = started.block_id.unwrap();
    while svc.pushes.try_pop().await.is_some() {}
    let at = |s: i64| t0 + ChronoDuration::seconds(s);
    let mut current: Option<(&'static str, i64)> = None;
    let mut pushes = Vec::new();
    let event = |app: &str, from: i64, until: Option<i64>| RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: at(from),
        duration_seconds: until.map_or(0, |u| u64::try_from(u - from).unwrap()),
        app_name: app.into(),
        window_title: format!("window {from}"),
        bundle_id: None,
        declared_app_category: None,
        document_type_ids: Vec::new(),
        focused_document_url: None,
        in_progress: until.is_none(),
    };
    for step in steps {
        match *step {
            Step::Switch(app, s) => {
                if let Some((prev, from)) = current {
                    svc.router
                        .route(ClientMessage::RawEvent(event(prev, from, Some(s))))
                        .await
                        .unwrap();
                }
                if in_progress {
                    svc.router
                        .route(ClientMessage::RawEvent(event(app, s, None)))
                        .await
                        .unwrap();
                }
                current = Some((app, s));
            }
            Step::StopCollection(s) => {
                if let Some((prev, from)) = current.take() {
                    svc.router
                        .route(ClientMessage::RawEvent(event(prev, from, Some(s))))
                        .await
                        .unwrap();
                }
            }
            Step::Pause(s) => {
                svc.manager.pause(block_id, at(s)).unwrap();
            }
            Step::Resume(s) => {
                svc.manager.resume(block_id, at(s)).unwrap();
            }
            Step::Restart(s) => {
                svc = service(&persistence, &ingested);
                svc.manager.recover_after_restart(at(s)).unwrap();
            }
            Step::Tick(s) => {
                svc.manager.request_state(at(s)).unwrap();
            }
        }
        while let Some(message) = svc.pushes.try_pop().await {
            if let ServerMessage::WorkBlockState(snapshot) = message {
                let second = match *step {
                    Step::Switch(_, second)
                    | Step::Pause(second)
                    | Step::Resume(second)
                    | Step::Restart(second)
                    | Step::Tick(second)
                    | Step::StopCollection(second) => second,
                };
                pushes.push((second, snapshot.active_intervention.is_some()));
            }
        }
    }
    let rel = |d: DateTime<Utc>| (d - t0).num_seconds();
    let id = block_id.to_string();
    let decisions = blocks
        .decisions(&id)
        .unwrap()
        .into_iter()
        .map(|d| {
            (
                rel(d.occurred_at),
                d.gate_verdict.as_str().to_owned(),
                d.switch_count,
            )
        })
        .collect();
    let offer = blocks.intervention(&id).unwrap().map(|r| {
        (
            rel(r.offered_at),
            format!("{:?}", r.outcome),
            r.outcome_at.map(rel),
        )
    });
    let spans = blocks
        .observations(&id)
        .unwrap()
        .into_iter()
        .filter_map(|o| {
            let e = o.ended_at?;
            (e > o.occurred_at).then(|| (rel(o.occurred_at), rel(e), o.category))
        })
        .collect();
    let result = blocks.result(&id).unwrap().map(|r| {
        (
            r.switch_away_count,
            r.recovery_count,
            r.longest_uninterrupted_seconds,
            format!("{:.3}", r.coverage_ratio),
            r.safe_evidence_category,
        )
    });
    let ledger_rows = raw
        .events_between(
            t0 - ChronoDuration::hours(1),
            t0 + ChronoDuration::hours(4),
            1000,
        )
        .unwrap()
        .len();
    (
        Outcome {
            decisions,
            offer,
            spans,
            result,
            ledger_rows,
            ingested: ingested.0.load(Ordering::SeqCst),
        },
        pushes,
    )
}

const X: &str = "Xcode";
const S: &str = "Slack";

/// An anchor, then two departures inside ten minutes after the warm-up. The
/// next departure is the third, and the gate offers on it.
fn warmup() -> Vec<Step> {
    vec![
        Step::Switch(X, 1),
        Step::Switch(S, 200),
        Step::Switch(X, 215),
        Step::Switch(S, 260),
        Step::Switch(X, 275),
    ]
}

type Decision = (i64, String, u32);

/// The warm-up's decisions, the same both ways: the first anchor dwell and
/// the four dwells before the third departure.
fn warmup_decisions() -> Vec<Decision> {
    with(&[
        (0, "abstained_warmup", 0),
        (199, "abstained_min_switches", 1),
        (214, "abstained_min_switches", 1),
        (259, "abstained_min_switches", 2),
        (274, "abstained_min_switches", 2),
    ])
}

fn with(decisions: &[(i64, &str, u32)]) -> Vec<Decision> {
    decisions
        .iter()
        .map(|(at, verdict, switches)| (*at, (*verdict).to_owned(), *switches))
        .collect()
}

fn with_warmup(rest: &[(i64, &str, u32)]) -> Vec<Decision> {
    let mut decisions = warmup_decisions();
    decisions.extend(with(rest));
    decisions
}

fn offer(at: i64, outcome: &str, outcome_at: i64) -> Option<(i64, String, Option<i64>)> {
    Some((at, outcome.to_owned(), Some(outcome_at)))
}

/// Everything measured rather than decided is the same both ways.
fn assert_same_evidence(closed: &Outcome, live: &Outcome) {
    assert_eq!(closed.spans, live.spans, "observed spans");
    assert_eq!(closed.result, live.result, "end-of-block result");
    assert_eq!(
        closed.ledger_rows, live.ledger_rows,
        "raw_event_buffer rows"
    );
    assert_eq!(
        closed.ingested, live.ingested,
        "events reaching the upload queue"
    );
}

/// The Mac's timeline, run to the end of the block: the only change is when
/// the offer reaches the person.
#[tokio::test]
async fn without_a_boundary_only_the_moment_of_the_offer_moves() {
    let mut steps = warmup();
    steps.extend([
        Step::Switch(S, 320),
        Step::Switch(X, 368),
        Step::Switch(X, 369),
        Step::Tick(1600),
    ]);
    let (closed, closed_pushes) = run(&steps, 1500, false).await;
    let (live, live_pushes) = run(&steps, 1500, true).await;
    assert_eq!(closed, live);
    assert_eq!(live.offer, offer(319, "Returned", 367));
    assert!(closed_pushes.contains(&(368, true)) && !closed_pushes.contains(&(320, true)));
    assert!(live_pushes.contains(&(320, true)) && !live_pushes.contains(&(368, true)));
}

/// A pause while the person is away. The in-progress report decided on the
/// away dwell when it began; its closed report, after the resume, re-opens
/// the ledger and decides nothing. A closed-only client reported that dwell
/// only after the resume, when the window had moved on, so it never offered.
#[tokio::test]
async fn a_pause_between_a_dwells_two_reports_does_not_decide_on_it_twice() {
    let mut steps = warmup();
    steps.extend([
        Step::Switch(S, 320),
        Step::Pause(330),
        Step::Resume(1000),
        Step::Switch(X, 1050),
        Step::Switch(S, 1100),
        Step::Tick(3000),
    ]);
    let (closed, _) = run(&steps, 1500, false).await;
    let (live, _) = run(&steps, 1500, true).await;
    assert_same_evidence(&closed, &live);
    assert_eq!(
        closed.decisions,
        with_warmup(&[
            (999, "abstained_min_switches", 1),
            (1049, "abstained_min_switches", 1),
        ])
    );
    assert_eq!(closed.offer, None);
    assert_eq!(
        live.decisions,
        with_warmup(&[
            (319, "offered", 3),
            (1049, "abstained_block_cap", 0),
            (1099, "abstained_block_cap", 0),
        ]),
        "nothing is decided at the resume, 999"
    );
    assert_eq!(live.offer, offer(319, "Returned", 1049));
}

/// A service restart while the person is away, and one during an anchor
/// dwell. The router's memory of the in-progress report is gone after a
/// restart, so the database alone has to recognise the closed report as the
/// dwell already decided on.
#[tokio::test]
async fn a_restart_between_a_dwells_two_reports_does_not_decide_on_it_twice() {
    let mut away = warmup();
    away.extend([
        Step::Switch(S, 320),
        Step::Restart(340),
        Step::Switch(X, 400),
        Step::Switch(S, 450),
        Step::Tick(1600),
    ]);
    let (closed, _) = run(&away, 1500, false).await;
    let (live, _) = run(&away, 1500, true).await;
    assert_same_evidence(&closed, &live);
    assert_eq!(
        closed.decisions,
        with_warmup(&[(339, "offered", 3), (399, "abstained_block_cap", 0)])
    );
    assert_eq!(
        live.decisions,
        with_warmup(&[
            (319, "offered", 3),
            (399, "abstained_block_cap", 0),
            (449, "abstained_block_cap", 0),
        ]),
        "nothing is decided at the restart, 339"
    );
    assert_eq!(live.offer, offer(319, "Returned", 399));

    let mut anchor = warmup();
    anchor.extend([
        Step::Restart(290),
        Step::Switch(S, 320),
        Step::Switch(X, 368),
        Step::Tick(1600),
    ]);
    let (closed, _) = run(&anchor, 1500, false).await;
    let (live, _) = run(&anchor, 1500, true).await;
    assert_same_evidence(&closed, &live);
    assert_eq!(
        closed.decisions,
        with(&[
            (0, "abstained_warmup", 0),
            (199, "abstained_min_switches", 1),
            (214, "abstained_min_switches", 1),
            (259, "abstained_min_switches", 2),
            (289, "abstained_min_switches", 2),
            (319, "offered", 3),
        ])
    );
    assert_eq!(
        live.decisions,
        with_warmup(&[(319, "offered", 3), (367, "abstained_block_cap", 0)]),
        "the anchor dwell is decided once, at 274, and not again at the restart"
    );
}

/// Sleep pauses the block, and the client closes the dwell it was in; the two
/// reach the service in either order. With the in-progress report the away
/// dwell is decided on when it began, whichever arrives first. Closed-only,
/// it was decided on only when its closed report won the race, and that offer
/// was pushed as the Mac went to sleep.
#[tokio::test]
async fn sleep_either_side_of_the_closed_report_decides_on_the_dwell_once() {
    for pause_first in [true, false] {
        let mut steps = warmup();
        steps.push(Step::Switch(S, 320));
        if pause_first {
            steps.extend([Step::Pause(330), Step::StopCollection(330)]);
        } else {
            steps.extend([Step::StopCollection(330), Step::Pause(330)]);
        }
        steps.extend([Step::Resume(900), Step::Switch(X, 905), Step::Tick(3000)]);
        let (closed, closed_pushes) = run(&steps, 1500, false).await;
        let (live, live_pushes) = run(&steps, 1500, true).await;
        assert_same_evidence(&closed, &live);
        if pause_first {
            assert_eq!(closed.decisions, warmup_decisions());
            assert_eq!(closed.offer, None);
        } else {
            assert_eq!(closed.decisions, with_warmup(&[(319, "offered", 3)]));
            assert!(closed_pushes.contains(&(330, true)));
        }
        assert_eq!(
            live.decisions,
            with_warmup(&[(319, "offered", 3), (904, "abstained_block_cap", 0)]),
            "pause first: {pause_first}"
        );
        assert!(live_pushes.contains(&(320, true)));
        assert_eq!(live.offer, offer(319, "Returned", 904));
    }
}

/// The case the fix exists for, and one reason this is drift policy version
/// 3: the person leaves and is still away when the block ends. Closed-only,
/// the away dwell's report arrived after the block had ended, so the gate
/// never saw the departure and made no offer. With the in-progress report it
/// is decided on, and offered, when it begins.
#[tokio::test]
async fn a_departure_still_in_progress_when_the_block_ends_is_decided_on() {
    let mut steps = warmup();
    steps.extend([Step::Switch(S, 320), Step::Tick(1600)]);
    let (closed, _) = run(&steps, 1500, false).await;
    let (live, live_pushes) = run(&steps, 1500, true).await;
    assert_same_evidence(&closed, &live);
    assert_eq!(closed.decisions, warmup_decisions());
    assert_eq!(closed.offer, None);
    assert_eq!(live.decisions, with_warmup(&[(319, "offered", 3)]));
    assert_eq!(live.offer, offer(319, "NoResponse", 1499));
    assert!(live_pushes.contains(&(320, true)));
}

/// Forty switches one to three seconds apart: the same decisions, spans and
/// ledger both ways.
#[tokio::test]
async fn rapid_switching_is_decided_the_same_both_ways() {
    let mut steps = vec![Step::Switch(X, 1)];
    let mut at = 190;
    for index in 0..40 {
        steps.push(Step::Switch(if index % 2 == 0 { S } else { X }, at));
        at += 1 + (index % 3);
    }
    steps.push(Step::Switch(X, at + 5));
    steps.push(Step::Tick(1600));
    let (closed, _) = run(&steps, 1500, false).await;
    let (live, _) = run(&steps, 1500, true).await;
    assert_eq!(closed, live);
    assert!(live.offer.is_some());
}

/// A forty-minute away dwell. Closed-only, the offer, stamped at the
/// departure, reached the client forty minutes later, when the person came
/// back. With the in-progress report it is pushed at the departure. Coverage,
/// the ledger and the upload queue are unchanged. The away row now ends at the
/// reported return, as it always did when a later switch was reported before
/// the block ended, instead of at the capped length of its closed report.
#[tokio::test]
async fn a_long_away_dwell_is_offered_on_when_it_begins() {
    let mut steps = warmup();
    steps.extend([
        Step::Switch(S, 320),
        Step::Switch(X, 320 + 2400),
        Step::Tick(4000),
    ]);
    let (closed, closed_pushes) = run(&steps, 3600, false).await;
    let (live, live_pushes) = run(&steps, 3600, true).await;
    assert!(closed_pushes.contains(&(2720, true)) && !closed_pushes.contains(&(320, true)));
    assert!(live_pushes.contains(&(320, true)) && !live_pushes.contains(&(2720, true)));
    assert_eq!(closed.offer, offer(319, "NoResponse", 3599));
    assert_eq!(live.offer, offer(319, "Returned", 2719));
    assert_eq!(closed.ledger_rows, live.ledger_rows);
    assert_eq!(closed.ingested, live.ingested);
    let coverage = |outcome: &Outcome| outcome.result.as_ref().map(|result| result.3.clone());
    assert_eq!(coverage(&closed), coverage(&live));
}
