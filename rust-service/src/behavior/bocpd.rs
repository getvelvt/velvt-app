//! Bayesian online change-point detection (Adams & MacKay 2007) over the frozen
//! feature contract. **Shadow only.**
//!
//! Per `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.1. This module computes a number and
//! nothing else. It has no handle on the drift gate, no handle on delivery, and
//! nothing in the shipped path calls it. Every output is a candidate for a log
//! line and for nothing else — the quantity that *should eventually* stand
//! behind the drift gate is not the quantity that stands behind it today, and
//! the two must not be confused while the second is unvalidated.
//!
//! # The model
//!
//! One observation per closed run: `(c_t, d_t)` where `c_t` is the category
//! index into the closed 8-category taxonomy and `d_t = ln(1 + min(dwell,
//! 1800))`. The emission is the conjugate product
//!
//! ```text
//!   Categorical(c_t | pi) · Dirichlet(pi | alpha = 0.5)
//!     x  Normal(d_t | mu, 1/tau) · NormalGamma(mu, tau | mu0, kappa0, a0, b0)
//! ```
//!
//! Both halves are conjugate, so the posterior predictive of a segment is
//! closed-form — a Dirichlet-multinomial term times a Student-t term — and the
//! run-length recursion needs no sampling, no optimiser, and **no dependency**.
//! If this file ever needs a crate, the model is wrong, not the manifest.
//!
//! # Independence, stated rather than assumed away
//!
//! The emission treats category and dwell as independent *given the segment*.
//! They are not independent in the data: a `COMMUNICATION` run is short and a
//! `FOCUS_WORK` run is long, which is precisely why they carry different dwell
//! medians in the trace generator. Inside a segment whose category mixture is
//! stable this costs sharpness, not correctness — the marginal is still
//! time-homogeneous — but it does mean a segment containing several categories
//! learns a *mixture* of log-normals through a single Student-t, and the
//! resulting fat residuals are one of the two sources of false alarm measured
//! in `tests/behavior_segmentation.rs`. Recording it here so the measured
//! false-alarm rate is read as a property of a stated model, not a surprise.
//!
//! # What the output is, and is not
//!
//! [`BocpdUpdate::p_recent_change`] is `P(run_length < 3 | x_{1:t})` — the
//! posterior mass on "the current segment started within the last three runs".
//! It is a statement about a **segmentation of observed categories and
//! durations**. It is not a statement about attention, intent, distraction, or
//! anything on the not-identifiable list in [`super::features`].
//!
//! # `P(r_t = 0)` is exactly the hazard, always. Read the threshold against it.
//!
//! This is a property of Adams & MacKay under a constant hazard, it is not
//! obvious, and it decides what any threshold on the output can mean. Derived
//! here rather than left for someone to rediscover after shipping a threshold.
//!
//! Both branches of the recursion use the *same* predictive `pi_r(x_t)`,
//! because in A&M's indexing `r_t = 0` means the change point falls after
//! `x_t` — so `x_t` still belongs to the old segment under both hypotheses.
//! Writing `S = sum_r P(r_{t-1} = r, x_{1:t-1}) pi_r(x_t)`:
//!
//! ```text
//!   P(r_t = 0,   x_{1:t}) = H·S
//!   P(r_t = r+1, x_{1:t}) = (1-H)·joint_r,  which sum to (1-H)·S
//!   evidence              = H·S + (1-H)·S = S
//!   P(r_t = 0 | x_{1:t})  = H·S / S = H, for every t, on every data set
//! ```
//!
//! Nothing observed so far is informative about whether a change falls *after*
//! the most recent point, so the posterior returns the prior. That is correct,
//! and it puts a hard floor of `H = 1/6 = 0.167` under `P(run_length < 3)`.
//!
//! Two consequences, both load-bearing:
//!
//! 1. **A threshold at or below `H` fires on every observation forever.** The
//!    informative content lives entirely in `P(r=1) + P(r=2)` — the mass on
//!    "the change fell in one of the last two gaps". See
//!    [`BocpdConfig::recent_change_floor`].
//! 2. **The signal is a transient exactly `horizon` observations wide.** Three
//!    runs after a change the true run length is 3, `P(run_length < 3)` falls
//!    back toward the floor, and a scorer that samples the stream at one fixed
//!    offset reads a working detector as a broken one. Detection has to be
//!    scored as "did it cross inside a window", which is what the validation
//!    suite does.

// Nothing in the shipped path calls this yet, by design — it ships in shadow
// (§ 5), and a shadow model with no caller is the correct amount of coupling
// for a model that must not be able to change what a user sees. A module-level
// allow keeps the file free of per-item attributes; a binary crate has no
// notion of a symbol that is public for someone else to use.
#![allow(dead_code)]

use std::f64::consts::PI;
use std::fmt;

/// Size of the closed category vocabulary. Must equal
/// [`super::features::CATEGORIES`]`.len()` — a Dirichlet over the wrong number
/// of categories is wrong silently. `super::mod`'s tests assert the equality.
pub const CATEGORY_COUNT: usize = 8;

/// The dwell clip, in seconds. Swift's number, restated: see
/// [`super::features::DWELL_CLIP_SECONDS`].
pub const DWELL_CLIP_SECONDS: f64 = 1800.0;

/// Hazard rate as a mean run length, in runs. `H = 1 / LAMBDA_RUNS`.
///
/// `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.1 starts here and says personalise only
/// after § 4 — that is, only after there is a per-user posterior to personalise
/// *from*. Six runs is roughly the twenty-minute figure in the spec at the
/// assumed dwell medians; it is an assumption, not a measurement.
pub const DEFAULT_LAMBDA_RUNS: f64 = 6.0;

/// Truncation of the run-length posterior. Cost is `O(R_MAX)` per observation.
pub const DEFAULT_R_MAX: usize = 64;

/// Dirichlet concentration on the 8-category emission. `alpha < 1` is
/// deliberately sparsity-favouring: a segment is expected to be dominated by
/// one or two categories, not to spread over all eight.
pub const DEFAULT_DIRICHLET_ALPHA: f64 = 0.5;

/// The horizon in `P(run_length < H)`. Three, per the spec.
pub const DEFAULT_RECENT_RUN_HORIZON: usize = 3;

/// Observations consumed before [`BocpdUpdate::warm`] is set.
///
/// Not a tuning knob — an arithmetic fact. At `t = 1` the run length can only
/// be 0 or 1, so `P(run_length < 3) = 1` regardless of the data, and the same
/// degeneracy shrinks but persists for the first few observations. Anything
/// that scores this detector must drop the cold prefix or it is scoring the
/// support of the posterior rather than the posterior.
pub const WARMUP_RUNS: usize = 8;

/// `d_t = ln(1 + min(dwell_seconds, 1800))`, the one definition.
///
/// The HMM in [`super::hmm`] consumes the same quantity and deliberately does
/// not re-derive it: two definitions of `d_t` is two feature spaces.
#[must_use]
pub fn log_dwell(dwell_seconds: f64) -> f64 {
    (1.0 + dwell_seconds.clamp(0.0, DWELL_CLIP_SECONDS)).ln()
}

/// One closed run, as the detector sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunObservation {
    /// Index into the closed taxonomy, `0..CATEGORY_COUNT`.
    pub category: usize,
    /// Raw dwell in seconds. Clipped and log-transformed by [`log_dwell`].
    pub dwell_seconds: f64,
}

impl RunObservation {
    #[must_use]
    pub fn new(category: usize, dwell_seconds: f64) -> Self {
        Self {
            category,
            dwell_seconds,
        }
    }
}

/// The refusals. A detector that silently coerces a bad input is a detector
/// whose logged output cannot be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BocpdError {
    /// A category index outside the closed vocabulary. Almost always a
    /// taxonomy that grew without this constant growing with it.
    UnknownCategory(usize),
    /// A dwell that is not a finite, non-negative number of seconds.
    NonFiniteDwell,
}

impl fmt::Display for BocpdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCategory(index) => write!(
                formatter,
                "category index {index} is outside the closed vocabulary of \
                 {CATEGORY_COUNT}"
            ),
            Self::NonFiniteDwell => write!(formatter, "dwell is not a finite, non-negative number"),
        }
    }
}

impl std::error::Error for BocpdError {}

/// Normal-Gamma prior on `(mu, tau)` for `d_t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalGammaPrior {
    /// Prior mean of `d_t`.
    pub mu: f64,
    /// Prior strength on the mean, in pseudo-observations.
    pub kappa: f64,
    /// Gamma shape.
    pub alpha: f64,
    /// Gamma rate.
    pub beta: f64,
}

impl Default for NormalGammaPrior {
    /// Weak and centred on a five-minute run.
    ///
    /// `mu = ln(1 + 300) = 5.71` sits between the assumed `COMMUNICATION` and
    /// `FOCUS_WORK` medians. `kappa = 1` is one pseudo-observation, so the
    /// second real run already outweighs it. `alpha = beta = 1` puts the prior
    /// residual standard deviation near 1 in log space — a factor of `e` in
    /// seconds, which is the right order for a quantity whose plausible range
    /// spans a minute to half an hour.
    ///
    /// Every one of those four numbers is an assumption. None is measured.
    fn default() -> Self {
        Self {
            mu: (1.0 + 300.0f64).ln(),
            kappa: 1.0,
            alpha: 1.0,
            beta: 1.0,
        }
    }
}

/// The detector's configuration, logged with every result. A comparison across
/// configurations is a comparison of two different detectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BocpdConfig {
    pub lambda_runs: f64,
    pub r_max: usize,
    pub dirichlet_alpha: f64,
    pub dwell_prior: NormalGammaPrior,
    pub recent_run_horizon: usize,
}

impl Default for BocpdConfig {
    fn default() -> Self {
        Self {
            lambda_runs: DEFAULT_LAMBDA_RUNS,
            r_max: DEFAULT_R_MAX,
            dirichlet_alpha: DEFAULT_DIRICHLET_ALPHA,
            dwell_prior: NormalGammaPrior::default(),
            recent_run_horizon: DEFAULT_RECENT_RUN_HORIZON,
        }
    }
}

impl BocpdConfig {
    /// `H = 1 / lambda`, the constant hazard.
    #[must_use]
    pub fn hazard(&self) -> f64 {
        1.0 / self.lambda_runs
    }

    /// The hard floor under [`BocpdUpdate::p_recent_change`], which is exactly
    /// the hazard. Derived in the module documentation.
    ///
    /// A threshold at or below this value fires on every observation of every
    /// stream, including a perfectly stationary one. At the defaults the floor
    /// is `1/6 = 0.167`.
    #[must_use]
    pub fn recent_change_floor(&self) -> f64 {
        self.hazard()
    }
}

/// Sufficient statistics for one run-length hypothesis. Conjugacy is the whole
/// reason this is five numbers and not a history buffer.
#[derive(Debug, Clone, Copy)]
struct SegmentStats {
    counts: [f64; CATEGORY_COUNT],
    category_total: f64,
    dwell_count: f64,
    dwell_mean: f64,
    /// Welford's `M2`: the sum of squared deviations from the running mean.
    dwell_m2: f64,
}

impl SegmentStats {
    const EMPTY: Self = Self {
        counts: [0.0; CATEGORY_COUNT],
        category_total: 0.0,
        dwell_count: 0.0,
        dwell_mean: 0.0,
        dwell_m2: 0.0,
    };

    fn absorb(mut self, category: usize, log_dwell_value: f64) -> Self {
        self.counts[category] += 1.0;
        self.category_total += 1.0;
        self.dwell_count += 1.0;
        let delta = log_dwell_value - self.dwell_mean;
        self.dwell_mean += delta / self.dwell_count;
        self.dwell_m2 += delta * (log_dwell_value - self.dwell_mean);
        self
    }
}

/// One step of the detector. This is the shape of the shadow log line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BocpdUpdate {
    /// One-based count of observations consumed, including this one.
    pub index: usize,
    /// `P(run_length < recent_run_horizon | x_{1:t})`.
    pub p_recent_change: f64,
    /// The run length with the most posterior mass.
    pub map_run_length: usize,
    /// Posterior mean run length, in runs.
    pub expected_run_length: f64,
    /// False while the posterior's *support* still forces the answer. See
    /// [`WARMUP_RUNS`]. A scorer that ignores this is scoring arithmetic.
    pub warm: bool,
}

/// Adams & MacKay's run-length recursion, in log space.
///
/// Log space rather than normalise-every-step: the two are equivalent until a
/// long segment makes every predictive tiny at once, at which point the
/// normalising version divides zero by zero and this one does not.
#[derive(Debug, Clone)]
pub struct Bocpd {
    config: BocpdConfig,
    log_hazard: f64,
    log_survive: f64,
    /// Normalised log posterior over run length; index `r` is `P(r_t = r)`.
    log_posterior: Vec<f64>,
    /// Sufficient statistics, parallel to `log_posterior`.
    stats: Vec<SegmentStats>,
    observed: usize,
}

impl Default for Bocpd {
    fn default() -> Self {
        Self::new(BocpdConfig::default())
    }
}

impl Bocpd {
    #[must_use]
    pub fn new(config: BocpdConfig) -> Self {
        let hazard = 1.0 / config.lambda_runs;
        Self {
            config,
            log_hazard: hazard.ln(),
            log_survive: (1.0 - hazard).ln(),
            // P(r_0 = 0) = 1: the stream begins at a change point.
            log_posterior: vec![0.0],
            stats: vec![SegmentStats::EMPTY],
            observed: 0,
        }
    }

    #[must_use]
    pub fn config(&self) -> &BocpdConfig {
        &self.config
    }

    /// The normalised run-length posterior, `P(r_t = r)` for `r` in index
    /// order. Exposed for the invariant tests, which are the only reason to
    /// look at it directly.
    #[must_use]
    pub fn run_length_posterior(&self) -> Vec<f64> {
        self.log_posterior.iter().map(|value| value.exp()).collect()
    }

    /// Consumes one closed run and returns the updated shadow quantities.
    ///
    /// # Errors
    ///
    /// [`BocpdError::UnknownCategory`] if the category index is outside the
    /// closed vocabulary; [`BocpdError::NonFiniteDwell`] for a dwell that is
    /// not a finite non-negative number. Neither is coerced: a detector that
    /// quietly repairs its input logs a number about data it did not receive.
    pub fn observe(&mut self, run: RunObservation) -> Result<BocpdUpdate, BocpdError> {
        if run.category >= CATEGORY_COUNT {
            return Err(BocpdError::UnknownCategory(run.category));
        }
        if !run.dwell_seconds.is_finite() || run.dwell_seconds < 0.0 {
            return Err(BocpdError::NonFiniteDwell);
        }
        let d = log_dwell(run.dwell_seconds);

        // 1. Predictive of this observation under every run-length hypothesis.
        let log_predictive: Vec<f64> = self
            .stats
            .iter()
            .map(|stats| self.log_posterior_predictive(stats, run.category, d))
            .collect();

        // 2. Growth and change-point messages.
        let joint: Vec<f64> = self
            .log_posterior
            .iter()
            .zip(&log_predictive)
            .map(|(posterior, predictive)| posterior + predictive)
            .collect();

        let mut next = Vec::with_capacity(joint.len() + 1);
        next.push(log_sum_exp(&joint) + self.log_hazard);
        next.extend(joint.iter().map(|value| value + self.log_survive));

        let mut next_stats = Vec::with_capacity(self.stats.len() + 1);
        next_stats.push(SegmentStats::EMPTY);
        next_stats.extend(self.stats.iter().map(|stats| stats.absorb(run.category, d)));

        // 3. Truncate at R_max, then renormalise — in that order, so the
        //    posterior that is reported is a posterior over the hypotheses that
        //    were kept and not a sub-stochastic remnant of a larger one.
        let keep = self.config.r_max + 1;
        next.truncate(keep);
        next_stats.truncate(keep);
        let total = log_sum_exp(&next);
        for value in &mut next {
            *value -= total;
        }

        self.log_posterior = next;
        self.stats = next_stats;
        self.observed += 1;

        Ok(self.summarise())
    }

    fn summarise(&self) -> BocpdUpdate {
        let mut p_recent_change = 0.0;
        let mut map_run_length = 0usize;
        let mut map_mass = f64::NEG_INFINITY;
        let mut expected_run_length = 0.0;
        for (run_length, log_mass) in self.log_posterior.iter().enumerate() {
            let mass = log_mass.exp();
            if run_length < self.config.recent_run_horizon {
                p_recent_change += mass;
            }
            if *log_mass > map_mass {
                map_mass = *log_mass;
                map_run_length = run_length;
            }
            expected_run_length += mass * run_length as f64;
        }
        BocpdUpdate {
            index: self.observed,
            p_recent_change: p_recent_change.clamp(0.0, 1.0),
            map_run_length,
            expected_run_length,
            warm: self.observed >= WARMUP_RUNS,
        }
    }

    /// `ln p(c_t, d_t | run length r)` — Dirichlet-multinomial times Student-t.
    fn log_posterior_predictive(&self, stats: &SegmentStats, category: usize, d: f64) -> f64 {
        let alpha = self.config.dirichlet_alpha;
        let categorical = ((stats.counts[category] + alpha)
            / (stats.category_total + CATEGORY_COUNT as f64 * alpha))
            .ln();

        let prior = self.config.dwell_prior;
        let n = stats.dwell_count;
        let kappa_n = prior.kappa + n;
        let mu_n = (prior.kappa * prior.mu + n * stats.dwell_mean) / kappa_n;
        let alpha_n = prior.alpha + 0.5 * n;
        let mean_gap = stats.dwell_mean - prior.mu;
        let beta_n = prior.beta
            + 0.5 * stats.dwell_m2
            + (prior.kappa * n * mean_gap * mean_gap) / (2.0 * kappa_n);

        let nu = 2.0 * alpha_n;
        let scale_squared = beta_n * (kappa_n + 1.0) / (alpha_n * kappa_n);
        let z = d - mu_n;
        let student_t = ln_gamma(0.5 * (nu + 1.0))
            - ln_gamma(0.5 * nu)
            - 0.5 * (nu * PI * scale_squared).ln()
            - 0.5 * (nu + 1.0) * (1.0 + z * z / (nu * scale_squared)).ln();

        categorical + student_t
    }
}

/// Numerically stable `ln(sum(exp(values)))`.
fn log_sum_exp(values: &[f64]) -> f64 {
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        return max;
    }
    let sum: f64 = values.iter().map(|value| (value - max).exp()).sum();
    max + sum.ln()
}

/// Lanczos coefficients, `g = 7`, `n = 9`. Roughly 15 significant digits over
/// the positive reals, which is far more than a log-likelihood needs.
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

/// `ln |Gamma(x)|`, Lanczos.
///
/// Hand-rolled because the alternative is a dependency for two calls per
/// observation, and `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.1 is explicit that this
/// layer is dependency-free.
fn ln_gamma(x: f64) -> f64 {
    if x < 0.5 {
        // Reflection. Not reachable at the shipped priors (`nu/2 >= alpha0 = 1`)
        // but present so the function is correct rather than correct-in-range.
        return (PI / (PI * x).sin()).abs().ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut series = LANCZOS[0];
    for (offset, coefficient) in LANCZOS.iter().enumerate().skip(1) {
        series += coefficient / (x + offset as f64);
    }
    let t = x + 7.5;
    0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + series.ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(left: f64, right: f64, tolerance: f64) -> bool {
        (left - right).abs() <= tolerance
    }

    /// The special-function implementation is the one piece of this file that
    /// is arithmetic rather than modelling, so it is checked against closed
    /// forms rather than against itself.
    #[test]
    fn ln_gamma_matches_closed_forms() {
        assert!(approx(ln_gamma(1.0), 0.0, 1e-12));
        assert!(approx(ln_gamma(2.0), 0.0, 1e-12));
        assert!(approx(ln_gamma(0.5), PI.sqrt().ln(), 1e-12));
        assert!(approx(ln_gamma(5.0), 24.0f64.ln(), 1e-11));
        assert!(approx(ln_gamma(10.0), 362_880.0f64.ln(), 1e-10));
        // The reflection branch.
        assert!(approx(
            ln_gamma(0.25) + ln_gamma(0.75),
            (PI / (PI * 0.25).sin()).ln(),
            1e-11
        ));
    }

    #[test]
    fn log_sum_exp_is_stable_at_extreme_magnitudes() {
        assert!(approx(log_sum_exp(&[0.0, 0.0]), 2.0f64.ln(), 1e-12));
        // Would overflow if exponentiated directly.
        assert!(approx(
            log_sum_exp(&[1000.0, 1000.0]),
            1000.0 + 2.0f64.ln(),
            1e-9
        ));
        // Would underflow to zero if exponentiated directly.
        assert!(approx(
            log_sum_exp(&[-1000.0, -1000.0]),
            -1000.0 + 2.0f64.ln(),
            1e-9
        ));
        assert_eq!(log_sum_exp(&[f64::NEG_INFINITY]), f64::NEG_INFINITY);
    }

    /// The Student-t predictive must integrate to one over `d`. If it does not,
    /// the dwell half of the emission is not a density and every run-length
    /// comparison is scaled by an unknown constant.
    #[test]
    fn the_dwell_predictive_is_a_normalised_density() {
        let detector = Bocpd::default();
        let stats = SegmentStats::EMPTY
            .absorb(0, log_dwell(600.0))
            .absorb(0, log_dwell(900.0))
            .absorb(0, log_dwell(400.0));
        // Trapezoid over a wide grid; the categorical factor is a constant here
        // and is divided back out.
        let categorical = ((3.0 + 0.5f64) / (3.0 + 8.0 * 0.5)).ln();
        let step = 0.002;
        let mut integral = 0.0;
        let mut d = -20.0;
        while d < 30.0 {
            integral +=
                (detector.log_posterior_predictive(&stats, 0, d) - categorical).exp() * step;
            d += step;
        }
        assert!(
            approx(integral, 1.0, 1e-3),
            "dwell predictive integrates to {integral}, not 1"
        );
    }

    /// The categorical half must be a proper distribution over the eight
    /// categories at every run length.
    #[test]
    fn the_category_predictive_sums_to_one_over_the_vocabulary() {
        let detector = Bocpd::default();
        let stats = SegmentStats::EMPTY
            .absorb(3, log_dwell(120.0))
            .absorb(3, log_dwell(90.0))
            .absorb(5, log_dwell(300.0));
        let d = log_dwell(200.0);
        let dwell_only = {
            // Recover the Student-t factor by dividing out a known categorical.
            let categorical = ((stats.counts[3] + 0.5) / (3.0 + 4.0)).ln();
            detector.log_posterior_predictive(&stats, 3, d) - categorical
        };
        let total: f64 = (0..CATEGORY_COUNT)
            .map(|category| {
                (detector.log_posterior_predictive(&stats, category, d) - dwell_only).exp()
            })
            .sum();
        assert!(
            approx(total, 1.0, 1e-12),
            "category predictive sums to {total}"
        );
    }

    /// The run-length posterior is a posterior: it sums to one after every
    /// observation, including after truncation.
    #[test]
    fn the_run_length_posterior_stays_normalised_and_truncated() {
        let config = BocpdConfig {
            r_max: 16,
            ..BocpdConfig::default()
        };
        let mut detector = Bocpd::new(config);
        for index in 0..200 {
            let run =
                RunObservation::new(index % CATEGORY_COUNT, 60.0 + (index % 13) as f64 * 40.0);
            detector.observe(run).unwrap();
            let posterior = detector.run_length_posterior();
            assert!(
                posterior.len() <= 17,
                "posterior grew to {} past R_max + 1",
                posterior.len()
            );
            let total: f64 = posterior.iter().sum();
            assert!(
                approx(total, 1.0, 1e-9),
                "posterior sums to {total} at step {index}"
            );
            assert!(posterior.iter().all(|mass| *mass >= 0.0));
        }
    }

    /// A perfectly stationary stream must let the run length grow, and must
    /// leave `P(run_length < 3)` sitting on its floor rather than above it. If
    /// the detector cannot hold a segment together on data with one category
    /// and one dwell, nothing it says about real data means anything.
    #[test]
    fn a_stationary_stream_grows_the_run_length() {
        let mut detector = Bocpd::default();
        let mut last = None;
        for index in 0..40 {
            // A trace with zero variation would make the Student-t degenerate;
            // this varies by a couple of seconds, which is stationary noise.
            let dwell = 600.0 + ((index % 5) as f64 - 2.0) * 3.0;
            last = Some(detector.observe(RunObservation::new(0, dwell)).unwrap());
        }
        let update = last.unwrap();
        let floor = BocpdConfig::default().recent_change_floor();
        assert!(
            update.expected_run_length > 20.0,
            "expected run length {} on a stationary stream",
            update.expected_run_length
        );
        assert!(update.p_recent_change >= floor - 1e-12);
        assert!(
            update.p_recent_change < floor + 0.01,
            "P(recent change) {} sits {} above its floor on a stationary stream",
            update.p_recent_change,
            update.p_recent_change - floor
        );
    }

    /// And an abrupt change must move it. The pair is the point: either test
    /// alone is passed by a degenerate detector.
    ///
    /// The assertion is on the transient, not on a fixed offset. Three runs
    /// after the change the true run length is 3, `P(run_length < 3)` is
    /// correctly back near its floor, and a test that sampled there would read
    /// a working detector as a broken one.
    #[test]
    fn an_abrupt_change_raises_the_recent_change_mass() {
        let mut detector = Bocpd::default();
        for index in 0..30 {
            let dwell = 900.0 + ((index % 5) as f64 - 2.0) * 5.0;
            detector.observe(RunObservation::new(0, dwell)).unwrap();
        }
        let before = detector.observe(RunObservation::new(0, 900.0)).unwrap();
        let floor = BocpdConfig::default().recent_change_floor();
        assert!(before.p_recent_change < floor + 0.01);

        let mut trace = Vec::new();
        for index in 0..6 {
            let dwell = 45.0 + (index % 3) as f64 * 4.0;
            trace.push(
                detector
                    .observe(RunObservation::new(3, dwell))
                    .unwrap()
                    .p_recent_change,
            );
        }
        assert!(
            trace[0] > 0.9,
            "P(recent change) reached only {} on the first run of a new regime, \
             against {} before it",
            trace[0],
            before.p_recent_change
        );
        assert!(trace[1] > 0.9, "and only {} on the second", trace[1]);
        assert!(
            trace[5] < floor + 0.05,
            "the transient did not decay: {} six runs into the new regime",
            trace[5]
        );
    }

    /// An erratic-but-stationary stream, used by the two tests below. Category
    /// uniform over the vocabulary, dwell uniform over the clip range, nothing
    /// depending on anything.
    fn shake(detector: &mut Bocpd, steps: usize) -> Vec<Vec<f64>> {
        let mut rolling = 12_345u64;
        let mut posteriors = Vec::with_capacity(steps);
        for _ in 0..steps {
            rolling = rolling
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let category = ((rolling >> 33) % CATEGORY_COUNT as u64) as usize;
            let dwell = 30.0 + ((rolling >> 20) % 1700) as f64;
            detector
                .observe(RunObservation::new(category, dwell))
                .unwrap();
            posteriors.push(detector.run_length_posterior());
        }
        posteriors
    }

    /// `P(r_t = 0) = H` exactly, at every step, on every data set — until
    /// truncation binds. This is the derivation in the module documentation,
    /// asserted rather than described. It is the floor under every threshold
    /// anyone will ever put on this detector, and it would be very easy to ship
    /// a threshold below it.
    #[test]
    fn the_change_point_mass_is_exactly_the_hazard_until_truncation_binds() {
        let config = BocpdConfig::default();
        assert!(approx(config.hazard(), 1.0 / 6.0, 1e-12));
        assert!(approx(config.recent_change_floor(), 1.0 / 6.0, 1e-12));

        let mut detector = Bocpd::new(config);
        for (step, posterior) in shake(&mut detector, config.r_max).iter().enumerate() {
            assert!(
                approx(posterior[0], config.hazard(), 1e-12),
                "P(r = 0) was {} rather than the hazard at step {step}, before \
                 truncation can bind",
                posterior[0]
            );
        }
    }

    /// And once truncation does bind, the identity degrades by exactly the
    /// discarded tail mass — which is the truncation error, measured here
    /// because this is the only place anything measures it.
    ///
    /// The comparison against a larger `R_max` is what makes this a test rather
    /// than a recorded constant: if the deviation did not shrink when the
    /// posterior was allowed to grow longer, it would be a defect and not
    /// truncation.
    ///
    /// It also says something about the choice of `R_max = 64`. On an i.i.d.
    /// stream the detector correctly wants one unbounded segment, so mass piles
    /// against the truncation boundary and stays there. Sixty-four is a bound on
    /// cost, and the price is a small persistent distortion on exactly the data
    /// where the answer is "nothing happened".
    #[test]
    fn truncation_error_shrinks_when_the_posterior_is_allowed_to_grow() {
        let worst = |r_max: usize| {
            let config = BocpdConfig {
                r_max,
                ..BocpdConfig::default()
            };
            let mut detector = Bocpd::new(config);
            shake(&mut detector, 400)
                .iter()
                .skip(r_max)
                .map(|posterior| (posterior[0] - config.hazard()).abs())
                .fold(0.0f64, f64::max)
        };
        let shipped = worst(DEFAULT_R_MAX);
        let generous = worst(256);
        assert!(
            shipped > 0.0,
            "truncation never bound, so this test measured nothing"
        );
        assert!(
            generous < shipped,
            "raising R_max from {DEFAULT_R_MAX} to 256 did not reduce the \
             deviation ({generous} against {shipped}), so it is not truncation"
        );
        assert!(
            shipped < 0.05,
            "truncating at R_max = {DEFAULT_R_MAX} moved the change-point mass \
             by {shipped}, which is large enough to matter to a threshold"
        );
    }

    /// Bad input is refused, never coerced.
    #[test]
    fn out_of_vocabulary_and_non_finite_input_is_refused() {
        let mut detector = Bocpd::default();
        assert_eq!(
            detector.observe(RunObservation::new(CATEGORY_COUNT, 100.0)),
            Err(BocpdError::UnknownCategory(CATEGORY_COUNT))
        );
        assert_eq!(
            detector.observe(RunObservation::new(0, f64::NAN)),
            Err(BocpdError::NonFiniteDwell)
        );
        assert_eq!(
            detector.observe(RunObservation::new(0, -1.0)),
            Err(BocpdError::NonFiniteDwell)
        );
        // And a refusal leaves the detector untouched.
        assert_eq!(detector.run_length_posterior().len(), 1);
    }

    /// `d_t` saturates where the collection agent's clip saturates, and nowhere
    /// else.
    #[test]
    fn log_dwell_saturates_at_the_collection_clip() {
        assert!(approx(log_dwell(0.0), 0.0, 1e-12));
        assert!(approx(log_dwell(1800.0), 1801.0f64.ln(), 1e-12));
        assert!(approx(log_dwell(100_000.0), 1801.0f64.ln(), 1e-12));
    }
}
