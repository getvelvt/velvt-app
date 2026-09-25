#!/usr/bin/env bash
# prove_egress.sh — check Velvt's egress ledger and print what it says was sent.
#
# Velvt's helper (velvt-service) writes one row to the `egress_ledger` table
# BEFORE every HTTP request it makes, and does not send a request it could not
# record. Each row holds when, the method and URL, the body's byte count, the
# SHA-256 of the exact body bytes, whether an account token was attached, and
# the hash of the row before it. This script recomputes every hash itself, so
# you do not have to trust the app's own account of its chain. It needs bash,
# sqlite3 and perl, all of which ship with macOS. Nothing is fetched.
#
# What a passing check proves:
#   * No row was edited, and none was removed from the middle or the start
#     except by retention (30 days or 100,000 rows), which leaves a checkpoint
#     naming the last row it removed. Each row's hash covers every field and
#     the previous row's hash, so changing one row breaks every row after it.
#
# What it cannot prove, stated plainly:
#   * That nothing was cut off the END, or that the whole file was not rebuilt
#     by someone with write access to it: the file is on your disk and holds
#     its own anchor. The head hash printed at the bottom is the external
#     anchor: write it down, and a later run that no longer contains that row
#     (before retention would have removed it) means the ledger was rewritten.
#   * What the network saw. DNS lookups and TLS handshakes are not rows, and a
#     request that never connected is still a row, because it is written first.
#   * Requests made by anything other than the helper. The Velvt app's own
#     update check, when a build enables it, is not in this ledger.
#
# Two bodies carry a secret: sign-up and log-in send your password, and a
# token refresh sends a refresh token. For those rows the stored hash is of the
# body with the secret replaced by "[redacted]" (marked "secrets redacted"
# below), so this file never holds a guessable hash of your password. The byte
# count is still what was sent.
#
# To see what WOULD be sent next, byte for byte, without sending it:
#   /Applications/Velvt.app/Contents/Resources/velvt-service --dry-run-egress
# The "body sha256" it prints for a queued batch is the hash this ledger
# records when that batch is sent.
#
# Usage:
#   ./scripts/prove_egress.sh                        # the default database
#   ./scripts/prove_egress.sh /path/to/db.sqlite3    # somewhere else
#   ./scripts/prove_egress.sh --limit 50             # print the newest 50 rows
#   ./scripts/prove_egress.sh --all                  # print every row
#
# Exit codes: 0 the chain is intact, 1 it is broken or could not be read,
# 2 usage.

set -euo pipefail

LIMIT=20
SHOW_ALL=0
DB_ARG=""
GENESIS="0000000000000000000000000000000000000000000000000000000000000000"

usage() {
  sed -n '2,47p' "$0" | sed 's/^# \{0,1\}//'
}

while (( $# )); do
  case "$1" in
    --all)     SHOW_ALL=1; shift ;;
    --limit)   LIMIT="${2:?--limit needs a number}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*)        echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)         DB_ARG="$1"; shift ;;
  esac
done

case "$LIMIT" in ''|*[!0-9]*) echo "--limit must be a number" >&2; exit 2 ;; esac

DB="${DB_ARG:-${VELVT_DATABASE_PATH:-$HOME/.velvt/velvt-service.sqlite3}}"

command -v sqlite3 >/dev/null 2>&1 || {
  echo "ERROR: sqlite3 was not found on PATH. macOS ships one at /usr/bin/sqlite3." >&2
  exit 1
}
perl -MDigest::SHA -e 1 >/dev/null 2>&1 || {
  echo "ERROR: perl with Digest::SHA was not found. macOS ships both at /usr/bin/perl." >&2
  exit 1
}

if [[ ! -f "$DB" ]]; then
  cat >&2 <<NODB
No Velvt database at:
  $DB

Either Velvt has never run on this Mac, or it stores its data somewhere else
(\$VELVT_DATABASE_PATH).
NODB
  exit 1
fi

# Work on a copy, as prove_local.sh does, so nothing here can write to or lock
# Velvt's own file. The write-ahead log is copied too, so a running helper's
# most recent rows are included.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SNAP="$WORK/snapshot.sqlite3"
cp "$DB" "$SNAP"
if [[ -f "$DB-wal" ]]; then cp "$DB-wal" "$SNAP-wal"; fi
if [[ -f "$DB-shm" ]]; then cp "$DB-shm" "$SNAP-shm"; fi

q() { sqlite3 -batch -noheader -separator "$(printf '\t')" "$SNAP" "$1"; }

if ! q "SELECT 1;" >/dev/null 2>&1; then
  echo "ERROR: $DB is not a readable SQLite database." >&2
  exit 1
fi

if [[ -z "$(q "SELECT name FROM sqlite_master WHERE type='table' AND name='egress_ledger';")" ]]; then
  cat >&2 <<NOLEDGER
This database has no egress_ledger table. It was last opened by a Velvt helper
older than the first one that keeps the ledger, so there is nothing to check.
NOLEDGER
  exit 1
fi

# The checkpoint and the rows are read in one statement batch against one copy,
# so a prune cannot land between the two reads. The line each row's hash is
# taken over is built here, in SQL, from the stored columns: the same format
# rust-service/src/egress/mod.rs (`chain_line`) hashes, pinned by a shared test
# vector in scripts/tests/prove_egress_test.sh.
CHAIN_SQL="
SELECT 'anchor', COALESCE(MAX(through_seq), 0),
       COALESCE((SELECT through_hash FROM egress_ledger_checkpoint
                 ORDER BY through_seq DESC LIMIT 1), '$GENESIS'), ''
  FROM egress_ledger_checkpoint;
SELECT seq, prev_hash, entry_hash,
       'velvt-egress-v1|' || seq || '|' || recorded_at || '|' || method || '|' ||
       endpoint || '|' || body_bytes || '|' || body_sha256 || '|' || body_redacted ||
       '|' || bearer || '|' || prev_hash
  FROM egress_ledger ORDER BY seq;
"

VERDICT="$(q "$CHAIN_SQL" | perl -MDigest::SHA=sha256_hex -e '
  my ($expect_seq, $expect_prev, $anchor_seq, $count) = (1, "", 0, 0);
  while (my $row = <STDIN>) {
    chomp $row;
    my ($seq, $prev, $hash, $line) = split /\t/, $row, 4;
    if ($seq eq "anchor") {
      ($anchor_seq, $expect_prev) = ($prev, $hash);
      $expect_seq = $anchor_seq + 1;
      next;
    }
    if ($seq != $expect_seq) {
      print "BROKEN\t$seq\trow $seq follows row " . ($expect_seq - 1) .
            ": a row is missing or out of order\n";
      exit 0;
    }
    if ($prev ne $expect_prev) {
      print "BROKEN\t$seq\trow $seq does not link to the row before it\n";
      exit 0;
    }
    if (sha256_hex($line) ne $hash) {
      print "BROKEN\t$seq\trow $seq does not match its own hash: it was edited\n";
      exit 0;
    }
    ($expect_prev, $expect_seq, $count) = ($hash, $expect_seq + 1, $count + 1);
  }
  print "INTACT\t$count\t$anchor_seq\t" . ($expect_seq - 1) . "\t$expect_prev\n";
')" || VERDICT=""

rule() { printf '%s\n' "----------------------------------------------------------------------"; }

total="$(q "SELECT COUNT(*) FROM egress_ledger;")"
span="$(q "SELECT COALESCE(datetime(MIN(recorded_at), 'unixepoch'), '-') || ' to ' ||
                  COALESCE(datetime(MAX(recorded_at), 'unixepoch'), '-') || ' UTC'
           FROM egress_ledger;")"

printf '\n'
printf 'WHAT VELVT'"'"'S HELPER RECORDED SENDING FROM THIS MAC\n'
printf '======================================================================\n'
printf 'database : %s\n' "$DB"
printf 'read     : from a throwaway copy; the file above is never opened here\n'
printf 'rows     : %s, %s\n' "$total" "$span"
printf '\n'

rule
printf 'BY ENDPOINT (query strings removed)\n'
rule
q "SELECT printf('%-6s %-58s %6d request(s) %10d body bytes',
                 method,
                 CASE WHEN instr(endpoint, '?') > 0 THEN substr(endpoint, 1, instr(endpoint, '?') - 1)
                      ELSE endpoint END,
                 COUNT(*), SUM(body_bytes))
     FROM egress_ledger
     GROUP BY method, CASE WHEN instr(endpoint, '?') > 0 THEN substr(endpoint, 1, instr(endpoint, '?') - 1)
                           ELSE endpoint END
     ORDER BY COUNT(*) DESC;"
printf '\n'

rule
if (( SHOW_ALL )); then
  printf 'EVERY ROW, oldest first\n'
  ROW_FILTER=""
else
  printf 'THE NEWEST %s ROWS, oldest first (--all shows every row)\n' "$LIMIT"
  ROW_FILTER="WHERE seq > (SELECT COALESCE(MAX(seq), 0) FROM egress_ledger) - $LIMIT"
fi
rule
q "SELECT printf('#%d  %s UTC  %s %s', seq, datetime(recorded_at, 'unixepoch'), method, endpoint)
          || char(10) ||
          printf('     %d body bytes  sha256 %s%s%s', body_bytes, body_sha256,
                 CASE bearer WHEN 1 THEN '  account token attached' ELSE '' END,
                 CASE body_redacted WHEN 1 THEN '  secrets redacted before hashing' ELSE '' END)
     FROM egress_ledger $ROW_FILTER ORDER BY seq;"
printf '\n'

rule
IFS=$'\t' read -r status first second third fourth <<<"$VERDICT"
if [[ "$status" == "INTACT" ]]; then
  printf 'CHAIN INTACT: %s row(s) recomputed and linked.\n' "$first"
  if (( second > 0 )); then
    printf 'Rows 1 to %s were removed by retention; the chain is checked from the\n' "$second"
    printf 'checkpoint that names row %s.\n' "$second"
  else
    printf 'Nothing has been removed by retention; the chain starts at row 1.\n'
  fi
  if (( third > 0 )); then
    printf '\nHead: row %s, hash %s\n' "$third" "$fourth"
    printf 'Write the head down. A later run that no longer contains that row, before\n'
    printf 'retention would have removed it, means the ledger was rewritten.\n'
  fi
  rule
  exit 0
fi
if [[ "$status" != "BROKEN" ]]; then
  printf 'COULD NOT CHECK THE CHAIN: the ledger could not be read in full.\n'
  rule
  exit 1
fi
printf 'CHAIN BROKEN at row %s: %s\n' "$first" "$second"
printf 'The ledger was changed after it was written.\n'
rule
exit 1
