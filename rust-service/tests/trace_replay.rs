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

use std::{collections::BTreeMap, fs, path::PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use velvt_service::{persistence::SqlitePersistence, work_block::WorkBlockManager};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, StartWorkBlock, WorkBlockIntensity,
    WorkBlockPurpose,
};

const SUITE_A: &str = "SYNTHETIC-suite-a-recovery.jsonl";
const SUITE_B_NULL: &str = "SYNTHETIC-suite-b-null.jsonl";
const SUITE_B_COMPRESSED: &str = "SYNTHETIC-suite-b-null-compressed.jsonl";
const MANIFEST: &str = "SYNTHETIC-manifest.json";

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

fn load_suite(name: &str) -> (Header, Vec<Trace>) {
    let body = read_fixture(name);
    let mut lines = body.lines();
    let header: Header =
        serde_json::from_str(lines.next().unwrap_or_else(|| {
            panic!("{name} is empty; the first line must be the header record")
        }))
        .unwrap_or_else(|error| panic!("{name}: unreadable header record: {error}"));

    let traces: Vec<Trace> = lines
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

struct Replayed {
    offers: usize,
    first_offer_at: Option<(usize, i64)>,
    blocks: usize,
    observations: usize,
    stored_occurred_at: Vec<i64>,
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
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.work_block_repo();
    let manager = WorkBlockManager::new(repo.clone());

    let mut cursor = origin();
    let mut offers = 0usize;
    let mut first_offer_at = None;
    let mut observations = 0usize;
    let mut stored_occurred_at: Vec<i64> = Vec::new();

    for (index, spec) in trace.blocks.iter().enumerate() {
        let start = cursor;
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
            .unwrap_or_else(|error| {
                panic!(
                    "{}: could not start block {index}: {error:?}",
                    trace.trace_id
                )
            });
        let block_id = snapshot
            .block_id
            .unwrap_or_else(|| panic!("{}: an active block has no id", trace.trace_id));

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
                        "{}: observe_safe_category failed in block {index} at t={}: {error:?}",
                        trace.trace_id, observation.t
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
                    "{}: the action registry is closed; nothing else may be offered",
                    trace.trace_id
                );
            }
        }

        // The per-block cap is the denominator of the pre-registered primary
        // outcome, so it is asserted on every block of every trace rather than
        // in one test of its own.
        assert!(
            offers_this_block <= 1,
            "{}: block {index} delivered {offers_this_block} offers; the cap is one",
            trace.trace_id
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
        manager.end(block_id, end_at).unwrap_or_else(|error| {
            panic!("{}: could not end block {index}: {error:?}", trace.trace_id)
        });

        cursor = end_at + Duration::seconds(GAP_BETWEEN_BLOCKS_SECONDS);
    }

    Replayed {
        offers,
        first_offer_at,
        blocks: trace.blocks.len(),
        observations,
        stored_occurred_at,
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

    for name in [SUITE_A, SUITE_B_NULL, SUITE_B_COMPRESSED] {
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
    let (header, traces) = load_suite(SUITE_A);
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
    let (header, traces) = load_suite(SUITE_B_NULL);
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
    let (header, traces) = load_suite(SUITE_B_COMPRESSED);
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
    let (_header, traces) = load_suite(SUITE_A);
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
    let (_header, traces) = load_suite(SUITE_A);
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
