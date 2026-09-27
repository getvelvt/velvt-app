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

# The withhold rule's precision behaves at its extremes: if nobody benefits,
# everyone it flags is right; if everybody benefits by a lot and differs by
# nothing, nobody is truly below the floor and precision is undefined.
nobody = sim.simulate_withhold_rule(
    A(effect=0.0, effect_sd=0.0, blocks_per_week=10, eligible_per_block=3,
      replicates=50), 52)
assert nobody.flagged_share > 0 and nobody.precision == 1.0, nobody
everybody = sim.simulate_withhold_rule(A(effect=0.30, effect_sd=0.0, replicates=50), 26)
assert everybody.truly_below_floor_share == 0 and everybody.recall is None, everybody
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
assert first["one_person"]["withhold_rule"] != other["one_person"]["withhold_rule"], \
    "changing the seed did not change the simulation"
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
[[ "$(grep -c '^ *[0-9]' "$work/table.txt")" -ge 6 ]] || fail "the scenario table is short"

# ---------------------------------------------------------------------------
# 4. Nonsense inputs are refused, not simulated.
# ---------------------------------------------------------------------------
for bad in "--testers 0" "--censored 1.5" "--baseline 0.9 --effect 0.2" "--rule-level 0.3"; do
  # shellcheck disable=SC2086
  if "$simulator" $bad > /dev/null 2>&1; then
    fail "accepted: $bad"
  fi
done

echo "simulate_nudge_designs_test.sh: OK"
