#!/usr/bin/env bash
# The design simulator is what the founder reads to decide how much data a
# personal nudge rule needs, so its arithmetic is pinned to the numbers already
# pre-registered, its randomness to a seed, and its output to the copy rules.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
simulator="$repo_root/scripts/simulate_nudge_designs.py"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$simulator" ]] || { echo "ERROR: $simulator is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. The arithmetic reproduces what is already pre-registered and planned.
# ---------------------------------------------------------------------------
python3 - "$simulator" <<'PY' || fail "the power arithmetic drifted from the pre-registration"
import importlib.util, math, sys
spec = importlib.util.spec_from_file_location("sim", sys.argv[1])
sim = importlib.util.module_from_spec(spec)
# Dataclasses resolve their annotations through sys.modules.
sys.modules["sim"] = sim
spec.loader.exec_module(sim)

# traction-summary.md, 2026-08-21: 0.30 baseline, 15 points, alpha 0.05
# two-sided, 80% power, 70/30 allocation -> about 390 eligible points.
n = sim.required_points(0.30, 0.15)
assert 389 <= n <= 391, n
# Halving the effect: the statement's "quadruples to about 1,560" is the x4
# rule of thumb; the exact figure is a little lower.
n_half = sim.required_points(0.30, 0.075)
assert 1_400 <= n_half <= 1_600, n_half
assert n_half > 3.5 * n

# design0: about 77 of a person's own randomized points before a true
# 15-point effect shows the right sign nine times in ten.
assert 76 <= sim.points_to_see_the_sign(0.30, 0.15) <= 78

A = sim.Assumptions
# Equal effects: the spread adjustment changes nothing.
assert abs(sim.required_points_with_spread(A(effect_sd=0.0)) - n) < 1e-9
# People who differ need more points, and more testers need fewer.
assert sim.required_points_with_spread(A(effect_sd=0.05)) > n
assert (sim.required_points_with_spread(A(effect_sd=0.05, testers=40))
        < sim.required_points_with_spread(A(effect_sd=0.05, testers=10)))
# Past a point no number of points is enough with this many testers.
assert sim.required_points_with_spread(A(effect=0.075, effect_sd=0.10, testers=10)) is None

# One draw per block, only with 900 s left: more eligible points help with
# diminishing returns, none with 900 s left means no randomized point at all.
low = sim.randomized_points_per_block(A(eligible_per_block=0.5))
high = sim.randomized_points_per_block(A(eligible_per_block=1.5))
assert 0 < low < high < 1
assert sim.randomized_points_per_block(A(share_with_900s=0.0)) == 0
assert sim.randomized_points_per_block(A(eligible_per_block=50.0, censored=0.0)) <= 1.0
assert sim.weeks_to(390, 0) is None

# The effect-based rule's precision behaves at its extremes: if nobody
# benefits, everyone it flags is right; if everybody benefits by a lot and
# differs by nothing, nobody is truly below the floor and precision is
# undefined.
nobody = sim.simulate_effect_rule(
    A(effect=0.0, effect_sd=0.0, blocks_per_week=10, eligible_per_block=3,
      replicates=50), 52)
assert nobody.flagged_share > 0 and nobody.precision == 1.0, nobody
everybody = sim.simulate_effect_rule(A(effect=0.30, effect_sd=0.0, replicates=50), 26)
assert everybody.truly_below_floor_share == 0 and everybody.recall is None, everybody
PY

# ---------------------------------------------------------------------------
# 1b. The declared candidate is the ledger's rule, not a cousin of it. The
#     vector is `the_declared_rule_matches_its_offline_port` in
#     rust-service/src/behavior/returns.rs; both sides pin these numbers.
# ---------------------------------------------------------------------------
python3 - "$simulator" <<'PY' || fail "the declared candidate drifted from ReturnLedger::would_withhold"
import importlib.util, random, sys
spec = importlib.util.spec_from_file_location("sim", sys.argv[1])
sim = importlib.util.module_from_spec(spec)
sys.modules["sim"] = sim
spec.loader.exec_module(sim)

# Block k, two rows: the first in the first third, to work-adjacent when k is
# even and communication when odd, returning unless k % 4 == 3; the second in
# the middle third, to feeds and video when k % 3 == 0 and work-adjacent
# otherwise, returning unless k % 5 == 0. Part of the day cycles with k % 3.
rows = []
for k in range(20):
    first = "departure.work_adjacent" if k % 2 == 0 else "departure.communication"
    second = "departure.feeds_and_video" if k % 3 == 0 else "departure.work_adjacent"
    hour = sim.HOUR_CELLS[k % 3]
    rows.append(sim.LedgerRow(k, (first, "elapsed.first_third", hour), k % 4 != 3))
    rows.append(sim.LedgerRow(k, (second, "elapsed.middle_third", hour), k % 5 != 0))
assert not sim.ledger_abstains(rows)
pinned = {
    "departure.communication": (5, 10, 10, 0.392546752285),
    "departure.feeds_and_video": (5, 7, 7, 0.536916437166),
    "departure.work_adjacent": (21, 23, 17, 0.792387031023),
    "elapsed.first_third": (15, 20, 20, 0.627717218147),
    "elapsed.middle_third": (16, 20, 20, 0.675806342703),
    "elapsed.final_third": (0, 0, 0, 0.422990126298),
    "hour.morning": (10, 14, 7, 0.561184701132),
    "hour.afternoon": (11, 14, 7, 0.622012140453),
    "hour.evening_and_night": (10, 12, 6, 0.651963632261),
}
for cell, (returned, resolved, blocks, lower) in pinned.items():
    estimate = sim.cell_estimate(rows, cell)
    assert (estimate["returned"], estimate["resolved"], estimate["blocks"]) == \
        (returned, resolved, blocks), (cell, estimate)
    assert abs(estimate["lower"] - lower) < 1e-9, (cell, estimate["lower"])
points = [
    (("departure.work_adjacent", "elapsed.middle_third", "hour.afternoon"), True, 0.622012140453),
    (("departure.work_adjacent", "elapsed.first_third", "hour.morning"), False, 0.561184701132),
    (("departure.communication", "elapsed.first_third", "hour.afternoon"), False, 0.392546752285),
    (("departure.feeds_and_video", "elapsed.middle_third", "hour.morning"), False, None),
]
for point, fires, min_lower in points:
    candidate = sim.would_withhold(rows, point)
    assert candidate["would_withhold"] is fires, (point, candidate)
    if min_lower is None:
        assert candidate["min_lower"] is None, (point, candidate)
    else:
        assert abs(candidate["min_lower"] - min_lower) < 1e-9, (point, candidate)
    # The cheap test the Monte Carlo uses gives the same answer.
    assert sim.would_withhold(rows, point, exact=False)["would_withhold"] is fires, point

# ...on random ledgers too, including ones near the 0.60 line.
rng = random.Random(7)
checked = 0
for _ in range(300):
    rate = rng.uniform(0.5, 0.95)
    rows = [sim.LedgerRow(block, sim._draw_cells(rng), rng.random() < rate)
            for block in range(rng.randint(4, 30)) for _ in range(rng.randint(0, 4))]
    point = sim._draw_cells(rng)
    exact = sim.would_withhold(rows, point)
    assert exact["would_withhold"] == sim.would_withhold(rows, point, exact=False)["would_withhold"]
    checked += exact["min_lower"] is not None
assert checked > 100, checked

# Below the ledger's floors it abstains, and an abstaining ledger never fires.
A = sim.Assumptions
thin = sim.simulate_declared_rule(A(blocks_per_week=1, departures_per_block=1, replicates=20))
assert thin["abstained_share"] > 0.99 and thin["flagged_share"] == 0, thin
# With plenty of rows and everyone coming back on their own, it fires often;
# under the headroom link the people it fires for have small effects, and
# with no link its precision is the base rate of small effects.
busy = A(blocks_per_week=10, departures_per_block=4, own_return_mean=0.8,
         own_return_sd=0.1, replicates=50)
result = sim.simulate_declared_rule(busy)
assert result["abstained_share"] < 0.05 and result["flagged_share"] > 0.2, result
headroom = result["by_link"]["headroom"]
unrelated = result["by_link"]["unrelated"]
assert headroom["precision"] > unrelated["precision"], result
assert abs(unrelated["precision"] - unrelated["truly_below_floor_share"]) < 0.1, result
PY

# ---------------------------------------------------------------------------
# 2. Deterministic from the seed; a different seed moves only the Monte Carlo.
# ---------------------------------------------------------------------------
args=(--testers 10 --blocks-per-week 3 --eligible-per-block 1.5 --replicates 100 --json)
"$simulator" "${args[@]}" > "$work/first.json"
"$simulator" "${args[@]}" > "$work/second.json"
cmp -s "$work/first.json" "$work/second.json" || fail "the same seed gave two answers"
"$simulator" "${args[@]}" --seed 7 > "$work/other.json"
python3 - "$work/first.json" "$work/other.json" <<'PY' || fail "the seed is decorative or leaks"
import json, sys
first, other = (json.load(open(path)) for path in sys.argv[1:])
assert first["average_effect"] == other["average_effect"], "the closed form moved with the seed"
assert first["one_person"]["effect_rule"] != other["one_person"]["effect_rule"], \
    "changing the seed did not change the simulation"
assert first["declared_rule"] != other["declared_rule"], \
    "changing the seed did not change the declared candidate's simulation"
PY

# ---------------------------------------------------------------------------
# 3. The output says what it is and makes no claim the product cannot.
# ---------------------------------------------------------------------------
"$simulator" > "$work/one.txt"
"$simulator" --scenarios > "$work/table.txt"
for file in "$work/one.txt" "$work/table.txt"; do
  head -1 "$file" | grep -q '^SYNTHETIC design simulation' \
    || fail "$(basename "$file") is not labelled SYNTHETIC on its first line"
  if grep -Eiq '\blearn|\badapt|\bpredict|\bsmarter\b|behaviou?ral model' "$file"; then
    fail "$(basename "$file") uses banned capability copy"
  fi
done
grep -q 'not enabled' "$work/one.txt" || fail "the output does not say v6 is not enabled"
grep -q '>= 900 s left' "$work/one.txt" || fail "the 900-second draw rule is not stated"
# The effect-based rule is never passed off as the declared one.
grep -q 'ReturnLedger::would_withhold' "$work/one.txt" \
  || fail "the output does not name the declared candidate it simulates"
grep -q 'NOT the declared candidate' "$work/one.txt" \
  || fail "the effect-based rule is not labelled as not the declared candidate"
[[ "$(grep -c '^ *[0-9]' "$work/table.txt")" -ge 6 ]] || fail "the scenario table is short"

# ---------------------------------------------------------------------------
# 4. Nonsense inputs are refused, not simulated.
# ---------------------------------------------------------------------------
for bad in "--testers 0" "--censored 1.5" "--baseline 0.9 --effect 0.2" "--rule-level 0.3" \
           "--ledger-censored -0.1" "--own-return-sd 0.6" "--departures-per-block -1"; do
  # shellcheck disable=SC2086
  if "$simulator" $bad > /dev/null 2>&1; then
    fail "accepted: $bad"
  fi
done

echo "simulate_nudge_designs_test.sh: OK"
