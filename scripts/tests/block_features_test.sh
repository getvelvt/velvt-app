#!/usr/bin/env bash
# The block-features file of export format 7, held to one set of answers by
# every implementation of it.
#
# The question after a block is compared, offline, with a summary of the same
# block. That summary needs the block's observation rows and its stored
# end-of-block result, which never leave the tester's Mac, so the exporter
# computes it there, in SQL, and writes only the summary
# (`<stem>-blockfeatures.csv`). `analyze_cohort.py` restates every definition
# (`block_features`).
#
# 1. Both must give the answers in fixtures/block-feature-vectors.json, which
#    were worked out by hand, one rule per case. The app's own end-of-block
#    result is held to the same vectors in
#    rust-service/src/work_block/block_feature_vectors.rs, so the stored
#    fields the exporter reads and the card branch it derives are the app's.
# 2. Only blocks with a rating row get a summary, and the file carries no
#    category text, no observation time and nothing from the result's
#    sentence.
# 3. Both must agree on a few hundred random blocks: odd categories, every
#    status and confidence, open and zero-length rows, results missing, not
#    JSON or naming a value outside the closed set, decisions under two
#    policies, withheld offers. Agreement is not correctness; the vectors are.
#    It catches the two drifting apart.
# 4. The column lists are one contract: the exporter's headers, this script's
#    and, once it is merged, analyze_ratings.py's.
# 5. The hour bucket and weekend are local: the vectors run again under two
#    other time zones (one on a half hour), where both must give the local
#    answer the vectors' `zones` hold, not the UTC one.
#
# The databases are built by replaying the shipped migrations, up to the
# schema with the question after a block (0048).

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
export_script="$repo_root/scripts/export_cohort_evidence.sh"
analyze="$repo_root/scripts/analyze_cohort.py"
ratings_analysis="$repo_root/scripts/analyze_ratings.py"
vectors="$repo_root/scripts/tests/fixtures/block-feature-vectors.json"
export FIXTURE_MIGRATIONS_DIR="$repo_root/rust-service/migrations"
# shellcheck source=lib/fixture_db.sh
source "$repo_root/scripts/tests/lib/fixture_db.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -f "$vectors" ]] || { echo "ERROR: missing $vectors" >&2; exit 1; }

tester_bash="bash"
if [[ -x /bin/bash ]]; then tester_bash="/bin/bash"; fi
# The hour bucket and weekend are local; the vectors' `expect` is worked out
# in UTC, and section 5 runs them under other zones.
export TZ=UTC

fail() { echo "FAIL: $*" >&2; exit 1; }

INSTALLED_AT=1600000000
# Never selected: written into every stored result's sentence, and into one
# confident observation's category.
S_SENTENCE='ZZSENTINELRESULTSENTENCEZZ'
S_CATEGORY='ZZSENTINELCATEGORYZZ'

# Writes a spec's blocks into a migrated database: the block, its
# observations, its stored result, its decision rows, its offer and a rating
# row. A case may hold `payload_raw` (stored as is), `no_result`, or
# `unrated` (no rating row: it must get no summary).
cat > "$work/seed.py" <<'PY'
import json, sqlite3, sys

db, spec_path, sentence = sys.argv[1], sys.argv[2], sys.argv[3]
cases = json.load(open(spec_path))["cases"]
connection = sqlite3.connect(db)
for number, case in enumerate(cases):
    base = case["base"]
    block_id = f"block-{number:04d}"
    block = case["block"]
    started = base + block["started_at"]
    ended = base + block["ended_at"]
    connection.execute(
        "INSERT INTO work_block (block_id, phase, purpose, intensity, planned_duration_seconds,"
        " started_at, total_paused_seconds, ended_at, intention_expires_at, origin)"
        " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (block_id, block["phase"], block["purpose"], block["intensity"],
         block["planned_duration_seconds"], started, block["total_paused_seconds"], ended,
         ended + 86400, block["origin"]),
    )
    for start, end, category, status, confidence in case["observations"]:
        connection.execute(
            "INSERT INTO work_block_observation (block_id, occurred_at, ended_at, category,"
            " classification_status, classification_confidence) VALUES (?, ?, ?, ?, ?, ?)",
            (block_id, base + start, None if end is None else base + end,
             category, status, confidence),
        )
    if "payload_raw" in case:
        payload = case["payload_raw"]
    elif case.get("no_result"):
        payload = None
    else:
        stored = dict(case["result"])
        stored.update({
            "planned_duration_seconds": block["planned_duration_seconds"],
            "confidence": "medium",
            "observation": sentence,
            "next_action": {"action_id": "protect_next_10",
                            "label": "Protect the next 10 minutes.", "duration_seconds": 600},
        })
        payload = json.dumps(stored)
    if payload is not None:
        connection.execute(
            "INSERT INTO work_block_result (block_id, payload) VALUES (?, ?)", (block_id, payload))
    for index, version in enumerate(case["decisions"]):
        connection.execute(
            "INSERT INTO intervention_decision_log (decision_id, occurred_at, block_id,"
            " policy_version, anchor_category, switch_count, elapsed_seconds, remaining_seconds,"
            " gate_verdict, propensity) VALUES (?, ?, ?, ?, NULL, 0, 60, 600, 'abstained_warmup', 1.0)",
            (f"{block_id}-d{index}", started + 60 + index, block_id, version),
        )
    if case["offer"] is not None:
        connection.execute(
            "INSERT INTO work_block_intervention (block_id, offered_at, action_id, anchor_category,"
            " switch_count, window_seconds, outcome, outcome_at, salience)"
            " VALUES (?, ?, 'protect_next_10', 'FOCUS_WORK', 3, 600, ?, ?, 'normal')",
            (block_id, started + 300, case["offer"], started + 400),
        )
    if not case.get("unrated"):
        connection.execute(
            "INSERT INTO work_block_self_rating (block_id, prompt_version, eligible_at)"
            " VALUES (?, 1, ?)", (block_id, ended))
connection.commit()
PY

# Compares the exporter's file, and block_features, with a spec. With
# `--expect` the vectors' own answers are the reference; with `--agree` the
# two implementations are compared with each other.
cat > "$work/compare.py" <<'PY'
import csv, importlib.util, json, sys

analyze_path, spec_path, features_path, mode = sys.argv[1:5]
module_spec = importlib.util.spec_from_file_location("analyze_cohort", analyze_path)
analyze = importlib.util.module_from_spec(module_spec)
sys.modules["analyze_cohort"] = analyze   # dataclasses look their module up here
module_spec.loader.exec_module(analyze)

cases = json.load(open(spec_path))["cases"]
with open(features_path) as handle:
    reader = csv.DictReader(handle)
    assert tuple(reader.fieldnames) == analyze.BLOCK_FEATURE_COLUMNS, reader.fieldnames
    exported = {row["block_id"]: row for row in reader}

# The exact vectors avoid rounding ties. On random data the two round a tie
# at the sixth decimal differently (SQLite half away from zero, Python half to
# even), so they may differ by one unit there.
TOLERANCE = 1e-9 if mode == "--expect" else 1.01e-6
FLOATS = {"work_anchor_share", "switch_aways_per_covered_hour", "longest_run_share",
          "recovery_ratio", "feed_share", "unlogged_share"}

def same(column, cell, value):
    if value is None:
        return cell == ""
    if column in FLOATS:
        return cell != "" and abs(float(cell) - value) <= TOLERANCE
    if isinstance(value, int):
        return cell != "" and float(cell) == value
    return cell == value

problems = []
rated = set()
for number, case in enumerate(cases):
    block_id = f"block-{number:04d}"
    if case.get("unrated"):
        if block_id in exported:
            problems.append(f"{case['name']}: a block with no rating row got a summary")
        continue
    rated.add(block_id)
    base = case["base"]
    block = dict(case["block"], block_id=block_id,
                 started_at=base + case["block"]["started_at"],
                 ended_at=base + case["block"]["ended_at"])
    observations = [
        dict(occurred_at=base + start, ended_at=None if end is None else base + end,
             category=category, classification_status=status,
             classification_confidence=confidence)
        for start, end, category, status, confidence in case["observations"]
    ]
    if "payload_raw" in case:
        try:
            result = json.loads(case["payload_raw"])
        except ValueError:
            result = None
    else:
        result = None if case.get("no_result") else case["result"]
    restated = analyze.block_features(block, observations, result, case["decisions"], case["offer"])
    row = exported.get(block_id)
    if row is None:
        problems.append(f"{case['name']}: no row in the block-features file")
        continue
    for column in analyze.BLOCK_FEATURE_COLUMNS:
        if mode == "--expect" and column != "block_id":
            want = case["expect"][column]
            if not same(column, row[column], want):
                problems.append(f"{case['name']}: exporter {column}={row[column]!r}, expected {want!r}")
            if restated[column] != want and not (
                column in FLOATS and want is not None and restated[column] is not None
                and abs(restated[column] - want) <= TOLERANCE
            ):
                problems.append(f"{case['name']}: block_features {column}={restated[column]!r}, "
                                f"expected {want!r}")
        elif not same(column, row[column], restated[column]):
            problems.append(f"{case['name']}: exporter {column}={row[column]!r}, "
                            f"block_features {restated[column]!r}")
extra = sorted(set(exported) - rated)
if extra:
    problems.append(f"summaries of blocks nobody rated: {extra[:5]}")
if problems:
    raise SystemExit("\n".join(problems[:40]))
print(f"{len(rated)} block(s) agree")
PY

run_case() { # NAME SPEC MODE
  local name="$1" spec="$2" mode="$3"
  local db="$work/$name.sqlite3" out="$work/$name"
  migrate_fixture_db "$db" "$FIXTURE_MIGRATIONS_RATING" "$INSTALLED_AT"
  python3 "$work/seed.py" "$db" "$spec" "$S_SENTENCE" || fail "$name: seeding failed"
  mkdir -p "$out"
  VELVT_DATABASE_PATH="$db" "$tester_bash" "$export_script" "$out/export.csv" \
    > "$out/stdout.txt" 2>&1 || fail "$name: exporter exited non-zero:$(printf '\n')$(cat "$out/stdout.txt")"
  [[ -f "$out/export-blockfeatures.csv" ]] || fail "$name: no block-features file"
  python3 "$work/compare.py" "$analyze" "$spec" "$out/export-blockfeatures.csv" "$mode" \
    || fail "$name: block features disagree"
}

# ===========================================================================
# 1. The hand-worked vectors, plus one block nobody rated (2), and one with
#    the sentinel category, which must leave no trace (2).
# ===========================================================================
python3 - "$vectors" "$work/vectors.json" "$S_CATEGORY" <<'PY'
import copy, json, sys
spec = json.load(open(sys.argv[1]))
unrated = copy.deepcopy(spec["cases"][1])
unrated.update(name="unrated", unrated=True)
spec["cases"].append(unrated)
private = copy.deepcopy(spec["cases"][0])
private["name"] = "private_category"
private["observations"].append([1500, 1600, sys.argv[3], "classified", "high"])
# The sentinel category is confident and not work: it moves the shares, so
# the case is compared with block_features rather than with the vector.
private["expect"] = None
spec["cases"].append(private)
json.dump(spec, open(sys.argv[2], "w"))
PY
python3 - "$work/vectors.json" <<'PY'
import json, sys
spec = json.load(open(sys.argv[1]))
spec["cases"] = [c for c in spec["cases"] if c["expect"] is not None or c.get("unrated")]
json.dump(spec, open(sys.argv[1] + ".expect", "w"))
PY
run_case vectors "$work/vectors.json.expect" --expect
run_case private "$work/vectors.json" --agree

# ===========================================================================
# 2. Data minimization: no summary without a rating row (checked in both runs
#    above), and nothing private in the file.
# ===========================================================================
features="$work/private/export-blockfeatures.csv"
grep -qF -- "$S_CATEGORY" "$features" && fail "a category's text reached the block-features file"
grep -qF -- "$S_SENTENCE" "$work/private"/export*.csv && fail "the result's sentence was exported"
[[ "$(sqlite3 "$work/private.sqlite3" "SELECT COUNT(*) FROM work_block_observation WHERE category = '$S_CATEGORY';")" == "1" ]] \
  || fail "seeding failed, the category check would be vacuous"
[[ "$(sqlite3 "$work/private.sqlite3" "SELECT COUNT(*) FROM work_block_result WHERE payload LIKE '%$S_SENTENCE%';")" -ge 1 ]] \
  || fail "seeding failed, the sentence check would be vacuous"
# Every vector time is 1.8e9 or so: a 10-digit number in the file would be
# an observation or block time.
grep -qE '[0-9]{10}' "$features" && fail "an epoch time is in the block-features file"

# The vectors themselves: every case names its rule, and every closed-set
# value the file can hold is pinned by at least one case.
python3 - "$vectors" <<'PY' || fail "the vector file is incomplete"
import json, sys
cases = json.load(open(sys.argv[1]))["cases"]
names = [c["name"] for c in cases]
assert len(names) == len(set(names)), "duplicate case names"
assert all(c.get("rule") for c in cases), "a case without its rule"
expects = [c["expect"] for c in cases]
assert {e["card_branch"] for e in expects} == {"stayed", "came_back", "ended_elsewhere", "low_coverage"}
assert {e["card_anchor_is_work"] for e in expects} == {0, 1, None}
# A card about something other than work, in more than one branch.
assert {e["card_branch"] for e in expects if e["card_anchor_is_work"] == 0} >= {"stayed", "came_back"}
assert {e["coverage_class"] for e in expects} == {"insufficient", "partial", "good"}
assert {e["hour_bucket"] for e in expects} == {"night", "morning", "afternoon", "evening"}
assert {e["weekend"] for e in expects} == {0, 1}
assert {e["offer_delivered"] for e in expects} == {0, 1}
assert None in {e["policy_version"] for e in expects}
assert None in {e["purpose"] for e in expects}
# Every case has a local answer in each zone, and each zone moves at least
# one bucket and one weekend flag off its UTC value.
zones = json.load(open(sys.argv[1]))["zones"]
assert len(zones) >= 2, zones
for zone, answers in zones.items():
    assert sorted(answers) == sorted(names), (zone, sorted(set(names) ^ set(answers)))
    utc = {c["name"]: [c["expect"]["hour_bucket"], c["expect"]["weekend"]] for c in cases}
    assert any(answers[n][0] != utc[n][0] for n in names), zone
    assert any(answers[n][1] != utc[n][1] for n in names), zone
PY

# ===========================================================================
# 3. The two implementations agree on random blocks.
# ===========================================================================
python3 - "$work/random.json" <<'PY'
import json, random, sys

rng = random.Random(20261017)
CATEGORIES = ["FOCUS_WORK"] * 4 + ["COMMUNICATION", "REFERENCE", "TASK_MANAGEMENT",
              "SOCIAL_FEED", "PASSIVE_CONSUMPTION", "SYSTEM", "UNLOGGED", "UNCLASSIFIED",
              "focus_work", "social_feed", "Odd Label"]
EVIDENCE = [("classified", "high")] * 5 + [("classified", "medium")] * 2 + [
    ("classified", "low"), ("ambiguous", "low"), ("ambiguous", "medium"), ("unclassified", "none")]
OUTCOMES = [None] * 4 + ["returned", "accepted_action", "not_helpful", "wrong_classification",
                         "was_focused", "dismissed", "no_response", "delivery_suppressed_dnd",
                         "withheld_demotion"]
cases = []
for number in range(300):
    base = 1790000000 + number * 7919
    t = 0
    rows = []
    for _ in range(rng.randint(0, 14)):
        if rng.random() < 0.1:
            t += rng.randint(1, 600)                  # a pause
        start = t
        duration = rng.choice([0, 1, 59, 60, 300]) if rng.random() < 0.3 else rng.randint(0, 900)
        status, confidence = rng.choice(EVIDENCE)
        rows.append([start, start + duration, rng.choice(CATEGORIES), status, confidence])
        t = start + duration
    if rows and rng.random() < 0.15:
        rows[-1][1] = None                            # one row left open, at most
    paused = rng.choice([0, 0, 0, rng.randint(1, 900)])
    ended = t + paused + rng.choice([0, 0, rng.randint(1, 600)])
    if ended - paused <= 0:
        ended = paused + rng.randint(1, 300)
    elapsed = max(0, ended - paused)
    covered = rng.randint(0, elapsed) if rng.random() < 0.9 else elapsed
    ratio = min(1.0, covered / elapsed) if elapsed else 0.0
    switch = rng.choice([0, 0, 1, 2, 3, 7])
    result = {
        "elapsed_duration_seconds": elapsed,
        "coverage": "insufficient" if ratio < 0.25 else "partial" if ratio < 0.75 else "good",
        "coverage_ratio": ratio,
        "switch_away_count": switch,
        "recovery_count": rng.randint(0, switch),
        "longest_uninterrupted_seconds": rng.randint(0, max(1, elapsed)),
        "safe_evidence_category": None if ratio < 0.25 or rng.random() < 0.05 else
                                  rng.choice(["FOCUS_WORK", "REFERENCE", "PASSIVE_CONSUMPTION",
                                              "SOCIAL_FEED", "focus_work", "Odd Label"]),
    }
    if rng.random() < 0.04:
        result["coverage"] = "weird"
    if rng.random() < 0.04:
        del result[rng.choice(sorted(result))]
    case = {
        "name": f"random-{number}", "base": base,
        "block": {"started_at": 0, "ended_at": ended, "total_paused_seconds": paused,
                  "phase": rng.choice(["completed", "abandoned", "expired"]),
                  "planned_duration_seconds": rng.randint(300, 10800),
                  "purpose": rng.choice([None, "deep_work", "study", "creative_practice",
                                         "healthy_tech_use", "work_life_boundary"]),
                  "intensity": rng.choice(["light", "medium", "intense"]),
                  "origin": rng.choice(["manual", "invitation"])},
        "observations": rows,
        "result": result,
        "decisions": rng.choice([[], [6], [6, 6], [5, 6], [5], [6, 6, 6]]),
        "offer": rng.choice(OUTCOMES),
    }
    roll = rng.random()
    if roll < 0.04:
        case["no_result"] = True
    elif roll < 0.07:
        case["payload_raw"] = "{not json"
    elif roll < 0.1:
        case["unrated"] = True
    cases.append(case)
json.dump({"cases": cases}, open(sys.argv[1], "w"))
PY
run_case random "$work/random.json" --agree
python3 - "$work/random/export-blockfeatures.csv" <<'PY' || fail "the random corpus did not reach every branch"
import csv, sys
from collections import Counter
rows = list(csv.DictReader(open(sys.argv[1])))
branches = Counter(r["card_branch"] for r in rows)
assert all(branches[b] >= 5 for b in ("stayed", "came_back", "ended_elsewhere", "low_coverage")), branches
assert branches[""] >= 3, branches                               # no result, or not JSON
assert Counter(r["coverage_class"] for r in rows)["unrecognized"] >= 3
assert Counter(r["policy_version"] for r in rows)[""] >= 20
anchor = Counter(r["card_anchor_is_work"] for r in rows)
assert anchor["0"] >= 20 and anchor["1"] >= 20 and anchor[""] >= 20, anchor
assert {r["hour_bucket"] for r in rows} == {"night", "morning", "afternoon", "evening"}
assert any(r["offer_delivered"] == "0" and r["offer_outcome"] == "" for r in rows)
assert any(float(r["unlogged_share"] or 0) > 0 for r in rows)
PY

# ===========================================================================
# 4. One contract for the column lists.
# ===========================================================================
python3 - "$analyze" "$export_script" "$ratings_analysis" <<'PY' || fail "the format 7 column lists drifted apart"
import ast, re, sys
from pathlib import Path

def tuples(path):
    tree = ast.parse(Path(path).read_text())
    found = {}
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            name = getattr(node.targets[0], "id", None)
            if name in ("RATINGS_COLUMNS", "BLOCK_FEATURE_COLUMNS"):
                found[name] = tuple(ast.literal_eval(node.value))
    return found

analysed = tuples(sys.argv[1])
exporter = Path(sys.argv[2]).read_text()
headers = re.findall(r"write_csv \"\$(RATINGS_OUT|BLOCKFEATURES_OUT)\" \\\n\s+'([^']+)'", exporter)
written = {kind: tuple(header.split(",")) for kind, header in headers}
assert written["RATINGS_OUT"] == analysed["RATINGS_COLUMNS"], written
assert written["BLOCKFEATURES_OUT"] == analysed["BLOCK_FEATURE_COLUMNS"], written
ratings = Path(sys.argv[3])
if ratings.is_file():
    contract = tuples(ratings)
    assert contract == analysed, (contract, analysed)
    print("analyze_ratings.py reads the same columns")
else:
    print("analyze_ratings.py is not on this branch; its columns are checked once it is")
PY

# ===========================================================================
# 5. Local time. The same vectors under two other zones: the exporter's
#    `localtime` and block_features' time.localtime must both follow TZ, the
#    stand-in here for the tester's Mac's zone. Kolkata is on a half hour.
# ===========================================================================
for zone in America/Los_Angeles Asia/Kolkata; do
  tag="$(printf '%s' "$zone" | tr '/A-Z' '-a-z')"
  python3 - "$work/vectors.json.expect" "$zone" "$work/vectors-$tag.json" <<'PY'
import json, sys
spec = json.load(open(sys.argv[1]))
answers = spec["zones"][sys.argv[2]]
moved = 0
for case in spec["cases"]:
    if case["expect"] is None or case.get("unrated"):
        continue
    bucket, weekend = answers[case["name"]]
    moved += [bucket, weekend] != [case["expect"]["hour_bucket"], case["expect"]["weekend"]]
    case["expect"].update(hour_bucket=bucket, weekend=weekend)
assert moved, "this zone moves nothing, so it cannot tell local time from UTC"
json.dump(spec, open(sys.argv[3], "w"))
PY
  ( export TZ="$zone"; run_case "vectors-$tag" "$work/vectors-$tag.json" --expect )
done

echo "block_features_test.sh: OK"
