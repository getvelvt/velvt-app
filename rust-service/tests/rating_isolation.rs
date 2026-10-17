//! The question after a block never reaches a drift decision (attention plan
//! § 3.3 "Isolation" and § 4.3).
//!
//! Three checks, each failing on its own:
//!
//! 1. **The replay.** One synthetic trace of seventeen blocks over nineteen
//!    days runs through the real `WorkBlockManager` twice, on two databases.
//!    One arm has no question at all. The other attaches the question,
//!    turns it on after day 14 and, between blocks, records every kind of
//!    row: answered straight away, answered through the link after Not now,
//!    eligible and never shown, expired by the next block, expired while the
//!    Mac slept, and a block too short to ask about. It also turns the
//!    question off and on again after a block, and every report about that
//!    block is refused. The trace reaches offers,
//!    replies, backoff, a Focus hold, a pause, a restart and demotion. Every
//!    table except the question's own two must come out byte-identical, row
//!    by row and column by column as SQLite's `quote()` writes them, and every
//!    snapshot the manager returned must match once the question's two fields
//!    are removed. Block and decision ids are random, so each is replaced by
//!    its order of first appearance; a pushed offer's title and body are left
//!    out because their wording is chosen from the block id. Nothing else is
//!    normalized.
//! 2. **The hook.** A spy source records every call the manager makes to it.
//!    Driven through a block with an offer, a pause and an end, it is called
//!    only for the snapshot of an ended block.
//! 3. **The source.** The question's tables, store and manager are named only
//!    in the files that own, wire or route them, and inside `work_block` the
//!    source is called from one function, `rating_surfaces`. No drift,
//!    receipts, initiation, Focus, dashboard, category-prompt, behaviour,
//!    retention, upload or delivery module names them.
//!
//! What this does not cover: the classification ladder and the router's
//! ingestion path, which #90's `tests/golden_replay.rs` drives. That harness
//! is not on this branch. A question-on arm of it, with the trace moved past
//! the Mac's first 14 days and the question's messages sent through the
//! router between blocks, is added when this branch is rebased onto it
//! (plan § 3.3).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;
use uuid::Uuid;
use velvt_service::persistence::{DecisionFocusState, SqlitePersistence};
use velvt_service::self_rating::RatingPromptManager;
use velvt_service::work_block::{
    EndedBlock, FocusStateSource, RatingPromptSource, RatingSurfaces, WorkBlockManager,
};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, InterventionResponse, StartWorkBlock,
    WorkBlockIntensity, WorkBlockPhase, WorkBlockPurpose, WorkBlockRatingAnswer, WorkBlockSnapshot,
};

const DAY: i64 = 86_400;
const HOUR: i64 = 3_600;
/// The question's own tables, the only ones allowed to differ.
const RATING_TABLES: [&str; 2] = ["rating_prompt_settings", "work_block_self_rating"];
/// Written when a database is opened, before the trace begins: each install's
/// random salts, and the wall-clock time each migration was applied. They
/// differ between any two databases and no decision reads them.
const WRITTEN_AT_OPEN: [&str; 3] = ["embedding_salt", "schema_migration", "stable_key_salt"];

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap()
}

// ---------------------------------------------------------------------------
// 1. The replay
// ---------------------------------------------------------------------------

/// Focus on for fixed spans of the trace, the same in both arms.
struct ScriptedFocus(Vec<(i64, i64)>);

impl FocusStateSource for ScriptedFocus {
    fn is_focus_active(&self, at_time: DateTime<Utc>) -> bool {
        self.0
            .iter()
            .any(|(start, end)| at(*start) <= at_time && at_time < at(*end))
    }

    fn focus_state_at_decision(&self, at_time: DateTime<Utc>) -> DecisionFocusState {
        if self.is_focus_active(at_time) {
            DecisionFocusState::On
        } else {
            DecisionFocusState::Off
        }
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("velvt-rating-isolation-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn database(&self) -> PathBuf {
        self.0.join("velvt-service.sqlite3")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Arm {
    scratch: Scratch,
    manager: WorkBlockManager,
    rating: Option<Arc<RatingPromptManager>>,
    snapshots: Vec<WorkBlockSnapshot>,
}

impl Arm {
    fn new(with_question: bool, focus: Vec<(i64, i64)>) -> Self {
        let scratch = Scratch::new();
        let persistence = SqlitePersistence::open(scratch.database()).unwrap();
        let mut manager = WorkBlockManager::new(persistence.work_block_repo())
            .with_focus_source(Arc::new(ScriptedFocus(focus)) as Arc<dyn FocusStateSource>);
        let rating = with_question
            .then(|| Arc::new(RatingPromptManager::new(persistence.self_rating_repo())));
        if let Some(rating) = &rating {
            manager = manager.with_rating_prompt(Arc::clone(rating) as Arc<dyn RatingPromptSource>);
        }
        Self {
            scratch,
            manager,
            rating,
            snapshots: Vec::new(),
        }
    }

    fn keep(&mut self, snapshot: WorkBlockSnapshot) -> WorkBlockSnapshot {
        self.snapshots.push(snapshot.clone());
        snapshot
    }

    fn start(&mut self, start: i64, planned_seconds: u32) -> Uuid {
        let snapshot = self
            .manager
            .start(
                StartWorkBlock {
                    intention: Some("Write the report".into()),
                    planned_duration_seconds: planned_seconds,
                    purpose: Some(WorkBlockPurpose::DeepWork),
                    intensity: WorkBlockIntensity::Medium,
                    invitation_id: None,
                },
                at(start),
            )
            .unwrap();
        self.keep(snapshot).block_id.unwrap()
    }

    /// Reports each observation in turn, answering the offer with `reply`
    /// the moment one is delivered.
    fn observe(
        &mut self,
        block_id: Uuid,
        start: i64,
        observations: &[(i64, &str)],
        reply: Option<InterventionResponse>,
    ) {
        for (offset, category) in observations {
            let outcome = self
                .manager
                .observe_safe_category(
                    category,
                    ClassificationStatus::Classified,
                    ClassificationConfidence::High,
                    at(start + offset),
                )
                .unwrap();
            if let Some(outcome) = outcome {
                let offered = outcome.intervention.is_some();
                self.keep(outcome.snapshot);
                if let (true, Some(response)) = (offered, reply) {
                    let answered = self
                        .manager
                        .report_intervention_outcome(block_id, response, at(start + offset + 5))
                        .unwrap();
                    self.keep(answered);
                }
            }
        }
    }

    fn end(&mut self, block_id: Uuid, at_offset: i64) {
        let snapshot = self.manager.end(block_id, at(at_offset)).unwrap();
        self.keep(snapshot);
    }

    fn state(&mut self, at_offset: i64) -> WorkBlockSnapshot {
        let snapshot = self.manager.request_state(at(at_offset)).unwrap();
        self.keep(snapshot)
    }

    fn pause(&mut self, block_id: Uuid, at_offset: i64) {
        let snapshot = self.manager.pause(block_id, at(at_offset)).unwrap();
        self.keep(snapshot);
    }

    fn resume(&mut self, block_id: Uuid, at_offset: i64) {
        let snapshot = self.manager.resume(block_id, at(at_offset)).unwrap();
        self.keep(snapshot);
    }

    fn restart(&mut self, at_offset: i64) {
        let snapshot = self.manager.recover_after_restart(at(at_offset)).unwrap();
        self.keep(snapshot);
    }

    // The question's side of the trace. A no-op on the arm without it.

    fn turn_on(&self, at_offset: i64) {
        if let Some(rating) = &self.rating {
            assert!(rating.set_enabled(true, at(at_offset)).unwrap().enabled);
        }
    }

    fn turn_off(&self, at_offset: i64) {
        if let Some(rating) = &self.rating {
            assert!(!rating.set_enabled(false, at(at_offset)).unwrap().enabled);
        }
    }

    /// Every report about `block_id` is refused.
    fn refused(&self, block_id: Uuid, at_offset: i64) {
        if let Some(rating) = &self.rating {
            let now = at(at_offset);
            assert!(!rating.record_shown(block_id, now).unwrap());
            assert!(!rating.record_not_now(block_id, now).unwrap());
            assert!(!rating
                .record_answer(block_id, WorkBlockRatingAnswer::All, false, now)
                .unwrap());
        }
    }

    fn shown(&self, block_id: Uuid, at_offset: i64) {
        if let Some(rating) = &self.rating {
            assert!(rating.record_shown(block_id, at(at_offset)).unwrap());
        }
    }

    fn not_now(&self, block_id: Uuid, at_offset: i64) {
        if let Some(rating) = &self.rating {
            assert!(rating.record_not_now(block_id, at(at_offset)).unwrap());
        }
    }

    fn answer(
        &self,
        block_id: Uuid,
        answer: WorkBlockRatingAnswer,
        via_link: bool,
        at_offset: i64,
    ) {
        if let Some(rating) = &self.rating {
            assert!(rating
                .record_answer(block_id, answer, via_link, at(at_offset))
                .unwrap());
        }
    }
}

/// Establishes FOCUS_WORK, then switches away and back inside ten minutes,
/// enough for the gate to offer, then returns to the anchor.
const DRIFT: [(i64, &str); 9] = [
    (10, "FOCUS_WORK"),
    (400, "COMMUNICATION"),
    (420, "FOCUS_WORK"),
    (440, "COMMUNICATION"),
    (460, "FOCUS_WORK"),
    (480, "COMMUNICATION"),
    (500, "FOCUS_WORK"),
    (520, "SOCIAL_FEED"),
    (900, "FOCUS_WORK"),
];

/// The trace. Every call is made on both arms; the question's calls do
/// nothing on the arm without it.
fn run(arm: &mut Arm) {
    // Day 0: the Mac's first block starts its primary period. A drift, a
    // "not helpful" reply, and an early end.
    let b1 = arm.start(0, 1_800);
    arm.observe(b1, 0, &DRIFT, Some(InterventionResponse::NotHelpful));
    arm.end(b1, 1_200);

    // Day 2: a quiet block that completes on its deadline.
    let start = 2 * DAY;
    let b2 = arm.start(start, 1_500);
    arm.observe(
        b2,
        start,
        &[(10, "FOCUS_WORK"), (300, "REFERENCE"), (600, "FOCUS_WORK")],
        None,
    );
    arm.state(start + 1_500);

    // Day 15: the question is turned on.
    arm.turn_on(15 * DAY);

    // Answered straight away, after a "was focused" reply.
    let start = 15 * DAY + HOUR;
    let b3 = arm.start(start, 1_800);
    arm.observe(b3, start, &DRIFT, Some(InterventionResponse::WasFocused));
    arm.state(start + 1_800);
    arm.shown(b3, start + 1_815);
    arm.answer(b3, WorkBlockRatingAnswer::All, false, start + 1_830);

    // Too short to ask about.
    let start = 15 * DAY + 3 * HOUR;
    let b4 = arm.start(start, 1_800);
    arm.observe(
        b4,
        start,
        &[(10, "FOCUS_WORK"), (60, "COMMUNICATION")],
        None,
    );
    arm.end(b4, start + 240);
    arm.state(start + 250);

    // Paused and resumed, the offer closed by a return; Not now, then an
    // answer through the link.
    let start = 15 * DAY + 5 * HOUR;
    let b5 = arm.start(start, 3_600);
    arm.observe(b5, start, &DRIFT, None);
    arm.pause(b5, start + 1_000);
    arm.resume(b5, start + 1_300);
    arm.observe(
        b5,
        start,
        &[(1_400, "REFERENCE"), (1_700, "FOCUS_WORK")],
        None,
    );
    arm.end(b5, start + 2_400);
    arm.shown(b5, start + 2_410);
    arm.not_now(b5, start + 2_420);
    arm.state(start + 2_500);
    arm.answer(b5, WorkBlockRatingAnswer::Most, true, start + 3_000);

    // Focus on for the whole block: the offer is held. Eligible and never
    // shown, then the next block's start expires it, as it does the next
    // block's own question.
    let b6_start = 15 * DAY + 8 * HOUR;
    let b6 = arm.start(b6_start, 1_800);
    arm.observe(b6, b6_start, &DRIFT, None);
    arm.state(b6_start + 1_800);
    // Off and on again: b6's question stays hidden, and every report about
    // it is refused.
    arm.turn_off(b6_start + 1_860);
    arm.turn_on(b6_start + 1_920);
    arm.state(b6_start + 1_930);
    arm.refused(b6, b6_start + 1_940);

    let start = b6_start + 1_800 + 1_800;
    let b7 = arm.start(start, 1_800);
    arm.observe(b7, start, &DRIFT, Some(InterventionResponse::NotHelpful));
    arm.end(b7, start + 1_000);

    // Inside the cooldown the "not helpful" reply started.
    let start = start + 1_000 + 600;
    let b8 = arm.start(start, 1_800);
    arm.observe(b8, start, &DRIFT, None);
    arm.state(start + 1_800);
    arm.shown(b8, start + 1_810);
    arm.answer(b8, WorkBlockRatingAnswer::Little, false, start + 1_820);

    // Day 16: the service stops mid-block and restarts three hours after the
    // deadline. The block expires, and so does its question, unseen.
    let start = 16 * DAY;
    let b9 = arm.start(start, 1_800);
    arm.observe(
        b9,
        start,
        &[(10, "FOCUS_WORK"), (400, "COMMUNICATION")],
        None,
    );
    arm.restart(start + 1_800 + 3 * HOUR);

    // Days 17 to 19: offers answered "was focused" often enough to demote
    // the detector, each block's question answered in turn.
    let answers = WorkBlockRatingAnswer::ALL;
    for index in 0..8_i64 {
        let start = 17 * DAY + index * 6 * HOUR;
        let block = arm.start(start, 1_800);
        let reply = if index % 2 == 0 {
            InterventionResponse::WasFocused
        } else {
            InterventionResponse::AcceptedAction
        };
        arm.observe(block, start, &DRIFT, Some(reply));
        arm.state(start + 1_800);
        arm.shown(block, start + 1_805);
        arm.answer(
            block,
            answers[usize::try_from(index).unwrap() % answers.len()],
            false,
            start + 1_810,
        );
    }
    arm.state(19 * DAY + 12 * HOUR);
}

fn focus_spans() -> Vec<(i64, i64)> {
    let b6_start = 15 * DAY + 8 * HOUR;
    vec![(b6_start, b6_start + 1_800)]
}

/// Every table but the question's, each row as `quote()` writes it, with ids
/// replaced by their order of first appearance, rows sorted per table.
fn dump(path: &Path) -> BTreeMap<String, Vec<String>> {
    let connection = Connection::open(path).unwrap();
    let ids = id_ordinals(&connection);
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut dumped = BTreeMap::new();
    for table in tables {
        if RATING_TABLES.contains(&table.as_str()) || WRITTEN_AT_OPEN.contains(&table.as_str()) {
            continue;
        }
        let columns: Vec<(String, Option<String>)> = connection
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // A column SQLite fills from its own clock when a row is inserted
        // (`DEFAULT (unixepoch())`) records when the test ran, not anything
        // the trace did; its value is masked, and its presence still compared.
        let select = columns
            .iter()
            .map(|(column, default)| {
                let clock = default.as_deref().is_some_and(|default| {
                    default.contains("unixepoch") || default.contains("'now'")
                });
                if clock {
                    format!("'{column}=<clock>'")
                } else {
                    format!("'{column}=' || quote(\"{column}\")")
                }
            })
            .collect::<Vec<_>>()
            .join(" || ' ' || ");
        let mut rows: Vec<String> = connection
            .prepare(&format!("SELECT {select} FROM \"{table}\""))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| normalize(&row.unwrap(), &ids))
            .collect();
        rows.sort();
        dumped.insert(table, rows);
    }
    dumped
}

/// Block ids and decision ids, in the order they were created.
fn id_ordinals(connection: &Connection) -> Vec<(String, String)> {
    let mut ids = Vec::new();
    for (query, prefix) in [
        ("SELECT block_id FROM work_block ORDER BY rowid", "block"),
        (
            "SELECT decision_id FROM intervention_decision_log ORDER BY rowid",
            "decision",
        ),
    ] {
        let found: Vec<String> = connection
            .prepare(query)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        ids.extend(
            found
                .into_iter()
                .enumerate()
                .map(|(index, id)| (id, format!("<{prefix}-{index}>"))),
        );
    }
    ids
}

fn normalize(text: &str, ids: &[(String, String)]) -> String {
    ids.iter().fold(text.to_owned(), |text, (id, ordinal)| {
        text.replace(id, ordinal)
    })
}

/// A snapshot as the arm without the question would have produced it.
fn without_the_question(snapshot: &WorkBlockSnapshot, ids: &[(String, String)]) -> String {
    let mut value = serde_json::to_value(snapshot).unwrap();
    let payload = value.as_object_mut().unwrap();
    payload.remove("rating_prompt");
    payload.remove("rating_opt_in");
    if let Some(Value::Object(offer)) = payload.get_mut("active_intervention") {
        offer.remove("title");
        offer.remove("body");
    }
    normalize(&value.to_string(), ids)
}

#[test]
fn every_drift_table_is_byte_identical_with_and_without_the_question() {
    let mut without = Arm::new(false, focus_spans());
    let mut with = Arm::new(true, focus_spans());
    run(&mut without);
    run(&mut with);

    let without_tables = dump(&without.scratch.database());
    let with_tables = dump(&with.scratch.database());
    assert_eq!(
        without_tables.keys().collect::<Vec<_>>(),
        with_tables.keys().collect::<Vec<_>>()
    );
    for (table, rows) in &without_tables {
        assert_eq!(
            rows, &with_tables[table],
            "`{table}` differs once the question is on and answered"
        );
    }

    let without_ids = id_ordinals(&Connection::open(without.scratch.database()).unwrap());
    let with_ids = id_ordinals(&Connection::open(with.scratch.database()).unwrap());
    assert_eq!(without.snapshots.len(), with.snapshots.len());
    for (index, (plain, asked)) in without.snapshots.iter().zip(&with.snapshots).enumerate() {
        assert_eq!(
            without_the_question(plain, &without_ids),
            without_the_question(asked, &with_ids),
            "snapshot {index} differs outside the question's fields"
        );
        assert_eq!(plain.rating_prompt, None);
        assert_eq!(plain.rating_opt_in, None);
    }

    assert_the_comparison_means_something(&without, &with, &without_tables);
}

/// Two empty records would also be equal. The trace has to reach the drift
/// machinery's every stateful path, and the question's arm has to hold every
/// kind of row, for the equality above to say anything.
fn assert_the_comparison_means_something(
    without: &Arm,
    with: &Arm,
    tables: &BTreeMap<String, Vec<String>>,
) {
    let verdicts: HashMap<String, usize> = tables["intervention_decision_log"]
        .iter()
        .filter_map(|row| {
            row.split(' ')
                .find_map(|cell| cell.strip_prefix("gate_verdict="))
                .map(str::to_owned)
        })
        .fold(HashMap::new(), |mut counts, verdict| {
            *counts.entry(verdict).or_default() += 1;
            counts
        });
    for verdict in [
        "'offered'",
        "'abstained_warmup'",
        "'abstained_backoff'",
        "'suppressed_dnd'",
        "'withheld_demotion'",
        "'abstained_block_cap'",
    ] {
        assert!(
            verdicts.contains_key(verdict),
            "the trace never reached {verdict}: {verdicts:?}"
        );
    }
    let outcomes = tables["work_block_intervention"].join("\n");
    for outcome in [
        "'not_helpful'",
        "'was_focused'",
        "'accepted_action'",
        "'returned'",
    ] {
        assert!(outcomes.contains(outcome), "no {outcome} offer: {outcomes}");
    }
    assert!(
        tables["intervention_demotion_state"]
            .iter()
            .any(|row| row.contains("state='demoted'")),
        "the detector never demoted: {:?}",
        tables["intervention_demotion_state"]
    );

    let count = |path: &Path, condition: &str| -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row(
                &format!("SELECT COUNT(*) FROM work_block_self_rating WHERE {condition}"),
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let asked = with.scratch.database();
    assert_eq!(count(&without.scratch.database(), "1"), 0);
    assert_eq!(
        count(&asked, "1"),
        14,
        "every block after day 15 but the short one"
    );
    assert_eq!(count(&asked, "answer IS NOT NULL"), 11);
    assert_eq!(count(&asked, "card_seen_before_answer = 1"), 1);
    assert_eq!(count(&asked, "not_now_at IS NOT NULL"), 1);
    // Two ended by the next block's start, one while the service was down.
    assert_eq!(
        count(&asked, "expired_at IS NOT NULL AND shown_at IS NULL"),
        3
    );
    assert!(with
        .snapshots
        .iter()
        .any(|snapshot| snapshot.rating_prompt.is_some()));
}

// ---------------------------------------------------------------------------
// 2. The hook
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Spy(Mutex<Vec<WorkBlockPhase>>);

impl RatingPromptSource for Spy {
    fn surfaces_for(&self, block: &EndedBlock, _now: DateTime<Utc>) -> RatingSurfaces {
        self.0.lock().unwrap().push(block.phase);
        RatingSurfaces::default()
    }
}

/// While a block runs the source is never called: not when it starts, not
/// on an observation, an offer, a reply, a pause or a resume. It is called
/// for the snapshot of the ended block only.
#[test]
fn the_question_is_consulted_only_for_an_ended_block() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let spy = Arc::new(Spy::default());
    let manager = WorkBlockManager::new(persistence.work_block_repo())
        .with_rating_prompt(Arc::clone(&spy) as Arc<dyn RatingPromptSource>);
    let block = manager
        .start(
            StartWorkBlock {
                intention: None,
                planned_duration_seconds: 1_800,
                purpose: None,
                intensity: WorkBlockIntensity::Medium,
                invitation_id: None,
            },
            at(0),
        )
        .unwrap()
        .block_id
        .unwrap();
    let mut offered = false;
    for (offset, category) in DRIFT {
        if let Some(outcome) = manager
            .observe_safe_category(
                category,
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                at(offset),
            )
            .unwrap()
        {
            if outcome.intervention.is_some() {
                offered = true;
                manager
                    .report_intervention_outcome(
                        block,
                        InterventionResponse::NotHelpful,
                        at(offset + 5),
                    )
                    .unwrap();
            }
        }
    }
    assert!(offered, "the drift never reached an offer");
    manager.pause(block, at(1_000)).unwrap();
    manager.resume(block, at(1_100)).unwrap();
    manager.request_state(at(1_200)).unwrap();
    assert!(
        spy.0.lock().unwrap().is_empty(),
        "consulted while the block ran: {:?}",
        spy.0.lock().unwrap()
    );

    manager.end(block, at(1_300)).unwrap();
    manager.request_state(at(1_400)).unwrap();
    assert_eq!(
        *spy.0.lock().unwrap(),
        [WorkBlockPhase::Abandoned, WorkBlockPhase::Abandoned]
    );
}

// ---------------------------------------------------------------------------
// 3. The source
// ---------------------------------------------------------------------------

/// Names that reach the question's record.
const RATING_NAMES: [&str; 7] = [
    "work_block_self_rating",
    "rating_prompt_settings",
    "SelfRatingRepo",
    "self_rating_repo",
    "RatingPromptManager",
    "RatingPromptSource",
    "self_rating::",
];

/// The only files that may name it, each for its stated reason.
const ALLOWED: [(&str, &str); 8] = [
    ("src/self_rating/mod.rs", "the question's rules"),
    ("src/persistence/self_rating.rs", "the question's store"),
    ("src/persistence/mod.rs", "the store's export"),
    ("src/persistence/sqlite.rs", "the store's constructor"),
    (
        "src/work_block/mod.rs",
        "the hook, called from `rating_surfaces` only",
    ),
    ("src/ipc/router.rs", "the protocol 38 messages"),
    ("src/main.rs", "wiring"),
    ("src/lib.rs", "the module declaration"),
];

fn source_files(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_the_question_s_own_files_name_its_record() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    source_files(&root.join("src"), &mut files);
    assert!(files.len() > 40, "found only {} source files", files.len());
    let mut strays = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        // The log name of the protocol 38 reply is spelled like the table.
        let text = std::fs::read_to_string(&file).unwrap().replace(
            r#"ServerMessage::RatingPromptSettings(_) => "rating_prompt_settings""#,
            "",
        );
        let named: Vec<&str> = RATING_NAMES
            .into_iter()
            .filter(|name| text.contains(name))
            .collect();
        if !named.is_empty() && !ALLOWED.iter().any(|(allowed, _)| *allowed == relative) {
            strays.push(format!("{relative} names {named:?}"));
        }
    }
    assert!(
        strays.is_empty(),
        "the question's record is named outside its own files: {strays:?}"
    );
}

/// Inside `work_block`, the manager reaches its source from one function,
/// and only the snapshot builder calls that function.
#[test]
fn the_work_block_manager_calls_the_question_from_one_place() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/work_block/mod.rs"),
    )
    .unwrap();
    let body = function_body(&text, "fn rating_surfaces(");
    // The field itself, not `self.rating_surfaces`.
    let field_uses: Vec<usize> = text
        .match_indices("self.rating")
        .map(|(index, _)| index)
        .filter(|index| {
            !text[index + "self.rating".len()..]
                .starts_with(|next: char| next.is_alphanumeric() || next == '_')
        })
        .collect();
    assert!(field_uses.iter().any(|index| body.contains(index)));
    for index in field_uses {
        let is_the_builder = text[index..].starts_with("self.rating = Some(rating)");
        assert!(
            is_the_builder || body.contains(&index),
            "`self.rating` is used outside `rating_surfaces`: {:?}",
            &text[index.saturating_sub(80)..index + 40]
        );
    }
    let callers: Vec<usize> = text
        .match_indices("self.rating_surfaces(")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        callers.len(),
        1,
        "rating_surfaces has {} callers",
        callers.len()
    );
    assert!(
        function_body(&text, "fn snapshot_observed(").contains(&callers[0]),
        "rating_surfaces is called outside the snapshot builder"
    );
    for (index, _) in text.match_indices(".surfaces_for(") {
        assert!(
            body.contains(&index),
            "the source is called outside `rating_surfaces`"
        );
    }
}

/// The byte range of the function whose signature starts with `signature`,
/// from the signature to the closing brace at its own indentation.
fn function_body(text: &str, signature: &str) -> std::ops::Range<usize> {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("no `{signature}` in work_block"));
    let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
    let indent = &text[line_start..start];
    let indent = &indent[..indent.len() - indent.trim_start().len()];
    let closing = format!("\n{indent}}}\n");
    let end = text[start..]
        .find(&closing)
        .map(|offset| start + offset + closing.len())
        .unwrap_or_else(|| panic!("`{signature}` has no closing brace"));
    start..end
}
