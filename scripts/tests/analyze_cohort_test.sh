#!/usr/bin/env bash
# Regression tests for the cohort analysis harness.
#
# Every section pins a defect of one kind: the instrument stating something the
# data does not support, or the pre-registration declaring a rule the code did
# not apply.
#
#   1. Withheld decisions (migrations 0020, 0023) were routed to `malformed` and
#      DROPPED. They are partitioned out of every delivered denominator instead.
#   2. The 2026-08-09 outcome was retired on 2026-08-21 and the script kept
#      printing it under "PRIMARY OUTCOME". Sustained anchor engagement is the
#      primary outcome; the old figure is labelled retired and descriptive.
#   3. No ratio may be printed without the power verdict beside it.
#   4. The founder-device exclusion was printed in the footer and applied to
#      nothing. It drops the export whole.
#   5. The warm-up exclusion compared planned_duration_seconds with 300, which
#      the schema makes impossible to trigger. Since 2026-09-25 it is 180 s of
#      ELAPSED time: ended_at - started_at - total_paused_seconds.
#   6. Policy v1 to v5 are never pooled (2026-09-25 to 2026-09-27), and only v5
#      is analysed. An intervention takes the policy of its block's
#      offered/withheld_demotion/suppressed_dnd decision; without one it is
#      counted and excluded. Exports without a decision log are excluded whole.
#   7. no_response is split by card_seen_at into seen / unseen / unknown, and
#      unknown is never folded into unseen.
#   8. Every declared block counts, including blocks with no offer; a
#      participant with blocks and zero offers is not a dropped row.
#   9. The 0.1.6 measures: completion by origin, invitation acceptance, the
#      explain-tap rate, and decision-log integrity.
#  11. The 2026-08-17 measure 3, corrections made per participant, was
#      pre-registered and unmeasurable: no export carried it. It is counted from
#      the corrections CSV, as a count, and a missing file is never a zero.
#  12. The primary outcome was reported as NOT COMPUTED because no export
#      carried the ledger it needs. It is computed from the outcomes CSV the
#      exporter works out on the Mac: numerator and denominator on policy v5
#      eligible points, censoring by reason, the 10% and 25% censoring rules,
#      the two secondary outcomes, and every way a label can be missing or
#      wrong, which is reported and never imputed. Every result carries the
#      2026-08-21 label: associational, since every eligible point was offered
#      and there is no silence arm. observer_gap is broken down by cause.
#
# Fixtures are CSVs written the way export_cohort_evidence.sh writes them.
# export_cohort_evidence_test.sh covers the databases behind them, across
# protocol 25, 28 and 30.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
analyze="$repo_root/scripts/analyze_cohort.py"
fixture="$repo_root/scripts/tests/fixtures/tester-withheld-regression.csv"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$analyze" ]] || { echo "ERROR: $analyze is not executable" >&2; exit 1; }
[[ -f "$fixture" ]] || { echo "ERROR: missing fixture $fixture" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

# check JSON_FILE PYTHON_ASSERTIONS: runs the assertions with `r` bound to the
# analysis result.
check() {
  local json="$1" message="$2"
  python3 -c "
import json, sys
r = json.load(open(sys.argv[1]))
$(cat)
" "$json" || fail "$message"
}

# A fixture writer. Reads one participant's spec as JSON on stdin and writes
# that participant's export files into DIR, with the exporter's headers.
cat > "$work/make_export.py" <<'PY'
import csv, json, sys
from pathlib import Path

HEADERS = {
    "offers": "block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention,card_seen_at,card_seen",
    "decisions": "decision_id,occurred_at,block_id,policy_version,anchor_category,switch_count,elapsed_seconds,remaining_seconds,gate_verdict,propensity,anchor_seen_within_600s,outcome_at,block_started_at,block_ended_at,block_total_paused_seconds,block_planned_duration_seconds",
    "blocks": "block_id,origin,phase,started_at,ended_at,total_paused_seconds,planned_duration_seconds",
    "invitations": "invitation_id,offered_at,action_id,policy_version,backoff_policy_version,outcome,outcome_at",
    "explain": "week_start_local_date,taps,delivered_interventions,blocks_declared",
    "corrections": "scope,category,rules,corrections",
    "outcomes": "decision_id,censor_reason,observer_gap_cause,sustained_anchor_900s,departure_free_600s,seconds_to_sustained_return,departure_category",
}
T0 = 1800000000
spec = json.load(sys.stdin)
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
name = out.name
blocks = {b["block_id"]: b for b in spec.get("blocks", [])}

def block_defaults(b):
    b.setdefault("origin", "manual")
    b.setdefault("phase", "completed")
    b.setdefault("total_paused_seconds", 0)
    b.setdefault("planned_duration_seconds", 3000)
    if "ended_at" not in b:
        b["ended_at"] = b["started_at"] + 3000
    return b

for b in blocks.values():
    block_defaults(b)

def write(kind, rows):
    header = HEADERS[kind].split(",")
    with open(out / (name + ("" if kind == "offers" else f"-{kind}") + ".csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(header)
        for row in rows:
            w.writerow(["" if row.get(col) is None else row.get(col) for col in header])

offers = []
for o in spec.get("offers", []):
    b = blocks[o["block_id"]]
    offered = o.get("offered_at", b["started_at"] + 600)
    outcome_at = o.get("outcome_at")
    offers.append(dict(
        block_id=o["block_id"], purpose="deep_work", intensity="medium",
        planned_duration_seconds=b["planned_duration_seconds"], block_phase=b["phase"],
        started_at=b["started_at"], ended_at=b["ended_at"],
        total_paused_seconds=b["total_paused_seconds"], offered_at=offered,
        remaining_seconds_at_offer=0, action_id="protect_next_10",
        anchor_category="FOCUS_WORK", switch_count=3, window_seconds=600,
        salience=o.get("salience", "normal"), outcome=o["outcome"], outcome_at=outcome_at,
        seconds_to_outcome=None if outcome_at is None else outcome_at - offered,
        returned_within_10min=int(o["outcome"] == "returned" and outcome_at is not None and outcome_at - offered <= 600),
        wrong_intervention=int(o["outcome"] in ("was_focused", "wrong_classification")),
        card_seen_at=o.get("card_seen_at"), card_seen=o.get("card_seen", "unknown"),
    ))
write("offers", offers)

if spec.get("decisions") is not None:
    rows = []
    for i, d in enumerate(spec["decisions"]):
        b = blocks.get(d.get("block_id"), {})
        rows.append(dict(
            decision_id=d.get("decision_id", f"{name}-d{i}"),
            occurred_at=d.get("occurred_at", b.get("started_at", T0) + 600),
            # Unless a row says otherwise, it is under the analysed policy.
            block_id=d.get("block_id"), policy_version=d.get("policy_version", 5),
            anchor_category=d.get("anchor_category", "FOCUS_WORK"), switch_count=3,
            elapsed_seconds=600,
            remaining_seconds=600, gate_verdict=d["gate_verdict"],
            propensity=d.get("propensity", "1.0"),
            anchor_seen_within_600s=d.get("anchor_seen_within_600s"), outcome_at=None,
            block_started_at=b.get("started_at"), block_ended_at=b.get("ended_at"),
            block_total_paused_seconds=b.get("total_paused_seconds"),
            block_planned_duration_seconds=b.get("planned_duration_seconds"),
        ))
    write("decisions", rows)
if spec.get("write_blocks", True):
    write("blocks", list(blocks.values()))
if spec.get("invitations") is not None:
    write("invitations", [dict({"invitation_id": f"inv{i}", "action_id": "soft_start_25",
                                "policy_version": 1, "backoff_policy_version": 1}, **inv)
                          for i, inv in enumerate(spec["invitations"])])
if spec.get("explain") is not None:
    write("explain", spec["explain"])
if spec.get("corrections") is not None:
    write("corrections", spec["corrections"])
if spec.get("outcomes") is not None:
    write("outcomes", spec["outcomes"])
meta = {"export_format": "4" if spec.get("outcomes") is not None else "3",
        "exported_at": str(T0 + 7 * 86400), "schema_version": "36",
        "decision_log": "present" if spec.get("decisions") is not None else "absent",
        "invitations": "present" if spec.get("invitations") is not None else "absent",
        "explain_probe": "present" if spec.get("explain") is not None else "absent",
        "corrections": "present" if spec.get("corrections") is not None else "absent",
        "card_seen_recorded_since": str(T0 - 86400), "invitations_enabled": "1"}
if spec.get("outcomes") is not None:
    meta.update({"outcomes": "present", "outcomes_definition_version": "1"})
meta.update(spec.get("meta", {}))
if spec.get("write_meta", True):
    with open(out / f"{name}-meta.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["key", "value"])
        for k, v in meta.items():
            w.writerow([k, v])
PY
make_export() { python3 "$work/make_export.py" "$1"; }

# ===========================================================================
# 1. The four-row fixture: returned, accepted_action, delivery_suppressed_dnd,
#    withheld_demotion. Its companions sit beside it; the analyser finds them
#    from the per-offer CSV alone. Its per-offer CSV predates card_seen_at.
# ===========================================================================
"$analyze" --json "$fixture" > "$work/result.json"
check "$work/result.json" "four-row fixture assertions" <<'PY'
problems = []
def eq(path, actual, expected):
    if actual != expected:
        problems.append(f"{path}: expected {expected!r}, got {actual!r}")

# Nothing may be dropped. This is the assertion the 2026-08-21 bug failed.
eq("data_quality.malformed", r["data_quality"]["malformed"], [])
eq("participants.analysed", r["participants"]["analysed"], 1)

d = r["decisions_recorded"]
eq("decisions_recorded", (d["total"], d["delivered"], d["withheld"]), (4, 2, 2))

# The withheld rows are recovered but must NOT inflate the delivered
# denominator. 2, not 4.
retired = r["retired_return_within_10min"]
eq("retired.denominator", retired["denominator"], 2)
eq("retired.numerator", retired["numerator"], 1)
eq("trust.denominator", r["trust"]["denominator"], 2)

# The retired figure carries its retirement in the machine-readable output too.
if not retired["status"].startswith("DESCRIPTIVE") or "2026-08-21" not in retired["status"]:
    problems.append(f"retired.status: {retired['status']!r}")
# This export predates the outcomes file: the primary outcome is not
# measurable from it, and nothing is approximated in its place.
if "primary_outcome" in r and r["primary_outcome"].get("numerator") is not None:
    problems.append("the primary outcome's numerator was approximated")
eq("primary.eligible_decision_points", r["primary_outcome"]["eligible_decision_points"], 2)
eq("primary.not_measurable", r["primary_outcome"]["not_measurable"],
   {"no outcomes file (an export from before export format 4)": 2})
eq("primary.with_outcome_row", r["primary_outcome"]["with_outcome_row"], 0)

# The power statement is data, not decoration.
power = r["power"]
eq("power.required", power["required_decision_points"], 390)
eq("power.observed", power["observed_eligible_decision_points"], 2)
eq("power.sufficient", power["sufficient"], False)

held = r["withheld"]
eq("withheld.total", held["total"], 2)
eq("withheld.by_outcome", held["by_outcome"],
   {"delivery_suppressed_dnd": 1, "withheld_demotion": 1})
eq("withheld.share", held["share_of_recorded_decisions"], "2/4")
eq("outcome_distribution", r["outcome_distribution"], {"accepted_action": 1, "returned": 1})
eq("salience.normal.offers", r["salience_split"]["normal"]["offers"], 1)
eq("salience.quiet.offers", r["salience_split"]["quiet"]["offers"], 1)

# An export from before card_seen_at is exported says "unknown", never "unseen".
eq("card_seen.delivered", r["card_seen"]["delivered"], {"seen": 0, "unseen": 0, "unknown": 2})

if problems:
    raise SystemExit("\n".join(problems))
PY

report="$("$analyze" "$fixture")"
for needle in "WITHHELD (recorded, never delivered)" \
              "delivery_suppressed_dnd" \
              "withheld_demotion" \
              "OUTCOME DISTRIBUTION (delivered only)" \
              "PRIMARY OUTCOME: sustained anchor engagement (2026-08-21)" \
              "NOT MEASURABLE. NOT MEASURABLE from these exports" \
              "DESCRIPTIVE — RETIRED 2026-08-21, NOT THE PRIMARY OUTCOME" \
              "390 eligible decision" \
              "NO_RESPONSE BY CARD_SEEN_AT" \
              "BLOCKS DECLARED PER PARTICIPANT" \
              "INVITED VERSUS SELF-DECLARED COMPLETION" \
              "INVITATION ACCEPTANCE" \
              "EXPLAIN-TAP RATE" \
              "CORRECTIONS PER PARTICIPANT (2026-08-17 measure 3; a count, not engagement)" \
              "DECISION-LOG INTEGRITY"; do
  grep -qF -- "$needle" <<<"$report" || fail "report omits: $needle"
done

# ===========================================================================
# 2. The heading the 2026-08-21 amendment retired must not come back.
# ===========================================================================
if grep -qF "PRIMARY OUTCOME (pre-registered 2026-08-09)" <<<"$report"; then
  fail "the retired 2026-08-09 outcome is headlined as the primary one again"
fi
grep -qF "NOT COMPUTED HERE" <<<"$report" && fail "the primary outcome still says it is not computed"
grep -qF "returned within 600s: 1/2 = 50.0% — underpowered, see POWER above" <<<"$report" \
  || fail "delivered denominator drifted off 2, or the power marker went missing"

# ===========================================================================
# 3. No percentage leaves this script without the power verdict beside it.
# ===========================================================================
check_power_markers() {
  local line
  while IFS= read -r line; do
    case "$line" in
      *" = "*"%"*)
        grep -qF -- "see POWER above" <<<"$line" || fail "a bare percentage was printed: $line" ;;
    esac
  done <<<"$1"
}
check_power_markers "$report"

# ===========================================================================
# 4. A withheld row that lies about itself is still not a return; an unknown
#    outcome is still caught; a header-only export is a result.
# ===========================================================================
liar_dir="$work/liar/tester-liar"
mkdir -p "$liar_dir"
head -1 "$fixture" > "$liar_dir/tester-liar.csv"
grep 'withheld_demotion' "$fixture" | sed 's/,0,0$/,1,0/' >> "$liar_dir/tester-liar.csv"
cp "$repo_root/scripts/tests/fixtures/tester-withheld-regression-decisions.csv" "$liar_dir/tester-liar-decisions.csv"
"$analyze" --json "$liar_dir" > "$work/liar.json"
check "$work/liar.json" "a withheld row was counted as a return" <<'PY'
assert r["retired_return_within_10min"]["denominator"] == 0, r["retired_return_within_10min"]
assert r["retired_return_within_10min"]["numerator"] == 0, r["retired_return_within_10min"]
assert r["withheld"]["total"] == 1, r["withheld"]
assert r["data_quality"]["malformed"] == [], r["data_quality"]
PY

unknown_dir="$work/unknown/tester-unknown"
mkdir -p "$unknown_dir"
head -1 "$fixture" > "$unknown_dir/tester-unknown.csv"
head -2 "$fixture" | tail -1 | sed 's/,returned,/,teleported,/' >> "$unknown_dir/tester-unknown.csv"
cp "$repo_root/scripts/tests/fixtures/tester-withheld-regression-decisions.csv" "$unknown_dir/tester-unknown-decisions.csv"
"$analyze" --json "$unknown_dir" > "$work/unknown.json"
check "$work/unknown.json" "an unknown outcome was silently accepted" <<'PY'
assert any("teleported" in m for m in r["data_quality"]["malformed"]), r["data_quality"]
assert r["retired_return_within_10min"]["denominator"] == 0, r["retired_return_within_10min"]
PY

# ===========================================================================
# 5. Founder device: dropped whole, by name convention or by flag, and never
#    by a name that merely contains the word.
# ===========================================================================
copy_fixture_as() { # DIR NAME
  mkdir -p "$1"
  local suffix
  for suffix in "" -decisions -blocks -invitations -explain -meta; do
    cp "$repo_root/scripts/tests/fixtures/tester-withheld-regression$suffix.csv" "$1/$2$suffix.csv"
  done
}
copy_fixture_as "$work/f1" founder-mac
"$analyze" --json "$work/f1/founder-mac.csv" > "$work/founder.json"
check "$work/founder.json" "a founder-named export was analysed anyway" <<'PY'
assert r["participants"]["exports_received"] == 0, r["participants"]
assert r["decisions_recorded"]["total"] == 0, r["decisions_recorded"]
assert r["power"]["observed_eligible_decision_points"] == 0, r["power"]
assert r["blocks_per_participant"]["participants_measured"] == 0, r["blocks_per_participant"]
assert r["exclusions"]["founder_devices_excluded"] == ["founder-mac"], r["exclusions"]
PY

copy_fixture_as "$work/f2" tester-is-the-founder
"$analyze" --json --founder-device tester-is-the-founder "$work/f2/tester-is-the-founder.csv" > "$work/named.json"
check "$work/named.json" "--founder-device excluded nothing" <<'PY'
assert r["decisions_recorded"]["delivered"] == 0, r["decisions_recorded"]
assert r["exclusions"]["founder_devices_excluded"] == ["tester-is-the-founder"], r["exclusions"]
PY

copy_fixture_as "$work/f3" refounder-study
"$analyze" --json "$work/f3/refounder-study.csv" > "$work/notfounder.json"
check "$work/notfounder.json" "the founder exclusion matched a name it should not have" <<'PY'
assert r["decisions_recorded"]["delivered"] == 2, r["decisions_recorded"]
assert r["exclusions"]["founder_devices_excluded"] == [], r["exclusions"]
PY

# And the reinstall exclusion, declared 2026-08-09, is applied by name.
"$analyze" --json --reinstalled refounder-study "$work/f3/refounder-study.csv" > "$work/reinstalled.json"
check "$work/reinstalled.json" "--reinstalled excluded nothing" <<'PY'
assert r["participants"]["exports_received"] == 1, r["participants"]
assert r["participants"]["analysed"] == 0, r["participants"]
assert "reinstalled" in r["participants"]["excluded_whole"]["refounder-study"], r["participants"]
assert r["decisions_recorded"]["total"] == 0, r["decisions_recorded"]
PY

# ===========================================================================
# 6. Warm-up: 180 s of ELAPSED time. planned_duration_seconds is at least 300
#    by schema, so the old rule never fired; these blocks prove the new one
#    does, and that it reads pauses, the exact boundary and open blocks right.
# ===========================================================================
make_export "$work/warmup/p-warmup" <<'JSON'
{"blocks": [
   {"block_id": "short",    "started_at": 1800000000, "ended_at": 1800000170, "planned_duration_seconds": 300},
   {"block_id": "paused",   "started_at": 1800010000, "ended_at": 1800010400, "total_paused_seconds": 230, "planned_duration_seconds": 300},
   {"block_id": "boundary", "started_at": 1800020000, "ended_at": 1800020180, "planned_duration_seconds": 300},
   {"block_id": "open",     "started_at": 1800030000, "ended_at": null, "phase": "active"},
   {"block_id": "long",     "started_at": 1800040000}
 ],
 "offers": [
   {"block_id": "short",    "offered_at": 1800000100, "outcome": "no_response"},
   {"block_id": "paused",   "offered_at": 1800010200, "outcome": "no_response"},
   {"block_id": "boundary", "offered_at": 1800020179, "outcome": "returned", "outcome_at": 1800020180},
   {"block_id": "open",     "offered_at": 1800030600, "outcome": "offered"},
   {"block_id": "long",     "outcome": "was_focused", "outcome_at": 1800040700}
 ],
 "decisions": [
   {"block_id": "short", "gate_verdict": "offered", "occurred_at": 1800000100},
   {"block_id": "paused", "gate_verdict": "offered", "occurred_at": 1800010200},
   {"block_id": "boundary", "gate_verdict": "offered", "occurred_at": 1800020179},
   {"block_id": "open", "gate_verdict": "offered", "occurred_at": 1800030600},
   {"block_id": "long", "gate_verdict": "offered"}
 ]}
JSON
"$analyze" --json "$work/warmup/p-warmup" > "$work/warmup.json"
check "$work/warmup.json" "the 180 s elapsed-time warm-up exclusion" <<'PY'
d = r["decisions_recorded"]
# short (170 s) and paused (400 - 230 = 170 s) are excluded; boundary (180 s),
# open (no end yet) and long are kept.
assert d["delivered"] == 3, d
assert r["outcome_distribution"] == {"offered": 1, "returned": 1, "was_focused": 1}, r["outcome_distribution"]
assert r["primary_outcome"]["eligible_decision_points"] == 3, r["primary_outcome"]
reasons = r["exclusions"]["by_reason"]
assert reasons.get("block elapsed under the 180s warm-up") == 4, reasons   # 2 offers + 2 decisions
# The boundary block's decision point is censored: the block ended 1 s after
# it, inside the 900 s horizon. The open block's horizon ended well before the
# export did, so neither it nor the long block's is censored by what the
# export can see.
censored = r["primary_outcome"]["censored_visible_in_export"]
assert censored == {"block ended before the horizon elapsed": 1}, censored
assert r["primary_outcome"]["not_censored_by_block_or_export_end"] == 2, r["primary_outcome"]
assert r["primary_outcome"]["with_outcome_row"] == 0, r["primary_outcome"]
PY

# ===========================================================================
# 7. Policy: attributed through the decision log, never pooled.
# ===========================================================================
make_export "$work/policy/p-policy" <<'JSON'
{"blocks": [
   {"block_id": "v1-offer",      "started_at": 1800000000},
   {"block_id": "v5-offer",      "started_at": 1800100000},
   {"block_id": "no-decision",   "started_at": 1800200000},
   {"block_id": "abstained-only","started_at": 1800300000},
   {"block_id": "conflicting",   "started_at": 1800400000},
   {"block_id": "v5-dnd",        "started_at": 1800500000},
   {"block_id": "v1-era-block",  "started_at": 1800050000, "ended_at": 1800050600, "phase": "abandoned"}
 ],
 "offers": [
   {"block_id": "v1-offer",       "outcome": "returned", "outcome_at": 1800000700},
   {"block_id": "v5-offer",       "outcome": "no_response", "card_seen": "seen", "card_seen_at": 1800100601},
   {"block_id": "no-decision",    "outcome": "no_response"},
   {"block_id": "abstained-only", "outcome": "no_response"},
   {"block_id": "conflicting",    "outcome": "dismissed"},
   {"block_id": "v5-dnd",         "outcome": "delivery_suppressed_dnd"}
 ],
 "decisions": [
   {"block_id": "v1-offer", "gate_verdict": "abstained_warmup", "policy_version": 1, "occurred_at": 1800000100},
   {"block_id": "v1-offer", "gate_verdict": "offered", "policy_version": 1, "occurred_at": 1800060000},
   {"block_id": "v5-offer", "gate_verdict": "abstained_min_switches"},
   {"block_id": "v5-offer", "gate_verdict": "offered", "anchor_seen_within_600s": 1},
   {"block_id": "abstained-only", "gate_verdict": "abstained_min_switches"},
   {"block_id": "conflicting", "gate_verdict": "offered", "policy_version": 1, "occurred_at": 1800000200},
   {"block_id": "conflicting", "gate_verdict": "offered", "policy_version": 5},
   {"block_id": "v5-dnd", "gate_verdict": "suppressed_dnd", "anchor_seen_within_600s": 0}
 ],
 "invitations": [
   {"offered_at": 1800040000, "outcome": "accepted"},
   {"offered_at": 1800150000, "outcome": "dismissed"}
 ],
 "explain": [
   {"week_start_local_date": "2027-01-11", "taps": 5, "delivered_interventions": 1, "blocks_declared": 2},
   {"week_start_local_date": "2027-01-18", "taps": 1, "delivered_interventions": 2, "blocks_declared": 3}
 ]}
JSON
"$analyze" --json "$work/policy/p-policy" > "$work/policy.json"
check "$work/policy.json" "policy attribution and the never-pooled rule" <<'PY'
p = r["policy"]
assert p["analysed_policy_version"] == 5, p
assert p["decisions_by_policy_version"] == {"1": 3, "5": 5}, p
# v1-offer -> 1; v5-offer, v5-dnd -> 5; no-decision and abstained-only have no
# attributing verdict; conflicting has both.
assert p["interventions_by_attribution"] == {"1": 1, "5": 2, "conflicting": 1, "unattributed": 2}, p
d = r["decisions_recorded"]
assert (d["delivered"], d["withheld"]) == (1, 1), d
assert r["retired_return_within_10min"]["numerator"] == 0, r["retired_return_within_10min"]
assert r["card_seen"]["no_response"] == {"seen": 1, "unseen": 0, "unknown": 0}, r["card_seen"]
# Eligible: v5 'offered' rows only (v5-offer, conflicting's v5 row).
assert r["power"]["observed_eligible_decision_points"] == 2, r["power"]
integrity = r["decision_log_integrity"]
assert integrity["rows"] == 5, integrity
assert integrity["by_gate_verdict"] == {"abstained_min_switches": 2, "offered": 2, "suppressed_dnd": 1}, integrity
# Rows with no policy column are v5 only after this Mac's last decision under
# another policy (1800060000): the v1-era block and the first invitation are
# excluded, and the explain week holding that decision is too.
assert p["rows_before_last_other_policy_decision_excluded"] == {"blocks": 2, "explain_weeks": 1, "invitations": 1}, p
assert r["blocks_per_participant"]["per_participant"]["p-policy"]["blocks_declared"] == 5, r["blocks_per_participant"]
inv = r["invitation_acceptance"]
assert (inv["accepted"], inv["terminal"]) == (0, 1), inv
ex = r["explain_tap_rate"]
assert (ex["taps"], ex["delivered_interventions"]) == (1, 2), ex
assert p["non_monotonic_policy_history"] == [], p
PY

# ===========================================================================
# 7b. A Mac upgraded from a policy-v4 build (1.0.13) to a policy-v5 build. v4
#     filed a browser tab by keyword rules over its hostname and title, else
#     the ambiguous prior; v5 files it by its site and drops the keyword tiers
#     for a readable site, so decision points differ in both directions and
#     v4's switch counts mean something else: its rows are counted and
#     excluded, never pooled with v5.
# ===========================================================================
make_export "$work/upgraded/p-v4-to-v5" <<'JSON'
{"blocks": [
   {"block_id": "v4-block", "started_at": 1800000000},
   {"block_id": "v5-block", "started_at": 1800200000}
 ],
 "offers": [
   {"block_id": "v4-block", "outcome": "returned", "outcome_at": 1800000650},
   {"block_id": "v5-block", "outcome": "returned", "outcome_at": 1800200650}
 ],
 "decisions": [
   {"block_id": "v4-block", "gate_verdict": "abstained_min_switches", "policy_version": 4, "occurred_at": 1800000300},
   {"block_id": "v4-block", "gate_verdict": "offered", "policy_version": 4, "occurred_at": 1800000600},
   {"block_id": "v5-block", "gate_verdict": "abstained_min_switches"},
   {"block_id": "v5-block", "gate_verdict": "offered"}
 ]}
JSON
"$analyze" --json "$work/upgraded/p-v4-to-v5" > "$work/upgraded.json"
check "$work/upgraded.json" "policy v4 rows pooled with v5" <<'PY'
p = r["policy"]
assert p["decisions_by_policy_version"] == {"4": 2, "5": 2}, p
assert p["interventions_by_attribution"] == {"4": 1, "5": 1}, p
assert r["power"]["observed_eligible_decision_points"] == 1, r["power"]
assert r["decision_log_integrity"]["rows"] == 2, r["decision_log_integrity"]
assert r["decisions_recorded"]["total"] == 1, r["decisions_recorded"]
assert p["rows_before_last_other_policy_decision_excluded"] == {"blocks": 1}, p
reasons = r["exclusions"]["by_reason"]
assert reasons["intervention under policy_version 4"] == 1, reasons
assert reasons["decision rows not under policy_version 5"] == 2, reasons
PY
upgraded_report="$("$analyze" "$work/upgraded/p-v4-to-v5")"
for needle in "COHORT ANALYSIS (drift policy v5 only)" \
              "every result uses policy_version 5 only" \
              "DECISION-LOG INTEGRITY (policy_version 5 rows)"; do
  grep -qF -- "$needle" <<<"$upgraded_report" || fail "upgraded report omits: $needle"
done

# ===========================================================================
# 8. An export without the decision log cannot be attributed to a policy and
#    is excluded whole, with the reason, whichever way the log went missing.
# ===========================================================================
make_export "$work/nolog/p-absent" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000650}]}
JSON
make_export "$work/nolog/p-lost" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000650}],
 "decisions": [{"block_id": "b", "gate_verdict": "offered"}]}
JSON
rm "$work/nolog/p-lost/p-lost-decisions.csv"
make_export "$work/nolog/p-old" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000650}],
 "write_meta": false, "write_blocks": false}
JSON
"$analyze" --json "$work/nolog/p-absent" "$work/nolog/p-lost" "$work/nolog/p-old" > "$work/nolog.json"
check "$work/nolog.json" "exports without a decision log" <<'PY'
whole = r["participants"]["excluded_whole"]
assert r["participants"]["exports_received"] == 3, r["participants"]
assert r["participants"]["analysed"] == 0, r["participants"]
assert "predates migration 0026" in whole["p-absent"], whole
assert "missing from what was received" in whole["p-lost"], whole
assert "no decision log received" in whole["p-old"], whole
assert any("p-lost" in m for m in r["data_quality"]["malformed"]), r["data_quality"]
assert r["decisions_recorded"]["total"] == 0, r["decisions_recorded"]
PY

# ===========================================================================
# 9. Blocks, completion by origin, invitations, the explain probe and the
#    decision log's integrity, across three participants. p-idle declared
#    blocks and never got an offer; p-none declared nothing.
# ===========================================================================
make_export "$work/cohort/p-busy" <<'JSON'
{"blocks": [
   {"block_id": "a", "started_at": 1800000000, "origin": "manual"},
   {"block_id": "b", "started_at": 1800100000, "origin": "invitation"},
   {"block_id": "c", "started_at": 1800200000, "origin": "invitation", "phase": "abandoned", "ended_at": 1800200060},
   {"block_id": "d", "started_at": 1800700000, "origin": "manual", "phase": "expired"}
 ],
 "offers": [
   {"block_id": "a", "outcome": "no_response", "card_seen": "unseen"},
   {"block_id": "b", "outcome": "no_response", "card_seen": "seen", "card_seen_at": 1800100601},
   {"block_id": "d", "outcome": "no_response", "card_seen": "unknown"}
 ],
 "decisions": [
   {"block_id": "a", "gate_verdict": "offered", "anchor_seen_within_600s": 1},
   {"block_id": "b", "gate_verdict": "offered", "anchor_seen_within_600s": 0},
   {"block_id": "c", "gate_verdict": "abstained_warmup"},
   {"block_id": "d", "gate_verdict": "offered", "propensity": "0.7"}
 ],
 "invitations": [
   {"offered_at": 1800090000, "outcome": "accepted"},
   {"offered_at": 1800190000, "outcome": "accepted"},
   {"offered_at": 1800290000, "outcome": "expired"},
   {"offered_at": 1800390000, "outcome": "offered"}
 ],
 "explain": [
   {"week_start_local_date": "2027-01-11", "taps": 1, "delivered_interventions": 2, "blocks_declared": 3},
   {"week_start_local_date": "2027-01-18", "taps": 0, "delivered_interventions": 1, "blocks_declared": 1}
 ],
 "meta": {"invitations_enabled": "1"}}
JSON
make_export "$work/cohort/p-idle" <<'JSON'
{"blocks": [
   {"block_id": "x", "started_at": 1800000000},
   {"block_id": "y", "started_at": 1800300000, "phase": "active", "ended_at": null}
 ],
 "offers": [],
 "decisions": [{"block_id": "x", "gate_verdict": "abstained_min_switches"}],
 "invitations": [{"offered_at": 1800050000, "outcome": "dismissed"}],
 "explain": [{"week_start_local_date": "2027-01-11", "taps": 0, "delivered_interventions": 0, "blocks_declared": 2}],
 "meta": {"invitations_enabled": "0"}}
JSON
make_export "$work/cohort/p-none" <<'JSON'
{"blocks": [], "offers": [], "decisions": [], "invitations": [], "explain": []}
JSON
"$analyze" --json --cohort-start 2027-01-15 --cohort-weeks 1 "$work/cohort"/*/ > "$work/cohort.json"
check "$work/cohort.json" "the 0.1.6 measures" <<'PY'
assert r["data_quality"]["malformed"] == [], r["data_quality"]
assert r["participants"]["analysed"] == 3, r["participants"]
assert r["participants"]["exported_zero_offers"] == ["p-idle", "p-none"], r["participants"]

b = r["blocks_per_participant"]
per = b["per_participant"]
assert per["p-busy"]["blocks_declared"] == 4, per
assert per["p-idle"]["blocks_declared"] == 2, per     # blocks, zero offers: counted
assert per["p-none"]["blocks_declared"] == 0, per     # nothing declared: a zero, not a gap
assert b["participants_with_zero_blocks"] == ["p-none"], b
# Cohort week 1 is 2027-01-15 00:00 UTC (1799971200) to +7 days (1800576000).
assert per["p-busy"]["by_cohort_week"] == [3], per
assert b["blocks_outside_cohort_window"] == 1, b
assert b["blocks_per_participant_week"] == "5/3", b

c = r["completion_by_origin"]
assert c["by_origin"]["manual"] == {"completed": 2, "terminal": 3, "abandoned": 0, "expired": 1}, c
assert c["by_origin"]["invitation"] == {"completed": 1, "terminal": 2, "abandoned": 1, "expired": 0}, c
assert c["open_at_export_excluded"] == 1, c

inv = r["invitation_acceptance"]
assert (inv["accepted"], inv["terminal"], inv["unresolved_offered_excluded"]) == (2, 4, 1), inv
assert inv["participants_with_invitations_off"] == ["p-idle"], inv
assert inv["participants_measured"] == 3, inv

ex = r["explain_tap_rate"]
assert (ex["taps"], ex["delivered_interventions"]) == (1, 3), ex
assert ex["weekly_active_participant_weeks"] == 3, ex
assert ex["weekly_active_participant_weeks_with_a_tap"] == 1, ex

assert r["card_seen"]["no_response"] == {"seen": 1, "unseen": 1, "unknown": 1}, r["card_seen"]

integrity = r["decision_log_integrity"]
assert integrity["rows"] == 5, integrity
assert integrity["propensity_not_1_0_protocol_deviations"] == 1, integrity
assert integrity["anchor_seen_within_600s"] == {"1": 1, "0": 1, "NULL": 3}, integrity
PY
cohort_report="$("$analyze" --cohort-start 2027-01-15 "$work/cohort"/*/)"
check_power_markers "$cohort_report"
grep -qF "p-none" <<<"$cohort_report" || fail "a participant with no blocks vanished from the report"

# ===========================================================================
# 10. Files passed one by one group into one participant, exactly as a folder
#     does; a companion without its per-offer CSV is reported, not guessed.
# ===========================================================================
"$analyze" --json "$work/cohort/p-busy"/*.csv > "$work/files.json"
"$analyze" --json "$work/cohort/p-busy" > "$work/folder.json"
python3 - "$work/files.json" "$work/folder.json" <<'PY' || fail "files and folder disagree"
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:])
assert a == b, "passing the files gave a different answer than passing the folder"
assert a["participants"]["analysed"] == 1, a["participants"]
PY
mkdir -p "$work/orphan"
cp "$work/cohort/p-busy/p-busy-decisions.csv" "$work/orphan/"
"$analyze" --json "$work/orphan/p-busy-decisions.csv" > "$work/orphan.json"
check "$work/orphan.json" "an orphaned companion file was analysed" <<'PY'
assert r["participants"]["analysed"] == 0, r["participants"]
assert any("no per-offer CSV" in m for m in r["data_quality"]["malformed"]), r["data_quality"]
PY

# Two testers who both kept the default, date-stamped file name collide on one
# participant name. Neither is analysed from a mix of the two.
mkdir -p "$work/clash/a" "$work/clash/b"
copy_fixture_as "$work/clash/a" velvt-cohort-2027-01-22
copy_fixture_as "$work/clash/b" velvt-cohort-2027-01-22
"$analyze" --json "$work/clash/a"/*.csv "$work/clash/b"/*.csv > "$work/clash.json"
check "$work/clash.json" "two exports with one name were merged" <<'PY2'
assert r["participants"]["analysed"] == 0, r["participants"]
assert "more than one export" in r["participants"]["excluded_whole"]["velvt-cohort-2027-01-22"], r["participants"]
assert r["decisions_recorded"]["total"] == 0, r["decisions_recorded"]
PY2

# ===========================================================================
# 11. Corrections made per participant (2026-08-17 measure 3). A count per
#     participant, summed from the app rows and the site rows (0040, taught
#     from 1.0.14); window rules are descriptive; a participant with no
#     corrections is a zero; one whose export has no file is not measurable,
#     never a zero; an export whose database could not hold a site rule says
#     so; a malformed row is reported and not counted; a founder export's
#     corrections go nowhere.
# ===========================================================================
make_export "$work/corr/p-teacher" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [{"block_id": "a", "gate_verdict": "abstained_min_switches"}],
 "corrections": [
   {"scope": "app", "category": "FOCUS_WORK", "rules": 2, "corrections": 5},
   {"scope": "app", "category": "COMMUNICATION", "rules": 1, "corrections": 1},
   {"scope": "site", "category": "REFERENCE", "rules": 2, "corrections": 3},
   {"scope": "window", "category": "REFERENCE", "rules": 3}
 ],
 "meta": {"site_rules": "present"}}
JSON
make_export "$work/corr/p-zero" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [], "corrections": []}
JSON
make_export "$work/corr/p-older" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [], "meta": {"export_format": "2"}}
JSON
python3 - "$work/corr/p-older/p-older-meta.csv" <<'PY'
import csv, sys
rows = [r for r in csv.reader(open(sys.argv[1])) if r[0] != "corrections"]
csv.writer(open(sys.argv[1], "w", newline="")).writerows(rows)
PY
make_export "$work/corr/p-lost" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [],
 "corrections": [{"scope": "app", "category": "SYSTEM", "rules": 1, "corrections": 1}]}
JSON
rm "$work/corr/p-lost/p-lost-corrections.csv"
make_export "$work/corr/p-bad" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [],
 "corrections": [
   {"scope": "app", "category": "Slack", "rules": 1, "corrections": 1},
   {"scope": "app", "category": "SYSTEM", "rules": 2, "corrections": 1},
   {"scope": "app", "category": "REFERENCE", "rules": 1, "corrections": 4},
   {"scope": "app", "category": "REFERENCE", "rules": 1, "corrections": 4},
   {"scope": "site", "category": "REFERENCE", "rules": 1, "corrections": 2},
   {"scope": "site", "category": "SOCIAL_FEED", "rules": 2, "corrections": 1},
   {"scope": "site", "category": "SOCIAL_FEED", "rules": 1},
   {"scope": "window", "category": "SYSTEM", "rules": 0}
 ],
 "meta": {"site_rules": "present"}}
JSON
make_export "$work/corr/p-upgraded" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [],
 "decisions": [{"block_id": "a", "gate_verdict": "abstained_warmup", "policy_version": 1}],
 "corrections": [{"scope": "app", "category": "FOCUS_WORK", "rules": 1, "corrections": 2}]}
JSON
make_export "$work/corr/founder-corr" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [],
 "corrections": [{"scope": "app", "category": "FOCUS_WORK", "rules": 9, "corrections": 99}]}
JSON
"$analyze" --json "$work/corr"/*/ > "$work/corr.json"
check "$work/corr.json" "corrections made per participant" <<'PY'
c = r["corrections_per_participant"]
per = c["per_participant"]
assert sorted(per) == ["p-bad", "p-teacher", "p-upgraded", "p-zero"], sorted(per)
assert per["p-teacher"] == {
    "corrections": 9, "app_scoped_corrections": 6, "site_scoped_corrections": 3,
    "site_rules_recordable": True,
    "applications_with_an_app_rule": 3, "sites_with_a_site_rule": 2, "window_rules": 3,
    "app_scoped_corrections_by_category": {"COMMUNICATION": 1, "FOCUS_WORK": 5},
    "site_scoped_corrections_by_category": {"REFERENCE": 3}}, per["p-teacher"]
assert per["p-zero"]["app_scoped_corrections"] == 0, per["p-zero"]
assert per["p-zero"]["corrections"] == 0, per["p-zero"]
assert c["participants_with_zero_app_scoped_corrections"] == ["p-zero"], c
assert c["participants_with_zero_corrections"] == ["p-zero"], c
# No `site_rules` key in the meta file: an older database or an older
# exporter, so a zero site count there is not a measured zero.
assert c["site_rules_not_recordable"] == ["p-upgraded", "p-zero"], c
# Only the first app REFERENCE row survives; the unknown category, the app row
# with fewer corrections than rules, the duplicate and the empty window row
# are each reported. A site row is validated like an app row: one with fewer
# corrections than rules and one with no counts at all are reported, and a
# site rule in the same category as an app rule is not a duplicate.
assert per["p-bad"]["app_scoped_corrections"] == 4, per["p-bad"]
assert per["p-bad"]["site_scoped_corrections"] == 2, per["p-bad"]
assert per["p-bad"]["window_rules"] == 0, per["p-bad"]
bad = [m for m in r["data_quality"]["malformed"] if m.startswith("p-bad:")]
assert len(bad) == 6, bad
assert sum("scope='site'" in m and "site corrections missing or fewer" in m
           for m in bad) == 2, bad
# A file that never came and a file that never existed are not zeros.
assert c["not_measurable_for"] == ["p-lost", "p-older"], c
assert any("p-lost: the export wrote a corrections file" in m
           for m in r["data_quality"]["malformed"]), r["data_quality"]
assert not any(m.startswith("p-older:") for m in r["data_quality"]["malformed"]), r["data_quality"]
# No timestamp, so counts cannot be cut at a policy upgrade: said, not hidden.
assert c["includes_history_before_analysed_policy"] == ["p-upgraded"], c
assert c["total_app_scoped_corrections"] == 6 + 0 + 4 + 2, c
assert c["app_scoped_corrections_by_category"] == {
    "COMMUNICATION": 1, "FOCUS_WORK": 7, "REFERENCE": 4}, c
assert c["total_site_scoped_corrections"] == 3 + 2, c
assert c["site_scoped_corrections_by_category"] == {"REFERENCE": 5}, c
assert c["total_corrections"] == 12 + 5, c
assert r["exclusions"]["founder_devices_excluded"] == ["founder-corr"], r["exclusions"]
PY
corr_report="$("$analyze" "$work/corr"/*/)"
check_power_markers "$corr_report"
for needle in "p-teacher                   6 app-scoped correction(s) on 3 app(s); 3 window rule(s)" \
              "3 site-scoped correction(s) on 2 site(s); 9 correction(s) in all" \
              "site rules not recordable (database before migration 0040); 0 correction(s) in all" \
              "site-scoped corrections by category: {'REFERENCE': 5}" \
              "site rules not recordable: ['p-upgraded', 'p-zero']" \
              "no corrections file: ['p-lost', 'p-older']" \
              "counts include time before policy v5, not separable: ['p-upgraded']" \
              "Each count is a lower bound."; do
  grep -qF -- "$needle" <<<"$corr_report" || fail "corrections report omits: $needle"
done

# ===========================================================================
# 12. The primary outcome, from the outcomes file (instrument addendum,
#     2026-09-27). Scenario A is reportable: 1 of 10 eligible points censored,
#     not above 10%. B adds a block that ended inside the horizon and a
#     horizon still running at export: 3 of 12, above 10% but not above 25%,
#     so the censored count sits beside every result. C adds one more gap:
#     4 of 13, above 25%, so it is reported as insufficient and as counts.
# ===========================================================================
make_export "$work/outcomes/p-labelled" <<'JSON'
{"blocks": [
   {"block_id": "b1", "started_at": 1800000000}, {"block_id": "b2", "started_at": 1800010000},
   {"block_id": "b3", "started_at": 1800020000}, {"block_id": "b4", "started_at": 1800030000},
   {"block_id": "b5", "started_at": 1800040000}, {"block_id": "b6", "started_at": 1800050000},
   {"block_id": "b7", "started_at": 1800060000}, {"block_id": "b8", "started_at": 1800070000},
   {"block_id": "b9", "started_at": 1800080000}, {"block_id": "b10", "started_at": 1800090000},
   {"block_id": "v4", "started_at": 1799000000}
 ],
 "offers": [],
 "decisions": [
   {"decision_id": "v4-offer", "block_id": "v4", "gate_verdict": "offered", "policy_version": 4},
   {"decision_id": "d1", "block_id": "b1", "gate_verdict": "offered"},
   {"decision_id": "d2", "block_id": "b2", "gate_verdict": "offered"},
   {"decision_id": "d3", "block_id": "b3", "gate_verdict": "offered"},
   {"decision_id": "d4", "block_id": "b4", "gate_verdict": "offered"},
   {"decision_id": "d5", "block_id": "b5", "gate_verdict": "offered"},
   {"decision_id": "d6", "block_id": "b6", "gate_verdict": "offered"},
   {"decision_id": "d7", "block_id": "b7", "gate_verdict": "offered"},
   {"decision_id": "d8", "block_id": "b8", "gate_verdict": "offered"},
   {"decision_id": "d9", "block_id": "b9", "gate_verdict": "offered"},
   {"decision_id": "d10", "block_id": "b10", "gate_verdict": "offered"},
   {"decision_id": "a1", "block_id": "b1", "gate_verdict": "abstained_min_switches", "occurred_at": 1800000300}
 ],
 "outcomes": [
   {"decision_id": "v4-offer", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "seconds_to_sustained_return": 5, "departure_category": "SOCIAL_FEED"},
   {"decision_id": "a1", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "seconds_to_sustained_return": 0},
   {"decision_id": "d1", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "seconds_to_sustained_return": 30, "departure_category": "SOCIAL_FEED"},
   {"decision_id": "d2", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 0, "seconds_to_sustained_return": 120, "departure_category": "COMMUNICATION"},
   {"decision_id": "d3", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "seconds_to_sustained_return": 60, "departure_category": "SOCIAL_FEED"},
   {"decision_id": "d4", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 1, "departure_category": "SOCIAL_FEED"},
   {"decision_id": "d5", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 0, "seconds_to_sustained_return": 450, "departure_category": "REFERENCE"},
   {"decision_id": "d6", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 0, "departure_category": "COMMUNICATION"},
   {"decision_id": "d7", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "seconds_to_sustained_return": 10, "departure_category": "PASSIVE_CONSUMPTION"},
   {"decision_id": "d8", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 1, "departure_category": "SOCIAL_FEED"},
   {"decision_id": "d9", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 0, "seconds_to_sustained_return": 200, "departure_category": "COMMUNICATION"},
   {"decision_id": "d10", "censor_reason": "observer_gap", "observer_gap_cause": "final_dwell", "departure_category": "SOCIAL_FEED"}
 ]}
JSON
"$analyze" --json "$work/outcomes/p-labelled" > "$work/outcomes-a.json"
check "$work/outcomes-a.json" "the primary outcome from the outcomes file (scenario A)" <<'PY'
p = r["primary_outcome"]
assert r["data_quality"]["malformed"] == [], r["data_quality"]
assert p["status"].startswith("COMPUTED"), p["status"]
# The v4 offer and the abstention have outcome rows and are not eligible.
assert p["eligible_decision_points"] == 10, p
assert p["not_measurable"] == {}, p
assert p["with_outcome_row"] == 10, p
assert p["censored"] == {"block_ended": 0, "export_ended": 0, "observer_gap": 1}, p
assert p["observer_gap_by_cause"] == {"pause": 0, "final_dwell": 1, "other": 0}, p
assert "not censoring reasons of their own" in p["observer_gap_by_cause_note"], p
assert "how often that happens is unknown" in p["observer_gap_by_cause_note"], p
# 2026-08-21: associational until randomization is on, in the JSON too.
for needle in ("Associational, not an effect of the nudge", "propensity 1.0", "no silence arm"):
    assert needle in p["associational"], p["associational"]
assert (p["censored_total"], p["censored_share"]) == (1, "1/10"), p
assert p["censoring_verdict"].startswith("at or below 10%"), p["censoring_verdict"]
assert p["reportable"] is True, p
assert (p["numerator"], p["denominator"]) == (5, 9), p
free = p["secondary"]["departure_free_600s"]
assert (free["numerator"], free["denominator"]) == (5, 9), free
ret = p["secondary"]["time_to_sustained_return"]
# Returned: 10, 30, 60, 120, 200, 450; three never did inside the horizon.
assert (ret["points"], ret["reached_within_horizon"], ret["not_reached_within_horizon"]) == (9, 6, 3), ret
assert ret["median_seconds"] == 200, ret           # the 5th of 9 to return
assert ret["restricted_mean_seconds"] == 296.7, ret  # (870 + 3 * 600) / 9
assert p["departure_category_counts"] == {
    "COMMUNICATION": 3, "PASSIVE_CONSUMPTION": 1, "REFERENCE": 1, "SOCIAL_FEED": 5}, p
assert r["power"]["observed_uncensored_decision_points"] == 9, r["power"]
PY
report_a="$("$analyze" "$work/outcomes/p-labelled")"
check_power_markers "$report_a"
for needle in "PRIMARY OUTCOME: sustained anchor engagement (2026-08-21)" \
              "censored: 1/10 (block_ended 0, observer_gap 1, export_ended 0); never counted as 0, never imputed" \
              "observer_gap by cause: final dwell never measured 1, pause (yours, or sleep) 0, other (a restart, or not attributable) 0" \
              "sustained: 5/9 = 55.6% — underpowered, see POWER above; associational, not an effect of the nudge" \
              "secondary, departure-free 600 s: 5/9 = 55.6% — underpowered, see POWER above; associational, not an effect of the nudge" \
              "returned within the horizon at 6 of 9; median 200 s; restricted mean 296.7 s; associational, not an effect of the nudge" \
              "Associational, not an effect of the nudge: every eligible point was" \
              "9 with an uncensored outcome"; do
  grep -qF -- "$needle" <<<"$report_a" || fail "outcomes report (A) omits: $needle"
done
grep -qF "censored 1/10" <<<"$report_a" && fail "censoring at 10% was printed beside the results"
# Every rendered primary-outcome result says it is associational, on its own
# line, so a line quoted alone cannot drop it.
check_associational() {
  local line count=0
  while IFS= read -r line; do
    case "$line" in
      "  sustained: "*|"  secondary, "*|"    sustained at "*|"    departure-free 600 s at "*|"    returned to a sustained run "*)
        count=$((count + 1))
        grep -qF -- "; associational, not an effect of the nudge" <<<"$line" \
          || fail "a primary-outcome result without the associational label: $line" ;;
    esac
  done <<<"$1"
  (( count >= 3 )) || fail "expected at least three primary-outcome results, found $count"
  grep -qF "at propensity 1.0, so there is no silence arm" <<<"$(tr -s ' \n' ' ' <<<"$1")" \
    || fail "the associational statement is missing"
}
check_associational "$report_a"

# B. Two more points, both censored where the decisions file agrees: a block
#    that ended 100 s after its offer, and an open block whose offer came 400 s
#    before the export (exported_at is T0 + 7 days in make_export).
make_export "$work/outcomes/p-short" <<'JSON'
{"blocks": [
   {"block_id": "ended", "started_at": 1800100000, "ended_at": 1800100700},
   {"block_id": "open", "started_at": 1800603800, "ended_at": null, "phase": "active"}
 ],
 "offers": [],
 "decisions": [
   {"decision_id": "e1", "block_id": "ended", "gate_verdict": "offered"},
   {"decision_id": "o1", "block_id": "open", "gate_verdict": "offered", "occurred_at": 1800604400}
 ],
 "outcomes": [
   {"decision_id": "e1", "censor_reason": "block_ended", "departure_category": "SOCIAL_FEED"},
   {"decision_id": "o1", "censor_reason": "export_ended", "departure_category": "COMMUNICATION"}
 ]}
JSON
"$analyze" --json "$work/outcomes/p-labelled" "$work/outcomes/p-short" > "$work/outcomes-b.json"
check "$work/outcomes-b.json" "censoring above 10% (scenario B)" <<'PY'
p = r["primary_outcome"]
assert r["data_quality"]["malformed"] == [], r["data_quality"]
assert p["censored"] == {"block_ended": 1, "export_ended": 1, "observer_gap": 1}, p
assert p["censored_share"] == "3/12", p
assert p["censoring_verdict"].startswith("above 10%"), p["censoring_verdict"]
assert p["reportable"] is True, p
assert (p["numerator"], p["denominator"]) == (5, 9), p
# The decisions file on its own sees the two block/export ends, not the gap.
assert p["censored_visible_in_export"] == {
    "block ended before the horizon elapsed": 1, "the export ends inside the horizon": 1}, p
PY
report_b="$("$analyze" "$work/outcomes/p-labelled" "$work/outcomes/p-short")"
check_power_markers "$report_b"
grep -qF "sustained: 5/9 = 55.6% — underpowered, see POWER above; censored 3/12; associational, not an effect of the nudge" <<<"$report_b" \
  || fail "above 10% censored, the censored count is not beside the result"
check_associational "$report_b"

# C. One more gap: 4 of 13 is above 25%. Counts, and no estimate.
make_export "$work/outcomes/p-gappy" <<'JSON'
{"blocks": [{"block_id": "g", "started_at": 1800200000}],
 "offers": [],
 "decisions": [{"decision_id": "g1", "block_id": "g", "gate_verdict": "offered"}],
 "outcomes": [{"decision_id": "g1", "censor_reason": "observer_gap", "observer_gap_cause": "pause"}]}
JSON
"$analyze" --json "$work/outcomes"/*/ > "$work/outcomes-c.json"
check "$work/outcomes-c.json" "censoring above 25% (scenario C)" <<'PY'
p = r["primary_outcome"]
assert p["censored_share"] == "4/13", p
assert p["observer_gap_by_cause"] == {"pause": 1, "final_dwell": 1, "other": 0}, p
assert p["censoring_verdict"].startswith("above 25%"), p["censoring_verdict"]
assert p["reportable"] is False, p
assert (p["numerator"], p["denominator"]) == (5, 9), p
assert p["departure_category_counts"]["none (at the anchor, or no evidence yet)"] == 1, p
ret = p["secondary"]["time_to_sustained_return"]
assert (ret["reached_within_horizon"], ret["points"]) == (6, 9), ret
assert (ret["median_seconds"], ret["restricted_mean_seconds"]) == (None, None), ret
PY
report_c="$("$analyze" "$work/outcomes"/*/)"
check_power_markers "$report_c"
grep -qF "INSUFFICIENT, not an estimate:" <<<"$report_c" || fail "above 25% censored was not called insufficient"
grep -qF "sustained at 5 of 9 uncensored point(s)" <<<"$report_c" || fail "the insufficient result lost its counts"
check_associational "$report_c"
grep -qE "sustained: .*%|median [0-9]" <<<"$report_c" && fail "an estimate was printed above 25% censored"
grep -qF "returned to a sustained run within the horizon at 6 of 9" <<<"$report_c" \
  || fail "the insufficient time to return lost its counts"

# D. Every way an outcome can be missing or wrong is reported, and the point
#    is not measurable rather than imputed: a malformed row, a second row for
#    one decision, an eligible decision with no row, a row the decisions file
#    contradicts, a category outside the eight, a file under another
#    definition version, and a file the export wrote that never arrived.
make_export "$work/outcomes-bad/p-bad" <<'JSON'
{"blocks": [
   {"block_id": "m1", "started_at": 1800000000}, {"block_id": "m2", "started_at": 1800010000},
   {"block_id": "m3", "started_at": 1800020000}, {"block_id": "m4", "started_at": 1800030000},
   {"block_id": "m5", "started_at": 1800040000, "ended_at": 1800040700},
   {"block_id": "m6", "started_at": 1800050000},
   {"block_id": "m7", "started_at": 1800060000}, {"block_id": "m8", "started_at": 1800070000},
   {"block_id": "m9", "started_at": 1800080000, "ended_at": null, "phase": "active"}
 ],
 "offers": [],
 "decisions": [
   {"decision_id": "labelled-censored", "block_id": "m1", "gate_verdict": "offered"},
   {"decision_id": "twice", "block_id": "m2", "gate_verdict": "offered"},
   {"decision_id": "missing", "block_id": "m3", "gate_verdict": "offered"},
   {"decision_id": "odd-category", "block_id": "m4", "gate_verdict": "offered"},
   {"decision_id": "contradicted", "block_id": "m5", "gate_verdict": "offered"},
   {"decision_id": "fine", "block_id": "m6", "gate_verdict": "offered"},
   {"decision_id": "odd-cause", "block_id": "m7", "gate_verdict": "offered"},
   {"decision_id": "cause-uncensored", "block_id": "m8", "gate_verdict": "offered"},
   {"decision_id": "final-in-open-block", "block_id": "m9", "gate_verdict": "offered"}
 ],
 "outcomes": [
   {"decision_id": "labelled-censored", "censor_reason": "observer_gap", "observer_gap_cause": "other", "sustained_anchor_900s": 0},
   {"decision_id": "twice", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1},
   {"decision_id": "twice", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 1},
   {"decision_id": "odd-category", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1, "departure_category": "Slack"},
   {"decision_id": "contradicted", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1},
   {"decision_id": "fine", "censor_reason": "none", "sustained_anchor_900s": 0, "departure_free_600s": 1, "seconds_to_sustained_return": 601},
   {"decision_id": "odd-cause", "censor_reason": "observer_gap", "observer_gap_cause": "lost_observer"},
   {"decision_id": "cause-uncensored", "censor_reason": "none", "observer_gap_cause": "pause", "sustained_anchor_900s": 1, "departure_free_600s": 1},
   {"decision_id": "final-in-open-block", "censor_reason": "observer_gap", "observer_gap_cause": "final_dwell"}
 ]}
JSON
make_export "$work/outcomes-bad/p-v2" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [], "decisions": [{"decision_id": "x", "block_id": "b", "gate_verdict": "offered"}],
 "outcomes": [{"decision_id": "x", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1}],
 "meta": {"outcomes_definition_version": "2"}}
JSON
make_export "$work/outcomes-bad/p-lost" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [], "decisions": [{"decision_id": "x", "block_id": "b", "gate_verdict": "offered"}],
 "outcomes": [{"decision_id": "x", "censor_reason": "none", "sustained_anchor_900s": 1, "departure_free_600s": 1}]}
JSON
rm "$work/outcomes-bad/p-lost/p-lost-outcomes.csv"
make_export "$work/outcomes-bad/p-old" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [], "decisions": [{"decision_id": "x", "block_id": "b", "gate_verdict": "offered"}]}
JSON
"$analyze" --json "$work/outcomes-bad"/*/ > "$work/outcomes-d.json"
check "$work/outcomes-d.json" "missing and malformed outcome rows" <<'PY'
p = r["primary_outcome"]
malformed = r["data_quality"]["malformed"]
def has(text):
    return any(text in m for m in malformed)
assert has("p-bad: outcome for decision labelled-censored: censored (observer_gap) but labelled"), malformed
assert has("p-bad: outcome for decision twice: a second row for this decision"), malformed
assert has("p-bad: eligible decision missing has no outcome row"), malformed
assert has("p-bad: outcome for decision odd-category: departure_category 'Slack'"), malformed
assert has("p-bad: outcome for decision contradicted: censor_reason 'none', but the block ended at 1800040700"), malformed
assert has("p-bad: outcome for decision fine: seconds_to_sustained_return '601' is not 0 to 600"), malformed
assert has("p-bad: outcome for decision odd-cause: observer_gap_cause 'lost_observer' is not one of pause, final_dwell, other"), malformed
assert has("p-bad: outcome for decision cause-uncensored: observer_gap_cause 'pause' on a point censored as 'none'"), malformed
assert has("p-bad: outcome for decision final-in-open-block: an unmeasured final dwell in a block that had not ended"), malformed
assert has("p-v2: outcomes computed under definition version 2"), malformed
assert has("p-lost: the export wrote a outcomes file and it was not received"), malformed
assert not any(m.startswith("p-old:") for m in malformed), malformed
assert p["not_measurable"] == {
    "no outcomes file (an export from before export format 4)": 1,
    "no row for this decision in the outcomes file": 1,
    "outcome row contradicts the decisions file": 2,
    "outcome row malformed": 6,
    "outcomes computed under definition version 2; this script reads version 1": 1,
    "outcomes file written and not received": 1,
}, p["not_measurable"]
assert p["with_outcome_row"] == 0, p
assert p["numerator"] is None, p
PY

echo "analyze_cohort_test.sh: OK"
