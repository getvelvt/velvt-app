#!/usr/bin/env bash
# Contract test for the egress-ledger verifier.
#
# `prove_egress.sh` is the thing a stranger runs to check that Velvt's record of
# what it sent has not been altered. The failure that matters is a verifier that
# says INTACT about a ledger that was changed, so most of this file edits a
# ledger in each way the chain is meant to catch and asserts the verifier names
# the row.
#
# The rows are built here, in shell, from the same line format the Rust service
# hashes. The first row is the shared test vector that
# `egress::tests::chain_line_matches_the_shared_test_vector` pins in Rust, so if
# either side changes the format, one of the two tests fails.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
prove="$repo_root/scripts/prove_egress.sh"
migrations="$repo_root/rust-service/migrations"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -x "$prove" ]] || { echo "ERROR: $prove is not executable" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

GENESIS="0000000000000000000000000000000000000000000000000000000000000000"
sha() { printf '%s' "$1" | shasum -a 256 | cut -d' ' -f1; }

new_db() {
  local db="$1"
  sqlite3 "$db" "CREATE TABLE schema_migration (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      version INTEGER NOT NULL UNIQUE,
      name TEXT NOT NULL,
      created_at INTEGER NOT NULL DEFAULT (unixepoch())
  );"
  for migration in "$migrations"/*.sql; do
    sqlite3 "$db" < "$migration" || fail "migration failed: $migration"
  done
}

# append DB SEQ RECORDED_AT METHOD URL BODY REDACTED BEARER PREV -> prints the entry hash
append() {
  local db="$1" seq="$2" at="$3" method="$4" url="$5" body="$6" redacted="$7" bearer="$8" prev="$9"
  local bytes body_sha line hash
  bytes="$(printf '%s' "$body" | wc -c | tr -d ' ')"
  body_sha="$(sha "$body")"
  line="velvt-egress-v1|$seq|$at|$method|$url|$bytes|$body_sha|$redacted|$bearer|$prev"
  hash="$(sha "$line")"
  sqlite3 "$db" "INSERT INTO egress_ledger (seq, recorded_at, method, endpoint, body_bytes,
      body_sha256, body_redacted, bearer, prev_hash, entry_hash)
    VALUES ($seq, $at, '$method', '$url', $bytes, '$body_sha', $redacted, $bearer,
      '$prev', '$hash');" || fail "could not append row $seq"
  printf '%s' "$hash"
}

# ---------------------------------------------------------------------------
# 1. The shared vector: the shell and Rust agree on the hashed line.
# ---------------------------------------------------------------------------
vector_line="velvt-egress-v1|1|1800000000|POST|https://api.example.test/v1/events/batches|16|$(sha '{"batch_id":"b"}')|0|1|$GENESIS"
[[ "$(sha "$vector_line")" == "d6fd72f7470223ada172c5b57c615365750251795539350b1a1a8d70adad0a5b" ]] \
  || fail "the shared test vector no longer hashes to the value rust-service pins"

build_chain() {
  local db="$1" h1 h2
  new_db "$db"
  h1="$(append "$db" 1 1800000000 POST https://api.example.test/v1/events/batches '{"batch_id":"b"}' 0 1 "$GENESIS")"
  [[ "$h1" == "d6fd72f7470223ada172c5b57c615365750251795539350b1a1a8d70adad0a5b" ]] \
    || fail "row 1 is not the shared test vector"
  h2="$(append "$db" 2 1800000060 POST https://api.example.test/v1/auth/login '{"email":"a@example.test","password":"[redacted]"}' 1 0 "$h1")"
  append "$db" 3 1800000120 GET 'https://api.example.test/v1/insights/daily?date=2027-01-15' '' 0 1 "$h2" >/dev/null
}

# ---------------------------------------------------------------------------
# 2. An intact chain passes, and every row is printed.
# ---------------------------------------------------------------------------
db="$work/intact.sqlite3"
build_chain "$db"
out="$work/intact.txt"
"$prove" "$db" > "$out" 2>&1 || { cat "$out" >&2; fail "an intact chain did not pass"; }
grep -qF "CHAIN INTACT: 3 row(s) recomputed and linked." "$out" || fail "no INTACT verdict"
grep -qF "Nothing has been removed by retention" "$out" || fail "retention state not reported"
head_hash="$(sqlite3 "$db" "SELECT entry_hash FROM egress_ledger WHERE seq = 3;")"
grep -qF "Head: row 3, hash $head_hash" "$out" || fail "the head hash was not printed"
grep -qF "POST https://api.example.test/v1/events/batches" "$out" || fail "row 1 not printed"
grep -qF "secrets redacted before hashing" "$out" || fail "a redacted row was not marked"
grep -qF "account token attached" "$out" || fail "a bearer row was not marked"
grep -qE "^GET +https://api.example.test/v1/insights/daily +1 request" "$out" \
  || fail "the by-endpoint summary did not strip the query string"

"$prove" --limit 1 "$db" > "$work/limit.txt" 2>&1 || fail "--limit run failed"
if grep -qF "#1 " "$work/limit.txt"; then fail "--limit 1 printed an old row"; fi
grep -qF "#3 " "$work/limit.txt" || fail "--limit 1 did not print the newest row"
grep -qF "CHAIN INTACT: 3 row(s)" "$work/limit.txt" || fail "--limit narrowed the check, not just the print"

# The file itself is never written.
checksum_before="$(shasum "$db" | cut -d' ' -f1)"
"$prove" "$db" >/dev/null 2>&1
[[ "$checksum_before" == "$(shasum "$db" | cut -d' ' -f1)" ]] || fail "the database was modified"
[[ ! -f "$db-wal" ]] || fail "a -wal file was left beside the database"

# ---------------------------------------------------------------------------
# 3. The database refuses the easy edits on its own.
# ---------------------------------------------------------------------------
if sqlite3 "$db" "UPDATE egress_ledger SET body_bytes = 1 WHERE seq = 2;" 2>/dev/null; then
  fail "the schema allowed an UPDATE"
fi
if sqlite3 "$db" "DELETE FROM egress_ledger WHERE seq = 2;" 2>/dev/null; then
  fail "the schema allowed a DELETE outside retention"
fi

# ---------------------------------------------------------------------------
# 4. With the triggers dropped, every kind of edit is named by row.
# ---------------------------------------------------------------------------
expect_broken() {
  local db="$1" want="$2" name="$3" report="$work/$3.txt"
  if "$prove" "$db" > "$report" 2>&1; then
    cat "$report" >&2
    fail "$name: an altered ledger passed"
  fi
  grep -qF "$want" "$report" || { cat "$report" >&2; fail "$name: expected '$want'"; }
}

edited="$work/edited.sqlite3"
build_chain "$edited"
sqlite3 "$edited" "DROP TRIGGER trg_egress_ledger_no_update;
                   UPDATE egress_ledger SET body_bytes = 999 WHERE seq = 2;"
expect_broken "$edited" "CHAIN BROKEN at row 2: row 2 does not match its own hash" edited

rehashed="$work/rehashed.sqlite3"
build_chain "$rehashed"
row2="$(sqlite3 -separator '|' "$rehashed" "SELECT seq, recorded_at, method, endpoint FROM egress_ledger WHERE seq = 2;")"
prev1="$(sqlite3 "$rehashed" "SELECT entry_hash FROM egress_ledger WHERE seq = 1;")"
forged="$(sha "velvt-egress-v1|$row2|5|$(sha 'other')|0|0|$prev1")"
sqlite3 "$rehashed" "DROP TRIGGER trg_egress_ledger_no_update;
  UPDATE egress_ledger SET body_bytes = 5, body_sha256 = '$(sha 'other')',
    body_redacted = 0, entry_hash = '$forged' WHERE seq = 2;"
expect_broken "$rehashed" "CHAIN BROKEN at row 3: row 3 does not link to the row before it" rehashed

removed="$work/removed.sqlite3"
build_chain "$removed"
sqlite3 "$removed" "DROP TRIGGER trg_egress_ledger_delete_behind_checkpoint;
                    DELETE FROM egress_ledger WHERE seq = 2;"
expect_broken "$removed" "CHAIN BROKEN at row 3: row 3 follows row 1" removed

head_cut="$work/head_cut.sqlite3"
build_chain "$head_cut"
sqlite3 "$head_cut" "DROP TRIGGER trg_egress_ledger_delete_behind_checkpoint;
                     DELETE FROM egress_ledger WHERE seq = 1;"
expect_broken "$head_cut" "CHAIN BROKEN at row 2: row 2 follows row 0" head_cut

# ---------------------------------------------------------------------------
# 5. Retention: rows removed behind a checkpoint still verify.
# ---------------------------------------------------------------------------
pruned="$work/pruned.sqlite3"
build_chain "$pruned"
through="$(sqlite3 "$pruned" "SELECT entry_hash FROM egress_ledger WHERE seq = 1;")"
sqlite3 "$pruned" "INSERT INTO egress_ledger_checkpoint (through_seq, through_hash, created_at)
                   VALUES (1, '$through', 1800000200);
                   DELETE FROM egress_ledger WHERE seq <= 1;"
"$prove" "$pruned" > "$work/pruned.txt" 2>&1 || { cat "$work/pruned.txt" >&2; fail "a pruned chain did not pass"; }
grep -qF "CHAIN INTACT: 2 row(s)" "$work/pruned.txt" || fail "pruned chain count wrong"
grep -qF "Rows 1 to 1 were removed by retention" "$work/pruned.txt" || fail "pruning not reported"

# ---------------------------------------------------------------------------
# 6. Clean, explained failures.
# ---------------------------------------------------------------------------
if "$prove" "$work/nope.sqlite3" > "$work/nodb.txt" 2>&1; then fail "a missing database exited zero"; fi
grep -qF "No Velvt database at:" "$work/nodb.txt" || fail "missing-database message is unhelpful"

old="$work/old.sqlite3"
sqlite3 "$old" "CREATE TABLE upload_batch (batch_id TEXT);"
if "$prove" "$old" > "$work/old.txt" 2>&1; then fail "a database with no ledger exited zero"; fi
grep -qF "has no egress_ledger table" "$work/old.txt" || fail "no-ledger message is unhelpful"

empty="$work/empty.sqlite3"
new_db "$empty"
"$prove" "$empty" > "$work/empty.txt" 2>&1 || fail "an empty ledger did not pass"
grep -qF "CHAIN INTACT: 0 row(s)" "$work/empty.txt" || fail "an empty ledger was not reported"

echo "prove_egress_test.sh: OK"
