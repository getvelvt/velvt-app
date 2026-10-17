#!/usr/bin/env bash
# Regression tests for the self-rating analysis (scripts/analyze_ratings.py).
#
# Every section pins a rule the pre-registration states, so that the code and
# the text cannot drift apart unnoticed:
#
#   1. The population rules, in their order: malformed rows and rows with no
#      block features are listed and excluded; other prompt versions and
#      policy versions are excluded and never pooled; a rated block that
#      started inside its Mac's v6 primary period, or ran under 5 minutes, is a
#      protocol deviation; late answers, the two descriptive purposes,
#      insufficient coverage and unlogged share above 0.30 each leave the
#      primary.
#   2. The v6 period start comes from the decision log, and a meta value that
#      disagrees with it is a protocol deviation.
#   3. Missing stays missing: Not now and expiry count in the answer rate's
#      denominator and in no rating distribution.
#   4. Founder, older-format and incomplete exports are dropped whole; a
#      founder export is still parsed as a pipeline check, with no statistic.
#   5. Below 100 primary ratings the default run reports progress by status
#      only: no answer and no feature value.
#   6. The one look is refused until the stop rule is met, and H1 and H2 are
#      withheld below 100 and 200 primary ratings when it is met by time; the
#      look carries the descriptive readout (kappa on primary ratings, offer
#      replies in three groups, the sustained-anchor share by tier).
#   7. The unit is the person: --same-person merges one tester's Macs.
#   8. Synthetic and real exports never mix, and the synthetic-only switches
#      are refused on real exports.
#   9. The config is a closed schema.
#  10. The statistics against hand-computed values, and the ordinal fit
#      against a closed form and finite differences.
#  11. The report refuses words that describe something the analysis is not.
#  12. Two runs on the same exports give the same JSON, and --provenance
#      matches shasum.

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
    "blocks": ("-blocks.csv", ("block_id", "started_at")),
    "decisions": ("-decisions.csv", ("decision_id", "block_id", "policy_version", "block_started_at")),
    "outcomes": ("-outcomes.csv", ("decision_id", "censor_reason", "sustained_anchor_900s")),
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

# The fixtures. V is the v6 period start; ENABLED is when the question was
# turned on. `python3 fixtures.py WEEKS` sets every export's exported_at to
# ENABLED + WEEKS weeks; without it the exports end 3 days after ENABLED.
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
]
p01_blocks = [{"block_id": f"b{i:02d}", "started_at": str(t - 2000)} for i in range(1, 19)]
p01_blocks[10]["started_at"] = str(V + 3 * DAY)          # b11 started inside the period
p01_blocks += [{"block_id": f"d{i}", "started_at": str(ENABLED - (i + 1) * DAY)} for i in range(4)]
# b19 has no start in the blocks file.

# p04: its decision log puts the v6 period start 2 days after the meta's.
c = V + 2 * DAY
p04 = {
    "name": "p04",
    "meta": meta,
    "ratings": [answered("c01", V + 16 * DAY, "most"), answered("c02", V + 21 * DAY, "all")],
    "features": [feature("c01"), feature("c02", card_branch="stayed")],
    "blocks": [{"block_id": "c01", "started_at": str(V + 15 * DAY)},
               {"block_id": "c02", "started_at": str(V + 20 * DAY)}],
    "decisions": [{"decision_id": "x0", "block_id": "old", "policy_version": "5",
                   "block_started_at": str(V - 5 * DAY)},
                  {"decision_id": "x1", "block_id": "first", "policy_version": "6",
                   "block_started_at": str(c)},
                  {"decision_id": "x2", "block_id": "c02", "policy_version": "6",
                   "block_started_at": str(V + 20 * DAY)}],
    "outcomes": [{"decision_id": "x2", "censor_reason": "none", "sustained_anchor_900s": "1"}],
}

def founder_feature(block):
    return dict(base_feature, block_id=block, card_branch="stayed")

others = [
    {"name": "founder-mac", "meta": meta,
     "ratings": [answered("f1", t, "all"),
                 rating("f2", t, shown_at=str(t + 1), not_now_at=str(t + 2))],
     "features": [founder_feature("f1"), founder_feature("f2")],
     "blocks": []},
    {"name": "p02-old", "meta": dict(meta, export_format="6"), "ratings": [], "features": [], "blocks": []},
    {"name": "p03-incomplete", "meta": meta, "ratings": [], "blocks": []},
]
for spec in [{"name": "p01", "ratings": p01_ratings, "features": p01_features, "meta": meta,
              "blocks": p01_blocks}, p04] + others:
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
# 1-5. The default run below 100 primary ratings.
# ---------------------------------------------------------------------------
"$analyze" --json "$cohort"/*/ > "$work/progress.json" || fail "the default run failed"
check "$work/progress.json" "population rules" <<'PY'
rows = r["rows"]
assert rows["excluded"] == {
    "malformed block-feature row": 1,
    "malformed rating row": 3,
    "no block-feature row": 1,
    "no start for the block in the blocks file": 1,
    "policy_version is not 6 (never pooled)": 1,
    "rating prompt version is not 1": 1,
}, rows["excluded"]
assert rows["protocol_deviations"] == {
    "block shorter than 300 s": 1,
    "rated block started inside the v6 primary period (before day 14)": 2,
    "v6_period_start in the meta disagrees with the decision log (exports)": 1,
}, rows["protocol_deviations"]
p = r["progress"]
assert p["eligible"] == 10, p["eligible"]
assert p["by_state"]["answered"] == 8 and p["by_state"]["not_now"] == 1, p["by_state"]
assert p["by_state"]["expired_never_shown"] == 1, p["by_state"]
assert p["late_answers"] == 1 and p["descriptive_purpose_answers"] == 1, p
assert p["card_population"] == 6, p["card_population"]
assert p["primary_exclusions"] == {
    "coverage class insufficient": 1,
    "unlogged or needs-a-category share above 0.30": 1,
}, p["primary_exclusions"]
assert p["primary"] == 4 and p["primary_testers"] == 2, p
PY
check "$work/progress.json" "whole-export exclusions and the founder pipeline check" <<'PY'
e = r["exports"]
assert e["analysed"] == ["p01", "p04"], e["analysed"]
assert set(e["excluded_whole"]) == {"founder-mac", "p02-old", "p03-incomplete"}, e["excluded_whole"]
assert "older than 7" in e["excluded_whole"]["p02-old"], e["excluded_whole"]
assert "-blockfeatures.csv" in e["excluded_whole"]["p03-incomplete"], e["excluded_whole"]
assert e["founder_pipeline_check"] == {"founder-mac": {"answered": 1, "block_feature_rows": 2, "not_now": 1}}, e
PY
check "$work/progress.json" "below 100 primary ratings, progress by status only" <<'PY'
assert r["mode"] == "progress", r["mode"]
for key in ("readout", "final_look"):
    assert key not in r, key
text = json.dumps(r)
for word in ("work_anchor_share", "by_answer", "by_tier", "most", "little"):
    assert word not in text, word
PY
"$analyze" "$cohort"/*/ > "$work/progress.txt" || fail "the text run failed"
grep -q "PRIMARY ratings: 4 from 2 tester(s)" "$work/progress.txt" || fail "text lacks the primary count"
grep -q "PROTOCOL DEVIATION, rated block started inside the v6 primary period" "$work/progress.txt" \
  || fail "text does not name the deviation"
grep -q "founder pipeline check (n = 1, founder; not a result)" "$work/progress.txt" \
  || fail "text does not label the founder check"
grep -q "Not yet: it is read once there are 100 primary" "$work/progress.txt" \
  || fail "text does not withhold the readout"
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

late="$work/late"
build "$late" 17
"$analyze" --json --final-look "$late"/*/ > "$work/late.json" || fail "the look at 17 weeks failed"
check "$work/late.json" "the look by time, with H1 and H2 withheld and the readout shown" <<'PY'
s = r["stop_rule"]
assert s["met"] and s["met_by_time"] and not s["met_by_count"], s
look = r["final_look"]
assert look["stop_rule_met"] and "refused" not in look, look
assert look["h1"] == "not reported: 4 primary ratings, below 100", look["h1"]
assert look["h2"] == "not reported: 4 primary ratings, below 200", look["h2"]
d = r["readout"]
card = d["card"]
assert card["population"] == 6 and card["table"]["low_coverage"]["good"] == 1, card["table"]
assert card["kappa_rows"] == 4 and card["weighted_kappa"] == 1.0, card
assert d["distribution"]["primary"]["by_tier"] == {"low": 1, "good": 1, "high": 2}, d["distribution"]
assert d["distribution"]["late"]["by_answer"]["half"] == 1, d["distribution"]["late"]
offers = d["offers"]
assert offers["offered_blocks"] == 1 and offers["by_reply"]["was_focused"]["good"] == 1, offers
sustained = d["sustained_anchor"]
assert sustained["exports_with_labels"] == 1 and sustained["exports_without_labels"] == ["p01"], sustained
assert sustained["by_tier"]["high"] == {"blocks": 1, "mean_share": 1.0, "median_share": 1.0}, sustained
rates = d["answer_rate"]
assert rates["overall"] == {"eligible": 10, "answered": 8, "rate": 0.8}, rates["overall"]
assert rates["by_person"]["p04"] == {"eligible": 1, "answered": 1, "rate": 1.0}, rates["by_person"]
macs = {m["mac"]: m for m in d["reactivity"]["macs"]}
assert macs["p01"] == {"mac": "p01", "status": "measured", "before": 5, "after": 17}, macs["p01"]
assert d["person_variation"]["people"] == 2, d["person_variation"]
PY
echo "ok: the one look is refused early, withheld below its minimums, and carries the readout"

# ---------------------------------------------------------------------------
# 7. The unit is the person.
# ---------------------------------------------------------------------------
"$analyze" --json --same-person p04,p01 "$cohort"/*/ > "$work/same.json"
check "$work/same.json" "--same-person did not merge the two Macs" <<'PY'
assert r["exports"]["people"] == {"p01": "p01", "p04": "p01"}, r["exports"]["people"]
assert r["progress"]["primary"] == 4 and r["progress"]["primary_testers"] == 1, r["progress"]
PY
echo "ok: one tester's Macs are one person"

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
PY
expect_status 2 "a config with an extra key was accepted" "$analyze" --config "$work/extra.json" "$cohort/p01/"
expect_status 2 "a config with a missing key was accepted" "$analyze" --config "$work/missing.json" "$cohort/p01/"
expect_status 2 "a config with a mistyped value was accepted" "$analyze" --config "$work/nested.json" "$cohort/p01/"
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

# The longest run is floored at 1 second, not at a share.
row = {c: "" for c in ar.BLOCK_FEATURE_COLUMNS}
row.update(block_id="x", block_feature_version="1", policy_version="6", card_branch="came_back",
           elapsed_seconds="600", covered_seconds="500", coverage_class="good",
           work_anchor_share="0.5", switch_aways_per_covered_hour="1", longest_run_share="0",
           recovery_ratio="0", feed_share="0", unlogged_share="0", offer_delivered="0",
           purpose="study", intensity="light", planned_minutes="25", weekend="0")
parsed, error = ar._parse_features(row)
assert error is None, error
assert abs(ar._model_value(parsed, "log_longest_run_share", 1) - math.log(1 / 600)) < 1e-12

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
PY
echo "ok: statistics, ordinal fit, wording"

# ---------------------------------------------------------------------------
# 12. Determinism and provenance.
# ---------------------------------------------------------------------------
"$analyze" --json --final-look "$late"/*/ > "$work/late-again.json"
cmp -s "$work/late.json" "$work/late-again.json" || fail "two runs on the same exports differ"
"$analyze" --final-look "$late"/*/ > /dev/null || fail "the text report of the look failed"
"$analyze" --provenance > "$work/provenance.json"
check "$work/provenance.json" "--provenance disagrees with shasum" <<PY
import hashlib
assert r["analysis_sha256"] == hashlib.sha256(open("$analyze", "rb").read()).hexdigest()
assert r["config_sha256"] == hashlib.sha256(open("$config", "rb").read()).hexdigest()
PY
echo "ok: determinism and provenance"

echo "analyze_ratings_test.sh: all checks passed"
