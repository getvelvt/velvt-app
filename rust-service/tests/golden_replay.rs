//! Golden replay: one recorded trace through the classification ladder, the
//! router, the drift gate and the decision log, compared byte for byte with
//! what the shipped code recorded.
//!
//! The personal-vault design (`docs/architecture/personal-vault-and-devices.md`
//! § 11.3, invariant I5) asks every change near classification provenance or
//! the vault to prove that no drift decision moved. This is the harness that
//! proof runs on.
//!
//! **What runs.** `tests/fixtures/golden_replay/trace.json` is a SYNTHETIC
//! trace of six declared blocks. Each dwell is reported the way a client on
//! protocol 32 or later reports it, in progress when it begins and closed when
//! it ends, through the real `R7Router`. So every report is classified by the
//! shipped ladder (`register_builtin_plugins_with_embedding` with the salted
//! Tier 2 plugin, as `main.rs` builds it) and reaches the real
//! `WorkBlockManager`. Blocks are started, paused, resumed, answered and ended
//! on that same manager at the trace's own instants, because the router stamps
//! those commands with the wall clock. Two rules are taught through the router
//! part way through, an application and a site, the way the needs-a-category
//! list teaches them. Finally the decision-outcome pass
//! (`InterventionDecisionOutcomeTarget`) resolves every decision's 600-second
//! horizon. The trace is dated 2023 so that pass, which reads the wall clock,
//! finds every horizon closed.
//!
//! **What is compared.** Every work-block push the router sent, and every row
//! of `work_block_intervention`, `intervention_decision_log` and
//! `work_block_observation`, each column as SQLite's `quote()` writes it: the
//! stored value with its type. Block and decision ids are random UUIDs, so
//! each is replaced by its order of first appearance. Nothing else is
//! normalized. A pushed offer's title and body are left out: they are copy,
//! chosen after the decision.
//!
//! **What a failure means.** A decision, a logged decision or an observation
//! changed. Re-record (`VELVT_GOLDEN_REPLAY_RECORD=1 cargo test --test
//! golden_replay`) only for a change meant to move decisions, which is a new
//! `DRIFT_POLICY_VERSION`; the golden's first line names the version it was
//! recorded under. Record mode enforces that: it refuses to overwrite a golden
//! recorded under the current version with a different one unless
//! `VELVT_GOLDEN_REPLAY_SAME_POLICY` holds the reason, for a change that is
//! not a policy change (an amendment in a parent PR, a new trace step). Such a
//! re-record is its own commit, recorded on the code before the change it
//! protects, and names the reason.
//!
//! **Provenance.** `decisions.golden` was recorded before migration 0045 added
//! `work_block_observation.classification_source` and
//! `intervention_decision_log.synced_rule_evidence`. Both record where
//! evidence came from and decide nothing, so they are left out of it
//! (`RECORD_ONLY_COLUMNS`), and it still proves that the decisions are the
//! ones the code made before the columns existed. They are compared on their
//! own in `provenance.golden`, which records no decision and is re-recorded
//! whenever what the columns record changes. A row's source is the rung of the
//! report that opened it, unless a rule classified a report the row absorbed
//! (same evidence, so no row of its own), in which case it names the rule:
//! each post-teach dwell of the taught app and site shows `user_rule`, even
//! the two that fall inside a row a seed entry opened.
//!
//! **Not reached.** No activity in the trace reaches Tier 2, so `embedding`
//! never appears. The Focus source and Velvt's quiet hours are wired as
//! `main.rs` wires them, but the trace reports no Focus transition or
//! availability and sets no quiet hours, so every decision logs
//! `focus_state = 'unknown'` and nothing is held or quieted by them. The
//! populated-vault arm (design § 11.3, PR A7) needs a trace that reaches all
//! three: an activity that would otherwise reach Tier 2 (the vault rung sits
//! before the plugins), Focus on during a block, and quiet hours. It is
//! recorded as a second trace and golden on the code before A7, so this
//! golden stays as it is.
//!
//! **The question after a block** (attention plan § 3.3 and § 4.3). Two more
//! arms replay the same trace after the Mac's first 14 days, which is the
//! only time the question can be asked: a block 15 days before the trace
//! starts the Mac's primary period, and is removed before the dump, so the
//! dump is the trace's own. One arm has no question. The other attaches one
//! `RatingPromptManager` to the manager and the router, as `main.rs` does,
//! turns the question on before the trace, and after each block records one
//! kind of row ([`AFTER_BLOCK`]): answered, answered through the link after
//! Not now, seen and never answered, never shown, and hidden by turning the
//! question off and on again. The router stamps the question's messages with
//! the wall clock, years after this 2023 trace, so those rows are recorded
//! through the manager on the trace's own clock; the router's handlers are
//! also driven between blocks, with every message the question has, and are
//! refused or change nothing, as a late message would be. Both arms must
//! reproduce `decisions.golden` and `provenance.golden` exactly, and the
//! question's arm must hold every kind of row, so the comparison is not
//! vacuous.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use uuid::Uuid;
use velvt_service::abstraction::{
    app_stable_key_for, site_stable_key_for, AbstractedEvent, AbstractionEngine,
    EmbeddingSimilarityPlugin, Taxonomy,
};
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::retention::{InterventionDecisionOutcomeTarget, RetentionTarget};
use velvt_service::self_rating::RatingPromptManager;
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{
    FocusStateSource, RatingPromptSource, WorkBlockManager, DRIFT_POLICY_VERSION,
};
use velvt_shared_types::{
    ClientMessage, InterventionResponse, RateWorkBlock, RatingPromptPresentation, RawEvent,
    RawEventAck, RawEventStatus, RequestRatingPromptSettings, RequestWorkBlockState,
    RespondRatingOptIn, ServerMessage, SetApplicationCategory, SetRatingPromptEnabled,
    SetSiteCategory, StartWorkBlock, WorkBlockIntensity, WorkBlockPurpose, WorkBlockRatingAnswer,
    WorkBlockRatingNotNow, WorkBlockRatingShown,
};

const TRACE: &str = "trace.json";
const DECISIONS_GOLDEN: &str = "decisions.golden";
const PROVENANCE_GOLDEN: &str = "provenance.golden";
const RECORD_ENV: &str = "VELVT_GOLDEN_REPLAY_RECORD";
/// The reason a golden recorded under the current `DRIFT_POLICY_VERSION` is
/// re-recorded under that same version. Record mode refuses without it.
const SAME_POLICY_ENV: &str = "VELVT_GOLDEN_REPLAY_SAME_POLICY";

/// The tables the comparison reads, in the order the golden lists them.
const TABLES: [&str; 3] = [
    "work_block_intervention",
    "intervention_decision_log",
    "work_block_observation",
];

/// Columns that record where evidence came from and never decide anything
/// (migration 0045), left out of `decisions.golden` and compared in
/// `provenance.golden` instead. A column added to these tables later lands in
/// `decisions.golden` and fails it, until it is listed here with tests of its
/// own or the golden is re-recorded under a new policy version.
const RECORD_ONLY_COLUMNS: [(&str, &str); 2] = [
    ("work_block_observation", "classification_source"),
    ("intervention_decision_log", "synced_rule_evidence"),
];

/// The sources the trace is written to produce: every rung a Mac reaches
/// without a Tier 2 match. `vault_rule` is not among them and must never be.
/// `embedding` is missing because no activity reaches Tier 2; the trace A7
/// adds has to reach it (see "Not reached" above).
const EXPECTED_SOURCES: [&str; 6] = [
    "'declared_app_category'",
    "'declared_document_types'",
    "'fallback'",
    "'heuristic'",
    "'seed'",
    "'user_rule'",
];

/// A fixed Tier 2 salt. The shipped service mints one at random per install;
/// a random one here would make the Tier 2 rung, and so the golden, depend on
/// the run.
const EMBEDDING_SALT: [u8; 32] = [0x5a; 32];

// ---------------------------------------------------------------------------
// The trace
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace {
    about: String,
    origin: i64,
    activities: BTreeMap<String, Activity>,
    steps: Vec<Step>,
}

/// What the Mac client would report about one window: facts only, as Swift
/// reports them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activity {
    app: String,
    bundle_id: Option<String>,
    title: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    declared_app_category: Option<String>,
    #[serde(default)]
    document_type_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
enum Step {
    Start {
        at: i64,
        minutes: u32,
        purpose: String,
        intensity: String,
    },
    Dwell {
        at: i64,
        until: i64,
        activity: String,
    },
    Pause {
        at: i64,
    },
    Resume {
        at: i64,
    },
    Reply {
        at: i64,
        response: String,
    },
    End {
        at: i64,
    },
    TeachApp {
        at: i64,
        app: String,
        category: String,
    },
    TeachSite {
        at: i64,
        host: String,
        category: String,
    },
}

impl Step {
    fn at(&self) -> i64 {
        match self {
            Self::Start { at, .. }
            | Self::Dwell { at, .. }
            | Self::Pause { at }
            | Self::Resume { at }
            | Self::Reply { at, .. }
            | Self::End { at }
            | Self::TeachApp { at, .. }
            | Self::TeachSite { at, .. } => *at,
        }
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("golden_replay")
}

fn load_trace() -> Trace {
    let path = fixtures_dir().join(TRACE);
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing {}: {error}", path.display()));
    let trace: Trace = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("unreadable {}: {error}", path.display()));
    assert!(
        trace.about.starts_with("SYNTHETIC."),
        "the trace must say it is synthetic: it is committed and nothing in it may come from a person"
    );
    let mut previous = i64::MIN;
    for step in &trace.steps {
        assert!(
            step.at() >= previous,
            "steps must be in time order; {step:?} comes after t={previous}"
        );
        previous = step.at();
        if let Step::Dwell {
            at,
            until,
            activity,
        } = step
        {
            assert!(until > at, "{activity} at t={at} must end after it begins");
            assert!(
                trace.activities.contains_key(activity),
                "{activity} is not one of the trace's activities"
            );
        }
    }
    trace
}

fn purpose_of(value: &str) -> WorkBlockPurpose {
    match value {
        "deep_work" => WorkBlockPurpose::DeepWork,
        "study" => WorkBlockPurpose::Study,
        "creative_practice" => WorkBlockPurpose::CreativePractice,
        "healthy_tech_use" => WorkBlockPurpose::HealthyTechUse,
        "work_life_boundary" => WorkBlockPurpose::WorkLifeBoundary,
        other => panic!("unknown purpose in the trace: {other}"),
    }
}

fn intensity_of(value: &str) -> WorkBlockIntensity {
    match value {
        "light" => WorkBlockIntensity::Light,
        "medium" => WorkBlockIntensity::Medium,
        "intense" => WorkBlockIntensity::Intense,
        other => panic!("unknown intensity in the trace: {other}"),
    }
}

fn response_of(value: &str) -> InterventionResponse {
    match value {
        "accepted_action" => InterventionResponse::AcceptedAction,
        "not_helpful" => InterventionResponse::NotHelpful,
        "wrong_classification" => InterventionResponse::WrongClassification,
        "was_focused" => InterventionResponse::WasFocused,
        "dismissed" => InterventionResponse::Dismissed,
        other => panic!("unknown reply in the trace: {other}"),
    }
}

// ---------------------------------------------------------------------------
// The service, wired as main.rs wires the parts this trace reaches: the
// ladder, the router, the work-block manager, and the Focus source the manager
// and the router both read. Initiation, receipts, the category prompt and the
// upload path are not wired: no step in the trace reaches them.
// ---------------------------------------------------------------------------

/// No cloud is reachable. Nothing in this trace needs one, and a request that
/// were made would fail rather than be answered.
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

/// The upload queue is not what this harness measures.
struct DiscardingIngestor;

impl EventIngestor for DiscardingIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a AbstractedEvent,
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

/// A database file of its own, so the comparison can read it with a plain
/// connection, removed when the replay is done.
struct ScratchDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl ScratchDatabase {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("velvt-golden-replay-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("velvt-service.sqlite3");
        Self { directory, path }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

struct Service {
    persistence: SqlitePersistence,
    router: R7Router,
    manager: Arc<WorkBlockManager>,
    pushes: Arc<PushQueue>,
    /// The question after a block, on the arm that has it.
    rating: Option<Arc<RatingPromptManager>>,
}

fn service(scratch: &ScratchDatabase, arm: Arm) -> Service {
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    // Replaced before anything is classified, so no cached sketch was made
    // under the salt the database minted.
    Connection::open(&scratch.path)
        .unwrap()
        .execute(
            "INSERT INTO embedding_salt(id, salt) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET salt = excluded.salt",
            [EMBEDDING_SALT.to_vec()],
        )
        .unwrap();

    let taxonomy = Taxonomy::from_builtin().unwrap();
    let salt = persistence.abstraction_map_repo().embedding_salt().unwrap();
    let embedding = EmbeddingSimilarityPlugin::builtin_salted(taxonomy.version(), salt)
        .unwrap()
        .with_learning_store(persistence.semantic_learning_store());
    let engine = AbstractionEngine::builder(persistence.abstraction_mapping_store(), taxonomy)
        .register_builtin_plugins_with_embedding(Some(embedding))
        .build()
        .unwrap();

    // As main.rs wires it: the manager holds offers while Focus is on and
    // records what was known about Focus on each decision, and the router
    // quiets an offer inside Velvt's quiet hours. The trace reports no Focus
    // state and sets no quiet hours, so neither changes anything here.
    let focus = FocusManager::new(persistence.focus_repo());
    // As main.rs wires it: one manager for the question, which the
    // work-block manager consults while it builds an ended block's snapshot
    // and the router hands the question's messages to.
    let rating = (arm == Arm::PastDay14WithQuestion)
        .then(|| Arc::new(RatingPromptManager::new(persistence.self_rating_repo())));
    let mut manager = WorkBlockManager::new(persistence.work_block_repo())
        .with_focus_source(Arc::clone(&focus) as Arc<dyn FocusStateSource>);
    if let Some(rating) = &rating {
        manager = manager.with_rating_prompt(Arc::clone(rating) as Arc<dyn RatingPromptSource>);
    }
    let manager = Arc::new(manager);
    let pushes = PushQueue::new(256);
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp),
        Arc::new(OfflineHttp),
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    let router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        Arc::new(engine),
        persistence.raw_event_repo(),
        Arc::new(DiscardingIngestor),
        account,
    )
    .with_classification_corrections(
        persistence.abstraction_map_repo(),
        persistence.upload_batch_repo(),
        Arc::new(OfflineHttp),
    )
    .with_work_blocks(manager.clone(), PushAdapter::new(pushes.clone()))
    .with_focus(focus);
    let router = match &rating {
        Some(rating) => router.with_rating_prompt(Arc::clone(rating)),
        None => router,
    };
    Service {
        persistence,
        router,
        manager,
        pushes,
        rating,
    }
}

// ---------------------------------------------------------------------------
// The question after a block
// ---------------------------------------------------------------------------

/// How the trace is replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// The golden's own replay: the trace is the Mac's first evening.
    Plain,
    /// A block 15 days earlier starts the Mac's primary period, so the trace
    /// runs after its first 14 days, with no question.
    PastDay14,
    /// The same, with the question attached and turned on before the trace.
    PastDay14WithQuestion,
}

/// The first block of the Mac, this long before the trace's origin.
const PERIOD_SEED_OFFSET: i64 = -15 * 86_400;
/// When the question is turned on, before the trace's first block and after
/// the seeded period's end (day 14, one day before the origin).
const TURNED_ON_OFFSET: i64 = -3_600;

/// What happens to the question about each of the trace's blocks, in order.
#[derive(Debug, Clone, Copy)]
enum AfterBlock {
    /// Seen, then answered.
    Answered(WorkBlockRatingAnswer),
    /// Seen, Not now, then answered through the link with the card seen.
    AnsweredThroughTheLink(WorkBlockRatingAnswer),
    /// Seen and never answered: the next block's start expires it.
    SeenOnly,
    /// Never on screen: the next block's start expires it.
    NeverShown,
    /// The question turned off and on again in Settings. It stays hidden,
    /// every report about the block is refused, and the next block's start
    /// expires it.
    OffAndOnAgain,
}

const AFTER_BLOCK: [AfterBlock; 6] = [
    AfterBlock::Answered(WorkBlockRatingAnswer::Most),
    AfterBlock::AnsweredThroughTheLink(WorkBlockRatingAnswer::Half),
    AfterBlock::SeenOnly,
    AfterBlock::NeverShown,
    AfterBlock::OffAndOnAgain,
    AfterBlock::Answered(WorkBlockRatingAnswer::Little),
];

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// One closed report still to send: the dwell's index in the trace and when
/// it ends.
struct PendingClose {
    step: usize,
    until: i64,
}

struct Replayer<'a> {
    trace: &'a Trace,
    service: Service,
    block: Option<Uuid>,
    pending: Vec<PendingClose>,
    /// Each work-block push, as one line: which report sent it and the offer
    /// it carried, if any.
    pushes: Vec<String>,
    /// How many of the trace's blocks have ended.
    ended: usize,
    /// How many snapshots carried the question, on the question's arm.
    asked: usize,
}

impl Replayer<'_> {
    fn instant(&self, offset: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(self.trace.origin, 0).unwrap() + Duration::seconds(offset)
    }

    fn block(&self, step: &Step) -> Uuid {
        self.block
            .unwrap_or_else(|| panic!("{step:?} needs a block, and none has started"))
    }

    fn raw_event(&self, step: usize, in_progress: bool) -> ClientMessage {
        let Step::Dwell {
            at,
            until,
            activity,
        } = &self.trace.steps[step]
        else {
            unreachable!("only a dwell is reported");
        };
        let activity_facts = &self.trace.activities[activity];
        ClientMessage::RawEvent(RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: self.instant(*at),
            duration_seconds: if in_progress {
                0
            } else {
                u64::try_from(until - at).unwrap()
            },
            app_name: activity_facts.app.clone(),
            window_title: activity_facts.title.clone(),
            bundle_id: activity_facts.bundle_id.clone(),
            declared_app_category: activity_facts.declared_app_category.clone(),
            document_type_ids: activity_facts.document_type_ids.clone(),
            focused_document_url: activity_facts.url.clone(),
            in_progress,
        })
    }

    async fn report(&mut self, step: usize, in_progress: bool) {
        let ack = self
            .service
            .router
            .route(self.raw_event(step, in_progress))
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
            "step {step} was not accepted: {ack:?}"
        );
        let kind = if in_progress { "in_progress" } else { "closed" };
        while let Some(message) = self.service.pushes.try_pop().await {
            let ServerMessage::WorkBlockState(snapshot) = message else {
                continue;
            };
            let offer = snapshot.active_intervention.map_or_else(
                || "no offer".to_owned(),
                |offer| {
                    format!(
                        "offer action_id={} anchor_category={} switch_count={} \
                         window_seconds={} offered_at={} salience={:?}",
                        offer.action_id,
                        offer.anchor_category,
                        offer.switch_count,
                        offer.window_seconds,
                        offer.offered_at.timestamp(),
                        offer.salience,
                    )
                },
            );
            self.pushes.push(format!("step {step} {kind}: {offer}"));
        }
    }

    /// Sends every closed report due by `at`, earliest first. A dwell that
    /// ends at the instant the next one begins is closed before it, which is
    /// the order a client sends them in.
    async fn close_due(&mut self, at: i64) {
        self.pending.sort_by_key(|pending| pending.until);
        while self.pending.first().is_some_and(|due| due.until <= at) {
            let due = self.pending.remove(0);
            self.report(due.step, false).await;
        }
    }

    async fn teach(&mut self, message: ClientMessage, step: usize) {
        let answer = self.service.router.route(message).await.unwrap();
        assert!(
            matches!(answer, Some(ServerMessage::MenuStatus(_))),
            "step {step} did not teach its rule: {answer:?}"
        );
    }

    /// The question's side of the trace, after a block ends at `end`. Does
    /// nothing on an arm without the question. Rows are recorded on the
    /// trace's clock through the manager the router holds; then, for a block
    /// whose question is already closed, every message the question has is
    /// also sent through the router, which stamps the wall clock, so each is
    /// refused or changes nothing.
    async fn after_block_end(&mut self, block: Uuid, end: i64) {
        let Some(rating) = self.service.rating.clone() else {
            return;
        };
        let what = AFTER_BLOCK[self.ended];
        self.ended += 1;
        let manager = self.service.manager.clone();
        let snapshot = manager.request_state(self.instant(end + 5)).unwrap();
        let prompt = snapshot.rating_prompt.as_ref();
        if prompt.is_some() {
            self.asked += 1;
        }
        let at = |offset: i64| self.instant(end + offset);
        match what {
            AfterBlock::Answered(answer) => {
                assert_eq!(prompt.map(|prompt| prompt.block_id), Some(block));
                assert!(rating.record_shown(block, at(5)).unwrap());
                assert!(rating.record_answer(block, answer, false, at(20)).unwrap());
            }
            AfterBlock::AnsweredThroughTheLink(answer) => {
                assert!(rating.record_shown(block, at(5)).unwrap());
                assert!(rating.record_not_now(block, at(10)).unwrap());
                let link = manager.request_state(at(15)).unwrap().rating_prompt;
                assert_eq!(
                    link.map(|prompt| prompt.presentation),
                    Some(RatingPromptPresentation::Link)
                );
                assert!(rating.record_answer(block, answer, true, at(300)).unwrap());
            }
            AfterBlock::SeenOnly => {
                assert!(rating.record_shown(block, at(5)).unwrap());
            }
            AfterBlock::NeverShown => {
                assert!(prompt.is_some(), "eligible, and never reported seen");
            }
            AfterBlock::OffAndOnAgain => {
                assert!(prompt.is_some());
                assert!(!rating.set_enabled(false, at(30)).unwrap().enabled);
                assert!(rating.set_enabled(true, at(60)).unwrap().enabled);
                assert_eq!(manager.request_state(at(65)).unwrap().rating_prompt, None);
                assert!(!rating.record_shown(block, at(70)).unwrap());
                assert!(!rating.record_not_now(block, at(71)).unwrap());
                assert!(!rating
                    .record_answer(block, WorkBlockRatingAnswer::All, false, at(72))
                    .unwrap());
            }
        }
        if matches!(
            what,
            AfterBlock::Answered(_) | AfterBlock::AnsweredThroughTheLink(_)
        ) {
            self.late_messages(block).await;
        }
    }

    /// Every message the question has, sent through the router for a block
    /// whose question is answered. Each is refused or changes nothing, the
    /// replies carry neither the question nor the invitation, and nothing is
    /// pushed.
    async fn late_messages(&mut self, block: Uuid) {
        let router = &self.service.router;
        let messages = [
            ClientMessage::WorkBlockRatingShown(WorkBlockRatingShown { block_id: block }),
            ClientMessage::WorkBlockRatingNotNow(WorkBlockRatingNotNow { block_id: block }),
            ClientMessage::RateWorkBlock(RateWorkBlock {
                block_id: block,
                answer: WorkBlockRatingAnswer::All,
                via_link: false,
            }),
            ClientMessage::RespondRatingOptIn(RespondRatingOptIn { turn_on: false }),
            ClientMessage::RespondRatingOptIn(RespondRatingOptIn { turn_on: true }),
            ClientMessage::RequestRatingPromptSettings(RequestRatingPromptSettings {}),
            ClientMessage::RequestWorkBlockState(RequestWorkBlockState {}),
        ];
        for message in messages {
            match router.route(message.clone()).await.unwrap() {
                None => assert!(matches!(message, ClientMessage::WorkBlockRatingShown(_))),
                Some(ServerMessage::WorkBlockState(snapshot)) => {
                    assert_eq!(snapshot.rating_prompt, None, "{message:?}");
                    assert_eq!(snapshot.rating_opt_in, None, "{message:?}");
                }
                Some(ServerMessage::RatingPromptSettings(settings)) => {
                    assert!(settings.enabled && settings.available, "{message:?}");
                }
                Some(other) => panic!("{message:?} was answered with {other:?}"),
            }
        }
        assert!(
            self.service.pushes.try_pop().await.is_none(),
            "a message about the question pushed a work-block state"
        );
    }

    async fn run(&mut self) {
        let trace = self.trace;
        for (index, step) in trace.steps.iter().enumerate() {
            self.close_due(step.at()).await;
            let at = self.instant(step.at());
            let manager = self.service.manager.clone();
            match step {
                Step::Start {
                    minutes,
                    purpose,
                    intensity,
                    ..
                } => {
                    let started = manager
                        .start(
                            StartWorkBlock {
                                intention: None,
                                planned_duration_seconds: minutes * 60,
                                purpose: Some(purpose_of(purpose)),
                                intensity: intensity_of(intensity),
                                invitation_id: None,
                            },
                            at,
                        )
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
                    self.block = started.block_id;
                }
                Step::Dwell { until, .. } => {
                    self.pending.push(PendingClose {
                        step: index,
                        until: *until,
                    });
                    self.report(index, true).await;
                }
                Step::Pause { .. } => {
                    manager
                        .pause(self.block(step), at)
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
                }
                Step::Resume { .. } => {
                    manager
                        .resume(self.block(step), at)
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
                }
                Step::Reply { response, .. } => {
                    manager
                        .report_intervention_outcome(self.block(step), response_of(response), at)
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
                }
                Step::End { at: offset } => {
                    manager
                        .end(self.block(step), at)
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
                    self.after_block_end(self.block(step), *offset).await;
                }
                Step::TeachApp { app, category, .. } => {
                    let salt = self
                        .service
                        .persistence
                        .abstraction_map_repo()
                        .stable_key_salt()
                        .unwrap();
                    let message = ClientMessage::SetApplicationCategory(SetApplicationCategory {
                        app_stable_id: app_stable_key_for(&salt, app),
                        category: category.clone(),
                        activity_name: None,
                    });
                    self.teach(message, index).await;
                }
                Step::TeachSite { host, category, .. } => {
                    let salt = self
                        .service
                        .persistence
                        .abstraction_map_repo()
                        .stable_key_salt()
                        .unwrap();
                    let message = ClientMessage::SetSiteCategory(SetSiteCategory {
                        site_stable_id: site_stable_key_for(&salt, host),
                        category: category.clone(),
                        activity_name: None,
                    });
                    self.teach(message, index).await;
                }
            }
        }
        self.close_due(i64::MAX).await;
        InterventionDecisionOutcomeTarget::new(self.service.persistence.work_block_repo(), 10_000)
            .run_cleanup()
            .unwrap();
    }
}

/// The trace, replayed into a fresh database.
#[derive(Debug, PartialEq)]
struct Replayed {
    decisions: String,
    provenance: String,
    /// Every row of `work_block_self_rating`, block ids canonical, on the
    /// question's arm; empty otherwise.
    ratings: Vec<String>,
    /// How many snapshots carried the question.
    asked: usize,
}

async fn replay(trace: &Trace) -> Replayed {
    replay_arm(trace, Arm::Plain).await
}

async fn replay_arm(trace: &Trace, arm: Arm) -> Replayed {
    let scratch = ScratchDatabase::new();
    let service = service(&scratch, arm);
    let origin = DateTime::from_timestamp(trace.origin, 0).unwrap();
    let seed = (arm != Arm::Plain).then(|| {
        let started = service
            .manager
            .start(
                StartWorkBlock {
                    intention: None,
                    planned_duration_seconds: 1_500,
                    purpose: Some(WorkBlockPurpose::DeepWork),
                    intensity: WorkBlockIntensity::Medium,
                    invitation_id: None,
                },
                origin + Duration::seconds(PERIOD_SEED_OFFSET),
            )
            .unwrap();
        let seed = started.block_id.unwrap();
        service
            .manager
            .end(seed, origin + Duration::seconds(PERIOD_SEED_OFFSET + 1_200))
            .unwrap();
        seed
    });
    if let Some(rating) = &service.rating {
        let turned_on = rating
            .set_enabled(true, origin + Duration::seconds(TURNED_ON_OFFSET))
            .unwrap();
        assert!(turned_on.enabled, "the seeded period is over by then");
    }
    let mut replayer = Replayer {
        trace,
        service,
        block: None,
        pending: Vec::new(),
        pushes: Vec::new(),
        ended: 0,
        asked: 0,
    };
    replayer.run().await;
    if replayer.service.rating.is_some() {
        // The Settings toggle's off, through the router, after the last
        // block: it changes no decision either.
        let off = replayer
            .service
            .router
            .route(ClientMessage::SetRatingPromptEnabled(
                SetRatingPromptEnabled { enabled: false },
            ))
            .await
            .unwrap();
        assert!(
            matches!(off, Some(ServerMessage::RatingPromptSettings(settings)) if !settings.enabled)
        );
    }
    let connection = Connection::open(&scratch.path).unwrap();
    if let Some(seed) = seed {
        // The seed only places the period. It has no row in any compared
        // table, and it is removed so the dump numbers the trace's blocks
        // as the golden does.
        let seed = seed.to_string();
        for table in TABLES {
            let rows: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE block_id = ?1"),
                    [&seed],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0, "the seeded block has a row in {table}");
        }
        assert_eq!(
            connection
                .execute("DELETE FROM work_block WHERE block_id = ?1", [&seed])
                .unwrap(),
            1
        );
    }
    let ids = canonical_ids(&connection);
    Replayed {
        decisions: decisions_dump(&connection, &replayer.pushes),
        provenance: provenance_dump(&connection),
        ratings: table_lines(
            &connection,
            "work_block_self_rating",
            &column_names(&connection, "work_block_self_rating"),
            &ids,
        ),
        asked: replayer.asked,
    }
}

// ---------------------------------------------------------------------------
// The dump
// ---------------------------------------------------------------------------

fn column_names(connection: &Connection, table: &str) -> Vec<String> {
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Random ids, quoted as `quote()` writes them, mapped to their order of
/// first appearance.
fn canonical_ids(connection: &Connection) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for (query, prefix) in [
        (
            "SELECT quote(block_id) FROM work_block ORDER BY started_at, rowid",
            "block",
        ),
        (
            "SELECT quote(decision_id) FROM intervention_decision_log ORDER BY rowid",
            "decision",
        ),
    ] {
        let values: Vec<String> = connection
            .prepare(query)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (index, value) in values.into_iter().enumerate() {
            ids.insert(value, format!("'{prefix}-{}'", index + 1));
        }
    }
    ids
}

/// Every row of `table`, one line each, each named column as `quote()`
/// writes it.
fn table_lines(
    connection: &Connection,
    table: &str,
    columns: &[String],
    ids: &HashMap<String, String>,
) -> Vec<String> {
    let selected = columns
        .iter()
        .map(|column| format!("quote(\"{column}\")"))
        .collect::<Vec<_>>()
        .join(", ");
    connection
        .prepare(&format!(
            "SELECT {selected} FROM \"{table}\" ORDER BY rowid"
        ))
        .unwrap()
        .query_map([], |row| {
            Ok(columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    let value: String = row.get(index).unwrap();
                    let value = ids.get(&value).cloned().unwrap_or(value);
                    format!("{column}={value}")
                })
                .collect::<Vec<_>>()
                .join(" | "))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn decisions_dump(connection: &Connection, pushes: &[String]) -> String {
    let ids = canonical_ids(connection);
    let mut out = format!(
        "# golden replay of tests/fixtures/golden_replay/{TRACE}, drift policy {DRIFT_POLICY_VERSION}\n"
    );
    out.push_str("\n[pushes]\n");
    for push in pushes {
        out.push_str(push);
        out.push('\n');
    }
    for table in TABLES {
        let columns: Vec<String> = column_names(connection, table)
            .into_iter()
            .filter(|column| !RECORD_ONLY_COLUMNS.contains(&(table, column.as_str())))
            .collect();
        out.push_str(&format!("\n[{table}]\n"));
        for line in table_lines(connection, table, &columns, &ids) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Each row's record-only columns, beside enough of the row to read them by.
fn provenance_dump(connection: &Connection) -> String {
    let ids = canonical_ids(connection);
    let mut out =
        format!("# provenance in the golden replay of tests/fixtures/golden_replay/{TRACE}\n");
    let observation_context: &[&str] = &["id", "block_id", "occurred_at", "ended_at", "category"];
    let decision_context: &[&str] = &["decision_id", "block_id", "gate_verdict"];
    for (table, context) in [
        ("work_block_observation", observation_context),
        ("intervention_decision_log", decision_context),
    ] {
        let mut columns: Vec<String> = context.iter().map(|&column| column.to_owned()).collect();
        columns.extend(
            RECORD_ONLY_COLUMNS
                .iter()
                .filter(|(owner, _)| *owner == table)
                .map(|(_, column)| (*column).to_owned()),
        );
        out.push_str(&format!("\n[{table}]\n"));
        for line in table_lines(connection, table, &columns, &ids) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Why record mode must not overwrite `existing` with `actual`, if it must
/// not: `existing` names the current `DRIFT_POLICY_VERSION` on its first line,
/// `actual` differs from it, and no reason for a same-version re-record was
/// given. A golden that names no policy (`provenance.golden`) records no
/// decision, and one recorded under another version is replaced by the bump
/// that made this one.
fn same_policy_rerecord_refusal(
    name: &str,
    existing: &str,
    actual: &str,
    reason: Option<&str>,
) -> Option<String> {
    let current = format!(", drift policy {DRIFT_POLICY_VERSION}");
    let recorded_under_current = existing
        .lines()
        .next()
        .is_some_and(|header| header.ends_with(&current));
    let has_reason = reason.is_some_and(|reason| !reason.trim().is_empty());
    (recorded_under_current && existing != actual && !has_reason).then(|| {
        format!(
            "{name} was recorded under drift policy {DRIFT_POLICY_VERSION} and the replay now \
             differs from it. A changed decision is a new DRIFT_POLICY_VERSION. If this is not a \
             policy change (an amendment in a parent PR, a new trace step), re-record on the code \
             before the change, in a commit of its own, with {SAME_POLICY_ENV} set to the reason."
        )
    })
}

/// Compares `actual` with the committed golden, or writes it when recording.
fn check_golden(name: &str, actual: &str) {
    let path = fixtures_dir().join(name);
    if std::env::var_os(RECORD_ENV).is_some() {
        if let Ok(existing) = std::fs::read_to_string(&path) {
            let reason = std::env::var(SAME_POLICY_ENV).ok();
            if let Some(refusal) =
                same_policy_rerecord_refusal(name, &existing, actual, reason.as_deref())
            {
                panic!("{refusal}");
            }
            if existing != actual {
                if let Some(reason) = reason {
                    println!("re-recording {name} under the same policy: {reason}");
                }
            }
        }
        std::fs::write(&path, actual).unwrap();
        println!("recorded {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing {}: {error}. Record it with {RECORD_ENV}=1 cargo test --test golden_replay",
            path.display()
        )
    });
    if expected == actual {
        return;
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let mut differences = Vec::new();
    for index in 0..expected_lines.len().max(actual_lines.len()) {
        let (want, got) = (expected_lines.get(index), actual_lines.get(index));
        if want != got {
            differences.push(format!(
                "line {}:\n  golden:   {}\n  replayed: {}",
                index + 1,
                want.unwrap_or(&"<none>"),
                got.unwrap_or(&"<none>")
            ));
        }
        if differences.len() == 12 {
            break;
        }
    }
    panic!(
        "the replay no longer matches {name}: a decision, a logged decision or an \
         observation changed. Re-record only under a new DRIFT_POLICY_VERSION.\n{}",
        differences.join("\n")
    );
}

/// The `name=value` fields of every line in one section of a dump.
fn section<'a>(dump: &'a str, name: &str) -> Vec<BTreeMap<&'a str, &'a str>> {
    let header = format!("[{name}]");
    dump.lines()
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| !line.is_empty())
        .map(|line| {
            line.split(" | ")
                .map(|field| field.split_once('=').unwrap())
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The proof itself: the decisions, the decision log and the observations are
/// the ones recorded in `decisions.golden`.
#[tokio::test]
async fn the_recorded_trace_replays_to_the_golden_decisions() {
    let replayed = replay(&load_trace()).await;
    check_golden(DECISIONS_GOLDEN, &replayed.decisions);
}

/// A golden is only evidence if the replay cannot differ from run to run: the
/// ids are drawn at random, the Tier 2 salt is pinned, and every instant comes
/// from the trace. Two replays into two databases agree exactly, so a
/// difference from the golden is a change in the code.
#[tokio::test]
async fn two_replays_of_the_trace_agree_exactly() {
    let trace = load_trace();
    let first = replay(&trace).await;
    let second = replay(&trace).await;
    assert_eq!(first, second);
}

/// The golden would also hold for a harness that never reached the gate. It
/// does not: the trace reaches the branches it was written for, makes three
/// offers (one answered `was_focused`, one quiet after the cooldown that answer
/// earned), and every decision with an anchor has its horizon resolved.
#[tokio::test]
async fn the_trace_reaches_the_gate_branches_it_was_written_for() {
    let dump = replay(&load_trace()).await.decisions;

    let verdicts: BTreeSet<&str> = section(&dump, "intervention_decision_log")
        .iter()
        .map(|row| row["gate_verdict"])
        .collect();
    for verdict in [
        "'abstained_warmup'",
        "'abstained_min_switches'",
        "'abstained_at_anchor'",
        "'offered'",
        "'abstained_block_cap'",
        "'abstained_remaining'",
        "'abstained_backoff'",
        "'abstained_no_anchor'",
    ] {
        assert!(
            verdicts.contains(verdict),
            "no decision reached {verdict}; the trace reached {verdicts:?}"
        );
    }

    let offers = section(&dump, "work_block_intervention");
    assert_eq!(offers.len(), 3, "{offers:?}");
    let outcomes: Vec<(&str, &str)> = offers
        .iter()
        .map(|offer| (offer["outcome"], offer["salience"]))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("'returned'", "'normal'"),
            ("'was_focused'", "'normal'"),
            ("'returned'", "'quiet'"),
        ]
    );

    assert!(
        section(&dump, "intervention_decision_log")
            .iter()
            .filter(|row| row["anchor_category"] != "NULL")
            .all(|row| row["anchor_seen_within_600s"] != "NULL"),
        "a decision with an anchor was left unresolved"
    );
    assert!(dump.contains(": offer action_id=protect_next_10"));
}

/// Migration 0045: every observation records the rung that classified the
/// report that opened it, or a rule that classified a report it absorbed, as
/// `provenance.golden` lists them. The trace reaches every rung a Mac reaches
/// without a Tier 2 match, including the two rules it teaches, and nothing
/// records `vault_rule`, which no build can produce.
#[tokio::test]
async fn every_observation_records_the_rung_that_classified_it() {
    let provenance = replay(&load_trace()).await.provenance;
    check_golden(PROVENANCE_GOLDEN, &provenance);

    let observations = section(&provenance, "work_block_observation");
    assert!(!observations.is_empty());
    let sources: BTreeSet<&str> = observations
        .iter()
        .map(|row| row["classification_source"])
        .collect();
    assert_eq!(
        sources,
        EXPECTED_SOURCES.into_iter().collect::<BTreeSet<_>>(),
        "an observation recorded no source, or one the trace does not produce"
    );
}

/// The instant `offset` seconds into the trace, as `quote()` writes a stored
/// timestamp.
fn stored_instant(trace: &Trace, offset: i64) -> i64 {
    trace.origin + offset
}

/// The host of a URL in the trace, which only ever holds `scheme://host/...`.
fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme.split('/').next().unwrap_or(after_scheme)
}

/// Every dwell after a rule is taught that the rule applies to, as
/// `(trace offset it begins at, offset it ends at, activity)`.
fn dwells_a_taught_rule_classifies(trace: &Trace) -> Vec<(i64, i64, &str)> {
    let mut taught_apps: Vec<(i64, &str)> = Vec::new();
    let mut taught_hosts: Vec<(i64, &str)> = Vec::new();
    for step in &trace.steps {
        match step {
            Step::TeachApp { at, app, .. } => taught_apps.push((*at, app)),
            Step::TeachSite { at, host, .. } => taught_hosts.push((*at, host)),
            _ => {}
        }
    }
    trace
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Dwell {
                at,
                until,
                activity,
            } => {
                let facts = &trace.activities[activity];
                let by_app = taught_apps
                    .iter()
                    .any(|(taught_at, app)| taught_at < at && facts.app == *app);
                let by_site = facts.url.as_deref().is_some_and(|url| {
                    taught_hosts
                        .iter()
                        .any(|(taught_at, host)| taught_at < at && host_of(url) == *host)
                });
                (by_app || by_site).then_some((*at, *until, activity.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// Migration 0045: a dwell a taught rule classifies shows `user_rule` on
/// every observation row that spans it. That includes a dwell whose evidence
/// matched the open row, so the row absorbed it and opened none for it
/// (Qwybex at t=3900 and the forum at t=12200, both inside a row a seed entry
/// opened). Without its rule Qwybex is `UNLOGGED` (block 1, t=600), so that
/// dwell would have been a switch away: a row that showed only `seed` would
/// hide the rule's effect on what the gate counted, which is what design
/// § 11.1 lock 3 has to expose for a synced rule. And no row shows
/// `user_rule` unless it spans such a dwell.
#[tokio::test]
async fn every_dwell_a_taught_rule_classifies_is_attributed_to_it() {
    let trace = load_trace();
    let provenance = replay(&trace).await.provenance;
    let observations = section(&provenance, "work_block_observation");
    let span = |row: &BTreeMap<&str, &str>| -> (i64, i64) {
        let occurred_at: i64 = row["occurred_at"].parse().unwrap();
        let ended_at: i64 = row["ended_at"].parse().unwrap_or(i64::MAX);
        (occurred_at, ended_at)
    };
    let overlaps = |row: &BTreeMap<&str, &str>, at: i64, until: i64| {
        let (occurred_at, ended_at) = span(row);
        occurred_at < stored_instant(&trace, until) && ended_at > stored_instant(&trace, at)
    };

    let taught = dwells_a_taught_rule_classifies(&trace);
    assert_eq!(
        taught.iter().map(|(at, ..)| *at).collect::<Vec<_>>(),
        vec![3900, 6340, 12200, 12335],
        "the trace no longer has the taught dwells this test was written for"
    );
    for (at, until, activity) in &taught {
        let spanning: Vec<_> = observations
            .iter()
            .filter(|row| overlaps(row, *at, *until))
            .collect();
        assert!(
            !spanning.is_empty(),
            "no observation spans {activity} at t={at}"
        );
        for row in spanning {
            assert_eq!(
                row["classification_source"], "'user_rule'",
                "{activity} at t={at}, which a taught rule classified, is recorded as {row:?}"
            );
        }
    }
    for row in observations
        .iter()
        .filter(|row| row["classification_source"] == "'user_rule'")
    {
        assert!(
            taught
                .iter()
                .any(|(at, until, _)| overlaps(row, *at, *until)),
            "{row:?} shows user_rule but spans no dwell a taught rule classified"
        );
    }
}

/// The re-record rule is enforced, not only written down: record mode
/// refuses to replace a golden recorded under the current policy version with
/// a different one unless a reason is given.
#[test]
fn record_mode_refuses_a_same_policy_rerecord_without_a_reason() {
    let current = format!("# golden, drift policy {DRIFT_POLICY_VERSION}\nrow=1\n");
    let changed = format!("# golden, drift policy {DRIFT_POLICY_VERSION}\nrow=2\n");
    let older = format!(
        "# golden, drift policy {}\nrow=1\n",
        DRIFT_POLICY_VERSION - 1
    );
    let unversioned = "# provenance\nrow=1\n";

    assert!(same_policy_rerecord_refusal("g", &current, &changed, None).is_some());
    assert!(same_policy_rerecord_refusal("g", &current, &changed, Some("  ")).is_some());
    assert!(same_policy_rerecord_refusal("g", &current, &current, None).is_none());
    assert!(
        same_policy_rerecord_refusal("g", &current, &changed, Some("parent #83 amendment"))
            .is_none()
    );
    assert!(same_policy_rerecord_refusal("g", &older, &changed, None).is_none());
    assert!(
        same_policy_rerecord_refusal("g", unversioned, "# provenance\nrow=2\n", None).is_none()
    );
}

/// Migration 0045: no decision counts any synced-rule evidence. Nothing can
/// classify by a synced rule while the rung that would is not compiled in, so
/// the column is 0 on every row the gate logged.
#[tokio::test]
async fn no_decision_counts_synced_rule_evidence() {
    let provenance = replay(&load_trace()).await.provenance;
    let decisions = section(&provenance, "intervention_decision_log");
    assert!(!decisions.is_empty());
    assert!(
        decisions
            .iter()
            .all(|row| row["synced_rule_evidence"] == "0"),
        "{decisions:?}"
    );
}

/// Fails with the first lines that differ, rather than two whole dumps.
fn assert_same_dump(what: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let differences: Vec<String> = (0..expected_lines.len().max(actual_lines.len()))
        .filter(|&index| expected_lines.get(index) != actual_lines.get(index))
        .take(12)
        .map(|index| {
            format!(
                "line {}:\n  golden:   {}\n  replayed: {}",
                index + 1,
                expected_lines.get(index).unwrap_or(&"<none>"),
                actual_lines.get(index).unwrap_or(&"<none>")
            )
        })
        .collect();
    panic!("{what} differs:\n{}", differences.join("\n"));
}

/// Attention plan § 3.3 and § 4.3: the question after a block moves no
/// decision. The trace replayed after the Mac's first 14 days, with no
/// question and then with the question on and every kind of row recorded,
/// reproduces `decisions.golden` and `provenance.golden` byte for byte: the
/// pushes, every offer, every logged decision and every observation, with
/// their sources.
#[tokio::test]
async fn the_question_after_a_block_moves_no_decision() {
    let trace = load_trace();
    let plain = replay(&trace).await;
    let without = replay_arm(&trace, Arm::PastDay14).await;
    let with = replay_arm(&trace, Arm::PastDay14WithQuestion).await;
    if std::env::var_os(RECORD_ENV).is_none() {
        let golden = |name: &str| std::fs::read_to_string(fixtures_dir().join(name)).unwrap();
        assert_same_dump(
            DECISIONS_GOLDEN,
            &golden(DECISIONS_GOLDEN),
            &plain.decisions,
        );
        assert_same_dump(
            PROVENANCE_GOLDEN,
            &golden(PROVENANCE_GOLDEN),
            &plain.provenance,
        );
    }
    for (arm, replayed) in [("after day 14", &without), ("with the question", &with)] {
        assert_same_dump(
            &format!("{DECISIONS_GOLDEN}, replayed {arm}"),
            &plain.decisions,
            &replayed.decisions,
        );
        assert_same_dump(
            &format!("{PROVENANCE_GOLDEN}, replayed {arm}"),
            &plain.provenance,
            &replayed.provenance,
        );
    }
    assert!(without.ratings.is_empty() && without.asked == 0);
}

/// The comparison above would also hold for a question that never asked.
/// It asks: every one of the six blocks is eligible, and the question's arm
/// holds every kind of row, each expiry at its deadline (the next block's
/// start), not when the service noticed.
#[tokio::test]
async fn the_question_s_arm_records_every_kind_of_row() {
    let trace = load_trace();
    let with = replay_arm(&trace, Arm::PastDay14WithQuestion).await;
    assert!(
        with.asked >= 5,
        "{} snapshots carried the question",
        with.asked
    );
    let rows: Vec<BTreeMap<&str, &str>> = with
        .ratings
        .iter()
        .map(|line| {
            line.split(" | ")
                .map(|field| field.split_once('=').unwrap())
                .collect()
        })
        .collect();
    let starts: Vec<i64> = trace
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Start { at, .. } => Some(*at),
            _ => None,
        })
        .collect();
    let stored = |offset: i64| (trace.origin + offset).to_string();
    let field = |block: usize, column: &str| -> String {
        let id = format!("'block-{block}'");
        let row = rows
            .iter()
            .find(|row| row["block_id"] == id)
            .unwrap_or_else(|| panic!("no row for block {block}: {rows:?}"));
        row[column].to_owned()
    };
    assert_eq!(rows.len(), 6, "{rows:?}");
    // Block 1: answered straight away.
    assert_eq!(field(1, "answer"), "'most'");
    assert_eq!(field(1, "card_seen_before_answer"), "0");
    // Block 2: Not now, then answered through the link.
    assert_eq!(field(2, "answer"), "'half'");
    assert_eq!(field(2, "card_seen_before_answer"), "1");
    assert_ne!(field(2, "not_now_at"), "NULL");
    // Block 3: seen, never answered, expired by block 4's start.
    assert_ne!(field(3, "shown_at"), "NULL");
    assert_eq!(field(3, "expired_at"), stored(starts[3]));
    // Block 4: never shown, expired by block 5's start.
    assert_eq!(field(4, "shown_at"), "NULL");
    assert_eq!(field(4, "expired_at"), stored(starts[4]));
    // Block 5: hidden by off and on again, never shown, expired by block 6.
    assert_eq!(field(5, "shown_at"), "NULL");
    assert_eq!(field(5, "answer"), "NULL");
    assert_eq!(field(5, "expired_at"), stored(starts[5]));
    // Block 6: answered.
    assert_eq!(field(6, "answer"), "'little'");
}
