#!/usr/bin/env bash
# Contract test for the participant-facing export.
#
# Two things are being pinned here and they pull in opposite directions.
#
# 1. The export must carry ENOUGH: `started_at`, `ended_at`,
#    `total_paused_seconds` and a derived `remaining_seconds_at_offer`, so the
#    offer instant has a denominator and `DRIFT_MIN_REMAINING_SECONDS` is
#    auditable from the CSV alone.
#
# 2. The export must carry NOTHING ELSE. The script's own disclosure promises
#    a participant that no intention, app name, window title, URL or filename
#    can appear. That promise is tested by seeding every one of those fields
#    with a sentinel string and grepping the produced file for it. A promise
#    in a heredoc that nothing verifies is not a promise.
#
# The database is built by replaying the shipped migrations, so the test moves
# with the schema instead of pinning a hand-written copy of it.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
export_script="$repo_root/scripts/export_cohort_evidence.sh"
analyze="$repo_root/scripts/analyze_cohort.py"
migrations="$repo_root/rust-service/migrations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -f "$export_script" ]] || { echo "ERROR: missing $export_script" >&2; exit 1; }

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

# ---------------------------------------------------------------------------
# Sentinels. Every one of these is a string the export promises cannot appear
# in its output. They are seeded into the exact columns that really hold that
# kind of text on a participant's Mac.
# ---------------------------------------------------------------------------
S_INTENTION='ZZSENTINELINTENTIONZZ'
S_APPNAME='ZZSENTINELAPPNAMEZZ'
S_DISPLAY='ZZSENTINELDISPLAYLABELZZ'
S_LABEL='ZZSENTINELEVENTLABELZZ'
S_MAPNAME='ZZSENTINELMAPDISPLAYNAMEZZ'
S_OVERRIDE='ZZSENTINELOVERRIDENAMEZZ'
S_URLHOST='ZZSENTINELURLHOSTZZ'

# Block 1: a plain 3000s block, offer at +600, no pause.
#   remaining = 3000 - (600 - 0) = 2400
# Block 2: a 3000s block with 300s of pause, offer at +900.
#   remaining = 3000 - (900 - 300) = 2400
# Block 3: an offer past the planned end, to prove the clamp.
#   remaining = MAX(0, 3000 - 3600) = 0
sqlite3 "$db" <<SQL
INSERT INTO work_block (block_id, phase, intention, purpose, intensity,
    planned_duration_seconds, started_at, total_paused_seconds, ended_at,
    intention_expires_at)
VALUES
 ('block-1','completed','$S_INTENTION deep dive','deep_work','medium',
   3000, 1800000000, 0, 1800003000, 1800086400),
 ('block-2','completed','$S_INTENTION with a pause','study','light',
   3000, 1800010000, 300, 1800013300, 1800096400),
 ('block-3','completed','$S_INTENTION overran','deep_work','intense',
   3000, 1800020000, 0, 1800023000, 1800106400);

INSERT INTO work_block_intervention (block_id, offered_at, action_id,
    anchor_category, switch_count, window_seconds, outcome, outcome_at, salience)
VALUES
 ('block-1', 1800000600, 'protect_next_10', 'FOCUS_WORK', 4, 600,
   'returned', 1800000900, 'normal'),
 ('block-2', 1800010900, 'protect_next_10', 'FOCUS_WORK', 5, 600,
   'delivery_suppressed_dnd', 1800010900, 'normal'),
 ('block-3', 1800023600, 'protect_next_10', 'REFERENCE', 6, 600,
   'withheld_demotion', 1800023600, 'normal');

-- Observation rows exist and must not be exported at all.
INSERT INTO work_block_observation (block_id, occurred_at, ended_at, category,
    classification_status, classification_confidence)
VALUES ('block-1', 1800000010, 1800000400, 'FOCUS_WORK', 'classified', 'high');

-- Every local-only text column that really holds raw identity on a Mac.
INSERT INTO raw_event_buffer (event_id, stable_id, label, category,
    taxonomy_version, occurred_at, classification_tier, classification_status,
    classification_confidence, classification_source, local_display_label,
    local_name_suggestion)
VALUES ('evt-1','stable-1','communication:$S_LABEL','COMMUNICATION','mvp-1',
    1800000100,'exact_match','classified','high','seed','$S_DISPLAY','$S_APPNAME');

INSERT INTO abstraction_map (key_hash, stable_id, label, category,
    taxonomy_version, display_name)
VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1','stable-1','communication:chat','COMMUNICATION','mvp-1','$S_MAPNAME');

INSERT INTO personal_app_override (app_key_hash, category, activity_name)
VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2','FOCUS_WORK','$S_OVERRIDE');

INSERT INTO personal_override (key_hash, category, activity_name)
VALUES ('ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc3','REFERENCE','$S_URLHOST');
SQL

out="$work/export.csv"
VELVT_DATABASE_PATH="$db" "$export_script" "$out" > "$work/stdout.txt" 2>&1 \
  || fail "exporter exited non-zero:$(printf '\n')$(cat "$work/stdout.txt")"

# ---------------------------------------------------------------------------
# 1. The header is exactly the new column list, in order.
# ---------------------------------------------------------------------------
expected_header='block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention'
actual_header="$(head -1 "$out")"
[[ "$actual_header" == "$expected_header" ]] || {
  echo "expected: $expected_header" >&2
  echo "actual:   $actual_header" >&2
  fail "header drifted from the SELECT aliases"
}

# The header and the data rows must agree on field count, or every downstream
# column read is off by one.
python3 - "$out" <<'PY' || fail "header/row arity mismatch"
import csv, sys
rows = list(csv.reader(open(sys.argv[1])))
width = len(rows[0])
for index, row in enumerate(rows[1:], start=2):
    if len(row) != width:
        raise SystemExit(f"row {index}: {len(row)} fields, header has {width}")
PY

# ---------------------------------------------------------------------------
# 2. The new columns carry the right arithmetic.
# ---------------------------------------------------------------------------
python3 - "$out" <<'PY' || fail "derived remaining_seconds_at_offer is wrong"
import csv, sys
rows = {r["block_id"]: r for r in csv.DictReader(open(sys.argv[1]))}
expected = {
    # (started_at, ended_at, total_paused_seconds, remaining_seconds_at_offer)
    "block-1": ("1800000000", "1800003000", "0",   "2400"),
    "block-2": ("1800010000", "1800013300", "300", "2400"),
    "block-3": ("1800020000", "1800023000", "0",   "0"),
}
problems = []
for block, (started, ended, paused, remaining) in expected.items():
    row = rows.get(block)
    if row is None:
        problems.append(f"{block}: missing from export")
        continue
    got = (row["started_at"], row["ended_at"], row["total_paused_seconds"],
           row["remaining_seconds_at_offer"])
    if got != (started, ended, paused, remaining):
        problems.append(f"{block}: expected {(started, ended, paused, remaining)}, got {got}")
if problems:
    raise SystemExit("\n".join(problems))
PY

# ---------------------------------------------------------------------------
# 3. The privacy promise. Not one sentinel may appear anywhere in the file —
#    not in a row, not in the header, not in a trailing byte.
# ---------------------------------------------------------------------------
for sentinel in "$S_INTENTION" "$S_APPNAME" "$S_DISPLAY" "$S_LABEL" \
                "$S_MAPNAME" "$S_OVERRIDE" "$S_URLHOST"; do
  if grep -qF -- "$sentinel" "$out"; then
    echo "--- offending export ---" >&2
    cat "$out" >&2
    fail "sentinel leaked into the export: $sentinel"
  fi
done

# The same sentinels must not reach the participant's terminal either.
for sentinel in "$S_INTENTION" "$S_APPNAME" "$S_DISPLAY" "$S_LABEL"; do
  if grep -qF -- "$sentinel" "$work/stdout.txt"; then
    fail "sentinel leaked into the script's own output: $sentinel"
  fi
done

# The sentinels really were in the database — otherwise the greps above pass
# for the wrong reason and this test proves nothing.
for probe in \
  "SELECT COUNT(*) FROM work_block WHERE intention LIKE '%$S_INTENTION%';" \
  "SELECT COUNT(*) FROM raw_event_buffer WHERE local_name_suggestion = '$S_APPNAME';" \
  "SELECT COUNT(*) FROM abstraction_map WHERE display_name = '$S_MAPNAME';"; do
  count="$(sqlite3 "$db" "$probe")"
  [[ "$count" -ge 1 ]] || fail "seeding failed, the leak test would be vacuous: $probe"
done

# ---------------------------------------------------------------------------
# 4. The disclosure the participant reads must name the new columns, or the
#    export carries fields the consent text never mentioned.
# ---------------------------------------------------------------------------
# The disclosure is hard-wrapped, so match against a whitespace-folded copy —
# otherwise the assertion breaks every time someone rewraps a paragraph, and a
# test that breaks on reflow gets deleted rather than fixed.
folded="$(tr '\n' ' ' < "$work/stdout.txt" | tr -s ' ')"
for phrase in "block start and end times" "total time paused" \
              "how much of the block was left when the offer fired" \
              "Times are plain epoch seconds."; do
  grep -qF -- "$phrase" <<<"$folded" || fail "disclosure omits: $phrase"
done
grep -qF "Does NOT contain: your block intentions, app names, window titles, URLs, filenames, or anything you typed or read." \
  <<<"$folded" || fail "the negative disclosure went missing"

# ---------------------------------------------------------------------------
# 5. The analyser reads what the exporter writes, including the two withheld
#    rows, without dropping anything.
# ---------------------------------------------------------------------------
cp "$out" "$work/tester-roundtrip.csv"
"$analyze" --json "$work/tester-roundtrip.csv" > "$work/analysis.json"
python3 - "$work/analysis.json" <<'PY' || fail "exporter/analyser round trip"
import json, sys
result = json.load(open(sys.argv[1]))
assert result["data_quality"]["malformed"] == [], result["data_quality"]
assert result["decisions_recorded"] == {
    "definition": result["decisions_recorded"]["definition"],
    "total": 3, "delivered": 1, "withheld": 2,
}, result["decisions_recorded"]
assert result["primary_outcome"]["denominator"] == 1, result["primary_outcome"]
assert result["withheld"]["by_outcome"] == {
    "delivery_suppressed_dnd": 1, "withheld_demotion": 1
}, result["withheld"]
PY

# ---------------------------------------------------------------------------
# 6. There is still no `SELECT *` anywhere in the exporter's executable text.
#    Comments are stripped first — the header prose says the words "SELECT *"
#    in the course of promising there isn't one, and that promise should not
#    be what breaks the test.
# ---------------------------------------------------------------------------
if sed 's/[[:space:]]*#.*$//' "$export_script" | grep -qiE 'select[[:space:]]+\*'; then
  fail "a SELECT * appeared in the exporter"
fi

echo "export_cohort_evidence_test.sh: OK"
