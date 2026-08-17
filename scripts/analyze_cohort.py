#!/usr/bin/env python3
"""Computes the pre-registered cohort outcomes from tester CSV exports.

Input is whatever `export_cohort_evidence.sh` produced on each tester's Mac —
one CSV per participant, passed as arguments. Output is the numbers named in
`pitch-deck-inputs/evidence/traction-summary.md`, each with its numerator,
denominator, window, and exclusions stated, because a ratio on its own is not
reportable evidence.

This script computes. It does not decide. Every threshold and definition here
is read from the pre-registration written on 2026-08-09, before any data
existed; nothing may be added after seeing results. If a definition turns out
to be wrong, amend it in a dated note stating what was known at the time.

Usage:
    ./scripts/analyze_cohort.py tester-*.csv
    ./scripts/analyze_cohort.py --json tester-*.csv
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

# Pre-registered 2026-08-09. The state machine records `returned` whenever the
# anchor category reappears while an offer is unanswered, with no time bound —
# a return forty minutes later in a sixty-minute block still stores as
# `returned`. Counting that column alone overstates the primary outcome, so the
# bound is applied here.
RETURN_WINDOW_SECONDS = 600

# Pre-registered 2026-08-09. `was_focused` disputes the judgment; the offer
# should never have fired. `wrong_classification` disputes a label. They are
# reported separately as well as combined, because they are different failures.
WRONG_INTERVENTION_OUTCOMES = ("was_focused", "wrong_classification")

# Blocks shorter than the drift gate's warm-up cannot produce an offer, so they
# cannot inform the metric. Declared in advance as an exclusion.
WARMUP_EXCLUSION_SECONDS = 300

TERMINAL_OUTCOMES = (
    "accepted_action",
    "returned",
    "not_helpful",
    "wrong_classification",
    "was_focused",
    "dismissed",
    "no_response",
)


@dataclass
class Offer:
    participant: str
    block_id: str
    outcome: str
    salience: str
    offered_at: int
    outcome_at: int | None
    planned_duration_seconds: int
    returned_within_window: bool


@dataclass
class Excluded:
    participant: str
    block_id: str
    reason: str


@dataclass
class Cohort:
    offers: list[Offer] = field(default_factory=list)
    excluded: list[Excluded] = field(default_factory=list)
    participants: set[str] = field(default_factory=set)
    empty_exports: list[str] = field(default_factory=list)
    malformed: list[str] = field(default_factory=list)


def _int(row: dict, key: str) -> int | None:
    value = (row.get(key) or "").strip()
    if not value:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def load(paths: list[Path]) -> Cohort:
    cohort = Cohort()
    for path in paths:
        participant = path.stem
        cohort.participants.add(participant)
        try:
            text = path.read_text()
        except OSError as error:
            cohort.malformed.append(f"{participant}: unreadable ({error})")
            continue

        # A participant who used Velvt without ever triggering an offer exports
        # a header and no rows. That is a real result — the gate never fired —
        # and it belongs in the denominator of participation, not in the bin.
        rows = list(csv.DictReader(text.splitlines()))
        if not rows:
            cohort.empty_exports.append(participant)
            continue
        if "outcome" not in (rows[0].keys()):
            cohort.malformed.append(f"{participant}: no 'outcome' column")
            continue

        for row in rows:
            block_id = (row.get("block_id") or "").strip()
            planned = _int(row, "planned_duration_seconds") or 0
            if planned and planned < WARMUP_EXCLUSION_SECONDS:
                cohort.excluded.append(
                    Excluded(participant, block_id, "block shorter than the drift warm-up")
                )
                continue

            outcome = (row.get("outcome") or "").strip()
            if outcome not in TERMINAL_OUTCOMES and outcome != "offered":
                cohort.malformed.append(f"{participant}: unknown outcome {outcome!r}")
                continue

            offered_at = _int(row, "offered_at") or 0
            outcome_at = _int(row, "outcome_at")

            # Prefer the exporter's own bounded column when present; fall back
            # to computing it, so an older export still analyses correctly.
            flag = (row.get("returned_within_10min") or "").strip()
            if flag in ("0", "1"):
                returned = flag == "1"
            else:
                returned = (
                    outcome == "returned"
                    and outcome_at is not None
                    and (outcome_at - offered_at) <= RETURN_WINDOW_SECONDS
                )

            cohort.offers.append(
                Offer(
                    participant=participant,
                    block_id=block_id,
                    outcome=outcome,
                    salience=(row.get("salience") or "normal").strip() or "normal",
                    offered_at=offered_at,
                    outcome_at=outcome_at,
                    planned_duration_seconds=planned,
                    returned_within_window=returned,
                )
            )
    return cohort


def analyse(cohort: Cohort) -> dict:
    offers = cohort.offers
    denominator = len(offers)

    returned = [o for o in offers if o.returned_within_window]
    raw_returned = [o for o in offers if o.outcome == "returned"]
    was_focused = [o for o in offers if o.outcome == "was_focused"]
    wrong_class = [o for o in offers if o.outcome == "wrong_classification"]
    wrong_any = [o for o in offers if o.outcome in WRONG_INTERVENTION_OUTCOMES]
    silent = [o for o in offers if o.outcome == "no_response"]

    by_salience: dict[str, dict] = {}
    for salience in ("normal", "quiet"):
        subset = [o for o in offers if o.salience == salience]
        by_salience[salience] = {
            "offers": len(subset),
            "returned_within_10min": sum(1 for o in subset if o.returned_within_window),
            "no_response": sum(1 for o in subset if o.outcome == "no_response"),
        }

    return {
        "participants": {
            "exports_received": len(cohort.participants),
            "with_at_least_one_offer": len({o.participant for o in offers}),
            "exported_zero_offers": sorted(cohort.empty_exports),
        },
        "primary_outcome": {
            "definition": (
                "Of drift interventions delivered, the fraction followed by a return "
                "to the anchor category within 10 minutes."
            ),
            "numerator": len(returned),
            "denominator": denominator,
            "window_seconds": RETURN_WINDOW_SECONDS,
            "unbounded_returned_numerator": len(raw_returned),
            "note": (
                "The unbounded count is shown only to expose the gap. The "
                "pre-registered metric is the bounded one."
            ),
        },
        "trust": {
            "definition": "Offers the user says should not have fired.",
            "was_focused": len(was_focused),
            "wrong_classification": len(wrong_class),
            "combined_numerator": len(wrong_any),
            "denominator": denominator,
            "auto_demotion_threshold": 0.15,
        },
        "silence": {
            "no_response": len(silent),
            "note": "Silence is not a refusal. It stays in the denominator.",
        },
        "outcome_distribution": dict(
            sorted(Counter(o.outcome for o in offers).items())
        ),
        "salience_split": by_salience,
        "exclusions": {
            "declared_in_advance": [
                f"blocks shorter than {WARMUP_EXCLUSION_SECONDS}s (below the drift warm-up)",
                "the founder's own device",
                "any participant who reinstalled mid-cohort (local history resets with the database)",
            ],
            "applied_here": len(cohort.excluded),
            "detail": [
                {"participant": e.participant, "block_id": e.block_id, "reason": e.reason}
                for e in cohort.excluded
            ],
        },
        "data_quality": {"malformed": cohort.malformed},
    }


def _ratio(numerator: int, denominator: int) -> str:
    if denominator == 0:
        return "not computable (denominator is 0)"
    return f"{numerator}/{denominator} = {numerator / denominator:.1%}"


def render(result: dict) -> str:
    out: list[str] = []
    add = out.append
    p = result["participants"]
    add("COHORT ANALYSIS")
    add("=" * 64)
    add(f"exports received:        {p['exports_received']}")
    add(f"  with >=1 offer:        {p['with_at_least_one_offer']}")
    add(f"  exported zero offers:  {len(p['exported_zero_offers'])} {p['exported_zero_offers'] or ''}")
    if p["exports_received"] and not p["with_at_least_one_offer"]:
        add("")
        add("  No offer fired for anyone. That is a result, not a failure of")
        add("  collection: the gate's thresholds were never met. Report it.")

    primary = result["primary_outcome"]
    add("")
    add("PRIMARY OUTCOME (pre-registered 2026-08-09)")
    add("-" * 64)
    add(f"  {primary['definition']}")
    add(f"  returned within {primary['window_seconds']}s: "
        f"{_ratio(primary['numerator'], primary['denominator'])}")
    if primary["unbounded_returned_numerator"] != primary["numerator"]:
        add(f"  unbounded 'returned' would report {primary['unbounded_returned_numerator']}"
            f"/{primary['denominator']} — overstated, do not use")

    trust = result["trust"]
    add("")
    add("TRUST (wrong-intervention rate)")
    add("-" * 64)
    add(f"  was_focused:          {_ratio(trust['was_focused'], trust['denominator'])}")
    add(f"  wrong_classification: {_ratio(trust['wrong_classification'], trust['denominator'])}")
    add(f"  combined:             {_ratio(trust['combined_numerator'], trust['denominator'])}")
    if trust["denominator"]:
        rate = trust["combined_numerator"] / trust["denominator"]
        if rate > trust["auto_demotion_threshold"]:
            add(f"  ABOVE the {trust['auto_demotion_threshold']:.0%} auto-demotion threshold.")

    add("")
    add("OUTCOME DISTRIBUTION")
    add("-" * 64)
    if result["outcome_distribution"]:
        for outcome, count in result["outcome_distribution"].items():
            add(f"  {outcome:24} {count}")
    else:
        add("  (no offers)")
    add(f"  silence (no_response): {result['silence']['no_response']} — "
        "not a refusal, stays in the denominator")

    add("")
    add("SALIENCE SPLIT")
    add("-" * 64)
    add("  A quiet offer rendered the in-app card and sent no notification, so an")
    add("  ignored quiet offer never rang. Pooling understates responsiveness.")
    for salience, stats in result["salience_split"].items():
        add(f"  {salience:8} offers={stats['offers']:4} "
            f"returned={stats['returned_within_10min']:4} "
            f"no_response={stats['no_response']:4}")

    exclusions = result["exclusions"]
    add("")
    add("EXCLUSIONS (declared in advance)")
    add("-" * 64)
    for line in exclusions["declared_in_advance"]:
        add(f"  - {line}")
    add(f"  applied to this data: {exclusions['applied_here']} row(s)")

    if result["data_quality"]["malformed"]:
        add("")
        add("DATA QUALITY")
        add("-" * 64)
        for issue in result["data_quality"]["malformed"]:
            add(f"  ! {issue}")

    add("")
    add("Report numerator and denominator, never the ratio alone.")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csvs", nargs="+", type=Path, help="tester CSV exports")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a report")
    args = parser.parse_args()

    missing = [p for p in args.csvs if not p.exists()]
    if missing:
        print(f"ERROR: no such file: {', '.join(str(p) for p in missing)}", file=sys.stderr)
        return 1

    result = analyse(load(args.csvs))
    print(json.dumps(result, indent=2) if args.json else render(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
