//! Antecedent mining: risk differences over a closed candidate family, with
//! four independent multiplicity controls.
//!
//! Per `03-BEHAVIORAL-ENGINE-SPEC.md` § 3. This is the layer the spec calls
//! "the one most likely to ship a lie", and the reason is arithmetic rather
//! than rhetorical: 108 hypotheses tested nightly against a few dozen episodes
//! will produce something that looks like a pattern almost every night. **A
//! confident false pattern is worse than a tracker; it is a confident
//! tracker.**
//!
//! # Everything here is shadow
//!
//! Nothing in this module has a caller in the shipped path. It produces rows
//! for `antecedent_finding` and numbers for tests. There is no IPC message, no
//! copy string, no user-visible artefact, and no path from a result here to the
//! drift gate or the delivery path. Surfacing is Week 8+ and gated on real
//! data; the database's own trigger is the backstop that makes forgetting that
//! an `ABORT` rather than a regression.
//!
//! # The unit of analysis, and what `Y` is not
//!
//! One **episode onset** — a transition into the alternating state, from the
//! nightly segmenter, inside a declared block. `Y_j = 1` iff no confident
//! anchor observation follows within [`candidates::HORIZON_SECONDS`].
//!
//! **`Y` is a behavioural proxy, not a productivity label.** It records that
//! the anchor category was not seen again for ten minutes. It does not record
//! that the ten minutes were wasted, and nothing in this module may be read as
//! saying so. The only real label is the user's own reply, and there is at most
//! one per block.
//!
//! # The association measure is a risk difference, and only a risk difference
//!
//! `RD = P(Y=1 | A) - P(Y=1 | not A)`, with a `Beta(1,1)` posterior on each arm
//! and a 90% credible interval on the difference by Monte Carlo. No odds
//! ratios: RD is the quantity a copy surface could state in a sentence, and a
//! surface that renders a quantity the analysis did not compute is how honest
//! products drift. Computing only what could be said is a constraint, not an
//! omission.
//!
//! # The four controls, and why each is separately load-bearing
//!
//! 1. **Permutation null**, by *circular block shift at day boundaries*. Not
//!    i.i.d. resampling: a person's bad days are bad all day, so `Y` is
//!    autocorrelated within a day, and an i.i.d. permutation would understate
//!    the null variance and turn day-level clustering into significance.
//!    Rotating whole days keeps every within-day sequence intact.
//! 2. **Benjamini-Hochberg** at `q = 0.10`, over [`candidates::FAMILY_SIZE`] —
//!    the compile-time constant, not the number of candidates that happened to
//!    have support. See `candidates`' module docs for why that distinction is
//!    the whole point.
//! 3. **Minimum support**, [`MIN_SUPPORT_PER_ARM`] episodes present *and*
//!    absent. Below that, abstain — not "report with wide error bars".
//! 4. **Held-out replication.** Discovery runs on the earlier window and
//!    confirmation on a later, unseen one. Discovery and confirmation never
//!    share an episode.
//!
//! # The permutation null has a resolution floor, and it is not small
//!
//! A day-aligned circular shift over `D` observed days admits exactly `D - 1`
//! distinct non-identity permutations. Asking for 2,000 does not create more of
//! them; it draws 2,000 times from a null with `D - 1` atoms. So the smallest
//! rank-based p-value the permutation test can ever return is `1 / D`, and at
//! four weeks of weekday history that is `1/20 = 0.05`.
//!
//! Benjamini-Hochberg's threshold for the top-ranked of 108 hypotheses at
//! `q = 0.10` is `0.10 / 108 = 0.000926`. **A rank-based permutation p-value can
//! never reach it at any history length this product will see** — even a year
//! of weekdays gives `1/260 = 0.0038`.
//!
//! That is not a reason to weaken the correction. It is a reason to use the
//! permutation distribution for what it is actually good for: estimating the
//! null's *scale* under the real autocorrelation. So the pipeline computes two
//! p-values and requires both:
//!
//! - `permutation_p` — the rank-based value, floored at `1/D`, used as a
//!   screen at `alpha = 0.05`. It answers "is the observed effect outside the
//!   range that shifting the calendar produces at all".
//! - `calibrated_p` — the observed risk difference studentised against the
//!   permutation null's mean and standard deviation, then referred to a normal
//!   tail. It inherits the permutation null's variance, which is the part that
//!   i.i.d. reasoning gets wrong, and it has the resolution BH needs.
//!
//! Both are reported on every result. Neither is dropped when it is
//! inconvenient.
//!
//! # Sequential looking: the horn this file chooses
//!
//! `03` § 3.4 says to either run the analysis once at a pre-declared `n` or to
//! cost the repeated looks with an alpha-spending function, and to write down
//! which. **This file chooses: once, at a pre-declared `n` of
//! [`PRE_DECLARED_EPISODE_COUNT`] episodes.** [`MinerConfig::default`] enforces
//! it, and `0029`'s unique index makes recording the same window twice an
//! error. Running nightly and surfacing the first thing to cross threshold is
//! an uncontrolled multiple-comparison procedure no matter how good the
//! per-run correction is, and it is the item that is easiest to leave
//! unaccounted for.
//!
//! # No causal language, structurally
//!
//! Nothing here produces a sentence. A `Finding` carries a candidate id, two
//! windows' counts, a risk difference, an interval, and a q-value. The copy
//! layer selects from a closed template set. `03` § 3.6's rules — no causal
//! verbs, no extrapolation, and above all **no recommendation on the basis of
//! an association** — are enforceable only because this module never emits
//! prose to enforce them against.

// The miner ships before its caller: nothing in the shipped path mines
// antecedents, by design (`03` § 5).
#![allow(dead_code)]

use super::candidates::{self, Candidate, EpisodeFeatures, Presence};

/// Minimum episodes with the antecedent present, and minimum absent, before a
/// candidate is testable at all. `03` § 3.4 item 3.
///
/// `03` § 3.5 states the bind this creates and does not pretend to escape it:
/// enforce 12/12 and almost nothing is testable in month one; relax it and
/// multiplicity eats you. There is no tuning that escapes it — it is a property
/// of the data volume.
pub const MIN_SUPPORT_PER_ARM: usize = 12;

/// Benjamini-Hochberg target false discovery rate.
pub const FDR_Q: f64 = 0.10;

/// Permutations requested. The number actually usable is `min(this, D - 1)`
/// where `D` is the number of observed days, and both are reported.
pub const REQUESTED_PERMUTATIONS: usize = 2_000;

/// Screening level for the rank-based permutation p-value.
pub const PERMUTATION_SCREEN_ALPHA: f64 = 0.05;

/// Level for the held-out confirmation test, uncorrected.
pub const CONFIRMATION_ALPHA: f64 = 0.05;

/// Credible mass on the risk-difference interval.
pub const CREDIBLE_MASS: f64 = 0.90;

/// Monte Carlo draws behind each credible interval.
pub const CREDIBLE_INTERVAL_DRAWS: usize = 20_000;

/// The pre-declared `n` at which the single look happens. See the module docs.
///
/// Chosen as `5 x (2 x MIN_SUPPORT_PER_ARM)`: the support floor needs 24
/// episodes in a window for one candidate at prevalence 1.0, and a realistic
/// antecedent is present in a minority of episodes, so a window that can test
/// anything at all needs several times that. It is a floor on when to look, not
/// a promise that looking will find something.
pub const PRE_DECLARED_EPISODE_COUNT: usize = 120;

/// Fewest observed days before a permutation null is worth computing. Below
/// this the null has fewer than nine atoms and the rank p-value cannot go under
/// 0.1, so every result would abstain anyway — abstaining with a stated reason
/// is better than reporting a p-value whose floor exceeds the threshold.
pub const MIN_DAYS_FOR_PERMUTATION: usize = 10;

/// Share of observed days assigned to the discovery window. The remainder is
/// the held-out confirmation window, and the two never share an episode.
pub const DISCOVERY_DAY_FRACTION: f64 = 0.60;

/// One episode onset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Episode {
    /// Local day index. Any monotone integer labelling of local days; the
    /// permutation blocks on it and the window split cuts on it.
    pub day_index: i64,
    /// Onset time in seconds. Used only to order episodes within a day.
    pub onset_at: i64,
    pub features: EpisodeFeatures,
    /// `Y = 1` iff no confident anchor observation within the horizon. A
    /// behavioural proxy, not a productivity label.
    pub outcome: bool,
}

/// Which multiplicity controls are active.
///
/// This exists so that "the correction is load-bearing" is a measurement rather
/// than a claim. `03-BEHAVIORAL-ENGINE-VALIDATION.md`'s inversion discipline:
/// if switching a control off does not change the output, it was never wired
/// in, and the passing test proved nothing.
///
/// [`Controls::shipped`] is the only configuration any real analysis may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub permutation_null: bool,
    pub fdr: bool,
    pub min_support: bool,
    pub held_out_replication: bool,
}

impl Controls {
    /// All four on. The only configuration a real analysis may use.
    pub fn shipped() -> Self {
        Self {
            permutation_null: true,
            fdr: true,
            min_support: true,
            held_out_replication: true,
        }
    }

    /// All four off: the naive analyst. A two-proportion z-test at `alpha =
    /// 0.05` per candidate, no support floor, no correction, no replication.
    /// Used only to measure what the controls are buying.
    pub fn disabled() -> Self {
        Self {
            permutation_null: false,
            fdr: false,
            min_support: false,
            held_out_replication: false,
        }
    }

    pub fn all_on(self) -> bool {
        self == Self::shipped()
    }
}

/// Analysis parameters. [`MinerConfig::default`] is the pre-registered
/// configuration; every field that differs from it must be reported alongside
/// any number derived from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinerConfig {
    pub min_support: usize,
    pub fdr_q: f64,
    pub permutations: usize,
    pub permutation_screen_alpha: f64,
    /// Level for the held-out confirmation test. Uncorrected on purpose: at
    /// confirmation there is exactly **one** pre-specified hypothesis per
    /// discovery, chosen before the held-out window was looked at, so there is
    /// no multiplicity left to correct for.
    pub confirmation_alpha: f64,
    pub naive_alpha: f64,
    pub credible_mass: f64,
    pub credible_interval_draws: usize,
    pub discovery_day_fraction: f64,
    pub min_days_for_permutation: usize,
    /// The pre-declared single-look `n`. `None` disables the rule, which is
    /// legitimate only for a power sweep that is measuring the effect of
    /// history length and must not be blocked by it.
    pub single_look_at_episodes: Option<usize>,
    pub controls: Controls,
    /// The logged family size. Defaults to [`candidates::FAMILY_SIZE`]; it is a
    /// parameter only so a test can prove that shrinking the denominator
    /// changes the answer.
    pub family_size: usize,
    pub seed: u64,
}

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            min_support: MIN_SUPPORT_PER_ARM,
            fdr_q: FDR_Q,
            permutations: REQUESTED_PERMUTATIONS,
            permutation_screen_alpha: PERMUTATION_SCREEN_ALPHA,
            confirmation_alpha: CONFIRMATION_ALPHA,
            naive_alpha: 0.05,
            credible_mass: CREDIBLE_MASS,
            credible_interval_draws: CREDIBLE_INTERVAL_DRAWS,
            discovery_day_fraction: DISCOVERY_DAY_FRACTION,
            min_days_for_permutation: MIN_DAYS_FOR_PERMUTATION,
            single_look_at_episodes: Some(PRE_DECLARED_EPISODE_COUNT),
            controls: Controls::shipped(),
            family_size: candidates::FAMILY_SIZE,
            seed: 0x5645_4C56_5420_4131,
        }
    }
}

/// Why the analysis declined to produce a result. Typed, because "no findings"
/// and "not enough data to look" are different facts and a caller that cannot
/// tell them apart will eventually report one as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abstention {
    /// Fewer episodes than the pre-declared single-look `n`.
    AwaitingPreDeclaredN { episodes: usize, required: usize },
    /// Not enough episodes in the discovery window for any candidate to reach
    /// the support floor.
    InsufficientEpisodes { episodes: usize, required: usize },
    /// Too few observed days for a day-aligned permutation null.
    InsufficientDays { days: usize, required: usize },
    /// No held-out window: every observed day fell in the discovery window.
    NoHeldOutWindow { days: usize },
}

/// Counts for one arm of a contingency table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Arm {
    pub episodes: usize,
    pub events: usize,
}

impl Arm {
    pub fn risk(&self) -> f64 {
        if self.episodes == 0 {
            f64::NAN
        } else {
            self.events as f64 / self.episodes as f64
        }
    }
}

/// A 90% credible interval on the risk difference, by Monte Carlo over
/// `Beta(1,1)` posteriors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CredibleInterval {
    pub lower: f64,
    pub upper: f64,
    pub mass: f64,
    pub draws: usize,
}

impl CredibleInterval {
    pub fn excludes_zero(&self) -> bool {
        (self.lower > 0.0 && self.upper > 0.0) || (self.lower < 0.0 && self.upper < 0.0)
    }
}

/// Everything computed for one candidate in one window.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateResult {
    pub candidate: Candidate,
    pub candidate_id: String,
    pub present: Arm,
    pub absent: Arm,
    /// Episodes for which the antecedent was neither present nor absent, and
    /// which are therefore in neither arm.
    pub unobserved: usize,
    pub risk_difference: f64,
    pub credible_interval: Option<CredibleInterval>,
    /// Rank-based permutation p-value. `None` when the permutation control is
    /// off.
    pub permutation_p: Option<f64>,
    /// The p-value Benjamini-Hochberg ranks: the risk difference studentised
    /// against the permutation null when that control is on, and a naive
    /// pooled two-proportion z-test when it is off.
    pub calibrated_p: f64,
    /// Benjamini-Hochberg q-value over the logged family size.
    pub q_value: f64,
    pub passed_support: bool,
    pub passed_permutation: bool,
    pub passed_fdr: bool,
}

impl CandidateResult {
    /// Survived every enabled discovery-stage control.
    pub fn is_discovery(&self) -> bool {
        self.passed_support && self.passed_permutation && self.passed_fdr
    }

    /// Total episodes contributing to the estimate. This is what
    /// `antecedent_finding.support_episodes` stores.
    pub fn support_episodes(&self) -> usize {
        self.present.episodes + self.absent.episodes
    }
}

/// One window's analysis over the whole family.
#[derive(Debug, Clone, PartialEq)]
pub struct MiningReport {
    pub registry_version: u32,
    /// The compile-time family size, logged with the result per `03` § 3.2.
    pub family_size: usize,
    pub outcome_id: &'static str,
    pub horizon_seconds: i64,
    pub episodes: usize,
    pub days: usize,
    pub first_day: i64,
    pub last_day: i64,
    /// Distinct day-aligned circular shifts that exist. `days - 1`.
    pub distinct_shifts_available: usize,
    /// Shifts actually evaluated.
    pub permutations_used: usize,
    /// `1 / days`: the smallest rank-based permutation p-value that can occur.
    pub permutation_p_floor: f64,
    /// `fdr_q / family_size`: the Benjamini-Hochberg threshold for the
    /// top-ranked hypothesis. Reported next to the floor because the pair is
    /// the whole story about what this layer can and cannot conclude.
    pub bh_threshold_for_rank_one: f64,
    pub candidates_tested: usize,
    pub results: Vec<CandidateResult>,
}

impl MiningReport {
    pub fn discoveries(&self) -> Vec<&CandidateResult> {
        self.results.iter().filter(|r| r.is_discovery()).collect()
    }
}

/// A discovery that has been carried to the held-out window.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub candidate: Candidate,
    pub candidate_id: String,
    pub registry_version: u32,
    pub family_size: usize,
    pub discovery: CandidateResult,
    /// The same candidate re-estimated on the held-out window. `None` when the
    /// held-out control is disabled.
    pub confirmation: Option<CandidateResult>,
    /// Replicated: same sign, support floor met again, and a credible interval
    /// that excludes zero — all on data the discovery never saw.
    pub confirmed: bool,
    /// Why confirmation failed, when it did.
    pub confirmation_failure: Option<ConfirmationFailure>,
}

/// Why a discovery did not replicate. Reported rather than discarded: a
/// discovery that fails to replicate is the system working, and the count of
/// them is evidence about the discovery stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationFailure {
    SupportLost {
        present: usize,
        absent: usize,
    },
    /// The held-out window has too few days to build the same permutation null
    /// the discovery window was tested against. Confirming against a weaker
    /// test than the one that produced the discovery would make replication
    /// easier than discovery, which is backwards.
    NoPermutationNull {
        days: usize,
    },
    /// The effect did not survive the same test on data the discovery never
    /// saw.
    NotSignificantOnHeldOut,
    SignFlipped,
    IntervalIncludesZero,
}

/// The whole analysis: one discovery window, one held-out window, and the
/// findings that survived both.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub registry_version: u32,
    pub family_size: usize,
    pub episodes: usize,
    pub days: usize,
    pub discovery_days: (i64, i64),
    pub confirmation_days: Option<(i64, i64)>,
    pub abstention: Option<Abstention>,
    pub discovery: Option<MiningReport>,
    pub findings: Vec<Finding>,
    pub controls: Controls,
}

impl Analysis {
    /// The findings that a surface would be permitted to render, if a surface
    /// existed. It does not: nothing in the shipped path reads this, and
    /// `antecedent_finding`'s trigger is the backstop.
    ///
    /// This is the count the NULL acceptance criterion is measured on.
    pub fn surfaceable(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| f.confirmed).collect()
    }

    pub fn surfaceable_count(&self) -> usize {
        self.surfaceable().len()
    }
}

// ---------------------------------------------------------------------------
// The analysis
// ---------------------------------------------------------------------------

/// Split the episodes by day, mine the earlier window, and carry every
/// discovery to the later one.
///
/// `episodes` need not be sorted; they are sorted internally by
/// `(day_index, onset_at)` because both the day blocking and the circular shift
/// depend on time order.
pub fn analyse(episodes: &[Episode], config: &MinerConfig) -> Analysis {
    let mut ordered = episodes.to_vec();
    ordered.sort_by_key(|episode| (episode.day_index, episode.onset_at));

    let days = distinct_days(&ordered);
    let day_count = days.len();
    let base = Analysis {
        registry_version: candidates::CANDIDATE_REGISTRY_VERSION,
        family_size: config.family_size,
        episodes: ordered.len(),
        days: day_count,
        discovery_days: (
            days.first().copied().unwrap_or(0),
            days.last().copied().unwrap_or(0),
        ),
        confirmation_days: None,
        abstention: None,
        discovery: None,
        findings: Vec::new(),
        controls: config.controls,
    };

    if let Some(required) = config.single_look_at_episodes {
        if ordered.len() < required {
            return Analysis {
                abstention: Some(Abstention::AwaitingPreDeclaredN {
                    episodes: ordered.len(),
                    required,
                }),
                ..base
            };
        }
    }

    // Split on days, never inside one: a day that straddled both windows would
    // leak the within-day autocorrelation the whole design is built around.
    let (discovery_episodes, confirmation_episodes, discovery_days, confirmation_days) =
        if config.controls.held_out_replication {
            let cut = ((day_count as f64) * config.discovery_day_fraction).floor() as usize;
            let cut = cut.clamp(0, day_count);
            if cut == 0 || cut == day_count {
                return Analysis {
                    abstention: Some(Abstention::NoHeldOutWindow { days: day_count }),
                    ..base
                };
            }
            let boundary = days[cut];
            let discovery: Vec<Episode> = ordered
                .iter()
                .copied()
                .filter(|e| e.day_index < boundary)
                .collect();
            let confirmation: Vec<Episode> = ordered
                .iter()
                .copied()
                .filter(|e| e.day_index >= boundary)
                .collect();
            (
                discovery,
                Some(confirmation),
                (days[0], days[cut - 1]),
                Some((days[cut], days[day_count - 1])),
            )
        } else {
            (
                ordered.clone(),
                None,
                (
                    days.first().copied().unwrap_or(0),
                    days.last().copied().unwrap_or(0),
                ),
                None,
            )
        };

    let base = Analysis {
        discovery_days,
        confirmation_days,
        ..base
    };

    let report = match mine(&discovery_episodes, config) {
        Ok(report) => report,
        Err(abstention) => {
            return Analysis {
                abstention: Some(abstention),
                ..base
            }
        }
    };

    let mut rng = Rng::new(config.seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut findings = Vec::new();
    for discovery in report.results.iter().filter(|r| r.is_discovery()) {
        let (confirmation, confirmed, failure) = match &confirmation_episodes {
            None => (None, true, None),
            Some(held_out) => {
                let mut result = estimate(discovery.candidate, held_out);
                result.passed_support = !config.controls.min_support
                    || (result.present.episodes >= config.min_support
                        && result.absent.episodes >= config.min_support);
                // A single pre-specified hypothesis: nothing to correct for, so
                // the FDR flag is not the thing being reported here.
                result.passed_fdr = true;

                let held_out_days = distinct_days(held_out).len();
                let mut no_null = false;
                if result.passed_support {
                    // The SAME test the discovery stage applied. The credible
                    // interval alone is an i.i.d. Beta-binomial statement, and
                    // i.i.d. reasoning about a day-autocorrelated series is
                    // exactly what the permutation null exists to replace --
                    // confirming with the weaker of the two tests would make
                    // replication easier than discovery.
                    if config.controls.permutation_null {
                        if held_out_days >= config.min_days_for_permutation {
                            let (_, calibrated) = permutation_test(
                                discovery.candidate,
                                held_out,
                                config.permutations,
                                &mut rng,
                            );
                            result.calibrated_p = calibrated;
                            result.passed_permutation = calibrated <= config.confirmation_alpha;
                        } else {
                            no_null = true;
                        }
                    } else {
                        result.calibrated_p =
                            naive_two_proportion_p(&result.present, &result.absent);
                        result.passed_permutation =
                            result.calibrated_p <= config.confirmation_alpha;
                    }
                    result.credible_interval = Some(credible_interval(
                        &result.present,
                        &result.absent,
                        config.credible_mass,
                        config.credible_interval_draws,
                        &mut rng,
                    ));
                }
                let failure = if !result.passed_support {
                    Some(ConfirmationFailure::SupportLost {
                        present: result.present.episodes,
                        absent: result.absent.episodes,
                    })
                } else if no_null {
                    Some(ConfirmationFailure::NoPermutationNull {
                        days: held_out_days,
                    })
                } else if result.risk_difference.signum() != discovery.risk_difference.signum() {
                    Some(ConfirmationFailure::SignFlipped)
                } else if !result.passed_permutation {
                    Some(ConfirmationFailure::NotSignificantOnHeldOut)
                } else if !result
                    .credible_interval
                    .map(|interval| interval.excludes_zero())
                    .unwrap_or(false)
                {
                    Some(ConfirmationFailure::IntervalIncludesZero)
                } else {
                    None
                };
                (Some(result), failure.is_none(), failure)
            }
        };
        findings.push(Finding {
            candidate: discovery.candidate,
            candidate_id: discovery.candidate_id.clone(),
            registry_version: candidates::CANDIDATE_REGISTRY_VERSION,
            family_size: config.family_size,
            discovery: discovery.clone(),
            confirmation,
            confirmed,
            confirmation_failure: failure,
        });
    }

    Analysis {
        discovery: Some(report),
        findings,
        ..base
    }
}

/// One window, the whole family. Returns a typed abstention rather than an
/// empty result when the window cannot support an analysis at all.
pub fn mine(episodes: &[Episode], config: &MinerConfig) -> Result<MiningReport, Abstention> {
    let mut ordered = episodes.to_vec();
    ordered.sort_by_key(|episode| (episode.day_index, episode.onset_at));

    let minimum_episodes = 2 * config.min_support.max(1);
    if ordered.len() < minimum_episodes {
        return Err(Abstention::InsufficientEpisodes {
            episodes: ordered.len(),
            required: minimum_episodes,
        });
    }
    let days = distinct_days(&ordered);
    if config.controls.permutation_null && days.len() < config.min_days_for_permutation {
        return Err(Abstention::InsufficientDays {
            days: days.len(),
            required: config.min_days_for_permutation,
        });
    }

    let outcomes: Vec<bool> = ordered.iter().map(|episode| episode.outcome).collect();
    let day_starts = day_start_indices(&ordered);
    let mut rng = Rng::new(config.seed);
    let shifts = if config.controls.permutation_null {
        choose_shifts(&day_starts, config.permutations, &mut rng)
    } else {
        Vec::new()
    };

    let registry = candidates::registry();
    let mut results: Vec<CandidateResult> = Vec::with_capacity(registry.len());
    for candidate in registry {
        let mut result = estimate(candidate, &ordered);
        result.passed_support = if config.controls.min_support {
            result.present.episodes >= config.min_support
                && result.absent.episodes >= config.min_support
        } else {
            // Both arms must still be non-empty or the risk difference does not
            // exist. That is estimability, not a multiplicity control.
            result.present.episodes > 0 && result.absent.episodes > 0
        };
        if !result.passed_support {
            result.calibrated_p = 1.0;
            results.push(result);
            continue;
        }

        if config.controls.permutation_null {
            let present_index = arm_indices(candidate, &ordered, Presence::Present);
            let absent_index = arm_indices(candidate, &ordered, Presence::Absent);
            let null: Vec<f64> = shifts
                .iter()
                .map(|shift| {
                    shifted_risk_difference(&present_index, &absent_index, &outcomes, *shift)
                })
                .collect();
            let (rank_p, calibrated) = permutation_p_values(result.risk_difference, &null);
            result.permutation_p = Some(rank_p);
            result.calibrated_p = calibrated;
            result.passed_permutation = rank_p <= config.permutation_screen_alpha;
        } else {
            result.calibrated_p = naive_two_proportion_p(&result.present, &result.absent);
            result.permutation_p = None;
            result.passed_permutation = true;
        }
        results.push(result);
    }

    let tested: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, result)| result.passed_support)
        .map(|(index, _)| index)
        .collect();

    apply_fdr(&mut results, &tested, config);

    // The credible interval is an estimate attached to a reported association,
    // not a filter. It is computed for every candidate that reaches the
    // reporting stage; computing 108 intervals a night and discarding 107 is
    // work, not evidence.
    let mut ci_rng = Rng::new(config.seed ^ 0xA076_1D64_78BD_642F);
    for result in results.iter_mut() {
        if result.is_discovery() {
            result.credible_interval = Some(credible_interval(
                &result.present,
                &result.absent,
                config.credible_mass,
                config.credible_interval_draws,
                &mut ci_rng,
            ));
        }
    }

    Ok(MiningReport {
        registry_version: candidates::CANDIDATE_REGISTRY_VERSION,
        family_size: config.family_size,
        outcome_id: candidates::OUTCOME_ID,
        horizon_seconds: candidates::HORIZON_SECONDS,
        episodes: ordered.len(),
        days: days.len(),
        first_day: days[0],
        last_day: days[days.len() - 1],
        distinct_shifts_available: days.len().saturating_sub(1),
        permutations_used: shifts.len(),
        permutation_p_floor: 1.0 / (shifts.len() as f64 + 1.0),
        bh_threshold_for_rank_one: config.fdr_q / config.family_size as f64,
        candidates_tested: tested.len(),
        results,
    })
}

/// The day-aligned circular block-shift test for one candidate on one window.
///
/// Returns `(rank_p, calibrated_p)`. Used by both stages, so the held-out
/// window is judged by the same instrument the discovery window was.
fn permutation_test(
    candidate: Candidate,
    ordered: &[Episode],
    permutations: usize,
    rng: &mut Rng,
) -> (f64, f64) {
    let outcomes: Vec<bool> = ordered.iter().map(|episode| episode.outcome).collect();
    let day_starts = day_start_indices(ordered);
    let shifts = choose_shifts(&day_starts, permutations, rng);
    let present_index = arm_indices(candidate, ordered, Presence::Present);
    let absent_index = arm_indices(candidate, ordered, Presence::Absent);
    if present_index.is_empty() || absent_index.is_empty() {
        return (1.0, 1.0);
    }
    let observed = present_index
        .iter()
        .filter(|index| outcomes[**index as usize])
        .count() as f64
        / present_index.len() as f64
        - absent_index
            .iter()
            .filter(|index| outcomes[**index as usize])
            .count() as f64
            / absent_index.len() as f64;
    let null: Vec<f64> = shifts
        .iter()
        .map(|shift| shifted_risk_difference(&present_index, &absent_index, &outcomes, *shift))
        .collect();
    permutation_p_values(observed, &null)
}

fn distinct_days(ordered: &[Episode]) -> Vec<i64> {
    let mut days = Vec::new();
    for episode in ordered {
        if days.last() != Some(&episode.day_index) {
            days.push(episode.day_index);
        }
    }
    days
}

/// Index into `ordered` of the first episode of each day. These are the only
/// offsets a *block* shift may use: shifting by anything else would cut a day
/// in half and destroy the within-day autocorrelation the null is supposed to
/// preserve.
fn day_start_indices(ordered: &[Episode]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut current: Option<i64> = None;
    for (index, episode) in ordered.iter().enumerate() {
        if current != Some(episode.day_index) {
            starts.push(index);
            current = Some(episode.day_index);
        }
    }
    starts
}

/// The distinct non-identity day-aligned shifts, or a sample of them.
///
/// There are exactly `day_starts.len() - 1` of them. If fewer are available
/// than were requested, **all** are used and the shortfall is reported rather
/// than papered over by sampling with replacement — 2,000 draws from a
/// 19-atom null is still a 19-atom null, and reporting 2,000 would misstate the
/// resolution by two orders of magnitude.
fn choose_shifts(day_starts: &[usize], requested: usize, rng: &mut Rng) -> Vec<usize> {
    let mut available: Vec<usize> = day_starts.iter().skip(1).copied().collect();
    if available.len() <= requested {
        return available;
    }
    // Partial Fisher-Yates: deterministic given the seed.
    for index in 0..requested {
        let pick = index + (rng.next_u64() as usize) % (available.len() - index);
        available.swap(index, pick);
    }
    available.truncate(requested);
    available.sort_unstable();
    available
}

fn arm_indices(candidate: Candidate, ordered: &[Episode], want: Presence) -> Vec<u32> {
    ordered
        .iter()
        .enumerate()
        .filter(|(_, episode)| candidate.presence(&episode.features) == want)
        .map(|(index, _)| index as u32)
        .collect()
}

fn shifted_risk_difference(
    present: &[u32],
    absent: &[u32],
    outcomes: &[bool],
    shift: usize,
) -> f64 {
    let n = outcomes.len();
    let mut present_events = 0usize;
    for index in present {
        if outcomes[(*index as usize + shift) % n] {
            present_events += 1;
        }
    }
    let mut absent_events = 0usize;
    for index in absent {
        if outcomes[(*index as usize + shift) % n] {
            absent_events += 1;
        }
    }
    present_events as f64 / present.len() as f64 - absent_events as f64 / absent.len() as f64
}

/// Point estimate and contingency table for one candidate in one window.
pub fn estimate(candidate: Candidate, ordered: &[Episode]) -> CandidateResult {
    let mut present = Arm::default();
    let mut absent = Arm::default();
    let mut unobserved = 0usize;
    for episode in ordered {
        match candidate.presence(&episode.features) {
            Presence::Present => {
                present.episodes += 1;
                present.events += usize::from(episode.outcome);
            }
            Presence::Absent => {
                absent.episodes += 1;
                absent.events += usize::from(episode.outcome);
            }
            Presence::Unobserved => unobserved += 1,
        }
    }
    let risk_difference = if present.episodes == 0 || absent.episodes == 0 {
        0.0
    } else {
        present.risk() - absent.risk()
    };
    CandidateResult {
        candidate,
        candidate_id: candidate.id(),
        present,
        absent,
        unobserved,
        risk_difference,
        credible_interval: None,
        permutation_p: None,
        calibrated_p: 1.0,
        q_value: 1.0,
        passed_support: false,
        passed_permutation: false,
        passed_fdr: false,
    }
}

/// Rank-based and scale-calibrated p-values from the permutation null.
///
/// Both are two-sided and both centre on the null's own mean rather than on
/// zero: a day-aligned shift over days of unequal length does not guarantee a
/// null centred exactly at zero, and assuming it does would bias every result
/// in whichever direction the imbalance runs.
fn permutation_p_values(observed: f64, null: &[f64]) -> (f64, f64) {
    if null.is_empty() {
        return (1.0, 1.0);
    }
    let m = null.len() as f64;
    let mean = null.iter().sum::<f64>() / m;
    let deviation = (observed - mean).abs();
    let at_least_as_extreme = null
        .iter()
        .filter(|value| (*value - mean).abs() >= deviation - 1e-12)
        .count();
    let rank_p = (1.0 + at_least_as_extreme as f64) / (1.0 + m);

    if null.len() < 3 {
        return (rank_p, rank_p);
    }
    let variance = null
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f64>()
        / (m - 1.0);
    let sd = variance.sqrt();
    if sd <= 0.0 || !sd.is_finite() {
        // Every shift produced the same statistic: the null has no scale, so
        // there is nothing to studentise against and the rank p is the only
        // honest answer.
        return (rank_p, 1.0);
    }
    let z = deviation / sd;
    (rank_p, two_sided_normal_p(z))
}

/// A pooled two-proportion z-test. This is what a naive analyst would reach
/// for, and it is used only when the permutation control is switched off — it
/// assumes independent episodes, which behavioural day-series are not.
fn naive_two_proportion_p(present: &Arm, absent: &Arm) -> f64 {
    if present.episodes == 0 || absent.episodes == 0 {
        return 1.0;
    }
    let n1 = present.episodes as f64;
    let n0 = absent.episodes as f64;
    let pooled = (present.events + absent.events) as f64 / (n1 + n0);
    let variance = pooled * (1.0 - pooled) * (1.0 / n1 + 1.0 / n0);
    if variance <= 0.0 || !variance.is_finite() {
        return 1.0;
    }
    let z = (present.risk() - absent.risk()).abs() / variance.sqrt();
    two_sided_normal_p(z)
}

/// Benjamini-Hochberg over the **logged family size**, not the tested count.
///
/// Untested candidates are treated as nulls that did not reject. That is the
/// conservative reading and it is the one `03` § 3.2 asks for: a denominator
/// that shrinks when the data is sparse is a denominator the data chose.
fn apply_fdr(results: &mut [CandidateResult], tested: &[usize], config: &MinerConfig) {
    if tested.is_empty() {
        return;
    }
    let family = config.family_size.max(tested.len()) as f64;
    let mut ranked: Vec<(usize, f64)> = tested
        .iter()
        .map(|index| (*index, results[*index].calibrated_p))
        .collect();
    ranked.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step-up q-values: q_(i) = min over j >= i of K * p_(j) / j.
    let mut running = 1.0f64;
    for position in (0..ranked.len()).rev() {
        let rank = position as f64 + 1.0;
        let value = (family * ranked[position].1 / rank).min(1.0);
        running = running.min(value);
        results[ranked[position].0].q_value = running;
    }

    if config.controls.fdr {
        // Largest i with p_(i) <= i * q / K, then reject everything up to it.
        let mut largest: Option<usize> = None;
        for (position, (_, p)) in ranked.iter().enumerate() {
            let rank = position as f64 + 1.0;
            if *p <= rank * config.fdr_q / family {
                largest = Some(position);
            }
        }
        if let Some(cut) = largest {
            for (index, _) in ranked.iter().take(cut + 1) {
                results[*index].passed_fdr = true;
            }
        }
    } else {
        for (index, p) in ranked {
            results[index].passed_fdr = p <= config.naive_alpha;
        }
    }
}

/// 90% credible interval on `RD` by Monte Carlo over independent `Beta(1,1)`
/// posteriors.
///
/// `Beta(1,1)` is uniform, so each arm's posterior is `Beta(1 + events, 1 +
/// non-events)`. Both shape parameters are therefore integers of at least one,
/// which is the regime Marsaglia-Tsang handles without a boost step.
pub fn credible_interval(
    present: &Arm,
    absent: &Arm,
    mass: f64,
    draws: usize,
    rng: &mut Rng,
) -> CredibleInterval {
    let mut differences = Vec::with_capacity(draws);
    let a1 = 1.0 + present.events as f64;
    let b1 = 1.0 + (present.episodes - present.events) as f64;
    let a0 = 1.0 + absent.events as f64;
    let b0 = 1.0 + (absent.episodes - absent.events) as f64;
    for _ in 0..draws {
        differences.push(rng.beta(a1, b1) - rng.beta(a0, b0));
    }
    differences.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let tail = (1.0 - mass) / 2.0;
    let lower = quantile(&differences, tail);
    let upper = quantile(&differences, 1.0 - tail);
    CredibleInterval {
        lower,
        upper,
        mass,
        draws,
    }
}

fn quantile(sorted: &[f64], probability: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let position = probability * (sorted.len() as f64 - 1.0);
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    if low == high {
        sorted[low]
    } else {
        let weight = position - low as f64;
        sorted[low] * (1.0 - weight) + sorted[high] * weight
    }
}

/// Two-sided normal tail probability.
///
/// `erfc` by the Numerical Recipes Chebyshev-style rational form, whose
/// *fractional* error is below 1.2e-7 everywhere — fractional rather than
/// absolute is what matters here, because Benjamini-Hochberg's threshold for
/// the top-ranked of 108 hypotheses is 9.3e-4 and an absolute-error
/// approximation would be comparing noise to it.
pub fn two_sided_normal_p(z: f64) -> f64 {
    erfc(z.abs() / std::f64::consts::SQRT_2).clamp(0.0, 1.0)
}

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

// ---------------------------------------------------------------------------
// Seeded sampling, with no new dependencies
// ---------------------------------------------------------------------------

/// xoshiro256** with a splitmix64 seed expansion.
///
/// Hand-rolled for the same reason `bocpd.rs` hand-rolls its log-gamma: adding
/// a crate to draw uniforms would be a supply-chain change in a week that is
/// meant to be protocol-frozen and dependency-frozen. Determinism from the seed
/// is the only property required, and it is tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut value = z;
            value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            value ^ (value >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    /// Uniform on `(0, 1)`. Open at both ends so `ln` is always finite.
    pub fn uniform(&mut self) -> f64 {
        let bits = self.next_u64() >> 11;
        (bits as f64 + 0.5) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Standard normal, Marsaglia polar. One of the two variates is discarded
    /// rather than cached, so a draw sequence depends only on how many draws
    /// were requested and not on the order they were requested in.
    pub fn normal(&mut self) -> f64 {
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                return u * (-2.0 * s.ln() / s).sqrt();
            }
        }
    }

    /// `Gamma(shape, 1)` by Marsaglia-Tsang, for `shape >= 1`. Every shape this
    /// module asks for is `1 + a non-negative count`, so the `shape < 1` boost
    /// step is unreachable and is deliberately absent rather than present and
    /// untested.
    pub fn gamma(&mut self, shape: f64) -> f64 {
        debug_assert!(shape >= 1.0, "gamma shape below 1 is not reachable here");
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = (1.0 + c * x).powi(3);
            if v <= 0.0 {
                continue;
            }
            let u = self.uniform();
            if u < 1.0 - 0.033_1 * x * x * x * x {
                return d * v;
            }
            if u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }

    pub fn beta(&mut self, alpha: f64, beta: f64) -> f64 {
        let x = self.gamma(alpha);
        let y = self.gamma(beta);
        x / (x + y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(hour: u8, preceding: Option<u8>) -> EpisodeFeatures {
        EpisodeFeatures {
            local_hour: hour,
            weekend: false,
            preceding_category: preceding,
            preceding_transition: preceding.map(|c| (c, c)),
            block_elapsed_seconds: 900,
            focus_active: false,
            prior_block_phase: Some(2),
            prior_intervention_outcome: None,
            run_index: 2,
            gap_seconds: Some(120),
        }
    }

    #[test]
    fn the_rng_is_deterministic_from_the_seed() {
        let first: Vec<u64> = (0..8).map(|_| Rng::new(7).next_u64()).collect();
        let mut rng = Rng::new(7);
        let stream: Vec<u64> = (0..8).map(|_| rng.next_u64()).collect();
        assert_eq!(first[0], stream[0]);
        let mut other = Rng::new(8);
        assert_ne!(stream[0], other.next_u64(), "the seed is decorative");
    }

    #[test]
    fn beta_draws_recover_their_mean() {
        let mut rng = Rng::new(11);
        let draws: Vec<f64> = (0..20_000).map(|_| rng.beta(4.0, 8.0)).collect();
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        assert!(
            (mean - 4.0 / 12.0).abs() < 0.005,
            "Beta(4,8) sampled a mean of {mean}, expected 0.3333"
        );
    }

    #[test]
    fn the_normal_tail_is_accurate_where_benjamini_hochberg_reads_it() {
        // Two-sided p at these z, to seven figures.
        for (z, expected) in [
            (1.959_963_985, 0.05),
            (2.575_829_304, 0.01),
            (3.290_526_731, 0.001),
            (3.890_591_886, 0.000_1),
        ] {
            let measured = two_sided_normal_p(z);
            assert!(
                (measured - expected).abs() / expected < 2e-5,
                "two_sided_normal_p({z}) = {measured}, expected {expected}"
            );
        }
    }

    /// The credible interval must contain the truth about as often as it says
    /// it does. Checked by simulation against a known pair of risks.
    #[test]
    fn the_ninety_percent_interval_covers_about_ninety_percent_of_the_time() {
        let mut rng = Rng::new(3);
        let (p1, p0) = (0.40, 0.70);
        let mut covered = 0;
        let trials = 400;
        for _ in 0..trials {
            let mut present = Arm::default();
            let mut absent = Arm::default();
            for _ in 0..60 {
                present.episodes += 1;
                present.events += usize::from(rng.uniform() < p1);
                absent.episodes += 1;
                absent.events += usize::from(rng.uniform() < p0);
            }
            let interval = credible_interval(&present, &absent, 0.90, 4_000, &mut rng);
            if interval.lower <= p1 - p0 && p1 - p0 <= interval.upper {
                covered += 1;
            }
        }
        let coverage = covered as f64 / trials as f64;
        assert!(
            (0.85..=0.96).contains(&coverage),
            "90% interval covered {coverage} of the time over {trials} trials"
        );
    }

    /// A day-aligned shift may only land on a day boundary. If it could land
    /// mid-day the within-day autocorrelation would be cut, which is the exact
    /// failure the circular *block* shift exists to avoid.
    #[test]
    fn permutation_shifts_land_only_on_day_boundaries() {
        let episodes: Vec<Episode> = (0..40)
            .map(|index| Episode {
                day_index: index / 4,
                onset_at: index * 100,
                features: features(9, Some(0)),
                outcome: index % 3 == 0,
            })
            .collect();
        let starts = day_start_indices(&episodes);
        assert_eq!(starts, vec![0, 4, 8, 12, 16, 20, 24, 28, 32, 36]);
        let mut rng = Rng::new(5);
        let shifts = choose_shifts(&starts, 2_000, &mut rng);
        assert_eq!(shifts.len(), starts.len() - 1);
        for shift in shifts {
            assert!(starts.contains(&shift));
            assert_ne!(shift, 0);
        }
    }

    /// Asking for 2,000 permutations does not create 2,000 of them. This test
    /// is the record of the resolution floor stated in the module docs.
    #[test]
    fn the_permutation_null_has_only_as_many_atoms_as_there_are_days() {
        let episodes: Vec<Episode> = (0..120)
            .map(|index| Episode {
                day_index: index / 6,
                onset_at: index * 100,
                features: features(9, Some((index % 8) as u8)),
                outcome: index % 2 == 0,
            })
            .collect();
        let config = MinerConfig {
            single_look_at_episodes: None,
            ..MinerConfig::default()
        };
        let report = mine(&episodes, &config).expect("20 days is enough to mine");
        assert_eq!(report.days, 20);
        assert_eq!(report.distinct_shifts_available, 19);
        assert_eq!(
            report.permutations_used, 19,
            "2,000 were requested and 19 exist"
        );
        assert!((report.permutation_p_floor - 1.0 / 20.0).abs() < 1e-12);
        assert!(
            report.permutation_p_floor > report.bh_threshold_for_rank_one,
            "the rank-based floor {} is supposed to sit ABOVE the BH threshold \
             {}; if it no longer does, the module docs are wrong",
            report.permutation_p_floor,
            report.bh_threshold_for_rank_one
        );
    }

    #[test]
    fn a_window_with_too_few_episodes_abstains_with_a_reason() {
        let episodes: Vec<Episode> = (0..10)
            .map(|index| Episode {
                day_index: index,
                onset_at: 0,
                features: features(9, Some(0)),
                outcome: false,
            })
            .collect();
        let config = MinerConfig {
            single_look_at_episodes: None,
            ..MinerConfig::default()
        };
        assert_eq!(
            mine(&episodes, &config),
            Err(Abstention::InsufficientEpisodes {
                episodes: 10,
                required: 24
            })
        );
    }

    #[test]
    fn the_single_look_rule_abstains_before_the_pre_declared_n() {
        let episodes: Vec<Episode> = (0..60)
            .map(|index| Episode {
                day_index: index / 3,
                onset_at: index,
                features: features(9, Some(0)),
                outcome: index % 2 == 0,
            })
            .collect();
        let analysis = analyse(&episodes, &MinerConfig::default());
        assert_eq!(
            analysis.abstention,
            Some(Abstention::AwaitingPreDeclaredN {
                episodes: 60,
                required: PRE_DECLARED_EPISODE_COUNT
            })
        );
        assert_eq!(analysis.surfaceable_count(), 0);
    }

    /// Discovery and confirmation must never share an episode. Checked by
    /// counting, because a fencepost error here would silently make the
    /// held-out control a no-op while every test still passed.
    #[test]
    fn the_two_windows_partition_the_episodes_and_never_overlap() {
        let episodes: Vec<Episode> = (0..200)
            .map(|index| Episode {
                day_index: index / 5,
                onset_at: index,
                features: features(9, Some((index % 8) as u8)),
                outcome: index % 3 == 0,
            })
            .collect();
        let analysis = analyse(&episodes, &MinerConfig::default());
        let (discovery_first, discovery_last) = analysis.discovery_days;
        let (confirm_first, confirm_last) = analysis
            .confirmation_days
            .expect("40 days leaves a held-out window");
        assert!(
            discovery_last < confirm_first,
            "windows overlap: discovery ends {discovery_last}, confirmation starts {confirm_first}"
        );
        assert_eq!(discovery_first, 0);
        assert_eq!(confirm_last, 39);
        let report = analysis.discovery.expect("the discovery window mined");
        assert_eq!(report.days, 24);
        assert_eq!(report.episodes, 120);
    }

    /// A candidate with the antecedent on one side of the support floor and
    /// off the other must abstain, and it must stay in the denominator.
    #[test]
    fn the_support_floor_abstains_rather_than_reporting_a_wide_interval() {
        let mut episodes: Vec<Episode> = Vec::new();
        for index in 0..200i64 {
            // Hour 3 appears eleven times: one short of the floor.
            let hour = if index < 11 { 3 } else { 9 };
            episodes.push(Episode {
                day_index: index / 5,
                onset_at: index,
                features: features(hour, Some(0)),
                outcome: index % 2 == 0,
            });
        }
        let config = MinerConfig {
            single_look_at_episodes: None,
            ..MinerConfig::default()
        };
        let report = mine(&episodes, &config).expect("mined");
        let bin_one = report
            .results
            .iter()
            .find(|result| result.candidate_id == "tod_03_06")
            .expect("the 03-06 bin is registered");
        assert_eq!(bin_one.present.episodes, 11);
        assert!(!bin_one.passed_support);
        assert!(!bin_one.is_discovery());
        // Still in the family: the denominator does not shrink.
        assert_eq!(report.family_size, candidates::FAMILY_SIZE);
        assert!(report.candidates_tested < report.family_size);
    }

    /// Benjamini-Hochberg over the logged family, not the tested count. If the
    /// denominator were the tested count, this p-value would pass.
    #[test]
    fn the_fdr_denominator_is_the_logged_family_size() {
        let mut results = vec![
            CandidateResult {
                calibrated_p: 0.004,
                passed_support: true,
                ..estimate(Candidate::TimeOfDay(0), &[])
            },
            CandidateResult {
                calibrated_p: 0.60,
                passed_support: true,
                ..estimate(Candidate::TimeOfDay(1), &[])
            },
        ];
        let tested = vec![0usize, 1usize];

        let wide = MinerConfig::default();
        apply_fdr(&mut results, &tested, &wide);
        assert!(
            !results[0].passed_fdr,
            "p = 0.004 must not clear 1 * 0.10 / 108 = 0.000926"
        );

        // The same p-value against a family of two, which is what a denominator
        // chosen by the data would look like.
        let mut narrow_results = results.clone();
        for result in narrow_results.iter_mut() {
            result.passed_fdr = false;
        }
        let narrow = MinerConfig {
            family_size: 2,
            ..MinerConfig::default()
        };
        apply_fdr(&mut narrow_results, &tested, &narrow);
        assert!(
            narrow_results[0].passed_fdr,
            "the denominator is not wired into the decision at all"
        );
    }

    /// The permutation null must be sensitive to day-level clustering. Build
    /// outcomes that are constant within a day and independent of the
    /// antecedent, and the naive test and the permutation test must disagree.
    #[test]
    fn the_permutation_null_prices_day_clustering_that_the_naive_test_ignores() {
        let mut rng = Rng::new(19);
        let mut episodes = Vec::new();
        for day in 0..40i64 {
            // The whole day is good or bad. Nothing to do with the antecedent.
            let bad_day = rng.uniform() < 0.5;
            for slot in 0..6 {
                episodes.push(Episode {
                    day_index: day,
                    onset_at: slot,
                    // The antecedent is also a day-level property, so under
                    // i.i.d. reasoning the two look strongly associated.
                    features: features(if day % 2 == 0 { 9 } else { 15 }, Some(0)),
                    outcome: bad_day,
                });
            }
        }
        let config = MinerConfig {
            single_look_at_episodes: None,
            ..MinerConfig::default()
        };
        let report = mine(&episodes, &config).expect("mined");
        let clustered = report
            .results
            .iter()
            .find(|result| result.candidate_id == "tod_09_12")
            .expect("registered");
        let naive = naive_two_proportion_p(&clustered.present, &clustered.absent);
        assert!(
            clustered.calibrated_p > naive,
            "the permutation-calibrated p ({}) must exceed the naive p ({naive}) \
             when the outcome is clustered by day; if it does not, the null is \
             not preserving the autocorrelation",
            clustered.calibrated_p
        );
    }
}
