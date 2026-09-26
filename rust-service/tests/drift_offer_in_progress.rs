//! When the drift offer reaches the client, through the real router,
//! abstraction engine, work-block manager and persistence.
//!
//! Swift reports a dwell when it ends, because only then is its length known.
//! Up to protocol 31 that was the only report, so the gate learned of a
//! departure at the moment the person came back, and the offer it pushed was
//! withdrawn as `returned` by the very next report. On 2026-09-25 that was one
//! second later, and no notification was ever posted. Protocol 32 adds the
//! in-progress report: the same dwell, at its start.
//!
//! The timeline is the founder's, compressed: an anchor app, three short
//! departures inside ten minutes after the three-minute warm-up, and a return.

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
use velvt_service::persistence::{
    GateVerdict, RawEventRepo, SqlitePersistence, WorkBlockInterventionOutcome, WorkBlockRepo,
};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    ClientMessage, RawEvent, RawEventAck, RawEventStatus, ServerMessage, StartWorkBlock,
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

type IngestorFuture<'a, T> = Pin<
    Box<
        dyn std::future::Future<Output = Result<T, velvt_service::upload::CoordinatorError>>
            + Send
            + 'a,
    >,
>;

/// Counts what reaches the upload queue, which is the one thing an
/// in-progress report must never do.
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

struct Harness {
    router: R7Router,
    pushes: Arc<PushQueue>,
    blocks: Arc<dyn WorkBlockRepo>,
    raw_events: Arc<dyn RawEventRepo>,
    ingested: Arc<CountingIngestor>,
    _auth: tokio::sync::watch::Sender<AuthState>,
    _persistence: SqlitePersistence,
}

fn harness() -> Harness {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let pushes = PushQueue::new(64);
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
    let ingested = Arc::new(CountingIngestor::default());
    // Signed in, so every closed report is upload eligible and the count of
    // what reached the queue means something.
    let (auth, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: "device".into(),
    });
    let router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        abstraction_engine,
        persistence.raw_event_repo(),
        ingested.clone() as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(work_blocks, PushAdapter::new(pushes.clone()))
    .with_auth_state(auth_state);
    Harness {
        router,
        pushes,
        blocks: persistence.work_block_repo(),
        raw_events: persistence.raw_event_repo(),
        ingested,
        _auth: auth,
        _persistence: persistence,
    }
}

const ANCHOR_APP: &str = "Xcode";
const AWAY_APP: &str = "Slack";

/// One dwell: the app, and the seconds after the block started at which it
/// began and ended.
struct Dwell {
    app: &'static str,
    from: i64,
    until: i64,
}

/// Three departures from the anchor after the three-minute warm-up, all
/// inside ten minutes, then a return. The third departure is the one the gate
/// offers on; the last dwell is the return, still in progress when the
/// sequence stops.
const TIMELINE: [Dwell; 8] = [
    Dwell {
        app: ANCHOR_APP,
        from: 10,
        until: 200,
    },
    Dwell {
        app: AWAY_APP,
        from: 200,
        until: 215,
    },
    Dwell {
        app: ANCHOR_APP,
        from: 215,
        until: 260,
    },
    Dwell {
        app: AWAY_APP,
        from: 260,
        until: 275,
    },
    Dwell {
        app: ANCHOR_APP,
        from: 275,
        until: 320,
    },
    Dwell {
        app: AWAY_APP,
        from: 320,
        until: 368,
    },
    Dwell {
        app: ANCHOR_APP,
        from: 368,
        until: 369,
    },
    Dwell {
        app: ANCHOR_APP,
        from: 369,
        until: 400,
    },
];
/// Index in `TIMELINE` of the departure the gate offers on.
const OFFER_DWELL: usize = 5;

fn report(started_at: DateTime<Utc>, dwell: &Dwell, in_progress: bool) -> ClientMessage {
    ClientMessage::RawEvent(RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: started_at + ChronoDuration::seconds(dwell.from),
        duration_seconds: if in_progress {
            0
        } else {
            u64::try_from(dwell.until - dwell.from).unwrap()
        },
        app_name: dwell.app.into(),
        // A new title per dwell, as switching windows produces, so no two
        // consecutive dwells are the same activity to the client.
        window_title: format!("window {}", dwell.from),
        bundle_id: None,
        declared_app_category: None,
        document_type_ids: Vec::new(),
        focused_document_url: None,
        in_progress,
    })
}

/// Which report, in wire order, a message was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    InProgress(usize),
    Closed(usize),
}

/// The reports a client sends for `TIMELINE`, in the order it sends them.
///
/// A protocol-31 client sends each dwell once, closed, when the next one
/// begins. A protocol-32 client also reports each dwell in progress when it
/// begins, right after the closed report of the one before it.
fn wire_order(with_in_progress: bool) -> Vec<Report> {
    let mut order = Vec::new();
    for index in 0..TIMELINE.len() {
        if index > 0 {
            order.push(Report::Closed(index - 1));
        }
        if with_in_progress {
            order.push(Report::InProgress(index));
        }
    }
    order
}

async fn start_block(router: &R7Router) -> WorkBlockSnapshot {
    let started = router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: 25 * 60,
            purpose: Some(WorkBlockPurpose::DeepWork),
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(started)) = started else {
        panic!("start returns work-block state");
    };
    started
}

/// Every work-block push produced by one report, oldest first.
async fn drain_pushes(pushes: &PushQueue) -> Vec<WorkBlockSnapshot> {
    let mut snapshots = Vec::new();
    while let Some(message) = pushes.try_pop().await {
        if let ServerMessage::WorkBlockState(snapshot) = message {
            snapshots.push(snapshot);
        }
    }
    snapshots
}

/// Sends `order` and returns, for each report, the work-block pushes it
/// caused.
async fn replay(
    harness: &Harness,
    order: &[Report],
) -> (WorkBlockSnapshot, Vec<(Report, Vec<WorkBlockSnapshot>)>) {
    let started = start_block(&harness.router).await;
    drain_pushes(&harness.pushes).await;
    let started_at = started.started_at.expect("a started block has a start");
    let mut pushed = Vec::new();
    for report_kind in order {
        let (index, in_progress) = match *report_kind {
            Report::InProgress(index) => (index, true),
            Report::Closed(index) => (index, false),
        };
        let ack = harness
            .router
            .route(report(started_at, &TIMELINE[index], in_progress))
            .await
            .unwrap();
        assert!(
            matches!(
                ack,
                Some(ServerMessage::RawEventAck(RawEventAck {
                    status: RawEventStatus::Accepted,
                    ..
                }))
            ),
            "{report_kind:?} was not accepted: {ack:?}"
        );
        pushed.push((*report_kind, drain_pushes(&harness.pushes).await));
    }
    (started, pushed)
}

/// The reports whose pushes carried a live offer, and the first report whose
/// push withdrew it again.
fn offer_window(pushed: &[(Report, Vec<WorkBlockSnapshot>)]) -> (Vec<Report>, Option<Report>) {
    let mut live = Vec::new();
    let mut withdrawn = None;
    for (report_kind, snapshots) in pushed {
        for snapshot in snapshots {
            if snapshot.active_intervention.is_some() {
                live.push(*report_kind);
            } else if !live.is_empty() && withdrawn.is_none() {
                withdrawn = Some(*report_kind);
            }
        }
    }
    (live, withdrawn)
}

/// What the founder's Mac received on 2026-09-25, reproduced: with closed
/// reports only, the offer is pushed by the closed report of the away dwell.
/// That report is sent when the person leaves the away app, and in this
/// timeline, as on the Mac, they leave it for the anchor. The closed report
/// of that one-second anchor dwell withdraws the offer before a person could
/// read it.
#[tokio::test]
async fn closed_reports_alone_push_the_offer_only_after_the_person_is_back() {
    let harness = harness();
    let (_, pushed) = replay(&harness, &wire_order(false)).await;
    let (live, withdrawn) = offer_window(&pushed);

    assert_eq!(
        live,
        vec![Report::Closed(OFFER_DWELL)],
        "the only push carrying the offer is the away dwell's closed report"
    );
    assert_eq!(
        withdrawn,
        Some(Report::Closed(OFFER_DWELL + 1)),
        "the next report, the one-second return, withdraws it"
    );
}

/// The fix. With in-progress reports the offer is pushed when the person
/// arrives in the away app, stays live while they are there — the closed
/// report of that same dwell pushes nothing — and is withdrawn only by the
/// report that says they came back.
#[tokio::test]
async fn an_in_progress_report_pushes_the_offer_while_the_person_is_away() {
    let harness = harness();
    let (_, pushed) = replay(&harness, &wire_order(true)).await;
    let (live, withdrawn) = offer_window(&pushed);

    assert_eq!(
        live,
        vec![Report::InProgress(OFFER_DWELL)],
        "the offer is pushed once, by the away dwell's in-progress report"
    );
    assert_eq!(
        withdrawn,
        Some(Report::InProgress(OFFER_DWELL + 1)),
        "it is withdrawn by the return's in-progress report, not before"
    );
    let closed_away = pushed
        .iter()
        .find(|(report_kind, _)| *report_kind == Report::Closed(OFFER_DWELL))
        .unwrap();
    assert!(
        closed_away.1.is_empty(),
        "the away dwell's closed report changes nothing: {:?}",
        closed_away.1
    );
}

/// The drift policy did not change, only when its evidence arrives. The same
/// timeline sent both ways produces the same decisions, stamped at the same
/// instants, the same offer and outcome, the same observed spans, and the
/// same event ledger, and only closed reports reach the upload queue.
#[tokio::test]
async fn in_progress_reports_change_when_the_gate_decides_and_nothing_it_decides() {
    let closed_only = harness();
    let (closed_block, _) = replay(&closed_only, &wire_order(false)).await;
    let with_in_progress = harness();
    let (live_block, _) = replay(&with_in_progress, &wire_order(true)).await;

    let relative = |block: &WorkBlockSnapshot, at: DateTime<Utc>| {
        (at - block.started_at.unwrap()).num_seconds()
    };
    let decisions = |harness: &Harness, block: &WorkBlockSnapshot| {
        harness
            .blocks
            .decisions(&block.block_id.unwrap().to_string())
            .unwrap()
            .into_iter()
            .map(|decision| {
                (
                    relative(block, decision.occurred_at),
                    decision.gate_verdict,
                    decision.anchor_category,
                    decision.switch_count,
                    decision.elapsed_seconds,
                    decision.remaining_seconds,
                    decision.policy_version,
                )
            })
            .collect::<Vec<_>>()
    };
    let closed_decisions = decisions(&closed_only, &closed_block);
    assert_eq!(closed_decisions, decisions(&with_in_progress, &live_block));
    assert_eq!(
        closed_decisions
            .iter()
            .filter(|decision| decision.1 == GateVerdict::Offered)
            .count(),
        1
    );

    let offer = |harness: &Harness, block: &WorkBlockSnapshot| {
        let row = harness
            .blocks
            .intervention(&block.block_id.unwrap().to_string())
            .unwrap()
            .expect("the timeline produces an offer");
        (
            relative(block, row.offered_at),
            row.outcome,
            row.outcome_at.map(|at| relative(block, at)),
            row.switch_count,
            row.salience,
        )
    };
    let closed_offer = offer(&closed_only, &closed_block);
    assert_eq!(closed_offer, offer(&with_in_progress, &live_block));
    // Stamped at the away dwell's start, which the store keeps to the second.
    let away_start = TIMELINE[OFFER_DWELL].from;
    assert!(
        (away_start - 1..=away_start).contains(&closed_offer.0),
        "offered at {} seconds, not at the away dwell's start",
        closed_offer.0
    );
    assert_eq!(closed_offer.1, WorkBlockInterventionOutcome::Returned);

    // Rows the in-progress report opens and a boundary closes where they
    // opened claim nothing, so the spans are compared, not the row count.
    let spans = |harness: &Harness, block: &WorkBlockSnapshot| {
        harness
            .blocks
            .observations(&block.block_id.unwrap().to_string())
            .unwrap()
            .into_iter()
            .filter_map(|observation| {
                let ended_at = observation.ended_at?;
                (ended_at > observation.occurred_at).then(|| {
                    (
                        relative(block, observation.occurred_at),
                        relative(block, ended_at),
                        observation.category,
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        spans(&closed_only, &closed_block),
        spans(&with_in_progress, &live_block)
    );

    let ledger = |harness: &Harness, block: &WorkBlockSnapshot| {
        let started_at = block.started_at.unwrap();
        harness
            .raw_events
            .events_between(started_at, started_at + ChronoDuration::hours(1), 100)
            .unwrap()
            .into_iter()
            .map(|entry| {
                (
                    relative(block, entry.occurred_at),
                    entry.duration_seconds,
                    entry.label,
                    entry.category,
                    entry.classification_status,
                    entry.classification_confidence,
                )
            })
            .collect::<Vec<_>>()
    };
    let closed_ledger = ledger(&closed_only, &closed_block);
    assert_eq!(closed_ledger.len(), TIMELINE.len() - 1);
    assert_eq!(closed_ledger, ledger(&with_in_progress, &live_block));
    assert_eq!(
        closed_only.ingested.0.load(Ordering::SeqCst),
        TIMELINE.len() - 1
    );
    assert_eq!(
        with_in_progress.ingested.0.load(Ordering::SeqCst),
        TIMELINE.len() - 1,
        "only closed reports reach the upload queue"
    );
}

/// Outside a block there is nothing for the gate to decide, so an in-progress
/// report is acknowledged and goes nowhere: no event row, no upload, no push.
#[tokio::test]
async fn outside_a_block_an_in_progress_report_is_acknowledged_and_goes_nowhere() {
    let harness = harness();
    let now = Utc::now();
    let ack = harness
        .router
        .route(report(now, &TIMELINE[0], true))
        .await
        .unwrap();
    assert!(matches!(
        ack,
        Some(ServerMessage::RawEventAck(RawEventAck {
            status: RawEventStatus::Accepted,
            ..
        }))
    ));
    assert!(drain_pushes(&harness.pushes).await.is_empty());
    assert!(harness
        .raw_events
        .events_between(
            now - ChronoDuration::hours(1),
            now + ChronoDuration::hours(1),
            100
        )
        .unwrap()
        .is_empty());
    assert_eq!(harness.ingested.0.load(Ordering::SeqCst), 0);
}
