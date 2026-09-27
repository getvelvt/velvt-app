#!/usr/bin/env python3
"""How much data does a personal nudge rule need? An offline design simulator.

Everything this prints is computed from ASSUMPTIONS passed on the command line.
Nothing here reads a database, a tester's export or `~/.velvt`, and no number it
prints is a measurement of anyone. The eligible-point rate in particular has
never been measured on any Mac but the founder's, where it is near zero.

It answers three questions about the drafted (not enabled) v6 design, in which
the unchanged v5 gate decides WHEN a point is eligible and a fixed draw decides
WHETHER to offer there:

  1. The average effect. At the pre-registered power statement (0.30 baseline
     under silence, alpha 0.05 two-sided, 80% power, offer p = 0.7 and silence
     p = 0.3), how many randomized, scored eligible points are needed, and how
     many weeks does the cohort take to produce them?

  2. The declared withhold candidate. `ReturnLedger::would_withhold` in
     `rust-service/src/behavior/returns.rs` is the one per-person rule declared
     for offline evaluation. It never looks at randomized points: it withholds
     at a point when, in each of the point's three context cells, the lower end
     of the 80% interval on the person's own return rate over the last 28 days
     is at least 0.60 (no-nudge returns after departures the gate did not act
     on). How often does it answer at all, how often does it fire, and how
     often is the person it fires for one the nudge does not help? That last
     part depends entirely on how a person's own-return rate relates to what a
     nudge does for them, which nothing has measured, so it is shown under two
     stated links.

  3. An effect-based rule, for comparison. NOT the declared candidate, and not
     the v6 draft's own rule: withhold for a person when the one-sided upper
     bound (at --rule-level) of their own randomized offer-minus-silence
     difference is below --benefit-floor. It shows how many randomized points
     one person needs before any rule of that kind is mostly right.

v6 as drafted, and as simulated here:

  - At most one randomized decision per block, the block's first eligible
    point with at least 900 seconds left. A point with less left is offered
    as v5 would, with propensity 1.0, and never enters the comparison: its
    900-second horizon cannot fit inside the block.
  - Eligible points per block are Poisson with the given mean, so a block
    yields a draw with probability 1 - exp(-mean x share_with_900s).
  - A drawn point is scored unless an observer gap censors it.
  - Per-person effects vary: effect_i ~ Normal(effect, effect_sd).

Usage:
    ./scripts/simulate_nudge_designs.py --testers 10 --blocks-per-week 3 \\
        --eligible-per-block 0.5 --effect 0.15
    ./scripts/simulate_nudge_designs.py --scenarios        # the preset table
    ./scripts/simulate_nudge_designs.py ... --json         # machine-readable

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from dataclasses import asdict, dataclass
from statistics import NormalDist

LABEL = ("SYNTHETIC design simulation. Every input is an assumption, not a "
         "measurement, and no output is evidence about any person.")

# The pre-registered design (traction-summary.md, 2026-08-21; plan/06).
P_OFFER = 0.7
PRE_REGISTERED_BASELINE = 0.30
PRE_REGISTERED_ALPHA = 0.05
PRE_REGISTERED_POWER = 0.80
DRAW_REQUIRES_SECONDS_LEFT = 900

# Horizons at which the effect-based rule is reported, in weeks.
RULE_WEEKS = (4, 8, 13, 26, 52)

# How a person's effect relates to their own-return rate, for the declared
# candidate. Both are assumptions; neither is measured.
#   unrelated: effect_i ~ Normal(effect, effect_sd), whatever the person's rate.
#   headroom:  effect_i = effect x (1 - rate_i) / (1 - mean rate) + Normal(0,
#              effect_sd): a person who comes back on their own more often has
#              less room for a nudge to help. This is the premise the declared
#              candidate rests on.
LINKS = ("headroom", "unrelated")
LINK_TEXT = {
    "headroom": "if people who come back on their own more gain less from a nudge",
    "unrelated": "if the two are unrelated",
}

# People simulated per replicate for the declared candidate. It is a rule
# about one person, so the number of testers does not enter it.
DECLARED_PEOPLE_PER_REPLICATE = 50

NORMAL = NormalDist()


@dataclass(frozen=True)
class Assumptions:
    testers: int = 10
    blocks_per_week: float = 3.0
    eligible_per_block: float = 0.5
    share_with_900s: float = 0.75
    censored: float = 0.10
    baseline: float = PRE_REGISTERED_BASELINE
    effect: float = 0.15
    effect_sd: float = 0.05
    alpha: float = PRE_REGISTERED_ALPHA
    power: float = PRE_REGISTERED_POWER
    benefit_floor: float = 0.05
    rule_level: float = 0.80
    departures_per_block: float = 3.0
    ledger_censored: float = 0.30
    own_return_mean: float = 0.55
    own_return_sd: float = 0.15
    replicates: int = 400
    seed: int = 20260927

    def validate(self) -> None:
        problems = []
        if self.testers < 1:
            problems.append("--testers must be at least 1")
        if self.blocks_per_week < 0:
            problems.append("--blocks-per-week must be non-negative")
        if self.eligible_per_block < 0:
            problems.append("--eligible-per-block must be non-negative")
        if self.departures_per_block < 0:
            problems.append("--departures-per-block must be non-negative")
        for name in ("share_with_900s", "censored", "ledger_censored"):
            if not 0.0 <= getattr(self, name) <= 1.0:
                problems.append(f"--{name.replace('_', '-')} must be within [0, 1]")
        if not 0.0 < self.baseline < 1.0:
            problems.append("--baseline must be within (0, 1)")
        if not 0.0 < self.baseline + self.effect < 1.0:
            problems.append("--baseline + --effect must be within (0, 1)")
        if self.effect_sd < 0:
            problems.append("--effect-sd must be non-negative")
        if not 0.0 < self.own_return_mean < 1.0:
            problems.append("--own-return-mean must be within (0, 1)")
        if not 0.0 < self.own_return_sd ** 2 < self.own_return_mean * (1 - self.own_return_mean):
            problems.append("--own-return-sd must be positive and below "
                            "sqrt(mean x (1 - mean))")
        if not 0.0 < self.alpha < 1.0 or not 0.0 < self.power < 1.0:
            problems.append("--alpha and --power must be within (0, 1)")
        if not 0.5 <= self.rule_level < 1.0:
            problems.append("--rule-level must be within [0.5, 1)")
        if self.replicates < 1:
            problems.append("--replicates must be at least 1")
        if problems:
            raise ValueError("; ".join(problems))


# ---------------------------------------------------------------------------
# The average effect
# ---------------------------------------------------------------------------

def randomized_points_per_block(a: Assumptions) -> float:
    """Scored randomized points a block yields: at most one draw per block."""
    drawable = a.eligible_per_block * a.share_with_900s
    return (1.0 - math.exp(-drawable)) * (1.0 - a.censored)


def required_points(baseline: float, effect: float, alpha: float = PRE_REGISTERED_ALPHA,
                    power: float = PRE_REGISTERED_POWER, p_offer: float = P_OFFER) -> float:
    """Pooled randomized points for a two-sided difference in proportions.

    Unequal allocation, variance pooled under the null for the alpha term and
    unpooled under the alternative for the power term. At the pre-registered
    inputs this is the 2026-08-21 power statement's 390.
    """
    p_silence = baseline
    p_offered = baseline + effect
    allocation = 1.0 / p_offer + 1.0 / (1.0 - p_offer)
    pooled = p_offer * p_offered + (1.0 - p_offer) * p_silence
    null_sd = math.sqrt(pooled * (1.0 - pooled) * allocation)
    alternative_sd = math.sqrt(p_offered * (1.0 - p_offered) / p_offer
                               + p_silence * (1.0 - p_silence) / (1.0 - p_offer))
    z_alpha = NORMAL.inv_cdf(1.0 - alpha / 2.0)
    z_power = NORMAL.inv_cdf(power)
    return ((z_alpha * null_sd + z_power * alternative_sd) / effect) ** 2


def required_points_with_spread(a: Assumptions) -> float | None:
    """`required_points` once people differ in their effect.

    The pooled estimate's variance is the within-person part, which shrinks as
    points accrue, plus effect_sd^2 / testers, which does not. If that floor
    alone exceeds the variance the test can afford, no number of points
    reaches the power asked for with this many testers, and this is None.
    """
    base = required_points(a.baseline, a.effect, a.alpha, a.power)
    z = NORMAL.inv_cdf(1.0 - a.alpha / 2.0) + NORMAL.inv_cdf(a.power)
    share = (a.effect_sd * z / a.effect) ** 2 / a.testers
    if share >= 1.0:
        return None
    return base / (1.0 - share)


def weeks_to(points: float | None, per_week: float) -> float | None:
    if points is None or per_week <= 0:
        return None
    return points / per_week


# ---------------------------------------------------------------------------
# The declared withhold candidate: a port of `ReturnLedger::would_withhold`
# (rust-service/src/behavior/returns.rs, RETURN_LEDGER_MODEL_VERSION 1). Every
# constant and every step below restates that file, which is the authority.
# Both sides pin the same vector (`the_declared_rule_matches_its_offline_port`
# there, the port's check in scripts/tests/simulate_nudge_designs_test.sh
# here), so a change made on one side only fails a test.
# ---------------------------------------------------------------------------

LEDGER_MODEL_VERSION = 1
LEDGER_LOOKBACK_DAYS = 28
LEDGER_MIN_BLOCKS = 6
LEDGER_MIN_RESOLVED_ROWS = 30
LEDGER_MIN_CELL_ROWS = 8
LEDGER_MIN_CELL_BLOCKS = 3
LEDGER_BASELINE_PRIOR = (4.0, 4.0)
LEDGER_CELL_PRIOR_STRENGTH = 4.0
LEDGER_WITHIN_BLOCK_CORRELATION = 0.3
LEDGER_INTERVAL_MASS = 0.80
LEDGER_WITHHOLD_LOWER_BOUND = 0.60

DEPARTURE_CELLS = ("departure.communication", "departure.feeds_and_video",
                   "departure.work_adjacent")
ELAPSED_CELLS = ("elapsed.first_third", "elapsed.middle_third", "elapsed.final_third")
HOUR_CELLS = ("hour.morning", "hour.afternoon", "hour.evening_and_night")

# ASSUMPTION, the same as suite E's: where departures go. The block third and
# the part of the day are uniform.
DEPARTURE_WEIGHTS = (0.40, 0.30, 0.30)

# Lanczos coefficients, g = 7, n = 9: the table `returns.rs` and `bocpd.rs` use.
_LANCZOS = (
    0.9999999999998099, 676.5203681218851, -1259.1392167224028,
    771.3234287776531, -176.6150291621406, 12.507343278686905,
    -0.13857109526572012, 9.984369578019572e-6, 1.5056327351493116e-7,
)


def _ln_gamma(x: float) -> float:
    x -= 1.0
    series = _LANCZOS[0]
    for offset, coefficient in enumerate(_LANCZOS[1:], start=1):
        series += coefficient / (x + offset)
    t = x + 7.5
    return 0.5 * math.log(2.0 * math.pi) + (x + 0.5) * math.log(t) - t + math.log(series)


def _beta_continued_fraction(a: float, b: float, x: float) -> float:
    tiny, epsilon = 1e-300, 1e-15

    def floor(value: float) -> float:
        return tiny if abs(value) < tiny else value

    c = 1.0
    d = 1.0 / floor(1.0 - (a + b) * x / (a + 1.0))
    fraction = d
    for step in range(1, 501):
        m = float(step)
        even = m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m))
        d = 1.0 / floor(1.0 + even * d)
        c = floor(1.0 + even / c)
        fraction *= d * c
        odd = -(a + m) * (a + b + m) * x / ((a + 2.0 * m) * (a + 2.0 * m + 1.0))
        d = 1.0 / floor(1.0 + odd * d)
        c = floor(1.0 + odd / c)
        delta = d * c
        fraction *= delta
        if abs(delta - 1.0) < epsilon:
            break
    return fraction


def beta_cdf(x: float, a: float, b: float) -> float:
    """The regularized incomplete beta function: the Beta(a, b) CDF."""
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    front = math.exp(_ln_gamma(a + b) - _ln_gamma(a) - _ln_gamma(b)
                     + a * math.log(x) + b * math.log(1.0 - x))
    if x < (a + 1.0) / (a + b + 2.0):
        return front * _beta_continued_fraction(a, b, x) / a
    return 1.0 - front * _beta_continued_fraction(b, a, 1.0 - x) / b


def beta_quantile(p: float, a: float, b: float) -> float:
    """The p quantile of Beta(a, b), by the same sixty halvings as the ledger."""
    low, high = 0.0, 1.0
    for _ in range(60):
        middle = 0.5 * (low + high)
        if beta_cdf(middle, a, b) < p:
            low = middle
        else:
            high = middle
    return 0.5 * (low + high)


@dataclass(frozen=True)
class LedgerRow:
    """One resolved row: its block, its three cells, and whether it returned."""
    block: int
    cells: tuple[str, str, str]
    returned: bool


def _tally(rows: list[LedgerRow]) -> dict[int, tuple[int, int]]:
    blocks: dict[int, tuple[int, int]] = {}
    for row in rows:
        count, returned = blocks.get(row.block, (0, 0))
        blocks[row.block] = (count + 1, returned + int(row.returned))
    return blocks


def _posterior(prior: tuple[float, float], tally: dict[int, tuple[int, int]]) -> tuple[float, float]:
    """Kish-weighted pseudo-counts on `prior`. No block is disputed here."""
    returned = resolved = 0.0
    for rows, block_returned in tally.values():
        per_row = 1.0 / (1.0 + (rows - 1) * LEDGER_WITHIN_BLOCK_CORRELATION)
        returned += per_row * block_returned
        resolved += per_row * rows
    return prior[0] + returned, prior[1] + (resolved - returned)


def _cell_prior(rows: list[LedgerRow]) -> tuple[float, float]:
    alpha, beta = _posterior(LEDGER_BASELINE_PRIOR, _tally(rows))
    centre = alpha / (alpha + beta)
    return LEDGER_CELL_PRIOR_STRENGTH * centre, LEDGER_CELL_PRIOR_STRENGTH * (1.0 - centre)


def ledger_abstains(rows: list[LedgerRow]) -> bool:
    return len(_tally(rows)) < LEDGER_MIN_BLOCKS or len(rows) < LEDGER_MIN_RESOLVED_ROWS


def cell_estimate(rows: list[LedgerRow], cell: str) -> dict:
    """The ledger's `RateEstimate` for one cell: counts and the lower bound."""
    tally = _tally([row for row in rows if cell in row.cells])
    alpha, beta = _posterior(_cell_prior(rows), tally)
    tail = (1.0 - LEDGER_INTERVAL_MASS) / 2.0
    return {
        "returned": sum(returned for _, returned in tally.values()),
        "resolved": sum(count for count, _ in tally.values()),
        "blocks": len(tally),
        "posterior": (alpha, beta),
        "lower": beta_quantile(tail, alpha, beta),
    }


def would_withhold(rows: list[LedgerRow], point: tuple[str, str, str],
                   exact: bool = True) -> dict:
    """`ReturnLedger::would_withhold` at a point with these three cells.

    `exact` computes each lower bound as the ledger does, by bisection.
    Without it, "lower bound >= 0.60" is tested as "CDF at 0.60 <= 0.10",
    which is the same statement for a continuous, increasing CDF and sixty
    times cheaper; the Monte Carlo uses it, and the test checks the two agree.
    """
    abstained = ledger_abstains(rows)
    tail = (1.0 - LEDGER_INTERVAL_MASS) / 2.0
    lowers: list[float] = []
    clears = True
    prior = _cell_prior(rows)
    for cell in point:
        tally = _tally([row for row in rows if cell in row.cells])
        if (sum(count for count, _ in tally.values()) < LEDGER_MIN_CELL_ROWS
                or len(tally) < LEDGER_MIN_CELL_BLOCKS):
            return {"would_withhold": False, "min_lower": None, "abstained": abstained}
        alpha, beta = _posterior(prior, tally)
        if exact:
            lowers.append(beta_quantile(tail, alpha, beta))
        else:
            clears = clears and beta_cdf(LEDGER_WITHHOLD_LOWER_BOUND, alpha, beta) <= tail
    if exact:
        min_lower = min(lowers)
        fires = min_lower >= LEDGER_WITHHOLD_LOWER_BOUND
    else:
        min_lower, fires = None, clears
    return {"would_withhold": not abstained and fires, "min_lower": min_lower,
            "abstained": abstained}


def _draw_cells(rng: random.Random) -> tuple[str, str, str]:
    roll = rng.random()
    departure = DEPARTURE_CELLS[-1]
    for cell, weight in zip(DEPARTURE_CELLS, DEPARTURE_WEIGHTS):
        if roll < weight:
            departure = cell
            break
        roll -= weight
    return departure, rng.choice(ELAPSED_CELLS), rng.choice(HOUR_CELLS)


def _beta_parameters(mean: float, sd: float) -> tuple[float, float]:
    strength = mean * (1.0 - mean) / sd ** 2 - 1.0
    return mean * strength, (1.0 - mean) * strength


def simulate_declared_rule(a: Assumptions) -> dict:
    """Monte Carlo of the declared candidate, on one 28-day window per person.

    The candidate reads only the last 28 days, so what it does does not grow
    with weeks of use: from week 4 on, it is this. Each simulated person has an
    own-return rate ~ Beta(mean, sd). Their resolved ledger rows are Poisson
    departures per block, less the censored share, each returning at that
    rate in every cell alike; so any cell the rule treats differently is
    noise. The rows are drawn with no bias from the treated censoring, which
    in the ledger makes rates read high: this is the rule at its best. The
    rule is asked about one eligible point per person, with cells drawn the
    same way. `replicates x DECLARED_PEOPLE_PER_REPLICATE` people in all.
    """
    rng = random.Random(f"{a.seed}:declared")
    alpha, beta = _beta_parameters(a.own_return_mean, a.own_return_sd)
    blocks_in_window = a.blocks_per_week * LEDGER_LOOKBACK_DAYS / 7.0
    people = a.replicates * DECLARED_PEOPLE_PER_REPLICATE
    abstained = flagged = 0
    rows_total = 0
    below = {link: 0 for link in LINKS}
    hits = {link: 0 for link in LINKS}
    for _ in range(people):
        own_rate = rng.betavariate(alpha, beta)
        noise = rng.gauss(0.0, a.effect_sd)
        effects = {
            "headroom": a.effect * (1.0 - own_rate) / (1.0 - a.own_return_mean) + noise,
            "unrelated": a.effect + noise,
        }
        rows: list[LedgerRow] = []
        for block in range(_poisson(rng, blocks_in_window)):
            for _ in range(_poisson(rng, a.departures_per_block)):
                if rng.random() < a.ledger_censored:
                    continue
                rows.append(LedgerRow(block, _draw_cells(rng), rng.random() < own_rate))
        rows_total += len(rows)
        candidate = would_withhold(rows, _draw_cells(rng), exact=False)
        abstained += candidate["abstained"]
        flagged += candidate["would_withhold"]
        for link in LINKS:
            is_below = effects[link] < a.benefit_floor
            below[link] += is_below
            hits[link] += is_below and candidate["would_withhold"]
    return {
        "model_version": LEDGER_MODEL_VERSION,
        "window_days": LEDGER_LOOKBACK_DAYS,
        "people": people,
        "resolved_rows_per_person": rows_total / people,
        "abstained_share": abstained / people,
        "flagged_share": flagged / people,
        "by_link": {
            link: {
                "truly_below_floor_share": below[link] / people,
                "precision": hits[link] / flagged if flagged else None,
                "recall": hits[link] / below[link] if below[link] else None,
            }
            for link in LINKS
        },
    }


# ---------------------------------------------------------------------------
# One person: the effect-based rule, for comparison only
# ---------------------------------------------------------------------------

def per_person_variance_unit(baseline: float, effect: float) -> float:
    """n x Var(offer-minus-silence difference) for one person at 70/30."""
    p_offered = baseline + effect
    return (p_offered * (1.0 - p_offered) / P_OFFER
            + baseline * (1.0 - baseline) / (1.0 - P_OFFER))


def points_to_see_the_sign(baseline: float, effect: float, confidence: float = 0.90) -> float:
    """A person's own randomized points before a true effect of this size
    shows the right sign with the given probability."""
    z = NORMAL.inv_cdf(confidence)
    return per_person_variance_unit(baseline, effect) * (z / effect) ** 2


@dataclass(frozen=True)
class RuleResult:
    weeks: int
    points_per_person: float
    standard_error: float | None
    flagged_share: float
    truly_below_floor_share: float
    precision: float | None
    recall: float | None


def _binomial(rng: random.Random, n: int, p: float) -> int:
    return sum(1 for _ in range(n) if rng.random() < p)


def _poisson(rng: random.Random, mean: float) -> int:
    # Knuth; the means here are small enough.
    if mean <= 0:
        return 0
    if mean > 500:
        return max(0, round(rng.gauss(mean, math.sqrt(mean))))
    limit = math.exp(-mean)
    count, product = 0, rng.random()
    while product > limit:
        count += 1
        product *= rng.random()
    return count


def simulate_effect_rule(a: Assumptions, weeks: int) -> RuleResult:
    """Monte Carlo of the effect-based rule after `weeks` of randomized points.

    NOT the declared candidate: see the module docs. Each replicate is one
    cohort of `testers` people. A person's scored randomized points are
    Poisson; each is offered with p = 0.7; outcomes are Bernoulli at the
    person's own rates. The estimate adds one return and one non-return to
    each arm so a person with an empty arm still has one.
    """
    rng = random.Random(f"{a.seed}:{weeks}")
    per_week = a.blocks_per_week * randomized_points_per_block(a)
    mean_points = per_week * weeks
    z_rule = NORMAL.inv_cdf(a.rule_level)
    flagged = truly_below = hits = people = 0
    for _ in range(a.replicates):
        for _ in range(a.testers):
            people += 1
            effect_i = rng.gauss(a.effect, a.effect_sd)
            p_silence = a.baseline
            p_offered = min(0.999, max(0.001, a.baseline + effect_i))
            below = effect_i < a.benefit_floor
            truly_below += below
            n = _poisson(rng, mean_points)
            offered = _binomial(rng, n, P_OFFER)
            silent = n - offered
            if offered == 0 or silent == 0:
                continue
            ret_offered = _binomial(rng, offered, p_offered)
            ret_silent = _binomial(rng, silent, p_silence)
            rate_offered = (ret_offered + 1) / (offered + 2)
            rate_silent = (ret_silent + 1) / (silent + 2)
            estimate = rate_offered - rate_silent
            error = math.sqrt(rate_offered * (1 - rate_offered) / (offered + 2)
                              + rate_silent * (1 - rate_silent) / (silent + 2))
            if estimate + z_rule * error < a.benefit_floor:
                flagged += 1
                hits += below
    se = (math.sqrt(per_person_variance_unit(a.baseline, a.effect) / mean_points)
          if mean_points > 0 else None)
    return RuleResult(
        weeks=weeks,
        points_per_person=mean_points,
        standard_error=se,
        flagged_share=flagged / people,
        truly_below_floor_share=truly_below / people,
        precision=hits / flagged if flagged else None,
        recall=hits / truly_below if truly_below else None,
    )


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

def evaluate(a: Assumptions) -> dict:
    a.validate()
    per_block = randomized_points_per_block(a)
    cohort_per_week = a.testers * a.blocks_per_week * per_block
    base = required_points(a.baseline, a.effect, a.alpha, a.power)
    spread = required_points_with_spread(a)
    sign_points = points_to_see_the_sign(a.baseline, a.effect)
    person_per_week = a.blocks_per_week * per_block
    return {
        "label": LABEL,
        "assumptions": asdict(a),
        "design": {
            "p_offer": P_OFFER,
            "p_silence": round(1 - P_OFFER, 2),
            "draws_per_block_at_most": 1,
            "draw_requires_seconds_left": DRAW_REQUIRES_SECONDS_LEFT,
        },
        "randomized_points_per_block": per_block,
        "cohort_points_per_week": cohort_per_week,
        "average_effect": {
            "required_points_equal_effects": base,
            "required_points_with_effect_sd": spread,
            "reachable": spread is not None,
            "weeks_to_required": weeks_to(spread, cohort_per_week),
        },
        "declared_rule": simulate_declared_rule(a),
        "one_person": {
            "points_to_see_the_sign_90": sign_points,
            "weeks_to_see_the_sign_90": weeks_to(sign_points, person_per_week),
            "effect_rule": [asdict(simulate_effect_rule(a, weeks)) for weeks in RULE_WEEKS],
        },
    }


def _fmt(value: float | None, digits: int = 1, never: str = "never") -> str:
    if value is None:
        return never
    return f"{value:,.{digits}f}"


def _share(value: float | None) -> str:
    return "-" if value is None else f"{value:.0%}"


def render(result: dict) -> str:
    a = result["assumptions"]
    effect = result["average_effect"]
    person = result["one_person"]
    declared = result["declared_rule"]
    lines = [
        result["label"],
        "",
        "v6 as drafted (not enabled): the v5 gate decides eligibility; offer "
        f"p={P_OFFER:.1f}, silence p={1 - P_OFFER:.1f}; at most one draw per block, "
        f"only with >= {DRAW_REQUIRES_SECONDS_LEFT} s left.",
        "",
        f"Assumed: {a['testers']} testers x {a['blocks_per_week']:g} blocks/week; "
        f"{a['eligible_per_block']:g} eligible points/block (Poisson), "
        f"{a['share_with_900s']:.0%} of them with >= 900 s left, "
        f"{a['censored']:.0%} censored by observer gaps.",
        f"Scored randomized points: {result['randomized_points_per_block']:.3f} per block, "
        f"{result['cohort_points_per_week']:.1f} per week across the cohort.",
        "",
        f"Average effect of {a['effect'] * 100:g} points over a {a['baseline']:.2f} baseline "
        f"(alpha {a['alpha']:g} two-sided, power {a['power']:.0%}):",
        f"  points needed if everyone's effect were equal: "
        f"{_fmt(effect['required_points_equal_effects'], 0)}",
        f"  points needed with effect SD {a['effect_sd']:g} across {a['testers']} testers: "
        f"{_fmt(effect['required_points_with_effect_sd'], 0, 'not reachable with this many testers')}",
        f"  weeks to get there: {_fmt(effect['weeks_to_required'])}",
        "",
        "The declared withhold candidate (ReturnLedger::would_withhold, ledger model "
        f"{declared['model_version']}): withhold where the 80% lower bound of the person's "
        f"own return rate is >= {LEDGER_WITHHOLD_LOWER_BOUND:.2f} in each of the point's "
        f"three cells, over the last {declared['window_days']} days.",
        f"  Assumed: {a['departures_per_block']:g} departures the gate leaves alone per "
        f"block, {a['ledger_censored']:.0%} of them censored; own-return rate mean "
        f"{a['own_return_mean']:.2f}, SD {a['own_return_sd']:.2f}, the same in every cell.",
        f"  resolved rows per person in the window: {declared['resolved_rows_per_person']:.1f}; "
        f"abstains for {declared['abstained_share']:.0%}; fires for "
        f"{declared['flagged_share']:.1%} ({declared['people']:,} simulated people)",
    ]
    for link in LINKS:
        entry = declared["by_link"][link]
        lines.append(
            f"  {LINK_TEXT[link]}: the nudge is worth < {a['benefit_floor'] * 100:g} "
            f"points to {entry['truly_below_floor_share']:.0%} of people; of those it "
            f"fires for, {_share(entry['precision'])} are among them (precision), and it "
            f"finds {_share(entry['recall'])} of them (recall)"
        )
    lines += [
        "",
        f"One person, at {result['cohort_points_per_week'] / max(a['testers'], 1):.2f} "
        "scored randomized points a week:",
        f"  points before a true {a['effect'] * 100:g}-point effect shows the right sign "
        f"9 times in 10: {person['points_to_see_the_sign_90']:.0f} "
        f"({_fmt(person['weeks_to_see_the_sign_90'])} weeks)",
        f"  effect-based rule, for comparison (NOT the declared candidate): withhold when "
        f"the {a['rule_level']:.0%} upper bound of the person's own offer-minus-silence "
        f"effect is below {a['benefit_floor'] * 100:g} points ({a['replicates']} simulated "
        "cohorts)",
        "    weeks  points/person  SE of effect  flagged  truly below  precision  recall",
    ]
    for row in person["effect_rule"]:
        lines.append(
            f"    {row['weeks']:>5}  {row['points_per_person']:>13.1f}  "
            f"{_fmt(row['standard_error'], 3, '-'):>12}  {row['flagged_share']:>7.1%}  "
            f"{row['truly_below_floor_share']:>11.1%}  "
            f"{_fmt(None if row['precision'] is None else row['precision'] * 100, 0, '-'):>8}%  "
            f"{_fmt(None if row['recall'] is None else row['recall'] * 100, 0, '-'):>5}%"
        )
    return "\n".join(lines) + "\n"


SCENARIOS = (
    # (testers, blocks per week, eligible points per block, effect)
    (10, 3, 0.5, 0.15),
    (10, 3, 1.5, 0.15),
    (10, 3, 0.5, 0.075),
    (10, 3, 1.5, 0.075),
    (40, 3, 0.5, 0.15),
    (40, 3, 1.5, 0.15),
)


def render_scenarios(base: Assumptions) -> tuple[str, list[dict]]:
    rows = []
    for testers, blocks, eligible, effect in SCENARIOS:
        a = Assumptions(**{**asdict(base), "testers": testers, "blocks_per_week": blocks,
                           "eligible_per_block": eligible, "effect": effect})
        rows.append(evaluate(a))
    lines = [
        LABEL,
        "",
        f"Preset scenarios. Shared assumptions: {base.share_with_900s:.0%} of eligible "
        f"points with >= 900 s left, {base.censored:.0%} censored, baseline "
        f"{base.baseline:.2f}, effect SD {base.effect_sd:g}. Declared candidate: "
        f"{base.departures_per_block:g} departures the gate leaves alone per block, "
        f"{base.ledger_censored:.0%} censored, own-return rate {base.own_return_mean:.2f} "
        f"(SD {base.own_return_sd:.2f}); precision under the headroom link / with no "
        f"link. Effect-based rule (not declared) at {base.rule_level:.0%} against a "
        f"{base.benefit_floor * 100:g}-point floor.",
        "",
        "testers  blocks/wk  eligible/block  effect  points/wk  N needed  weeks to N  "
        "person: weeks to sign  declared: abstains  fires  precision  "
        "effect-based precision @26w  @52w",
    ]
    for row in rows:
        a = row["assumptions"]
        declared = row["declared_rule"]
        rule = {entry["weeks"]: entry for entry in row["one_person"]["effect_rule"]}
        precision = (f"{_share(declared['by_link']['headroom']['precision'])}/"
                     f"{_share(declared['by_link']['unrelated']['precision'])}")
        lines.append(
            f"{a['testers']:>7}  {a['blocks_per_week']:>9g}  {a['eligible_per_block']:>14g}  "
            f"{a['effect'] * 100:>5g}p  {row['cohort_points_per_week']:>9.1f}  "
            f"{_fmt(row['average_effect']['required_points_with_effect_sd'], 0, 'unreach.'):>8}  "
            f"{_fmt(row['average_effect']['weeks_to_required']):>10}  "
            f"{_fmt(row['one_person']['weeks_to_see_the_sign_90']):>21}  "
            f"{declared['abstained_share']:>18.0%}  {declared['flagged_share']:>5.1%}  "
            f"{precision:>9}  "
            f"{_share(rule[26]['precision']):>27}  {_share(rule[52]['precision']):>4}"
        )
    return "\n".join(lines) + "\n", rows


def parse(argv: list[str]) -> tuple[Assumptions, argparse.Namespace]:
    defaults = Assumptions()
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--testers", type=int, default=defaults.testers)
    parser.add_argument("--blocks-per-week", type=float, default=defaults.blocks_per_week)
    parser.add_argument("--eligible-per-block", type=float, default=defaults.eligible_per_block,
                        help="mean eligible decision points per block under the v5 gate; "
                             "never measured off the founder's Mac")
    parser.add_argument("--share-with-900s", type=float, default=defaults.share_with_900s,
                        help="share of eligible points with at least 900 s left")
    parser.add_argument("--censored", type=float, default=defaults.censored,
                        help="share of drawn points censored by an observer gap")
    parser.add_argument("--baseline", type=float, default=defaults.baseline,
                        help="sustained-return rate under silence")
    parser.add_argument("--effect", type=float, default=defaults.effect,
                        help="average offer-minus-silence difference, as a proportion")
    parser.add_argument("--effect-sd", type=float, default=defaults.effect_sd,
                        help="spread of that difference across people")
    parser.add_argument("--alpha", type=float, default=defaults.alpha)
    parser.add_argument("--power", type=float, default=defaults.power)
    parser.add_argument("--benefit-floor", type=float, default=defaults.benefit_floor,
                        help="withholding is right for a person whose effect is below "
                             "this; the effect-based rule also withholds on it")
    parser.add_argument("--rule-level", type=float, default=defaults.rule_level,
                        help="the effect-based rule's one-sided level")
    parser.add_argument("--departures-per-block", type=float,
                        default=defaults.departures_per_block,
                        help="mean departures per block the gate does not act on, the "
                             "declared candidate's rows; never measured")
    parser.add_argument("--ledger-censored", type=float, default=defaults.ledger_censored,
                        help="share of those departures censored (block end, gaps, offers)")
    parser.add_argument("--own-return-mean", type=float, default=defaults.own_return_mean,
                        help="mean across people of their own return rate after such a "
                             "departure")
    parser.add_argument("--own-return-sd", type=float, default=defaults.own_return_sd,
                        help="spread of that rate across people")
    parser.add_argument("--replicates", type=int, default=defaults.replicates)
    parser.add_argument("--seed", type=int, default=defaults.seed)
    parser.add_argument("--scenarios", action="store_true",
                        help="print the preset scenario table instead of one scenario")
    parser.add_argument("--json", action="store_true", help="print JSON")
    args = parser.parse_args(argv)
    assumptions = Assumptions(
        testers=args.testers, blocks_per_week=args.blocks_per_week,
        eligible_per_block=args.eligible_per_block, share_with_900s=args.share_with_900s,
        censored=args.censored, baseline=args.baseline, effect=args.effect,
        effect_sd=args.effect_sd, alpha=args.alpha, power=args.power,
        benefit_floor=args.benefit_floor, rule_level=args.rule_level,
        departures_per_block=args.departures_per_block,
        ledger_censored=args.ledger_censored, own_return_mean=args.own_return_mean,
        own_return_sd=args.own_return_sd, replicates=args.replicates, seed=args.seed,
    )
    return assumptions, args


def main(argv: list[str] | None = None) -> int:
    assumptions, args = parse(sys.argv[1:] if argv is None else argv)
    try:
        assumptions.validate()
    except ValueError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    if args.scenarios:
        text, rows = render_scenarios(assumptions)
        if args.json:
            print(json.dumps({"label": LABEL, "scenarios": rows}, indent=2, sort_keys=True))
        else:
            print(text, end="")
        return 0
    result = evaluate(assumptions)
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(render(result), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
