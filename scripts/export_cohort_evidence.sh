#!/usr/bin/env bash
# Exports the alpha cohort's intervention evidence, and nothing else.
#
# Work-block state never leaves the device on its own: `work_block`,
# `work_block_observation`, and `work_block_intervention` are local-only by
# design and no upload path touches them. That is the right default, but it
# means the pre-registered primary outcome — did a bounded, in-the-moment
# intervention change what the person did — is not measurable without the
# participant deliberately handing it over. This script is that hand-over.
#
# What it emits: one row per intervention offer, with safe taxonomy categories
# and timings only.
#
# What it cannot emit, by construction: the free-form block intention, app
# names, window titles, URLs, filenames, or any observation rows. The query
# below names every column it selects; there is no `SELECT *` anywhere in it.
#
# Usage:
#   ./scripts/export_cohort_evidence.sh                 # writes to ./velvt-cohort-<date>.csv
#   ./scripts/export_cohort_evidence.sh /tmp/out.csv    # explicit destination
#
# Read the file before sending it. It is plain CSV.

set -euo pipefail

DB="${VELVT_DATABASE_PATH:-$HOME/.velvt/velvt-service.sqlite3}"
OUT="${1:-./velvt-cohort-$(date -u +%Y-%m-%d).csv}"

if [[ ! -f "$DB" ]]; then
  echo "No Velvt database at $DB." >&2
  echo "Either Velvt has not run on this Mac yet, or it stores data elsewhere." >&2
  exit 1
fi

command -v sqlite3 >/dev/null 2>&1 || {
  echo "ERROR: sqlite3 is required and was not found on PATH." >&2
  exit 1
}

# Read-only, and against a copy: never risk a live database the app may be
# writing to. `immutable=1` also avoids creating -wal/-shm files beside it.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$DB" "$WORK/snapshot.sqlite"

# A database written by a build older than protocol 25 has neither `salience`
# nor the `was_focused` outcome, so an export from it would silently omit the
# trust metric rather than report it as zero. Fail loudly instead.
if ! sqlite3 -readonly "$WORK/snapshot.sqlite" \
     "SELECT salience FROM work_block_intervention LIMIT 1;" >/dev/null 2>&1; then
  if sqlite3 -readonly "$WORK/snapshot.sqlite" \
       "SELECT 1 FROM sqlite_master WHERE type='table' AND name='work_block_intervention';" \
       | grep -q 1; then
    echo "ERROR: this database predates protocol 25 — no 'salience' column." >&2
    echo "It cannot record the \"I was focused\" reply, so an export would" >&2
    echo "understate the wrong-intervention rate. Update Velvt, use it for a" >&2
    echo "while, then re-run this script." >&2
  else
    echo "ERROR: no work-block data in $DB. Has a work block ever been started?" >&2
  fi
  exit 1
fi

# The header is written here rather than by sqlite3's -header, which emits
# nothing at all when the result set is empty. A participant who used Velvt
# but never triggered an offer is a real and wanted data point — the gate
# never firing is one of the outcomes this cohort exists to detect — so their
# export must be a valid CSV with a header and zero rows, not an empty file
# that reads as a broken script. Keep this list in step with the aliases below.
printf '%s\n' \
    'block_id,purpose,intensity,planned_duration_seconds,block_phase,offered_at,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention' \
    > "$OUT"

sqlite3 -readonly -noheader -csv "$WORK/snapshot.sqlite" >> "$OUT" <<'SQL'
SELECT
    i.block_id                                   AS block_id,
    b.purpose                                    AS purpose,
    b.intensity                                  AS intensity,
    b.planned_duration_seconds                   AS planned_duration_seconds,
    b.phase                                      AS block_phase,
    i.offered_at                                 AS offered_at,
    i.action_id                                  AS action_id,
    i.anchor_category                            AS anchor_category,
    i.switch_count                               AS switch_count,
    i.window_seconds                             AS window_seconds,
    i.salience                                   AS salience,
    i.outcome                                    AS outcome,
    i.outcome_at                                 AS outcome_at,
    CASE
        WHEN i.outcome_at IS NULL THEN NULL
        ELSE i.outcome_at - i.offered_at
    END                                          AS seconds_to_outcome,
    -- The pre-registered primary outcome. The state machine records a return
    -- whenever the anchor category reappears while the offer is unanswered,
    -- with no time bound, so the 10-minute window is applied here rather than
    -- in the app. Counting `outcome = 'returned'` alone would overstate it.
    CASE
        WHEN i.outcome = 'returned'
             AND i.outcome_at IS NOT NULL
             AND (i.outcome_at - i.offered_at) <= 600 THEN 1
        ELSE 0
    END                                          AS returned_within_10min,
    -- The trust metric: the offer should not have fired at all.
    CASE
        WHEN i.outcome IN ('was_focused', 'wrong_classification') THEN 1
        ELSE 0
    END                                          AS wrong_intervention
FROM work_block_intervention AS i
JOIN work_block AS b ON b.block_id = i.block_id
ORDER BY i.offered_at;
SQL

ROWS=$(( $(wc -l < "$OUT") - 1 ))

cat <<SUMMARY

Wrote $ROWS intervention record(s) to:
  $OUT
SUMMARY

if (( ROWS == 0 )); then
  cat <<'EMPTY'

No offer ever fired on this Mac. That is a result, not a failure: it says the
detector's thresholds were never met here, which is exactly the kind of thing
this cohort is meant to find out. Please send the file anyway — an export with
zero rows still counts, and leaving it out would quietly bias the numbers
toward the people who did get interrupted.
EMPTY
fi

cat <<SUMMARY

Contains: block id, purpose, intensity, planned duration, offer time, anchor
category, switch count, salience, outcome, and outcome time.

Does NOT contain: your block intentions, app names, window titles, URLs,
filenames, or anything you typed or read. Open it and check before sending.
SUMMARY
