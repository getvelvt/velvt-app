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
# What it emits: TWO files.
#
#   1. One row per DELIVERED intervention, from `work_block_intervention`, with
#      safe taxonomy categories and timings only. Two of those outcomes are
#      terminal at creation and reached no channel —
#      `delivery_suppressed_dnd` (migration 0020) and `withheld_demotion`
#      (migration 0023). They are exported like any other row;
#      `analyze_cohort.py` partitions them out of the delivered denominator.
#   2. One row per RECORDED DECISION, from `intervention_decision_log`
#      (migration 0026), written to `<name>-decisions.csv` beside the first.
#      This is the larger file and the one that matters: the gate writes a row
#      every time it evaluates, including every time it decides to stay silent,
#      and the replacement primary outcome's denominator is those rows and not
#      the delivered ones. Exporting only file 1 discards every abstention,
#      which is most of them.
#
# What neither can emit, by construction: the free-form block intention, app
# names, window titles, URLs, filenames, or any observation rows. Both queries
# below name every column they select; there is no `SELECT *` anywhere in this
# file.
#
# Usage:
#   ./scripts/export_cohort_evidence.sh                 # writes to ./velvt-cohort-<date>.csv
#   ./scripts/export_cohort_evidence.sh /tmp/out.csv    # explicit destination
#
# Read both files before sending them. They are plain CSV.

set -euo pipefail

DB="${VELVT_DATABASE_PATH:-$HOME/.velvt/velvt-service.sqlite3}"
OUT="${1:-./velvt-cohort-$(date -u +%Y-%m-%d).csv}"
# Beside the first file, never inside it: the two have different row grains and
# different denominators, and one CSV carrying both would be pooled by the first
# person who opened it in a spreadsheet.
DECISIONS_OUT="${OUT%.csv}-decisions.csv"

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
    'block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention' \
    > "$OUT"

sqlite3 -readonly -noheader -csv "$WORK/snapshot.sqlite" >> "$OUT" <<'SQL'
SELECT
    i.block_id                                   AS block_id,
    b.purpose                                    AS purpose,
    b.intensity                                  AS intensity,
    b.planned_duration_seconds                   AS planned_duration_seconds,
    b.phase                                      AS block_phase,
    -- Block timing. Without these the offer instant has no denominator: an
    -- offer at t+600 means something different in a 5-minute block than in a
    -- 3-hour one, and `DRIFT_MIN_REMAINING_SECONDS` is a gate on exactly this
    -- quantity. Epoch seconds, no timezone, no local date.
    b.started_at                                 AS started_at,
    b.ended_at                                   AS ended_at,
    b.total_paused_seconds                       AS total_paused_seconds,
    i.offered_at                                 AS offered_at,
    -- Derived, using the shipped gate's own arithmetic: elapsed is wall time
    -- since start minus accumulated pause, clamped at zero, and remaining is
    -- the planned duration minus that, clamped at zero.
    --
    -- Honest caveat, because this column will be read as if it were recorded
    -- at the offer instant and it is not: `total_paused_seconds` is the
    -- block's FINAL accumulated pause, so a block paused *after* the offer
    -- makes this an under-estimate of what the gate actually saw. The three
    -- raw inputs are exported beside it so anyone can recompute or discard it.
    MAX(0, b.planned_duration_seconds
           - MAX(0, MAX(0, i.offered_at - b.started_at) - b.total_paused_seconds)
    )                                            AS remaining_seconds_at_offer,
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

# ---------------------------------------------------------------------------
# The second file. `work_block_intervention` holds only the decisions that
# became a delivered offer; `intervention_decision_log` holds every evaluation
# the gate made, abstentions included, and the pre-registered denominator for
# the replacement primary outcome is that log. An export that carried only the
# first file discarded the abstentions, which is most of the rows and all of
# the ones that say when Velvt chose to stay quiet.
#
# The block columns come from a LEFT JOIN because a decision row can name a
# block that produced no offer, and without the block's end the analysis cannot
# apply its own censoring rule — a decision whose 900-second horizon runs past
# the end of the block is censored, not counted as a failure.
# ---------------------------------------------------------------------------
DECISION_ROWS="not exported"
if sqlite3 -readonly "$WORK/snapshot.sqlite" \
     "SELECT 1 FROM sqlite_master WHERE type='table' AND name='intervention_decision_log';" \
     | grep -q 1; then
  printf '%s\n' \
      'decision_id,occurred_at,block_id,policy_version,anchor_category,switch_count,elapsed_seconds,remaining_seconds,gate_verdict,propensity,anchor_seen_within_600s,outcome_at,block_started_at,block_ended_at,block_planned_duration_seconds' \
      > "$DECISIONS_OUT"
  sqlite3 -readonly -noheader -csv "$WORK/snapshot.sqlite" >> "$DECISIONS_OUT" <<'SQL'
SELECT
    d.decision_id                                AS decision_id,
    d.occurred_at                                AS occurred_at,
    d.block_id                                   AS block_id,
    d.policy_version                             AS policy_version,
    d.anchor_category                            AS anchor_category,
    d.switch_count                               AS switch_count,
    d.elapsed_seconds                            AS elapsed_seconds,
    d.remaining_seconds                          AS remaining_seconds,
    d.gate_verdict                               AS gate_verdict,
    d.propensity                                 AS propensity,
    -- NULL here means the 600-second horizon was never resolved, which is not
    -- the same as "did not return". Resolved-and-negative is 0. Analysis must
    -- not read one as the other.
    d.anchor_seen_within_600s                    AS anchor_seen_within_600s,
    d.outcome_at                                 AS outcome_at,
    b.started_at                                 AS block_started_at,
    b.ended_at                                   AS block_ended_at,
    b.planned_duration_seconds                   AS block_planned_duration_seconds
FROM intervention_decision_log AS d
LEFT JOIN work_block AS b ON b.block_id = d.block_id
ORDER BY d.occurred_at;
SQL
  DECISION_ROWS=$(( $(wc -l < "$DECISIONS_OUT") - 1 ))
fi

cat <<SUMMARY

Wrote $ROWS intervention record(s) to:
  $OUT
SUMMARY

if [[ "$DECISION_ROWS" == "not exported" ]]; then
  cat <<'NOLOG'

This database predates migration 0026, so it has no decision log and no second
file was written. Every abstention the gate made on this Mac is unrecorded --
not missing from the export, absent from the database. Say so when you send it.
NOLOG
else
  cat <<SUMMARY

Wrote $DECISION_ROWS recorded decision(s) to:
  $DECISIONS_OUT
SUMMARY
fi

if (( ROWS == 0 )); then
  cat <<'EMPTY'

No offer ever fired on this Mac. That is a result, not a failure: it says the
detector's thresholds were never met here, which is exactly the kind of thing
this cohort is meant to find out. Please send the files anyway — an export with
zero rows still counts, and leaving it out would quietly bias the numbers
toward the people who did get interrupted. The decisions file will still have
rows in it: the gate recorded every time it looked and chose not to speak.
EMPTY
fi

cat <<SUMMARY

Contains: block id, purpose, intensity, planned duration, block phase, block
start and end times, total time paused, offer time, how much of the block was
left when the offer fired, anchor category, switch count, salience, outcome,
and outcome time. Times are plain epoch seconds.

The decisions file contains: a decision id, when the gate evaluated, the block
it belonged to, the policy version, the broad anchor category, switch count,
elapsed and remaining seconds, the verdict — including every verdict that means
Velvt looked and chose to stay quiet — the propensity, the resolved outcome
flag, and the same three block timings.

Does NOT contain: your block intentions, app names, window titles, URLs,
filenames, or anything you typed or read. Open it and check before sending.
That applies to both files.
SUMMARY
