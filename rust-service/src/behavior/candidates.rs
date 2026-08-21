//! The closed, versioned candidate registry for antecedent mining.
//!
//! Per `03-BEHAVIORAL-ENGINE-SPEC.md` § 3.2. This module holds no statistics
//! and makes no decision. It answers exactly one question — *is antecedent `A`
//! present, absent, or unobserved for this episode* — and it answers it for a
//! fixed set of candidates whose size is a compile-time constant.
//!
//! # Why the family size is a constant and not a count of what was tested
//!
//! A family whose size depends on the data is a family the data chose. Letting
//! low-support candidates drop out of the denominator is the classic way a
//! multiplicity correction is quietly defeated: the analyst tests 108
//! hypotheses, corrects over the 19 that happened to have support, and reports
//! an FDR guarantee that was never earned.
//!
//! So [`FAMILY_SIZE`] is fixed at compile time, it is logged with every result,
//! and it counts **candidates x outcomes x horizons** rather than candidates
//! alone — because a second outcome definition or a second horizon multiplies
//! the number of chances to be wrong and must multiply the correction with it.
//! A candidate that fails the support floor is not removed from the family; it
//! abstains, and it stays in the denominator as an untested null.
//!
//! # Two deliberate departures from the spec table, both upward
//!
//! 1. **Time-of-day is 8 bins, not 6.** The spec says "3h bins | 6", which is
//!    arithmetically impossible: three-hour bins over a 24-hour day give eight.
//!    Six would require either four-hour bins or dropping six hours of the day,
//!    and dropping hours makes the dimension non-exhaustive — an episode at
//!    04:00 would belong to no bin and silently sit in the "absent" arm of
//!    every time-of-day candidate. Eight exhaustive bins, and the family size
//!    grows by two.
//!
//! 2. **All 64 ordered transitions are registered, not the ~15-25 that reach
//!    support.** The spec's "≈63-80 marginal candidates" is only reachable by
//!    letting the data pick which transitions are in the family. See above.
//!    Registering all 64 and gating at evaluation costs a larger denominator
//!    and buys a correction that is actually valid.
//!
//! The registered total is therefore [`CANDIDATE_COUNT`] = 108 rather than the
//! spec's 63-80. That is the honest number and it is the one that is logged.
//!
//! # Marginals only
//!
//! V1 tests marginals. Permitting 2-way conjunctions takes the space to roughly
//! 2,200 and the expected false-discovery count from ~3 to ~110 (`03` § 3.2).
//! There is no conjunction constructor in this file, so enabling them is a code
//! change and a registry version bump, not a config flag.

// The registry ships before its only consumer's caller does: nothing in the
// shipped path mines antecedents, by design (`03` § 5). A module-level allow
// keeps adding a level to the registry a one-line change.
#![allow(dead_code)]

/// The closed category vocabulary, `|C| = 8`. Restated here rather than
/// imported so this module compiles standalone in the validation test crate;
/// `behavior/mod.rs` asserts it against the frozen feature contract.
pub const CATEGORY_COUNT: usize = 8;

/// Mirrors `behavior/features.rs::CATEGORIES`, which mirrors the shipped
/// taxonomy. Order is load-bearing: candidate ids embed the category name, and
/// a reordering that changed a name-to-index mapping would silently repoint
/// every stored finding.
pub const CATEGORIES: [&str; CATEGORY_COUNT] = [
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    "UNLOGGED",
];

/// The version of this registry. Stored on every finding. A finding discovered
/// under one registry version is not comparable to one discovered under
/// another, and `antecedent_finding.retraction_reason` carries
/// `registry_version_change` for exactly that reason.
pub const CANDIDATE_REGISTRY_VERSION: u32 = 1;

// --- Dimension cardinalities. Each is a separate constant so that a change to
// --- one is a one-line diff whose effect on FAMILY_SIZE is mechanical.

/// Three-hour bins over the whole 24-hour day. Eight, not the spec's six; see
/// the module docs.
pub const TIME_OF_DAY_BINS: usize = 8;
/// Weekday, weekend.
pub const DAY_TYPE_LEVELS: usize = 2;
/// The category of the run immediately preceding the episode onset.
pub const PRECEDING_CATEGORY_LEVELS: usize = CATEGORY_COUNT;
/// The ordered pair `(c_{t-2}, c_{t-1})` immediately preceding onset. All 64
/// are registered; support gating happens at evaluation.
pub const PRECEDING_TRANSITION_LEVELS: usize = CATEGORY_COUNT * CATEGORY_COUNT;
/// Block elapsed at onset: 0-10, 10-25, 25-45, 45+ minutes.
pub const BLOCK_ELAPSED_BINS: usize = 4;
/// System Focus/DND active or not.
pub const FOCUS_LEVELS: usize = 2;
/// The prior block's terminal phase.
pub const PRIOR_BLOCK_PHASE_LEVELS: usize = 5;
/// The prior block's intervention outcome, from the shipped closed vocabulary.
pub const PRIOR_INTERVENTION_OUTCOME_LEVELS: usize = 9;
/// Run index within the block at onset: 0-2, 3-7, 8+.
pub const RUN_INDEX_BINS: usize = 3;
/// Gap since the previous run ended: <60s, 60-300s, 300s+.
pub const GAP_BINS: usize = 3;

/// The prior block's phase vocabulary, from `0009_work_blocks.sql`.
pub const BLOCK_PHASES: [&str; PRIOR_BLOCK_PHASE_LEVELS] =
    ["active", "paused", "completed", "abandoned", "expired"];

/// The prior block's intervention outcome vocabulary, from
/// `0020_delivery_suppressed_dnd_outcome.sql`, which is the current head of the
/// vocabulary chain that began at `0015`. Restated here so that a vocabulary
/// that grows without this registry growing with it fails a test rather than
/// silently shrinking the family.
pub const INTERVENTION_OUTCOMES: [&str; PRIOR_INTERVENTION_OUTCOME_LEVELS] = [
    "offered",
    "accepted_action",
    "returned",
    "not_helpful",
    "wrong_classification",
    "was_focused",
    "dismissed",
    "delivery_suppressed_dnd",
    "no_response",
];

/// The number of registered candidates. **Marginals only.**
pub const CANDIDATE_COUNT: usize = TIME_OF_DAY_BINS
    + DAY_TYPE_LEVELS
    + PRECEDING_CATEGORY_LEVELS
    + PRECEDING_TRANSITION_LEVELS
    + BLOCK_ELAPSED_BINS
    + FOCUS_LEVELS
    + PRIOR_BLOCK_PHASE_LEVELS
    + PRIOR_INTERVENTION_OUTCOME_LEVELS
    + RUN_INDEX_BINS
    + GAP_BINS;

/// Outcome definitions in the family. **One** in v1: `Y = 1` iff no confident
/// anchor observation follows the onset within the horizon (`03` § 3.1).
///
/// This is a multiplier on [`FAMILY_SIZE`], not a comment. Adding a second
/// outcome — "the block was abandoned", say — doubles the number of chances to
/// be wrong, and the correction has to know that.
pub const OUTCOME_COUNT: usize = 1;

/// Horizons in the family. **One** in v1: 600 seconds, the same horizon the
/// decision log already backfills, so the antecedent layer and the policy layer
/// cannot disagree about what "shortly afterwards" means.
pub const HORIZON_COUNT: usize = 1;

/// The logged family size: **candidates x outcomes x horizons**.
///
/// This is the Benjamini-Hochberg denominator. It is a compile-time constant,
/// it is written into every result, and it does not move when the data is
/// sparse.
pub const FAMILY_SIZE: usize = CANDIDATE_COUNT * OUTCOME_COUNT * HORIZON_COUNT;

/// The single outcome definition's identifier, logged with every result.
pub const OUTCOME_ID: &str = "no_confident_anchor_within_horizon";

/// The single horizon, in seconds. Matches
/// `features::PROXIMAL_OUTCOME_HORIZON_SECONDS`.
pub const HORIZON_SECONDS: i64 = 600;

/// Which dimension a candidate belongs to. Used for reporting only — the
/// analysis never groups by dimension, because grouping would be a second,
/// undeclared look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dimension {
    TimeOfDay,
    DayType,
    PrecedingCategory,
    PrecedingTransition,
    BlockElapsed,
    Focus,
    PriorBlockPhase,
    PriorInterventionOutcome,
    RunIndex,
    Gap,
}

impl Dimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::TimeOfDay => "time_of_day",
            Dimension::DayType => "day_type",
            Dimension::PrecedingCategory => "preceding_category",
            Dimension::PrecedingTransition => "preceding_transition",
            Dimension::BlockElapsed => "block_elapsed",
            Dimension::Focus => "focus",
            Dimension::PriorBlockPhase => "prior_block_phase",
            Dimension::PriorInterventionOutcome => "prior_intervention_outcome",
            Dimension::RunIndex => "run_index",
            Dimension::Gap => "gap",
        }
    }
}

/// Whether an antecedent was present, absent, or **not observed at all** for a
/// given episode.
///
/// The third value is the one that matters. An episode that is the first of its
/// block has no preceding run, so "the preceding category was COMMUNICATION" is
/// not false for it — it is unasked. Folding unobserved into absent would put
/// every first-of-block episode into the control arm of all eight
/// preceding-category candidates and manufacture an association out of block
/// position. Unobserved episodes are dropped from **both** arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
    Unobserved,
}

/// The per-episode inputs the registry reads. Every field is either a closed
/// vocabulary index or a coarse number; none can hold an application name, a
/// label, a window title, a URL, or intention text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeFeatures {
    /// Local hour, 0-23, from `focus_observer_state.utc_offset_seconds`.
    pub local_hour: u8,
    /// Weekend, per the local calendar.
    pub weekend: bool,
    /// Category index of the run immediately before onset. `None` when the
    /// onset is the first run of its block.
    pub preceding_category: Option<u8>,
    /// `(c_{t-2}, c_{t-1})`. `None` when fewer than two runs precede onset.
    pub preceding_transition: Option<(u8, u8)>,
    /// Seconds elapsed in the declared block at onset.
    pub block_elapsed_seconds: i64,
    /// System Focus/DND active at onset.
    pub focus_active: bool,
    /// The previous block's terminal phase, as an index into [`BLOCK_PHASES`].
    /// `None` when this is the first block observed.
    pub prior_block_phase: Option<u8>,
    /// The previous block's intervention outcome, as an index into
    /// [`INTERVENTION_OUTCOMES`]. `None` when the previous block carried no
    /// intervention — which is not the same as an intervention that produced
    /// no response, and is why this is an `Option` and not a tenth level.
    pub prior_intervention_outcome: Option<u8>,
    /// Run index within the block at onset.
    pub run_index: u32,
    /// Seconds between the end of the previous run and this onset. `None` when
    /// there is no previous run.
    pub gap_seconds: Option<i64>,
}

/// One registered antecedent.
///
/// `Copy` and payload-carrying rather than a string id, so that a candidate
/// that is not in the registry cannot be constructed by typo. [`Candidate::id`]
/// produces the stable string the database stores, and [`from_id`] is the only
/// way back — a stored id that does not round-trip is a finding for a candidate
/// that does not exist, and it must not load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Candidate {
    /// Three-hour bin index, 0-7. Bin `b` covers `[3b, 3b+3)`.
    TimeOfDay(u8),
    /// `true` = weekend.
    DayType(bool),
    PrecedingCategory(u8),
    PrecedingTransition(u8, u8),
    /// 0: 0-10 min, 1: 10-25, 2: 25-45, 3: 45+.
    BlockElapsed(u8),
    FocusActive(bool),
    PriorBlockPhase(u8),
    PriorInterventionOutcome(u8),
    /// 0: runs 0-2, 1: 3-7, 2: 8+.
    RunIndex(u8),
    /// 0: <60s, 1: 60-300s, 2: 300s+.
    Gap(u8),
}

/// Block-elapsed bin edges in seconds, from `03` § 3.2's `{0-10, 10-25, 25-45,
/// 45+}` minutes.
const BLOCK_ELAPSED_EDGES: [i64; 3] = [600, 1500, 2700];
/// Run-index bin edges: `0-2 | 3-7 | 8+`.
const RUN_INDEX_EDGES: [u32; 2] = [3, 8];
/// Gap bin edges in seconds: `<60 | 60-300 | 300+`.
const GAP_EDGES: [i64; 2] = [60, 300];

fn bin_i64(value: i64, edges: &[i64]) -> u8 {
    let mut bin = 0u8;
    for edge in edges {
        if value >= *edge {
            bin += 1;
        }
    }
    bin
}

fn bin_u32(value: u32, edges: &[u32]) -> u8 {
    let mut bin = 0u8;
    for edge in edges {
        if value >= *edge {
            bin += 1;
        }
    }
    bin
}

impl Candidate {
    pub fn dimension(self) -> Dimension {
        match self {
            Candidate::TimeOfDay(_) => Dimension::TimeOfDay,
            Candidate::DayType(_) => Dimension::DayType,
            Candidate::PrecedingCategory(_) => Dimension::PrecedingCategory,
            Candidate::PrecedingTransition(_, _) => Dimension::PrecedingTransition,
            Candidate::BlockElapsed(_) => Dimension::BlockElapsed,
            Candidate::FocusActive(_) => Dimension::Focus,
            Candidate::PriorBlockPhase(_) => Dimension::PriorBlockPhase,
            Candidate::PriorInterventionOutcome(_) => Dimension::PriorInterventionOutcome,
            Candidate::RunIndex(_) => Dimension::RunIndex,
            Candidate::Gap(_) => Dimension::Gap,
        }
    }

    /// The stable identifier stored in `antecedent_finding.candidate_id`.
    ///
    /// Deliberately human-readable and deliberately **not** a copy string: it
    /// is an analysis key, it never reaches a user, and the copy layer selects
    /// its own template from a closed set. Nothing here is a sentence.
    pub fn id(self) -> String {
        match self {
            Candidate::TimeOfDay(bin) => format!("tod_{:02}_{:02}", bin * 3, bin * 3 + 3),
            Candidate::DayType(weekend) => {
                format!("daytype_{}", if weekend { "weekend" } else { "weekday" })
            }
            Candidate::PrecedingCategory(category) => {
                format!("prevcat_{}", CATEGORIES[category as usize])
            }
            Candidate::PrecedingTransition(from, to) => format!(
                "prevtrans_{}__{}",
                CATEGORIES[from as usize], CATEGORIES[to as usize]
            ),
            Candidate::BlockElapsed(bin) => format!("elapsed_{bin}"),
            Candidate::FocusActive(active) => {
                format!("focus_{}", if active { "active" } else { "inactive" })
            }
            Candidate::PriorBlockPhase(phase) => {
                format!("priorphase_{}", BLOCK_PHASES[phase as usize])
            }
            Candidate::PriorInterventionOutcome(outcome) => {
                format!("prioroutcome_{}", INTERVENTION_OUTCOMES[outcome as usize])
            }
            Candidate::RunIndex(bin) => format!("runidx_{bin}"),
            Candidate::Gap(bin) => format!("gap_{bin}"),
        }
    }

    /// Whether this antecedent is present, absent, or unobserved for `features`.
    pub fn presence(self, features: &EpisodeFeatures) -> Presence {
        fn from_bool(matched: bool) -> Presence {
            if matched {
                Presence::Present
            } else {
                Presence::Absent
            }
        }
        match self {
            Candidate::TimeOfDay(bin) => from_bool(features.local_hour / 3 == bin),
            Candidate::DayType(weekend) => from_bool(features.weekend == weekend),
            Candidate::PrecedingCategory(category) => match features.preceding_category {
                Some(observed) => from_bool(observed == category),
                None => Presence::Unobserved,
            },
            Candidate::PrecedingTransition(from, to) => match features.preceding_transition {
                Some(observed) => from_bool(observed == (from, to)),
                None => Presence::Unobserved,
            },
            Candidate::BlockElapsed(bin) => {
                from_bool(bin_i64(features.block_elapsed_seconds, &BLOCK_ELAPSED_EDGES) == bin)
            }
            Candidate::FocusActive(active) => from_bool(features.focus_active == active),
            Candidate::PriorBlockPhase(phase) => match features.prior_block_phase {
                Some(observed) => from_bool(observed == phase),
                None => Presence::Unobserved,
            },
            Candidate::PriorInterventionOutcome(outcome) => {
                match features.prior_intervention_outcome {
                    Some(observed) => from_bool(observed == outcome),
                    None => Presence::Unobserved,
                }
            }
            Candidate::RunIndex(bin) => {
                from_bool(bin_u32(features.run_index, &RUN_INDEX_EDGES) == bin)
            }
            Candidate::Gap(bin) => match features.gap_seconds {
                Some(gap) => from_bool(bin_i64(gap, &GAP_EDGES) == bin),
                None => Presence::Unobserved,
            },
        }
    }
}

/// The registry, in a fixed order. Length is [`CANDIDATE_COUNT`] and there is a
/// test that says so.
///
/// Rebuilt on each call rather than cached: it is 108 enum values, the caller
/// builds it once per analysis, and a `OnceLock` would be a cache with no
/// invalidation story for a registry that is versioned.
pub fn registry() -> Vec<Candidate> {
    let mut candidates = Vec::with_capacity(CANDIDATE_COUNT);
    for bin in 0..TIME_OF_DAY_BINS as u8 {
        candidates.push(Candidate::TimeOfDay(bin));
    }
    candidates.push(Candidate::DayType(false));
    candidates.push(Candidate::DayType(true));
    for category in 0..CATEGORY_COUNT as u8 {
        candidates.push(Candidate::PrecedingCategory(category));
    }
    for from in 0..CATEGORY_COUNT as u8 {
        for to in 0..CATEGORY_COUNT as u8 {
            candidates.push(Candidate::PrecedingTransition(from, to));
        }
    }
    for bin in 0..BLOCK_ELAPSED_BINS as u8 {
        candidates.push(Candidate::BlockElapsed(bin));
    }
    candidates.push(Candidate::FocusActive(false));
    candidates.push(Candidate::FocusActive(true));
    for phase in 0..PRIOR_BLOCK_PHASE_LEVELS as u8 {
        candidates.push(Candidate::PriorBlockPhase(phase));
    }
    for outcome in 0..PRIOR_INTERVENTION_OUTCOME_LEVELS as u8 {
        candidates.push(Candidate::PriorInterventionOutcome(outcome));
    }
    for bin in 0..RUN_INDEX_BINS as u8 {
        candidates.push(Candidate::RunIndex(bin));
    }
    for bin in 0..GAP_BINS as u8 {
        candidates.push(Candidate::Gap(bin));
    }
    candidates
}

/// The only way back from a stored id. A finding whose `candidate_id` is not in
/// the registry cannot be loaded, which is the same closed-registry discipline
/// the intervention action ids and explanation claims already use.
pub fn from_id(id: &str) -> Option<Candidate> {
    registry()
        .into_iter()
        .find(|candidate| candidate.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_registry_is_exactly_the_compile_time_family() {
        assert_eq!(registry().len(), CANDIDATE_COUNT);
        assert_eq!(CANDIDATE_COUNT, 108);
        assert_eq!(FAMILY_SIZE, CANDIDATE_COUNT * OUTCOME_COUNT * HORIZON_COUNT);
    }

    /// The family size must count outcomes and horizons. If someone adds a
    /// second outcome definition without multiplying the family, the FDR
    /// denominator is silently half what it should be.
    #[test]
    fn the_family_size_multiplies_outcomes_and_horizons() {
        // Restated as arithmetic rather than as a comment, so the relationship
        // survives an edit to any one of the three.
        assert_eq!(FAMILY_SIZE % CANDIDATE_COUNT, 0);
        assert_eq!(FAMILY_SIZE / CANDIDATE_COUNT, OUTCOME_COUNT * HORIZON_COUNT);
    }

    #[test]
    fn candidate_ids_are_unique_and_round_trip() {
        let candidates = registry();
        let ids: BTreeSet<String> = candidates.iter().map(|c| c.id()).collect();
        assert_eq!(ids.len(), candidates.len(), "duplicate candidate id");
        for candidate in candidates {
            assert_eq!(from_id(&candidate.id()), Some(candidate));
        }
        assert_eq!(from_id("prevcat_NOT_A_CATEGORY"), None);
        assert_eq!(from_id(""), None);
    }

    /// Every dimension must be exhaustive over its observable range, or an
    /// episode falls into no level and sits in the control arm of the entire
    /// dimension. Checked by construction over the full input range.
    #[test]
    fn each_dimension_puts_every_episode_in_exactly_one_level() {
        let candidates = registry();
        let base = EpisodeFeatures {
            local_hour: 0,
            weekend: false,
            preceding_category: Some(0),
            preceding_transition: Some((0, 0)),
            block_elapsed_seconds: 0,
            focus_active: false,
            prior_block_phase: Some(0),
            prior_intervention_outcome: Some(0),
            run_index: 0,
            gap_seconds: Some(0),
        };
        let mut probes = Vec::new();
        for hour in 0..24u8 {
            probes.push(EpisodeFeatures {
                local_hour: hour,
                ..base
            });
        }
        for elapsed in [0i64, 599, 600, 1499, 1500, 2699, 2700, 100_000] {
            probes.push(EpisodeFeatures {
                block_elapsed_seconds: elapsed,
                ..base
            });
        }
        for index in [0u32, 2, 3, 7, 8, 5000] {
            probes.push(EpisodeFeatures {
                run_index: index,
                ..base
            });
        }
        for gap in [0i64, 59, 60, 299, 300, 86_400] {
            probes.push(EpisodeFeatures {
                gap_seconds: Some(gap),
                ..base
            });
        }
        for category in 0..CATEGORY_COUNT as u8 {
            probes.push(EpisodeFeatures {
                preceding_category: Some(category),
                preceding_transition: Some((category, category)),
                ..base
            });
        }
        for weekend in [false, true] {
            for focus in [false, true] {
                probes.push(EpisodeFeatures {
                    weekend,
                    focus_active: focus,
                    ..base
                });
            }
        }
        for phase in 0..PRIOR_BLOCK_PHASE_LEVELS as u8 {
            probes.push(EpisodeFeatures {
                prior_block_phase: Some(phase),
                ..base
            });
        }
        for outcome in 0..PRIOR_INTERVENTION_OUTCOME_LEVELS as u8 {
            probes.push(EpisodeFeatures {
                prior_intervention_outcome: Some(outcome),
                ..base
            });
        }

        for probe in probes {
            for dimension in [
                Dimension::TimeOfDay,
                Dimension::DayType,
                Dimension::PrecedingCategory,
                Dimension::PrecedingTransition,
                Dimension::BlockElapsed,
                Dimension::Focus,
                Dimension::PriorBlockPhase,
                Dimension::PriorInterventionOutcome,
                Dimension::RunIndex,
                Dimension::Gap,
            ] {
                let present = candidates
                    .iter()
                    .filter(|c| c.dimension() == dimension)
                    .filter(|c| c.presence(&probe) == Presence::Present)
                    .count();
                assert_eq!(
                    present,
                    1,
                    "dimension {} put {present} levels on {probe:?}; exactly one is required",
                    dimension.as_str()
                );
            }
        }
    }

    /// An unobserved input must never land in the absent arm. This is the
    /// assertion that stops block position being laundered into an association
    /// about the preceding category.
    #[test]
    fn an_unobserved_input_is_dropped_from_both_arms() {
        let features = EpisodeFeatures {
            local_hour: 9,
            weekend: false,
            preceding_category: None,
            preceding_transition: None,
            block_elapsed_seconds: 300,
            focus_active: false,
            prior_block_phase: None,
            prior_intervention_outcome: None,
            run_index: 0,
            gap_seconds: None,
        };
        for candidate in registry() {
            let presence = candidate.presence(&features);
            match candidate.dimension() {
                Dimension::PrecedingCategory
                | Dimension::PrecedingTransition
                | Dimension::PriorBlockPhase
                | Dimension::PriorInterventionOutcome
                | Dimension::Gap => assert_eq!(
                    presence,
                    Presence::Unobserved,
                    "{} treated a missing input as an answer",
                    candidate.id()
                ),
                _ => assert_ne!(presence, Presence::Unobserved),
            }
        }
    }

    /// The intervention-outcome vocabulary is the shipped one. If a migration
    /// widens it and this registry does not follow, the family silently shrinks
    /// and the correction is computed over the wrong denominator.
    #[test]
    fn the_intervention_outcome_vocabulary_matches_the_shipped_migration() {
        let sql = include_str!("../../migrations/0020_delivery_suppressed_dnd_outcome.sql");
        let start = sql
            .find("CHECK(outcome IN (")
            .expect("0020 still constrains the outcome vocabulary");
        let body = &sql[start..];
        let end = body.find("))").expect("the CHECK list is closed");
        let listed: Vec<&str> = body[..end]
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim().trim_end_matches(',').trim_matches('\'');
                if trimmed.is_empty() || trimmed.starts_with("CHECK") {
                    None
                } else {
                    Some(trimmed)
                }
            })
            .collect();
        assert_eq!(
            listed, INTERVENTION_OUTCOMES,
            "the shipped intervention-outcome vocabulary moved; the candidate \
             registry's family size is now computed over the wrong denominator"
        );
    }

    /// The phase vocabulary, same reasoning, against `0009`.
    #[test]
    fn the_block_phase_vocabulary_matches_the_shipped_migration() {
        let sql = include_str!("../../migrations/0009_work_blocks.sql");
        for phase in BLOCK_PHASES {
            assert!(
                sql.contains(&format!("'{phase}'")),
                "phase `{phase}` is not in 0009"
            );
        }
    }

    /// Marginals only. There is no conjunction constructor, and this test is
    /// the record that its absence is deliberate rather than an oversight.
    #[test]
    fn the_registry_contains_no_conjunctions() {
        for candidate in registry() {
            let id = candidate.id();
            assert!(
                !id.contains('&'),
                "{id} looks like a conjunction; v1 tests marginals only"
            );
        }
        // Two-way conjunctions over these dimensions would be this many
        // hypotheses. Stated as arithmetic so the cost of enabling them is a
        // number rather than a memory.
        let dimensions = [
            TIME_OF_DAY_BINS,
            DAY_TYPE_LEVELS,
            PRECEDING_CATEGORY_LEVELS,
            PRECEDING_TRANSITION_LEVELS,
            BLOCK_ELAPSED_BINS,
            FOCUS_LEVELS,
            PRIOR_BLOCK_PHASE_LEVELS,
            PRIOR_INTERVENTION_OUTCOME_LEVELS,
            RUN_INDEX_BINS,
            GAP_BINS,
        ];
        let mut pairs = 0usize;
        for (first, left) in dimensions.iter().enumerate() {
            for right in dimensions.iter().skip(first + 1) {
                pairs += left * right;
            }
        }
        assert!(
            pairs > 20 * CANDIDATE_COUNT,
            "the conjunction space is supposed to be an order of magnitude \
             larger; it measured {pairs} against {CANDIDATE_COUNT}"
        );
    }
}
