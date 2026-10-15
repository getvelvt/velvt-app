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
//! recorded under.

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
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::retention::{InterventionDecisionOutcomeTarget, RetentionTarget};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{WorkBlockManager, DRIFT_POLICY_VERSION};
use velvt_shared_types::{
    ClientMessage, InterventionResponse, RawEvent, RawEventAck, RawEventStatus, ServerMessage,
    SetApplicationCategory, SetSiteCategory, StartWorkBlock, WorkBlockIntensity, WorkBlockPurpose,
};

const TRACE: &str = "trace.json";
const DECISIONS_GOLDEN: &str = "decisions.golden";
const RECORD_ENV: &str = "VELVT_GOLDEN_REPLAY_RECORD";

/// The tables the comparison reads, in the order the golden lists them.
const TABLES: [&str; 3] = [
    "work_block_intervention",
    "intervention_decision_log",
    "work_block_observation",
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
// The service, wired as main.rs wires the parts this trace reaches
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
}

fn service(scratch: &ScratchDatabase) -> Service {
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

    let manager = Arc::new(WorkBlockManager::new(persistence.work_block_repo()));
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
    .with_work_blocks(manager.clone(), PushAdapter::new(pushes.clone()));
    Service {
        persistence,
        router,
        manager,
        pushes,
    }
}

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
                Step::End { .. } => {
                    manager
                        .end(self.block(step), at)
                        .unwrap_or_else(|error| panic!("step {index}: {error:?}"));
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
struct Replayed {
    decisions: String,
}

async fn replay(trace: &Trace) -> Replayed {
    let scratch = ScratchDatabase::new();
    let mut replayer = Replayer {
        trace,
        service: service(&scratch),
        block: None,
        pending: Vec::new(),
        pushes: Vec::new(),
    };
    replayer.run().await;
    let connection = Connection::open(&scratch.path).unwrap();
    Replayed {
        decisions: decisions_dump(&connection, &replayer.pushes),
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
        let columns = column_names(connection, table);
        out.push_str(&format!("\n[{table}]\n"));
        for line in table_lines(connection, table, &columns, &ids) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Compares `actual` with the committed golden, or writes it when recording.
fn check_golden(name: &str, actual: &str) {
    let path = fixtures_dir().join(name);
    if std::env::var_os(RECORD_ENV).is_some() {
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
    assert_eq!(first.decisions, second.decisions);
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
