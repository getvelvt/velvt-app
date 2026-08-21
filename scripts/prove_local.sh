#!/usr/bin/env bash
# prove_local.sh — print every piece of text Velvt has stored on this Mac.
#
# Run this on your own machine. You do not have to trust the claim that Velvt
# keeps your activity local and abstracted; you can read the database yourself,
# and this script is the reading. It is deliberately boring: bash and the
# `sqlite3` that ships with macOS, nothing else to install, nothing fetched.
#
# How it works, so you can check that it is not hiding anything:
#
#   1. Every table is discovered from `sqlite_master`. Nothing is hard-coded,
#      so a table added by a future version still shows up here.
#   2. For each table, every column whose declared type is textual is listed
#      BY NAME, and its distinct values are printed. Numbers and timestamps
#      are counted but not enumerated — they cannot carry a window title.
#   3. There is no `SELECT *` in this file. Every column read is named, the
#      same discipline the cohort exporter uses.
#
# It will show you application names. That is not a bug and it is not a leak:
# `raw_event_buffer.local_name_suggestion` holds the raw application name for
# up to seven days so the app can offer you a one-tap rename instead of showing
# you "Unclassified". It is documented in PRIVACY.md, it is redacted from logs,
# and it exists in no upload payload. This script prints it rather than hiding
# it, because a proof that quietly skips the awkward column proves nothing.
#
# Usage:
#   ./scripts/prove_local.sh                        # the default database
#   ./scripts/prove_local.sh /path/to/db.sqlite3    # somewhere else
#   ./scripts/prove_local.sh --all                  # every distinct value
#   ./scripts/prove_local.sh --limit 25 --width 100
#
# Exit codes: 0 read the database, 1 could not.

set -euo pipefail

LIMIT=8
WIDTH=56
SHOW_ALL=0
DB_ARG=""

usage() {
  sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'
}

while (( $# )); do
  case "$1" in
    --all)    SHOW_ALL=1; shift ;;
    --limit)  LIMIT="${2:?--limit needs a number}"; shift 2 ;;
    --width)  WIDTH="${2:?--width needs a number}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*)       echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)        DB_ARG="$1"; shift ;;
  esac
done

case "$LIMIT" in ''|*[!0-9]*) echo "--limit must be a number" >&2; exit 2 ;; esac
case "$WIDTH" in ''|*[!0-9]*) echo "--width must be a number" >&2; exit 2 ;; esac
(( WIDTH >= 8 )) || WIDTH=8

DB="${DB_ARG:-${VELVT_DATABASE_PATH:-$HOME/.velvt/velvt-service.sqlite3}}"

command -v sqlite3 >/dev/null 2>&1 || {
  echo "ERROR: sqlite3 was not found on PATH. macOS ships one at /usr/bin/sqlite3." >&2
  exit 1
}

if [[ ! -f "$DB" ]]; then
  cat >&2 <<NODB
No Velvt database at:
  $DB

Either Velvt has never run on this Mac, or it stores its data somewhere else
(\$VELVT_DATABASE_PATH). Nothing to show, which is itself a fact you can check.
NODB
  exit 1
fi

# Work on a copy. Velvt's own file is never opened by this script, so nothing
# here can write to it, lock it, or leave a -wal beside it. The write-ahead log
# is copied too when present, so a running service's most recent commits are
# included rather than silently missing.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SNAP="$WORK/snapshot.sqlite3"
cp "$DB" "$SNAP"
if [[ -f "$DB-wal" ]]; then cp "$DB-wal" "$SNAP-wal"; fi
if [[ -f "$DB-shm" ]]; then cp "$DB-shm" "$SNAP-shm"; fi

q() { sqlite3 -batch -noheader -separator "$(printf '\x1f')" "$SNAP" "$1"; }

if ! q "SELECT 1;" >/dev/null 2>&1; then
  echo "ERROR: $DB is not a readable SQLite database." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# The columns that can carry something recognisably yours. Every one of these
# is device-local and appears in no upload payload. They are annotated inline
# in the walk below so a reader meets the disclosure at the same moment they
# meet the data, instead of having to hold a footnote in their head.
# ---------------------------------------------------------------------------
annotation_for() {
  case "$1.$2" in
    raw_event_buffer.local_name_suggestion)
      echo "RAW APPLICATION NAME — device-local, 7-day expiry, powers the one-tap rename" ;;
    raw_event_buffer.local_display_label)
      echo "local display string, e.g. Coding or Gmail — derived, but it can name a service" ;;
    abstraction_map.display_name)
      echo "local display name for the activity list" ;;
    personal_override.activity_name|personal_app_override.activity_name)
      echo "a name YOU typed when you corrected a classification" ;;
    work_block.intention)
      echo "the sentence you typed when you started a block — expires after 24h" ;;
    history_cache.payload|insight_cache.payload|weekly_digest.payload|work_block_result.payload)
      echo "a JSON summary Velvt rendered for itself; raise --width to read it whole" ;;
    *) echo "" ;;
  esac
}

is_text_type() {
  case "$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')" in
    *TEXT*|*CHAR*|*CLOB*) return 0 ;;
    *) return 1 ;;
  esac
}

rule() { printf '%s\n' "----------------------------------------------------------------------"; }

bytes="$(wc -c < "$DB" | tr -d ' ')"
tables="$(q "SELECT name FROM sqlite_master
             WHERE type='table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name;")"

printf '\n'
printf 'WHAT VELVT HAS STORED ON THIS MAC\n'
printf '======================================================================\n'
printf 'database : %s\n' "$DB"
printf 'size     : %s bytes\n' "$bytes"
printf 'read     : from a throwaway copy; the file above is never opened here\n'
printf 'shown    : every table from sqlite_master, every textual column by name\n'
if (( SHOW_ALL )); then
  printf 'values   : all distinct values, truncated to %s characters\n' "$WIDTH"
else
  printf 'values   : up to %s distinct values per column, most frequent first,\n' "$LIMIT"
  printf '           truncated to %s characters (--all shows everything)\n' "$WIDTH"
fi
printf '\n'
printf 'Numeric and timestamp columns are counted but not listed: an integer\n'
printf 'cannot hold a window title. Every textual column is listed, including\n'
printf 'the ones that name applications.\n'

# ---------------------------------------------------------------------------
# Inventory first, so the reader knows the shape before the detail.
# ---------------------------------------------------------------------------
printf '\n'
printf 'TABLE INVENTORY\n'
rule
printf '  %-34s %10s %8s %8s\n' "table" "rows" "text" "other"
while IFS= read -r table; do
  [[ -n "$table" ]] || continue
  case "$table" in
    [A-Za-z_]*) : ;;
    *) continue ;;
  esac
  rows="$(q "SELECT COUNT(*) FROM \"$table\";")"
  text_cols=0
  other_cols=0
  while IFS="$(printf '\x1f')" read -r _cid cname ctype _rest; do
    [[ -n "${cname:-}" ]] || continue
    if is_text_type "${ctype:-}"; then
      text_cols=$((text_cols + 1))
    else
      other_cols=$((other_cols + 1))
    fi
  done <<EOF
$(q "PRAGMA table_info(\"$table\");")
EOF
  printf '  %-34s %10s %8s %8s\n' "$table" "$rows" "$text_cols" "$other_cols"
done <<EOF
$tables
EOF

# ---------------------------------------------------------------------------
# The walk. One block per table, one paragraph per textual column.
# ---------------------------------------------------------------------------
printf '\n'
printf 'EVERY TEXTUAL COLUMN, BY NAME\n'
rule

US="$(printf '\x1f')"

while IFS= read -r table; do
  [[ -n "$table" ]] || continue
  case "$table" in
    [A-Za-z_]*) : ;;
    *) continue ;;
  esac

  rows="$(q "SELECT COUNT(*) FROM \"$table\";")"
  columns="$(q "PRAGMA table_info(\"$table\");")"

  text_columns=""
  while IFS="$US" read -r _cid cname ctype _rest; do
    [[ -n "${cname:-}" ]] || continue
    is_text_type "${ctype:-}" || continue
    text_columns="${text_columns}${cname}
"
  done <<EOF
$columns
EOF

  printf '\n'
  printf '  == %s  (%s row%s)\n' "$table" "$rows" "$([[ "$rows" == "1" ]] || printf 's')"

  if [[ -z "$text_columns" ]]; then
    printf '     no textual columns — numbers and timestamps only\n'
    continue
  fi
  if [[ "$rows" == "0" ]]; then
    printf '     empty. Textual columns that exist but hold nothing:\n'
    while IFS= read -r cname; do
      [[ -n "$cname" ]] || continue
      printf '       %s\n' "$cname"
    done <<EOF
$text_columns
EOF
    continue
  fi

  while IFS= read -r cname; do
    [[ -n "$cname" ]] || continue

    stats="$(q "SELECT COUNT(\"$cname\"),
                       COUNT(DISTINCT \"$cname\"),
                       SUM(CASE WHEN \"$cname\" IS NULL THEN 1 ELSE 0 END)
                FROM \"$table\";")"
    non_null="${stats%%$US*}"
    rest="${stats#*$US}"
    distinct="${rest%%$US*}"
    nulls="${rest#*$US}"

    note="$(annotation_for "$table" "$cname")"
    if [[ -n "$note" ]]; then
      printf '     %s\n' "$cname"
      printf '       [%s]\n' "$note"
    else
      printf '     %s\n' "$cname"
    fi
    printf '       %s non-null, %s null, %s distinct\n' \
      "$non_null" "$nulls" "$distinct"

    if [[ "$non_null" == "0" ]]; then
      printf '       (nothing stored)\n'
      continue
    fi

    limit_clause="LIMIT $LIMIT"
    (( SHOW_ALL )) && limit_clause=""

    # Newlines and carriage returns are folded to spaces so one stored value is
    # always one printed line; a value that runs past --width is cut and marked.
    values="$(q "
      SELECT CASE
               WHEN length(v) > $WIDTH THEN substr(v, 1, $WIDTH) || ' [cut]'
               ELSE v
             END,
             n
      FROM (
        SELECT replace(replace(CAST(\"$cname\" AS TEXT), char(10), ' '), char(13), ' ') AS v,
               COUNT(*) AS n
        FROM \"$table\"
        WHERE \"$cname\" IS NOT NULL
        GROUP BY 1
        ORDER BY n DESC, 1 ASC
        $limit_clause
      );")"

    shown=0
    while IFS="$US" read -r value count; do
      [[ -n "${count:-}" ]] || continue
      [[ -n "$value" ]] || value="(empty string)"
      printf '         %8s  %s\n' "x$count" "$value"
      shown=$((shown + 1))
    done <<EOF
$values
EOF

    if (( ! SHOW_ALL )) && [[ "$distinct" -gt "$shown" ]]; then
      printf '         ... %s more distinct value(s) not shown; re-run with --all\n' \
        "$((distinct - shown))"
    fi
  done <<EOF
$text_columns
EOF
done <<EOF
$tables
EOF

# ---------------------------------------------------------------------------
# What leaves. Naming the six uploaded fields beside the local dump is the
# whole point: the contrast is the claim.
# ---------------------------------------------------------------------------
printf '\n'
printf 'WHAT LEAVES THIS MAC\n'
rule
cat <<'LEAVES'
  Everything above is local. When Velvt uploads, one event serialises to
  exactly six fields — one of which is a two-field payload — hand-written in
  rust-service/src/upload/dto.rs so no struct field can be added by accident:

      event_id                  a random per-event id
      occurred_at               when it happened
      abstraction_type          a category-scoped bucket, not your label
      abstraction_type_version  which vocabulary that bucket came from
      classification_tier       how it was classified
      payload                    { duration_seconds, category }

  Not in that list, and not representable in it: the application names printed
  above, the local display labels, the stable ids, any window title, any URL,
  any filename, and the sentence you typed when you started a work block.

  Work-block tables are local-only in a stronger sense — no upload path reads
  them at all. That is why sharing them for the alpha cohort needs its own
  deliberate script (scripts/export_cohort_evidence.sh), and why that script
  names every column it selects too.
LEAVES

printf '\n'
printf 'WHAT THIS DOES AND DOES NOT PROVE\n'
rule
cat <<'CAVEAT'
  It proves what is on disk right now, on this Mac, and it proves it by reading
  it out rather than by asserting it.

  It does not prove what the app sends — for that, watch the network, or read
  the six-field serialiser named above. It does not prove what a future version
  will store. Re-run it after any update; that is the point of it being ten
  seconds of bash rather than a paragraph in a document.
CAVEAT
printf '\n'
