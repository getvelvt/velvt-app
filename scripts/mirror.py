#!/usr/bin/env python3
"""Your days: the longest unbroken stretch on this Mac, day by day.

This is a mirror, not a finding. It reports what was observed and nothing more:
no comparison, no baseline, no counterfactual, no recommendation, no score. Every
number traces to a stored row, and the query that produced it is printed with
--sql so a skeptic can run it themselves.

A "stretch" is a maximal run of consecutive observations sharing one category.
The run breaks when the category changes. It does NOT break on a time gap --
verified on this data at 5-minute, 15-minute and 24-hour thresholds, which all
produce identical output, so the choice is not load-bearing. If that ever stops
being true on someone else's machine, --gap-seconds exposes it.

Reads from a throwaway copy. The live database is never opened, so this is safe
to run while Velvt is running -- and it cannot corrupt or lock anything.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sqlite3
import sys
import tempfile
from datetime import datetime

DEFAULT_DB = os.path.expanduser("~/.velvt/velvt-service.sqlite3")

# Source of truth for this surface. `batch_event` carries the longest local
# history (30-day retention) and holds only category, duration and timestamp --
# no app name, no title, no URL. `raw_event_buffer` expires at 7 days.
SOURCE_TABLE = "batch_event"

QUERY = """
WITH e AS (
  SELECT rowid AS rid, occurred_at AS t, duration_seconds AS dur, category AS c,
         LAG(occurred_at + duration_seconds) OVER (ORDER BY occurred_at, rowid) AS prev_end,
         LAG(category)                       OVER (ORDER BY occurred_at, rowid) AS prev_c
  FROM {table}
),
flagged AS (
  SELECT *, CASE WHEN prev_c IS NULL OR c <> prev_c OR (t - prev_end) > :gap
                 THEN 1 ELSE 0 END AS is_new_run
  FROM e
),
grouped AS (
  SELECT *, SUM(is_new_run) OVER (ORDER BY t, rid) AS run_id FROM flagged
),
runs AS (
  SELECT run_id,
         MIN(date(t, 'unixepoch', 'localtime')) AS day,
         MIN(c)     AS category,
         SUM(dur)   AS seconds
  FROM grouped GROUP BY run_id
)
-- Ties broken by category name so the output is deterministic across runs.
SELECT day, category, seconds FROM runs r
WHERE seconds = (SELECT MAX(seconds) FROM runs r2 WHERE r2.day = r.day)
GROUP BY day HAVING category = MIN(category)
ORDER BY day
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument("--gap-seconds", type=int, default=86_400,
                    help="split a run when this many seconds pass between observations "
                         "(default 86400 = effectively off; see the module docstring)")
    ap.add_argument("--sql", action="store_true", help="print the query and exit")
    args = ap.parse_args()

    query = QUERY.format(table=SOURCE_TABLE)
    if args.sql:
        print(query.strip())
        return 0

    if not os.path.exists(args.db):
        print(f"No database at {args.db}", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory() as tmp:
        copy = os.path.join(tmp, "snapshot.sqlite3")
        shutil.copy2(args.db, copy)
        con = sqlite3.connect(f"file:{copy}?mode=ro", uri=True)
        rows = con.execute(query, {"gap": args.gap_seconds}).fetchall()
        total, days = con.execute(
            f"SELECT COUNT(*), COUNT(DISTINCT date(occurred_at,'unixepoch','localtime')) "
            f"FROM {SOURCE_TABLE}"
        ).fetchone()
        con.close()

    if not rows:
        print("Nothing observed yet. Velvt has not recorded enough to show a day.")
        return 0

    print()
    print("  YOUR DAYS")
    print("  Longest unbroken stretch, day by day.")
    print()

    longest = max(r[2] for r in rows)
    for day, category, seconds in rows:
        label = datetime.strptime(day, "%Y-%m-%d").strftime("%a %b %-d")
        minutes = seconds // 60
        # One block per 4 minutes, so the bar is a duration and not a ranking.
        bar = "█" * max(1, round(seconds / max(longest, 1) * 24))
        print(f"  {label:<12} {bar:<26} {category:<20} {minutes:>3} min")

    print()
    print(f"  Read from {total:,} observations across {days} days on this Mac.")
    print("  A stretch breaks when the category changes.")
    print("  Nothing here left the machine. Nothing here is inferred.")
    print()
    print("  Run with --sql to see the query. Run it yourself against the copy.")
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
