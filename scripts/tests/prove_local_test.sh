#!/usr/bin/env bash
# Contract test for the demo asset.
#
# `prove_local.sh` is shown to a stranger on their own machine, so the only
# failure mode that matters is a column that quietly does not get printed. The
# assertions here are therefore mostly about completeness, not formatting:
# every table in `sqlite_master`, every textual column in it, by name.
#
# It also pins the awkward one. `raw_event_buffer.local_name_suggestion` holds
# raw application names. The script must PRINT them, labelled. A version of
# this script that hid that column would pass a naive eyeball test and fail the
# only test that counts.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
prove="$repo_root/scripts/prove_local.sh"
migrations="$repo_root/rust-service/migrations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -x "$prove" ]] || { echo "ERROR: $prove is not executable" >&2; exit 1; }

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

APPNAME='Cathode Ray Tube Terminal'
INTENTION='finish the grant application'
LONGVALUE="$(python3 -c 'print("L" * 400)')"

sqlite3 "$db" <<SQL
INSERT INTO raw_event_buffer (event_id, stable_id, label, category,
    taxonomy_version, occurred_at, classification_tier, classification_status,
    classification_confidence, classification_source, local_display_label,
    local_name_suggestion)
VALUES
 ('evt-1','stable-1','document:code','FOCUS_WORK','mvp-1',1800000000,
   'fallback','unclassified','none','fallback',NULL,'$APPNAME'),
 ('evt-2','stable-1','document:code','FOCUS_WORK','mvp-1',1800000060,
   'fallback','unclassified','none','fallback',NULL,'$APPNAME'),
 ('evt-3','stable-2','$LONGVALUE','REFERENCE','mvp-1',1800000120,
   'exact_match','classified','high','seed','Browser',NULL);

INSERT INTO work_block (block_id, phase, intention, intensity,
    planned_duration_seconds, started_at, intention_expires_at)
VALUES ('block-1','active','$INTENTION','medium',1500,1800000000,1800086400);
SQL

out="$work/report.txt"
"$prove" "$db" > "$out" 2>&1 || fail "prove_local.sh exited non-zero"

# ---------------------------------------------------------------------------
# 1. Completeness. Every table, and every textual column of every table, must
#    appear in the output. Derived from the database, not from a list in this
#    test, so a new migration cannot leave a column invisible.
# ---------------------------------------------------------------------------
missing_tables=""
missing_columns=""
while IFS= read -r table; do
  [[ -n "$table" ]] || continue
  grep -qF -- "== $table " "$out" || missing_tables="$missing_tables $table"
  while IFS='|' read -r _cid cname ctype _rest; do
    [[ -n "${cname:-}" ]] || continue
    case "$(printf '%s' "${ctype:-}" | tr '[:lower:]' '[:upper:]')" in
      *TEXT*|*CHAR*|*CLOB*) : ;;
      *) continue ;;
    esac
    grep -qE "^ +$cname\$" "$out" || missing_columns="$missing_columns $table.$cname"
  done < <(sqlite3 "$db" "PRAGMA table_info(\"$table\");")
done < <(sqlite3 "$db" "SELECT name FROM sqlite_master
                        WHERE type='table' AND name NOT LIKE 'sqlite_%'
                        ORDER BY name;")

[[ -z "$missing_tables" ]]  || fail "tables missing from the report:$missing_tables"
[[ -z "$missing_columns" ]] || fail "text columns missing from the report:$missing_columns"

# ---------------------------------------------------------------------------
# 2. The awkward column is printed, with its disclosure, not hidden.
# ---------------------------------------------------------------------------
grep -qF "local_name_suggestion" "$out" || fail "local_name_suggestion was not listed"
grep -qF "$APPNAME" "$out" || fail "the stored application name was not printed"
grep -qF "RAW APPLICATION NAME" "$out" || fail "the app-name column was printed unlabelled"
grep -qF "$INTENTION" "$out" || fail "the stored work-block intention was not printed"

# Counts, not just presence: 2 non-null of 3 rows, 1 distinct value.
grep -qE "2 non-null, 1 null, 1 distinct" "$out" \
  || fail "local_name_suggestion counts are wrong"

# ---------------------------------------------------------------------------
# 3. Long values are truncated and MARKED as truncated, so nobody reads a cut
#    string as the whole stored value.
# ---------------------------------------------------------------------------
grep -qF "[cut]" "$out" || fail "a 400-character value was not marked as truncated"
if grep -qF "$LONGVALUE" "$out"; then
  fail "a 400-character value was printed whole at the default width"
fi
"$prove" --width 500 "$db" > "$work/wide.txt" 2>&1 || fail "--width run failed"
grep -qF "$LONGVALUE" "$work/wide.txt" || fail "--width 500 did not widen the output"

# ---------------------------------------------------------------------------
# 4. The discipline the script claims for itself.
# ---------------------------------------------------------------------------
if sed 's/[[:space:]]*#.*$//' "$prove" | grep -qiE 'select[[:space:]]+\*'; then
  fail "prove_local.sh contains a SELECT *"
fi
for forbidden in python perl ruby jq node awk; do
  if sed 's/[[:space:]]*#.*$//' "$prove" | grep -qE "(^|[^[:alnum:]_])$forbidden[0-9]?([^[:alnum:]_]|\$)"; then
    fail "prove_local.sh grew a dependency on $forbidden"
  fi
done

# ---------------------------------------------------------------------------
# 5. It never writes to the database it was pointed at, and leaves no -wal.
# ---------------------------------------------------------------------------
before="$(sqlite3 "$db" "SELECT COUNT(*) FROM raw_event_buffer;")"
checksum_before="$(shasum "$db" | cut -d' ' -f1)"
"$prove" "$db" >/dev/null 2>&1
checksum_after="$(shasum "$db" | cut -d' ' -f1)"
after="$(sqlite3 "$db" "SELECT COUNT(*) FROM raw_event_buffer;")"
[[ "$before" == "$after" ]] || fail "row count changed under a read-only script"
[[ "$checksum_before" == "$checksum_after" ]] || fail "the database file was modified"
[[ ! -f "$db-wal" ]] || fail "a -wal file was left beside the participant's database"

# ---------------------------------------------------------------------------
# 6. A missing database is a clean, explained failure, not a stack of errors.
# ---------------------------------------------------------------------------
if "$prove" "$work/nope.sqlite3" >"$work/nodb.txt" 2>&1; then
  fail "a missing database exited zero"
fi
grep -qF "No Velvt database at:" "$work/nodb.txt" || fail "missing-database message is unhelpful"

# ---------------------------------------------------------------------------
# 7. An empty but valid database still produces a report.
# ---------------------------------------------------------------------------
empty="$work/empty.sqlite3"
sqlite3 "$empty" "CREATE TABLE schema_migration (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version INTEGER NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);"
for migration in "$migrations"/*.sql; do sqlite3 "$empty" < "$migration"; done
"$prove" "$empty" > "$work/empty.txt" 2>&1 || fail "empty database run failed"
grep -qF "WHAT VELVT HAS STORED ON THIS MAC" "$work/empty.txt" || fail "no header on an empty database"
grep -qF "empty. Textual columns that exist but hold nothing:" "$work/empty.txt" \
  || fail "empty tables were not reported as empty"

echo "prove_local_test.sh: OK"
