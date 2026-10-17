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
#   6. Policy v1 to v6 are never pooled (2026-09-25 to 2026-10-07), and only v6
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
#      exporter works out on the Mac: numerator and denominator on policy v6
#      eligible points, censoring by reason, the 10% and 25% censoring rules,
#      the two secondary outcomes, and every way a label can be missing or
#      wrong, which is reported and never imputed. Every result carries the
#      2026-08-21 label: associational, since every eligible point was offered
#      and there is no silence arm. observer_gap is broken down by cause.
#  13. Whether a nudge was shown was never recorded (1.0.14 simulation, B7).
#      From protocol 35 the export carries what became of each offer's
#      notification. It is reported as an ADDITIONAL descriptive breakdown and
#      changes no pre-registered denominator; an older export is unknown,
#      never not_reported. A posted notification is not called shown: a Focus
#      Velvt could not detect still holds it.
#  14. Policy 6 holds two anchor rules on develop: builds cut between #78 and
#      #83 took the category with the most confident time, later ones a work
#      category only (2026-10-15). Nothing checked that no build from the gap
#      reached a tester. A policy-6 row whose anchor is not a work category can
#      only come from one; that export is reported and excluded whole.
#  15. An offer made during a Focus Velvt could not see was counted exactly
#      like one made with no Focus (1.0.14 simulation, B5): builds without the
#      Communication Notifications entitlement cannot read Focus status. From
#      protocol 36 each decision says on, off or unknown, and offers are broken
#      down by it as an ADDITIONAL descriptive figure that changes no
#      pre-registered denominator. An older row is not_recorded, never off.
#  16. Every v6 row counted, whatever its date: nothing kept a later
#      instrument, such as the question after a block, out of the pooled v6
#      figures. The v6 primary-period amendment (2026-10-17, pending) counts
#      each Mac's first 14 days only, reports rows after day 14 and rows after
#      the question was turned on as two strata never pooled, counts the
#      protocol deviations, and checks a period start the export states.
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
    "offers": "block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention,card_seen_at,card_seen,delivery_channel,delivery_reported_at,notification_delivery",
    "decisions": "decision_id,occurred_at,block_id,policy_version,anchor_category,switch_count,elapsed_seconds,remaining_seconds,gate_verdict,propensity,anchor_seen_within_600s,outcome_at,focus_state,block_started_at,block_ended_at,block_total_paused_seconds,block_planned_duration_seconds",
    "blocks": "block_id,origin,phase,started_at,ended_at,total_paused_seconds,planned_duration_seconds",
    "invitations": "invitation_id,offered_at,action_id,policy_version,backoff_policy_version,outcome,outcome_at",
    "explain": "week_start_local_date,taps,delivered_interventions,blocks_declared",
    "corrections": "scope,category,rules,corrections",
    "outcomes": "decision_id,censor_reason,observer_gap_cause,sustained_anchor_900s,departure_free_600s,seconds_to_sustained_return,departure_category",
    "ratings": "block_id,prompt_version,eligible_at,shown_at,answered_at,not_now_at,expired_at,card_seen_before_answer,answer",
    "blockfeatures": "block_id,block_feature_version,policy_version,card_branch,elapsed_seconds,covered_seconds,coverage_class,work_anchor_share,switch_aways_per_covered_hour,longest_run_share,recovery_ratio,feed_share,unlogged_share,offer_delivered,offer_outcome,purpose,intensity,planned_minutes,origin,hour_bucket,weekend",
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
        anchor_category=o.get("anchor_category", "FOCUS_WORK"), switch_count=3, window_seconds=600,
        salience=o.get("salience", "normal"), outcome=o["outcome"], outcome_at=outcome_at,
        seconds_to_outcome=None if outcome_at is None else outcome_at - offered,
        returned_within_10min=int(o["outcome"] == "returned" and outcome_at is not None and outcome_at - offered <= 600),
        wrong_intervention=int(o["outcome"] in ("was_focused", "wrong_classification")),
        card_seen_at=o.get("card_seen_at"), card_seen=o.get("card_seen", "unknown"),
        delivery_channel=o.get("delivery_channel"),
        delivery_reported_at=o.get("delivery_reported_at"),
        notification_delivery=o.get("notification_delivery"),
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
            block_id=d.get("block_id"), policy_version=d.get("policy_version", 6),
            anchor_category=d.get("anchor_category", "FOCUS_WORK"), switch_count=3,
            elapsed_seconds=600,
            remaining_seconds=600, gate_verdict=d["gate_verdict"],
            propensity=d.get("propensity", "1.0"),
            anchor_seen_within_600s=d.get("anchor_seen_within_600s"), outcome_at=None,
            focus_state=d.get("focus_state"),
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
# Export format 7: a rating row (unanswered) per listed block, and a
# block-features file, which this script never reads.
if spec.get("rated") is not None:
    write("ratings", [{"block_id": b, "prompt_version": 1,
                       "eligible_at": blocks[b]["ended_at"], "card_seen_before_answer": 0}
                      for b in spec["rated"]])
    write("blockfeatures", [{"block_id": b, "block_feature_version": 1} for b in spec["rated"]])
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
# And one from before delivery_channel says "unknown", never "not_reported".
nd = r["notification_delivery"]
eq("notification_delivery.unknown", nd["by_channel"]["unknown"]["offers"], 2)
eq("notification_delivery.not_recorded", (nd["posted_with_visible_alert_style"], nd["not_posted_with_visible_alert_style"], nd["not_recorded"]), (0, 0, 2))

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
grep -qF "returned within 600s, unanswered offers only: 1/2 = 50.0% — underpowered, see POWER above" <<<"$report" \
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
   {"block_id": "v6-offer",      "started_at": 1800100000},
   {"block_id": "no-decision",   "started_at": 1800200000},
   {"block_id": "abstained-only","started_at": 1800300000},
   {"block_id": "conflicting",   "started_at": 1800400000},
   {"block_id": "v6-dnd",        "started_at": 1800500000},
   {"block_id": "v1-era-block",  "started_at": 1800050000, "ended_at": 1800050600, "phase": "abandoned"}
 ],
 "offers": [
   {"block_id": "v1-offer",       "outcome": "returned", "outcome_at": 1800000700},
   {"block_id": "v6-offer",       "outcome": "no_response", "card_seen": "seen", "card_seen_at": 1800100601},
   {"block_id": "no-decision",    "outcome": "no_response"},
   {"block_id": "abstained-only", "outcome": "no_response"},
   {"block_id": "conflicting",    "outcome": "dismissed"},
   {"block_id": "v6-dnd",         "outcome": "delivery_suppressed_dnd"}
 ],
 "decisions": [
   {"block_id": "v1-offer", "gate_verdict": "abstained_warmup", "policy_version": 1, "occurred_at": 1800000100},
   {"block_id": "v1-offer", "gate_verdict": "offered", "policy_version": 1, "occurred_at": 1800060000},
   {"block_id": "v6-offer", "gate_verdict": "abstained_min_switches"},
   {"block_id": "v6-offer", "gate_verdict": "offered", "anchor_seen_within_600s": 1},
   {"block_id": "abstained-only", "gate_verdict": "abstained_min_switches"},
   {"block_id": "conflicting", "gate_verdict": "offered", "policy_version": 1, "occurred_at": 1800000200},
   {"block_id": "conflicting", "gate_verdict": "offered", "policy_version": 6},
   {"block_id": "v6-dnd", "gate_verdict": "suppressed_dnd", "anchor_seen_within_600s": 0}
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
assert p["analysed_policy_version"] == 6, p
assert p["decisions_by_policy_version"] == {"1": 3, "6": 5}, p
# v1-offer -> 1; v6-offer, v6-dnd -> 6; no-decision and abstained-only have no
# attributing verdict; conflicting has both.
assert p["interventions_by_attribution"] == {"1": 1, "6": 2, "conflicting": 1, "unattributed": 2}, p
d = r["decisions_recorded"]
assert (d["delivered"], d["withheld"]) == (1, 1), d
assert r["retired_return_within_10min"]["numerator"] == 0, r["retired_return_within_10min"]
assert r["card_seen"]["no_response"] == {"seen": 1, "unseen": 0, "unknown": 0}, r["card_seen"]
# Eligible: v6 'offered' rows only (v6-offer, conflicting's v6 row).
assert r["power"]["observed_eligible_decision_points"] == 2, r["power"]
integrity = r["decision_log_integrity"]
assert integrity["rows"] == 5, integrity
assert integrity["by_gate_verdict"] == {"abstained_min_switches": 2, "offered": 2, "suppressed_dnd": 1}, integrity
# Rows with no policy column are v6 only after this Mac's last decision under
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
# 7b. A Mac upgraded from a policy-v5 build to a policy-v6 build. v5 filed
#     every YouTube tab as PASSIVE_CONSUMPTION and TikTok's web app as
#     PASSIVE_CONSUMPTION; v6 decides a YouTube tab by its video title (or
#     not at all) and files TikTok as SOCIAL_FEED, so anchors, switches and
#     decision points differ wherever either was open: v5 rows are counted
#     and excluded, never pooled with v6.
# ===========================================================================
make_export "$work/upgraded/p-v5-to-v6" <<'JSON'
{"blocks": [
   {"block_id": "v5-block", "started_at": 1800000000},
   {"block_id": "v6-block", "started_at": 1800200000}
 ],
 "offers": [
   {"block_id": "v5-block", "outcome": "returned", "outcome_at": 1800000650},
   {"block_id": "v6-block", "outcome": "returned", "outcome_at": 1800200650}
 ],
 "decisions": [
   {"block_id": "v5-block", "gate_verdict": "abstained_min_switches", "policy_version": 5, "occurred_at": 1800000300},
   {"block_id": "v5-block", "gate_verdict": "offered", "policy_version": 5, "occurred_at": 1800000600},
   {"block_id": "v6-block", "gate_verdict": "abstained_min_switches"},
   {"block_id": "v6-block", "gate_verdict": "offered"}
 ]}
JSON
"$analyze" --json "$work/upgraded/p-v5-to-v6" > "$work/upgraded.json"
check "$work/upgraded.json" "policy v5 rows pooled with v6" <<'PY'
p = r["policy"]
assert p["decisions_by_policy_version"] == {"5": 2, "6": 2}, p
assert p["interventions_by_attribution"] == {"5": 1, "6": 1}, p
assert r["power"]["observed_eligible_decision_points"] == 1, r["power"]
assert r["decision_log_integrity"]["rows"] == 2, r["decision_log_integrity"]
assert r["decisions_recorded"]["total"] == 1, r["decisions_recorded"]
assert p["rows_before_last_other_policy_decision_excluded"] == {"blocks": 1}, p
reasons = r["exclusions"]["by_reason"]
assert reasons["intervention under policy_version 5"] == 1, reasons
assert reasons["decision rows not under policy_version 6"] == 2, reasons
PY
upgraded_report="$("$analyze" "$work/upgraded/p-v5-to-v6")"
for needle in "COHORT ANALYSIS (drift policy v6 only)" \
              "every result uses policy_version 6 only" \
              "DECISION-LOG INTEGRITY (policy_version 6 rows)"; do
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
# p-none declared no block, so it has no v6 primary period and counts in no
# figure but blocks declared (period amendment item 1): it is not measured
# here, and is listed once, under the period.
assert inv["participants_measured"] == 2, inv
assert r["v6_primary_period"]["participants_without_a_period"] == ["p-none"], r["v6_primary_period"]

# Both Macs' periods start on Friday 2027-01-15 08:00 UTC, the first block.
# The week of Monday 2027-01-11, widened by a day at each end, holds that
# start, so it straddles and counts in no figure (item 2); the week of
# 2027-01-18 lies wholly inside the period.
ex = r["explain_tap_rate"]
assert (ex["taps"], ex["delivered_interventions"]) == (0, 1), ex
assert ex["weekly_active_participant_weeks"] == 1, ex
assert ex["weekly_active_participant_weeks_with_a_tap"] == 0, ex
assert r["v6_primary_period"]["rows_by_placement"]["explain_weeks"] == {"in_period": 1, "straddles": 2}, r["v6_primary_period"]

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
# A block under policy 1, then one under policy 6 after the upgrade: the
# second starts the v6 primary period. A Mac whose only block came before the
# upgrade has no period, and its counts would go in no figure (item 1).
make_export "$work/corr/p-upgraded" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}, {"block_id": "b", "started_at": 1800100000}],
 "offers": [],
 "decisions": [{"block_id": "a", "gate_verdict": "abstained_warmup", "policy_version": 1},
               {"block_id": "b", "gate_verdict": "abstained_warmup"}],
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
              "counts include time before policy v6, not separable: ['p-upgraded']" \
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

# ===========================================================================
# 13. What became of each nudge's notification (protocol 35, migration 0043).
#     A descriptive breakdown beside the pre-registered figures, which it
#     leaves exactly as they were: the same export with the delivery columns
#     emptied analyses to the same result everywhere else.
# ===========================================================================
delivery_spec='{"blocks": [
   {"block_id": "posted",    "started_at": 1800000000},
   {"block_id": "no-style",  "started_at": 1800010000},
   {"block_id": "denied",    "started_at": 1800020000},
   {"block_id": "quiet",     "started_at": 1800030000},
   {"block_id": "withdrawn", "started_at": 1800040000},
   {"block_id": "stale",     "started_at": 1800045000},
   {"block_id": "silent",    "started_at": 1800050000},
   {"block_id": "raw-only",  "started_at": 1800060000},
   {"block_id": "held",      "started_at": 1800070000}
 ],
 "offers": [
   {"block_id": "posted",    "outcome": "returned", "outcome_at": 1800000700, "delivery_channel": "notification_posted", "delivery_reported_at": 1800000601, "notification_delivery": "notification_posted"},
   {"block_id": "no-style",  "outcome": "no_response", "delivery_channel": "alert_style_none", "notification_delivery": "alert_style_none"},
   {"block_id": "denied",    "outcome": "returned", "outcome_at": 1800020700, "delivery_channel": "permission_denied", "notification_delivery": "permission_denied"},
   {"block_id": "quiet",     "outcome": "no_response", "salience": "quiet", "delivery_channel": "quiet", "notification_delivery": "quiet"},
   {"block_id": "withdrawn", "outcome": "returned", "outcome_at": 1800040601, "delivery_channel": "withdrawn", "notification_delivery": "withdrawn"},
   {"block_id": "stale",     "outcome": "no_response", "delivery_channel": "stale", "notification_delivery": "stale"},
   {"block_id": "silent",    "outcome": "dismissed", "notification_delivery": "not_reported"},
   {"block_id": "raw-only",  "outcome": "was_focused", "delivery_channel": "permission_unknown"},
   {"block_id": "held",      "outcome": "delivery_suppressed_dnd", "notification_delivery": "not_reported"}
 ],
 "decisions": [
   {"block_id": "posted",    "gate_verdict": "offered"},
   {"block_id": "no-style",  "gate_verdict": "offered"},
   {"block_id": "denied",    "gate_verdict": "offered"},
   {"block_id": "quiet",     "gate_verdict": "offered"},
   {"block_id": "withdrawn", "gate_verdict": "offered"},
   {"block_id": "stale",     "gate_verdict": "offered"},
   {"block_id": "silent",    "gate_verdict": "offered"},
   {"block_id": "raw-only",  "gate_verdict": "offered"},
   {"block_id": "held",      "gate_verdict": "suppressed_dnd"}
 ],
 "meta": {"export_format": "5", "delivery_channel_recorded_since": "1799900000"}}'
make_export "$work/delivery/p-delivery" <<<"$delivery_spec"
python3 - "$delivery_spec" > "$work/no-delivery-spec.json" <<'PY'
import json, sys
spec = json.loads(sys.argv[1])
for offer in spec["offers"]:
    for key in ("delivery_channel", "delivery_reported_at", "notification_delivery"):
        offer.pop(key, None)
print(json.dumps(spec))
PY
make_export "$work/no-delivery/p-delivery" < "$work/no-delivery-spec.json"
"$analyze" --json "$work/delivery/p-delivery" > "$work/delivery.json"
"$analyze" --json "$work/no-delivery/p-delivery" > "$work/no-delivery.json"
check "$work/delivery.json" "the notification-delivery breakdown" <<'PY'
nd = r["notification_delivery"]
assert nd["status"].startswith("DESCRIPTIVE"), nd["status"]
by = nd["by_channel"]
offers = {k: v["offers"] for k, v in by.items() if v["offers"]}
# Eight delivered offers; the Focus-held decision stays out, as it does of
# every delivered figure. raw-only has the channel but not the derived column.
assert offers == {"notification_posted": 1, "alert_style_none": 1, "permission_denied": 1,
                  "permission_unknown": 1, "quiet": 1, "withdrawn": 1, "stale": 1,
                  "not_reported": 1}, offers
assert by["notification_posted"]["returned_within_10min"] == 1, by
assert by["permission_denied"]["returned_within_10min"] == 1, by
assert by["alert_style_none"]["no_response"] == 1, by
assert (nd["posted_with_visible_alert_style"], nd["not_posted_with_visible_alert_style"],
        nd["not_recorded"], nd["delivered"]) == (1, 6, 1, 8), nd
# Posted is not seen: the definition says a Focus Velvt could not detect still
# holds a posted notification, so the figure is an upper bound.
assert "Focus" in nd["definition"] and "upper bound" in nd["definition"], nd["definition"]
assert "shown_on_screen" not in nd, nd
assert r["decisions_recorded"]["delivered"] == 8, r["decisions_recorded"]
PY
python3 - "$work/delivery.json" "$work/no-delivery.json" <<'PY' || fail "the delivery breakdown changed a pre-registered figure"
import json, sys
with_delivery, without = (json.load(open(path)) for path in sys.argv[1:3])
# The focus breakdown (15) is descriptive too, and counts posted offers in it.
for result in (with_delivery, without):
    result.pop("notification_delivery")
    result.pop("focus_state_at_decision")
assert with_delivery == without, [k for k in with_delivery if with_delivery[k] != without.get(k)]
PY
delivery_report="$("$analyze" "$work/delivery/p-delivery")"
for needle in "NOTIFICATION DELIVERY (descriptive, protocol 35)" \
              "Changes no" \
              "posted with a visible alert style: 1, not: 6, not recorded: 1, of delivered=8" \
              "Posted is not seen: a Focus mode Velvt could not detect holds a posted"; do
  grep -qF -- "$needle" <<<"$delivery_report" || fail "delivery report omits: $needle"
done

# ===========================================================================
# 14. A policy-6 anchor is a work category (2026-10-15, velvt-app #83). A v6
#     build cut before that rule wrote the same policy number with the
#     most-time anchor, and its rows show it only where that anchor was
#     PASSIVE_CONSUMPTION or SOCIAL_FEED, on a decision row or on the offer a
#     decision made. Such an export is malformed and excluded whole: its rows
#     with a work anchor cannot be told apart from a later build's. Rows under
#     policy 5 keep whatever anchor they recorded.
# ===========================================================================
make_export "$work/anchor-rule/p-gap-decision" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000700}],
 "decisions": [
   {"block_id": "b", "gate_verdict": "abstained_min_switches", "anchor_category": "PASSIVE_CONSUMPTION", "occurred_at": 1800000300},
   {"block_id": "b", "gate_verdict": "offered"}
 ]}
JSON
make_export "$work/anchor-rule/p-gap-offer" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000700, "anchor_category": "SOCIAL_FEED"}],
 "decisions": [{"block_id": "b", "gate_verdict": "offered"}]}
JSON
make_export "$work/anchor-rule/p-v5-drift-anchor" <<'JSON'
{"blocks": [
   {"block_id": "v5", "started_at": 1800000000},
   {"block_id": "v6", "started_at": 1800200000}
 ],
 "offers": [
   {"block_id": "v5", "outcome": "returned", "outcome_at": 1800000700, "anchor_category": "PASSIVE_CONSUMPTION"},
   {"block_id": "v6", "outcome": "returned", "outcome_at": 1800200700}
 ],
 "decisions": [
   {"block_id": "v5", "gate_verdict": "offered", "policy_version": 5, "anchor_category": "PASSIVE_CONSUMPTION", "occurred_at": 1800000600},
   {"block_id": "v6", "gate_verdict": "offered"}
 ]}
JSON
make_export "$work/anchor-rule/p-work-anchors" <<'JSON'
{"blocks": [
   {"block_id": "focus",     "started_at": 1800000000},
   {"block_id": "reference", "started_at": 1800010000},
   {"block_id": "tasks",     "started_at": 1800020000},
   {"block_id": "comms",     "started_at": 1800030000},
   {"block_id": "no-work",   "started_at": 1800040000}
 ],
 "offers": [
   {"block_id": "focus",     "outcome": "returned", "outcome_at": 1800000700},
   {"block_id": "reference", "outcome": "no_response", "anchor_category": "REFERENCE"},
   {"block_id": "tasks",     "outcome": "dismissed", "anchor_category": "TASK_MANAGEMENT"},
   {"block_id": "comms",     "outcome": "was_focused", "anchor_category": "communication"}
 ],
 "decisions": [
   {"block_id": "focus",     "gate_verdict": "offered"},
   {"block_id": "reference", "gate_verdict": "offered", "anchor_category": "REFERENCE"},
   {"block_id": "tasks",     "gate_verdict": "offered", "anchor_category": "TASK_MANAGEMENT"},
   {"block_id": "comms",     "gate_verdict": "offered", "anchor_category": "communication"},
   {"block_id": "no-work",   "gate_verdict": "abstained_no_anchor", "anchor_category": ""}
 ]}
JSON
"$analyze" --json "$work/anchor-rule/p-gap-decision" "$work/anchor-rule/p-gap-offer" \
  "$work/anchor-rule/p-v5-drift-anchor" "$work/anchor-rule/p-work-anchors" > "$work/anchor-rule.json"
check "$work/anchor-rule.json" "a policy-6 row with a non-work anchor" <<'PY'
parts = r["participants"]
assert parts["exports_received"] == 4, parts
assert parts["analysed"] == 2, parts
whole = parts["excluded_whole"]
assert sorted(whole) == ["p-gap-decision", "p-gap-offer"], whole
for name in ("p-gap-decision", "p-gap-offer"):
    assert "non-work anchor" in whole[name] and "2026-10-15 anchor rule" in whole[name], whole
malformed = r["data_quality"]["malformed"]
assert any(m.startswith("p-gap-decision:") and "'PASSIVE_CONSUMPTION'" in m for m in malformed), malformed
assert any(m.startswith("p-gap-offer:") and "'SOCIAL_FEED'" in m for m in malformed), malformed
assert not any(m.startswith(("p-v5-drift-anchor:", "p-work-anchors:")) for m in malformed), malformed
# One eligible v6 point from the upgraded Mac and four from the work anchors;
# nothing from the two excluded exports.
assert r["power"]["observed_eligible_decision_points"] == 5, r["power"]
assert r["policy"]["decisions_by_policy_version"] == {"5": 1, "6": 6}, r["policy"]
assert r["decisions_recorded"]["total"] == 5, r["decisions_recorded"]
PY
anchor_report="$("$analyze" "$work/anchor-rule/p-gap-decision")"
for needle in "p-gap-decision" "non-work anchor"; do
  grep -qF -- "$needle" <<<"$anchor_report" || fail "anchor-rule report omits: $needle"
done

# ===========================================================================
# 15. What Velvt knew about Focus when each offer was decided (protocol 36,
#     migration 0044). A descriptive breakdown beside the pre-registered
#     figures, which it leaves exactly as they were.
# ===========================================================================
focus_spec='{"blocks": [
   {"block_id": "on-held",        "started_at": 1800000000},
   {"block_id": "off-posted",     "started_at": 1800010000},
   {"block_id": "unknown-posted", "started_at": 1800020000},
   {"block_id": "unknown-denied", "started_at": 1800030000},
   {"block_id": "unknown-demoted","started_at": 1800040000},
   {"block_id": "before-0044",    "started_at": 1800050000}
 ],
 "offers": [
   {"block_id": "on-held",        "outcome": "delivery_suppressed_dnd", "notification_delivery": "not_reported"},
   {"block_id": "off-posted",     "outcome": "returned", "outcome_at": 1800010700, "delivery_channel": "notification_posted", "notification_delivery": "notification_posted"},
   {"block_id": "unknown-posted", "outcome": "no_response", "delivery_channel": "notification_posted", "notification_delivery": "notification_posted"},
   {"block_id": "unknown-denied", "outcome": "dismissed", "delivery_channel": "permission_denied", "notification_delivery": "permission_denied"},
   {"block_id": "unknown-demoted","outcome": "withheld_demotion", "notification_delivery": "not_reported"},
   {"block_id": "before-0044",    "outcome": "returned", "outcome_at": 1800050700, "notification_delivery": "unknown"}
 ],
 "decisions": [
   {"block_id": "on-held",        "gate_verdict": "abstained_min_switches", "focus_state": "on", "occurred_at": 1800000300},
   {"block_id": "on-held",        "gate_verdict": "suppressed_dnd", "focus_state": "on"},
   {"block_id": "off-posted",     "gate_verdict": "offered", "focus_state": "off"},
   {"block_id": "unknown-posted", "gate_verdict": "offered", "focus_state": "unknown"},
   {"block_id": "unknown-denied", "gate_verdict": "offered", "focus_state": "unknown"},
   {"block_id": "unknown-demoted","gate_verdict": "withheld_demotion", "focus_state": "unknown"},
   {"block_id": "before-0044",    "gate_verdict": "offered"}
 ],
 "meta": {"export_format": "6", "delivery_channel_recorded_since": "1799900000",
          "focus_state_recorded_since": "1800005000"}}'
make_export "$work/focus/p-focus" <<<"$focus_spec"
python3 - "$focus_spec" > "$work/no-focus-spec.json" <<'PY'
import json, sys
spec = json.loads(sys.argv[1])
for decision in spec["decisions"]:
    decision.pop("focus_state", None)
print(json.dumps(spec))
PY
make_export "$work/no-focus/p-focus" < "$work/no-focus-spec.json"
"$analyze" --json "$work/focus/p-focus" > "$work/focus.json"
"$analyze" --json "$work/no-focus/p-focus" > "$work/no-focus.json"
check "$work/focus.json" "the focus-state breakdown" <<'PY'
assert r["data_quality"]["malformed"] == [], r["data_quality"]
fs = r["focus_state_at_decision"]
assert fs["status"].startswith("DESCRIPTIVE"), fs["status"]
assert "Communication Notifications" in fs["definition"], fs["definition"]
assert "Never folded into off" in fs["definition"], fs["definition"]
by = fs["by_focus_state"]
summary = {k: (v["offers"], v["posted_with_visible_alert_style"], v["returned_within_10min"],
               v["no_response"], v["withheld"]) for k, v in by.items()}
assert summary == {
    "on": (0, 0, 0, 0, {"delivery_suppressed_dnd": 1}),
    "off": (1, 1, 1, 0, {}),
    "unknown": (2, 1, 0, 1, {"withheld_demotion": 1}),
    "not_recorded": (1, 0, 1, 0, {}),
}, summary
assert fs["delivered"] == 4, fs
assert r["decisions_recorded"]["delivered"] == 4, r["decisions_recorded"]
PY
check "$work/no-focus.json" "an export without focus_state" <<'PY'
by = r["focus_state_at_decision"]["by_focus_state"]
assert by["not_recorded"]["offers"] == 4, by
assert by["off"]["offers"] == 0 and by["unknown"]["offers"] == 0, by
PY
python3 - "$work/focus.json" "$work/no-focus.json" <<'PY' || fail "the focus breakdown changed a pre-registered figure"
import json, sys
with_focus, without = (json.load(open(path)) for path in sys.argv[1:3])
for result in (with_focus, without):
    result.pop("focus_state_at_decision")
assert with_focus == without, [k for k in with_focus if with_focus[k] != without.get(k)]
PY
focus_report="$("$analyze" "$work/focus/p-focus")"
for needle in "OFFERS BY FOCUS STATE AT DECISION (descriptive, protocol 36)" \
              "unknown: Velvt could not read Focus status, so a" \
              "  unknown      delivered=   2 posted_visible=   1 returned_unanswered=   0 no_response=   1 withheld=   1" \
              "  not_recorded delivered=   1"; do
  grep -qF -- "$needle" <<<"$focus_report" || fail "focus report omits: $needle"
done

# A value outside the closed set is reported, and its offer is not_recorded
# rather than guessed into a bucket.
make_export "$work/focus-bad/p-focus-bad" <<'JSON'
{"blocks": [{"block_id": "b", "started_at": 1800000000}],
 "offers": [{"block_id": "b", "outcome": "returned", "outcome_at": 1800000700}],
 "decisions": [{"block_id": "b", "gate_verdict": "offered", "focus_state": "dnd"}]}
JSON
"$analyze" --json "$work/focus-bad/p-focus-bad" > "$work/focus-bad.json"
check "$work/focus-bad.json" "a focus_state outside the closed set" <<'PY'
malformed = r["data_quality"]["malformed"]
assert any("focus_state='dnd'" in m for m in malformed), malformed
by = r["focus_state_at_decision"]["by_focus_state"]
assert by["not_recorded"]["offers"] == 1, by
# The decision itself still counts: focus_state is descriptive.
assert r["decisions_recorded"]["delivered"] == 1, r["decisions_recorded"]
PY

# ===========================================================================
# 16. The v6 primary period: each Mac's first 14 days (amendment items 1-6).
#
#     p-long upgraded from a policy-5 build, then used Velvt for 16 days. Its
#     period starts at its first block after the last policy-5 decision,
#     Saturday 2027-01-16 11:46:40 UTC (S), and ends at S + 1,209,600 s (E),
#     Saturday 2027-01-30 11:46:40. A block counts by its start: "day13"
#     starts on day 13 and counts whole, "edge-in" one second before E counts,
#     "edge-out" at E does not.
# ===========================================================================
make_export "$work/period/p-long" <<'JSON'
{"blocks": [
   {"block_id": "old",      "started_at": 1800000000},
   {"block_id": "first",    "started_at": 1800100000},
   {"block_id": "day13",    "started_at": 1801223200},
   {"block_id": "edge-in",  "started_at": 1801309599},
   {"block_id": "edge-out", "started_at": 1801309600},
   {"block_id": "late",     "started_at": 1801396000}
 ],
 "offers": [
   {"block_id": "day13",    "outcome": "returned", "outcome_at": 1801223900},
   {"block_id": "edge-in",  "outcome": "was_focused", "outcome_at": 1801310300},
   {"block_id": "edge-out", "outcome": "no_response"}
 ],
 "decisions": [
   {"block_id": "old",      "gate_verdict": "abstained_min_switches", "policy_version": 5, "occurred_at": 1800000600},
   {"block_id": "first",    "gate_verdict": "abstained_min_switches"},
   {"block_id": "day13",    "gate_verdict": "offered"},
   {"block_id": "edge-in",  "gate_verdict": "offered"},
   {"block_id": "edge-out", "gate_verdict": "offered"},
   {"block_id": "late",     "gate_verdict": "abstained_min_switches"}
 ],
 "invitations": [
   {"offered_at": 1800050000, "outcome": "dismissed"},
   {"offered_at": 1800186400, "outcome": "accepted"},
   {"offered_at": 1801313200, "outcome": "dismissed"}
 ],
 "explain": [
   {"week_start_local_date": "2027-01-11", "taps": 1, "delivered_interventions": 0, "blocks_declared": 1},
   {"week_start_local_date": "2027-01-18", "taps": 0, "delivered_interventions": 0, "blocks_declared": 0},
   {"week_start_local_date": "2027-01-25", "taps": 2, "delivered_interventions": 2, "blocks_declared": 2},
   {"week_start_local_date": "2027-02-01", "taps": 1, "delivered_interventions": 1, "blocks_declared": 1}
 ]}
JSON
"$analyze" --json --cohort-start 2027-01-16 --cohort-weeks 3 "$work/period/p-long" > "$work/period.json"
check "$work/period.json" "the v6 primary period" <<'PY'
S, E = 1800100000, 1800100000 + 1209600
period = r["v6_primary_period"]
entry = period["per_participant"]["p-long"]
assert (entry["start"], entry["end"], entry["upgrade_cut"]) == (S, E, 1800000600), entry
placed = period["rows_by_placement"]
# "old" is before the upgrade cut, excluded as it always was, and not placed.
assert placed["blocks"] == {"in_period": 3, "after_day_14": 2}, placed
assert placed["decisions"] == {"in_period": 3, "after_day_14": 2}, placed
assert placed["offers"] == {"in_period": 2, "after_day_14": 1}, placed
# The first invitation is after the cut and before the period's start: in no
# figure, counted.
assert placed["invitations"] == {"before_period": 1, "in_period": 1, "after_day_14": 1}, placed
# 01-11 holds the upgrade, so the existing rule excludes it before any
# placement; 01-25 holds E, widened by a day; 01-18 is inside, 02-01 after.
assert placed["explain_weeks"] == {"in_period": 1, "after_day_14": 1, "straddles": 1}, placed
assert r["policy"]["rows_before_last_other_policy_decision_excluded"] == {
    "blocks": 1, "explain_weeks": 1}, r["policy"]
assert period["protocol_deviations"]["total"] == 0, period

# Every figure is in-period only, the power count too (items 2 and 3).
assert r["power"]["observed_eligible_decision_points"] == 2, r["power"]
assert r["primary_outcome"]["eligible_decision_points"] == 2, r["primary_outcome"]
d = r["decisions_recorded"]
assert (d["delivered"], d["withheld"]) == (2, 0), d
assert (r["trust"]["was_focused"], r["trust"]["denominator"]) == (1, 2), r["trust"]
assert r["decision_log_integrity"]["rows"] == 3, r["decision_log_integrity"]
inv = r["invitation_acceptance"]
assert (inv["accepted"], inv["terminal"]) == (1, 1), inv
ex = r["explain_tap_rate"]
assert (ex["taps"], len(ex["participant_weeks"])) == (0, 1), ex

# Gate D measure 1 (item 6): in-period blocks only, the rest beside it, and
# the denominator unchanged: three weeks for one participant.
b = r["blocks_per_participant"]
assert b["per_participant"]["p-long"]["blocks_declared"] == 3, b
assert b["per_participant"]["p-long"]["blocks_outside_their_v6_period"] == 2, b
assert b["blocks_outside_their_v6_period"] == 2, b
assert b["blocks_per_participant_week"] == "3/3", b

# After day 14 (item 4): the same definitions, never pooled.
late = r["strata"]["after_day_14"]
assert late["blocks_declared"] == {"p-long": 2}, late
assert late["eligible_decision_points"] == 1, late
assert late["decisions_recorded"] == {"delivered": 1, "withheld": 0}, late
assert late["decision_log_rows"] == 2, late
assert late["invitations"]["terminal"] == 1 and late["explain"]["taps"] == 1, late
assert r["strata"]["question_on"]["blocks_declared"] == {}, r["strata"]
reasons = r["exclusions"]["by_reason"]
assert reasons["blocks after day 14 (the 'after day 14' stratum)"] == 2, reasons
assert reasons["invitations after the upgrade cut and before the period's start (in no figure)"] == 1, reasons
PY
period_report="$("$analyze" --cohort-start 2027-01-16 --cohort-weeks 3 "$work/period/p-long")"
check_power_markers "$period_report"
for needle in "V6 PRIMARY PERIOD (each Mac's first 14 days)" \
              "p-long                   2027-01-16 11:46 UTC to 2027-01-30 11:46 UTC; question first on: never" \
              "blocks           in_period 3, after_day_14 2" \
              "explain weeks    in_period 1, after_day_14 1, straddles 1" \
              "protocol deviations (item 5): 0" \
              "blocks outside their Mac's v6 period, not counted: 2" \
              "AFTER DAY 14 (descriptive stratum, never pooled)" \
              "QUESTION ON (descriptive stratum, never pooled)" \
              "sustained anchor 900 s: 0 of 0 uncensored point(s)"; do
  grep -qF -- "$needle" <<<"$period_report" || fail "period report omits: $needle"
done

# ===========================================================================
# 16b. The question turned on inside the period (item 5). Velvt refuses that,
#      so only a failed day-14 floor, or a history cleared after it was
#      turned on, can produce it. Every row at or after it is the "question
#      on" stratum, whether inside the period or after it; one inside the
#      period is a protocol deviation, and so is any block inside the period
#      with a rating row. p-early's period runs from Friday 2027-01-15 08:00
#      UTC to 2027-01-29 08:00; the question was first on Wednesday
#      2027-01-20 08:00. Its format 7 files sit in the same folder, and the
#      export is still one participant.
# ===========================================================================
make_export "$work/early/p-early" <<'JSON'
{"blocks": [
   {"block_id": "b1", "started_at": 1800000000},
   {"block_id": "b2", "started_at": 1800086400},
   {"block_id": "b3", "started_at": 1800518400},
   {"block_id": "b4", "started_at": 1801728000}
 ],
 "offers": [{"block_id": "b3", "outcome": "no_response"}],
 "decisions": [
   {"block_id": "b1", "gate_verdict": "offered"},
   {"block_id": "b3", "gate_verdict": "offered"},
   {"block_id": "b4", "gate_verdict": "offered"}
 ],
 "explain": [
   {"week_start_local_date": "2027-01-18", "taps": 1, "delivered_interventions": 1, "blocks_declared": 1}
 ],
 "corrections": [{"scope": "app", "category": "FOCUS_WORK", "rules": 1, "corrections": 1}],
 "rated": ["b2", "b4"],
 "meta": {"export_format": "7", "rating_prompt_enabled_at": "1800432000",
          "v6_period_start": "1800000000", "ratings": "present"}}
JSON
"$analyze" --json "$work/early/p-early" > "$work/early.json"
check "$work/early.json" "the question turned on inside the period" <<'PY'
assert r["participants"]["analysed"] == 1, r["participants"]
assert r["data_quality"]["malformed"] == [], r["data_quality"]
period = r["v6_primary_period"]
placed = period["rows_by_placement"]
assert placed["blocks"] == {"in_period": 2, "question_on": 2}, placed
# The week of 2027-01-18, widened, holds the moment it was turned on.
assert placed["explain_weeks"] == {"straddles": 1}, placed
deviations = period["protocol_deviations"]
assert deviations["rows_at_or_after_rating_prompt_enabled_at_inside_the_period"] == {
    "decisions": 1, "offers": 1, "blocks": 1}, deviations
assert deviations["rated_blocks_inside_the_period"] == {"p-early": 1}, deviations
assert deviations["total"] == 4, deviations
# Neither question-on block is in a figure or in "after day 14".
assert r["power"]["observed_eligible_decision_points"] == 1, r["power"]
assert r["decisions_recorded"]["delivered"] == 0, r["decisions_recorded"]
assert r["blocks_per_participant"]["per_participant"]["p-early"]["blocks_declared"] == 2, r
on = r["strata"]["question_on"]
assert on["blocks_declared"] == {"p-early": 2}, on
assert on["eligible_decision_points"] == 2 and on["decisions_recorded"]["delivered"] == 1, on
assert r["strata"]["after_day_14"]["blocks_declared"] == {}, r["strata"]
# Correction counts carry no time: reported whole, and flagged.
c = r["corrections_per_participant"]
assert c["per_participant"]["p-early"]["corrections"] == 1, c
assert c["includes_rows_outside_the_v6_period"] == ["p-early"], c
PY
early_report="$("$analyze" "$work/early/p-early")"
check_power_markers "$early_report"
for needle in "question first on: 2027-01-20 08:00 UTC" \
              "protocol deviations (item 5): 4" \
              "blocks inside the period with a rating row: {'p-early': 1}" \
              "counts carry no time and are reported whole"; do
  grep -qF -- "$needle" <<<"$early_report" || fail "early report omits: $needle"
done

# ===========================================================================
# 16c. A period start the export states (item 1). Earlier than its blocks
#      give: the history was cleared, so the reinstall exclusion applies.
#      Later: malformed, and the computed start is used. Stated with no block
#      at all: malformed, and the Mac has no period, so it counts only in
#      blocks declared, as a zero.
# ===========================================================================
make_export "$work/stated/p-cleared" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [{"block_id": "a", "gate_verdict": "abstained_warmup"}],
 "meta": {"v6_period_start": "1799913600"}}
JSON
make_export "$work/stated/p-later" <<'JSON'
{"blocks": [{"block_id": "a", "started_at": 1800000000}],
 "offers": [], "decisions": [{"block_id": "a", "gate_verdict": "abstained_warmup"}],
 "meta": {"v6_period_start": "1800000100"}}
JSON
make_export "$work/stated/p-noblock" <<'JSON'
{"blocks": [], "offers": [], "decisions": [],
 "invitations": [{"offered_at": 1800000000, "outcome": "dismissed"}],
 "explain": [{"week_start_local_date": "2027-01-11", "taps": 0, "delivered_interventions": 0, "blocks_declared": 0}],
 "corrections": [{"scope": "app", "category": "FOCUS_WORK", "rules": 1, "corrections": 3}],
 "meta": {"v6_period_start": "1800000000"}}
JSON
"$analyze" --json "$work/stated"/*/ > "$work/stated.json"
check "$work/stated.json" "a stated v6 period start" <<'PY'
whole = r["participants"]["excluded_whole"]
assert "history cleared" in whole["p-cleared"] and "reinstall" in whole["p-cleared"], whole
assert sorted(r["v6_primary_period"]["per_participant"]) == ["p-later", "p-noblock"], r
later = r["v6_primary_period"]["per_participant"]["p-later"]
assert (later["start"], later["stated_start"]) == (1800000000, 1800000100), later
problems = r["v6_primary_period"]["problems"]
assert any("p-later" in p and "computed start is used" in p for p in problems), problems
assert any("p-noblock" in p and "no block" in p for p in problems), problems
assert r["v6_primary_period"]["participants_without_a_period"] == ["p-noblock"], r
# Item 1: no period, no figure but blocks declared, where it is a zero.
assert r["blocks_per_participant"]["per_participant"]["p-noblock"]["blocks_declared"] == 0, r
# p-later wrote no invitations file; p-noblock's is not measured either.
assert r["invitation_acceptance"]["participants_measured"] == 0, r["invitation_acceptance"]
assert r["invitation_acceptance"]["not_measurable_for"] == ["p-later"], r["invitation_acceptance"]
assert r["explain_tap_rate"]["participant_weeks"] == [], r["explain_tap_rate"]
assert "p-noblock" not in r["corrections_per_participant"]["per_participant"], r
assert r["corrections_per_participant"]["not_measurable_for"] == ["p-later"], r
placed = r["v6_primary_period"]["rows_by_placement"]
assert placed["invitations"] == {"no_period": 1}, placed
assert placed["explain_weeks"] == {"no_period": 1}, placed
PY

echo "analyze_cohort_test.sh: OK"
