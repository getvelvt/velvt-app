#!/usr/bin/env python3
"""How much data does a personal nudge rule need? An offline design simulator.

Everything this prints is computed from ASSUMPTIONS passed on the command line.
Nothing here reads a database, a tester's export or `~/.velvt`, and no number it
prints is a measurement of anyone. The eligible-point rate in particular has
never been measured on any Mac but the founder's, where it is near zero.

It answers two questions about the drafted (not enabled) v6 design, in which
the unchanged v5 gate decides WHEN a point is eligible and a fixed draw decides
WHETHER to offer there:

  1. The average effect. At the pre-registered power statement (0.30 baseline
     under silence, alpha 0.05 two-sided, 80% power, offer p = 0.7 and silence
     p = 0.3), how many randomized, scored eligible points are needed, and how
     many weeks does the cohort take to produce them?

  2. One person. If a rule withheld the nudge for a person whose own randomized
     points said it did not help them, how often would that rule be right?

v6 as drafted, and as simulated here:

  - At most one randomized decision per block, the block's first eligible
    point with at least 900 seconds left. A point with less left is offered
    as v5 would, with propensity 1.0, and never enters the comparison: its
    900-second horizon cannot fit inside the block.
  - Eligible points per block are Poisson with the given mean, so a block
    yields a draw with probability 1 - exp(-mean x share_with_900s).
  - A drawn point is scored unless an observer gap censors it.
  - Per-person effects vary: effect_i ~ Normal(effect, effect_sd).

The per-person withhold rule is the one declared for offline evaluation:
withhold for a person when the one-sided upper bound (at --rule-level) of
their own offer-minus-silence difference is below --benefit-floor. Precision
is the share of flagged people whose true effect is below that floor.

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

# Horizons at which the per-person rule is reported, in weeks.
RULE_WEEKS = (4, 8, 13, 26, 52)

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
        for name in ("share_with_900s", "censored"):
            if not 0.0 <= getattr(self, name) <= 1.0:
                problems.append(f"--{name.replace('_', '-')} must be within [0, 1]")
        if not 0.0 < self.baseline < 1.0:
            problems.append("--baseline must be within (0, 1)")
        if not 0.0 < self.baseline + self.effect < 1.0:
            problems.append("--baseline + --effect must be within (0, 1)")
        if self.effect_sd < 0:
            problems.append("--effect-sd must be non-negative")
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
# One person
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


def simulate_withhold_rule(a: Assumptions, weeks: int) -> RuleResult:
    """Monte Carlo of the declared per-person withhold rule after `weeks`.

    Each replicate is one cohort of `testers` people. A person's scored
    randomized points are Poisson; each is offered with p = 0.7; outcomes are
    Bernoulli at the person's own rates. The estimate adds one return and one
    non-return to each arm so a person with an empty arm still has one.
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
        "one_person": {
            "points_to_see_the_sign_90": sign_points,
            "weeks_to_see_the_sign_90": weeks_to(sign_points, person_per_week),
            "withhold_rule": [asdict(simulate_withhold_rule(a, weeks)) for weeks in RULE_WEEKS],
        },
    }


def _fmt(value: float | None, digits: int = 1, never: str = "never") -> str:
    if value is None:
        return never
    return f"{value:,.{digits}f}"


def render(result: dict) -> str:
    a = result["assumptions"]
    effect = result["average_effect"]
    person = result["one_person"]
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
        f"One person, at {result['cohort_points_per_week'] / max(a['testers'], 1):.2f} "
        "scored randomized points a week:",
        f"  points before a true {a['effect'] * 100:g}-point effect shows the right sign "
        f"9 times in 10: {person['points_to_see_the_sign_90']:.0f} "
        f"({_fmt(person['weeks_to_see_the_sign_90'])} weeks)",
        f"  withhold rule: withhold when the {a['rule_level']:.0%} upper bound of the "
        f"person's own effect is below {a['benefit_floor'] * 100:g} points "
        f"({a['replicates']} simulated cohorts)",
        "    weeks  points/person  SE of effect  flagged  truly below  precision  recall",
    ]
    for row in person["withhold_rule"]:
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
        f"{base.baseline:.2f}, effect SD {base.effect_sd:g}, withhold rule at "
        f"{base.rule_level:.0%} against a {base.benefit_floor * 100:g}-point floor.",
        "",
        "testers  blocks/wk  eligible/block  effect  points/wk  N needed  weeks to N  "
        "person: weeks to sign  rule precision @26w  @52w",
    ]
    for row in rows:
        a = row["assumptions"]
        rule = {entry["weeks"]: entry for entry in row["one_person"]["withhold_rule"]}

        def precision(weeks: int) -> str:
            value = rule[weeks]["precision"]
            return "-" if value is None else f"{value:.0%}"

        lines.append(
            f"{a['testers']:>7}  {a['blocks_per_week']:>9g}  {a['eligible_per_block']:>14g}  "
            f"{a['effect'] * 100:>5g}p  {row['cohort_points_per_week']:>9.1f}  "
            f"{_fmt(row['average_effect']['required_points_with_effect_sd'], 0, 'unreach.'):>8}  "
            f"{_fmt(row['average_effect']['weeks_to_required']):>10}  "
            f"{_fmt(row['one_person']['weeks_to_see_the_sign_90']):>21}  "
            f"{precision(26):>19}  {precision(52):>4}"
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
                        help="the per-person rule withholds when the person's effect is "
                             "shown to be below this")
    parser.add_argument("--rule-level", type=float, default=defaults.rule_level)
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
        replicates=args.replicates, seed=args.seed,
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
