//! Replays SYNTHETIC behavioural traces through the shipped drift gate.
//!
//! The fixtures come from `scripts/generate_traces.py` and are replayed here
//! through the real `WorkBlockManager::observe_safe_category`, against a fresh
//! `SqlitePersistence::open_in_memory()` per trace. Nothing is stubbed and
//! nothing is written below the ingestion layer, so what these tests score is
//! the gate that ships.
//!
//! Two suites, answering different questions.
//!
//! **A — recovery.** Hand-authored patterns with known ground truth: planted
//! drift the gate must detect, and near-misses one step outside each of its
//! four thresholds where it must abstain. The negatives are the load-bearing
//! half — a suite of positives alone is passed by a gate that always fires.
//!
//! **B — null.** Pure noise, 100 traces. **Acceptance: zero offers.** An
//! engine that reports findings to a user whose data contains nothing is not
//! buggy, it is lying, and this is the test that would catch it.
//!
//! B ships a second arm with the same structureless generator and a
//! compressed dwell distribution, which must produce *some* offers. Without
//! it, the zero above is unfalsifiable: a harness that never reached the gate
//! would report zero just as convincingly.
//!
//! **E — the return ledger.** Multi-week synthetic people, replayed the same
//! way, then read back out of the database exactly as stored and handed to the
//! shadow ledger in `src/behavior/returns.rs`. Five families: PLANTED, NULL,
//! SPARSE, REGIME and CORRECTED. What is synthetic is the person; the decision
//! log, offers, block cap and backoff are the shipped gate's. A suite E number
//! is a claim about whether the ledger recovers a planted rate, never about a
//! person, and never about what a nudge does.
//!
//! ## The clock rule
//!
//! `append_observation` (`persistence/sqlite.rs`) persists the event's own
//! `occurred_at` and sets `updated_at = MAX(updated_at, occurred_at)`;
//! `effective_now(record, now) = now.max(record.updated_at)` is therefore a
//! monotonic floor, not a clock read. A trace whose offsets start after the
//! block start and increase monotonically is recorded at its true timestamps
//! at any replay speed. A backdated one collapses to a single instant, every
//! dwell computes to zero, no anchor is found, and the gate abstains — a
//! silent pass that exercised nothing.
//!
//! That is a source-derived claim, so this file confirms it directly rather
//! than assuming it: `observations_are_stored_at_the_timestamps_they_were_given`
//! reads the rows back, and `a_backdated_trace_collapses_to_a_single_instant`
//! shows the failure mode on an otherwise identical pattern.

// `behavior` is declared in `src/main.rs`, not `src/lib.rs`, so the ledger is
// included by path, as `behavior_segmentation.rs` and `behavior_antecedents.rs`
// include the models they validate. Its unit tests therefore run here too.
#[path = "../src/behavior/features.rs"]
mod features;
#[path = "../src/behavior/returns.rs"]
mod returns;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::OnceLock,
};

use chrono::{DateTime, Duration, Utc};
use serde::{de::DeserializeOwned, Deserialize};
use sha2::{Digest, Sha256};
use velvt_service::{
    persistence::{GateVerdict, SqlitePersistence, WorkBlockInterventionOutcome},
    work_block::WorkBlockManager,
};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, InterventionResponse, StartWorkBlock,
    WorkBlockIntensity, WorkBlockPurpose,
};

use returns::{
    context_of, departure_rows, Abstention, BlockEvidence, Cell, CensorReason, Controls, Direction,
    LedgerConfig, ReturnLedger, RowOutcome, Support,
};

const SUITE_A: &str = "SYNTHETIC-suite-a-recovery.jsonl";
const SUITE_B_NULL: &str = "SYNTHETIC-suite-b-null.jsonl";
const SUITE_B_COMPRESSED: &str = "SYNTHETIC-suite-b-null-compressed.jsonl";
const SUITE_E: &str = "SYNTHETIC-suite-e-returns.jsonl";
const MANIFEST: &str = "SYNTHETIC-manifest.json";

/// The shipped gate's switch threshold, restated from the fixture header's
/// `gate_constants` so the positivity check below has a number to hold the
/// ledger's counted rows under.
const DRIFT_MIN_SWITCHES: u32 = 3;

// ---------------------------------------------------------------------------
// Fixture shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Observation {
    t: i64,
    category: String,
    status: String,
    confidence: String,
}

#[derive(Debug, Deserialize)]
struct BlockSpec {
    planned_duration_seconds: u32,
    purpose: String,
    intensity: String,
    observations: Vec<Observation>,
    /// Suite E only: when the block starts, relative to the replay origin.
    /// Absent, blocks follow one another an hour apart.
    #[serde(default)]
    start_offset_seconds: Option<i64>,
    /// Suite E only: the reply given to an offer the moment it is made.
    #[serde(default)]
    reply_to_offer: Option<String>,
    /// Suite E only: every departure the generator planted, with its label.
    #[serde(default)]
    departures: Vec<PlantedDeparture>,
}

#[derive(Debug, Deserialize)]
struct PlantedDeparture {
    t: i64,
    cell: String,
    /// `returned`, `not_returned`, `censored` or `treated`.
    label: String,
}

#[derive(Debug, Deserialize)]
struct ReturnTrace {
    trace_id: String,
    family: String,
    blocks: Vec<BlockSpec>,
    truth: ReturnTruth,
}

#[derive(Debug, Deserialize)]
struct ReturnTruth {
    planted_cell: Option<String>,
    #[serde(default)]
    blocks_per_week: Option<usize>,
    /// Index of the first block after the change, for REGIME and CORRECTED.
    change_after_block: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct Trace {
    trace_id: String,
    family: String,
    /// One simulated user's declared blocks, in order. Replayed into a single
    /// database, because `backoff_state` and the demotion policy both read
    /// every earlier block and a one-block trace never reaches them.
    blocks: Vec<BlockSpec>,
    expect_offer: bool,
    expect_reason: String,
}

#[derive(Debug, Deserialize)]
struct Header {
    suite: String,
    acceptance: String,
    traces: usize,
    injection_method: String,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    sha256: String,
    traces: usize,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    generator: String,
    seed: i64,
    files: BTreeMap<String, ManifestEntry>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

fn traces_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("traces")
}

fn read_fixture(name: &str) -> String {
    let path = traces_dir().join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing trace fixture {}: {error}\n\
             Generate it with:  ./scripts/generate_traces.py",
            path.display()
        )
    })
}

fn sha256_hex(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn load_suite<T: DeserializeOwned>(name: &str) -> (Header, Vec<T>) {
    let body = read_fixture(name);
    let mut lines = body.lines();
    let header: Header =
        serde_json::from_str(lines.next().unwrap_or_else(|| {
            panic!("{name} is empty; the first line must be the header record")
        }))
        .unwrap_or_else(|error| panic!("{name}: unreadable header record: {error}"));

    let traces: Vec<T> = lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{name}: unreadable trace record: {error}"))
        })
        .collect();

    assert_eq!(
        header.traces,
        traces.len(),
        "{name}: the header claims {} traces and the file holds {}",
        header.traces,
        traces.len()
    );
    assert!(
        header.injection_method.contains("real ingestion path"),
        "{name}: the header must record how the traces were injected, because a \
         validation run that silently exercised nothing is worse than none"
    );
    (header, traces)
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

fn origin() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn status_of(value: &str) -> ClassificationStatus {
    match value {
        "classified" => ClassificationStatus::Classified,
        "ambiguous" => ClassificationStatus::Ambiguous,
        "unclassified" => ClassificationStatus::Unclassified,
        other => panic!("unknown classification status in fixture: {other}"),
    }
}

fn confidence_of(value: &str) -> ClassificationConfidence {
    match value {
        "high" => ClassificationConfidence::High,
        "medium" => ClassificationConfidence::Medium,
        "low" => ClassificationConfidence::Low,
        "none" => ClassificationConfidence::None,
        other => panic!("unknown classification confidence in fixture: {other}"),
    }
}

fn purpose_of(value: &str) -> WorkBlockPurpose {
    match value {
        "deep_work" => WorkBlockPurpose::DeepWork,
        "study" => WorkBlockPurpose::Study,
        "creative_practice" => WorkBlockPurpose::CreativePractice,
        "healthy_tech_use" => WorkBlockPurpose::HealthyTechUse,
        "work_life_boundary" => WorkBlockPurpose::WorkLifeBoundary,
        other => panic!("unknown purpose in fixture: {other}"),
    }
}

fn intensity_of(value: &str) -> WorkBlockIntensity {
    match value {
        "light" => WorkBlockIntensity::Light,
        "medium" => WorkBlockIntensity::Medium,
        "intense" => WorkBlockIntensity::Intense,
        other => panic!("unknown intensity in fixture: {other}"),
    }
}

fn response_of(value: &str) -> InterventionResponse {
    match value {
        "wrong_classification" => InterventionResponse::WrongClassification,
        "not_helpful" => InterventionResponse::NotHelpful,
        "was_focused" => InterventionResponse::WasFocused,
        "dismissed" => InterventionResponse::Dismissed,
        "accepted_action" => InterventionResponse::AcceptedAction,
        other => panic!("unknown reply in fixture: {other}"),
    }
}

struct Replayed {
    offers: usize,
    first_offer_at: Option<(usize, i64)>,
    blocks: usize,
    observations: usize,
    stored_occurred_at: Vec<i64>,
    /// Every block exactly as stored, for the return ledger.
    evidence: Vec<BlockEvidence>,
    /// When each block started, so a fixture offset can be matched to a row.
    started_at: Vec<DateTime<Utc>>,
}

/// An hour of undeclared time between one block ending and the next starting.
const GAP_BETWEEN_BLOCKS_SECONDS: i64 = 3_600;

/// One trace, one fresh in-memory database, every block in order.
///
/// The database has to be fresh per TRACE or the traces are not independent:
/// `backoff_state` and the demotion policy both read every earlier block in
/// the same database, so a shared database would let trace 7 silence trace 8
/// and the null suite's zero would be an artefact of the harness. Within a
/// trace the blocks deliberately DO share one database, because that is what a
/// real user's blocks do.
fn replay(trace: &Trace) -> Replayed {
    replay_blocks(&trace.trace_id, &trace.blocks)
}

fn replay_blocks(trace_id: &str, blocks: &[BlockSpec]) -> Replayed {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.work_block_repo();
    let manager = WorkBlockManager::new(repo.clone());

    let mut cursor = origin();
    let mut offers = 0usize;
    let mut first_offer_at = None;
    let mut observations = 0usize;
    let mut stored_occurred_at: Vec<i64> = Vec::new();
    let mut evidence: Vec<BlockEvidence> = Vec::new();
    let mut started_at: Vec<DateTime<Utc>> = Vec::new();
    let mut previous_end: Option<DateTime<Utc>> = None;

    for (index, spec) in blocks.iter().enumerate() {
        let start = match spec.start_offset_seconds {
            Some(offset) => {
                let start = origin() + Duration::seconds(offset);
                assert!(
                    previous_end.is_none_or(|end| start > end),
                    "{trace_id}: block {index} starts before the previous one ended"
                );
                start
            }
            None => cursor,
        };
        started_at.push(start);
        let snapshot = manager
            .start(
                StartWorkBlock {
                    // Never an intention: these fixtures must be publishable,
                    // and the intention field is the one free-form string in
                    // the whole work-block schema.
                    intention: None,
                    planned_duration_seconds: spec.planned_duration_seconds,
                    purpose: Some(purpose_of(&spec.purpose)),
                    intensity: intensity_of(&spec.intensity),
                    invitation_id: None,
                },
                start,
            )
            .unwrap_or_else(|error| panic!("{trace_id}: could not start block {index}: {error:?}"));
        let block_id = snapshot
            .block_id
            .unwrap_or_else(|| panic!("{trace_id}: an active block has no id"));

        let mut offers_this_block = 0usize;
        for observation in &spec.observations {
            observations += 1;
            let at = start + Duration::seconds(observation.t);
            let outcome = manager
                .observe_safe_category(
                    &observation.category,
                    status_of(&observation.status),
                    confidence_of(&observation.confidence),
                    at,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{trace_id}: observe_safe_category failed in block {index} at t={}: {error:?}",
                        observation.t
                    )
                });

            if let Some(intervention) = outcome.and_then(|outcome| outcome.intervention) {
                offers += 1;
                offers_this_block += 1;
                if first_offer_at.is_none() {
                    first_offer_at = Some((index, observation.t));
                }
                assert_eq!(
                    intervention.action_id, "protect_next_10",
                    "{trace_id}: the action registry is closed; nothing else may be offered"
                );
                if let Some(reply) = &spec.reply_to_offer {
                    manager
                        .report_intervention_outcome(block_id, response_of(reply), at)
                        .unwrap_or_else(|error| {
                            panic!("{trace_id}: could not reply in block {index}: {error:?}")
                        });
                }
            }
        }

        // The per-block cap is the denominator of the pre-registered primary
        // outcome, so it is asserted on every block of every trace rather than
        // in one test of its own.
        assert!(
            offers_this_block <= 1,
            "{trace_id}: block {index} delivered {offers_this_block} offers; the cap is one"
        );

        stored_occurred_at.extend(
            repo.observations(&block_id.to_string())
                .unwrap()
                .into_iter()
                .map(|observation| observation.occurred_at.timestamp()),
        );

        // End inside the planned window rather than at the deadline, so the
        // block closes as `completed` through the ordinary path instead of
        // racing the expiry branch.
        let last_offset = spec
            .observations
            .last()
            .map(|observation| observation.t)
            .unwrap_or(0);
        let end_at = start + Duration::seconds(last_offset + 1);
        manager
            .end(block_id, end_at)
            .unwrap_or_else(|error| panic!("{trace_id}: could not end block {index}: {error:?}"));

        // Read back exactly what the ledger would read on a Mac.
        let id = block_id.to_string();
        evidence.push(BlockEvidence {
            ended_at: repo.get(&id).unwrap().ended_at,
            observations: repo.observations(&id).unwrap(),
            decisions: repo.decisions(&id).unwrap(),
            intervention: repo.intervention(&id).unwrap(),
            category_corrections: repo.category_corrections(&id).unwrap().len(),
            block_id: id,
        });

        previous_end = Some(end_at);
        cursor = end_at + Duration::seconds(GAP_BETWEEN_BLOCKS_SECONDS);
    }

    Replayed {
        offers,
        first_offer_at,
        blocks: blocks.len(),
        observations,
        stored_occurred_at,
        evidence,
        started_at,
    }
}

// ---------------------------------------------------------------------------
// The fixtures are what the generator produced
// ---------------------------------------------------------------------------

/// A fixture edited by hand — or left stale after the generator changed —
/// would quietly change what every test below is asserting about. The digests
/// are recorded by the generator and checked here.
#[test]
fn fixtures_are_exactly_what_the_generator_produced() {
    let manifest: Manifest = serde_json::from_str(&read_fixture(MANIFEST)).unwrap();
    assert_eq!(manifest.generator, "scripts/generate_traces.py");
    assert!(
        manifest.seed > 0,
        "the seed must be recorded so results re-derive"
    );

    for name in [SUITE_A, SUITE_B_NULL, SUITE_B_COMPRESSED, SUITE_E] {
        let entry = manifest
            .files
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not in {MANIFEST}"));
        let body = read_fixture(name);
        assert_eq!(
            sha256_hex(&body),
            entry.sha256,
            "{name} does not match the digest in {MANIFEST}. Re-run \
             ./scripts/generate_traces.py rather than editing a fixture by hand."
        );
        assert_eq!(body.lines().count(), entry.traces + 1, "{name}: line count");
    }
}

// ---------------------------------------------------------------------------
// Suite A — recovery
// ---------------------------------------------------------------------------

#[test]
fn suite_a_recovers_planted_drift_and_abstains_on_every_near_miss() {
    let (header, traces) = load_suite::<Trace>(SUITE_A);
    assert!(header.suite.starts_with("A "), "{}", header.suite);
    assert!(header.acceptance.contains("expect_offer"));

    let mut failures: Vec<String> = Vec::new();
    let mut recovered = 0usize;
    let mut abstained = 0usize;

    for trace in &traces {
        let result = replay(trace);
        let offered = result.offers == 1;
        if offered == trace.expect_offer {
            if trace.expect_offer {
                recovered += 1;
            } else {
                abstained += 1;
            }
        } else if trace.expect_offer {
            failures.push(format!(
                "{}: planted drift was NOT recovered. Expected an offer because {}",
                trace.trace_id, trace.expect_reason
            ));
        } else {
            failures.push(format!(
                "{}: FALSE POSITIVE at t={:?}. The gate should have abstained because {}",
                trace.trace_id, result.first_offer_at, trace.expect_reason
            ));
        }
    }

    let planted = traces.iter().filter(|t| t.expect_offer).count();
    let negatives = traces.len() - planted;
    println!(
        "suite A: {recovered}/{planted} planted patterns recovered, \
         {abstained}/{negatives} near-misses correctly abstained"
    );

    assert!(
        planted >= 5,
        "too few planted patterns to be worth reporting"
    );
    assert!(
        negatives > planted,
        "the negatives must outnumber the positives, or a gate that always \
         fires would pass this suite"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Suite B — the null test. This is the credibility instrument.
// ---------------------------------------------------------------------------

/// Acceptance: zero offers across every null trace. Any offer here is a false
/// discovery — the gate claiming to have seen drift in data that contains no
/// structure at all.
#[test]
fn suite_b_null_traces_surface_zero_offers() {
    let (header, traces) = load_suite::<Trace>(SUITE_B_NULL);
    assert!(header.acceptance.contains("ZERO offers"));
    assert!(
        traces.len() >= 100,
        "the null suite needs at least 100 independent traces; it has {}",
        traces.len()
    );

    let mut offers = 0usize;
    let mut offending: Vec<String> = Vec::new();
    let mut observations = 0usize;
    let mut blocks = 0usize;

    for trace in &traces {
        assert_eq!(trace.family, "NULL");
        assert!(!trace.expect_offer, "{}: mislabelled", trace.trace_id);
        let result = replay(trace);
        offers += result.offers;
        observations += result.observations;
        blocks += result.blocks;
        if result.offers > 0 {
            offending.push(format!(
                "{} offered in block {:?}",
                trace.trace_id, result.first_offer_at
            ));
        }
    }

    println!(
        "suite B (null): {offers} offers surfaced across {} traces / {blocks} \
         declared blocks / {observations} observations",
        traces.len()
    );

    // Every observation is one evaluation of the gate. If that number is
    // small the zero above is weak evidence, so it is asserted rather than
    // merely printed.
    assert!(
        observations >= 2_000,
        "only {observations} evaluation points; the null result is too thin to \
         report. Raise --blocks-per-trace or --null-traces."
    );
    assert!(
        blocks >= 300,
        "only {blocks} declared blocks in the null suite"
    );

    assert_eq!(
        offers,
        0,
        "the gate surfaced {offers} offer(s) on pure noise: {}",
        offending.join(", ")
    );
}

/// The inversion that makes the zero above mean something.
///
/// Same generator, same absence of structure, dwell scaled down. If this also
/// reported zero, the honest reading of the previous test would be "the
/// harness never reached the gate", not "the gate is quiet on noise".
///
/// It is also the sharpest thing these fixtures say about the product: the
/// shipped gate is a threshold on switching RATE, and whether real users cross
/// it is an empirical question that only the cohort answers.
#[test]
fn suite_b_compressed_null_does_surface_offers_so_the_zero_is_falsifiable() {
    let (header, traces) = load_suite::<Trace>(SUITE_B_COMPRESSED);
    assert!(header.acceptance.contains("at least one offer"));

    let mut offers = 0usize;
    let mut traces_with_an_offer = 0usize;
    let mut blocks = 0usize;
    let mut observations = 0usize;

    for trace in &traces {
        assert_eq!(trace.family, "NULL_COMPRESSED");
        let result = replay(trace);
        offers += result.offers;
        blocks += result.blocks;
        observations += result.observations;
        if result.first_offer_at.is_some() {
            traces_with_an_offer += 1;
        }
    }

    println!(
        "suite B (null, compressed dwell): {offers} offers across {} traces / \
         {blocks} declared blocks / {observations} observations \
         ({traces_with_an_offer} traces produced at least one)",
        traces.len()
    );

    assert!(
        offers > 0,
        "compressed noise produced no offers either, so the null suite's zero \
         proves nothing about the gate — it may only prove the harness never \
         reached it"
    );
}

// ---------------------------------------------------------------------------
// The clock rule, confirmed rather than assumed
// ---------------------------------------------------------------------------

/// Confirms the source-derived claim the whole harness rests on: a monotone
/// trace is stored at its own timestamps, unmodified, through the real path.
#[test]
fn observations_are_stored_at_the_timestamps_they_were_given() {
    let (_header, traces) = load_suite::<Trace>(SUITE_A);
    let trace = traces
        .iter()
        .find(|trace| trace.trace_id == "A-PLANT-4SWITCH-FOCUS")
        .expect("the reference monotone trace");

    assert_eq!(trace.blocks.len(), 1, "suite A traces are single-block");
    let result = replay(trace);
    let expected: Vec<i64> = trace.blocks[0]
        .observations
        .iter()
        .map(|observation| origin().timestamp() + observation.t)
        .collect();

    assert_eq!(
        result.stored_occurred_at, expected,
        "observations were not stored at their own occurred_at. The harness's \
         replay speed would then be a hidden variable in every result above."
    );
}

/// The trap, demonstrated. `A-TRAP-BACKDATED` carries the same pattern as
/// `A-PLANT-4SWITCH-FOCUS` emitted in reverse. The monotonic floor pulls every
/// observation to the first instant, every dwell computes to zero, no anchor
/// is found, and the gate abstains — for a reason that has nothing to do with
/// the pattern. A harness that generated traces this way would report a
/// silent, meaningless pass.
#[test]
fn a_backdated_trace_collapses_to_a_single_instant() {
    let (_header, traces) = load_suite::<Trace>(SUITE_A);
    let planted = traces
        .iter()
        .find(|trace| trace.trace_id == "A-PLANT-4SWITCH-FOCUS")
        .expect("the planted trace");
    let trap = traces
        .iter()
        .find(|trace| trace.trace_id == "A-TRAP-BACKDATED")
        .expect("the trap trace");

    // Same observations, opposite order. If that ever stops being true the
    // comparison below stops meaning anything.
    let mut planted_pairs: Vec<(i64, &str)> = planted.blocks[0]
        .observations
        .iter()
        .map(|o| (o.t, o.category.as_str()))
        .collect();
    let mut trap_pairs: Vec<(i64, &str)> = trap.blocks[0]
        .observations
        .iter()
        .map(|o| (o.t, o.category.as_str()))
        .collect();
    planted_pairs.sort();
    trap_pairs.sort();
    assert_eq!(
        planted_pairs, trap_pairs,
        "the trap trace must be the planted one reordered, nothing else"
    );

    let planted_result = replay(planted);
    let trap_result = replay(trap);

    assert_eq!(planted_result.offers, 1, "the monotone pattern is detected");
    assert_eq!(
        trap_result.offers, 0,
        "the backdated pattern must not be detected"
    );

    let distinct: std::collections::BTreeSet<i64> =
        trap_result.stored_occurred_at.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        1,
        "the backdated trace should collapse to one instant, but landed on {} \
         distinct timestamps: {:?}",
        distinct.len(),
        distinct
    );
}

// ---------------------------------------------------------------------------
// Suite E — the return ledger
// ---------------------------------------------------------------------------

/// The fixture reads blocks at UTC, and the generator schedules them there.
const SUITE_E_UTC_OFFSET_SECONDS: i32 = 0;

struct ReplayedPerson {
    trace: ReturnTrace,
    replayed: Replayed,
}

/// Every suite E trace, replayed once per test binary. Replay is the slow
/// part and every test below reads the same rows, so they share them.
fn suite_e() -> &'static [ReplayedPerson] {
    static REPLAYED: OnceLock<Vec<ReplayedPerson>> = OnceLock::new();
    REPLAYED.get_or_init(|| {
        let (header, traces) = load_suite::<ReturnTrace>(SUITE_E);
        assert!(header.suite.starts_with("E "), "{}", header.suite);
        assert!(header.acceptance.contains("ZERO surfaced"));
        traces
            .into_iter()
            .map(|trace| {
                let replayed = replay_blocks(&trace.trace_id, &trace.blocks);
                ReplayedPerson { trace, replayed }
            })
            .collect()
    })
}

fn family<'a>(name: &str) -> Vec<&'a ReplayedPerson> {
    let found: Vec<&ReplayedPerson> = suite_e()
        .iter()
        .filter(|person| person.trace.family == name)
        .collect();
    assert!(!found.is_empty(), "suite E has no {name} traces");
    found
}

fn ledger_at(person: &ReplayedPerson, as_of: DateTime<Utc>, controls: Controls) -> ReturnLedger {
    let mut config = LedgerConfig::new(as_of, SUITE_E_UTC_OFFSET_SECONDS);
    config.controls = controls;
    ReturnLedger::build(&person.replayed.evidence, &config)
}

fn end_of(person: &ReplayedPerson, block: usize) -> DateTime<Utc> {
    person.replayed.evidence[block]
        .ended_at
        .expect("every replayed block is closed")
}

fn last_end(person: &ReplayedPerson) -> DateTime<Utc> {
    end_of(person, person.replayed.evidence.len() - 1)
}

fn communication_lower(ledger: &ReturnLedger) -> bool {
    ledger
        .surfaced()
        .contains(&(Cell::Communication, Direction::Lower))
}

fn percent(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f64 / total as f64
    }
}

/// The label function, end to end: every departure the ledger finds in rows
/// the shipped gate wrote carries exactly the label the generator planted for
/// it, in the cell it planted it in, and the ledger finds nothing that was not
/// planted. A planted departure it does not find must be one the gate logged
/// without an anchor, which in these traces only backoff does.
#[test]
fn suite_e_every_departure_carries_the_label_planted_for_it() {
    let mut matched: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unseen_in_backoff = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for person in suite_e() {
        for (index, (spec, block)) in person
            .trace
            .blocks
            .iter()
            .zip(&person.replayed.evidence)
            .enumerate()
        {
            let start = person.replayed.started_at[index];
            let rows: BTreeMap<i64, returns::DepartureRow> =
                departure_rows(block, SUITE_E_UTC_OFFSET_SECONDS)
                    .into_iter()
                    .map(|row| ((row.occurred_at - start).num_seconds(), row))
                    .collect();
            let planted: BTreeSet<i64> = spec.departures.iter().map(|d| d.t).collect();
            for t in rows.keys() {
                if !planted.contains(t) {
                    failures.push(format!(
                        "{} block {index}: a departure at t={t} was never planted",
                        person.trace.trace_id
                    ));
                }
            }
            for departure in &spec.departures {
                let Some(row) = rows.get(&departure.t) else {
                    let decision = block.decisions.iter().find(|decision| {
                        decision.occurred_at == start + Duration::seconds(departure.t)
                    });
                    if decision.is_some_and(|decision| {
                        decision.anchor_category.is_none()
                            && decision.gate_verdict == GateVerdict::AbstainedBackoff
                    }) {
                        unseen_in_backoff += 1;
                    } else {
                        failures.push(format!(
                            "{} block {index}: planted departure at t={} is missing: {:?}",
                            person.trace.trace_id,
                            departure.t,
                            decision.map(|d| d.gate_verdict)
                        ));
                    }
                    continue;
                };
                let found = match row.outcome {
                    RowOutcome::Resolved { returned: true, .. } => "returned",
                    RowOutcome::Resolved {
                        returned: false, ..
                    } => "not_returned",
                    RowOutcome::Censored(CensorReason::BlockEnded) => "censored",
                    RowOutcome::Censored(CensorReason::ObserverGap) => "observer_gap",
                    RowOutcome::Treated => "treated",
                };
                if found != departure.label || row.context.departure.id() != departure.cell {
                    failures.push(format!(
                        "{} block {index} t={}: planted {} in {}, ledger read {found} in {}",
                        person.trace.trace_id,
                        departure.t,
                        departure.label,
                        departure.cell,
                        row.context.departure.id()
                    ));
                }
                *matched.entry(found).or_default() += 1;
            }
        }
    }

    println!(
        "suite E labels: {matched:?} matched their planted label; {unseen_in_backoff} planted \
         departures fell in blocks the gate held in backoff and logged without an anchor"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for label in ["returned", "not_returned", "censored", "treated"] {
        assert!(
            matched.get(label).copied().unwrap_or(0) > 20,
            "too few `{label}` rows for the label check to mean anything: {matched:?}"
        );
    }
}

/// NULL: no structure. The ledger must surface nothing, on traces with enough
/// evidence that it had to try. The same traces with every control off must
/// surface something, or the zero proves only that the ledger never looked.
///
/// The declared per-person rate of a false surfacing is at most
/// `FAMILY_ALPHA * CONFIRMATION_ALPHA` = 0.02, before the within-block
/// weighting and the shrinkage make it smaller. The thresholds were frozen
/// after seeing this suite; one run afterwards on a fresh seed (777, 200 NULL
/// traces) surfaced a cell in 1/200, and the naive controls in 149/200.
#[test]
fn suite_e_null_surfaces_nothing_and_the_naive_controls_do() {
    let null = family("NULL");
    let mut surfaced = 0usize;
    let mut tested = 0usize;
    let mut naive_traces = 0usize;
    let mut naive_cells = 0usize;
    let mut offending: Vec<String> = Vec::new();

    for person in &null {
        let as_of = last_end(person);
        let ledger = ledger_at(person, as_of, Controls::shipped());
        if ledger.abstention.is_none() {
            tested += 1;
        }
        if !ledger.surfaced().is_empty() {
            surfaced += 1;
            offending.push(format!(
                "{}: {:?}",
                person.trace.trace_id,
                ledger.surfaced()
            ));
        }
        let naive = ledger_at(person, as_of, Controls::naive());
        if !naive.surfaced().is_empty() {
            naive_traces += 1;
            naive_cells += naive.surfaced().len();
        }
    }

    println!(
        "suite E NULL: {surfaced}/{} traces surfaced a cell with the shipped controls \
         ({tested} cleared the evidence floor); with every control off, {naive_traces}/{} \
         traces surfaced {naive_cells} cells",
        null.len(),
        null.len()
    );
    assert!(
        tested * 4 >= null.len() * 3,
        "only {tested}/{} NULL traces cleared the evidence floor; a zero from a ledger \
         that abstained is not a zero",
        null.len()
    );
    assert_eq!(
        surfaced,
        0,
        "false findings on pure noise: {}",
        offending.join(", ")
    );
    assert!(
        naive_traces * 4 >= null.len(),
        "the naive controls surfaced cells on only {naive_traces}/{} NULL traces; the \
         shipped zero is not shown to be the controls' doing",
        null.len()
    );
}

/// SPARSE: the planted structure is there, the evidence is not. Abstain, say
/// why, surface nothing, and never propose withholding.
#[test]
fn suite_e_sparse_abstains_with_a_stated_reason() {
    let sparse = family("SPARSE");
    let mut too_few_blocks = 0usize;
    let mut too_few_rows = 0usize;
    for person in &sparse {
        let ledger = ledger_at(person, last_end(person), Controls::shipped());
        match ledger.abstention {
            Some(Abstention::TooFewBlocks { .. }) => too_few_blocks += 1,
            Some(Abstention::TooFewResolvedRows { .. }) => too_few_rows += 1,
            None => panic!(
                "{}: the ledger answered on {} resolved rows from {} blocks",
                person.trace.trace_id,
                ledger.counts.resolved,
                ledger.counts.blocks_with_resolved_rows
            ),
        }
        assert!(ledger.surfaced().is_empty(), "{}", person.trace.trace_id);
        for block in &person.replayed.evidence {
            for decision in &block.decisions {
                if let Some(context) =
                    context_of(decision, &block.observations, SUITE_E_UTC_OFFSET_SECONDS)
                {
                    let candidate = ledger.would_withhold(&context);
                    assert!(candidate.abstained && !candidate.would_withhold);
                }
            }
        }
    }
    println!(
        "suite E SPARSE: {too_few_blocks} abstained on too few blocks, {too_few_rows} on too \
         few resolved rows, of {}",
        sparse.len()
    );
    assert!(
        too_few_blocks > 0 && too_few_rows > 0,
        "both abstention reasons are exercised"
    );
}

/// PLANTED: after a communication departure this synthetic person rarely
/// comes back on their own (0.25 against 0.70). The recovery at each volume is
/// reported, not chosen: it is the answer to "how much does a person have to
/// use Velvt before the ledger can say this". At the highest volume a strict
/// majority must find it, recovery must not fall as volume rises, and no trace
/// may find anything in the two dimensions nothing was planted in.
///
/// Recorded when the thresholds were frozen, on these fixtures: 3/12, 4/12 and
/// 7/12 at 3, 5 and 8 blocks a week. The thresholds were set while looking at
/// this suite, so it is not a clean test of them; one run on a fresh seed
/// (777, 40 traces a level) afterwards found 2/40, 16/40 and 28/40.
#[test]
fn suite_e_planted_is_found_at_volume_and_nothing_else_is() {
    let planted = family("PLANTED");
    let mut by_volume: BTreeMap<usize, (usize, usize, usize)> = BTreeMap::new();
    let mut statuses: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut spurious: Vec<String> = Vec::new();
    for person in &planted {
        assert_eq!(
            person.trace.truth.planted_cell.as_deref(),
            Some(Cell::Communication.id())
        );
        let volume = person
            .trace
            .truth
            .blocks_per_week
            .expect("PLANTED records its volume");
        let ledger = ledger_at(person, last_end(person), Controls::shipped());
        let entry = by_volume.entry(volume).or_default();
        entry.0 += 1;
        entry.1 += usize::from(ledger.abstention.is_none());
        entry.2 += usize::from(communication_lower(&ledger));
        let report = ledger.cell(Cell::Communication);
        *statuses
            .entry((volume, format!("{:?}", report.status)))
            .or_default() += 1;
        for (cell, direction) in ledger.surfaced() {
            if !matches!(
                cell,
                Cell::Communication | Cell::FeedsAndVideo | Cell::WorkAdjacent
            ) {
                spurious.push(format!(
                    "{}: {} {direction:?}",
                    person.trace.trace_id,
                    cell.id()
                ));
            }
        }
    }
    for (volume, (traces, answered, found)) in &by_volume {
        println!(
            "suite E PLANTED at {volume} blocks/week over the 28-day lookback: \
             communication-lower found in {found}/{traces} ({:.0}%), {answered} cleared the \
             evidence floor",
            percent(*found, *traces)
        );
    }
    println!("suite E PLANTED, the communication cell's status by volume: {statuses:?}");
    assert!(
        spurious.is_empty(),
        "cells nothing was planted in: {}",
        spurious.join(", ")
    );
    let (&top, &(traces, _, found)) = by_volume.iter().next_back().unwrap();
    assert!(
        found * 2 > traces,
        "at {top} blocks a week the planted cell was found in only {found}/{traces}"
    );
    let found_by_volume: Vec<usize> = by_volume.values().map(|(_, _, found)| *found).collect();
    assert!(
        found_by_volume.windows(2).all(|pair| pair[0] <= pair[1]),
        "recovery fell as volume rose: {by_volume:?}"
    );
}

/// REGIME: the pattern holds for four weeks and then stops. Found at the
/// change in a strict majority; gone, in every trace, once the 28-day lookback
/// has passed it. Recorded when frozen: 8/12 before, 0/12 after; on the fresh
/// seed, 32/40 and 0/40.
#[test]
fn suite_e_regime_is_found_before_the_change_and_retracted_after() {
    let regime = family("REGIME");
    let mut before = 0usize;
    let mut after = 0usize;
    for person in &regime {
        let change = person
            .trace
            .truth
            .change_after_block
            .expect("REGIME records its change");
        let at_change = ledger_at(person, end_of(person, change - 1), Controls::shipped());
        let at_end = ledger_at(person, last_end(person), Controls::shipped());
        before += usize::from(communication_lower(&at_change));
        after += usize::from(communication_lower(&at_end));
    }
    println!(
        "suite E REGIME: communication-lower found in {before}/{} at the change, still \
         surfaced in {after}/{} four weeks after it",
        regime.len(),
        regime.len()
    );
    assert!(
        before * 2 > regime.len(),
        "the pattern was found in only {before}/{} before the change",
        regime.len()
    );
    assert_eq!(
        after, 0,
        "a pattern that stopped four weeks ago is still surfaced"
    );
}

/// CORRECTED: halfway through, a correction moves a work tool out of
/// COMMUNICATION. Before it, the tool's quick returns hide how rarely chat
/// comes back; after it, the ledger reads the corrected rows. Nothing is
/// relabelled after the fact, and the blocks the person disputed are counted
/// and carry half weight.
#[test]
fn suite_e_corrected_follows_the_inputs_and_down_weights_disputes() {
    let corrected = family("CORRECTED");
    let mut before = 0usize;
    let mut after = 0usize;
    let mut disputed_total = 0usize;
    let mut communication_rate_fell = 0usize;
    for person in &corrected {
        let change = person
            .trace
            .truth
            .change_after_block
            .expect("CORRECTED records its change");
        let as_of = end_of(person, change - 1);
        let at_change = ledger_at(person, as_of, Controls::shipped());
        let at_end = ledger_at(person, last_end(person), Controls::shipped());
        before += usize::from(communication_lower(&at_change));
        after += usize::from(communication_lower(&at_end));

        // Disputes are read off the rows the gate wrote, not the fixture.
        let window_start = as_of - Duration::days(returns::LOOKBACK_DAYS);
        let expected_disputes = person
            .replayed
            .evidence
            .iter()
            .filter(|block| {
                block
                    .ended_at
                    .is_some_and(|end| end <= as_of && end > window_start)
            })
            .filter(|block| {
                block.intervention.as_ref().is_some_and(|offer| {
                    offer.outcome == WorkBlockInterventionOutcome::WrongClassification
                })
            })
            .count();
        assert_eq!(
            at_change.counts.disputed_blocks, expected_disputes,
            "{}",
            person.trace.trace_id
        );
        disputed_total += expected_disputes;
        assert!(
            at_change.baseline.effective_resolved < at_change.counts.resolved as f64,
            "{}: weighting did not reduce the effective count",
            person.trace.trace_id
        );
        let rate = |ledger: &ReturnLedger| {
            let estimate = ledger.cell(Cell::Communication).estimate;
            estimate.returned as f64 / estimate.resolved.max(1) as f64
        };
        communication_rate_fell += usize::from(rate(&at_end) < rate(&at_change));
    }
    println!(
        "suite E CORRECTED: communication-lower found in {before}/{n} before the correction \
         and {after}/{n} once corrected rows fill the lookback; the raw communication \
         return rate fell after the correction in {communication_rate_fell}/{n}; \
         {disputed_total} disputed blocks counted at half weight",
        n = corrected.len()
    );
    assert!(
        disputed_total > 0,
        "no block was disputed; the weighting went untested"
    );
    assert!(
        communication_rate_fell * 4 >= corrected.len() * 3,
        "the correction did not show up in the counts"
    );
    assert!(
        after > before,
        "the pattern the misfiled tool hid was not found once the correction flowed through"
    );
}

/// Positivity, checked on the rows rather than asserted in a comment. Under
/// v5 every counted row is a sub-threshold departure, and at every point the
/// gate offered the withhold candidate says it is extrapolating.
#[test]
fn suite_e_every_offered_point_is_outside_the_counted_rows() {
    let mut offered_points = 0usize;
    for person in suite_e() {
        let ledger = ledger_at(person, last_end(person), Controls::shipped());
        if let Some(most) = ledger.max_switch_count_counted {
            assert!(
                most < DRIFT_MIN_SWITCHES,
                "{}: a counted row had {most} switches; v5 offers at {DRIFT_MIN_SWITCHES}",
                person.trace.trace_id
            );
        }
        for block in &person.replayed.evidence {
            for decision in &block.decisions {
                if decision.gate_verdict != GateVerdict::Offered {
                    continue;
                }
                let context = context_of(decision, &block.observations, SUITE_E_UTC_OFFSET_SECONDS)
                    .expect("an offer is made on a departure");
                assert_eq!(
                    ledger.would_withhold(&context).support,
                    Support::Extrapolated,
                    "{}: the candidate claimed support at an offered point",
                    person.trace.trace_id
                );
                offered_points += 1;
            }
        }
    }
    println!("suite E positivity: {offered_points} offered points, every one extrapolated");
    assert!(offered_points > 50, "too few offers to check positivity on");
}

/// Replaying the same person into a fresh database, with fresh ids, gives the
/// same ledger bit for bit; and a ledger as of the change is unmoved by every
/// block that closed after it.
#[test]
fn suite_e_is_deterministic_and_as_of() {
    let person = family("REGIME")[0];
    let again = replay_blocks(&person.trace.trace_id, &person.trace.blocks);
    let as_of = last_end(person);
    let config = LedgerConfig::new(as_of, SUITE_E_UTC_OFFSET_SECONDS);
    assert_eq!(
        ReturnLedger::build(&person.replayed.evidence, &config),
        ReturnLedger::build(&again.evidence, &config),
        "the same trace replayed twice gave two ledgers"
    );

    let change = person.trace.truth.change_after_block.unwrap();
    let at_change = LedgerConfig::new(end_of(person, change - 1), SUITE_E_UTC_OFFSET_SECONDS);
    assert_eq!(
        ReturnLedger::build(&person.replayed.evidence, &at_change),
        ReturnLedger::build(&person.replayed.evidence[..change], &at_change),
        "blocks that closed after as_of changed the answer at as_of"
    );
}
