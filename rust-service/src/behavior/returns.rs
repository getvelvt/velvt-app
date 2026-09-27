//! The per-person return ledger. **Shadow only: nothing in the shipped path
//! calls it, and a test below fails the build if anything starts to.**
//!
//! # What it counts
//!
//! One row per *departure the drift gate saw and did not act on*: a decision
//! logged with an anchor, whose triggering observation is confident evidence
//! of a non-anchor category, and whose previous confident observation was the
//! anchor. That is `s_t` in [`super::features`], which is the gate's own
//! departure rule. The row's label is the pre-registered primary outcome,
//! computed the same way (`traction-summary.md`, 2026-08-21):
//!
//! - **returned**: at least [`SUSTAINED_ANCHOR_SECONDS`] of the next
//!   [`RETURN_HORIZON_SECONDS`] were spent in confident observations of the
//!   anchor *recorded on the decision row*. The anchor is never recomputed.
//! - **censored**, never imputed. Censored rows leave both numerator and
//!   denominator, and every reason is counted ([`CensorReason`]):
//!   - `block_ended`: the block ended inside the horizon;
//!   - `observer_gap`: the observation ledger does not cover all of it (a
//!     pause, a sleep, a service restart, or the dwell the block ended in,
//!     which no row measures);
//!   - `treated`: an offer, delivered or held, fell at or before the
//!     departure, or inside the horizon while the time before it still left
//!     the label open. What happens after an offer is not what happens on
//!     one's own, so the row is followed only up to the offer. If the time
//!     before the offer already decides the label (600 anchor seconds, or so
//!     few that the rest of the horizon could not make them up), the row is
//!     resolved on that time alone, because nothing after it could change the
//!     label.
//!
//! # The treated censoring is informative: expect the rates to read high
//!
//! v5 offers only on a departure that makes three inside ten minutes. So an
//! offer inside a departure's horizon means the person left again before the
//! fifteen minutes were up, as part of a run of departures, and a horizon
//! with a run in it usually holds less anchor time than one without. The rows
//! the offer censors are therefore most plausibly rows that were heading for
//! "did not return", and dropping them makes the rates here read **higher**
//! than the person's own rate, towards "came back on their own". The size
//! differs per cell, because offers follow some kinds of departure more than
//! others. The direction is not guaranteed: someone who darts away and
//! straight back three times may still have come back. Neither the size nor,
//! per cell, the sign can be estimated from v5 rows.
//! [`CellAccounting::censored_treated`] reports, per cell, how many
//! departures an offer censored, so a reader can see where the bias could be
//! large, and suite E's INFORMATIVE family shows it on planted rates.
//! [`ReturnLedger::would_withhold`] inherits it: a rate that reads high makes
//! a withhold rule fire more often than the person's own rate would.
//!
//! Resolved rows are tallied per person into a small frozen set of context
//! cells ([`RETURN_CELLS`]): which kind of category the departure went to, which third
//! of the block it happened in, and which part of the local day. Each cell is a
//! Beta-Binomial posterior shrunk towards the person's own overall rate, so the
//! default answer is "no difference from you in general".
//!
//! # What it cannot say, and why: positivity
//!
//! Drift policy v5 offers at every eligible point, with propensity 1.0. A
//! departure the gate left alone is therefore one the gate did not find
//! eligible, which in practice means the first or second switch inside ten
//! minutes. Every counted row comes from there, and [`ReturnLedger`] records
//! the largest `switch_count` it counted so that this is checked on real rows
//! rather than asserted. The held rows (Focus/DND, demotion) are the only v5
//! points with an eligible departure and no delivery; they are censored as
//! `treated` too, because the very states that held them also shape what the
//! person does next.
//!
//! So the ledger describes how often this person came back to their work on
//! their own after a departure the gate did not act on. It says nothing about
//! what a nudge does, and nothing about what silence would do at a point
//! where v5 offers. [`ReturnLedger::would_withhold`] is a candidate rule
//! declared before any data exists, for offline evaluation only. At an offered
//! point it is an extrapolation outside every counted row, and it reports
//! itself as one ([`WithholdSupport::Extrapolated`]). Whether withholding
//! helps can be estimated only from randomized rows: the declared v6 design
//! (offer with p = 0.7, silence with p = 0.3, drawn only when at least 900 s
//! remain), which is drafted and not enabled. Until those rows exist, nothing
//! here may reach the gate, delivery, IPC or any copy.
//!
//! # Determinism, and what "as of" means
//!
//! Pure functions over rows passed in. No database handle, no clock read, no
//! randomness, no stored state. Blocks are read only once closed, only if they
//! closed at or before `as_of`, and only if they closed inside the
//! [`LOOKBACK_DAYS`] before it; rows are visited in `(ended_at, block_id)`
//! order. A dispute counts only if it was recorded at or before `as_of`: a
//! "Wrong category" reply by its `outcome_at`, a correction by its
//! `corrected_at`. Both can be written after the block closed. So the same
//! rows give bit-identical output, and nothing recorded after `as_of` can
//! change an answer given at `as_of`.
//!
//! # Part of the day: one offset per block, when anyone knows it
//!
//! A block is read at its own [`BlockEvidence::utc_offset_seconds`] when the
//! caller has one, and at [`LedgerConfig::utc_offset_seconds`] otherwise. No
//! table stores a per-block offset today: `focus_observer_state` keeps only
//! the latest one. So a caller reading today's tables reads every block at
//! the current offset, and across a daylight-saving change or travel inside
//! the lookback, departures within the offset difference of 05:00, 12:00 or
//! 17:00 are filed in the neighbouring part of the day. Storing the offset per
//! block is a migration of its own. Until one exists, the hour cells are right
//! only for a person whose offset did not change in the 28 days read.
//!
//! # Privacy
//!
//! Everything it reads already exists on the Mac: `work_block_observation`,
//! `intervention_decision_log`, `work_block_intervention` and
//! `work_block_category_correction`. It writes nothing, so it adds no table,
//! no column and no retention window, and Clear Local Work Blocks deletes
//! every input it has. Its outputs are broad categories, block thirds, parts
//! of the day, counts and rates. No application, site, title or intention
//! text is read.
//!
//! `work_block_category_correction` has no production writer. Only the
//! persistence layer's own method writes it, and only tests call that method.
//! On a Mac the input is always empty, so a "Wrong category" reply is the only
//! dispute the ledger can see today.

// No caller in the shipped path, by design (module docs). The binary crate
// has no notion of a symbol public for someone else to use, so every item
// here would otherwise be dead code.
#![allow(dead_code)]

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Timelike, Utc};
use velvt_service::persistence::{
    GateVerdict, InterventionDecision, WorkBlockCategoryCorrection, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation,
};
use velvt_service::work_block::{is_confident, DRIFT_POLICY_VERSION};

use super::features::FEATURE_CONTRACT_VERSION;

/// Version of the ledger's rules: the cells, the label, the exclusions, the
/// priors and every threshold below. Stamped on every result beside
/// [`FEATURE_CONTRACT_VERSION`] and the drift policy version it read. A change
/// to any of them is a new version, and results under two versions are never
/// pooled.
pub const RETURN_LEDGER_MODEL_VERSION: u32 = 1;

/// The horizon of the pre-registered primary outcome.
pub const RETURN_HORIZON_SECONDS: i64 = 900;

/// Anchor seconds inside the horizon that make a departure a return.
pub const SUSTAINED_ANCHOR_SECONDS: i64 = 600;

// The primary outcome's horizon is deliberately longer than the 600-second
// proximal outcome stored on the decision row (`anchor_seen_within_600s`),
// which this ledger never reads: that column has no confidence filter and
// resolves a horizon past the block end to "did not return".
const _: () = assert!(RETURN_HORIZON_SECONDS > super::features::PROXIMAL_OUTCOME_HORIZON_SECONDS);

/// Only blocks that closed inside this many days before `as_of` are read. It
/// bounds how stale an answer can be after a person's routine changes; it adds
/// no retention, because the inputs live until Clear Local Work Blocks anyway.
pub const LOOKBACK_DAYS: i64 = 28;

/// Abstain below this many blocks contributing at least one resolved row.
pub const MIN_BLOCKS: usize = 6;

/// Abstain below this many resolved rows in the lookback.
pub const MIN_RESOLVED_ROWS: usize = 30;

/// A cell, and the rest of its dimension, is tested only with at least this
/// many resolved rows from at least [`MIN_CELL_BLOCKS`] blocks each, in the
/// window being tested.
pub const MIN_CELL_ROWS: usize = 8;
pub const MIN_CELL_BLOCKS: usize = 3;

/// The same floor for the held-out confirmation window, which is smaller.
pub const MIN_CONFIRMATION_ROWS: usize = 4;
pub const MIN_CONFIRMATION_BLOCKS: usize = 2;

/// Prior on a person's overall rate: Beta(4, 4), weak, centred on one half.
pub const BASELINE_PRIOR: (f64, f64) = (4.0, 4.0);

/// Pseudo-count strength of each cell's prior, centred on the person's own
/// overall rate. With it, a cell must earn a difference against four
/// imaginary departures that behaved like the person's average. Weak on
/// purpose: false findings are held off by the family-wise interval and the
/// held-out window, and a stronger prior only hides real differences at the
/// volumes a person produces in four weeks.
pub const CELL_PRIOR_STRENGTH: f64 = 4.0;

/// ASSUMPTION, not a measurement. Departures in one block are not independent:
/// a scattered afternoon scatters all of them. Rows from one block are weighted
/// by the Kish design effect `1 / (1 + (n_b - 1) * rho)` at this `rho`.
/// Revisit when real blocks exist.
pub const ASSUMED_WITHIN_BLOCK_CORRELATION: f64 = 0.3;

/// A block in which the person said the category was wrong ("Wrong category"
/// on the offer, or a block-scoped correction) counts at this weight. Its
/// categories are the thing disputed.
pub const DISPUTED_BLOCK_WEIGHT: f64 = 0.5;

/// Family-wise two-sided error rate over the whole cell family, Bonferroni
/// over [`RETURN_CELLS`]`.len()` contrasts. The family size is the compile-time
/// number of cells, never the number that happened to be testable.
pub const FAMILY_ALPHA: f64 = 0.20;

/// One-sided level at which the held-out window must repeat the direction.
pub const HELD_OUT_ALPHA: f64 = 0.10;

/// The later share of contributing blocks held out for confirmation, as a
/// fraction `numerator / denominator` of blocks, rounded down. The two windows
/// never share a block.
pub const CONFIRMATION_SHARE: (usize, usize) = (2, 5);

/// Mass of the interval reported on every rate.
pub const REPORTED_INTERVAL_MASS: f64 = 0.80;

/// The declared withhold candidate fires only where the lower end of the
/// reported interval, in every cell the point falls in, is at least this: the
/// person usually spends most of the next fifteen minutes back at their work
/// on their own after this kind of departure.
pub const WITHHOLD_LOWER_BOUND: f64 = 0.60;

// ---------------------------------------------------------------------------
// The frozen context cells
// ---------------------------------------------------------------------------

/// The three things a cell can describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CellDimension {
    /// Which kind of category the departure went to.
    Departure,
    /// Which third of the declared block the departure happened in.
    Elapsed,
    /// Which part of the local day.
    Hour,
}

/// One frozen context cell. Every resolved row falls in exactly one cell of
/// each dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReturnCell {
    /// COMMUNICATION.
    Communication,
    /// SOCIAL_FEED or PASSIVE_CONSUMPTION.
    FeedsAndVideo,
    /// REFERENCE, TASK_MANAGEMENT, or FOCUS_WORK that is not the anchor.
    WorkAdjacent,
    FirstThird,
    MiddleThird,
    FinalThird,
    /// Local 05:00-11:59.
    Morning,
    /// Local 12:00-16:59.
    Afternoon,
    /// Local 17:00-04:59.
    EveningAndNight,
}

/// The whole family, in reporting order. Adding, removing or redrawing a cell
/// is a [`RETURN_LEDGER_MODEL_VERSION`] change.
pub const RETURN_CELLS: [ReturnCell; 9] = [
    ReturnCell::Communication,
    ReturnCell::FeedsAndVideo,
    ReturnCell::WorkAdjacent,
    ReturnCell::FirstThird,
    ReturnCell::MiddleThird,
    ReturnCell::FinalThird,
    ReturnCell::Morning,
    ReturnCell::Afternoon,
    ReturnCell::EveningAndNight,
];

impl ReturnCell {
    /// Stable identifier for logs and offline analysis.
    pub fn id(self) -> &'static str {
        match self {
            Self::Communication => "departure.communication",
            Self::FeedsAndVideo => "departure.feeds_and_video",
            Self::WorkAdjacent => "departure.work_adjacent",
            Self::FirstThird => "elapsed.first_third",
            Self::MiddleThird => "elapsed.middle_third",
            Self::FinalThird => "elapsed.final_third",
            Self::Morning => "hour.morning",
            Self::Afternoon => "hour.afternoon",
            Self::EveningAndNight => "hour.evening_and_night",
        }
    }

    pub fn dimension(self) -> CellDimension {
        match self {
            Self::Communication | Self::FeedsAndVideo | Self::WorkAdjacent => {
                CellDimension::Departure
            }
            Self::FirstThird | Self::MiddleThird | Self::FinalThird => CellDimension::Elapsed,
            Self::Morning | Self::Afternoon | Self::EveningAndNight => CellDimension::Hour,
        }
    }
}

/// The departure cell for a confident category, or `None` for one that can
/// never be confident evidence (SYSTEM, UNLOGGED) or is outside the taxonomy.
pub fn departure_cell(category: &str) -> Option<ReturnCell> {
    match category.to_ascii_uppercase().as_str() {
        "COMMUNICATION" => Some(ReturnCell::Communication),
        "SOCIAL_FEED" | "PASSIVE_CONSUMPTION" => Some(ReturnCell::FeedsAndVideo),
        "REFERENCE" | "TASK_MANAGEMENT" | "FOCUS_WORK" => Some(ReturnCell::WorkAdjacent),
        _ => None,
    }
}

/// The block third from the decision row's own elapsed and remaining seconds.
/// `None` when the row carries no planned duration to divide by.
pub fn elapsed_cell(elapsed_seconds: u32, remaining_seconds: u32) -> Option<ReturnCell> {
    let planned = u64::from(elapsed_seconds) + u64::from(remaining_seconds);
    if planned == 0 {
        return None;
    }
    let scaled = 3 * u64::from(elapsed_seconds);
    Some(if scaled < planned {
        ReturnCell::FirstThird
    } else if scaled < 2 * planned {
        ReturnCell::MiddleThird
    } else {
        ReturnCell::FinalThird
    })
}

/// The part of the local day for an hour 0-23.
pub fn hour_cell(local_hour: u32) -> ReturnCell {
    match local_hour {
        5..=11 => ReturnCell::Morning,
        12..=16 => ReturnCell::Afternoon,
        _ => ReturnCell::EveningAndNight,
    }
}

fn local_hour(at: DateTime<Utc>, utc_offset_seconds: i32) -> u32 {
    (at + Duration::seconds(i64::from(utc_offset_seconds))).hour()
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Everything the ledger reads about one block, exactly as stored. The caller
/// reads it; the ledger never touches a database.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEvidence {
    pub block_id: String,
    /// `work_block.ended_at`. A block still running contributes nothing.
    pub ended_at: Option<DateTime<Utc>>,
    /// `work_block_observation`, oldest first.
    pub observations: Vec<WorkBlockObservation>,
    /// `intervention_decision_log` for the block, oldest first.
    pub decisions: Vec<InterventionDecision>,
    /// `work_block_intervention`: the one offer, delivered or held, if any.
    pub intervention: Option<WorkBlockIntervention>,
    /// `work_block_category_correction` for the block. Nothing in the shipped
    /// path writes that table today (module docs), so on a Mac this is empty.
    pub category_corrections: Vec<WorkBlockCategoryCorrection>,
    /// The UTC offset in force while the block ran, if the caller knows it.
    /// No table stores one today, so a caller reading today's tables passes
    /// `None` and the block is read at [`LedgerConfig::utc_offset_seconds`].
    pub utc_offset_seconds: Option<i32>,
}

impl BlockEvidence {
    /// The person had said, by `as_of`, that the categories in this block were
    /// wrong: a "Wrong category" reply or a block-scoped correction recorded
    /// at or before it. A reply with no time on it cannot be placed before
    /// `as_of` and does not count; the shipped path always records one.
    pub fn disputed_as_of(&self, as_of: DateTime<Utc>) -> bool {
        self.category_corrections
            .iter()
            .any(|correction| correction.corrected_at <= as_of)
            || self.intervention.as_ref().is_some_and(|offer| {
                offer.outcome == WorkBlockInterventionOutcome::WrongClassification
                    && offer.outcome_at.is_some_and(|replied| replied <= as_of)
            })
    }
}

/// What the ledger knows about one gate decision at the instant it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionContext {
    pub departure: ReturnCell,
    pub elapsed: ReturnCell,
    pub hour: ReturnCell,
    pub switch_count: u32,
    pub verdict: GateVerdict,
}

impl DecisionContext {
    pub fn cells(&self) -> [ReturnCell; 3] {
        [self.departure, self.elapsed, self.hour]
    }
}

/// The decision's context if it was made on a departure, `None` otherwise.
///
/// A departure is the gate's own rule (`s_t`): the observation the decision
/// was made on is confident evidence of a category other than the anchor on
/// the decision row, and the previous confident observation in the block was
/// that anchor. A decision logged before the gate had an anchor is never a
/// departure: the row does not say what the person departed from.
pub fn context_of(
    decision: &InterventionDecision,
    observations: &[WorkBlockObservation],
    utc_offset_seconds: i32,
) -> Option<DecisionContext> {
    let anchor = decision.anchor_category.as_deref()?;
    let index = observations
        .iter()
        .rposition(|observation| observation.occurred_at <= decision.occurred_at)?;
    let trigger = &observations[index];
    if trigger.occurred_at != decision.occurred_at
        || !confident(trigger)
        || trigger.category.eq_ignore_ascii_case(anchor)
    {
        return None;
    }
    let previous = observations[..index]
        .iter()
        .rfind(|observation| confident(observation))?;
    if !previous.category.eq_ignore_ascii_case(anchor) {
        return None;
    }
    Some(DecisionContext {
        departure: departure_cell(&trigger.category)?,
        elapsed: elapsed_cell(decision.elapsed_seconds, decision.remaining_seconds)?,
        hour: hour_cell(local_hour(decision.occurred_at, utc_offset_seconds)),
        switch_count: decision.switch_count,
        verdict: decision.gate_verdict,
    })
}

fn confident(observation: &WorkBlockObservation) -> bool {
    is_confident(
        &observation.category,
        observation.classification_status,
        observation.classification_confidence,
    )
}

// ---------------------------------------------------------------------------
// The label
// ---------------------------------------------------------------------------

/// Why a horizon could not be scored. Closed set: the first two are the
/// pre-registered censoring rule's; `Treated` is this ledger's own, because it
/// counts departures the gate did not act on and an offer is the gate acting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CensorReason {
    /// The block ended before the horizon elapsed.
    BlockEnded,
    /// Some of the horizon is covered by no observation row.
    ObserverGap,
    /// An offer, delivered or held, fell at or before the departure, or inside
    /// the horizon before the time preceding it decided the label. Informative:
    /// see the module docs for the direction of the bias it leaves.
    Treated,
}

impl CensorReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlockEnded => "block_ended",
            Self::ObserverGap => "observer_gap",
            Self::Treated => "treated",
        }
    }
}

/// What became of the horizon after a departure at `at` from `anchor`. The
/// core of the label function; the checks run in this order:
///
/// 1. The horizon runs past the block's end: censored, `block_ended`.
/// 2. Some of it is covered by no observation row: censored, `observer_gap`.
///    Every row counts towards coverage, confident or not: an ambiguous
///    minute was still observed.
/// 3. An offer, delivered or held, at or before `at`: censored, `treated`.
///    An offer inside the horizon: the row is followed up to the offer. At
///    least [`SUSTAINED_ANCHOR_SECONDS`] of anchor before it is a return; so
///    little that the rest of the horizon could not make up the difference is
///    not a return; anything between is censored, `treated`. The horizon is
///    `[at, at + 900 s)`, so an offer at its very end does not touch it.
/// 4. Otherwise the whole horizon is scored. Only confident observations of
///    `anchor` count towards the anchor.
///
/// Checks 1 and 2 come first so that a row decided before an offer is exactly
/// the row that would have been scored had the offer never come.
pub fn horizon_outcome(
    anchor: &str,
    at: DateTime<Utc>,
    block_ended_at: DateTime<Utc>,
    offered_at: Option<DateTime<Utc>>,
    observations: &[WorkBlockObservation],
) -> RowOutcome {
    let horizon_end = at + Duration::seconds(RETURN_HORIZON_SECONDS);
    if horizon_end > block_ended_at {
        return RowOutcome::Censored(CensorReason::BlockEnded);
    }
    let spans = |keep: &dyn Fn(&WorkBlockObservation) -> bool, until: DateTime<Utc>| {
        covered_seconds(
            observations
                .iter()
                .filter(|observation| keep(observation))
                .filter_map(|observation| {
                    observation
                        .ended_at
                        .map(|ended_at| (observation.occurred_at, ended_at))
                }),
            at,
            until,
        )
    };
    let is_anchor = |observation: &WorkBlockObservation| {
        confident(observation) && observation.category.eq_ignore_ascii_case(anchor)
    };
    if spans(&|_| true, horizon_end) < RETURN_HORIZON_SECONDS {
        return RowOutcome::Censored(CensorReason::ObserverGap);
    }
    match offered_at {
        Some(offered_at) if offered_at <= at => RowOutcome::Censored(CensorReason::Treated),
        Some(offered_at) if offered_at < horizon_end => {
            let before = spans(&is_anchor, offered_at);
            let rest = (horizon_end - offered_at).num_seconds();
            if before >= SUSTAINED_ANCHOR_SECONDS {
                RowOutcome::Resolved {
                    returned: true,
                    anchor_seconds: before,
                    decided_before_offer: true,
                }
            } else if before + rest < SUSTAINED_ANCHOR_SECONDS {
                RowOutcome::Resolved {
                    returned: false,
                    anchor_seconds: before,
                    decided_before_offer: true,
                }
            } else {
                RowOutcome::Censored(CensorReason::Treated)
            }
        }
        _ => {
            let anchor_seconds = spans(&is_anchor, horizon_end);
            RowOutcome::Resolved {
                returned: anchor_seconds >= SUSTAINED_ANCHOR_SECONDS,
                anchor_seconds,
                decided_before_offer: false,
            }
        }
    }
}

/// Seconds of `[from, until)` covered by the union of `spans`.
fn covered_seconds(
    spans: impl Iterator<Item = (DateTime<Utc>, DateTime<Utc>)>,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> i64 {
    let mut clipped: Vec<(DateTime<Utc>, DateTime<Utc>)> = spans
        .map(|(start, end)| (start.max(from), end.min(until)))
        .filter(|(start, end)| end > start)
        .collect();
    clipped.sort();
    let mut covered = 0_i64;
    let mut reach: Option<DateTime<Utc>> = None;
    for (start, end) in clipped {
        let start = reach.map_or(start, |reach| start.max(reach));
        if end > start {
            covered += (end - start).num_seconds();
        }
        reach = Some(reach.map_or(end, |reach| reach.max(end)));
    }
    covered
}

/// What became of one departure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOutcome {
    Resolved {
        returned: bool,
        /// Anchor seconds in the horizon; for a row decided before an offer,
        /// only those before the offer.
        anchor_seconds: i64,
        /// An offer fell inside the horizon and the time before it decided
        /// the label.
        decided_before_offer: bool,
    },
    Censored(CensorReason),
}

/// One departure and what became of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepartureRow {
    pub occurred_at: DateTime<Utc>,
    pub policy_version: u32,
    pub context: DecisionContext,
    pub outcome: RowOutcome,
}

/// Every departure in a closed block, with its outcome. This is the label
/// function; the ledger and any later export must agree with it.
///
/// The block is read at its own UTC offset when it carries one, and at
/// `fallback_utc_offset_seconds` otherwise (module docs).
///
/// A block that has not closed returns nothing: its last horizons cannot be
/// scored yet, and scoring them as censored would change once it closes.
pub fn departure_rows(
    block: &BlockEvidence,
    fallback_utc_offset_seconds: i32,
) -> Vec<DepartureRow> {
    let Some(ended_at) = block.ended_at else {
        return Vec::new();
    };
    let utc_offset_seconds = block
        .utc_offset_seconds
        .unwrap_or(fallback_utc_offset_seconds);
    let offered_at = block.intervention.as_ref().map(|offer| offer.offered_at);
    block
        .decisions
        .iter()
        .filter_map(|decision| {
            let context = context_of(decision, &block.observations, utc_offset_seconds)?;
            let anchor = decision.anchor_category.as_deref()?;
            let outcome = horizon_outcome(
                anchor,
                decision.occurred_at,
                ended_at,
                offered_at,
                &block.observations,
            );
            Some(DepartureRow {
                occurred_at: decision.occurred_at,
                policy_version: decision.policy_version,
                context,
                outcome,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The ledger's error controls. [`LedgerControls::shipped`] is the only configuration
/// any analysis may use; the others exist so that a test can show each control
/// is load-bearing, which is the inversion discipline `antecedents.rs` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerControls {
    /// Weight rows from one block by the Kish design effect.
    pub clustering: bool,
    /// Bonferroni over the whole cell family.
    pub bonferroni: bool,
    /// Require the held-out later blocks to repeat the direction.
    pub confirmation: bool,
}

impl LedgerControls {
    pub fn shipped() -> Self {
        Self {
            clustering: true,
            bonferroni: true,
            confirmation: true,
        }
    }

    /// Every control off: what an analyst reading one table at 80% would do.
    pub fn naive() -> Self {
        Self {
            clustering: false,
            bonferroni: false,
            confirmation: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerConfig {
    /// Nothing that closed after this instant is read.
    pub as_of: DateTime<Utc>,
    /// The offset a block is read at when it carries none of its own:
    /// `focus_observer_state.utc_offset_seconds`, the latest one known. The
    /// module docs say what that misfiles.
    pub utc_offset_seconds: i32,
    /// Only rows logged under this drift policy are counted. Versions are
    /// never pooled.
    pub policy_version: u32,
    pub controls: LedgerControls,
}

impl LedgerConfig {
    pub fn new(as_of: DateTime<Utc>, utc_offset_seconds: i32) -> Self {
        Self {
            as_of,
            utc_offset_seconds,
            policy_version: DRIFT_POLICY_VERSION,
            controls: LedgerControls::shipped(),
        }
    }
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------

/// Why the whole ledger declined to say anything beyond its raw counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerAbstention {
    TooFewBlocks { blocks: usize, required: usize },
    TooFewResolvedRows { rows: usize, required: usize },
}

/// How every departure in the lookback was accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LedgerCounts {
    /// Closed blocks inside the lookback.
    pub blocks_considered: usize,
    /// Blocks contributing at least one resolved row.
    pub blocks_with_resolved_rows: usize,
    /// Blocks the person disputed, counted at [`DISPUTED_BLOCK_WEIGHT`].
    pub disputed_blocks: usize,
    /// Departures under the configured policy version, whatever became of them.
    pub departures: usize,
    /// Censored by an offer. Informative censoring (module docs).
    pub censored_treated: usize,
    pub censored_block_ended: usize,
    pub censored_observer_gap: usize,
    pub resolved: usize,
    pub returned: usize,
    /// Resolved rows whose label the time before an offer decided. Included in
    /// `resolved`.
    pub resolved_before_offer: usize,
    /// Departures logged under another drift policy version, not counted.
    pub other_policy_version: usize,
}

/// How every departure in one cell was accounted for, in raw counts. The
/// treated count is the one to read beside the cell's rate: it is how many of
/// the cell's departures an offer censored, which is where the rate's upward
/// bias comes from (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAccounting {
    pub departures: usize,
    pub censored_treated: usize,
    pub censored_block_ended: usize,
    pub censored_observer_gap: usize,
    /// Resolved on the time before an offer. Included in the cell's estimate.
    pub resolved_before_offer: usize,
}

/// A rate with its raw counts. The counts are what a person could be shown;
/// the posterior is for analysis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateEstimate {
    pub returned: usize,
    pub resolved: usize,
    pub blocks: usize,
    /// Resolved rows after block weighting.
    pub effective_resolved: f64,
    pub posterior_mean: f64,
    /// The [`REPORTED_INTERVAL_MASS`] equal-tailed interval.
    pub lower: f64,
    pub upper: f64,
}

/// A cell against the rest of its dimension, inside one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastStat {
    /// Posterior mean of the cell minus posterior mean of the rest.
    pub difference: f64,
    /// `difference` over its posterior standard deviation.
    pub z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellDirection {
    /// Returns are rarer after departures in this cell than in the rest.
    Lower,
    /// Returns are more common.
    Higher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSurfaced {
    /// The cell or the rest is below the support floor in the discovery window.
    TooThinToTest,
    /// The family-wise interval includes no difference.
    IntervalIncludesZero,
    /// The held-out window is below its own support floor.
    ConfirmationTooThin,
    /// The held-out window did not repeat the direction at its level.
    NotConfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStatus {
    /// The whole ledger abstained.
    Abstained,
    NotSurfaced(NotSurfaced),
    Surfaced(CellDirection),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellReport {
    pub cell: ReturnCell,
    /// Over every resolved row in the lookback.
    pub estimate: RateEstimate,
    /// What became of every departure in the cell, resolved or not.
    pub accounting: CellAccounting,
    pub discovery: Option<ContrastStat>,
    pub confirmation: Option<ContrastStat>,
    pub status: CellStatus,
}

/// Whether the declared withhold candidate's rows cover the point it is asked
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithholdSupport {
    /// The point is the kind of departure the ledger counted: one the gate did
    /// not act on, at a switch count it has counted.
    WithinCountedRows,
    /// The gate offered or held at this point, or its switch count is beyond
    /// every counted row. The candidate is an extrapolation here, and under v5
    /// every offered point is one.
    Extrapolated,
}

/// The declared withhold candidate at one decision point. For offline
/// evaluation only; nothing in the shipped path may read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WithholdCandidate {
    pub model_version: u32,
    pub would_withhold: bool,
    /// The smallest lower interval end across the point's three cells, when
    /// every one of them clears the support floor.
    pub min_lower: Option<f64>,
    pub support: WithholdSupport,
    pub abstained: bool,
}

/// One person's ledger, as of one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnLedger {
    pub model_version: u32,
    pub feature_contract_version: u32,
    pub policy_version: u32,
    pub as_of: DateTime<Utc>,
    pub controls: LedgerControls,
    pub counts: LedgerCounts,
    pub baseline: RateEstimate,
    /// One report per cell, in [`RETURN_CELLS`] order.
    pub cells: Vec<CellReport>,
    pub abstention: Option<LedgerAbstention>,
    /// The largest `switch_count` among counted rows. Under v5 it is below the
    /// gate's switch threshold by construction, and the positivity argument in
    /// the module docs rests on that; it is recorded so it can be checked.
    pub max_switch_count_counted: Option<u32>,
    /// The |z| a discovery contrast had to reach.
    pub discovery_threshold: f64,
    /// The one-sided z the held-out window had to reach.
    pub confirmation_threshold: f64,
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

struct ResolvedRow {
    /// Position of the block in `(ended_at, block_id)` order.
    block: usize,
    weight: f64,
    context: DecisionContext,
    returned: bool,
}

#[derive(Default)]
struct Tally {
    /// block position -> (rows, returned, block weight)
    blocks: BTreeMap<usize, (usize, usize, f64)>,
}

impl Tally {
    fn of<'a>(rows: impl Iterator<Item = &'a ResolvedRow>) -> Self {
        let mut tally = Self::default();
        for row in rows {
            let entry = tally.blocks.entry(row.block).or_insert((0, 0, row.weight));
            entry.0 += 1;
            entry.1 += usize::from(row.returned);
        }
        tally
    }

    fn rows(&self) -> usize {
        self.blocks.values().map(|(rows, _, _)| rows).sum()
    }

    fn returned(&self) -> usize {
        self.blocks.values().map(|(_, returned, _)| returned).sum()
    }

    fn block_count(&self) -> usize {
        self.blocks.len()
    }

    fn clears(&self, rows: usize, blocks: usize) -> bool {
        self.rows() >= rows && self.block_count() >= blocks
    }

    /// Weighted (returned, resolved) pseudo-counts.
    fn weighted(&self, clustering: bool) -> (f64, f64) {
        let mut returned = 0.0;
        let mut resolved = 0.0;
        for &(rows, block_returned, weight) in self.blocks.values() {
            let design_effect = if clustering {
                1.0 + (rows as f64 - 1.0) * ASSUMED_WITHIN_BLOCK_CORRELATION
            } else {
                1.0
            };
            let per_row = weight / design_effect;
            returned += per_row * block_returned as f64;
            resolved += per_row * rows as f64;
        }
        (returned, resolved)
    }

    fn posterior(&self, prior: (f64, f64), clustering: bool) -> Posterior {
        let (returned, resolved) = self.weighted(clustering);
        Posterior {
            alpha: prior.0 + returned,
            beta: prior.1 + (resolved - returned),
        }
    }

    fn estimate(&self, prior: (f64, f64), clustering: bool) -> RateEstimate {
        let posterior = self.posterior(prior, clustering);
        let tail = (1.0 - REPORTED_INTERVAL_MASS) / 2.0;
        RateEstimate {
            returned: self.returned(),
            resolved: self.rows(),
            blocks: self.block_count(),
            effective_resolved: self.weighted(clustering).1,
            posterior_mean: posterior.mean(),
            lower: beta_quantile(tail, posterior.alpha, posterior.beta),
            upper: beta_quantile(1.0 - tail, posterior.alpha, posterior.beta),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Posterior {
    alpha: f64,
    beta: f64,
}

impl Posterior {
    fn mean(self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    fn variance(self) -> f64 {
        let total = self.alpha + self.beta;
        self.alpha * self.beta / (total * total * (total + 1.0))
    }
}

/// A cell's prior: [`CELL_PRIOR_STRENGTH`] pseudo-departures at the person's
/// own overall rate in the same rows.
fn cell_prior(all: &Tally, clustering: bool) -> (f64, f64) {
    let centre = all.posterior(BASELINE_PRIOR, clustering).mean();
    (
        CELL_PRIOR_STRENGTH * centre,
        CELL_PRIOR_STRENGTH * (1.0 - centre),
    )
}

/// ReturnCell against the rest of its dimension, over `rows`, if both clear the
/// floor.
fn contrast(
    rows: &[&ResolvedRow],
    cell: ReturnCell,
    floor: (usize, usize),
    clustering: bool,
) -> Option<ContrastStat> {
    let inside = Tally::of(
        rows.iter()
            .copied()
            .filter(|row| row.context.cells().contains(&cell)),
    );
    let outside = Tally::of(
        rows.iter()
            .copied()
            .filter(|row| !row.context.cells().contains(&cell)),
    );
    if !inside.clears(floor.0, floor.1) || !outside.clears(floor.0, floor.1) {
        return None;
    }
    let prior = cell_prior(&Tally::of(rows.iter().copied()), clustering);
    let inside = inside.posterior(prior, clustering);
    let outside = outside.posterior(prior, clustering);
    let difference = inside.mean() - outside.mean();
    let spread = (inside.variance() + outside.variance()).sqrt();
    Some(ContrastStat {
        difference,
        z: difference / spread,
    })
}

impl ReturnLedger {
    /// Builds one person's ledger from the blocks passed in. Pure: the same
    /// blocks and config give the same ledger, bit for bit.
    pub fn build(blocks: &[BlockEvidence], config: &LedgerConfig) -> Self {
        let controls = config.controls;
        let window_start = config.as_of - Duration::days(LOOKBACK_DAYS);
        let mut selected: Vec<(DateTime<Utc>, &BlockEvidence)> = blocks
            .iter()
            .filter_map(|block| block.ended_at.map(|ended_at| (ended_at, block)))
            .filter(|(ended_at, _)| *ended_at <= config.as_of && *ended_at > window_start)
            .collect();
        selected.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.block_id.cmp(&right.1.block_id))
        });

        let mut counts = LedgerCounts {
            blocks_considered: selected.len(),
            ..LedgerCounts::default()
        };
        let mut rows: Vec<ResolvedRow> = Vec::new();
        let mut accounting: BTreeMap<ReturnCell, CellAccounting> = BTreeMap::new();
        for (position, (_, block)) in selected.iter().enumerate() {
            let weight = if block.disputed_as_of(config.as_of) {
                counts.disputed_blocks += 1;
                DISPUTED_BLOCK_WEIGHT
            } else {
                1.0
            };
            for row in departure_rows(block, config.utc_offset_seconds) {
                if row.policy_version != config.policy_version {
                    counts.other_policy_version += 1;
                    continue;
                }
                counts.departures += 1;
                let cells = row.context.cells();
                let mut tally = |field: fn(&mut CellAccounting) -> &mut usize| {
                    for cell in cells {
                        *field(accounting.entry(cell).or_default()) += 1;
                    }
                };
                tally(|cell| &mut cell.departures);
                match row.outcome {
                    RowOutcome::Censored(CensorReason::Treated) => {
                        counts.censored_treated += 1;
                        tally(|cell| &mut cell.censored_treated);
                    }
                    RowOutcome::Censored(CensorReason::BlockEnded) => {
                        counts.censored_block_ended += 1;
                        tally(|cell| &mut cell.censored_block_ended);
                    }
                    RowOutcome::Censored(CensorReason::ObserverGap) => {
                        counts.censored_observer_gap += 1;
                        tally(|cell| &mut cell.censored_observer_gap);
                    }
                    RowOutcome::Resolved {
                        returned,
                        decided_before_offer,
                        ..
                    } => {
                        counts.resolved += 1;
                        counts.returned += usize::from(returned);
                        if decided_before_offer {
                            counts.resolved_before_offer += 1;
                            tally(|cell| &mut cell.resolved_before_offer);
                        }
                        rows.push(ResolvedRow {
                            block: position,
                            weight,
                            context: row.context,
                            returned,
                        });
                    }
                }
            }
        }

        let all = Tally::of(rows.iter());
        counts.blocks_with_resolved_rows = all.block_count();
        let abstention = if all.block_count() < MIN_BLOCKS {
            Some(LedgerAbstention::TooFewBlocks {
                blocks: all.block_count(),
                required: MIN_BLOCKS,
            })
        } else if all.rows() < MIN_RESOLVED_ROWS {
            Some(LedgerAbstention::TooFewResolvedRows {
                rows: all.rows(),
                required: MIN_RESOLVED_ROWS,
            })
        } else {
            None
        };

        let discovery_threshold = if controls.bonferroni {
            normal_upper_quantile(FAMILY_ALPHA / (2.0 * RETURN_CELLS.len() as f64))
        } else {
            normal_upper_quantile(FAMILY_ALPHA / 2.0)
        };
        let confirmation_threshold = normal_upper_quantile(HELD_OUT_ALPHA);

        // Discovery on the earlier blocks, confirmation on the later ones,
        // never sharing a block. Without the confirmation control the naive
        // analyst reads everything at once.
        let contributing: Vec<usize> = all.blocks.keys().copied().collect();
        let held_out = if controls.confirmation {
            contributing.len() * CONFIRMATION_SHARE.0 / CONFIRMATION_SHARE.1
        } else {
            0
        };
        let first_held_out = contributing
            .get(contributing.len() - held_out)
            .copied()
            .unwrap_or(usize::MAX);
        let discovery: Vec<&ResolvedRow> = rows
            .iter()
            .filter(|row| row.block < first_held_out)
            .collect();
        let confirmation: Vec<&ResolvedRow> = rows
            .iter()
            .filter(|row| row.block >= first_held_out)
            .collect();

        let prior = cell_prior(&all, controls.clustering);
        let cells = RETURN_CELLS
            .iter()
            .map(|&cell| {
                let estimate = Tally::of(
                    rows.iter()
                        .filter(|row| row.context.cells().contains(&cell)),
                )
                .estimate(prior, controls.clustering);
                let discovered = contrast(
                    &discovery,
                    cell,
                    (MIN_CELL_ROWS, MIN_CELL_BLOCKS),
                    controls.clustering,
                );
                let confirmed = if controls.confirmation {
                    contrast(
                        &confirmation,
                        cell,
                        (MIN_CONFIRMATION_ROWS, MIN_CONFIRMATION_BLOCKS),
                        controls.clustering,
                    )
                } else {
                    None
                };
                let status = if abstention.is_some() {
                    CellStatus::Abstained
                } else {
                    match discovered {
                        None => CellStatus::NotSurfaced(NotSurfaced::TooThinToTest),
                        Some(found) if found.z.abs() < discovery_threshold => {
                            CellStatus::NotSurfaced(NotSurfaced::IntervalIncludesZero)
                        }
                        Some(found) => {
                            let direction = if found.difference < 0.0 {
                                CellDirection::Lower
                            } else {
                                CellDirection::Higher
                            };
                            if !controls.confirmation {
                                CellStatus::Surfaced(direction)
                            } else {
                                match confirmed {
                                    None => {
                                        CellStatus::NotSurfaced(NotSurfaced::ConfirmationTooThin)
                                    }
                                    Some(held)
                                        if held.z * found.z.signum() >= confirmation_threshold =>
                                    {
                                        CellStatus::Surfaced(direction)
                                    }
                                    Some(_) => CellStatus::NotSurfaced(NotSurfaced::NotConfirmed),
                                }
                            }
                        }
                    }
                };
                CellReport {
                    cell,
                    estimate,
                    accounting: accounting.get(&cell).copied().unwrap_or_default(),
                    discovery: discovered,
                    confirmation: confirmed,
                    status,
                }
            })
            .collect();

        Self {
            model_version: RETURN_LEDGER_MODEL_VERSION,
            feature_contract_version: FEATURE_CONTRACT_VERSION,
            policy_version: config.policy_version,
            as_of: config.as_of,
            controls,
            counts,
            baseline: all.estimate(BASELINE_PRIOR, controls.clustering),
            cells,
            abstention,
            max_switch_count_counted: rows.iter().map(|row| row.context.switch_count).max(),
            discovery_threshold,
            confirmation_threshold,
        }
    }

    pub fn cell(&self, cell: ReturnCell) -> &CellReport {
        self.cells
            .iter()
            .find(|report| report.cell == cell)
            .expect("the ledger reports every frozen cell")
    }

    /// Cells whose difference from the rest of their dimension was found in
    /// the earlier blocks and repeated in the later ones.
    pub fn surfaced(&self) -> Vec<(ReturnCell, CellDirection)> {
        self.cells
            .iter()
            .filter_map(|report| match report.status {
                CellStatus::Surfaced(direction) => Some((report.cell, direction)),
                _ => None,
            })
            .collect()
    }

    /// The declared withhold candidate at one decision point, for offline
    /// evaluation only.
    ///
    /// It fires only when the ledger does not abstain, each of the point's
    /// three cells clears the support floor, and in each of them the lower
    /// end of the reported interval is at least [`WITHHOLD_LOWER_BOUND`]. It
    /// can only ever describe removing a nudge, never adding one. At a point
    /// where the gate offered or held, [`WithholdSupport::Extrapolated`] says the
    /// counted rows do not cover it.
    pub fn would_withhold(&self, context: &DecisionContext) -> WithholdCandidate {
        let counted_switches = self.max_switch_count_counted;
        let acted = matches!(
            context.verdict,
            GateVerdict::Offered | GateVerdict::SuppressedDnd | GateVerdict::WithheldDemotion
        );
        let support = if acted || counted_switches.is_none_or(|most| context.switch_count > most) {
            WithholdSupport::Extrapolated
        } else {
            WithholdSupport::WithinCountedRows
        };
        let lowers: Option<Vec<f64>> = context
            .cells()
            .iter()
            .map(|&cell| {
                let estimate = self.cell(cell).estimate;
                (estimate.resolved >= MIN_CELL_ROWS && estimate.blocks >= MIN_CELL_BLOCKS)
                    .then_some(estimate.lower)
            })
            .collect();
        let min_lower = lowers.map(|lowers| lowers.into_iter().fold(f64::INFINITY, f64::min));
        let abstained = self.abstention.is_some();
        WithholdCandidate {
            model_version: self.model_version,
            would_withhold: !abstained
                && min_lower.is_some_and(|lower| lower >= WITHHOLD_LOWER_BOUND),
            min_lower,
            support,
            abstained,
        }
    }
}

// ---------------------------------------------------------------------------
// Special functions. Hand-rolled, as `bocpd.rs` and `antecedents.rs` do, so
// the module needs no crate.
// ---------------------------------------------------------------------------

/// Lanczos coefficients, `g = 7`, `n = 9`; the same table as `bocpd.rs`.
const LANCZOS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_1,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_5e-7,
];

/// `ln Gamma(x)` for `x > 0`. Every argument here is a positive pseudo-count.
fn ln_gamma(x: f64) -> f64 {
    let x = x - 1.0;
    let mut series = LANCZOS[0];
    for (offset, coefficient) in LANCZOS.iter().enumerate().skip(1) {
        series += coefficient / (x + offset as f64);
    }
    let t = x + 7.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + series.ln()
}

/// Continued fraction for the incomplete beta function (modified Lentz).
fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    const EPSILON: f64 = 1e-15;
    let floor = |value: f64| if value.abs() < TINY { TINY } else { value };
    let mut c = 1.0;
    let mut d = 1.0 / floor(1.0 - (a + b) * x / (a + 1.0));
    let mut fraction = d;
    for step in 1..=500 {
        let m = f64::from(step);
        let even = m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m));
        d = 1.0 / floor(1.0 + even * d);
        c = floor(1.0 + even / c);
        fraction *= d * c;
        let odd = -(a + m) * (a + b + m) * x / ((a + 2.0 * m) * (a + 2.0 * m + 1.0));
        d = 1.0 / floor(1.0 + odd * d);
        c = floor(1.0 + odd / c);
        let delta = d * c;
        fraction *= delta;
        if (delta - 1.0).abs() < EPSILON {
            break;
        }
    }
    fraction
}

/// The regularized incomplete beta function `I_x(a, b)`: the Beta(a, b) CDF.
fn beta_cdf(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_continued_fraction(b, a, 1.0 - x) / b
    }
}

/// The `p` quantile of Beta(a, b), by bisection. Deterministic and monotone;
/// sixty halvings is below any difference a rate could show.
fn beta_quantile(p: f64, a: f64, b: f64) -> f64 {
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..60 {
        let middle = 0.5 * (low + high);
        if beta_cdf(middle, a, b) < p {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

/// Complementary error function, Numerical Recipes' Chebyshev fit, fractional
/// error below 1.2e-7 everywhere. The same fit `antecedents.rs` uses.
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98
                                + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let value = t * poly.exp();
    if x >= 0.0 {
        value
    } else {
        2.0 - value
    }
}

/// `z` with `P(Z > z) = tail` for a standard normal, by bisection.
fn normal_upper_quantile(tail: f64) -> f64 {
    let upper_tail = |z: f64| 0.5 * erfc(z / std::f64::consts::SQRT_2);
    let (mut low, mut high) = (-12.0_f64, 12.0_f64);
    for _ in 0..80 {
        let middle = 0.5 * (low + high);
        if upper_tail(middle) > tail {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use velvt_shared_types::{ClassificationConfidence, ClassificationStatus};

    fn approx(left: f64, right: f64, tolerance: f64) -> bool {
        (left - right).abs() <= tolerance
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn observation(start: i64, end: i64, category: &str) -> WorkBlockObservation {
        WorkBlockObservation {
            occurred_at: at(start),
            ended_at: Some(at(end)),
            category: category.to_owned(),
            classification_status: ClassificationStatus::Classified,
            classification_confidence: ClassificationConfidence::High,
        }
    }

    fn decision(
        t: i64,
        anchor: Option<&str>,
        switches: u32,
        verdict: GateVerdict,
    ) -> InterventionDecision {
        InterventionDecision {
            decision_id: format!("d{t}"),
            occurred_at: at(t),
            block_id: Some("b".to_owned()),
            policy_version: DRIFT_POLICY_VERSION,
            anchor_category: anchor.map(str::to_owned),
            switch_count: switches,
            elapsed_seconds: u32::try_from(t).unwrap(),
            remaining_seconds: 5_400 - u32::try_from(t).unwrap(),
            gate_verdict: verdict,
            propensity: 1.0,
            anchor_seen_within_600s: None,
            outcome_at: None,
        }
    }

    /// Anchor 0-500, a departure to `away` at 500 for `away_for` seconds, then
    /// the anchor until `end`.
    fn one_departure(away: &str, away_for: i64, end: i64) -> BlockEvidence {
        BlockEvidence {
            block_id: "b".to_owned(),
            ended_at: Some(at(end)),
            observations: vec![
                observation(10, 500, "FOCUS_WORK"),
                observation(500, 500 + away_for, away),
                observation(500 + away_for, end, "FOCUS_WORK"),
            ],
            decisions: vec![
                decision(10, None, 0, GateVerdict::AbstainedWarmup),
                decision(
                    500,
                    Some("FOCUS_WORK"),
                    1,
                    GateVerdict::AbstainedMinSwitches,
                ),
                decision(
                    500 + away_for,
                    Some("FOCUS_WORK"),
                    1,
                    GateVerdict::AbstainedAtAnchor,
                ),
            ],
            intervention: None,
            category_corrections: Vec::new(),
            utc_offset_seconds: None,
        }
    }

    fn offer_at(t: i64, outcome: WorkBlockInterventionOutcome) -> WorkBlockIntervention {
        WorkBlockIntervention {
            offered_at: at(t),
            action_id: "protect_next_10".to_owned(),
            anchor_category: "FOCUS_WORK".to_owned(),
            switch_count: 3,
            window_seconds: 600,
            outcome,
            outcome_at: None,
            salience: velvt_shared_types::InterventionSalience::Normal,
            card_seen_at: None,
        }
    }

    fn resolved(returned: bool, anchor_seconds: i64) -> RowOutcome {
        RowOutcome::Resolved {
            returned,
            anchor_seconds,
            decided_before_offer: false,
        }
    }

    #[test]
    fn a_short_departure_is_a_return_and_a_long_one_is_not() {
        let rows = departure_rows(&one_departure("COMMUNICATION", 120, 3_000), 0);
        assert_eq!(rows.len(), 1, "only the departure is a row: {rows:?}");
        assert_eq!(rows[0].outcome, resolved(true, 780));
        assert_eq!(rows[0].context.departure, ReturnCell::Communication);

        let rows = departure_rows(&one_departure("SOCIAL_FEED", 400, 3_000), 0);
        assert_eq!(rows[0].outcome, resolved(false, 500));
        assert_eq!(rows[0].context.departure, ReturnCell::FeedsAndVideo);

        // Exactly the threshold is a return: at least 600 of 900.
        let rows = departure_rows(&one_departure("REFERENCE", 300, 3_000), 0);
        assert_eq!(rows[0].outcome, resolved(true, 600));
    }

    #[test]
    fn a_horizon_past_the_block_end_is_censored_not_a_failure() {
        let rows = departure_rows(&one_departure("COMMUNICATION", 120, 1_399), 0);
        assert_eq!(
            rows[0].outcome,
            RowOutcome::Censored(CensorReason::BlockEnded)
        );
        // Ending exactly at the horizon's end is fully observed.
        let rows = departure_rows(&one_departure("COMMUNICATION", 120, 1_400), 0);
        assert!(matches!(rows[0].outcome, RowOutcome::Resolved { .. }));
    }

    #[test]
    fn a_gap_in_the_observation_ledger_is_censored() {
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        // A pause: the anchor row closes at 700 and the next opens at 760.
        block.observations[2].ended_at = Some(at(700));
        block
            .observations
            .push(observation(760, 3_000, "FOCUS_WORK"));
        let rows = departure_rows(&block, 0);
        assert_eq!(
            rows[0].outcome,
            RowOutcome::Censored(CensorReason::ObserverGap)
        );
    }

    #[test]
    fn unconfident_time_is_observed_but_is_not_the_anchor() {
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        block.observations[2].ended_at = Some(at(800));
        let mut ambiguous = observation(800, 1_100, "FOCUS_WORK");
        ambiguous.classification_status = ClassificationStatus::Ambiguous;
        ambiguous.classification_confidence = ClassificationConfidence::Low;
        block.observations.push(ambiguous);
        block
            .observations
            .push(observation(1_100, 3_000, "FOCUS_WORK"));
        let rows = departure_rows(&block, 0);
        // 620..800 (180) + 1100..1400 (300) = 480 anchor seconds, fully covered.
        assert_eq!(rows[0].outcome, resolved(false, 480));
    }

    #[test]
    fn an_offer_censors_the_horizon_unless_the_time_before_it_decided_the_label() {
        let with_offer = |away: i64, offered: i64| {
            let mut block = one_departure("COMMUNICATION", away, 3_000);
            block.intervention = Some(offer_at(offered, WorkBlockInterventionOutcome::NoResponse));
            departure_rows(&block, 0)[0].outcome
        };
        let treated = RowOutcome::Censored(CensorReason::Treated);

        // Departure at 500, back at 620. An offer at 700 leaves 80 anchor
        // seconds before it and 700 after: the label is open, so censored.
        assert_eq!(with_offer(120, 700), treated);
        // An offer at or before the departure censors the whole horizon.
        assert_eq!(with_offer(120, 500), treated);
        assert_eq!(with_offer(120, 300), treated);
        // 600 anchor seconds before the offer (620..1220) decide a return.
        assert_eq!(
            with_offer(120, 1_220),
            RowOutcome::Resolved {
                returned: true,
                anchor_seconds: 600,
                decided_before_offer: true,
            }
        );
        assert_eq!(with_offer(120, 1_219), treated);
        // Away 500..900, offer at 901: one anchor second before it and 499
        // left, so 600 is out of reach and the label is decided.
        assert_eq!(
            with_offer(400, 901),
            RowOutcome::Resolved {
                returned: false,
                anchor_seconds: 1,
                decided_before_offer: true,
            }
        );
        // An offer at 900 leaves 500 seconds, which cannot make 600 either;
        // one at 800 leaves exactly 600, so the label is still open.
        assert_eq!(
            with_offer(400, 900),
            RowOutcome::Resolved {
                returned: false,
                anchor_seconds: 0,
                decided_before_offer: true,
            }
        );
        assert_eq!(with_offer(400, 800), treated);
        // The horizon is [500, 1400): an offer at its very end does not touch
        // it, and the whole horizon is scored.
        assert_eq!(with_offer(120, 1_400), resolved(true, 780));

        // A decided row is the row that would have been scored with no offer:
        // a horizon past the block end or with a gap is censored for that
        // reason first.
        let mut block = one_departure("SOCIAL_FEED", 400, 1_399);
        block.intervention = Some(offer_at(901, WorkBlockInterventionOutcome::NoResponse));
        assert_eq!(
            departure_rows(&block, 0)[0].outcome,
            RowOutcome::Censored(CensorReason::BlockEnded)
        );
    }

    #[test]
    fn the_treated_count_is_reported_per_cell() {
        // Two blocks, one departure each: communication censored by an offer,
        // reference decided before one.
        let mut blocks = history(
            &[(true, "COMMUNICATION"), (false, "REFERENCE")],
            Duration::days(1),
        );
        let shift = blocks[0].decisions[1].occurred_at - at(500);
        blocks[0].intervention = Some(offer_at(700, WorkBlockInterventionOutcome::NoResponse));
        blocks[0].intervention.as_mut().unwrap().offered_at += shift;
        let shift = blocks[1].decisions[1].occurred_at - at(500);
        blocks[1].intervention = Some(offer_at(1_000, WorkBlockInterventionOutcome::NoResponse));
        blocks[1].intervention.as_mut().unwrap().offered_at += shift;
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), 0));

        assert_eq!(ledger.counts.departures, 2);
        assert_eq!(ledger.counts.censored_treated, 1);
        assert_eq!(ledger.counts.resolved, 1);
        assert_eq!(ledger.counts.resolved_before_offer, 1);
        let communication = ledger.cell(ReturnCell::Communication).accounting;
        assert_eq!(
            communication,
            CellAccounting {
                departures: 1,
                censored_treated: 1,
                ..CellAccounting::default()
            }
        );
        let adjacent = ledger.cell(ReturnCell::WorkAdjacent).accounting;
        assert_eq!(adjacent.censored_treated, 0);
        assert_eq!(adjacent.resolved_before_offer, 1);
        // Both departures fall in the first third, so that cell carries both.
        let first = ledger.cell(ReturnCell::FirstThird).accounting;
        assert_eq!((first.departures, first.censored_treated), (2, 1));
        assert_eq!(ledger.cell(ReturnCell::Communication).estimate.resolved, 0);
    }

    #[test]
    fn a_decision_without_an_anchor_or_on_the_anchor_is_not_a_departure() {
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        block.decisions[1].anchor_category = None;
        assert!(departure_rows(&block, 0).is_empty());

        // A second non-anchor observation after the first is not a fresh
        // departure: its previous confident observation was not the anchor.
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        block.observations[1].ended_at = Some(at(560));
        block
            .observations
            .insert(2, observation(560, 620, "REFERENCE"));
        block.decisions.insert(
            2,
            decision(
                560,
                Some("FOCUS_WORK"),
                1,
                GateVerdict::AbstainedMinSwitches,
            ),
        );
        assert_eq!(departure_rows(&block, 0).len(), 1);
    }

    #[test]
    fn an_open_block_contributes_nothing_until_it_closes() {
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        block.ended_at = None;
        assert!(departure_rows(&block, 0).is_empty());
    }

    #[test]
    fn the_cells_partition_every_departure() {
        assert_eq!(RETURN_CELLS.len(), 9);
        let mut ids: Vec<&str> = RETURN_CELLS.iter().map(|cell| cell.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RETURN_CELLS.len(), "cell ids are unique");
        for dimension in [
            CellDimension::Departure,
            CellDimension::Elapsed,
            CellDimension::Hour,
        ] {
            assert_eq!(
                RETURN_CELLS
                    .iter()
                    .filter(|cell| cell.dimension() == dimension)
                    .count(),
                3
            );
        }
        // Every confident category has a departure cell; the two that can
        // never be evidence have none.
        for category in super::super::features::CATEGORIES {
            let evidence = !matches!(category, "SYSTEM" | "UNLOGGED");
            assert_eq!(departure_cell(category).is_some(), evidence, "{category}");
        }
        for hour in 0..24 {
            let _ = hour_cell(hour);
        }
        assert_eq!(hour_cell(4), ReturnCell::EveningAndNight);
        assert_eq!(hour_cell(5), ReturnCell::Morning);
        assert_eq!(hour_cell(12), ReturnCell::Afternoon);
        assert_eq!(hour_cell(17), ReturnCell::EveningAndNight);
        assert_eq!(elapsed_cell(0, 3_600), Some(ReturnCell::FirstThird));
        assert_eq!(elapsed_cell(1_200, 2_400), Some(ReturnCell::MiddleThird));
        assert_eq!(elapsed_cell(2_400, 1_200), Some(ReturnCell::FinalThird));
        assert_eq!(elapsed_cell(0, 0), None);
    }

    #[test]
    fn each_block_is_read_at_its_own_offset_when_it_has_one() {
        // The departure is at 08:08 UTC: 11:38 at +3:30, a morning, and 12:08
        // at +4:00, after a daylight-saving change or a flight, an afternoon.
        let mut block = one_departure("COMMUNICATION", 120, 3_000);
        let fallback = 3 * 3_600 + 1_800;
        assert_eq!(
            departure_rows(&block, fallback)[0].context.hour,
            ReturnCell::Morning
        );
        block.utc_offset_seconds = Some(4 * 3_600);
        assert_eq!(
            departure_rows(&block, fallback)[0].context.hour,
            ReturnCell::Afternoon
        );

        // Through the ledger: one block per offset, read at its own.
        let mut blocks = history(&[(true, "COMMUNICATION"); 2], Duration::days(1));
        blocks[1].utc_offset_seconds = Some(4 * 3_600);
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), fallback));
        assert_eq!(ledger.cell(ReturnCell::Morning).accounting.departures, 1);
        assert_eq!(ledger.cell(ReturnCell::Afternoon).accounting.departures, 1);
    }

    #[test]
    fn the_part_of_the_day_is_local() {
        // 1_800_000_000 is 08:00 UTC.
        let block = one_departure("COMMUNICATION", 120, 3_000);
        assert_eq!(
            departure_rows(&block, 0)[0].context.hour,
            ReturnCell::Morning
        );
        assert_eq!(
            departure_rows(&block, 5 * 3_600)[0].context.hour,
            ReturnCell::Afternoon
        );
        assert_eq!(
            departure_rows(&block, -4 * 3_600)[0].context.hour,
            ReturnCell::EveningAndNight
        );
    }

    /// `block` moved later by `spacing * index`, as block `index`.
    fn shifted(mut block: BlockEvidence, index: usize, spacing: Duration) -> BlockEvidence {
        let shift = spacing * i32::try_from(index).unwrap();
        block.block_id = format!("b{index:03}");
        block.ended_at = block.ended_at.map(|t| t + shift);
        for observation in &mut block.observations {
            observation.occurred_at += shift;
            observation.ended_at = observation.ended_at.map(|t| t + shift);
        }
        for decision in &mut block.decisions {
            decision.occurred_at += shift;
        }
        block
    }

    /// One one-departure block per outcome, `spacing` apart, each returning
    /// or not.
    fn history(outcomes: &[(bool, &str)], spacing: Duration) -> Vec<BlockEvidence> {
        outcomes
            .iter()
            .enumerate()
            .map(|(index, &(returned, away))| {
                let block = one_departure(away, if returned { 120 } else { 450 }, 3_000);
                shifted(block, index, spacing)
            })
            .collect()
    }

    /// Two departures in one block, so rows share a block: one at 500 s, in
    /// the first third of a 5400 s block, and one at 2000 s, in the middle
    /// third. Each is `(category, returned)`.
    fn two_departures(first: (&str, bool), second: (&str, bool)) -> BlockEvidence {
        let away = |returned: bool| if returned { 120 } else { 450 };
        let (a, b) = (away(first.1), away(second.1));
        BlockEvidence {
            block_id: "b".to_owned(),
            ended_at: Some(at(5_000)),
            observations: vec![
                observation(10, 500, "FOCUS_WORK"),
                observation(500, 500 + a, first.0),
                observation(500 + a, 2_000, "FOCUS_WORK"),
                observation(2_000, 2_000 + b, second.0),
                observation(2_000 + b, 5_000, "FOCUS_WORK"),
            ],
            decisions: vec![
                decision(10, None, 0, GateVerdict::AbstainedWarmup),
                decision(
                    500,
                    Some("FOCUS_WORK"),
                    1,
                    GateVerdict::AbstainedMinSwitches,
                ),
                decision(
                    2_000,
                    Some("FOCUS_WORK"),
                    1,
                    GateVerdict::AbstainedMinSwitches,
                ),
            ],
            intervention: None,
            category_corrections: Vec::new(),
            utc_offset_seconds: None,
        }
    }

    /// `scripts/simulate_nudge_designs.py` carries a port of the declared
    /// rule's arithmetic, so that what it simulates is this rule and not a
    /// cousin of it. These numbers are pinned here and in its test
    /// (`scripts/tests/simulate_nudge_designs_test.sh`): a change to the
    /// priors, the weighting, the floors or the interval that is not made in
    /// both places fails one of the two.
    ///
    /// Block `k`, 8 hours apart: at 500 s a departure to REFERENCE when `k` is
    /// even and COMMUNICATION when odd, returning unless `k % 4 == 3`; at
    /// 2000 s one to SOCIAL_FEED when `k % 3 == 0` and REFERENCE otherwise,
    /// returning unless `k % 5 == 0`. Block `k` falls in the morning, the
    /// afternoon or the evening as `k % 3` is 0, 1 or 2.
    #[test]
    fn the_declared_rule_matches_its_offline_port() {
        let blocks: Vec<BlockEvidence> = (0..20_usize)
            .map(|k| {
                let first = (
                    if k % 2 == 0 {
                        "REFERENCE"
                    } else {
                        "COMMUNICATION"
                    },
                    k % 4 != 3,
                );
                let second = (
                    if k % 3 == 0 {
                        "SOCIAL_FEED"
                    } else {
                        "REFERENCE"
                    },
                    k % 5 != 0,
                );
                shifted(two_departures(first, second), k, Duration::hours(8))
            })
            .collect();
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(10 * 86_400), 0));
        assert_eq!(ledger.abstention, None);
        // (cell, returned, resolved, blocks, lower end of the 80% interval)
        let pinned = [
            (ReturnCell::Communication, 5, 10, 10, 0.392_546_752_285),
            (ReturnCell::FeedsAndVideo, 5, 7, 7, 0.536_916_437_166),
            (ReturnCell::WorkAdjacent, 21, 23, 17, 0.792_387_031_023),
            (ReturnCell::FirstThird, 15, 20, 20, 0.627_717_218_147),
            (ReturnCell::MiddleThird, 16, 20, 20, 0.675_806_342_703),
            (ReturnCell::FinalThird, 0, 0, 0, 0.422_990_126_298),
            (ReturnCell::Morning, 10, 14, 7, 0.561_184_701_132),
            (ReturnCell::Afternoon, 11, 14, 7, 0.622_012_140_453),
            (ReturnCell::EveningAndNight, 10, 12, 6, 0.651_963_632_261),
        ];
        for (cell, returned, resolved, blocks, lower) in pinned {
            let estimate = ledger.cell(cell).estimate;
            assert_eq!(
                (estimate.returned, estimate.resolved, estimate.blocks),
                (returned, resolved, blocks),
                "{}",
                cell.id()
            );
            assert!(
                approx(estimate.lower, lower, 1e-9),
                "{}: {}",
                cell.id(),
                estimate.lower
            );
        }
        // One point fires, two fall short of 0.60, one fails a cell's floor.
        let point = |departure, elapsed, hour| {
            ledger.would_withhold(&DecisionContext {
                departure,
                elapsed,
                hour,
                switch_count: 1,
                verdict: GateVerdict::AbstainedMinSwitches,
            })
        };
        for (departure, elapsed, hour, fires, min_lower) in [
            (
                ReturnCell::WorkAdjacent,
                ReturnCell::MiddleThird,
                ReturnCell::Afternoon,
                true,
                Some(0.622_012_140_453),
            ),
            (
                ReturnCell::WorkAdjacent,
                ReturnCell::FirstThird,
                ReturnCell::Morning,
                false,
                Some(0.561_184_701_132),
            ),
            (
                ReturnCell::Communication,
                ReturnCell::FirstThird,
                ReturnCell::Afternoon,
                false,
                Some(0.392_546_752_285),
            ),
            (
                ReturnCell::FeedsAndVideo,
                ReturnCell::MiddleThird,
                ReturnCell::Morning,
                false,
                None,
            ),
        ] {
            let candidate = point(departure, elapsed, hour);
            assert_eq!(
                candidate.would_withhold, fires,
                "{departure:?} {elapsed:?} {hour:?}"
            );
            match (candidate.min_lower, min_lower) {
                (Some(found), Some(pinned)) => assert!(approx(found, pinned, 1e-9)),
                (found, pinned) => assert_eq!(found, pinned),
            }
        }
    }

    #[test]
    fn it_abstains_below_the_block_floor_with_the_reason() {
        let blocks = history(&[(true, "COMMUNICATION"); 5], Duration::days(1));
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), 0));
        assert_eq!(
            ledger.abstention,
            Some(LedgerAbstention::TooFewBlocks {
                blocks: 5,
                required: MIN_BLOCKS
            })
        );
        assert!(ledger.surfaced().is_empty());
        assert!(ledger
            .cells
            .iter()
            .all(|report| report.status == CellStatus::Abstained));
        // The raw counts are still there: they are the honest part.
        assert_eq!(ledger.counts.resolved, 5);
        assert_eq!(ledger.counts.returned, 5);
    }

    #[test]
    fn it_abstains_below_the_row_floor_with_enough_blocks() {
        let blocks = history(&[(true, "COMMUNICATION"); 12], Duration::days(1));
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), 0));
        assert_eq!(
            ledger.abstention,
            Some(LedgerAbstention::TooFewResolvedRows {
                rows: 12,
                required: MIN_RESOLVED_ROWS
            })
        );
    }

    #[test]
    fn blocks_outside_the_lookback_or_after_as_of_are_not_read() {
        let blocks = history(&[(true, "COMMUNICATION"); 40], Duration::days(1));
        let as_of = blocks[35].ended_at.unwrap();
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(as_of, 0));
        // Blocks 8..=35 closed inside the 28 days ending at block 35.
        assert_eq!(ledger.counts.blocks_considered, 28);
        let earlier: Vec<BlockEvidence> = blocks[..=35].to_vec();
        assert_eq!(
            ReturnLedger::build(&earlier, &LedgerConfig::new(as_of, 0)),
            ledger,
            "a block that closed after as_of changed the answer"
        );
    }

    #[test]
    fn other_policy_versions_are_counted_aside_never_pooled() {
        let mut blocks = history(&[(true, "COMMUNICATION"); 3], Duration::days(1));
        for decision in &mut blocks[0].decisions {
            decision.policy_version = DRIFT_POLICY_VERSION - 1;
        }
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), 0));
        assert_eq!(ledger.counts.other_policy_version, 1);
        assert_eq!(ledger.counts.departures, 2);
    }

    #[test]
    fn a_disputed_block_counts_at_half_weight() {
        let mut blocks = history(&[(true, "COMMUNICATION"); 2], Duration::days(1));
        blocks[1].category_corrections = vec![WorkBlockCategoryCorrection {
            category: "COMMUNICATION".to_owned(),
            counts_as_category: "FOCUS_WORK".to_owned(),
            corrected_at: blocks[1].ended_at.unwrap(),
        }];
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(20 * 86_400), 0));
        assert_eq!(ledger.counts.disputed_blocks, 1);
        assert!(approx(
            ledger.baseline.effective_resolved,
            1.0 + DISPUTED_BLOCK_WEIGHT,
            1e-12
        ));
    }

    /// A reply or a correction can be written after its block closed. At an
    /// `as_of` before it was written, the ledger must not know about it.
    #[test]
    fn a_dispute_recorded_after_as_of_does_not_change_the_answer_at_as_of() {
        let clean = history(&[(true, "COMMUNICATION"); 2], Duration::days(1));
        let as_of = clean[1].ended_at.unwrap() + Duration::hours(1);
        let later = as_of + Duration::hours(1);
        let config = LedgerConfig::new(as_of, 0);
        let answer = ReturnLedger::build(&clean, &config);

        let mut replied = clean.clone();
        let mut offer = offer_at(3_500, WorkBlockInterventionOutcome::WrongClassification);
        offer.offered_at = replied[1].ended_at.unwrap() - Duration::seconds(100);
        offer.outcome_at = Some(later);
        replied[1].intervention = Some(offer);
        let mut corrected = clean.clone();
        corrected[0].category_corrections = vec![WorkBlockCategoryCorrection {
            category: "COMMUNICATION".to_owned(),
            counts_as_category: "FOCUS_WORK".to_owned(),
            corrected_at: later,
        }];
        for (name, blocks) in [("reply", &replied), ("correction", &corrected)] {
            assert_eq!(
                ReturnLedger::build(blocks, &config),
                answer,
                "a {name} written after as_of changed the answer at as_of"
            );
            let after = ReturnLedger::build(blocks, &LedgerConfig::new(later, 0));
            assert_eq!(after.counts.disputed_blocks, 1, "{name}");
        }

        // A reply with no time on it cannot be placed before as_of.
        let mut untimed = replied.clone();
        untimed[1].intervention.as_mut().unwrap().outcome_at = None;
        let far = LedgerConfig::new(later + Duration::days(1), 0);
        assert_eq!(
            ReturnLedger::build(&untimed, &far).counts.disputed_blocks,
            0
        );
    }

    #[test]
    fn a_clear_difference_surfaces_and_the_withhold_candidate_stays_declared() {
        // 30 blocks: communication departures rarely come back, the rest do.
        let outcomes: Vec<(bool, &str)> = (0..30)
            .flat_map(|index| {
                [
                    (index % 5 == 0, "COMMUNICATION"),
                    (index % 5 != 0, "REFERENCE"),
                ]
            })
            .collect();
        let blocks = history(&outcomes, Duration::hours(8));
        let ledger = ReturnLedger::build(&blocks, &LedgerConfig::new(at(30 * 86_400), 0));
        assert_eq!(ledger.abstention, None, "{:?}", ledger.counts);
        assert!(
            ledger
                .surfaced()
                .contains(&(ReturnCell::Communication, CellDirection::Lower)),
            "{:#?}",
            ledger.cells
        );

        let reference = DecisionContext {
            departure: ReturnCell::WorkAdjacent,
            elapsed: ReturnCell::FirstThird,
            hour: ReturnCell::Morning,
            switch_count: 1,
            verdict: GateVerdict::AbstainedMinSwitches,
        };
        let candidate = ledger.would_withhold(&reference);
        assert_eq!(candidate.support, WithholdSupport::WithinCountedRows);
        assert_eq!(candidate.model_version, RETURN_LEDGER_MODEL_VERSION);
        assert!(candidate.min_lower.is_some());

        // At an offered point the counted rows say nothing: it is flagged.
        let offered = DecisionContext {
            switch_count: 3,
            verdict: GateVerdict::Offered,
            ..reference
        };
        assert_eq!(
            ledger.would_withhold(&offered).support,
            WithholdSupport::Extrapolated
        );
        let beyond = DecisionContext {
            switch_count: 3,
            ..reference
        };
        assert_eq!(
            ledger.would_withhold(&beyond).support,
            WithholdSupport::Extrapolated
        );

        // Communication departures do not come back on their own, so the
        // candidate never fires there.
        let communication = DecisionContext {
            departure: ReturnCell::Communication,
            ..reference
        };
        assert!(!ledger.would_withhold(&communication).would_withhold);
    }

    #[test]
    fn the_same_rows_give_the_same_ledger_bit_for_bit() {
        let outcomes: Vec<(bool, &str)> = (0..20)
            .map(|index| (index % 3 != 0, "COMMUNICATION"))
            .collect();
        let blocks = history(&outcomes, Duration::hours(8));
        let config = LedgerConfig::new(at(40 * 86_400), 0);
        let mut reversed = blocks.clone();
        reversed.reverse();
        assert_eq!(
            ReturnLedger::build(&blocks, &config),
            ReturnLedger::build(&reversed, &config),
            "input order changed the ledger"
        );
    }

    #[test]
    fn the_result_is_stamped_with_every_version_it_depends_on() {
        let ledger = ReturnLedger::build(&[], &LedgerConfig::new(at(0), 0));
        assert_eq!(ledger.model_version, RETURN_LEDGER_MODEL_VERSION);
        assert_eq!(ledger.feature_contract_version, FEATURE_CONTRACT_VERSION);
        assert_eq!(ledger.policy_version, DRIFT_POLICY_VERSION);
        assert_eq!(ledger.counts, LedgerCounts::default());
        assert_eq!(ledger.max_switch_count_counted, None);
    }

    #[test]
    fn the_label_matches_the_pre_registered_outcome_and_the_contract() {
        assert_eq!(RETURN_HORIZON_SECONDS, 900);
        assert_eq!(SUSTAINED_ANCHOR_SECONDS, 600);
    }

    #[test]
    fn the_special_functions_match_closed_forms() {
        assert!(approx(ln_gamma(1.0), 0.0, 1e-12));
        assert!(approx(ln_gamma(5.0), 24.0_f64.ln(), 1e-11));
        for p in [0.05, 0.1, 0.5, 0.9, 0.95] {
            // Beta(1, 1) is uniform; Beta(a, 1) has CDF x^a; Beta(1, b) has
            // CDF 1 - (1 - x)^b.
            assert!(approx(beta_quantile(p, 1.0, 1.0), p, 1e-9));
            assert!(approx(beta_quantile(p, 3.0, 1.0), p.powf(1.0 / 3.0), 1e-9));
            assert!(approx(
                beta_quantile(p, 1.0, 4.0),
                1.0 - (1.0 - p).powf(0.25),
                1e-9
            ));
        }
        assert!(approx(beta_quantile(0.5, 7.0, 7.0), 0.5, 1e-9));
        for (tail, z) in [
            (0.025, 1.959_963_985),
            (0.10, 1.281_551_566),
            (0.005, 2.575_829_304),
        ] {
            assert!(
                approx(normal_upper_quantile(tail), z, 2e-6),
                "{tail}: {}",
                normal_upper_quantile(tail)
            );
        }
    }

    // -----------------------------------------------------------------------
    // No caller
    // -----------------------------------------------------------------------

    fn sources(root: &Path, extension: &str, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                sources(&path, extension, found);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                found.push(path);
            }
        }
    }

    /// Every file the tripwire reads, for a service directory laid out like
    /// `rust-service/`: its sources, the shared types, and the Swift client
    /// and IPC schema beside it.
    fn shipped_sources(service: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        sources(&service.join("src"), "rs", &mut files);
        sources(&service.join("shared-types").join("src"), "rs", &mut files);
        sources(
            &service.join("../swift-client/Sources"),
            "swift",
            &mut files,
        );
        sources(&service.join("../proto"), "json", &mut files);
        files
    }

    /// Every public item this file declares at the top level, read from the
    /// file itself, so an item added tomorrow is covered the day it lands.
    fn public_items() -> Vec<String> {
        const KEYWORDS: [&str; 8] = [
            "const ", "static ", "fn ", "struct ", "enum ", "type ", "trait ", "mod ",
        ];
        include_str!("returns.rs")
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("pub ")?;
                let rest = KEYWORDS
                    .iter()
                    .find_map(|keyword| rest.strip_prefix(keyword))?;
                let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }

    /// Spellings a Swift or JSON surface would give what the ledger computes.
    const SURFACE_NEEDLES: [&str; 6] = [
        "return_ledger",
        "returnLedger",
        "would_withhold",
        "wouldWithhold",
        "withhold_candidate",
        "withholdCandidate",
    ];

    /// Paths that reach the module: through `behavior`, through any prefix
    /// (`super::`, `self::`, `crate::behavior::`), or as a file.
    const PATH_NEEDLES: [&str; 3] = ["behavior::returns", "returns::", "returns.rs"];

    fn is_ident(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    /// `text` with every run of whitespace made one space and none around
    /// `::`, so a path or an alias split across lines or spaced out is still
    /// one string.
    fn flatten(text: &str) -> String {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(" ::", "::")
            .replace(":: ", "::")
    }

    /// Whether `needle` occurs in `text` and is not part of a longer
    /// identifier at either end.
    fn has_token(text: &str, needle: &str) -> bool {
        let starts_ident = needle.starts_with(is_ident);
        let ends_ident = needle.ends_with(is_ident);
        text.match_indices(needle).any(|(start, _)| {
            let before = text[..start].chars().next_back();
            let after = text[start + needle.len()..].chars().next();
            let glued_before = starts_ident && before.is_some_and(is_ident);
            let glued_after = ends_ident && after.is_some_and(is_ident);
            !(glued_before || glued_after)
        })
    }

    /// `returns as <alias>` followed by `;`, `,` or `}`: an aliased import of
    /// the module, and not the word "returns" in a sentence.
    fn aliases_the_module(flat: &str) -> bool {
        flat.match_indices("returns as ").any(|(start, needle)| {
            if flat[..start].chars().next_back().is_some_and(is_ident) {
                return false;
            }
            let rest = &flat[start + needle.len()..];
            let alias = rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len());
            alias > 0
                && matches!(
                    rest[alias..].trim_start().chars().next(),
                    Some(';' | ',' | '}')
                )
        })
    }

    /// Why `text` names this module, if it does.
    fn names_the_ledger(text: &str, items: &[String]) -> Option<String> {
        let flat = flatten(text);
        if let Some(path) = PATH_NEEDLES.iter().find(|path| has_token(&flat, path)) {
            return Some(format!("the path `{path}`"));
        }
        if aliases_the_module(&flat) {
            return Some("an aliased import of the module".to_owned());
        }
        if let Some(needle) = SURFACE_NEEDLES.iter().find(|needle| flat.contains(*needle)) {
            return Some(format!("`{needle}`"));
        }
        items
            .iter()
            .find(|item| has_token(&flat, item))
            .map(|item| format!("the item `{item}`"))
    }

    /// Every file under `service` that names this module, and why. The one
    /// exemption is this file, `<service>/src/behavior/returns.rs`. Returns
    /// the files read as well, so a caller can check the scan was not vacuous.
    fn callers(service: &Path) -> (Vec<PathBuf>, Vec<String>) {
        let this_file = service.join("src").join("behavior").join("returns.rs");
        let items = public_items();
        let files = shipped_sources(service);
        let offenders = files
            .iter()
            .filter(|path| **path != this_file)
            .filter_map(|path| {
                let text = std::fs::read_to_string(path).unwrap_or_default();
                names_the_ledger(&text, &items).map(|why| format!("{} names {why}", path.display()))
            })
            .collect();
        (files, offenders)
    }

    /// Nothing outside this file may name it: not the drift gate, delivery,
    /// the IPC router, any copy, the entry point, the Swift client or the IPC
    /// schema. The module is shadow only until randomized rows exist and a
    /// dated decision says otherwise; this is the tripwire.
    ///
    /// Every public item is matched by name, so each must be unique in the
    /// scanned tree. If another module ever declares one of the same name,
    /// this fails: rename the one here rather than weaken the scan.
    #[test]
    fn nothing_in_the_shipped_path_calls_the_return_ledger() {
        let service = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (files, offenders) = callers(service);
        assert!(
            offenders.is_empty(),
            "the return ledger is shadow only and must have no caller:\n{}",
            offenders.join("\n")
        );

        // The scan is not vacuous: it read the paths that matter, every kind
        // of file, and every public item of this module.
        for must in [
            "src/work_block/mod.rs",
            "src/delivery/mod.rs",
            "src/ipc/router.rs",
            "src/receipts/mod.rs",
            "src/behavior/mod.rs",
            "src/main.rs",
            "src/lib.rs",
            "shared-types/src/lib.rs",
        ] {
            assert!(
                files.iter().any(|path| path.ends_with(must)),
                "the no-caller scan never read {must}"
            );
        }
        for extension in ["swift", "json"] {
            assert!(
                files
                    .iter()
                    .any(|path| path.extension().is_some_and(|ext| ext == extension)),
                "the no-caller scan read no .{extension} file"
            );
        }
        let items = public_items();
        for item in [
            "ReturnLedger",
            "context_of",
            "departure_rows",
            "horizon_outcome",
            "departure_cell",
            "LedgerConfig",
            "BlockEvidence",
            "CellAccounting",
            "WithholdSupport",
            "RETURN_LEDGER_MODEL_VERSION",
        ] {
            assert!(
                items.iter().any(|found| found == item),
                "{item} is not in {items:?}"
            );
        }
        assert!(items.len() >= 45, "only {} public items read", items.len());
    }

    /// A tripwire is worth something only if it fires. Each caller below is
    /// planted into a copy of the layout the real scan reads, and the same
    /// scan must name the file it is in, for the reason given. Prose and near
    /// misses must not trip it, or it would be switched off.
    #[test]
    fn the_no_caller_tripwire_fires_on_planted_callers_and_only_on_them() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        // One per process: the unit tests and `trace_replay` both run this.
        let root =
            std::env::temp_dir().join(format!("velvt-returns-tripwire-{}", std::process::id()));
        let _cleanup = Cleanup(root.clone());
        let service = root.join("rust-service");
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let baseline: [(&str, &str); 5] = [
            (
                "rust-service/src/behavior/returns.rs",
                include_str!("returns.rs"),
            ),
            (
                "rust-service/src/behavior/mod.rs",
                "//! [`returns`] is shadow.\npub mod features;\npub mod returns;\n",
            ),
            (
                "rust-service/shared-types/src/lib.rs",
                "pub struct Snapshot;\n",
            ),
            (
                "swift-client/Sources/VelvtMac/UI/View.swift",
                "struct View {}\n",
            ),
            (
                "proto/schema/work_block_state.json",
                "{\"type\": \"object\"}\n",
            ),
        ];
        let reset = || {
            let _ = std::fs::remove_dir_all(&root);
            for (relative, text) in baseline {
                write(relative, text);
            }
        };

        reset();
        let (files, offenders) = callers(&service);
        assert_eq!(files.len(), baseline.len(), "{files:?}");
        assert!(
            offenders.is_empty(),
            "the clean copy tripped: {offenders:?}"
        );

        let planted: [(&str, &str, &str); 13] = [
            // The review's evasion: an aliased import, then a call through it.
            (
                "rust-service/src/ipc/router.rs",
                "use crate::behavior::returns as r;\n\
                 fn route() { let _ = r::context_of; }\n",
                "`behavior::returns`",
            ),
            (
                "rust-service/src/ipc/router.rs",
                "use super::super::behavior::{features, returns as ledger};\n\
                 fn route() { let _ = ledger::context_of; }\n",
                "aliased import",
            ),
            (
                "rust-service/src/ipc/router.rs",
                "use crate::behavior::{returns\n    as\n    r};\n",
                "aliased import",
            ),
            (
                "rust-service/src/work_block/mod.rs",
                "use crate::behavior::returns;\n",
                "`behavior::returns`",
            ),
            (
                "rust-service/src/work_block/mod.rs",
                "use super::behavior::{returns::*};\n",
                "`returns::`",
            ),
            (
                "rust-service/src/work_block/mod.rs",
                "fn gate() { let _ = returns :: departure_rows; }\n",
                "`returns::`",
            ),
            (
                "rust-service/src/delivery/mod.rs",
                "#[path = \"../behavior/returns.rs\"]\nmod copy;\n",
                "`returns.rs`",
            ),
            // A bare item, reached through a re-export somewhere else.
            (
                "rust-service/src/delivery/mod.rs",
                "fn f(block: &Evidence) -> usize { horizon_outcome_of(block) }\n\
                 fn g() -> u32 { RETURN_LEDGER_MODEL_VERSION }\n",
                "`RETURN_LEDGER_MODEL_VERSION`",
            ),
            (
                "rust-service/src/main.rs",
                "fn main() { let _ = LedgerConfig::new; }\n",
                "`LedgerConfig`",
            ),
            (
                "rust-service/src/behavior/mod.rs",
                "pub mod returns;\npub use self::returns::ReturnLedger as Profile;\n",
                "`returns::`",
            ),
            (
                "rust-service/shared-types/src/lib.rs",
                "pub struct Snapshot { pub would_withhold: bool }\n",
                "`would_withhold`",
            ),
            (
                "swift-client/Sources/VelvtMac/UI/View.swift",
                "let quiet = snapshot.wouldWithhold\n",
                "`wouldWithhold`",
            ),
            (
                "proto/schema/work_block_state.json",
                "{\"return_ledger\": {\"type\": \"object\"}}\n",
                "`return_ledger`",
            ),
        ];
        for (relative, text, reason) in planted {
            reset();
            write(relative, text);
            let (_, offenders) = callers(&service);
            let named: Vec<&String> = offenders
                .iter()
                .filter(|offender| offender.contains(relative))
                .collect();
            assert!(
                named.len() == 1 && named[0].contains(reason),
                "the tripwire missed {relative} ({reason}):\n{text}\nit reported {offenders:?}"
            );
            assert_eq!(offenders.len(), 1, "{offenders:?}");
        }

        for prose in [
            "/// The engine returns as `user_rule` once it is done.\n",
            "// This returns as soon as the block ends.\n",
            "use std::cell::RefCell;\nfn early_returns() -> RefCell<u8> { RefCell::new(0) }\n",
            "fn f() { let returns = 3; let _ = returns; }\n",
            "fn g() -> bool { self.horizon_outcomes.is_empty() }\n",
        ] {
            reset();
            write("rust-service/src/ipc/router.rs", prose);
            let (_, offenders) = callers(&service);
            assert!(
                offenders.is_empty(),
                "prose tripped it: {prose}{offenders:?}"
            );
        }
    }
}
