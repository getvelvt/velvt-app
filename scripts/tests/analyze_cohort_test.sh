#!/usr/bin/env bash
# Regression test for the cohort instrument.
#
# The defect this pins: `analyze_cohort.py` did not recognise
# `delivery_suppressed_dnd` (migration 0020) or `withheld_demotion`
# (migration 0023), routed both to `malformed`, and DROPPED the rows. On the
# four-row fixture beside this file the script reported a 2-row denominator
# for a 4-row export and two "unknown outcome" complaints. Data loss inside
# the measurement instrument, silently, in the direction that flatters the
# numerator.
#
# The fix is not "count them as delivered" — that would be the opposite
# error. A withheld decision reached no channel, so it can neither be
# returned to nor be wrong. It is partitioned out of every delivered
# denominator and reported in a block of its own.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
analyze="$repo_root/scripts/analyze_cohort.py"
fixture="$repo_root/scripts/tests/fixtures/tester-withheld-regression.csv"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$analyze" ]] || { echo "ERROR: $analyze is not executable" >&2; exit 1; }
[[ -f "$fixture" ]] || { echo "ERROR: missing fixture $fixture" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. The four-row fixture: one returned, one accepted_action, one
#    delivery_suppressed_dnd, one withheld_demotion.
# ---------------------------------------------------------------------------
"$analyze" --json "$fixture" > "$work/result.json"

python3 - "$work/result.json" <<'PY' || fail "four-row fixture assertions"
import json, sys

result = json.load(open(sys.argv[1]))
problems = []

def eq(path, actual, expected):
    if actual != expected:
        problems.append(f"{path}: expected {expected!r}, got {actual!r}")

# Nothing may be dropped. This is the assertion the bug failed.
eq("data_quality.malformed", result["data_quality"]["malformed"], [])

decisions = result["decisions_recorded"]
eq("decisions_recorded.total", decisions["total"], 4)
eq("decisions_recorded.delivered", decisions["delivered"], 2)
eq("decisions_recorded.withheld", decisions["withheld"], 2)

# The withheld rows are recovered but must NOT inflate the delivered
# denominator. 2, not 4.
primary = result["primary_outcome"]
eq("primary_outcome.denominator", primary["denominator"], 2)
eq("primary_outcome.numerator", primary["numerator"], 1)
eq("trust.denominator", result["trust"]["denominator"], 2)

held = result["withheld"]
eq("withheld.total", held["total"], 2)
eq("withheld.by_outcome", held["by_outcome"],
   {"delivery_suppressed_dnd": 1, "withheld_demotion": 1})
eq("withheld.share_of_recorded_decisions", held["share_of_recorded_decisions"], "2/4")

# The delivered distribution stays delivered-only.
eq("outcome_distribution", result["outcome_distribution"],
   {"accepted_action": 1, "returned": 1})

# A withheld row can never be counted as a return, whatever the export's own
# `returned_within_10min` column happens to say.
salience = result["salience_split"]
eq("salience_split.normal.offers", salience["normal"]["offers"], 1)
eq("salience_split.quiet.offers", salience["quiet"]["offers"], 1)

if problems:
    print("\n".join(problems), file=sys.stderr)
    sys.exit(1)
PY

# The human-readable report must name both withheld outcomes, or a reader of
# the terminal output would never learn the rows exist.
report="$("$analyze" "$fixture")"
for needle in "WITHHELD (recorded, never delivered)" \
              "delivery_suppressed_dnd" \
              "withheld_demotion" \
              "OUTCOME DISTRIBUTION (delivered only)"; do
  grep -qF -- "$needle" <<<"$report" || fail "report omits: $needle"
done
grep -qF "returned within 600s: 1/2" <<<"$report" \
  || fail "delivered denominator drifted off 2"

# ---------------------------------------------------------------------------
# 2. A withheld row that lies about itself is still not a return. The
#    exporter can never emit this, but a hand-edited CSV can, and the
#    partition must not depend on the exporter being honest.
# ---------------------------------------------------------------------------
liar="$work/tester-liar.csv"
head -1 "$fixture" > "$liar"
grep 'withheld_demotion' "$fixture" | sed 's/,0,0$/,1,0/' >> "$liar"
"$analyze" --json "$liar" > "$work/liar.json"
python3 - "$work/liar.json" <<'PY' || fail "a withheld row was counted as a return"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["primary_outcome"]["denominator"] == 0, result["primary_outcome"]
assert result["primary_outcome"]["numerator"] == 0, result["primary_outcome"]
assert result["withheld"]["total"] == 1, result["withheld"]
assert result["data_quality"]["malformed"] == [], result["data_quality"]
PY

# ---------------------------------------------------------------------------
# 3. The vocabulary guard still guards. Widening it for two known values must
#    not have opened it to anything.
# ---------------------------------------------------------------------------
unknown="$work/tester-unknown.csv"
head -1 "$fixture" > "$unknown"
head -2 "$fixture" | tail -1 | sed 's/,returned,/,teleported,/' >> "$unknown"
"$analyze" --json "$unknown" > "$work/unknown.json"
python3 - "$work/unknown.json" <<'PY' || fail "an unknown outcome was silently accepted"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["data_quality"]["malformed"], "unknown outcome was not flagged"
assert "teleported" in result["data_quality"]["malformed"][0], result["data_quality"]
assert result["primary_outcome"]["denominator"] == 0, result["primary_outcome"]
PY

# ---------------------------------------------------------------------------
# 4. An export with a header and zero rows is a real result, not a failure.
# ---------------------------------------------------------------------------
empty="$work/tester-silent.csv"
head -1 "$fixture" > "$empty"
"$analyze" --json "$empty" > "$work/empty.json"
python3 - "$work/empty.json" <<'PY' || fail "zero-row export mishandled"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["participants"]["exported_zero_offers"] == ["tester-silent"], result
assert result["decisions_recorded"]["total"] == 0, result
assert result["data_quality"]["malformed"] == [], result
PY

echo "analyze_cohort_test.sh: OK"
