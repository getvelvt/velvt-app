#!/usr/bin/env bash
# The probe's one hard rule is negative: it must never read `label`, which
# holds strings like `communication:slack` on a real Mac. Everything else here
# checks that the arithmetic is what it says it is, against a fixture whose
# transition matrix is countable by hand.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
probe="$repo_root/scripts/antecedent_probe.py"
migrations="$repo_root/rust-service/migrations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$probe" ]] || { echo "ERROR: $probe is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

db="$work/velvt-service.sqlite3"
sqlite3 "$db" "CREATE TABLE schema_migration (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version INTEGER NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);"
for migration in "$migrations"/*.sql; do
  sqlite3 "$db" < "$migration" || fail "migration failed: $migration"
done

# A hand-countable sequence. The label column carries a sentinel on every row,
# so any read of it shows up immediately in the output.
#
#   FOCUS_WORK, FOCUS_WORK, COMMUNICATION, FOCUS_WORK, REFERENCE, REFERENCE,
#   FOCUS_WORK, UNLOGGED, COMMUNICATION
#
# Collapsing runs, the transitions are:
#   FOCUS_WORK->COMMUNICATION   1
#   COMMUNICATION->FOCUS_WORK   1
#   FOCUS_WORK->REFERENCE       1
#   REFERENCE->FOCUS_WORK       1
#   FOCUS_WORK->UNLOGGED        1
#   UNLOGGED->COMMUNICATION     1
# Six transitions from nine events.
S_LABEL='ZZSENTINELLABELZZ'
i=0
for spec in "FOCUS_WORK 0" "FOCUS_WORK 60" "COMMUNICATION 120" "FOCUS_WORK 180" \
            "REFERENCE 240" "REFERENCE 300" "FOCUS_WORK 360" "UNLOGGED 420" \
            "COMMUNICATION 480"; do
  set -- $spec
  category="$1"
  offset="$2"
  i=$((i + 1))
  sqlite3 "$db" "INSERT INTO raw_event_buffer (event_id, stable_id, label,
      category, taxonomy_version, occurred_at, classification_tier,
      classification_status, classification_confidence, classification_source)
    VALUES ('evt-$i','stable-$i','$S_LABEL:$category','$category','mvp-1',
      $((1800000000 + offset)),'exact_match','classified','high','seed');"
done

"$probe" --db "$db" > "$work/report.txt" 2>&1 || fail "probe exited non-zero"
"$probe" --db "$db" --json > "$work/report.json" 2>&1 || fail "probe --json exited non-zero"

# ---------------------------------------------------------------------------
# 1. The rule. The sentinel from `label` must appear nowhere.
# ---------------------------------------------------------------------------
if grep -qF -- "$S_LABEL" "$work/report.txt" || grep -qF -- "$S_LABEL" "$work/report.json"; then
  fail "the probe read the label column"
fi
count="$(sqlite3 "$db" "SELECT COUNT(*) FROM raw_event_buffer WHERE label LIKE '%$S_LABEL%';")"
[[ "$count" == "9" ]] || fail "the sentinel was not in the database; the check is vacuous"

# And the guard is real, not decorative: a query naming `label` must be
# refused before it reaches SQLite.
python3 - "$probe" <<'PY' || fail "the forbidden-column guard does not fire"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("probe", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

for bad in (
    "SELECT label FROM raw_event_buffer;",
    "SELECT local_name_suggestion FROM raw_event_buffer;",
    "SELECT * FROM raw_event_buffer;",
    "SELECT category FROM work_block_intervention;",
):
    try:
        module.guard(bad)
    except module.ForbiddenColumn:
        continue
    raise SystemExit(f"guard accepted: {bad}")

# And it must still accept the queries the probe actually runs.
module.guard("SELECT category, COUNT(occurred_at) FROM raw_event_buffer GROUP BY category;")
module.guard(
    "WITH ordered AS (SELECT category, occurred_at, "
    "LAG(category) OVER (ORDER BY occurred_at, rowid) AS prev_category "
    "FROM raw_event_buffer) SELECT prev_category, category, COUNT(occurred_at) "
    "FROM ordered GROUP BY 1, 2;"
)
PY

# ---------------------------------------------------------------------------
# 2. The arithmetic, against a matrix that can be counted by hand.
# ---------------------------------------------------------------------------
python3 - "$work/report.json" <<'PY' || fail "transition arithmetic"
import json, sys
result = json.load(open(sys.argv[1]))
matrix = {(r["from"], r["to"]): r["count"] for r in result["transitions"]}
expected = {
    ("FOCUS_WORK", "COMMUNICATION"): 1,
    ("COMMUNICATION", "FOCUS_WORK"): 1,
    ("FOCUS_WORK", "REFERENCE"): 1,
    ("REFERENCE", "FOCUS_WORK"): 1,
    ("FOCUS_WORK", "UNLOGGED"): 1,
    ("UNLOGGED", "COMMUNICATION"): 1,
}
assert matrix == expected, f"expected {expected}, got {matrix}"
assert sum(matrix.values()) == 6, sum(matrix.values())
assert result["provenance"]["events"] == 9, result["provenance"]
assert result["provenance"]["columns_read"] == ["category", "occurred_at"], result["provenance"]
# Runs of one category must not appear as self-transitions.
assert not any(a == b for a, b in matrix), matrix
PY

# ---------------------------------------------------------------------------
# 3. The session gap drops the pair it should and nothing else. Every event
#    above is 60s apart, so a 30s gap leaves no transition standing.
# ---------------------------------------------------------------------------
"$probe" --db "$db" --session-gap 30 --json > "$work/gap.json" 2>&1 || fail "--session-gap failed"
python3 - "$work/gap.json" <<'PY' || fail "session gap arithmetic"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["transitions"] == [], result["transitions"]
meta = result["transition_meta"]
assert meta["session_gap_seconds"] == 30, meta
assert meta["pairs_dropped_by_session_gap"] == 6, meta
PY

# ---------------------------------------------------------------------------
# 4. The label the deck must carry is emitted, and so are the caveats.
# ---------------------------------------------------------------------------
grep -qF "n=1 · founder's own Mac" "$work/report.txt" || fail "the n=1 label is missing"
grep -qF "no outcome variable" "$work/report.txt" || fail "the no-outcome caveat is missing"
grep -qF "this is the instrument, not a result" "$work/report.txt" \
  || fail "the instrument-not-a-result label is missing"
grep -qF "WHAT THIS IS NOT" "$work/report.txt" || fail "the caveat block is missing"
grep -qF "HOUR OF DAY (local)" "$work/report.txt" || fail "the hour-of-day section is missing"

# ---------------------------------------------------------------------------
# 5. An empty buffer is a readable result, not a crash.
# ---------------------------------------------------------------------------
empty="$work/empty.sqlite3"
sqlite3 "$empty" "CREATE TABLE schema_migration (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version INTEGER NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);"
for migration in "$migrations"/*.sql; do sqlite3 "$empty" < "$migration"; done
"$probe" --db "$empty" > "$work/empty.txt" 2>&1 || fail "empty buffer exited non-zero"
grep -qF "No events in raw_event_buffer" "$work/empty.txt" \
  || fail "an empty buffer produced no explanation"

# A missing database is an error with a message.
if "$probe" --db "$work/nope.sqlite3" > "$work/nodb.txt" 2>&1; then
  fail "a missing database exited zero"
fi
grep -qF "no database at" "$work/nodb.txt" || fail "missing-database message is unhelpful"

# ---------------------------------------------------------------------------
# 6. It never writes to the database it read.
# ---------------------------------------------------------------------------
before="$(shasum "$db" | cut -d' ' -f1)"
"$probe" --db "$db" >/dev/null 2>&1
[[ "$before" == "$(shasum "$db" | cut -d' ' -f1)" ]] || fail "the probe modified the database"
[[ ! -f "$db-wal" ]] || fail "the probe left a -wal beside the database"

echo "antecedent_probe_test.sh: OK"
