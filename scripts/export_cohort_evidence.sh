#!/usr/bin/env bash
# Exports the alpha cohort's evidence, and nothing else.
#
# Work-block state never leaves the device on its own: `work_block`,
# `work_block_observation`, `work_block_intervention` and
# `intervention_decision_log` are local-only by design and no upload path
# touches them. That is the right default, but it means the pre-registered
# outcomes are not measurable without the participant deliberately handing
# them over. This script is that hand-over.
#
# What it emits: up to seven CSV files that share one name stem, each with its
# own row grain. They are kept apart on purpose: one file carrying two grains
# would be pooled by the first person who opened it in a spreadsheet.
#
#   <stem>.csv              one row per recorded intervention
#                           (`work_block_intervention`), with `card_seen_at`.
#                           Two outcomes are terminal at creation and reached no
#                           channel, `delivery_suppressed_dnd` (0020) and
#                           `withheld_demotion` (0023). They are exported like
#                           any other row; `analyze_cohort.py` partitions them
#                           out of the delivered denominator.
#   <stem>-decisions.csv    one row per gate evaluation
#                           (`intervention_decision_log`, migration 0026),
#                           abstentions included. It carries `policy_version`,
#                           which the per-offer rows do not, and it is the
#                           denominator of the replacement primary outcome.
#   <stem>-blocks.csv       one row per declared work block, including every
#                           block that never produced an offer. A cohort where
#                           people declare blocks and nothing fires must not
#                           look like one where nobody declared anything.
#   <stem>-invitations.csv  one row per initiation invitation (migration 0022).
#   <stem>-explain.csv      one row per local week: explain-this-nudge taps
#                           (migration 0025), and the delivered offers and
#                           declared blocks in that week, counted on this Mac.
#   <stem>-corrections.csv  one row per rule scope and broad category: how many
#                           classification rules this Mac holds, and for the
#                           app-scoped ones how many corrections wrote them
#                           (migration 0017). Counts only. No key, no typed
#                           name, no label: nothing that names an app or window.
#   <stem>-meta.csv         what this database could and could not record:
#                           schema version, whether the decision log exists,
#                           when card sightings started being recorded, and
#                           whether invitations are switched on.
#
# A database from an older build lacks some tables or columns. The export still
# runs: a missing column is written as an empty value, and a missing table
# writes no file and is recorded as `absent` in the meta file. The one hard
# stop is a database older than protocol 25, which cannot record the
# "I was focused" reply at all.
#
# What none of the files can emit, by construction: the free-form block
# intention, app names, window titles, URLs, filenames, or any observation rows.
# Every query below names every column it selects; there is no `SELECT *`
# anywhere in this file.
#
# Usage (a pasted or downloaded copy has no execute bit, so call it with bash):
#   bash export_cohort_evidence.sh                  # writes ./velvt-cohort-<date>*.csv
#   bash export_cohort_evidence.sh /tmp/out.csv     # writes /tmp/out.csv, /tmp/out-decisions.csv, ...
#
# Read the files before sending them. They are plain CSV.
#
# This file must run under the bash 3.2 that ships with macOS: no associative
# arrays, no `mapfile`, no `${var,,}`.

set -euo pipefail

DB="${VELVT_DATABASE_PATH:-$HOME/.velvt/velvt-service.sqlite3}"
OUT="${1:-./velvt-cohort-$(date -u +%Y-%m-%d).csv}"
STEM="${OUT%.csv}"
DECISIONS_OUT="$STEM-decisions.csv"
BLOCKS_OUT="$STEM-blocks.csv"
INVITATIONS_OUT="$STEM-invitations.csv"
EXPLAIN_OUT="$STEM-explain.csv"
CORRECTIONS_OUT="$STEM-corrections.csv"
META_OUT="$STEM-meta.csv"

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
# writing to. The live file is only ever read by `cp`.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SNAP="$WORK/snapshot.sqlite"
cp "$DB" "$SNAP"

scalar() { sqlite3 -readonly -noheader -batch "$SNAP" "$1"; }
has_table() {
  [[ "$(scalar "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '$1';")" == "1" ]]
}
has_column() {
  [[ "$(scalar "SELECT COUNT(*) FROM pragma_table_info('$1') WHERE name = '$2';")" == "1" ]]
}
# The epoch second at which migration N was applied on this Mac, or nothing.
# The service records every migration in `schema_migration` as it applies it.
migrated_at() {
  local value=""
  if has_table schema_migration; then
    value="$(scalar "SELECT created_at FROM schema_migration WHERE version = $1;")"
  fi
  if [[ "$value" =~ ^[0-9]+$ ]]; then printf '%s' "$value"; fi
}
# Runs one query and appends its rows, as CSV, under a header written here.
# The header is written by this script rather than by sqlite3's -header, which
# emits nothing at all when the result set is empty: a file with a header and
# zero rows is a result, an empty file reads as a broken script.
write_csv() {
  local path="$1" header="$2" sql="$3"
  printf '%s\n' "$header" > "$path"
  sqlite3 -readonly -noheader -csv "$SNAP" "$sql" >> "$path"
}
rows_in() { echo $(( $(wc -l < "$1") - 1 )); }

# A database written by a build older than protocol 25 has neither `salience`
# nor the `was_focused` outcome, so an export from it would silently omit the
# trust metric rather than report it as zero. Fail loudly instead.
if ! has_column work_block_intervention salience; then
  if has_table work_block_intervention; then
    echo "ERROR: this database predates protocol 25 — no 'salience' column." >&2
    echo "It cannot record the \"I was focused\" reply, so an export would" >&2
    echo "understate the wrong-intervention rate. Update Velvt, use it for a" >&2
    echo "while, then re-run this script." >&2
  else
    echo "ERROR: no work-block data in $DB. Has a work block ever been started?" >&2
  fi
  exit 1
fi

# Companion files from an earlier run in the same folder would otherwise sit
# beside this run's files and be read as part of it. Only this script's own
# output names are removed.
rm -f "$DECISIONS_OUT" "$BLOCKS_OUT" "$INVITATIONS_OUT" "$EXPLAIN_OUT" \
  "$CORRECTIONS_OUT" "$META_OUT"

# ---------------------------------------------------------------------------
# 1. One row per recorded intervention.
#
# `card_seen_at` arrived in migration 0032 (protocol 29). Every 1.0.9 database
# lacks it, and on a database that has it, NULL on a row written before the
# migration means "this Mac was not recording sightings yet", not "unseen".
# `card_seen` states which of the three it is, so nobody downstream has to
# know the migration's date:
#   seen     card_seen_at is set: the in-app card was drawn on screen.
#   unseen   NULL on a row offered after this Mac applied migration 0032.
#   unknown  NULL on a row offered before that, or no such column at all.
# ---------------------------------------------------------------------------
CARD_SEEN_SINCE="$(migrated_at 32)"
if has_column work_block_intervention card_seen_at; then
  CARD_SEEN_AT_SQL="i.card_seen_at"
  if [[ -n "$CARD_SEEN_SINCE" ]]; then
    CARD_SEEN_SQL="CASE
        WHEN i.card_seen_at IS NOT NULL THEN 'seen'
        WHEN i.offered_at >= $CARD_SEEN_SINCE THEN 'unseen'
        ELSE 'unknown'
    END"
  else
    CARD_SEEN_SQL="CASE WHEN i.card_seen_at IS NOT NULL THEN 'seen' ELSE 'unknown' END"
  fi
else
  CARD_SEEN_AT_SQL="NULL"
  CARD_SEEN_SQL="'unknown'"
fi

# `read -d ''` rather than `$(cat <<'SQL')`: bash 3.2 mis-parses a quoted heredoc
# inside a command substitution when the body holds an apostrophe.
IFS= read -r -d '' OFFERS_SQL <<'SQL' || true
SELECT
    i.block_id                                   AS block_id,
    b.purpose                                    AS purpose,
    b.intensity                                  AS intensity,
    b.planned_duration_seconds                   AS planned_duration_seconds,
    b.phase                                      AS block_phase,
    -- Block timing. Without these the offer instant has no denominator: an
    -- offer at t+600 means something different in a 5-minute block than in a
    -- 3-hour one, and DRIFT_MIN_REMAINING_SECONDS is a gate on exactly this
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
    -- at the offer instant and it is not: total_paused_seconds is the
    -- block's FINAL accumulated pause, so a block paused after the offer
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
    -- The 2026-08-09 outcome, RETIRED as primary on 2026-08-21 and kept as a
    -- descriptive figure. The state machine records a return whenever the
    -- anchor category reappears while the offer is unanswered, with no time
    -- bound, so the 10-minute window is applied here rather than in the app.
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
    END                                          AS wrong_intervention,
    @CARD_SEEN_AT@                               AS card_seen_at,
    @CARD_SEEN@                                  AS card_seen
FROM work_block_intervention AS i
JOIN work_block AS b ON b.block_id = i.block_id
ORDER BY i.offered_at, i.block_id;
SQL
OFFERS_SQL="${OFFERS_SQL/@CARD_SEEN_AT@/$CARD_SEEN_AT_SQL}"
OFFERS_SQL="${OFFERS_SQL/@CARD_SEEN@/$CARD_SEEN_SQL}"
# Keep this list in step with the aliases above.
write_csv "$OUT" \
  'block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention,card_seen_at,card_seen' \
  "$OFFERS_SQL"
OFFER_ROWS="$(rows_in "$OUT")"

# ---------------------------------------------------------------------------
# 2. One row per gate evaluation, abstentions included.
#
# `work_block_intervention` holds only the decisions that became an offer or a
# withheld offer; the decision log holds every evaluation, and the replacement
# primary outcome's denominator is that log. A `work_block_intervention` row
# has no policy column, so the analysis attributes each one to the policy
# version of this log's row for the same block. Without this file no row can
# be attributed to a drift policy version, and the 2026-09-25 amendment
# excludes it.
#
# The block columns come from a LEFT JOIN because the log's block_id is
# nullable, and the analysis needs the block's end to apply the warm-up
# exclusion and to censor a horizon that runs past the end of the block.
# ---------------------------------------------------------------------------
DECISION_LOG="absent"
DECISION_ROWS=0
if has_table intervention_decision_log; then
  DECISION_LOG="present"
  write_csv "$DECISIONS_OUT" \
    'decision_id,occurred_at,block_id,policy_version,anchor_category,switch_count,elapsed_seconds,remaining_seconds,gate_verdict,propensity,anchor_seen_within_600s,outcome_at,block_started_at,block_ended_at,block_total_paused_seconds,block_planned_duration_seconds' \
    "SELECT
        d.decision_id,
        d.occurred_at,
        d.block_id,
        d.policy_version,
        d.anchor_category,
        d.switch_count,
        d.elapsed_seconds,
        d.remaining_seconds,
        d.gate_verdict,
        d.propensity,
        -- NULL means the 600-second horizon was never resolved, which is not
        -- the same as 'did not return'. Resolved-and-negative is 0.
        d.anchor_seen_within_600s,
        d.outcome_at,
        b.started_at,
        b.ended_at,
        b.total_paused_seconds,
        b.planned_duration_seconds
     FROM intervention_decision_log AS d
     LEFT JOIN work_block AS b ON b.block_id = d.block_id
     ORDER BY d.occurred_at, d.decision_id;"
  DECISION_ROWS="$(rows_in "$DECISIONS_OUT")"
fi

# ---------------------------------------------------------------------------
# 3. One row per declared work block, whether or not it ever produced an
#    offer. No purpose, no intensity, no intention: how the block started
#    (`origin`, migration 0022), where it ended up, and its timings.
# ---------------------------------------------------------------------------
if has_column work_block origin; then ORIGIN_SQL="b.origin"; else ORIGIN_SQL="NULL"; fi
write_csv "$BLOCKS_OUT" \
  'block_id,origin,phase,started_at,ended_at,total_paused_seconds,planned_duration_seconds' \
  "SELECT
      b.block_id,
      $ORIGIN_SQL,
      b.phase,
      b.started_at,
      b.ended_at,
      b.total_paused_seconds,
      b.planned_duration_seconds
   FROM work_block AS b
   ORDER BY b.started_at, b.block_id;"
BLOCK_ROWS="$(rows_in "$BLOCKS_OUT")"

# ---------------------------------------------------------------------------
# 4. One row per initiation invitation. `local_date` is left out: it exists
#    only to enforce the daily cap, and with `offered_at` beside it it would
#    give away this Mac's time zone.
# ---------------------------------------------------------------------------
INVITATIONS="absent"
INVITATION_ROWS=0
if has_table initiation_invitation; then
  INVITATIONS="present"
  write_csv "$INVITATIONS_OUT" \
    'invitation_id,offered_at,action_id,policy_version,backoff_policy_version,outcome,outcome_at' \
    "SELECT
        n.invitation_id,
        n.offered_at,
        n.action_id,
        n.policy_version,
        n.backoff_policy_version,
        n.outcome,
        n.outcome_at
     FROM initiation_invitation AS n
     ORDER BY n.offered_at, n.invitation_id;"
  INVITATION_ROWS="$(rows_in "$INVITATIONS_OUT")"
fi

# ---------------------------------------------------------------------------
# 5. One row per local week. The probe stores only a tap count per local week
#    (Monday key), never which nudge or when. Its denominator is not stored:
#    the app reads it from `work_block_intervention` through the delivered
#    predicate (outcome not delivery_suppressed_dnd or withheld_demotion), and
#    so does this query, bucketed into the same Monday-to-Monday local weeks on
#    this Mac's clock. Weeks with delivered offers or declared blocks and no
#    taps are listed too: a week with no taps is a zero, not a missing row.
#    Offers and blocks from before this Mac had the probe (migration 0025)
#    are not counted, because no tap could have been recorded for them.
# ---------------------------------------------------------------------------
EXPLAIN_PROBE="absent"
EXPLAIN_ROWS=0
if has_table explain_probe_week; then
  EXPLAIN_PROBE="present"
  PROBE_SINCE="$(migrated_at 25)"
  PROBE_SINCE="${PROBE_SINCE:-0}"
  write_csv "$EXPLAIN_OUT" \
    'week_start_local_date,taps,delivered_interventions,blocks_declared' \
    "WITH
       offers AS (
         SELECT date(i.offered_at, 'unixepoch', 'localtime', '-6 days', 'weekday 1') AS week,
                COUNT(*) AS n
         FROM work_block_intervention AS i
         WHERE i.outcome NOT IN ('delivery_suppressed_dnd', 'withheld_demotion')
           AND i.offered_at >= $PROBE_SINCE
         GROUP BY week),
       blocks AS (
         SELECT date(b.started_at, 'unixepoch', 'localtime', '-6 days', 'weekday 1') AS week,
                COUNT(*) AS n
         FROM work_block AS b
         WHERE b.started_at >= $PROBE_SINCE
         GROUP BY week),
       weeks AS (
         SELECT p.week_start_local_date AS week FROM explain_probe_week AS p
         UNION SELECT o.week FROM offers AS o
         UNION SELECT k.week FROM blocks AS k)
     SELECT
        w.week,
        COALESCE(p.taps, 0),
        COALESCE(o.n, 0),
        COALESCE(k.n, 0)
     FROM weeks AS w
     LEFT JOIN explain_probe_week AS p ON p.week_start_local_date = w.week
     LEFT JOIN offers AS o ON o.week = w.week
     LEFT JOIN blocks AS k ON k.week = w.week
     ORDER BY w.week;"
  EXPLAIN_ROWS="$(rows_in "$EXPLAIN_OUT")"
fi

# ---------------------------------------------------------------------------
# 6. The classification corrections this Mac holds, as counts per rule scope
#    and broad category. This is the 2026-08-17 pre-registered measure 3,
#    "corrections made per participant", which no file carried before.
#
#    app     `personal_app_override` (0017): one row per application the person
#            taught a category, from a correction or from the list of apps
#            Velvt could not read (`set_application_category`, triage).
#            `correction_count` starts at 1 and each later correction
#            that lands on the same application adds 1, so its sum is the
#            number of app-scoped corrections behind the rules that exist now.
#    window  `personal_override` (0007): one row per corrected window. It keeps
#            no count, because correcting the same window again overwrites it,
#            so `corrections` is left empty rather than guessed.
#
#    Only rules that still exist can be counted. A correction the person
#    removed, or a Reset, takes its count with it, so every number here is a
#    lower bound. The block-scoped "Wrong category" reply to a drift offer
#    (`work_block_category_correction`) is not a classification rule and is
#    already in the offers file as outcome `wrong_classification`.
#
#    What is never selected: the key digests, the typed activity name, and
#    every label. `category` is written only when it is one of the eight the
#    service accepts for a correction (`override_label_for_category`,
#    rust-service/src/abstraction/engine.rs, unchanged since 1.0.1); anything
#    else becomes `unrecognized`. The IPC layer already refuses any other
#    value, so this matters only if something wrote the table around it, and
#    then free text still cannot leave in this column.
#
#    A 1.0.0 database (0016) has no app-scoped rules at all, so the measure
#    does not exist there: no file is written and the meta file says `absent`.
# ---------------------------------------------------------------------------
CORRECTIONS="absent"
CORRECTION_ROWS=0
if has_table personal_app_override && has_table personal_override; then
  CORRECTIONS="present"
  write_csv "$CORRECTIONS_OUT" \
    'scope,category,rules,corrections' \
    "WITH rules AS (
       SELECT 'app' AS scope, a.category AS category, a.correction_count AS corrections
         FROM personal_app_override AS a
       UNION ALL
       SELECT 'window' AS scope, w.category AS category, NULL AS corrections
         FROM personal_override AS w)
     SELECT
        r.scope,
        CASE
            WHEN r.category IN ('FOCUS_WORK', 'PASSIVE_CONSUMPTION', 'SOCIAL_FEED',
                                'COMMUNICATION', 'TASK_MANAGEMENT', 'REFERENCE',
                                'SYSTEM', 'UNLOGGED') THEN r.category
            ELSE 'unrecognized'
        END,
        COUNT(*),
        SUM(r.corrections)
     FROM rules AS r
     GROUP BY 1, 2
     ORDER BY 1, 2;"
  CORRECTION_ROWS="$(rows_in "$CORRECTIONS_OUT")"
fi

# ---------------------------------------------------------------------------
# 7. What this database could record. The analysis reads this to tell "the
#    database had no decision log" from "the decision-log file went missing on
#    the way", and to report who switched invitations off.
# ---------------------------------------------------------------------------
SCHEMA_VERSION=""
if has_table schema_migration; then
  SCHEMA_VERSION="$(scalar "SELECT MAX(version) FROM schema_migration;")"
fi
INVITATIONS_ENABLED=""
if has_table initiation_settings; then
  # No row means the default, which is on.
  INVITATIONS_ENABLED="$(scalar "SELECT COALESCE((SELECT s.invitations_enabled FROM initiation_settings AS s WHERE s.id = 1), 1);")"
fi
{
  printf 'key,value\n'
  printf 'export_format,3\n'
  printf 'exported_at,%s\n' "$(date -u +%s)"
  printf 'schema_version,%s\n' "$SCHEMA_VERSION"
  printf 'decision_log,%s\n' "$DECISION_LOG"
  printf 'invitations,%s\n' "$INVITATIONS"
  printf 'explain_probe,%s\n' "$EXPLAIN_PROBE"
  printf 'corrections,%s\n' "$CORRECTIONS"
  printf 'card_seen_recorded_since,%s\n' "$CARD_SEEN_SINCE"
  printf 'invitations_enabled,%s\n' "$INVITATIONS_ENABLED"
} > "$META_OUT"

# ---------------------------------------------------------------------------
# What the participant reads before sending anything.
# ---------------------------------------------------------------------------
cat <<SUMMARY

Wrote $OFFER_ROWS intervention record(s) to:
  $OUT
Wrote $BLOCK_ROWS work block(s) to:
  $BLOCKS_OUT
SUMMARY

if [[ "$DECISION_LOG" == "present" ]]; then
  cat <<SUMMARY
Wrote $DECISION_ROWS recorded decision(s) to:
  $DECISIONS_OUT
SUMMARY
fi
if [[ "$INVITATIONS" == "present" ]]; then
  cat <<SUMMARY
Wrote $INVITATION_ROWS invitation(s) to:
  $INVITATIONS_OUT
SUMMARY
fi
if [[ "$EXPLAIN_PROBE" == "present" ]]; then
  cat <<SUMMARY
Wrote $EXPLAIN_ROWS week(s) of explain-tap counts to:
  $EXPLAIN_OUT
SUMMARY
fi
if [[ "$CORRECTIONS" == "present" ]]; then
  cat <<SUMMARY
Wrote $CORRECTION_ROWS row(s) of correction counts to:
  $CORRECTIONS_OUT
SUMMARY
fi
cat <<SUMMARY
Wrote what this Mac's Velvt could record to:
  $META_OUT
SUMMARY

if [[ "$DECISION_LOG" == "absent" ]]; then
  cat <<'NOLOG'

This Velvt is too old to keep a decision log, so no decisions file was
written. Every time the detector looked and stayed quiet went unrecorded on
this Mac. Say so when you send the files.
NOLOG
fi

if (( OFFER_ROWS == 0 )); then
  cat <<'EMPTY'

No offer ever fired on this Mac. That is a result, not a failure: it says the
detector's thresholds were never met here, which is exactly the kind of thing
this cohort is meant to find out. Please send the files anyway. An export with
zero offers still counts, and leaving it out would quietly bias the numbers
toward the people who did get interrupted.
EMPTY
fi

cat <<SUMMARY

Please send every file listed above.

Contains: block id, purpose, intensity, planned duration, block phase, block
start and end times, total time paused, offer time, how much of the block was
left when the offer fired, anchor category, switch count, salience, outcome,
outcome time, and when the nudge card was first on screen. Times are plain
epoch seconds.

The decisions file adds each time the detector looked: when, the broad anchor
category, switch count, elapsed and remaining seconds, the policy version, the
verdict (including every time it chose to stay quiet), and whether you were
back in the anchor category within 10 minutes. The blocks file lists every
session you started, with how it started (by you or from an invitation), how
it ended, and its timings. The invitations file lists each invitation and your
answer. The explain file counts "Explain this nudge" taps per week, next to
the nudges and sessions in that week, with each week named by the date of its
Monday. The corrections file counts the category corrections you have made,
per broad category only: how many apps and how many windows you corrected,
and how many corrections the apps took in total. It does not say which apps
or windows they were.

Does NOT contain: your block intentions, app names, window titles, URLs,
filenames, or anything you typed or read. Open the files and check before
sending.
SUMMARY
