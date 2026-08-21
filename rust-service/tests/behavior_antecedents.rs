//! Validation of the shadow antecedent miner against synthetic episode traces
//! with known ground truth.
//!
//! `03-BEHAVIORAL-ENGINE-SPEC.md` § 3 calls this "the layer most likely to ship
//! a lie", and the reason is arithmetic: 108 hypotheses tested against a few
//! dozen episodes will produce something that looks like a pattern almost every
//! night. **A confident false pattern is worse than a tracker; it is a
//! confident tracker.** So the null test is the deliverable, and the recovery
//! curve is the honest answer to "how long until Velvt knows something about
//! me" rather than a number chosen to look good.
//!
//! # What this suite is evidence about, and what it is not
//!
//! `SYNTHETIC-suite-d-antecedents.jsonl` is **episode-level**. It is not
//! replayed through the ingestion path, and — this is the important one — its
//! episodes are **not produced by the nightly segmenter**. In the real pipeline
//! an episode onset comes out of `behavior/hmm.rs`, whose own recovery on suite
//! C is `MI(state; routine) = 0.4991` bits against a ceiling of 1.585.
//!
//! So every number here is a claim about **a statistical procedure given
//! correct episodes**. It is not a claim about the composition of segmenter and
//! miner, it is not a claim about the product, and no real user has ever
//! touched any of it. Saying exactly that is stronger than the alternative.
//!
//! # Nothing surfaces
//!
//! The miner has no caller in the shipped path, no IPC message carries a
//! finding, and no copy template renders one. `Analysis::surfaceable()` names
//! the findings a surface *would* be permitted to render if one existed. It
//! does not exist. `antecedent_finding`'s trigger is the backstop, and there is
//! a test below that fires it.
//!
//! # Why the module include, and what should replace it
//!
//! `behavior` is declared in `src/main.rs`, not `src/lib.rs`, so an integration
//! test cannot reach it through `velvt_service`. Until someone who owns
//! `lib.rs` moves it — one line, `pub mod behavior;` — the two miner files are
//! included here by path, exactly as `behavior_segmentation.rs` includes the
//! two segmentation files. The cost is that their unit tests compile and run in
//! two crates.

#[path = "../src/behavior/candidates.rs"]
mod candidates;

#[path = "../src/behavior/antecedents.rs"]
mod antecedents;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use antecedents::{
    analyse, Abstention, Analysis, Controls, Episode, MinerConfig, MIN_SUPPORT_PER_ARM,
    PRE_DECLARED_EPISODE_COUNT,
};
use candidates::EpisodeFeatures;

use velvt_service::persistence::{
    AntecedentFinding, AntecedentFindingState, AntecedentRetractionReason, SqlitePersistence,
};

const FIXTURE: &str = "SYNTHETIC-suite-d-antecedents.jsonl";

/// The alpha a naive analyst would use per candidate. Only ever applied with
/// every control switched off, to measure what the controls are buying.
const NAIVE_ALPHA: f64 = 0.05;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Header {
    kind: String,
    suite: String,
    schema: String,
    synthetic: bool,
    label: String,
    acceptance: String,
    injection_method: String,
    outcome: String,
    families: Vec<String>,
    planted_candidate_id: String,
    total_episodes: usize,
    traces: usize,
}

#[derive(Debug, Deserialize)]
struct GroundTruth {
    planted: bool,
    planted_candidate_id: Option<String>,
    #[serde(default)]
    entailed_candidate_ids: Vec<String>,
    planted_risk_difference: f64,
    episodes: usize,
    days: usize,
    #[serde(default)]
    realised_risk_difference: Option<f64>,
    #[serde(default)]
    realised_present_episodes: Option<usize>,
    #[serde(default)]
    realised_absent_episodes: Option<usize>,
    #[serde(default)]
    risk_given_antecedent: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Trace {
    trace_id: String,
    family: String,
    weeks: usize,
    /// `[day, onset, hour, weekend, prev_cat, cat_before_prev, elapsed, focus,
    /// prior_phase, prior_outcome, run_index, gap, y]`, with `-1` for
    /// unobserved.
    episodes: Vec<[i64; 13]>,
    ground_truth: GroundTruth,
}

impl Trace {
    /// Decode into the miner's episode type.
    ///
    /// `-1` becomes `None`, and `None` means **unobserved** — the miner drops
    /// those episodes from both arms rather than counting them as "the
    /// antecedent was absent". Collapsing the two here would quietly defeat the
    /// three-valued logic the registry was built around.
    fn decoded(&self) -> Vec<Episode> {
        self.episodes
            .iter()
            .map(|row| {
                let preceding = optional_index(row[4]);
                let before = optional_index(row[5]);
                Episode {
                    day_index: row[0],
                    onset_at: row[1],
                    features: EpisodeFeatures {
                        local_hour: row[2] as u8,
                        weekend: row[3] == 1,
                        preceding_category: preceding,
                        preceding_transition: match (before, preceding) {
                            (Some(first), Some(second)) => Some((first, second)),
                            _ => None,
                        },
                        block_elapsed_seconds: row[6],
                        focus_active: row[7] == 1,
                        prior_block_phase: optional_index(row[8]),
                        prior_intervention_outcome: optional_index(row[9]),
                        run_index: row[10] as u32,
                        gap_seconds: if row[11] < 0 { None } else { Some(row[11]) },
                    },
                    outcome: row[12] == 1,
                }
            })
            .collect()
    }
}

fn optional_index(value: i64) -> Option<u8> {
    if value < 0 {
        None
    } else {
        Some(value as u8)
    }
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the rust-service crate sits inside the repo")
        .join("scripts")
        .join("traces")
        .join(FIXTURE)
}

fn load() -> (Header, Vec<Trace>) {
    let raw = fs::read_to_string(fixture_path()).unwrap_or_else(|error| {
        panic!(
            "{} is missing ({error}). Run ./scripts/generate_traces.py",
            fixture_path().display()
        )
    });
    let mut lines = raw.lines();
    let header: Header = serde_json::from_str(lines.next().expect("the suite has a header record"))
        .expect("the header parses");
    let traces: Vec<Trace> = lines
        .map(|line| serde_json::from_str(line).expect("a trace parses"))
        .collect();
    (header, traces)
}

fn family<'a>(traces: &'a [Trace], name: &str) -> Vec<&'a Trace> {
    traces.iter().filter(|trace| trace.family == name).collect()
}

/// The configuration every sweep below uses.
///
/// `single_look_at_episodes` is **disabled** here, and that is a deliberate
/// departure that must be stated wherever a number from this file is quoted.
/// The shipped rule is one look at a pre-declared `n` of
/// [`PRE_DECLARED_EPISODE_COUNT`] episodes; a power sweep whose whole purpose is
/// to vary history length cannot be gated on reaching a fixed history length,
/// or every short cell would read "abstained" and the curve would measure the
/// rule instead of the procedure. The fraction of traces that would clear the
/// pre-declared `n` is reported separately, in the recovery table's own column.
fn sweep_config(controls: Controls) -> MinerConfig {
    MinerConfig {
        single_look_at_episodes: None,
        controls,
        naive_alpha: NAIVE_ALPHA,
        ..MinerConfig::default()
    }
}

/// How a trace's findings relate to what was planted.
#[derive(Debug, Default, Clone, Copy)]
struct Verdict {
    /// The planted candidate itself was surfaced.
    recovered: bool,
    /// A candidate that carries the planted signal **by construction** was
    /// surfaced — `prevtrans_X__COMMUNICATION`, whose second element *is* the
    /// preceding category. Neither a recovery nor a false discovery, and it
    /// gets its own column rather than being folded into whichever one flatters
    /// the result.
    entailed: bool,
    /// A candidate with no relationship to the plant was surfaced. These are
    /// the false discoveries.
    other: usize,
    abstained: bool,
    reached_pre_declared_n: bool,
}

fn verdict(analysis: &Analysis, truth: &GroundTruth, episodes: usize) -> Verdict {
    let entailed: BTreeSet<&str> = truth
        .entailed_candidate_ids
        .iter()
        .map(String::as_str)
        .collect();
    let planted = truth.planted_candidate_id.as_deref();
    let mut verdict = Verdict {
        abstained: analysis.abstention.is_some(),
        reached_pre_declared_n: episodes >= PRE_DECLARED_EPISODE_COUNT,
        ..Verdict::default()
    };
    for finding in analysis.surfaceable() {
        let id = finding.candidate_id.as_str();
        if Some(id) == planted {
            verdict.recovered = true;
        } else if entailed.contains(id) {
            verdict.entailed = true;
        } else {
            verdict.other += 1;
        }
    }
    verdict
}

fn percent(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f64 / total as f64
    }
}

// ---------------------------------------------------------------------------
// The fixture says what it is
// ---------------------------------------------------------------------------

#[test]
fn the_fixture_declares_itself_synthetic_and_uningested() {
    let (header, traces) = load();
    assert_eq!(header.kind, "header");
    assert!(header.synthetic);
    assert!(header.label.contains("SYNTHETIC"));
    assert!(header.suite.starts_with("D"));
    assert_eq!(header.schema, "velvt-episodes/1");
    assert!(
        header.injection_method.starts_with("NONE"),
        "suite D must declare that it is not replayed through the ingestion \
         path; it said `{}`",
        header.injection_method
    );
    assert!(
        header.outcome.contains("BEHAVIOURAL PROXY"),
        "the outcome must declare itself a proxy rather than a productivity \
         label; it said `{}`",
        header.outcome
    );
    assert!(header.acceptance.contains("ZERO surfaced findings"));
    assert_eq!(
        header.families,
        vec!["NULL", "PLANTED", "SATURATED", "SPARSE"]
    );
    assert_eq!(header.traces, traces.len());
    assert_eq!(
        header.total_episodes,
        traces
            .iter()
            .map(|trace| trace.episodes.len())
            .sum::<usize>()
    );
    // The planted candidate must exist in the registry, or the whole recovery
    // curve is measuring recovery of something that cannot be found.
    assert!(
        candidates::from_id(&header.planted_candidate_id).is_some(),
        "the planted candidate `{}` is not in the closed registry",
        header.planted_candidate_id
    );
}

// ---------------------------------------------------------------------------
// THE NULL TEST — the deliverable
// ---------------------------------------------------------------------------

/// 100 independent noise traces, eight weeks each. **Acceptance: zero surfaced
/// findings across all 100.**
///
/// The traces are not merely i.i.d. noise: the outcome carries a day-level
/// random effect and several candidates are day-clustered by construction, so
/// this is the configuration in which a test that assumes independent episodes
/// manufactures significance. The count is reported, not asserted to be zero
/// and then hidden.
#[test]
fn null_traces_surface_nothing() {
    let (_, traces) = load();
    let nulls = family(&traces, "NULL");
    assert_eq!(nulls.len(), 100, "the acceptance is stated over 100 traces");
    let config = sweep_config(Controls::shipped());

    let mut surfaced = 0usize;
    let mut discovered = 0usize;
    let mut abstained = 0usize;
    let mut abstention_reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut tested_total = 0usize;
    let mut confirmation_failures: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut offenders: Vec<String> = Vec::new();

    for trace in &nulls {
        let analysis = analyse(&trace.decoded(), &config);
        if let Some(reason) = analysis.abstention {
            abstained += 1;
            *abstention_reasons
                .entry(abstention_name(reason))
                .or_default() += 1;
        }
        if let Some(report) = &analysis.discovery {
            tested_total += report.candidates_tested;
        }
        discovered += analysis.findings.len();
        for finding in &analysis.findings {
            if let Some(failure) = finding.confirmation_failure {
                *confirmation_failures
                    .entry(confirmation_failure_name(failure))
                    .or_default() += 1;
            }
        }
        let count = analysis.surfaceable_count();
        surfaced += count;
        if count > 0 {
            offenders.push(format!(
                "{}: {}",
                trace.trace_id,
                analysis
                    .surfaceable()
                    .iter()
                    .map(|finding| finding.candidate_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    println!("\n=== SUITE D, NULL: 100 traces x 8 weeks, all four controls on ===");
    println!(
        "  family size (logged)          {}",
        candidates::FAMILY_SIZE
    );
    println!(
        "  candidates reaching support   {} across 100 traces ({:.1} per trace of {})",
        tested_total,
        tested_total as f64 / 100.0,
        candidates::FAMILY_SIZE
    );
    println!("  traces abstaining             {abstained}/100  {abstention_reasons:?}");
    println!("  discovery-stage discoveries   {discovered}");
    println!("  failed held-out confirmation  {confirmation_failures:?}");
    println!("  SURFACED FINDINGS             {surfaced}");
    for offender in &offenders {
        println!("    {offender}");
    }

    assert_eq!(
        surfaced, 0,
        "the acceptance criterion is ZERO surfaced findings on 100 null \
         traces; {surfaced} were surfaced: {offenders:?}"
    );
}

fn abstention_name(reason: Abstention) -> &'static str {
    match reason {
        Abstention::AwaitingPreDeclaredN { .. } => "awaiting_pre_declared_n",
        Abstention::InsufficientEpisodes { .. } => "insufficient_episodes",
        Abstention::InsufficientDays { .. } => "insufficient_days",
        Abstention::NoHeldOutWindow { .. } => "no_held_out_window",
    }
}

fn confirmation_failure_name(failure: antecedents::ConfirmationFailure) -> &'static str {
    match failure {
        antecedents::ConfirmationFailure::SupportLost { .. } => "support_lost",
        antecedents::ConfirmationFailure::NoPermutationNull { .. } => "no_permutation_null",
        antecedents::ConfirmationFailure::NotSignificantOnHeldOut => "not_significant_on_held_out",
        antecedents::ConfirmationFailure::SignFlipped => "sign_flipped",
        antecedents::ConfirmationFailure::IntervalIncludesZero => "interval_includes_zero",
    }
}

// ---------------------------------------------------------------------------
// THE INVERSION — proving each control is load-bearing
// ---------------------------------------------------------------------------

/// Measure what each multiplicity control is buying, in both directions.
///
/// **If disabling a control does not change the output, it is not wired in**,
/// and every passing run of `null_traces_surface_nothing` proved nothing.
///
/// One ladder is not enough to establish that, and the reason is worth stating
/// because it is the trap this test originally fell into. Removing ONE control
/// from the shipped four leaves three, and on a null family three are usually
/// enough — so a leave-one-out ladder reports several controls as "inert" when
/// what it has actually measured is that they are *jointly redundant on this
/// data*. The measurement that answers "is it wired in" is the other
/// direction: start from the naive analyst and turn each control on ALONE.
///
/// Both ladders are printed. The assertions are on the leave-one-in ladder.
#[test]
fn disabling_the_controls_produces_the_false_discoveries_they_prevent() {
    let (_, traces) = load();
    let nulls = family(&traces, "NULL");
    let decoded: Vec<Vec<Episode>> = nulls.iter().map(|trace| trace.decoded()).collect();

    let leave_one_out: [(&str, Controls); 6] = [
        ("all four on (shipped)", Controls::shipped()),
        (
            "minus held-out replication",
            Controls {
                held_out_replication: false,
                ..Controls::shipped()
            },
        ),
        (
            "minus Benjamini-Hochberg",
            Controls {
                fdr: false,
                ..Controls::shipped()
            },
        ),
        (
            "minus permutation null",
            Controls {
                permutation_null: false,
                ..Controls::shipped()
            },
        ),
        (
            "minus minimum support",
            Controls {
                min_support: false,
                ..Controls::shipped()
            },
        ),
        ("ALL FOUR OFF (naive)", Controls::disabled()),
    ];

    let leave_one_in: [(&str, Controls); 4] = [
        (
            "permutation null only",
            Controls {
                permutation_null: true,
                ..Controls::disabled()
            },
        ),
        (
            "Benjamini-Hochberg only",
            Controls {
                fdr: true,
                ..Controls::disabled()
            },
        ),
        (
            "minimum support only",
            Controls {
                min_support: true,
                ..Controls::disabled()
            },
        ),
        (
            "held-out replication only",
            Controls {
                held_out_replication: true,
                ..Controls::disabled()
            },
        ),
    ];

    fn run(decoded: &[Vec<Episode>], controls: Controls) -> (usize, f64, f64) {
        let config = sweep_config(controls);
        let mut surfaced = 0usize;
        let mut tested = 0usize;
        let mut reports = 0usize;
        for episodes in decoded {
            let analysis = analyse(episodes, &config);
            surfaced += analysis.surfaceable_count();
            if let Some(report) = &analysis.discovery {
                tested += report.candidates_tested;
                reports += 1;
            }
        }
        let mean_tested = if reports == 0 {
            0.0
        } else {
            tested as f64 / reports as f64
        };
        (
            surfaced,
            surfaced as f64 / decoded.len() as f64,
            mean_tested,
        )
    }

    println!("\n=== SUITE D, NULL: what each control is buying ===");
    println!("  100 traces x 8 weeks. Every surfaced finding is a FALSE DISCOVERY.");
    println!("\n  LEAVE-ONE-OUT, from the shipped configuration:");
    println!(
        "  {:<28} {:>9} {:>12} {:>10} {:>12}",
        "controls", "surfaced", "per trace", "tested K", "alpha x K"
    );
    let mut out: BTreeMap<&str, usize> = BTreeMap::new();
    for (label, controls) in leave_one_out {
        let (surfaced, per_trace, mean_tested) = run(&decoded, controls);
        println!(
            "  {label:<28} {surfaced:>9} {per_trace:>12.3} {mean_tested:>10.1} {:>12.2}",
            NAIVE_ALPHA * mean_tested
        );
        out.insert(label, surfaced);
    }

    println!("\n  LEAVE-ONE-IN, from the naive analyst (every other control OFF):");
    println!(
        "  {:<28} {:>9} {:>12} {:>10} {:>12}",
        "controls", "surfaced", "per trace", "tested K", "reduction"
    );
    let naive = out["ALL FOUR OFF (naive)"];
    let mut individually_inert: Vec<&str> = Vec::new();
    for (label, controls) in leave_one_in {
        let (surfaced, per_trace, mean_tested) = run(&decoded, controls);
        println!(
            "  {label:<28} {surfaced:>9} {per_trace:>12.3} {mean_tested:>10.1} {:>11.1}%",
            percent(naive.saturating_sub(surfaced), naive)
        );
        if surfaced >= naive {
            individually_inert.push(label);
        }
    }

    let shipped = out["all four on (shipped)"];
    println!(
        "\n  Shipped: {shipped}. Naive: {naive} ({:.2} per trace against an expected \
         alpha x K).",
        naive as f64 / decoded.len() as f64
    );
    println!(
        "  Controls whose INDIVIDUAL removal from the shipped four changed nothing: {:?}",
        out.iter()
            .filter(|(label, count)| **label != "all four on (shipped)" && **count == shipped)
            .map(|(label, _)| *label)
            .collect::<Vec<_>>()
    );
    println!(
        "  That is joint redundancy on this family, not a disconnected control. \
         The leave-one-in ladder is what settles it."
    );

    assert_eq!(shipped, 0);
    assert!(
        naive > 0,
        "switching every multiplicity control off changed nothing, which means \
         none of them is wired into the decision and the zero above is \
         meaningless"
    );
    assert!(
        individually_inert.is_empty(),
        "these controls did not reduce the false-discovery count even when they \
         were the ONLY control standing, which means they are not wired into \
         the decision at all: {individually_inert:?}"
    );
}

// ---------------------------------------------------------------------------
// THE RECOVERY CURVE
// ---------------------------------------------------------------------------

/// Plant an antecedent at four effect sizes across five history lengths and
/// report, at every cell, the fraction of traces that recover the planted
/// candidate and the fraction that surface something else instead.
///
/// **This curve is the honest answer to "how long until Velvt knows something
/// about me?"** It is reported, never asserted upward: the only assertions are
/// the ones that would catch the curve being silently improved by a change that
/// weakened a control — the RD = 0 column must stay at zero recovery, and the
/// shortest history must not beat the longest.
#[test]
fn the_recovery_curve_over_effect_size_and_history_length() {
    let (_, traces) = load();
    let planted = family(&traces, "PLANTED");
    let config = sweep_config(Controls::shipped());

    // cell -> (recovered, entailed, other, abstained, reached n, total, median episodes)
    let mut cells: BTreeMap<(i64, usize), Vec<(Verdict, usize)>> = BTreeMap::new();
    for trace in &planted {
        let episodes = trace.decoded();
        let analysis = analyse(&episodes, &config);
        let key = (
            (trace.ground_truth.planted_risk_difference * 100.0).round() as i64,
            trace.weeks,
        );
        cells.entry(key).or_default().push((
            verdict(&analysis, &trace.ground_truth, episodes.len()),
            episodes.len(),
        ));
    }

    println!("\n=== SUITE D, PLANTED: recovery curve, all four controls on ===");
    println!("  Baseline risk 0.70. Planted candidate `prevcat_COMMUNICATION`, prevalence 0.35.");
    println!("  `entailed` = a prevtrans_*__COMMUNICATION candidate, which carries the SAME");
    println!("  planted signal by construction: neither a recovery nor a false discovery.");
    println!("  `other` = any other candidate. Mostly false discoveries in this family, but");
    println!("  see the SATURATED test: with a marginals-only family and one real cause, some");
    println!("  other candidates carry genuine marginal associations induced by the plant.");
    println!(
        "\n  {:>6} {:>6} {:>6} {:>10} {:>10} {:>10} {:>10} {:>12}",
        "RD", "weeks", "n", "recovered", "entailed", "other", "abstained", "median eps"
    );
    let mut recovery: BTreeMap<(i64, usize), f64> = BTreeMap::new();
    for ((rd, weeks), outcomes) in &cells {
        let total = outcomes.len();
        let recovered = outcomes.iter().filter(|(v, _)| v.recovered).count();
        let entailed = outcomes.iter().filter(|(v, _)| v.entailed).count();
        let other = outcomes.iter().filter(|(v, _)| v.other > 0).count();
        let abstained = outcomes.iter().filter(|(v, _)| v.abstained).count();
        let mut sizes: Vec<usize> = outcomes.iter().map(|(_, n)| *n).collect();
        sizes.sort_unstable();
        println!(
            "  {:>6.2} {weeks:>6} {total:>6} {recovered:>4}/{total:<5} {entailed:>4}/{total:<5} \
             {other:>4}/{total:<5} {abstained:>4}/{total:<5} {:>12}",
            *rd as f64 / 100.0,
            sizes[sizes.len() / 2]
        );
        recovery.insert((*rd, *weeks), recovered as f64 / total as f64);
    }

    let reached: usize = cells
        .values()
        .flatten()
        .filter(|(v, _)| v.reached_pre_declared_n)
        .count();
    let all: usize = cells.values().map(Vec::len).sum();
    println!(
        "\n  Traces reaching the pre-declared single-look n of {PRE_DECLARED_EPISODE_COUNT} \
         episodes: {reached}/{all} ({:.1}%).",
        percent(reached, all)
    );
    println!(
        "  The sweep above runs with that rule DISABLED, or every short cell would read \
         `abstained` and the curve would measure the rule instead of the procedure."
    );

    // The RD = 0 column is a null in disguise. Recovery there is a false
    // discovery of the planted candidate specifically, and it must be rare.
    let null_column: Vec<f64> = recovery
        .iter()
        .filter(|((rd, _), _)| *rd == 0)
        .map(|(_, fraction)| *fraction)
        .collect();
    let null_recovery = null_column.iter().sum::<f64>() / null_column.len() as f64;
    println!(
        "  RD = 0.00 column (a null in disguise): planted candidate surfaced in \
         {:.1}% of traces.",
        100.0 * null_recovery
    );
    assert!(
        null_recovery <= 0.05,
        "the zero-effect column recovered the planted candidate {:.1}% of the \
         time; at RD = 0 there is nothing to recover, so this is a false \
         discovery rate and it is too high",
        100.0 * null_recovery
    );

    // Power must be monotone in the right direction, or something is wrong with
    // the plumbing rather than with the data.
    let largest = -30i64;
    let shortest = recovery.get(&(largest, 2)).copied().unwrap_or(0.0);
    let longest = recovery.get(&(largest, 12)).copied().unwrap_or(0.0);
    assert!(
        longest >= shortest,
        "twelve weeks recovered the largest planted effect LESS often ({longest:.2}) \
         than two weeks did ({shortest:.2}); that is not a power curve"
    );
}

// ---------------------------------------------------------------------------
// The inversion control for the pipeline as a whole
// ---------------------------------------------------------------------------

/// A huge effect over a long history must produce a surfaced finding.
///
/// Without this, "zero findings on 100 null traces" is unfalsifiable: a harness
/// that never reached the miner would report zero, and so would a miner that
/// can never confirm anything at all. This is suite B's `null-compressed` arm,
/// one layer up.
#[test]
fn a_saturated_effect_over_a_long_history_does_surface() {
    let (_, traces) = load();
    let saturated = family(&traces, "SATURATED");
    assert!(!saturated.is_empty());
    let config = sweep_config(Controls::shipped());

    let mut surfaced_total = 0usize;
    let mut recovered = 0usize;
    let mut other = 0usize;
    let mut rows: Vec<String> = Vec::new();
    for trace in &saturated {
        let episodes = trace.decoded();
        let analysis = analyse(&episodes, &config);
        let outcome = verdict(&analysis, &trace.ground_truth, episodes.len());
        surfaced_total += analysis.surfaceable_count();
        recovered += usize::from(outcome.recovered);
        other += outcome.other;
        let planted_result = analysis.surfaceable().into_iter().find(|finding| {
            Some(finding.candidate_id.as_str())
                == trace.ground_truth.planted_candidate_id.as_deref()
        });
        rows.push(format!(
            "  {:<20} {:>4} eps  planted RD {:>6.2}  realised {:>6.2}  surfaced {:>2}  {}",
            trace.trace_id,
            episodes.len(),
            trace.ground_truth.planted_risk_difference,
            trace
                .ground_truth
                .realised_risk_difference
                .unwrap_or(f64::NAN),
            analysis.surfaceable_count(),
            match planted_result {
                Some(finding) => format!(
                    "planted: discovery RD {:.3} q {:.2e}, held-out RD {:.3} [{:.3}, {:.3}]",
                    finding.discovery.risk_difference,
                    finding.discovery.q_value,
                    finding
                        .confirmation
                        .as_ref()
                        .map(|c| c.risk_difference)
                        .unwrap_or(f64::NAN),
                    finding
                        .confirmation
                        .as_ref()
                        .and_then(|c| c.credible_interval)
                        .map(|i| i.lower)
                        .unwrap_or(f64::NAN),
                    finding
                        .confirmation
                        .as_ref()
                        .and_then(|c| c.credible_interval)
                        .map(|i| i.upper)
                        .unwrap_or(f64::NAN),
                ),
                None => "planted candidate NOT surfaced".to_owned(),
            }
        ));
    }

    println!("\n=== SUITE D, SATURATED (inversion control): 26 weeks, planted RD -0.50 ===");
    for row in &rows {
        println!("{row}");
    }
    println!(
        "  planted candidate surfaced in {recovered}/{} traces; {surfaced_total} findings \
         surfaced in total, {other} of them on some other candidate.",
        saturated.len()
    );
    // Those others are NOT all false discoveries, and the distinction matters.
    // With a marginals-only family and ONE real cause, several other candidates
    // carry GENUINE marginal associations induced by the plant: every category
    // that is not COMMUNICATION inherits the elevated baseline risk, so
    // `prevcat_FOCUS_WORK` is a true association with the opposite sign.
    println!("  Those others are not all false discoveries. With a marginals-only family and");
    println!("  ONE real cause, other candidates carry GENUINE marginal associations induced by");
    println!("  the plant -- every category that is not COMMUNICATION inherits the elevated");
    println!("  baseline risk, so prevcat_FOCUS_WORK is a true association with the opposite");
    println!("  sign. `surfaced something else` is not a synonym for `false discovery`, and a");
    println!("  surface that rendered the top-ranked finding could render a shadow of the real");
    println!("  one instead of the real one.");

    assert!(
        surfaced_total > 0,
        "the inversion control surfaced nothing. Either the harness never \
         reaches the miner or the miner can never confirm anything, and in \
         either case the zero on the NULL family is meaningless."
    );
}

/// Light usage must abstain, with a reason that is distinguishable from
/// "looked and found nothing".
#[test]
fn sparse_traces_abstain_with_a_stated_reason() {
    let (_, traces) = load();
    let sparse = family(&traces, "SPARSE");
    assert!(!sparse.is_empty());
    let config = sweep_config(Controls::shipped());

    let mut reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
    for trace in &sparse {
        let episodes = trace.decoded();
        let analysis = analyse(&episodes, &config);
        let reason = analysis.abstention.unwrap_or_else(|| {
            panic!(
                "{} produced a result on {} episodes",
                trace.trace_id,
                episodes.len()
            )
        });
        *reasons.entry(abstention_name(reason)).or_default() += 1;
        assert_eq!(analysis.surfaceable_count(), 0);
    }
    println!("\n=== SUITE D, SPARSE: {} traces ===", sparse.len());
    println!("  abstention reasons: {reasons:?}");
    assert_eq!(reasons.values().sum::<usize>(), sparse.len());
}

// ---------------------------------------------------------------------------
// What the analysis logs about itself
// ---------------------------------------------------------------------------

/// Every result must carry the family size, and it must be the compile-time
/// constant rather than the number of candidates that happened to have support.
/// A result logged without it cannot be re-checked by anyone.
#[test]
fn every_result_logs_the_compile_time_family_size_not_the_tested_count() {
    let (_, traces) = load();
    let trace = family(&traces, "SATURATED")[0];
    let config = sweep_config(Controls::shipped());
    let analysis = analyse(&trace.decoded(), &config);
    let report = analysis.discovery.as_ref().expect("a 26-week trace mines");

    assert_eq!(analysis.family_size, candidates::FAMILY_SIZE);
    assert_eq!(report.family_size, candidates::FAMILY_SIZE);
    assert_eq!(
        report.registry_version,
        candidates::CANDIDATE_REGISTRY_VERSION
    );
    assert_eq!(report.outcome_id, candidates::OUTCOME_ID);
    assert_eq!(report.horizon_seconds, candidates::HORIZON_SECONDS);
    assert!(
        report.candidates_tested < report.family_size,
        "every one of {} candidates reached the support floor, which would mean \
         the floor is not applied",
        report.family_size
    );
    for finding in &analysis.findings {
        assert_eq!(finding.family_size, candidates::FAMILY_SIZE);
        assert_eq!(
            finding.registry_version,
            candidates::CANDIDATE_REGISTRY_VERSION
        );
    }
}

/// The permutation null's resolution floor, measured on real fixture traces
/// rather than argued in a comment.
///
/// A day-aligned circular shift over `D` days has exactly `D - 1` atoms, so the
/// smallest rank-based p-value is `1/D`. Benjamini-Hochberg's threshold for the
/// top-ranked of 108 hypotheses is `0.10/108 = 9.26e-4`. The two numbers are
/// printed side by side because their relationship is the single most important
/// property of this layer: **a rank-based permutation p-value cannot clear the
/// BH threshold at any history length this product will ever see**, which is
/// why the scale-calibrated p-value exists and why it is the one BH ranks.
#[test]
fn the_permutation_null_resolution_is_reported_against_the_fdr_threshold() {
    let (_, traces) = load();
    let config = sweep_config(Controls::shipped());
    println!("\n=== SUITE D: permutation resolution against the BH threshold ===");
    println!(
        "  {:<12} {:>6} {:>6} {:>10} {:>12} {:>14}",
        "family", "weeks", "days", "shifts", "p floor 1/D", "BH rank-1"
    );
    let mut checked = 0usize;
    for name in ["NULL", "PLANTED", "SATURATED"] {
        let group = family(&traces, name);
        let trace = group
            .iter()
            .max_by_key(|trace| trace.episodes.len())
            .expect("the family is non-empty");
        let analysis = analyse(&trace.decoded(), &config);
        let Some(report) = analysis.discovery else {
            continue;
        };
        println!(
            "  {name:<12} {:>6} {:>6} {:>10} {:>12.4} {:>14.2e}",
            trace.weeks,
            report.days,
            report.permutations_used,
            report.permutation_p_floor,
            report.bh_threshold_for_rank_one
        );
        assert_eq!(
            report.permutations_used, report.distinct_shifts_available,
            "2,000 permutations were requested and {} distinct ones exist; the \
             miner must use all of them and report the shortfall rather than \
             sampling the same {} atoms 2,000 times",
            report.distinct_shifts_available, report.distinct_shifts_available
        );
        assert!(
            report.permutation_p_floor > report.bh_threshold_for_rank_one,
            "the rank-based permutation floor has dropped below the BH \
             threshold; the module docs in antecedents.rs assert the opposite \
             and are now wrong"
        );
        checked += 1;
    }
    assert!(checked >= 2);
    println!(
        "  A rank-based permutation p can never reach the BH threshold at these \
         history lengths."
    );
    println!(
        "  BH therefore ranks the SCALE-CALIBRATED p; the rank p is a separate \
         screen at alpha = {NAIVE_ALPHA}."
    );
}

/// The support floor must be the thing that abstains, and it must abstain on
/// the arm that is short — not on the total.
#[test]
fn the_support_floor_is_enforced_per_arm_on_real_traces() {
    let (_, traces) = load();
    let trace = family(&traces, "PLANTED")[0];
    let config = sweep_config(Controls::shipped());
    let analysis = analyse(&trace.decoded(), &config);
    let Some(report) = analysis.discovery else {
        // Short traces legitimately abstain before mining.
        return;
    };
    for result in &report.results {
        if result.passed_support {
            assert!(result.present.episodes >= MIN_SUPPORT_PER_ARM);
            assert!(result.absent.episodes >= MIN_SUPPORT_PER_ARM);
        }
        // Unobserved episodes are in neither arm. If they were folded into the
        // absent arm the totals would always equal the episode count.
        assert_eq!(
            result.present.episodes + result.absent.episodes + result.unobserved,
            report.episodes
        );
    }
    let with_unobserved = report
        .results
        .iter()
        .filter(|result| result.unobserved > 0)
        .count();
    assert!(
        with_unobserved > 0,
        "no candidate saw an unobserved episode, so the three-valued logic is \
         untested by this fixture"
    );
}

// ---------------------------------------------------------------------------
// The database is where the honesty rule lives
// ---------------------------------------------------------------------------

fn a_finding(id: &str) -> AntecedentFinding {
    AntecedentFinding {
        finding_id: id.to_owned(),
        candidate_id: "prevcat_COMMUNICATION".to_owned(),
        candidate_registry_version: candidates::CANDIDATE_REGISTRY_VERSION,
        discovered_at: 1_800_000_000,
        discovery_window_start: "2026-06-01".to_owned(),
        discovery_window_end: "2026-07-15".to_owned(),
        support_episodes: 96,
        effect_size: -0.31,
        q_value: 0.021,
        confirmed_at: None,
        confirm_support_episodes: None,
        confirm_effect_size: None,
        state: AntecedentFindingState::Candidate,
        surfaced_at: None,
        retracted_at: None,
        retraction_reason: None,
        user_disputed_at: None,
    }
}

/// **The honesty rule as a database constraint.** Surfacing a finding that was
/// never confirmed on held-out data must be an `ABORT`, not a code review
/// comment.
#[test]
fn surfacing_an_unconfirmed_finding_is_aborted_by_the_database() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    repo.record_antecedent_finding(&a_finding("f-unconfirmed"))
        .unwrap();

    let error = repo
        .mark_antecedent_finding_surfaced("f-unconfirmed", 1_800_100_000)
        .expect_err("an unconfirmed finding must not be surfacable");
    let message = error.to_string();
    assert!(
        !message.is_empty(),
        "the abort produced no error the caller could act on"
    );

    // The row is untouched: a failed surface must not leave a half-surfaced
    // finding behind.
    let stored = repo
        .antecedent_finding("f-unconfirmed")
        .unwrap()
        .expect("the finding is still there");
    assert_eq!(stored.state, AntecedentFindingState::Candidate);
    assert_eq!(stored.surfaced_at, None);

    // And the same rule on the INSERT path, which the spec's `BEFORE UPDATE OF
    // state` trigger cannot see at all.
    let mut surfaced_at_birth = a_finding("f-born-surfaced");
    surfaced_at_birth.state = AntecedentFindingState::Surfaced;
    surfaced_at_birth.surfaced_at = Some(1_800_100_000);
    repo.record_antecedent_finding(&surfaced_at_birth)
        .expect_err(
            "a finding INSERTed directly as `surfaced` with no confirmation \
             walked straight past the honesty rule",
        );
    assert!(repo
        .antecedent_finding("f-born-surfaced")
        .unwrap()
        .is_none());
}

/// The confirmed path: discovery on one window, confirmation from a later one,
/// and only then may the state move.
#[test]
fn a_finding_confirmed_on_held_out_data_may_be_surfaced() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    repo.record_antecedent_finding(&a_finding("f-confirmed"))
        .unwrap();

    // 2026-08-15, comfortably after the discovery window ended.
    assert!(repo
        .confirm_antecedent_finding("f-confirmed", 1_786_000_000, 61, -0.27)
        .unwrap());
    let confirmed = repo
        .antecedent_finding("f-confirmed")
        .unwrap()
        .expect("stored");
    assert_eq!(confirmed.state, AntecedentFindingState::Confirmed);
    assert_eq!(confirmed.confirm_support_episodes, Some(61));
    assert_eq!(confirmed.confirm_effect_size, Some(-0.27));

    assert!(repo
        .mark_antecedent_finding_surfaced("f-confirmed", 1_786_100_000)
        .unwrap());
    let surfaced = repo
        .antecedent_finding("f-confirmed")
        .unwrap()
        .expect("stored");
    assert_eq!(surfaced.state, AntecedentFindingState::Surfaced);
    assert_eq!(surfaced.surfaced_at, Some(1_786_100_000));

    // The user disputes it. The dispute is recorded, the finding is retracted,
    // and the row stays so the same candidate is not re-surfaced without new
    // evidence.
    assert!(repo
        .dispute_antecedent_finding("f-confirmed", 1_786_200_000)
        .unwrap());
    let disputed = repo
        .antecedent_finding("f-confirmed")
        .unwrap()
        .expect("stored");
    assert_eq!(disputed.state, AntecedentFindingState::Disputed);
    assert_eq!(disputed.user_disputed_at, Some(1_786_200_000));
    assert_eq!(
        disputed.retraction_reason,
        Some(AntecedentRetractionReason::UserDisputed)
    );
}

/// A confirmation that predates the discovery window it confirms is what
/// "confirmed on the same data" looks like from the outside. Unrepresentable.
#[test]
fn a_confirmation_cannot_predate_the_window_it_confirms() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    repo.record_antecedent_finding(&a_finding("f-time-travel"))
        .unwrap();
    // 2026-06-10, inside the discovery window.
    repo.confirm_antecedent_finding("f-time-travel", 1_780_000_000, 40, -0.30)
        .expect_err("a confirmation from inside the discovery window is not held out");
    let stored = repo
        .antecedent_finding("f-time-travel")
        .unwrap()
        .expect("stored");
    assert_eq!(stored.confirmed_at, None);
    assert_eq!(stored.state, AntecedentFindingState::Candidate);
}

/// Recording the same look twice would double the apparent evidence for a
/// finding while adding none of it.
#[test]
fn the_same_look_cannot_be_recorded_twice() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    repo.record_antecedent_finding(&a_finding("f-first"))
        .unwrap();
    let mut duplicate = a_finding("f-second");
    duplicate.discovered_at += 86_400;
    repo.record_antecedent_finding(&duplicate).expect_err(
        "the same candidate, registry version and discovery window was recorded \
         twice; sequential looking is hard enough without counting one look as two",
    );

    // A different window is a different look and is allowed.
    let mut later = a_finding("f-later");
    later.discovery_window_start = "2026-07-16".to_owned();
    later.discovery_window_end = "2026-08-30".to_owned();
    repo.record_antecedent_finding(&later).unwrap();
    assert_eq!(
        repo.antecedent_findings_in_state(AntecedentFindingState::Candidate)
            .unwrap()
            .len(),
        2
    );
}

/// A finding whose `candidate_id` is not in the closed registry is a finding
/// for a candidate that does not exist. The registry is the only thing that can
/// mint one, and it must be the only thing that can read one back.
#[test]
fn stored_candidate_ids_round_trip_through_the_closed_registry() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    for (index, candidate) in candidates::registry().into_iter().enumerate() {
        let mut finding = a_finding(&format!("f-{index}"));
        finding.candidate_id = candidate.id();
        repo.record_antecedent_finding(&finding).unwrap();
    }
    let stored = repo
        .antecedent_findings_in_state(AntecedentFindingState::Candidate)
        .unwrap();
    assert_eq!(stored.len(), candidates::CANDIDATE_COUNT);
    for finding in stored {
        assert!(
            candidates::from_id(&finding.candidate_id).is_some(),
            "`{}` came out of the database and is not in the registry",
            finding.candidate_id
        );
    }
    assert_eq!(
        repo.clear_antecedent_findings().unwrap() as usize,
        candidates::CANDIDATE_COUNT
    );
}

/// The schema is the privacy guarantee. `antecedent_finding` holds registry
/// keys and numbers; a column that could carry an application name, a label, a
/// window title, a URL, or intention text would make a *claim* more informative
/// than the evidence it was derived from.
#[test]
fn the_findings_table_has_no_column_that_could_identify_an_application() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let schema = database.schema_sql().unwrap();
    // Comments are stripped first. `sqlite_master.sql` stores the CREATE
    // statement verbatim, comments included, and this table's comments discuss
    // copy surfaces and claims at length -- scanning the raw text would match
    // the prose rather than the schema.
    let definition = schema
        .iter()
        .find(|sql| sql.contains("CREATE TABLE antecedent_finding"))
        .expect("0029 applied")
        .lines()
        .map(|line| match line.find("--") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    for forbidden in [
        "stable_id",
        "label",
        "display_name",
        "local_display_label",
        "window_title",
        "url",
        "intention",
        "bundle",
        "app_name",
        "claim",
        "copy",
        "message",
        "body",
    ] {
        assert!(
            !definition.contains(forbidden),
            "antecedent_finding gained a `{forbidden}` column; a finding is a \
             key and two numbers, and the copy layer selects its own template"
        );
    }
    // Odds ratios are not computed, so there is nowhere to put one.
    assert!(!definition.contains("odds"));
}

/// End to end: mine a saturated trace, write what it found through the repo,
/// and confirm the shape survives the round trip. This is the only place the
/// miner and the database meet, and there is no third party — no IPC message,
/// no copy template, no surface.
#[test]
fn a_mined_finding_round_trips_into_the_findings_table() {
    let (_, traces) = load();
    let trace = family(&traces, "SATURATED")[0];
    let config = sweep_config(Controls::shipped());
    let analysis = analyse(&trace.decoded(), &config);
    assert!(
        !analysis.findings.is_empty(),
        "the saturated trace produced no discovery to store"
    );

    let database = SqlitePersistence::open_in_memory().unwrap();
    let repo = database.antecedent_finding_repo();
    let mut written = 0usize;
    for (index, finding) in analysis.findings.iter().enumerate() {
        let record = AntecedentFinding {
            finding_id: format!("{}-{index}", trace.trace_id),
            candidate_id: finding.candidate_id.clone(),
            candidate_registry_version: finding.registry_version,
            discovered_at: 1_786_000_000,
            discovery_window_start: "2026-02-02".to_owned(),
            discovery_window_end: "2026-06-01".to_owned(),
            support_episodes: finding.discovery.support_episodes() as u32,
            effect_size: finding.discovery.risk_difference,
            q_value: finding.discovery.q_value,
            confirmed_at: None,
            confirm_support_episodes: None,
            confirm_effect_size: None,
            state: AntecedentFindingState::Candidate,
            surfaced_at: None,
            retracted_at: None,
            retraction_reason: None,
            user_disputed_at: None,
        };
        repo.record_antecedent_finding(&record).unwrap();
        written += 1;

        if let Some(confirmation) = &finding.confirmation {
            if finding.confirmed {
                repo.confirm_antecedent_finding(
                    &record.finding_id,
                    1_786_500_000,
                    confirmation.support_episodes() as u32,
                    confirmation.risk_difference,
                )
                .unwrap();
            }
        }
    }
    assert_eq!(written, analysis.findings.len());

    let candidates_left = repo
        .antecedent_findings_in_state(AntecedentFindingState::Candidate)
        .unwrap();
    let confirmed = repo
        .antecedent_findings_in_state(AntecedentFindingState::Confirmed)
        .unwrap();
    println!(
        "\n=== SUITE D: {} mined findings written; {} confirmed on held-out data, \
         {} left as unconfirmed candidates ===",
        written,
        confirmed.len(),
        candidates_left.len()
    );
    for finding in &confirmed {
        println!(
            "  {:<28} RD {:>6.3} q {:>8.2e}  held-out RD {:>6.3} on {} episodes",
            finding.candidate_id,
            finding.effect_size,
            finding.q_value,
            finding.confirm_effect_size.unwrap_or(f64::NAN),
            finding.confirm_support_episodes.unwrap_or(0)
        );
    }
    // Nothing is surfaced. Not because a check refused — because nothing
    // asked. There is no surface.
    assert!(repo
        .antecedent_findings_in_state(AntecedentFindingState::Surfaced)
        .unwrap()
        .is_empty());
}

/// The fixture's own ground truth must be internally consistent. A generator
/// that quietly stopped planting anything would be caught here rather than by
/// a recovery curve that reads as a weak procedure.
#[test]
fn the_fixture_ground_truth_is_internally_consistent() {
    let (_, traces) = load();
    let mut planted_families: BTreeSet<&str> = BTreeSet::new();
    for trace in &traces {
        let truth = &trace.ground_truth;
        assert_eq!(truth.episodes, trace.episodes.len());
        let days: BTreeSet<i64> = trace.episodes.iter().map(|row| row[0]).collect();
        assert_eq!(truth.days, days.len());

        // The RD = 0.00 cell of the recovery grid is a NULL in disguise: it is
        // in the PLANTED family, it names the candidate that would have been
        // planted so the table can score it, and `planted` is false because
        // there is nothing there to find. That distinction is the whole point
        // of the cell and it must not be smoothed over here.
        let names_a_candidate = matches!(trace.family.as_str(), "PLANTED" | "SATURATED");
        assert_eq!(
            truth.planted_candidate_id.is_some(),
            names_a_candidate,
            "{} either names a planted candidate it should not, or fails to \
             name one it should",
            trace.trace_id
        );
        assert_eq!(
            truth.entailed_candidate_ids.len(),
            if names_a_candidate { 8 } else { 0 },
            "{} lists the wrong number of entailed candidates",
            trace.trace_id
        );
        assert_eq!(
            truth.planted,
            truth.planted_risk_difference != 0.0,
            "{} claims planted = {} at RD {}",
            trace.trace_id,
            truth.planted,
            truth.planted_risk_difference
        );

        if truth.planted {
            planted_families.insert(trace.family.as_str());
            let risk = truth
                .risk_given_antecedent
                .expect("a planted trace states the risk it planted");
            assert!(
                (risk - 0.70 - truth.planted_risk_difference).abs() < 1e-9,
                "{}: planted RD {} does not equal {risk} - 0.70",
                trace.trace_id,
                truth.planted_risk_difference
            );
            let present = truth.realised_present_episodes.unwrap_or(0);
            let absent = truth.realised_absent_episodes.unwrap_or(0);
            assert_eq!(present + absent, trace.episodes.len());
            assert!(
                present > 0 && absent > 0,
                "{} has an empty arm and cannot carry an association",
                trace.trace_id
            );
            // The realised effect must be in the neighbourhood of the planted
            // one, where "neighbourhood" is four standard errors of the RD
            // rather than a flat number. A flat tolerance would either fail on
            // two-week traces, whose arms hold ten episodes and whose sampling
            // error on RD is ~0.19, or be so wide that it says nothing about a
            // twenty-six-week one. The 0.10 addend covers the day-level random
            // effect, which the binomial standard error does not.
            let realised = truth.realised_risk_difference.expect("stated");
            let standard_error = (0.25 / present as f64 + 0.25 / absent as f64).sqrt();
            let tolerance = 4.0 * standard_error + 0.10;
            assert!(
                (realised - truth.planted_risk_difference).abs() < tolerance,
                "{}: planted RD {} but realised {realised}, which is outside \
                 {tolerance:.3} ({present} present, {absent} absent)",
                trace.trace_id,
                truth.planted_risk_difference
            );
        } else {
            assert_eq!(truth.planted_risk_difference, 0.0);
        }
    }
    assert_eq!(
        planted_families.into_iter().collect::<Vec<_>>(),
        vec!["PLANTED", "SATURATED"]
    );
}

/// What an imperfect segmenter would cost.
///
/// Every other number in this file is measured on **correct** episodes. In the
/// real pipeline an onset comes out of `behavior/hmm.rs`, and a misplaced onset
/// attributes the episode to the wrong preceding run — which is exactly an
/// attenuation of the antecedent. This sweep corrupts a fraction of the
/// preceding-category assignments and reports what happens to recovery.
///
/// It is a **sensitivity analysis, not a model of the HMM.** Nothing here
/// claims to know the segmenter's onset error rate; the sweep says how fast
/// recovery falls as that rate rises, so that when the rate is eventually
/// measured the consequence is already on the record.
#[test]
fn recovery_degrades_as_episode_onsets_are_misattributed() {
    let (_, traces) = load();
    let config = sweep_config(Controls::shipped());

    // The strongest cell of the grid, and the inversion control. A weaker cell
    // is at zero before the corruption starts and would show nothing.
    let strong: Vec<&Trace> = family(&traces, "PLANTED")
        .into_iter()
        .filter(|trace| {
            trace.weeks == 12 && (trace.ground_truth.planted_risk_difference + 0.30).abs() < 1e-9
        })
        .collect();
    let saturated = family(&traces, "SATURATED");
    assert_eq!(strong.len(), 16);

    println!("\n=== SUITE D: recovery against onset misattribution ===");
    println!("  A misplaced onset attributes the episode to the wrong preceding run. This is a");
    println!("  SENSITIVITY ANALYSIS, not a model of behavior/hmm.rs: nothing here measures the");
    println!("  segmenter's real onset error rate.");
    println!(
        "\n  {:>14} {:>22} {:>22}",
        "misattributed", "PLANTED 12wk RD -0.30", "SATURATED 26wk RD -0.50"
    );

    let mut previous: Option<(usize, usize)> = None;
    for rate in [0.0f64, 0.10, 0.20, 0.30, 0.50] {
        let mut counts = [0usize; 2];
        for (slot, group) in [&strong, &saturated].into_iter().enumerate() {
            for (index, trace) in group.iter().enumerate() {
                let mut rng = antecedents::Rng::new(0x00C0_FFEE ^ (index as u64) << 8);
                let mut episodes = trace.decoded();
                for episode in episodes.iter_mut() {
                    if rng.uniform() < rate {
                        let wrong = (rng.next_u64() % candidates::CATEGORY_COUNT as u64) as u8;
                        episode.features.preceding_category = Some(wrong);
                        episode.features.preceding_transition = episode
                            .features
                            .preceding_transition
                            .map(|(first, _)| (first, wrong));
                    }
                }
                let analysis = analyse(&episodes, &config);
                if verdict(&analysis, &trace.ground_truth, episodes.len()).recovered {
                    counts[slot] += 1;
                }
            }
        }
        println!(
            "  {:>13.0}% {:>15}/{:<6} {:>15}/{:<6}",
            100.0 * rate,
            counts[0],
            strong.len(),
            counts[1],
            saturated.len()
        );
        if rate == 0.0 {
            previous = Some((counts[0], counts[1]));
        }
    }

    let (base_strong, base_saturated) = previous.expect("the zero-corruption row ran first");
    assert!(
        base_saturated > 0,
        "the inversion control recovered nothing even with no corruption, so \
         this sweep is measuring a broken harness rather than attenuation"
    );
    println!(
        "  Baseline with no corruption: {base_strong}/{} and {base_saturated}/{}.",
        strong.len(),
        saturated.len()
    );
    println!(
        "  Every OTHER number in this file assumes correct episodes. This row is the only \
         one that does not."
    );
}
