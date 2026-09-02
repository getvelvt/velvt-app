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
#
# Three more defects are pinned here as of 2026-08-31, all of the same kind —
# the instrument stating something the data does not support:
#
#   * The 2026-08-09 outcome was retired by the 2026-08-21 amendment and this
#     script kept printing it under the heading "PRIMARY OUTCOME". Section 1
#     asserts the retired heading is gone and the descriptive one is present.
#     This assertion is the reason the label cannot quietly revert: the string
#     it used to pin was `returned within 600s: 1/2`, which passed green while
#     the heading above it was wrong.
#   * The founder-device exclusion was declared in the pre-registration,
#     printed in this script's own footer, and applied to nothing. Section 5.
#   * A ratio was printable without a power statement beside it. At the sample
#     sizes available that is the difference between a report and a
#     fabrication, per `traction-summary.md`. Section 6.

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

# The retired figure has to carry its own retirement in the machine-readable
# output too, or a consumer reading the JSON reintroduces the headline the
# amendment removed.
if not primary.get("status", "").startswith("DESCRIPTIVE"):
    problems.append(f"primary_outcome.status: expected a DESCRIPTIVE label, "
                    f"got {primary.get('status')!r}")
if "2026-08-21" not in primary.get("status", ""):
    problems.append("primary_outcome.status: does not name the retirement date")

# The power statement is data, not decoration: it is what makes 1/2 readable.
power = result["power"]
eq("power.required_decision_points", power["required_decision_points"], 390)
eq("power.observed_decisions_here", power["observed_decisions_here"], 4)
eq("power.sufficient", power["sufficient"], False)

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
              "OUTCOME DISTRIBUTION (delivered only)" \
              "DESCRIPTIVE — RETIRED 2026-08-21, NOT THE PRIMARY OUTCOME" \
              "PRIMARY OUTCOME (replacement, 2026-08-21) — NOT COMPUTED HERE" \
              "390 eligible decision"; do
  grep -qF -- "$needle" <<<"$report" || fail "report omits: $needle"
done

# The heading the amendment retired must not come back. `01-ACCEPTANCE-CRITERIA`
# D4 blessed it for ten days while every assertion in this file passed.
if grep -qF "PRIMARY OUTCOME (pre-registered 2026-08-09)" <<<"$report"; then
  fail "the retired 2026-08-09 outcome is headlined as the primary one again"
fi

grep -qF "returned within 600s: 1/2 = 50.0% — underpowered, see POWER above" <<<"$report" \
  || fail "delivered denominator drifted off 2, or the power marker went missing"

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

# ---------------------------------------------------------------------------
# 5. The founder-device exclusion was pre-registered before any data existed
#    and this script printed it in its own footer while applying it to
#    nothing. It has to remove the export, not describe removing it.
# ---------------------------------------------------------------------------
founder="$work/founder-mac.csv"
cp "$fixture" "$founder"
"$analyze" --json "$founder" > "$work/founder.json"
python3 - "$work/founder.json" <<'PY' || fail "a founder-named export was analysed anyway"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["participants"]["exports_received"] == 0, result["participants"]
assert result["decisions_recorded"]["total"] == 0, result["decisions_recorded"]
assert result["primary_outcome"]["denominator"] == 0, result["primary_outcome"]
assert result["exclusions"]["founder_devices_excluded"] == ["founder-mac"], \
    result["exclusions"]
PY

# A founder export that does not follow the naming convention has to be
# nameable, or the exclusion only works when someone remembers to rename a file.
named="$work/tester-is-the-founder.csv"
cp "$fixture" "$named"
"$analyze" --json --founder-device tester-is-the-founder "$named" > "$work/named.json"
python3 - "$work/named.json" <<'PY' || fail "--founder-device excluded nothing"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["primary_outcome"]["denominator"] == 0, result["primary_outcome"]
assert result["exclusions"]["founder_devices_excluded"] == ["tester-is-the-founder"], \
    result["exclusions"]
PY

# And it must not fire on a participant whose name merely contains the word.
# An over-broad exclusion silently shrinks the cohort, which is the same class
# of error in the other direction.
notfounder="$work/refounder-study.csv"
cp "$fixture" "$notfounder"
"$analyze" --json "$notfounder" > "$work/notfounder.json"
python3 - "$work/notfounder.json" <<'PY' || fail "the exclusion matched a name it should not have"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["primary_outcome"]["denominator"] == 2, result["primary_outcome"]
assert result["exclusions"]["founder_devices_excluded"] == [], result["exclusions"]
PY

# ---------------------------------------------------------------------------
# 6. No percentage leaves this script without the power verdict beside it.
#    `traction-summary.md`: "Numerator and denominator, with the power
#    statement attached. Never a bare ratio."
# ---------------------------------------------------------------------------
while IFS= read -r line; do
  case "$line" in
    *" = "*"%"*)
      grep -qF -- "underpowered, see POWER above" <<<"$line" \
        || fail "a bare percentage was printed: $line" ;;
  esac
done <<<"$report"

echo "analyze_cohort_test.sh: OK"
