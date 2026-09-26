#!/usr/bin/env python3
"""An instrument, not a result: what a category-transition probe can see.

Reads `raw_event_buffer` on this Mac and prints two things — a LAG-based
category transition matrix and an hour-of-day distribution. That is all. There
is no model here, no significance test, no multiplicity control, and above all
no outcome variable: nothing in `raw_event_buffer` records whether a block was
completed, so nothing here can say a transition predicts anything.

The honest label for any figure produced by this script, verbatim:

    Prototype · n=1 · founder's own Mac · <N> days · no outcome variable —
    this is the instrument, not a result.

Read that as a hard constraint on what may be claimed. One person's eight days
of category switches is a demonstration that the plumbing exists and the
arithmetic is reproducible. It is not evidence about users, about behaviour, or
about whether anyone's routine has structure in it.

Two columns are read: `category` and `occurred_at`. The `label` column is
NEVER read — locally it holds strings like `communication:slack`, which name a
service, and this probe is designed so that a screenshot of its output can be
shown to a stranger without redaction. `local_name_suggestion`,
`local_display_label` and `stable_id` are not read either, for the same
reason. The restriction is enforced below rather than merely intended.

Usage:
    ./scripts/antecedent_probe.py
    ./scripts/antecedent_probe.py --json
    ./scripts/antecedent_probe.py --exclude UNLOGGED,SYSTEM
    ./scripts/antecedent_probe.py --session-gap 1800
    ./scripts/antecedent_probe.py --db /path/to/velvt-service.sqlite3
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import sys
import tempfile
import textwrap
from pathlib import Path

# The only two columns this probe may read. Enforced at runtime against the
# text of every statement it executes, so widening it takes a deliberate edit
# here and not an absent-minded one in a query string.
PERMITTED_COLUMNS = ("category", "occurred_at")

# Columns that exist in the same table and must never be read. `label` is the
# one the plan calls out by name; the rest are listed because they are the
# other ways the same identity could leak into a screenshot.
FORBIDDEN_COLUMNS = (
    "label",
    "local_name_suggestion",
    "local_display_label",
    "stable_id",
    "app_stable_id",
    "event_id",
)

SOURCE_TABLE = "raw_event_buffer"


class ForbiddenColumn(RuntimeError):
    """Raised when a query would read something this probe promised not to."""


def guard(sql: str) -> str:
    """Refuses any statement mentioning a forbidden column, or a `SELECT *`.

    A promise in a docstring is not a promise. This runs on every statement
    before it reaches SQLite.
    """
    lowered = sql.lower()
    if re.search(r"select\s+\*", lowered):
        raise ForbiddenColumn("SELECT * is not permitted; name every column")
    for column in FORBIDDEN_COLUMNS:
        if re.search(rf"\b{re.escape(column)}\b", lowered):
            raise ForbiddenColumn(f"query reads the forbidden column {column!r}")
    # Common table expressions defined inside this same statement are not
    # another source of data; they are this one, reshaped.
    local_names = set(re.findall(r"\b([a-z_][a-z0-9_]*)\s+as\s*\(", lowered))
    for table in re.findall(r"\bfrom\s+([a-z_][a-z0-9_]*)", lowered):
        if table != SOURCE_TABLE and table not in local_names:
            raise ForbiddenColumn(
                f"query reads {table!r}; this probe reads {SOURCE_TABLE} only"
            )
    return sql


def open_snapshot(path: Path, work: Path) -> sqlite3.Connection:
    """Copies the database and reads the copy, so a running service is safe."""
    snapshot = work / "snapshot.sqlite3"
    shutil.copyfile(path, snapshot)
    for suffix in ("-wal", "-shm"):
        sidecar = Path(str(path) + suffix)
        if sidecar.exists():
            shutil.copyfile(sidecar, Path(str(snapshot) + suffix))
    return sqlite3.connect(str(snapshot))


def provenance(connection: sqlite3.Connection) -> dict:
    rows, first, last, days = connection.execute(
        guard(
            f"""
            SELECT COUNT(occurred_at),
                   MIN(occurred_at),
                   MAX(occurred_at),
                   COUNT(DISTINCT date(occurred_at, 'unixepoch', 'localtime'))
            FROM {SOURCE_TABLE};
            """
        )
    ).fetchone()
    first_local = last_local = None
    if rows:
        first_local, last_local = connection.execute(
            guard(
                f"""
                SELECT MIN(datetime(occurred_at, 'unixepoch', 'localtime')),
                       MAX(datetime(occurred_at, 'unixepoch', 'localtime'))
                FROM {SOURCE_TABLE};
                """
            )
        ).fetchone()
    return {
        "table": SOURCE_TABLE,
        "columns_read": list(PERMITTED_COLUMNS),
        "events": rows,
        "first_event_epoch": first,
        "last_event_epoch": last,
        "first_event_local": first_local,
        "last_event_local": last_local,
        "local_days_covered": days,
        "timezone": os.environ.get("TZ") or "system local time",
    }


def categories(connection: sqlite3.Connection, exclude: tuple[str, ...]) -> list[tuple[str, int]]:
    predicate, params = _exclusion(exclude, "category")
    return connection.execute(
        guard(
            f"""
            SELECT category, COUNT(occurred_at) AS n
            FROM {SOURCE_TABLE}
            {predicate}
            GROUP BY category
            ORDER BY n DESC, category ASC;
            """
        ),
        params,
    ).fetchall()


def _exclusion(exclude: tuple[str, ...], column: str) -> tuple[str, list]:
    if not exclude:
        return "", []
    placeholders = ", ".join("?" for _ in exclude)
    return f"WHERE {column} NOT IN ({placeholders})", list(exclude)


def transitions(
    connection: sqlite3.Connection,
    exclude: tuple[str, ...],
    session_gap: int,
) -> tuple[list[tuple[str, str, int]], dict]:
    """Consecutive-in-time category changes, via SQL LAG.

    A "transition" is a change of category between one event and the next in
    time. Consecutive events in the same category are one dwell, not many
    transitions, so `prev <> category` collapses them — otherwise a long
    uninterrupted stretch of one category would dominate the matrix with
    self-loops that mean nothing.

    `session_gap` optionally refuses to count a pair whose events are further
    apart than that many seconds. Overnight, the last event of Tuesday and the
    first of Wednesday are adjacent rows and are not a behavioural transition.
    Default 0 (count every adjacent pair) because that is the definition the
    first published figures used; pass `--session-gap 1800` to see how much of
    the matrix survives a stricter one.
    """
    predicate, params = _exclusion(exclude, "category")
    gap_clause = "AND (? = 0 OR occurred_at - prev_at <= ?)"

    rows = connection.execute(
        guard(
            f"""
            WITH ordered AS (
                SELECT category,
                       occurred_at,
                       LAG(category)    OVER (ORDER BY occurred_at, rowid) AS prev_category,
                       LAG(occurred_at) OVER (ORDER BY occurred_at, rowid) AS prev_at
                FROM {SOURCE_TABLE}
                {predicate}
            )
            SELECT prev_category, category, COUNT(occurred_at) AS n
            FROM ordered
            WHERE prev_category IS NOT NULL
              AND prev_category <> category
              {gap_clause}
            GROUP BY prev_category, category
            ORDER BY n DESC, prev_category ASC, category ASC;
            """
        ),
        params + [session_gap, session_gap],
    ).fetchall()

    dropped_by_gap = 0
    if session_gap:
        total_no_gap = connection.execute(
            guard(
                f"""
                WITH ordered AS (
                    SELECT category,
                           occurred_at,
                           LAG(category) OVER (ORDER BY occurred_at, rowid) AS prev_category
                    FROM {SOURCE_TABLE}
                    {predicate}
                )
                SELECT COUNT(occurred_at) FROM ordered
                WHERE prev_category IS NOT NULL AND prev_category <> category;
                """
            ),
            params,
        ).fetchone()[0]
        dropped_by_gap = total_no_gap - sum(row[2] for row in rows)

    meta = {
        "session_gap_seconds": session_gap,
        "pairs_dropped_by_session_gap": dropped_by_gap,
        "definition": (
            "adjacent-in-time events whose category differs; runs of the same "
            "category are one dwell, not repeated self-transitions"
        ),
    }
    return rows, meta


def hour_of_day(connection: sqlite3.Connection, exclude: tuple[str, ...]) -> list[tuple[str, int]]:
    predicate, params = _exclusion(exclude, "category")
    return connection.execute(
        guard(
            f"""
            SELECT strftime('%H', occurred_at, 'unixepoch', 'localtime') AS hour,
                   COUNT(occurred_at) AS n
            FROM {SOURCE_TABLE}
            {predicate}
            GROUP BY hour
            ORDER BY hour ASC;
            """
        ),
        params,
    ).fetchall()


def render(result: dict, top: int) -> str:
    out: list[str] = []
    add = out.append
    prov = result["provenance"]
    days = prov["local_days_covered"]

    add("CATEGORY TRANSITION PROBE")
    add("=" * 74)
    add(f"Prototype · n=1 · founder's own Mac · {days} days · no outcome variable —")
    add("this is the instrument, not a result.")
    add("")
    add("PROVENANCE")
    add("-" * 74)
    add(f"  table          {prov['table']}")
    add(f"  columns read   {', '.join(prov['columns_read'])}  (never `label`)")
    add(f"  events         {prov['events']}")
    add(f"  local days     {days}")
    add(f"  first event    {prov['first_event_local']} local")
    add(f"  last event     {prov['last_event_local']} local")
    if result["excluded_categories"]:
        add(f"  excluded       {', '.join(result['excluded_categories'])}")
        add("                 removed BEFORE the LAG, so events either side of an")
        add("                 excluded run become adjacent and count as one")
        add("                 transition. Compare against an unfiltered run.")

    add("")
    add("CATEGORY DISTRIBUTION")
    add("-" * 74)
    total_events = sum(count for _, count in result["categories"]) or 1
    for category, count in result["categories"]:
        share = count / total_events
        add(f"  {category:22} {count:8}  {share:6.1%}  {_bar(share, 30)}")

    matrix = result["transitions"]
    meta = result["transition_meta"]
    total = sum(row["count"] for row in matrix)
    add("")
    add("TRANSITION MATRIX (LAG over occurred_at)")
    add("-" * 74)
    for line in textwrap.wrap(meta["definition"] + ".", 70):
        add(f"  {line}")
    if meta["session_gap_seconds"]:
        add(f"  session gap: {meta['session_gap_seconds']}s "
            f"({meta['pairs_dropped_by_session_gap']} adjacent pairs dropped as "
            "not-a-transition)")
    else:
        add("  session gap: none — every adjacent pair counts, including the one")
        add("  that straddles a night. Re-run with --session-gap 1800 to see how")
        add("  much of this survives a stricter definition.")
    add(f"  transitions counted: {total}")
    add("")

    labels = result["matrix_labels"]
    if labels:
        width = max(len(label) for label in labels)
        width = max(width, 12)
        header = " " * (width + 4) + "".join(f"{label[:9]:>10}" for label in labels)
        add(header)
        add(" " * (width + 4) + "-" * (10 * len(labels)))
        lookup = {(row["from"], row["to"]): row["count"] for row in matrix}
        for source in labels:
            row_total = sum(lookup.get((source, target), 0) for target in labels)
            cells = "".join(
                f"{lookup.get((source, target), 0) or '.':>10}" for target in labels
            )
            add(f"  {source:<{width}} ->{cells}   | {row_total}")
        add("")
        add("  rows = preceding category, columns = following category, '.' = zero")

    add("")
    add(f"TOP {top} TRANSITIONS")
    add("-" * 74)
    for row in matrix[:top]:
        share = row["count"] / total if total else 0
        add(f"  {row['from']:>22} -> {row['to']:<22} {row['count']:6}  {share:6.1%}")

    add("")
    add("HOUR OF DAY (local)")
    add("-" * 74)
    hours = result["hour_of_day"]
    peak = max((count for _, count in hours), default=1) or 1
    for hour, count in hours:
        add(f"  {hour}:00  {count:7}  {_bar(count / peak, 46)}")

    add("")
    add("WHAT THIS IS NOT")
    add("-" * 74)
    add("  There is no outcome variable in this table. Nothing here records")
    add("  whether a work block was completed, so no transition above can be")
    add("  called an antecedent of anything. No significance test was run and")
    add("  no multiplicity control was applied, because there is no hypothesis")
    add("  to correct — these are raw counts.")
    add("")
    add("  n = 1. One person, one Mac, "
        f"{days} local day(s). It demonstrates that the")
    add("  transition arithmetic is reproducible from the shipped schema. It is")
    add("  not evidence about users.")
    add("")
    return "\n".join(out)


def _bar(fraction: float, width: int) -> str:
    filled = int(round(max(0.0, min(1.0, fraction)) * width))
    return "#" * filled


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--db",
        type=Path,
        default=Path(
            os.environ.get("VELVT_DATABASE_PATH")
            or (Path.home() / ".velvt" / "velvt-service.sqlite3")
        ),
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a report")
    parser.add_argument(
        "--exclude",
        default="",
        help="comma-separated categories to drop, e.g. UNLOGGED,SYSTEM",
    )
    parser.add_argument(
        "--session-gap",
        type=int,
        default=0,
        help="seconds; adjacent events further apart than this are not a transition",
    )
    parser.add_argument("--top", type=int, default=12, help="how many transitions to list")
    args = parser.parse_args()

    if not args.db.exists():
        print(f"ERROR: no database at {args.db}", file=sys.stderr)
        return 1
    if args.session_gap < 0:
        print("ERROR: --session-gap cannot be negative", file=sys.stderr)
        return 1

    exclude = tuple(
        part.strip().upper() for part in args.exclude.split(",") if part.strip()
    )

    with tempfile.TemporaryDirectory() as tmp:
        connection = open_snapshot(args.db, Path(tmp))
        try:
            prov = provenance(connection)
            if not prov["events"]:
                print(
                    f"No events in {SOURCE_TABLE}. Either Velvt has not run on this "
                    "Mac, or the 7-day buffer has expired. That is a readable "
                    "result, not an error.",
                    file=sys.stderr,
                )
                return 0
            category_rows = categories(connection, exclude)
            transition_rows, meta = transitions(connection, exclude, args.session_gap)
            hours = hour_of_day(connection, exclude)
        except sqlite3.OperationalError as error:
            if "LAG" in str(error).upper() or "window" in str(error).lower():
                print(
                    "ERROR: this sqlite3 build has no window functions, so the "
                    "LAG-based matrix cannot be computed. SQLite 3.28 or newer "
                    "is required; macOS 11 and later ship one.",
                    file=sys.stderr,
                )
                return 1
            raise
        finally:
            connection.close()

    labels = [category for category, _ in category_rows]
    result = {
        "label": (
            f"Prototype · n=1 · founder's own Mac · "
            f"{prov['local_days_covered']} days · no outcome variable — "
            "this is the instrument, not a result."
        ),
        "provenance": prov,
        "excluded_categories": list(exclude),
        "categories": category_rows,
        "matrix_labels": labels,
        "transitions": [
            {"from": source, "to": target, "count": count}
            for source, target, count in transition_rows
        ],
        "transition_meta": meta,
        "hour_of_day": hours,
        "caveats": [
            "no outcome variable exists in raw_event_buffer",
            "no significance test and no multiplicity control were applied",
            "n = 1, one Mac, one person",
            "the 7-day retention sweep bounds how much history can ever be here",
        ],
    }

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(render(result, args.top))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
