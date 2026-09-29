#!/usr/bin/env bash
# The per-decision outcome labels, held to one set of answers twice.
#
# The primary outcome of 2026-08-21 (at least 600 of the 900 seconds after a
# decision point in the anchor category) and its two secondary outcomes need
# the observation ledger, which never leaves the tester's Mac. So the exporter
# computes them there, in SQL, and writes only the labels
# (`<stem>-outcomes.csv`). `analyze_cohort.py` restates the definition a
# second at a time (`label_decision`).
#
# 1. Both must give the answers in fixtures/outcome-label-vectors.json, which
#    were worked out by hand from the definition, one rule per case, and
#    say why each observer_gap point is unobserved (`observer_gap_cause`).
# 2. Both must agree with each other on a few hundred random ledgers with
#    gaps, overlaps, open rows, unconfident rows and odd categories. Agreement
#    is not correctness; the vectors are. It catches the two drifting apart.
# 3. The labels file must carry nothing but the labels: no observation time,
#    and no category text outside the eight the service writes.
#
# The databases are built by replaying the shipped migrations, as the other
# measurement tests do, up to the policy v5 schema.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
export_script="$repo_root/scripts/export_cohort_evidence.sh"
analyze="$repo_root/scripts/analyze_cohort.py"
vectors="$repo_root/scripts/tests/fixtures/outcome-label-vectors.json"
export FIXTURE_MIGRATIONS_DIR="$repo_root/rust-service/migrations"
# shellcheck source=lib/fixture_db.sh
source "$repo_root/scripts/tests/lib/fixture_db.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -f "$vectors" ]] || { echo "ERROR: missing $vectors" >&2; exit 1; }

tester_bash="bash"
if [[ -x /bin/bash ]]; then tester_bash="/bin/bash"; fi

fail() { echo "FAIL: $*" >&2; exit 1; }

INSTALLED_AT=1600000000

# Writes the blocks, observations and decisions of a spec into a migrated
# database. The spec is either the vector file or the random corpus below.
cat > "$work/seed.py" <<'PY'
import json, sqlite3, sys

db, spec_path = sys.argv[1], sys.argv[2]
cases = json.load(open(spec_path))["cases"]
connection = sqlite3.connect(db)
for number, case in enumerate(cases):
    base = case["base"]
    block_id = f"vector-block-{number}"
    block = case["block"]
    started = base + block["started_at"]
    ended = None if block.get("ended_at") is None else base + block["ended_at"]
    paused_at = None if block.get("paused_at") is None else base + block["paused_at"]
    planned = block.get("planned_duration_seconds")
    if planned is None:
        span = (block["ended_at"] - block["started_at"]) if ended is not None else 3000
        planned = max(300, min(10800, span - block.get("total_paused_seconds", 0)))
    connection.execute(
        "INSERT INTO work_block (block_id, phase, purpose, intensity, planned_duration_seconds,"
        " started_at, paused_at, total_paused_seconds, ended_at, recovered_after_restart,"
        " intention_expires_at, origin)"
        " VALUES (?, ?, 'deep_work', 'medium', ?, ?, ?, ?, ?, ?, ?, 'manual')",
        (block_id, block["phase"], planned, started, paused_at,
         block.get("total_paused_seconds", 0), ended, block.get("recovered_after_restart", 0),
         started + 86400),
    )
    for start, end, category, status, confidence in case["observations"]:
        connection.execute(
            "INSERT INTO work_block_observation (block_id, occurred_at, ended_at, category,"
            " classification_status, classification_confidence) VALUES (?, ?, ?, ?, ?, ?)",
            (block_id, base + start, None if end is None else base + end,
             category, status, confidence),
        )
    for decision in case["decisions"]:
        at = base + decision["at"]
        elapsed = at - started
        connection.execute(
            "INSERT INTO intervention_decision_log (decision_id, occurred_at, block_id,"
            " policy_version, anchor_category, switch_count, elapsed_seconds,"
            " remaining_seconds, gate_verdict, propensity) VALUES (?, ?, ?, 5, ?, 3, ?, ?, ?, 1.0)",
            (decision["id"], at, block_id, decision["anchor"], elapsed,
             max(0, planned - elapsed), decision["verdict"]),
        )
connection.commit()
PY

# Compares the exporter's file, and label_decision, with a spec. With
# `--expect` the spec's own answers are the reference; without it, the two
# implementations are compared with each other.
cat > "$work/compare.py" <<'PY'
import csv, importlib.util, json, sys

analyze_path, spec_path, outcomes_path, meta_path, mode = sys.argv[1:6]
module_spec = importlib.util.spec_from_file_location("analyze_cohort", analyze_path)
analyze = importlib.util.module_from_spec(module_spec)
sys.modules["analyze_cohort"] = analyze   # dataclasses look their module up here
module_spec.loader.exec_module(analyze)

cases = json.load(open(spec_path))["cases"]
meta = {r["key"]: r["value"] for r in csv.DictReader(open(meta_path))}
exported_at = int(meta["exported_at"])
with open(outcomes_path) as handle:
    reader = csv.DictReader(handle)
    assert tuple(reader.fieldnames) == analyze.OUTCOME_COLUMNS, reader.fieldnames
    exported = {row["decision_id"]: row for row in reader}

LABELS = analyze.OUTCOME_COLUMNS[1:]

def cell(value):
    return "" if value is None else str(value)

problems = []
expected_ids = set()
checked = 0
for case in cases:
    base = case["base"]
    block = case["block"]
    ended = None if block.get("ended_at") is None else base + block["ended_at"]
    observations = [
        dict(occurred_at=base + start, ended_at=None if end is None else base + end,
             category=category, classification_status=status,
             classification_confidence=confidence)
        for start, end, category, status, confidence in case["observations"]
    ]
    for decision in case["decisions"]:
        did = decision["id"]
        if decision["anchor"] is None:
            if did in exported:
                problems.append(f"{case['name']}/{did}: a decision with no anchor got a row")
            if mode == "--expect" and decision.get("expect") is not None:
                problems.append(f"{case['name']}/{did}: a vector expects labels for no anchor")
            continue
        expected_ids.add(did)
        reference = analyze.label_decision(
            base + decision["at"], decision["anchor"], ended, exported_at, observations,
            block_phase=block["phase"],
            block_total_paused_seconds=block.get("total_paused_seconds", 0),
            block_recovered_after_restart=bool(block.get("recovered_after_restart", 0)),
        )
        reference = {key: cell(reference[key]) for key in LABELS}
        reference["censor_reason"] = reference["censor_reason"] or "none"
        got = exported.get(did)
        if got is None:
            problems.append(f"{case['name']}/{did}: no row in the outcomes file")
            continue
        got = {key: got[key] for key in LABELS}
        if mode == "--expect":
            want = {key: cell(decision["expect"][key]) for key in LABELS}
            if got != want:
                problems.append(f"{case['name']}/{did}: exporter {got}, expected {want}")
            if reference != want:
                problems.append(f"{case['name']}/{did}: label_decision {reference}, expected {want}")
        elif got != reference:
            problems.append(f"{case['name']}/{did}: exporter {got}, label_decision {reference}")
        checked += 1

extra = sorted(set(exported) - expected_ids)
if extra:
    problems.append(f"rows for decisions nobody asked about: {extra[:5]}")
if problems:
    raise SystemExit("\n".join(problems[:40]))
print(f"{checked} decision(s) agree")
PY

run_case() { # NAME SPEC MODE
  local name="$1" spec="$2" mode="$3"
  local db="$work/$name.sqlite3" out="$work/$name"
  migrate_fixture_db "$db" "$FIXTURE_MIGRATIONS_POLICY_V5" "$INSTALLED_AT"
  python3 "$work/seed.py" "$db" "$spec" || fail "$name: seeding failed"
  mkdir -p "$out"
  VELVT_DATABASE_PATH="$db" "$tester_bash" "$export_script" "$out/export.csv" \
    > "$out/stdout.txt" 2>&1 || fail "$name: exporter exited non-zero:$(printf '\n')$(cat "$out/stdout.txt")"
  [[ -f "$out/export-outcomes.csv" ]] || fail "$name: no outcomes file"
  python3 "$work/compare.py" "$analyze" "$spec" "$out/export-outcomes.csv" \
    "$out/export-meta.csv" "$mode" || fail "$name: labels disagree"
}

# ===========================================================================
# 1. The hand-worked vectors.
# ===========================================================================
run_case vectors "$vectors" --expect

# Nothing but labels left: no observation time (every vector time is at least
# 1.7e9, so any 10-digit number in the file would be one), and the category
# text nothing may carry is not there.
if grep -qE '[0-9]{10}' "$work/vectors/export-outcomes.csv"; then
  fail "an epoch time is in the outcomes file"
fi
grep -qiF "ZZ Private Label" "$work/vectors/export-outcomes.csv" \
  && fail "a category outside the eight left in departure_category"
[[ "$(sqlite3 "$work/vectors.sqlite3" "SELECT COUNT(*) FROM work_block_observation WHERE category = 'ZZ Private Label';")" == "1" ]] \
  || fail "seeding failed, the category check would be vacuous"

# The vectors themselves: every case names its rule, and every label column is
# pinned by at least one case with a value and one without.
python3 - "$vectors" <<'PY' || fail "the vector file is incomplete"
import json, sys
cases = json.load(open(sys.argv[1]))["cases"]
names = [c["name"] for c in cases]
assert len(names) == len(set(names)), "duplicate case names"
assert all(c.get("why") for c in cases), "a case without its reason"
expects = [d["expect"] for c in cases for d in c["decisions"] if d["expect"] is not None]
reasons = {e["censor_reason"] for e in expects}
assert reasons == {"none", "block_ended", "observer_gap", "export_ended"}, reasons
causes = {e["observer_gap_cause"] for e in expects if e["censor_reason"] == "observer_gap"}
assert causes == {"pause", "final_dwell", "other"}, causes
assert all(e["observer_gap_cause"] is None for e in expects if e["censor_reason"] != "observer_gap")
for key in ("sustained_anchor_900s", "departure_free_600s"):
    assert {e[key] for e in expects if e["censor_reason"] == "none"} == {0, 1}, key
returns = [e["seconds_to_sustained_return"] for e in expects if e["censor_reason"] == "none"]
assert None in returns and 0 in returns, returns
assert any(e["departure_category"] is None for e in expects)
assert any(e["departure_category"] == "unrecognized" for e in expects)
PY

# ===========================================================================
# 2. The two implementations agree on random ledgers.
# ===========================================================================
python3 - "$work/random.json" <<'PY'
import json, random, sys

rng = random.Random(20260927)
CATEGORIES = ["FOCUS_WORK"] * 5 + ["COMMUNICATION", "SOCIAL_FEED", "REFERENCE",
              "PASSIVE_CONSUMPTION", "SYSTEM", "UNLOGGED", "focus_work", "Odd Label"]
EVIDENCE = [("classified", "high")] * 6 + [("classified", "medium")] * 2 + [
    ("ambiguous", "low"), ("unclassified", "none"), ("classified", "low")]
DURATIONS = [0, 1, 5, 30, 60, 120, 200, 299, 300, 301, 450, 599, 600, 601, 900]
cases = []
for number in range(240):
    kind = rng.random()
    future = 0.85 <= kind < 0.93
    base = (4100000000 if future else 1690000000) + number * 20000
    t = 0
    rows = []
    for _ in range(rng.randint(1, 28)):
        if rng.random() < 0.08:
            t += rng.randint(1, 400)                  # a pause or an outage
        start = t
        if rows and rng.random() < 0.05:
            start = max(0, t - rng.randint(1, 200))   # a report out of order
        duration = rng.choice(DURATIONS) if rng.random() < 0.5 else rng.randint(0, 700)
        status, confidence = rng.choice(EVIDENCE)
        rows.append([start, start + duration, rng.choice(CATEGORIES), status, confidence])
        t = max(t, start + duration)
    if kind < 0.7:
        block = {"phase": "completed", "started_at": 0,
                 "ended_at": t + rng.choice([0, 0, rng.randint(1, 300)])}
        if rng.random() < 0.3:
            rows[-1][1] = rows[-1][0]                 # the final dwell, unmeasured
    elif kind < 0.93:
        block = {"phase": "active", "started_at": 0, "ended_at": None}
        rows[-1][1] = None                            # the dwell still open
    else:
        block = {"phase": "paused", "started_at": 0, "ended_at": None, "paused_at": t}
    # What the ledger does not say, and the gap's cause reads: paused time
    # (any gap above may or may not be one) and a restart.
    if rng.random() < 0.5:
        block["total_paused_seconds"] = rng.randint(1, 400)
    if rng.random() < 0.3:
        block["recovered_after_restart"] = 1
    starts = [row[0] for row in rows]
    decisions = []
    for index in range(rng.randint(1, 6)):
        at = rng.choice(starts) if rng.random() < 0.7 else rng.randint(0, t + 100)
        decisions.append({"id": f"r{number}-{index}", "at": at,
                          "anchor": rng.choice(["FOCUS_WORK", "FOCUS_WORK", "COMMUNICATION",
                                                "REFERENCE", None]),
                          "verdict": "abstained_min_switches"})
    cases.append({"name": f"random-{number}", "base": base, "block": block,
                  "observations": rows, "decisions": decisions})
json.dump({"cases": cases}, open(sys.argv[1], "w"))
PY
run_case random "$work/random.json" --agree
python3 - "$work/random/export-outcomes.csv" <<'PY' || fail "the random corpus did not reach every branch"
import csv, sys
from collections import Counter
rows = list(csv.DictReader(open(sys.argv[1])))
reasons = Counter(r["censor_reason"] for r in rows)
assert all(reasons[k] >= 5 for k in ("none", "block_ended", "observer_gap", "export_ended")), reasons
causes = Counter(r["observer_gap_cause"] for r in rows if r["censor_reason"] == "observer_gap")
assert all(causes[k] >= 5 for k in ("pause", "final_dwell", "other")), causes
assert not any(r["observer_gap_cause"] for r in rows if r["censor_reason"] != "observer_gap")
labelled = [r for r in rows if r["censor_reason"] == "none"]
for key in ("sustained_anchor_900s", "departure_free_600s"):
    assert {r[key] for r in labelled} == {"0", "1"}, key
assert any(r["seconds_to_sustained_return"] for r in labelled)
assert any(r["departure_category"] == "unrecognized" for r in rows)
PY

echo "outcome_labels_test.sh: OK"
