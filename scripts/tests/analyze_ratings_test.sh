#!/usr/bin/env bash
# Regression tests for the self-rating analysis (scripts/analyze_ratings.py).
#
# Every section pins a rule the pre-registration states, so that the code and
# the text cannot drift apart unnoticed:
#
#   1. The population rules, in their order: malformed rows (an answer after
#      Not now with card_seen_before_answer 0 included) and rows with no block
#      features are listed and excluded; other prompt versions and policy
#      versions are excluded and never pooled; a rated block that started
#      inside its Mac's v6 primary period, ran under 5 minutes, or became
#      eligible before the question was first turned on is a protocol
#      deviation; late answers, the two descriptive purposes, insufficient
#      coverage and unlogged share above 0.30 each leave the primary.
#   2. The v6 period start is the Mac's first block after the upgrade cut,
#      from the blocks and decisions files, whether or not that block has a
#      decision row. A stated start earlier than it is a cleared history (the
#      export is dropped whole); a later one is malformed and the computed
#      start is used. A policy_version 6 decision row with a non-work anchor
#      drops the export whole.
#   3. Missing stays missing: Not now and expiry count in the answer rate's
#      denominator and in no rating distribution.
#   4. Founder, older-format and incomplete exports (no decisions file
#      included) are dropped whole; a founder export is still parsed as a
#      pipeline check, with no statistic.
#   5. The default run reports progress by status only, at any count.
#   6. The one look is refused until the stop rule is met, is computed as of
#      the moment the rule was first met, and withholds H1 and H2 below 100
#      and 200 primary ratings at that moment; it carries the descriptives
#      (kappa on primary ratings, offer replies in three groups, the
#      sustained-anchor share over eligible decision points only, reactivity
#      per person).
#   7. The unit is the person: --same-person merges one tester's Macs, and a
#      name that matches no export, a name in two groups and a group of one
#      are refused.
#   8. Synthetic and real exports never mix, and the synthetic-only switches
#      are refused on real exports.
#   9. The config is a closed schema.
#  10. The statistics against hand-computed values, and the ordinal fit
#      against a closed form and finite differences.
#  11. The report, text or JSON, refuses words that describe something the
#      analysis is not.
#  12. Two runs on the same exports give the same JSON, and --provenance
#      matches shasum.
#  13. --readout is taken once: refused below 100 primary ratings and when the
#      stop rule came first, computed as of the 100th primary rating, and the
#      same on a later export with more rows. The look by count is computed as
#      of the 220th.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
analyze="$repo_root/scripts/analyze_ratings.py"
config="$repo_root/scripts/rating-analysis-config.json"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$analyze" ]] || { echo "ERROR: $analyze is not executable" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

# check JSON_FILE MESSAGE: runs the assertions on stdin with `r` bound to the
# analysis result.
check() {
  local json="$1" message="$2"
  python3 -c "
import json, sys
r = json.load(open(sys.argv[1]))
$(cat)
" "$json" || fail "$message"
}

# A fixture writer: one participant's spec as JSON on stdin, export files into
# DIR with the format-7 headers and the companion files' headers.
cat > "$work/make_export.py" <<'PY'
import csv, json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[2])
import analyze_ratings as ar

spec = json.load(sys.stdin)
folder = Path(sys.argv[1]) / spec["name"]
folder.mkdir(parents=True, exist_ok=True)
name = spec["name"]
files = {
    "ratings": ("-ratings.csv", ar.RATINGS_COLUMNS),
    "features": ("-blockfeatures.csv", ar.BLOCK_FEATURE_COLUMNS),
    "blocks": ("-blocks.csv", ar.BLOCK_COLUMNS),
    "decisions": ("-decisions.csv", ar.DECISION_COLUMNS),
    "outcomes": ("-outcomes.csv", ar.OUTCOME_COLUMNS),
}
for key, (suffix, columns) in files.items():
    if spec.get(key) is None:
        continue
    with (folder / f"{name}{suffix}").open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(columns), lineterminator="\n")
        writer.writeheader()
        for row in spec[key]:
            writer.writerow({c: row.get(c, "") for c in columns})
if spec.get("meta") is not None:
    with (folder / f"{name}-meta.csv").open("w", newline="") as handle:
        handle.write("key,value\n")
        for k, v in spec["meta"].items():
            handle.write(f"{k},{v}\n")
PY

# The fixtures. V is the v6 period start (each Mac's first block after the
# upgrade cut); ENABLED is when the question was first turned on.
# `python3 fixtures.py WEEKS` sets every export's exported_at to ENABLED + WEEKS
# weeks; without it the exports end 3 days after ENABLED.
cat > "$work/fixtures.py" <<'PY'
import json, sys
V = 1_790_000_000
DAY = 86400
ENABLED = V + 15 * DAY
weeks = float(sys.argv[1]) if len(sys.argv) > 1 else None
exported_at = str(ENABLED + int(weeks * 7 * DAY)) if weeks else str(ENABLED + 3 * DAY)
base_feature = {
    "block_feature_version": "1", "policy_version": "6", "card_branch": "came_back",
    "elapsed_seconds": "1800", "covered_seconds": "1500", "coverage_class": "good",
    "work_anchor_share": "0.7", "switch_aways_per_covered_hour": "4.0",
    "longest_run_share": "0.4", "recovery_ratio": "0.5", "feed_share": "0.1",
    "unlogged_share": "0.05", "offer_delivered": "0", "offer_outcome": "",
    "purpose": "deep_work", "intensity": "medium", "planned_minutes": "30",
    "origin": "manual", "hour_bucket": "morning", "weekend": "0",
}
meta = {
    "export_format": "7", "exported_at": exported_at, "rating_prompt_version": "1",
    "rating_prompt_enabled_at": str(ENABLED), "v6_period_start": str(V), "block_feature_version": "1",
}

def rating(block, at, **kw):
    row = {"block_id": block, "prompt_version": "1", "eligible_at": str(at),
           "card_seen_before_answer": "0"}
    row.update(kw)
    return row

def answered(block, at, answer, **kw):
    return rating(block, at, shown_at=str(at + 10), answered_at=str(at + 20), answer=answer, **kw)

def feature(block, **kw):
    row = dict(base_feature, block_id=block)
    row.update(kw)
    return row

def decision(decision_id, block, started, *, policy="6", verdict="offered", anchor="FOCUS_WORK",
             occurred=None, elapsed=1800):
    return {"decision_id": decision_id, "occurred_at": str(occurred or started + 600),
            "block_id": block, "policy_version": policy, "anchor_category": anchor,
            "gate_verdict": verdict, "block_started_at": str(started),
            "block_ended_at": str(started + elapsed), "block_total_paused_seconds": "0"}

t = ENABLED + DAY
p01_ratings = [
    answered("b01", t, "all"),
    answered("b02", t + 100, "most"),
    answered("b03", t + 200, "little"),
    rating("b04", t + 300, shown_at=str(t + 310), not_now_at=str(t + 320), expired_at=str(t + 7500)),
    rating("b05", t + 400, expired_at=str(t + 7600)),
    answered("b06", t + 500, "half", card_seen_before_answer="1"),
    answered("b07", t + 600, "all"),
    answered("b08", t + 700, "most"),
    answered("b09", t + 800, "all"),
    answered("b10", t + 900, "all"),
    answered("b11", t + 1000, "all"),
    answered("b12", t + 1100, "all"),
    answered("b13", t + 1200, "all", prompt_version="2"),
    rating("b14", t + 1300, shown_at=str(t + 1310), answer="all"),
    answered("b15", t + 1400, "all"),
    answered("b16", t + 1500, "sometimes"),
    answered("b01", t + 1600, "all"),
    answered("b18", t + 1700, "all"),
    answered("b19", t + 1800, "most"),
    answered("b20", ENABLED - 100, "all"),
    answered("b21", t + 2000, "most", not_now_at=str(t + 2015)),
]
p01_features = [
    feature("b01", card_branch="stayed"),
    feature("b02", offer_delivered="1", offer_outcome="was_focused"),
    feature("b03", card_branch="ended_elsewhere"),
    feature("b04"),
    feature("b05"),
    feature("b06"),
    feature("b07", purpose="healthy_tech_use"),
    feature("b08", coverage_class="insufficient", card_branch="low_coverage"),
    feature("b09", unlogged_share="0.4"),
    feature("b10", policy_version="5"),
    feature("b11"),
    feature("b12", elapsed_seconds="200"),
    feature("b13"),
    feature("b14"),
    feature("b16"),
    feature("b18", coverage_class="insufficient"),
    feature("b19"),
    feature("b20"),
    feature("b21"),
]
# Every rated block starts after day 14, except b11 (inside the period) and
# b20 (ended before the question was turned on). b19 has no start.
p01_blocks = [{"block_id": "first", "started_at": str(V)}]
p01_blocks += [{"block_id": f"b{i:02d}", "started_at": str(t - 2000)} for i in range(1, 22) if i != 19]
starts = {row["block_id"]: row for row in p01_blocks}
starts["b11"]["started_at"] = str(V + 3 * DAY)
starts["b20"]["started_at"] = str(ENABLED - 2000)
p01_blocks += [{"block_id": f"d{i}", "started_at": str(ENABLED - (i + 1) * DAY)} for i in range(4)]

# p04: a policy 5 block before the upgrade cut, then a first block with no
# decision row (day 0), which starts the period. The first v6 decision row is
# on day 2. c00 (day 13) is inside the period; c01 (day 15) is not, although
# it is inside 14 days of the first v6 decision row. c02 holds an offered
# point and two abstentions with labels: only the offered point is eligible.
p04 = {
    "name": "p04",
    "meta": meta,
    "ratings": [answered("c00", V + 16 * DAY, "half"), answered("c01", V + 16 * DAY + 100, "most"),
                answered("c02", V + 21 * DAY, "all")],
    "features": [feature("c00"), feature("c01"), feature("c02", card_branch="stayed")],
    "blocks": [{"block_id": "old", "started_at": str(V - 5 * DAY)},
               {"block_id": "first", "started_at": str(V)},
               {"block_id": "second", "started_at": str(V + 2 * DAY)},
               {"block_id": "c00", "started_at": str(V + 13 * DAY)},
               {"block_id": "c01", "started_at": str(V + 15 * DAY)},
               {"block_id": "c02", "started_at": str(V + 20 * DAY)}],
    "decisions": [decision("x0", "old", V - 5 * DAY, policy="5", anchor="SOCIAL_FEED"),
                  decision("x1", "second", V + 2 * DAY),
                  decision("x5", "c01", V + 15 * DAY),
                  decision("x2", "c02", V + 20 * DAY),
                  decision("x3", "c02", V + 20 * DAY, verdict="abstained_at_anchor", occurred=V + 20 * DAY + 700),
                  decision("x4", "c02", V + 20 * DAY, verdict="abstained_min_switches", occurred=V + 20 * DAY + 800)],
    "outcomes": [{"decision_id": "x5", "censor_reason": "observer_gap", "sustained_anchor_900s": ""},
                 {"decision_id": "x2", "censor_reason": "none", "sustained_anchor_900s": "0"},
                 {"decision_id": "x3", "censor_reason": "none", "sustained_anchor_900s": "1"},
                 {"decision_id": "x4", "censor_reason": "none", "sustained_anchor_900s": "1"}],
}

# p06-later: its meta states a start one day after its first block. The
# computed start is used, so its block at day 14 plus 600 s is out of the
# period (under the stated start it would be inside).
p06 = {
    "name": "p06-later",
    "meta": dict(meta, v6_period_start=str(V + DAY)),
    "ratings": [answered("e01", V + 16 * DAY, "all")],
    "features": [feature("e01", card_branch="stayed")],
    "blocks": [{"block_id": "first", "started_at": str(V)},
               {"block_id": "e01", "started_at": str(V + 14 * DAY + 600)}],
    "decisions": [],
}

def founder_feature(block):
    return dict(base_feature, block_id=block, card_branch="stayed")

others = [
    {"name": "founder-mac", "meta": meta,
     "ratings": [answered("f1", t, "all"),
                 rating("f2", t, shown_at=str(t + 1), not_now_at=str(t + 2))],
     "features": [founder_feature("f1"), founder_feature("f2")],
     "blocks": [], "decisions": []},
    {"name": "p02-old", "meta": dict(meta, export_format="6"), "ratings": [], "features": [],
     "blocks": [], "decisions": []},
    {"name": "p03-incomplete", "meta": meta, "ratings": [], "blocks": []},
    # A cleared history: the stated start is a day before its first block.
    {"name": "p05-cleared", "meta": dict(meta, v6_period_start=str(V - DAY)),
     "ratings": [answered("g01", V + 16 * DAY, "all")], "features": [feature("g01")],
     "blocks": [{"block_id": "g01", "started_at": str(V)}], "decisions": []},
    # A v6 build from before the anchor rule.
    {"name": "p07-anchor", "meta": meta,
     "ratings": [answered("h01", V + 16 * DAY, "all")], "features": [feature("h01")],
     "blocks": [{"block_id": "first", "started_at": str(V)},
                {"block_id": "h01", "started_at": str(V + 15 * DAY)}],
     "decisions": [decision("y1", "first", V, anchor="SOCIAL_FEED")]},
    {"name": "p08-nodecisions", "meta": meta,
     "ratings": [answered("i01", V + 16 * DAY, "all")], "features": [feature("i01")],
     "blocks": [{"block_id": "i01", "started_at": str(V)}]},
]
for spec in [{"name": "p01", "ratings": p01_ratings, "features": p01_features, "meta": meta,
              "blocks": p01_blocks, "decisions": []}, p04, p06] + others:
    print(json.dumps(spec))
PY

build() {
  local dir="$1"
  shift
  python3 "$work/fixtures.py" "$@" | while IFS= read -r line; do
    printf '%s' "$line" | python3 "$work/make_export.py" "$dir" "$repo_root/scripts"
  done
}

cohort="$work/cohort"
build "$cohort"

# ---------------------------------------------------------------------------
# 1-5. The default run.
# ---------------------------------------------------------------------------
"$analyze" --json "$cohort"/*/ > "$work/progress.json" || fail "the default run failed"
check "$work/progress.json" "population rules" <<'PY'
rows = r["rows"]
assert rows["excluded"] == {
    "malformed block-feature row": 1,
    "malformed rating row": 4,
    "no block-feature row": 1,
    "no start for the block in the blocks file": 1,
    "policy_version is not 6 (never pooled)": 1,
    "rating prompt version is not 1": 1,
}, rows["excluded"]
assert rows["protocol_deviations"] == {
    "block shorter than 300 s": 1,
    "rated block started inside the v6 primary period (before day 14)": 2,
    "rating row for a block that ended before the question was first turned on": 1,
}, rows["protocol_deviations"]
assert any("answered after Not now with card_seen_before_answer 0" in m for m in rows["malformed"]), rows
p = r["progress"]
assert p["eligible"] == 12, p["eligible"]
assert p["by_state"]["answered"] == 10 and p["by_state"]["not_now"] == 1, p["by_state"]
assert p["by_state"]["expired_never_shown"] == 1, p["by_state"]
assert p["late_answers"] == 1 and p["descriptive_purpose_answers"] == 1, p
assert p["card_population"] == 8, p["card_population"]
assert p["primary_exclusions"] == {
    "coverage class insufficient": 1,
    "unlogged or needs-a-category share above 0.30": 1,
}, p["primary_exclusions"]
assert p["primary"] == 6 and p["primary_testers"] == 3, p
PY
check "$work/progress.json" "the v6 period start" <<'PY'
V, DAY = 1_790_000_000, 86400
starts = r["exports"]["v6_period_starts"]
# p04's period starts at its first block after the policy 5 decision, which
# has no decision row; its first v6 decision row is two days later.
assert starts["p04"] == {"computed": V, "upgrade_cut": V - 5 * DAY + 600, "stated": V}, starts["p04"]
assert starts["p01"] == {"computed": V, "upgrade_cut": None, "stated": V}, starts["p01"]
# p06-later states a later start: malformed, and the computed one is used.
assert starts["p06-later"]["computed"] == V and starts["p06-later"]["stated"] == V + DAY, starts
assert r["exports"]["problems"] == {
    "v6_period_start in the meta later than the computed start (malformed; the computed start is used)": 1,
}, r["exports"]["problems"]
assert any(m.startswith("p06-later: the meta's v6_period_start") for m in r["rows"]["malformed"]), r["rows"]
PY
check "$work/progress.json" "whole-export exclusions and the founder pipeline check" <<'PY'
e = r["exports"]
assert e["analysed"] == ["p01", "p04", "p06-later"], e["analysed"]
assert set(e["excluded_whole"]) == {"founder-mac", "p02-old", "p03-incomplete", "p05-cleared",
                                    "p07-anchor", "p08-nodecisions"}, e["excluded_whole"]
assert "older than 7" in e["excluded_whole"]["p02-old"], e["excluded_whole"]
assert "-blockfeatures.csv" in e["excluded_whole"]["p03-incomplete"], e["excluded_whole"]
assert e["excluded_whole"]["p05-cleared"].startswith("history cleared"), e["excluded_whole"]
assert "reinstall exclusion" in e["excluded_whole"]["p05-cleared"], e["excluded_whole"]
assert "non-work anchor" in e["excluded_whole"]["p07-anchor"], e["excluded_whole"]
assert "-decisions.csv" in e["excluded_whole"]["p08-nodecisions"], e["excluded_whole"]
assert e["founder_pipeline_check"] == {"founder-mac": {"answered": 1, "block_feature_rows": 2, "not_now": 1}}, e
PY
check "$work/progress.json" "the default run is progress by status only" <<'PY'
assert r["mode"] == "progress", r["mode"]
for key in ("readout", "final_look"):
    assert key not in r, key
assert not r["stop_rule"]["readout_due"] and r["stop_rule"]["readout_cutoff"] is None, r["stop_rule"]
text = json.dumps(r)
for word in ("work_anchor_share", "by_answer", "by_tier", "most", "little"):
    assert word not in text, word
PY
"$analyze" "$cohort"/*/ > "$work/progress.txt" || fail "the text run failed"
grep -q "PRIMARY ratings: 6 from 3 tester(s)" "$work/progress.txt" || fail "text lacks the primary count"
grep -q "PROTOCOL DEVIATION, rated block started inside the v6 primary period" "$work/progress.txt" \
  || fail "text does not name the deviation"
grep -q "founder pipeline check (n = 1, founder; not a result)" "$work/progress.txt" \
  || fail "text does not label the founder check"
grep -q "Not yet: it is taken once, with --readout, when there are 100 primary" "$work/progress.txt" \
  || fail "text does not withhold the readout"
grep -q "one person, several Macs: none" "$work/progress.txt" || fail "text does not list --same-person"
echo "ok: population, period start, missingness, exclusions, progress only"

# ---------------------------------------------------------------------------
# 6. The one look.
# ---------------------------------------------------------------------------
set +e
"$analyze" --final-look "$cohort"/*/ > "$work/early.txt"
status=$?
set -e
[[ $status -eq 3 ]] || fail "--final-look before the stop rule exited $status, expected 3"
grep -q "REFUSED: the stop rule is not met" "$work/early.txt" || fail "early look not refused in the report"
grep -q "H1 (primary)" "$work/early.txt" && fail "an early look printed H1"
grep -q "1. DOES THE BLOCK SUMMARY" "$work/early.txt" && fail "an early look printed the readout"
set +e
"$analyze" --readout "$cohort"/*/ > "$work/early-readout.txt"
status=$?
set -e
[[ $status -eq 3 ]] || fail "--readout below 100 primary ratings exited $status, expected 3"
grep -q "REFUSED: 6 primary ratings; the readout is taken once there are 100" "$work/early-readout.txt" \
  || fail "an early readout was not refused"
grep -q "1. DOES THE BLOCK SUMMARY" "$work/early-readout.txt" && fail "an early readout printed answers"

late="$work/late"
build "$late" 17
"$analyze" --json --final-look "$late"/*/ > "$work/late.json" || fail "the look at 17 weeks failed"
check "$work/late.json" "the look by time, with H1 and H2 withheld and the descriptives shown" <<'PY'
V, DAY, WEEK = 1_790_000_000, 86400, 7 * 86400
ENABLED = V + 15 * DAY
s = r["stop_rule"]
assert s["met"] and s["met_by_time"] and not s["met_by_count"], s
assert s["met_by"] == "time" and s["look_cutoff"] == ENABLED + 16 * WEEK, s
assert s["readout_at_the_look"] and not s["readout_due"], s
look = r["final_look"]
assert look["stop_rule_met"] and "refused" not in look, look
assert look["cutoff"] == ENABLED + 16 * WEEK and look["primary_ratings_at_the_cutoff"] == 6, look
assert look["h1"] == "not reported: 6 primary ratings, below 100", look["h1"]
assert look["h2"] == "not reported: 6 primary ratings, below 200", look["h2"]
d = r["readout"]
assert d["cutoff"] == ENABLED + 16 * WEEK and d["exports_taken_before_the_cutoff"] == [], d["cutoff"]
card = d["card"]
assert card["population"] == 8 and card["table"]["low_coverage"]["good"] == 1, card["table"]
assert card["kappa_rows"] == 6 and card["weighted_kappa"] == 1.0, card
assert d["distribution"]["primary"]["by_tier"] == {"low": 1, "good": 2, "high": 3}, d["distribution"]
assert d["distribution"]["late"]["by_answer"]["half"] == 1, d["distribution"]["late"]
offers = d["offers"]
assert offers["offered_blocks"] == 1 and offers["by_reply"]["was_focused"]["good"] == 1, offers
sustained = d["sustained_anchor"]
assert sustained["exports_with_labels"] == 1, sustained
assert sustained["exports_without_labels"] == ["p01", "p06-later"], sustained
# c02's two labelled abstentions (sustained 1) are not eligible points; its
# one offered point (sustained 0) is. c01's offered point is censored.
assert sustained["blocks_with_a_measured_eligible_point"] == 1, sustained
assert sustained["by_tier"]["high"] == {"blocks": 1, "mean_share": 0.0, "median_share": 0.0}, sustained
assert sustained["by_tier"]["good"]["blocks"] == 0, sustained
rates = d["answer_rate"]
assert rates["overall"]["eligible"] == 12 and rates["overall"]["answered"] == 10, rates["overall"]
assert rates["by_person"]["p04"] == {"eligible": 2, "answered": 2, "rate": 1.0}, rates["by_person"]
people = {p["person"]: p for p in d["reactivity"]["people"]}
assert people["p01"] == {"person": "p01", "status": "measured", "macs": ["p01"], "before": 6, "after": 18}, people
assert people["p04"]["before"] == 2 and people["p04"]["after"] == 2, people
assert people["p06-later"]["before"] == 1 and people["p06-later"]["after"] == 0, people
assert d["person_variation"]["people"] == 3, d["person_variation"]
PY
"$analyze" --final-look "$late"/*/ > "$work/late.txt" || fail "the text report of the look failed"
grep -q "first met by time at" "$work/late.txt" || fail "the look does not print when the rule was met"
grep -q "DESCRIPTIVES AT THE LOOK" "$work/late.txt" || fail "the look does not carry the descriptives"
set +e
"$analyze" --readout "$late"/*/ > "$work/late-readout.txt"
status=$?
set -e
[[ $status -eq 3 ]] || fail "--readout after the stop rule came first exited $status, expected 3"
grep -q "REFUSED: the stop rule was met before primary ratings reached 100" "$work/late-readout.txt" \
  || fail "--readout did not defer to the look"
echo "ok: the one look is refused early, computed at its cutoff, withheld below its minimums"

# ---------------------------------------------------------------------------
# 7. The unit is the person.
# ---------------------------------------------------------------------------
"$analyze" --json --same-person p04,p01 "$cohort"/*/ > "$work/same.json"
check "$work/same.json" "--same-person did not merge the two Macs" <<'PY'
assert r["exports"]["people"] == {"p01": "p01", "p04": "p01", "p06-later": "p06-later"}, r["exports"]["people"]
assert r["exports"]["declared"]["same_person"] == [["p01", "p04"]], r["exports"]["declared"]
assert r["progress"]["primary"] == 6 and r["progress"]["primary_testers"] == 2, r["progress"]
PY
"$analyze" --same-person p04,p01 --reinstalled nobody "$cohort"/*/ > "$work/same.txt"
grep -q "one person, several Macs: p01, p04" "$work/same.txt" || fail "text does not print the person groups"
grep -q "declared but not among the exports: nobody" "$work/same.txt" \
  || fail "text does not flag a declared name that matches no export"
"$analyze" --json --final-look --same-person p01,p04 "$late"/*/ > "$work/same-late.json"
check "$work/same-late.json" "the reactivity proxy is not summed per person" <<'PY'
people = {p["person"]: p for p in r["readout"]["reactivity"]["people"]}
assert people["p01"] == {"person": "p01", "status": "measured", "macs": ["p01", "p04"],
                         "before": 8, "after": 20}, people
assert "p04" not in people, people
PY
expect_refusal() {
  local message="$1"
  shift
  set +e
  "$analyze" "$@" > /dev/null 2> "$work/stderr"
  local got=$?
  set -e
  [[ $got -eq 2 ]] || fail "$message (exit $got, expected 2)"
}
expect_refusal "a --same-person name that matches no export was accepted" \
  --same-person p01,p04x "$cohort"/*/
expect_refusal "a name in two --same-person groups was accepted" \
  --same-person p01,p04 --same-person p04,p06-later "$cohort"/*/
expect_refusal "a --same-person group of one was accepted" --same-person p01 "$cohort"/*/
grep -q "names fewer than two exports" "$work/stderr" || fail "the group-of-one refusal does not say why"
echo "ok: one tester's Macs are one person, and a typo is refused"

# ---------------------------------------------------------------------------
# 8. Synthetic and real never mix.
# ---------------------------------------------------------------------------
expect_status() {
  local want="$1" message="$2"
  shift 2
  set +e
  "$@" > /dev/null 2> "$work/stderr"
  local got=$?
  set -e
  [[ $got -eq $want ]] || fail "$message (exit $got, expected $want: $(cat "$work/stderr"))"
}
expect_status 2 "--ignore-stop-rule accepted on real exports" \
  "$analyze" --final-look --ignore-stop-rule "$cohort"/*/
expect_status 2 "--bootstrap-replicates accepted on real exports" \
  "$analyze" --bootstrap-replicates 10 "$cohort"/*/
expect_status 2 "--synthetic accepted on real exports" "$analyze" --synthetic "$cohort/p01/"
mixed="$work/mixed"
python3 "$work/fixtures.py" | head -1 | python3 -c "
import json, sys
spec = json.load(sys.stdin); spec['meta']['synthetic'] = '1'; print(json.dumps(spec))" \
  | python3 "$work/make_export.py" "$mixed" "$repo_root/scripts"
expect_status 2 "a synthetic export read as real" "$analyze" "$mixed/p01/"
"$analyze" --synthetic "$mixed/p01/" > "$work/synthetic.txt" || fail "a synthetic export was refused"
if grep -v '^\[SYNTHETIC contract test, not evidence\] ' "$work/synthetic.txt" | grep -q .; then
  fail "a synthetic report line is not stamped"
fi
echo "ok: synthetic and real exports never mix"

# ---------------------------------------------------------------------------
# 9. The config is a closed schema.
# ---------------------------------------------------------------------------
python3 - "$config" "$work" <<'PY'
import json, sys
config = json.load(open(sys.argv[1]))
extra = dict(config, tuned_after_seeing_data=True)
json.dump(extra, open(sys.argv[2] + "/extra.json", "w"))
missing = {k: v for k, v in config.items() if k != "stop_rule"}
json.dump(missing, open(sys.argv[2] + "/missing.json", "w"))
nested = json.loads(json.dumps(config)); nested["h2"]["ridge_lambda"] = "1"
json.dump(nested, open(sys.argv[2] + "/nested.json", "w"))
# The first draft floored the run at 1 second; that key is gone.
seconds = json.loads(json.dumps(config))
seconds["h2"]["longest_run_floor_seconds"] = seconds["h2"].pop("longest_run_share_floor")
json.dump(seconds, open(sys.argv[2] + "/seconds.json", "w"))
floor = json.loads(json.dumps(config)); floor["h2"]["longest_run_share_floor"] = 1
json.dump(floor, open(sys.argv[2] + "/floor.json", "w"))
PY
expect_status 2 "a config with an extra key was accepted" "$analyze" --config "$work/extra.json" "$cohort/p01/"
expect_status 2 "a config with a missing key was accepted" "$analyze" --config "$work/missing.json" "$cohort/p01/"
expect_status 2 "a config with a mistyped value was accepted" "$analyze" --config "$work/nested.json" "$cohort/p01/"
expect_status 2 "a config with the 1-second floor key was accepted" "$analyze" --config "$work/seconds.json" "$cohort/p01/"
expect_status 2 "a share floor of 1 was accepted" "$analyze" --config "$work/floor.json" "$cohort/p01/"
echo "ok: the config is closed"

# ---------------------------------------------------------------------------
# 10-11. Statistics, the ordinal fit, and wording.
# ---------------------------------------------------------------------------
python3 - "$repo_root/scripts" <<'PY' || fail "statistics or wording"
import math, random, sys
sys.path.insert(0, sys.argv[1])
import analyze_ratings as ar

assert ar.ranks([1, 2, 2, 3]) == [1.0, 2.5, 2.5, 4.0]
# ranks of y = [1, 2, 3.5, 5, 3.5]; r = 8 / sqrt(10 * 9.5)
assert abs(ar.spearman([1, 2, 3, 4, 5], [5, 6, 7, 8, 7]) - 8 / math.sqrt(95)) < 1e-12
assert ar.spearman([1, 1, 1], [1, 2, 3]) is None
assert ar.auroc([0.1, 0.4, 0.35, 0.8], [False, False, True, True]) == 0.75
assert ar.auroc([0.5, 0.5], [False, True]) == 0.5
assert ar.auroc([0.5, 0.6], [True, True]) is None
# Each of six cells holds one pair; by hand, kappa = (4/6 - 5/9) / (1 - 5/9) = 0.25.
assert abs(ar.weighted_kappa([0, 0, 1, 1, 2, 2], [0, 1, 1, 2, 2, 0], 3) - 0.25) < 1e-12
assert ar.weighted_kappa([0, 1, 2], [0, 1, 2], 3) == 1.0
assert ar.quantile([1, 2, 3, 4], 0.25) == 1.75 and ar.quantile([5], 0.9) == 5

# No feature, no person, no pseudo-row: the thresholds are the logits of the
# cumulative shares.
y = [0] * 10 + [1] * 20 + [2] * 30 + [3] * 25 + [4] * 15
fit = ar.fit_ordinal([[] for _ in y], y, None, 5, ridge=1.0, tau=0.0, pseudo_weight=0.0)
for got, share in zip(fit.thresholds, (0.10, 0.30, 0.60, 0.85)):
    assert abs(got - math.log(share / (1 - share))) < 1e-6, fit.thresholds
assert fit.converged

# The gradient and Hessian against central differences.
rng = random.Random(3)
X = [[rng.gauss(0, 1) for _ in range(3)] for _ in range(60)]
yy = [rng.randrange(5) for _ in range(60)]
groups = [rng.randrange(5) for _ in range(60)]
problem = ar._OrdinalProblem(X, yy, groups, 5, 1.0, 0.7, 0.5)
params = [-1.5, -0.4, 0.3, 1.2, 0.2, -0.3, 0.1] + [rng.gauss(0, 0.5) for _ in range(5)]
grad, hess = problem.derivatives(params)
h = 1e-6
for i in range(len(params)):
    up = params[:]; up[i] += h
    down = params[:]; down[i] -= h
    assert abs((problem.objective(up) - problem.objective(down)) / (2 * h) - grad[i]) < 1e-5
    g_up, _ = problem.derivatives(up)
    g_down, _ = problem.derivatives(down)
    for j in range(len(params)):
        assert abs((g_up[j] - g_down[j]) / (2 * h) - hess[i][j]) < 1e-5

# A level no training row used keeps a finite, ordered threshold.
y_gap = [0] * 10 + [1] * 10 + [3] * 10 + [4] * 10
fit = ar.fit_ordinal([[] for _ in y_gap], y_gap, None, 5, ridge=1.0, tau=0.0, pseudo_weight=0.5)
assert fit.converged and all(b > a for a, b in zip(fit.thresholds, fit.thresholds[1:])), fit.thresholds
assert ar.level_probabilities(fit.thresholds, 0.0)[2] > 0

# The longest run's share is floored at 0.01 before the log, whatever the
# block's length: a zero run gives log(0.01), not log(1 s / elapsed).
row = {c: "" for c in ar.BLOCK_FEATURE_COLUMNS}
row.update(block_id="x", block_feature_version="1", policy_version="6", card_branch="came_back",
           elapsed_seconds="600", covered_seconds="500", coverage_class="good",
           work_anchor_share="0.5", switch_aways_per_covered_hour="1", longest_run_share="0",
           recovery_ratio="0", feed_share="0", unlogged_share="0", offer_delivered="0",
           purpose="study", intensity="light", planned_minutes="25", weekend="0")
parsed, error = ar._parse_features(row)
assert error is None, error
assert abs(ar._model_value(parsed, "log_longest_run_share", 0.01) - math.log(0.01)) < 1e-12
parsed.longest_run_share = 0.005
assert abs(ar._model_value(parsed, "log_longest_run_share", 0.01) - math.log(0.01)) < 1e-12
parsed.longest_run_share = 0.4
assert abs(ar._model_value(parsed, "log_longest_run_share", 0.01) - math.log(0.4)) < 1e-12

# A block-feature row whose coverage and card branch disagree is malformed.
row.update(coverage_class="insufficient")
parsed, error = ar._parse_features(row)
assert parsed is None and "disagree" in error, error

for bad in ("model accuracy 0.8", "this predicts focus", "an attention model",
            "Velvt learns", "bad focus blocks", "behavioural modelling"):
    try:
        ar.check_wording(bad)
    except ar.WordingError:
        continue
    raise AssertionError(f"check_wording let through {bad!r}")
ar.check_wording(ar.AGREEMENT_STATEMENT)
# A participant's own folder name is not the report's wording.
ar.check_wording("excluded whole: adaptive-learner-mac: reinstalled", ("adaptive-learner-mac",))
# The JSON output is guarded too.
result = {"exports": {"received": ["p01"], "people": {"p01": "p01"}, "declared": {}},
          "provenance": {}, "statement": "the model's accuracy"}
try:
    ar.render_json(result)
except ar.WordingError:
    pass
else:
    raise AssertionError("render_json let a banned word through")
result["statement"] = ar.AGREEMENT_STATEMENT
ar.render_json(result)

# A row as of a moment: later events are undone, earlier ones kept.
row = ar.Rating(participant="p", person="p", block_id="b", prompt_version=1, eligible_at=100,
                shown_at=110, answered_at=500, not_now_at=None, expired_at=None,
                card_seen_before_answer=1, answer="most", code=3)
assert row.as_of(99) is None
then = row.as_of(200)
assert then.state == "open_shown" and then.answer == "" and then.code is None, then
assert then.card_seen_before_answer == 0, then
assert row.as_of(500).state == "answered" and row.as_of(500).card_seen_before_answer == 1
assert row.as_of(105).state == "open_never_shown"
PY
echo "ok: statistics, ordinal fit, wording"

# ---------------------------------------------------------------------------
# 12. Determinism and provenance.
# ---------------------------------------------------------------------------
"$analyze" --json --final-look "$late"/*/ > "$work/late-again.json"
cmp -s "$work/late.json" "$work/late-again.json" || fail "two runs on the same exports differ"
"$analyze" --provenance > "$work/provenance.json"
check "$work/provenance.json" "--provenance disagrees with shasum" <<PY
import hashlib
assert r["analysis_sha256"] == hashlib.sha256(open("$analyze", "rb").read()).hexdigest()
assert r["config_sha256"] == hashlib.sha256(open("$config", "rb").read()).hexdigest()
PY
echo "ok: determinism and provenance"

# ---------------------------------------------------------------------------
# 13. The readout is taken once, and the look is computed at its cutoff.
# ---------------------------------------------------------------------------
# SYNTHETIC exports (so the bootstrap can be short): s01 has 70 primary
# ratings and s02 50, interleaved, so the 100th primary rating is s02's 50th.
# s01 also has a row eligible before that moment and answered after it, and a
# row shown before it and put off with Not now after it. `more` adds 30
# ratings after the moment and a later export.
cat > "$work/readout_fixtures.py" <<'PY'
import json, sys
V = 1_790_000_000
DAY = 86400
ENABLED = V + 15 * DAY
t = ENABLED + DAY
C = t + 49520
more = len(sys.argv) > 1
levels = ["little", "less_than_half", "half", "most", "all"]
branches = ["ended_elsewhere", "came_back", "stayed"]

def export(name, times, extra=()):
    ratings, features, blocks = [], [], [{"block_id": "first", "started_at": str(V)}]
    for i, at in enumerate(times):
        block = f"{name}-b{i:03d}"
        ratings.append({"block_id": block, "prompt_version": "1", "eligible_at": str(at),
                        "shown_at": str(at + 10), "answered_at": str(at + 20),
                        "card_seen_before_answer": "0", "answer": levels[(i * 7 + len(name)) % 5]})
        features.append(dict(feature, block_id=block, card_branch=branches[i % 3],
                             work_anchor_share=str(round(0.1 + 0.8 * ((i * 37) % 100) / 100, 3))))
        blocks.append({"block_id": block, "started_at": str(at - 2000)})
    for row in extra:
        ratings.append(row)
        features.append(dict(feature, block_id=row["block_id"], card_branch="stayed",
                             work_anchor_share="0.5"))
        blocks.append({"block_id": row["block_id"], "started_at": str(int(row["eligible_at"]) - 2000)})
    return {"name": name, "ratings": ratings, "features": features, "blocks": blocks,
            "decisions": [], "meta": dict(meta, exported_at=str(t + (400000 if more else 200000)))}

feature = {
    "block_feature_version": "1", "policy_version": "6", "elapsed_seconds": "1800",
    "covered_seconds": "1500", "coverage_class": "good", "switch_aways_per_covered_hour": "4.0",
    "longest_run_share": "0.4", "recovery_ratio": "0.5", "feed_share": "0.1",
    "unlogged_share": "0.05", "offer_delivered": "0", "offer_outcome": "", "purpose": "deep_work",
    "intensity": "medium", "planned_minutes": "30", "origin": "manual",
    "hour_bucket": "morning", "weekend": "0",
}
meta = {"export_format": "7", "rating_prompt_version": "1", "rating_prompt_enabled_at": str(ENABLED),
        "v6_period_start": str(V), "block_feature_version": "1", "synthetic": "1"}
open_row = {"block_id": "s01-open", "prompt_version": "1", "eligible_at": str(C - 50),
            "shown_at": str(C - 40), "answered_at": str(C + 5000), "card_seen_before_answer": "0",
            "answer": "all"}
not_now_row = {"block_id": "s01-later", "prompt_version": "1", "eligible_at": str(C - 30),
               "shown_at": str(C - 25), "not_now_at": str(C + 100), "expired_at": str(C + 7000),
               "card_seen_before_answer": "0"}
s01 = [t + 1000 * i for i in range(70)]
s02 = [t + 500 + 1000 * i for i in range(50)] + ([t + 60000 + 1000 * i for i in range(30)] if more else [])
print(json.dumps(export("s01", s01, (open_row, not_now_row))))
print(json.dumps(export("s02", s02)))
PY
for variant in first more; do
  python3 "$work/readout_fixtures.py" $([[ $variant == more ]] && echo more) | while IFS= read -r line; do
    printf '%s' "$line" | python3 "$work/make_export.py" "$work/readout-$variant" "$repo_root/scripts"
  done
done
"$analyze" --json --synthetic "$work/readout-first"/*/ > "$work/due.json" || fail "the progress run at 121 failed"
check "$work/due.json" "the default run past 100 read answers" <<'PY'
C = 1_790_000_000 + 16 * 86400 + 49520
assert r["mode"] == "progress" and "readout" not in r, r["mode"]
assert r["progress"]["primary"] == 121, r["progress"]
assert r["stop_rule"]["readout_due"] and r["stop_rule"]["readout_cutoff"] == C, r["stop_rule"]
text = json.dumps(r)
for word in ("work_anchor_share", "by_answer", "by_tier", "most", "little"):
    assert word not in text, word
PY
"$analyze" --synthetic "$work/readout-first"/*/ > "$work/due.txt"
grep -q "Due: primary ratings reached 100 at" "$work/due.txt" || fail "the progress run does not say the readout is due"
grep -q "1. DOES THE BLOCK SUMMARY" "$work/due.txt" && fail "the progress run printed the readout"
for variant in first more; do
  "$analyze" --json --synthetic --readout --bootstrap-replicates 20 "$work/readout-$variant"/*/ \
    > "$work/readout-$variant.json" || fail "--readout failed on $variant"
done
check "$work/readout-first.json" "the readout is not computed as of the 100th primary rating" <<'PY'
C = 1_790_000_000 + 16 * 86400 + 49520
d = r["readout"]
assert r["mode"] == "readout" and "final_look" not in r, r["mode"]
assert d["cutoff"] == C, d["cutoff"]
assert d["distribution"]["primary"]["n"] == 100, d["distribution"]["primary"]
# Eligible by then: 100 answered, the row answered after it, the row put off after it.
assert d["answer_rate"]["overall"] == {"eligible": 102, "answered": 100, "rate": 100 / 102}, d["answer_rate"]
assert d["card"]["kappa_rows"] == 100, d["card"]
PY
python3 - "$work/readout-first.json" "$work/readout-more.json" <<'PY' || fail "the readout changed on a later export"
import json, sys
first, more = (json.load(open(path))["readout"] for path in sys.argv[1:])
assert first == more, "the readout differs"
PY
"$analyze" --synthetic --readout --bootstrap-replicates 20 "$work/readout-first"/*/ > "$work/readout.txt"
grep -q "as of $((1790000000 + 16 * 86400 + 49520)) (2026-" "$work/readout.txt" \
  || fail "the readout does not print its cutoff"

python3 - "$repo_root/scripts" <<'PY' || fail "the look by count"
import sys
sys.path.insert(0, sys.argv[1])
import analyze_ratings as ar
config = ar.load_config(ar.DEFAULT_CONFIG_PATH)
V, DAY = 1_790_000_000, 86400
ENABLED = V + 15 * DAY
t = ENABLED + DAY
levels = config["answer_levels"]
feature = {
    "block_feature_version": "1", "policy_version": "6", "card_branch": "came_back",
    "elapsed_seconds": "1800", "covered_seconds": "1500", "coverage_class": "good",
    "switch_aways_per_covered_hour": "4.0", "longest_run_share": "0.4", "recovery_ratio": "0.5",
    "feed_share": "0.1", "unlogged_share": "0.05", "offer_delivered": "0", "offer_outcome": "",
    "purpose": "deep_work", "intensity": "medium", "planned_minutes": "30", "origin": "manual",
    "hour_bucket": "morning", "weekend": "0",
}
exports = {}
for j in range(12):
    name = f"q{j:02d}"
    ratings, features, blocks = [], [], [{"block_id": "first", "started_at": str(V)}]
    for i in range(20):
        at = t + (i * 12 + j) * 100
        block = f"{name}-{i:02d}"
        ratings.append({"block_id": block, "prompt_version": "1", "eligible_at": str(at),
                        "shown_at": str(at + 10), "answered_at": str(at + 20),
                        "card_seen_before_answer": "0", "answer": levels[(i + j) % 5]})
        features.append(dict(feature, block_id=block, work_anchor_share=str((i * 7 % 20) / 20)))
        blocks.append({"block_id": block, "started_at": str(at - 2000)})
    meta = {"export_format": "7", "exported_at": str(t + 30000), "rating_prompt_version": "1",
            "rating_prompt_enabled_at": str(ENABLED), "v6_period_start": str(V),
            "block_feature_version": "1", "synthetic": "1"}
    exports[name] = ar.Export(name=name, meta=meta, ratings=ratings, features=features,
                              blocks=blocks, decisions=[])
cohort = ar.build_cohort(exports, config, synthetic=True)
assert len(cohort.primary) == 240
rule = ar.stop_rule(cohort, config)
assert rule["met_by"] == "count" and not rule["met_by_time"], rule
assert rule["look_cutoff"] == t + 219 * 100 + 20, rule
assert rule["readout_cutoff"] == t + 99 * 100 + 20 and rule["readout_due"], rule
result = ar.analyse(cohort, config, final_look=True, bootstrap_replicates=20, skip_h2=True)
look = result["final_look"]
assert look["cutoff"] == rule["look_cutoff"], look
assert look["primary_ratings_at_the_cutoff"] == 220 and look["testers_at_the_cutoff"] == 12, look
assert look["h1"]["n"] == 220, look["h1"]["n"]
assert result["readout"]["distribution"]["primary"]["n"] == 220, result["readout"]["distribution"]
# Fewer than 12 testers by the 220th rating: the count is met only when the
# twelfth person's first rating arrives.
few = {k: v for k, v in exports.items() if k != "q11"}
late = ar.Export(name="q11", meta=dict(exports["q11"].meta), ratings=[dict(r) for r in exports["q11"].ratings[:1]],
                 features=exports["q11"].features[:1], blocks=exports["q11"].blocks[:2], decisions=[])
late.ratings[0].update(eligible_at=str(t + 25000), shown_at=str(t + 25010), answered_at=str(t + 25020))
late.blocks[1]["started_at"] = str(t + 23000)
few["q11"] = late
rule = ar.stop_rule(ar.build_cohort(few, config, synthetic=True), config)
assert rule["look_cutoff"] == t + 25020 and rule["met_by"] == "count", rule
PY
echo "ok: the readout is taken once at the 100th primary rating; the look at the 220th"

echo "analyze_ratings_test.sh: all checks passed"
