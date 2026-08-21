//! A sticky hidden Markov model over the frozen feature contract, `S = 3`,
//! fitted offline. **Shadow only.**
//!
//! Per `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.2. Nothing in the shipped path calls
//! this, nothing it produces reaches a user, and the three states have no
//! names. They are indices.
//!
//! # The model
//!
//! Emission per state: `Categorical` over the 8 taxonomy categories with a
//! `Dirichlet(0.5)` prior, times a `Normal` on `d_t = ln(1 + min(dwell, 1800))`
//! with a Normal-Inverse-Gamma-shaped shrinkage prior. Transition: `Dirichlet`
//! with a self-transition boost `kappa = 10` — the sticky-HDP-HMM's `kappa`
//! without the nonparametric machinery, which is the part that would need a
//! sampler and therefore a dependency.
//!
//! # Why this is not an HSMM
//!
//! The objection is identifiability, not parameter count. Duration here is an
//! *observation*, not a latent dwell: `close_open_observation` has already
//! run-length-encoded the stream, so `d_t` is data. An HSMM would place a
//! duration distribution over runs that are themselves durations, and one
//! user's few hundred episodes cannot identify it. `03` § 2.2 says this; it is
//! restated here because the file is where someone would try to add one.
//!
//! # MAP, with one deliberate departure
//!
//! The spec says MAP/Baum-Welch. The categorical M-step uses the posterior
//! **mean**, `(count + alpha) / (total + C·alpha)`, not the MAP estimate
//! `(count + alpha - 1) / (total + C·alpha - C)`. At `alpha = 0.5` the MAP
//! estimate subtracts half a pseudo-count from every cell, which is negative
//! wherever the responsibility-weighted count is below 0.5 — a routine
//! occurrence for a rare category under a state that does not emit it. The
//! posterior mean is the standard variational-Bayes M-step, it is proper for
//! every count, and the difference is one half of a pseudo-observation against
//! counts in the hundreds. Recorded because a reader comparing this to a
//! textbook Baum-Welch derivation should find the discrepancy explained rather
//! than have to notice it.
//!
//! # Label switching, and why the fit is canonicalised
//!
//! EM has no preferred labelling of latent states, so two seeds can produce the
//! same model with the indices permuted. Aggregating occupancy across seeds
//! without fixing that produces a number that means nothing. Every fit is
//! therefore permuted into descending `log_dwell_mean` order before it is
//! returned. Dwell is continuous, so ties are effectively impossible and the
//! order is stable; self-transition probabilities can be near-equal across
//! states, which would make sorting on stickiness unstable in exactly the runs
//! where it matters.
//!
//! # The states are not validated and must not be named
//!
//! `03` § 2.2 suggests reading them post-hoc as roughly sustained, alternating
//! and away. That reading is a hypothesis about a model fitted to synthetic
//! data. It is not a finding, no user-facing string may contain it, and
//! `03` § 8 failure mode 1 — the states are the clock, not behaviour — is
//! measured in `tests/behavior_segmentation.rs` rather than assumed against.

// Shadow model, no caller. See the note in `bocpd.rs`.
#![allow(dead_code)]

use std::f64::consts::PI;
use std::fmt;

/// Number of latent states. Three, per the spec — not a tuning parameter.
pub const STATE_COUNT: usize = 3;

/// Size of the closed category vocabulary. Must equal
/// [`super::features::CATEGORIES`]`.len()`; `super::mod`'s tests assert it.
pub const CATEGORY_COUNT: usize = 8;

/// Self-transition boost on the transition Dirichlet.
pub const DEFAULT_STICKINESS_KAPPA: f64 = 10.0;

/// Dirichlet concentration on the categorical emission.
pub const DEFAULT_CATEGORY_ALPHA: f64 = 0.5;

/// Dirichlet concentration on the transition rows, before `kappa`.
pub const DEFAULT_TRANSITION_ALPHA: f64 = 1.0;

/// Free parameters in the model, counted rather than guessed:
/// `S(C-1)` categorical + `2S` Normal + `S(S-1)` transition + `S-1` initial
/// = `21 + 6 + 6 + 2 = 35`.
pub const FREE_PARAMETERS: usize = STATE_COUNT * (CATEGORY_COUNT - 1)
    + 2 * STATE_COUNT
    + STATE_COUNT * (STATE_COUNT - 1)
    + (STATE_COUNT - 1);

/// Minimum runs before the model will fit at all.
///
/// `5 x FREE_PARAMETERS = 175`, rounded up. This is a rule of thumb and is
/// labelled as one — there is no sample-size theory for a sticky HMM on
/// behavioural run sequences and pretending otherwise would be the exact kind
/// of false precision this project bans elsewhere. What it buys is a refusal
/// that is stated in the type system rather than a fit that silently returns
/// three states estimated from forty observations.
pub const MIN_RUNS_TO_FIT: usize = 200;

/// The rule of thumb above, enforced at compile time so that lowering
/// [`MIN_RUNS_TO_FIT`] without arguing about the parameter count fails the
/// build rather than a review.
const _: () = assert!(MIN_RUNS_TO_FIT >= 5 * FREE_PARAMETERS);

/// Why this model refused to produce a fit. Abstention is a first-class result:
/// "I do not have enough history" and "something broke" must never be the same
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HmmAbstention {
    /// Below [`MIN_RUNS_TO_FIT`]. The honest answer at small `n`.
    InsufficientRuns { runs: usize, required: usize },
    /// The two input slices disagree about how many runs there are.
    MismatchedInputs { categories: usize, log_dwell: usize },
    /// A category index outside the closed vocabulary.
    UnknownCategory(usize),
    /// A `d_t` that is not finite.
    NonFiniteDwell,
    /// Every restart produced a non-finite log-likelihood. A defect, not a
    /// small-sample condition, and reported separately for that reason.
    DegenerateLikelihood,
}

impl fmt::Display for HmmAbstention {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientRuns { runs, required } => write!(
                formatter,
                "insufficient history: {runs} closed runs, {required} required"
            ),
            Self::MismatchedInputs {
                categories,
                log_dwell,
            } => write!(
                formatter,
                "{categories} categories against {log_dwell} dwell values"
            ),
            Self::UnknownCategory(index) => write!(
                formatter,
                "category index {index} is outside the closed vocabulary of \
                 {CATEGORY_COUNT}"
            ),
            Self::NonFiniteDwell => write!(formatter, "a log dwell value is not finite"),
            Self::DegenerateLikelihood => {
                write!(formatter, "every restart produced a non-finite likelihood")
            }
        }
    }
}

impl std::error::Error for HmmAbstention {}

/// Fitting configuration, logged with every result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HmmConfig {
    pub max_iterations: usize,
    /// Relative change in log-likelihood below which EM stops.
    pub tolerance: f64,
    /// Random restarts. EM finds a local optimum; one start is one local
    /// optimum with no evidence about the others.
    pub restarts: usize,
    /// Seed for the restart initialiser. A fit that cannot be re-derived from a
    /// seed is not a result.
    pub seed: u64,
    pub stickiness_kappa: f64,
    pub category_alpha: f64,
    pub transition_alpha: f64,
    /// Pseudo-observations of shrinkage on each state's dwell mean, toward the
    /// pooled mean.
    pub dwell_mean_pseudo_counts: f64,
    /// Pseudo-observations of shrinkage on each state's dwell variance, toward
    /// the pooled variance. This is what stops a state collapsing onto a single
    /// point with zero variance and infinite likelihood, which is the standard
    /// unbounded-likelihood failure of Gaussian mixtures.
    pub dwell_variance_pseudo_counts: f64,
    /// Hard floor under any state's dwell variance.
    pub variance_floor: f64,
    pub min_runs: usize,
}

impl Default for HmmConfig {
    fn default() -> Self {
        Self {
            max_iterations: 200,
            tolerance: 1e-7,
            restarts: 8,
            seed: 20_260_821,
            stickiness_kappa: DEFAULT_STICKINESS_KAPPA,
            category_alpha: DEFAULT_CATEGORY_ALPHA,
            transition_alpha: DEFAULT_TRANSITION_ALPHA,
            dwell_mean_pseudo_counts: 1.0,
            dwell_variance_pseudo_counts: 2.0,
            variance_floor: 1e-3,
            min_runs: MIN_RUNS_TO_FIT,
        }
    }
}

/// The fitted parameters. State indices carry no meaning beyond the
/// canonicalisation described at the top of this file.
#[derive(Debug, Clone, PartialEq)]
pub struct StickyHmm {
    pub initial: [f64; STATE_COUNT],
    pub transition: [[f64; STATE_COUNT]; STATE_COUNT],
    pub category: [[f64; CATEGORY_COUNT]; STATE_COUNT],
    pub log_dwell_mean: [f64; STATE_COUNT],
    pub log_dwell_variance: [f64; STATE_COUNT],
}

impl StickyHmm {
    /// `P(state stays)`, per state. The quantity `kappa` is boosting.
    #[must_use]
    pub fn self_transition(&self) -> [f64; STATE_COUNT] {
        let mut out = [0.0; STATE_COUNT];
        for (state, row) in self.transition.iter().enumerate() {
            out[state] = row[state];
        }
        out
    }

    /// The category each state emits most often. A description of the fit, not
    /// a label for it.
    #[must_use]
    pub fn modal_category(&self) -> [usize; STATE_COUNT] {
        let mut out = [0usize; STATE_COUNT];
        for (state, row) in self.category.iter().enumerate() {
            let mut best = 0usize;
            for (index, mass) in row.iter().enumerate() {
                if *mass > row[best] {
                    best = index;
                }
            }
            out[state] = best;
        }
        out
    }

    fn log_emission(&self, state: usize, category: usize, d: f64) -> f64 {
        let variance = self.log_dwell_variance[state];
        let z = d - self.log_dwell_mean[state];
        let normal = -0.5 * (2.0 * PI * variance).ln() - (z * z) / (2.0 * variance);
        self.category[state][category].max(f64::MIN_POSITIVE).ln() + normal
    }

    fn permute(&self, order: &[usize; STATE_COUNT]) -> Self {
        let mut permuted = Self {
            initial: [0.0; STATE_COUNT],
            transition: [[0.0; STATE_COUNT]; STATE_COUNT],
            category: [[0.0; CATEGORY_COUNT]; STATE_COUNT],
            log_dwell_mean: [0.0; STATE_COUNT],
            log_dwell_variance: [0.0; STATE_COUNT],
        };
        for (target, &source) in order.iter().enumerate() {
            permuted.initial[target] = self.initial[source];
            permuted.category[target] = self.category[source];
            permuted.log_dwell_mean[target] = self.log_dwell_mean[source];
            permuted.log_dwell_variance[target] = self.log_dwell_variance[source];
            for (inner_target, &inner_source) in order.iter().enumerate() {
                permuted.transition[target][inner_target] = self.transition[source][inner_source];
            }
        }
        permuted
    }
}

/// A converged fit, with everything a report needs to be re-derived.
#[derive(Debug, Clone, PartialEq)]
pub struct HmmFit {
    pub model: StickyHmm,
    pub log_likelihood: f64,
    pub iterations: usize,
    pub restarts: usize,
    pub converged: bool,
    /// The MAP state path (Viterbi), one entry per run.
    pub states: Vec<usize>,
    /// Fraction of runs assigned to each state by the Viterbi path.
    pub occupancy: [f64; STATE_COUNT],
    /// Number of runs the fit consumed.
    pub runs: usize,
}

/// Fits the model. Offline: this is the nightly job, not an online update.
///
/// `categories` are indices into the closed taxonomy; `log_dwell` is `d_t`,
/// which callers should produce with [`super::bocpd::log_dwell`] rather than
/// re-deriving — two definitions of `d_t` is two feature spaces.
///
/// # Errors
///
/// [`HmmAbstention`], every variant of which is a refusal to answer rather than
/// a failure to compute. `InsufficientRuns` in particular is the expected
/// result for most installs most of the time.
pub fn fit(
    categories: &[usize],
    log_dwell: &[f64],
    config: &HmmConfig,
) -> Result<HmmFit, HmmAbstention> {
    if categories.len() != log_dwell.len() {
        return Err(HmmAbstention::MismatchedInputs {
            categories: categories.len(),
            log_dwell: log_dwell.len(),
        });
    }
    if categories.len() < config.min_runs {
        return Err(HmmAbstention::InsufficientRuns {
            runs: categories.len(),
            required: config.min_runs,
        });
    }
    if let Some(bad) = categories.iter().find(|index| **index >= CATEGORY_COUNT) {
        return Err(HmmAbstention::UnknownCategory(*bad));
    }
    if log_dwell.iter().any(|value| !value.is_finite()) {
        return Err(HmmAbstention::NonFiniteDwell);
    }

    let runs = categories.len();
    let pooled_mean = log_dwell.iter().sum::<f64>() / runs as f64;
    let pooled_variance = (log_dwell
        .iter()
        .map(|value| (value - pooled_mean).powi(2))
        .sum::<f64>()
        / runs as f64)
        .max(config.variance_floor);

    let mut rng = SplitMix64::new(config.seed);
    let mut best: Option<(StickyHmm, f64, usize, bool)> = None;

    for _ in 0..config.restarts.max(1) {
        let mut responsibilities = vec![[0.0f64; STATE_COUNT]; runs];
        for row in &mut responsibilities {
            let mut total = 0.0;
            for slot in row.iter_mut() {
                // Away from the uniform corner, or the first M-step produces
                // three identical states and EM never separates them.
                *slot = 0.1 + rng.next_f64();
                total += *slot;
            }
            for slot in row.iter_mut() {
                *slot /= total;
            }
        }
        let mut model = maximise(
            &responsibilities,
            &[[0.0; STATE_COUNT]; STATE_COUNT],
            categories,
            log_dwell,
            pooled_mean,
            pooled_variance,
            config,
        );

        let mut previous = f64::NEG_INFINITY;
        let mut converged = false;
        let mut iterations = 0usize;
        for iteration in 1..=config.max_iterations {
            iterations = iteration;
            let Some(expectation) = expect(&model, categories, log_dwell) else {
                break;
            };
            model = maximise(
                &expectation.gamma,
                &expectation.xi_sum,
                categories,
                log_dwell,
                pooled_mean,
                pooled_variance,
                config,
            );
            let improvement = expectation.log_likelihood - previous;
            if improvement.abs() <= config.tolerance * (1.0 + expectation.log_likelihood.abs()) {
                previous = expectation.log_likelihood;
                converged = true;
                break;
            }
            previous = expectation.log_likelihood;
        }

        if !previous.is_finite() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(_, score, _, _)| previous > *score)
        {
            best = Some((model, previous, iterations, converged));
        }
    }

    let Some((model, log_likelihood, iterations, converged)) = best else {
        return Err(HmmAbstention::DegenerateLikelihood);
    };

    // Canonical order: descending mean log dwell. See the module note on label
    // switching.
    let mut order = [0usize, 1, 2];
    order.sort_by(|left, right| {
        model.log_dwell_mean[*right]
            .partial_cmp(&model.log_dwell_mean[*left])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let model = model.permute(&order);

    let states = viterbi(&model, categories, log_dwell);
    let mut occupancy = [0.0f64; STATE_COUNT];
    for state in &states {
        occupancy[*state] += 1.0;
    }
    for slot in &mut occupancy {
        *slot /= runs as f64;
    }

    Ok(HmmFit {
        model,
        log_likelihood,
        iterations,
        restarts: config.restarts.max(1),
        converged,
        states,
        occupancy,
        runs,
    })
}

struct Expectation {
    gamma: Vec<[f64; STATE_COUNT]>,
    xi_sum: [[f64; STATE_COUNT]; STATE_COUNT],
    log_likelihood: f64,
}

/// Scaled forward-backward.
///
/// Emissions are computed in log space and rescaled per time step by their row
/// maximum before the forward pass, with the offset accumulated into the
/// log-likelihood. Without that, a state with a tight dwell variance assigns
/// underflowing density to a distant observation, the whole emission row is
/// zero, and the forward scaling factor divides by zero.
fn expect(model: &StickyHmm, categories: &[usize], log_dwell: &[f64]) -> Option<Expectation> {
    let runs = categories.len();
    let mut emission = vec![[0.0f64; STATE_COUNT]; runs];
    let mut offsets = vec![0.0f64; runs];
    for (index, slot) in emission.iter_mut().enumerate() {
        let mut logs = [0.0f64; STATE_COUNT];
        let mut peak = f64::NEG_INFINITY;
        for (state, value) in logs.iter_mut().enumerate() {
            *value = model.log_emission(state, categories[index], log_dwell[index]);
            peak = peak.max(*value);
        }
        if !peak.is_finite() {
            return None;
        }
        offsets[index] = peak;
        for (state, value) in slot.iter_mut().enumerate() {
            *value = (logs[state] - peak).exp();
        }
    }

    let mut alpha = vec![[0.0f64; STATE_COUNT]; runs];
    let mut scale = vec![0.0f64; runs];
    for state in 0..STATE_COUNT {
        alpha[0][state] = model.initial[state] * emission[0][state];
    }
    scale[0] = alpha[0].iter().sum();
    if scale[0] <= 0.0 || !scale[0].is_finite() {
        return None;
    }
    for slot in &mut alpha[0] {
        *slot /= scale[0];
    }
    for index in 1..runs {
        let carried = alpha[index - 1];
        let mut next = [0.0f64; STATE_COUNT];
        for (state, slot) in next.iter_mut().enumerate() {
            let mut total = 0.0;
            for (previous, weight) in carried.iter().enumerate() {
                total += weight * model.transition[previous][state];
            }
            *slot = total * emission[index][state];
        }
        let total: f64 = next.iter().sum();
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        for slot in &mut next {
            *slot /= total;
        }
        alpha[index] = next;
        scale[index] = total;
    }

    let mut beta = vec![[0.0f64; STATE_COUNT]; runs];
    beta[runs - 1] = [1.0; STATE_COUNT];
    for index in (0..runs - 1).rev() {
        let mut current = [0.0f64; STATE_COUNT];
        for (state, slot) in current.iter_mut().enumerate() {
            let mut total = 0.0;
            for next in 0..STATE_COUNT {
                total += model.transition[state][next]
                    * emission[index + 1][next]
                    * beta[index + 1][next];
            }
            *slot = total / scale[index + 1];
        }
        beta[index] = current;
    }

    let mut gamma = vec![[0.0f64; STATE_COUNT]; runs];
    for index in 0..runs {
        let mut total = 0.0;
        for state in 0..STATE_COUNT {
            gamma[index][state] = alpha[index][state] * beta[index][state];
            total += gamma[index][state];
        }
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        for slot in &mut gamma[index] {
            *slot /= total;
        }
    }

    let mut xi_sum = [[0.0f64; STATE_COUNT]; STATE_COUNT];
    for index in 0..runs - 1 {
        for state in 0..STATE_COUNT {
            for next in 0..STATE_COUNT {
                xi_sum[state][next] += alpha[index][state]
                    * model.transition[state][next]
                    * emission[index + 1][next]
                    * beta[index + 1][next]
                    / scale[index + 1];
            }
        }
    }

    let log_likelihood: f64 = scale
        .iter()
        .zip(&offsets)
        .map(|(factor, offset)| factor.ln() + offset)
        .sum();
    if !log_likelihood.is_finite() {
        return None;
    }

    Some(Expectation {
        gamma,
        xi_sum,
        log_likelihood,
    })
}

fn maximise(
    gamma: &[[f64; STATE_COUNT]],
    xi_sum: &[[f64; STATE_COUNT]; STATE_COUNT],
    categories: &[usize],
    log_dwell: &[f64],
    pooled_mean: f64,
    pooled_variance: f64,
    config: &HmmConfig,
) -> StickyHmm {
    let mut occupancy = [0.0f64; STATE_COUNT];
    let mut category_counts = [[0.0f64; CATEGORY_COUNT]; STATE_COUNT];
    let mut dwell_sum = [0.0f64; STATE_COUNT];
    for (index, row) in gamma.iter().enumerate() {
        for (state, weight) in row.iter().enumerate() {
            occupancy[state] += weight;
            category_counts[state][categories[index]] += weight;
            dwell_sum[state] += weight * log_dwell[index];
        }
    }

    let mut model = StickyHmm {
        initial: [0.0; STATE_COUNT],
        transition: [[0.0; STATE_COUNT]; STATE_COUNT],
        category: [[0.0; CATEGORY_COUNT]; STATE_COUNT],
        log_dwell_mean: [0.0; STATE_COUNT],
        log_dwell_variance: [0.0; STATE_COUNT],
    };

    // Initial distribution. With one sequence this is one observation against a
    // uniform pseudo-count, so it is very close to the prior and is not a
    // quantity anything should read.
    let first = gamma[0];
    for (state, slot) in model.initial.iter_mut().enumerate() {
        *slot = (first[state] + 1.0) / (1.0 + STATE_COUNT as f64);
    }

    // Transitions, with the self-transition boost.
    for (state, (destination, observed)) in
        model.transition.iter_mut().zip(xi_sum.iter()).enumerate()
    {
        let mut row = [0.0f64; STATE_COUNT];
        let mut total = 0.0;
        for (next, slot) in row.iter_mut().enumerate() {
            let sticky = if state == next {
                config.stickiness_kappa
            } else {
                0.0
            };
            *slot = observed[next] + config.transition_alpha + sticky;
            total += *slot;
        }
        for (out, value) in destination.iter_mut().zip(row.iter()) {
            *out = value / total;
        }
    }

    // Categorical emission: posterior mean, see the module note.
    for (state, destination) in model.category.iter_mut().enumerate() {
        let denominator = occupancy[state] + CATEGORY_COUNT as f64 * config.category_alpha;
        for (out, observed) in destination.iter_mut().zip(category_counts[state].iter()) {
            *out = (observed + config.category_alpha) / denominator;
        }
    }

    // Normal emission on d_t, shrunk toward the pooled moments. The shrinkage
    // is what bounds the likelihood: without it a state can collapse onto one
    // observation with zero variance and unbounded density.
    for (state, slot) in model.log_dwell_mean.iter_mut().enumerate() {
        *slot = (config.dwell_mean_pseudo_counts * pooled_mean + dwell_sum[state])
            / (config.dwell_mean_pseudo_counts + occupancy[state]);
    }
    let mut scatter = [0.0f64; STATE_COUNT];
    for (index, row) in gamma.iter().enumerate() {
        for (state, w) in row.iter().enumerate() {
            let z = log_dwell[index] - model.log_dwell_mean[state];
            scatter[state] += w * z * z;
        }
    }
    for (state, slot) in model.log_dwell_variance.iter_mut().enumerate() {
        let variance = (config.dwell_variance_pseudo_counts * pooled_variance + scatter[state])
            / (config.dwell_variance_pseudo_counts + occupancy[state]);
        *slot = variance.max(config.variance_floor);
    }

    model
}

/// MAP state path.
#[must_use]
pub fn viterbi(model: &StickyHmm, categories: &[usize], log_dwell: &[f64]) -> Vec<usize> {
    let runs = categories.len();
    if runs == 0 {
        return Vec::new();
    }
    let mut score = vec![[f64::NEG_INFINITY; STATE_COUNT]; runs];
    let mut back = vec![[0usize; STATE_COUNT]; runs];
    for (state, slot) in score[0].iter_mut().enumerate() {
        *slot = model.initial[state].max(f64::MIN_POSITIVE).ln()
            + model.log_emission(state, categories[0], log_dwell[0]);
    }
    for index in 1..runs {
        let carried = score[index - 1];
        let mut current = [f64::NEG_INFINITY; STATE_COUNT];
        let mut pointers = [0usize; STATE_COUNT];
        for (state, slot) in current.iter_mut().enumerate() {
            let mut best = f64::NEG_INFINITY;
            let mut best_previous = 0usize;
            for (previous, carried_score) in carried.iter().enumerate() {
                let candidate = carried_score
                    + model.transition[previous][state]
                        .max(f64::MIN_POSITIVE)
                        .ln();
                if candidate > best {
                    best = candidate;
                    best_previous = previous;
                }
            }
            *slot = best + model.log_emission(state, categories[index], log_dwell[index]);
            pointers[state] = best_previous;
        }
        score[index] = current;
        back[index] = pointers;
    }
    let mut path = vec![0usize; runs];
    let mut best = 0usize;
    for state in 1..STATE_COUNT {
        if score[runs - 1][state] > score[runs - 1][best] {
            best = state;
        }
    }
    path[runs - 1] = best;
    for index in (0..runs - 1).rev() {
        path[index] = back[index + 1][path[index + 1]];
    }
    path
}

/// SplitMix64. Deterministic, seedable, thirty lines, no dependency.
///
/// Used only to place EM restarts. It never touches user data and it is not a
/// randomisation device for any experiment — `03` § 6 requires those to be
/// reproducible from a logged seed, and this is not that seed.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-state sequence built by hand, with the states known. If the
    /// fitter cannot recover a separation this clean, nothing it says about
    /// behaviour means anything.
    /// The unit tests run in two crates: the binary's own test target, and
    /// again inside `tests/behavior_segmentation.rs`, which includes these
    /// modules by path because `behavior` is declared in `main.rs` rather than
    /// `lib.rs`. Keeping the fitting budget small here keeps that duplication
    /// cheap; the validation suite uses the real defaults.
    fn quick() -> HmmConfig {
        HmmConfig {
            restarts: 4,
            max_iterations: 60,
            ..HmmConfig::default()
        }
    }

    /// `d_t`, restated locally rather than imported: these tests compile in two
    /// different module trees and a `super::super` path resolves in only one.
    fn d(seconds: f64) -> f64 {
        (1.0 + seconds.min(1800.0)).ln()
    }

    fn synthetic_three_regime() -> (Vec<usize>, Vec<f64>, Vec<usize>) {
        let mut categories = Vec::new();
        let mut dwell = Vec::new();
        let mut truth = Vec::new();
        let mut rng = SplitMix64::new(7);
        for block in 0..40 {
            let (state, category, base) = match block % 3 {
                0 => (0usize, 0usize, 1200.0f64), // long FOCUS_WORK runs
                1 => (1, 3, 120.0),               // short COMMUNICATION runs
                _ => (2, 1, 400.0),               // medium PASSIVE_CONSUMPTION
            };
            for _ in 0..8 {
                categories.push(category);
                dwell.push(d(base * (0.75 + 0.5 * rng.next_f64())));
                truth.push(state);
            }
        }
        (categories, dwell, truth)
    }

    #[test]
    fn the_free_parameter_count_is_what_the_model_actually_has() {
        // 3x7 categorical + 3x2 Normal + 3x2 transition + 2 initial.
        let counted = STATE_COUNT * (CATEGORY_COUNT - 1)
            + 2 * STATE_COUNT
            + STATE_COUNT * (STATE_COUNT - 1)
            + (STATE_COUNT - 1);
        assert_eq!(FREE_PARAMETERS, counted);
        assert_eq!(counted, 35);
    }

    /// Abstention is a result and it is typed. This is `T9`/`T11` at the model
    /// layer: "not enough history" is a distinct value from "it broke".
    #[test]
    fn short_history_abstains_rather_than_fitting_noise() {
        let categories = vec![0usize; 40];
        let dwell = vec![6.0f64; 40];
        let outcome = fit(&categories, &dwell, &quick());
        assert_eq!(
            outcome,
            Err(HmmAbstention::InsufficientRuns {
                runs: 40,
                required: MIN_RUNS_TO_FIT
            })
        );
        assert_ne!(outcome, Err(HmmAbstention::DegenerateLikelihood));
    }

    #[test]
    fn malformed_input_is_refused_with_a_distinguishable_reason() {
        let config = quick();
        assert_eq!(
            fit(&[0usize; 300], &[6.0f64; 299], &config),
            Err(HmmAbstention::MismatchedInputs {
                categories: 300,
                log_dwell: 299
            })
        );
        let mut categories = vec![0usize; 300];
        categories[17] = CATEGORY_COUNT;
        assert_eq!(
            fit(&categories, &[6.0f64; 300], &config),
            Err(HmmAbstention::UnknownCategory(CATEGORY_COUNT))
        );
        let mut dwell = vec![6.0f64; 300];
        dwell[3] = f64::INFINITY;
        assert_eq!(
            fit(&[0usize; 300], &dwell, &config),
            Err(HmmAbstention::NonFiniteDwell)
        );
    }

    #[test]
    fn it_recovers_three_hand_built_regimes() {
        let (categories, dwell, truth) = synthetic_three_regime();
        let fitted = fit(&categories, &dwell, &quick()).unwrap();
        assert!(
            fitted.converged,
            "EM did not converge in {} iterations",
            fitted.iterations
        );

        // The Viterbi path may label the regimes in any order; what must hold
        // is that the partition matches. Score it as the best assignment of
        // fitted labels to true labels.
        let mut confusion = [[0usize; STATE_COUNT]; STATE_COUNT];
        for (fitted_state, true_state) in fitted.states.iter().zip(&truth) {
            confusion[*true_state][*fitted_state] += 1;
        }
        let mut best = 0usize;
        for permutation in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let hits: usize = (0..STATE_COUNT).map(|s| confusion[s][permutation[s]]).sum();
            best = best.max(hits);
        }
        let accuracy = best as f64 / truth.len() as f64;
        assert!(
            accuracy > 0.95,
            "recovered {accuracy:.3} of a hand-built three-regime partition"
        );
    }

    /// The stickiness prior must actually be load-bearing: raising `kappa`
    /// must raise the self-transition probabilities. If it does not, `kappa` is
    /// a comment rather than a prior.
    #[test]
    fn the_stickiness_prior_is_load_bearing() {
        let (categories, dwell, _) = synthetic_three_regime();
        let loose = HmmConfig {
            stickiness_kappa: 0.0,
            ..quick()
        };
        let sticky = HmmConfig {
            stickiness_kappa: 2000.0,
            ..quick()
        };
        let loose_fit = fit(&categories, &dwell, &loose).unwrap();
        let sticky_fit = fit(&categories, &dwell, &sticky).unwrap();
        let mean = |fit: &HmmFit| fit.model.self_transition().iter().sum::<f64>() / 3.0;
        assert!(
            mean(&sticky_fit) > mean(&loose_fit),
            "kappa did not increase stickiness: {:.4} at kappa=2000 against \
             {:.4} at kappa=0",
            mean(&sticky_fit),
            mean(&loose_fit)
        );
    }

    /// Every fitted distribution must be a distribution.
    #[test]
    fn the_fitted_parameters_are_proper_distributions() {
        let (categories, dwell, _) = synthetic_three_regime();
        let fitted = fit(&categories, &dwell, &quick()).unwrap();
        let close = |value: f64| (value - 1.0).abs() < 1e-9;
        assert!(close(fitted.model.initial.iter().sum::<f64>()));
        for row in &fitted.model.transition {
            assert!(close(row.iter().sum::<f64>()));
            assert!(row.iter().all(|mass| *mass > 0.0));
        }
        for row in &fitted.model.category {
            assert!(close(row.iter().sum::<f64>()));
            assert!(row.iter().all(|mass| *mass > 0.0));
        }
        assert!(fitted
            .model
            .log_dwell_variance
            .iter()
            .all(|variance| *variance >= HmmConfig::default().variance_floor));
        assert!(close(fitted.occupancy.iter().sum::<f64>()));
    }

    /// Canonical ordering removes label switching. Without it, two seeds
    /// produce the same model with permuted indices and any cross-seed
    /// occupancy average is meaningless.
    #[test]
    fn fits_are_canonically_ordered_by_dwell() {
        let (categories, dwell, _) = synthetic_three_regime();
        for seed in [1u64, 2, 3, 99, 20_260_821] {
            let fitted = fit(&categories, &dwell, &HmmConfig { seed, ..quick() }).unwrap();
            let means = fitted.model.log_dwell_mean;
            assert!(
                means[0] >= means[1] && means[1] >= means[2],
                "seed {seed} returned an unordered fit: {means:?}"
            );
        }
    }

    /// Same seed, same answer. A fit that cannot be re-derived is not a result.
    #[test]
    fn the_fit_is_reproducible_from_its_seed() {
        let (categories, dwell, _) = synthetic_three_regime();
        let config = quick();
        let first = fit(&categories, &dwell, &config).unwrap();
        let second = fit(&categories, &dwell, &config).unwrap();
        assert_eq!(first.model, second.model);
        assert_eq!(first.states, second.states);
        assert!((first.log_likelihood - second.log_likelihood).abs() < 1e-12);
    }
}
