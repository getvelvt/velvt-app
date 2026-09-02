#!/usr/bin/env python3
"""Computes the pre-registered cohort outcomes from tester CSV exports.

Input is whatever `export_cohort_evidence.sh` produced on each tester's Mac —
one CSV per participant, passed as arguments. Output is the numbers named in
`pitch-deck-inputs/evidence/traction-summary.md`, each with its numerator,
denominator, window, and exclusions stated, because a ratio on its own is not
reportable evidence.

This script computes. It does not decide. Every threshold and definition here
is read from the pre-registration written on 2026-08-09 and the amendment dated
2026-08-21, before any data existed; nothing may be added after seeing results.
If a definition turns out to be wrong, amend it in a dated note stating what was
known at the time.

What it reports is the RETIRED 2026-08-09 figure, labelled as retired, plus the
trust and withheld blocks. The replacement primary outcome — sustained anchor
engagement over a 900-second horizon — is not computed here, because its
denominator and its numerator both live outside this CSV. Saying so is cheaper
than a number that looks like it.

Usage:
    ./scripts/analyze_cohort.py tester-*.csv
    ./scripts/analyze_cohort.py --json tester-*.csv
    ./scripts/analyze_cohort.py --founder-device my-mac tester-*.csv
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

# The founder's own device, also declared in advance as an exclusion, and
# applied here rather than printed. A participant's identity is the CSV's
# filename stem — the export carries no device identifier and must not grow one
# — so the convention is that a founder export is named `founder.csv` or
# `founder-<something>.csv`, and `--founder-device NAME` names any export that
# is not. This list used to be rendered in the report's footer and enforced
# nowhere, which excluded exactly nothing.
FOUNDER_DEVICE_PREFIXES = ("founder-", "founder_")

# The power requirement from `traction-summary.md`, and every input to it is an
# assumption, none of them measured: a 0.30 baseline under silence, a
# 15-percentage-point effect worth detecting, alpha 0.05 two-sided, 80% power,
# and 70/30 allocation. Halving the detectable effect to 7.5 points quadruples
# the requirement to about 1,560. It is reported beside every ratio because a
# percentage computed from two rows is not a finding, and printing one without
# this number beside it is how it becomes read as one.
POWER_REQUIRED_DECISION_POINTS = 390
POWER_ASSUMPTIONS = (
    "0.30 baseline under silence, a 15-point effect worth detecting, "
    "alpha 0.05 two-sided, 80% power, 70/30 allocation"
)

# Terminal outcomes for an offer that was actually DELIVERED to a person: a
# banner or an in-app card reached them and this is how it resolved. These are
# the only rows that may sit in the primary-outcome denominator, because the
# primary outcome asks what a delivered interruption changed.
DELIVERED_TERMINAL_OUTCOMES = (
    "accepted_action",
    "returned",
    "not_helpful",
    "wrong_classification",
    "was_focused",
    "dismissed",
    "no_response",
)

# Terminal outcomes for a decision the gate made and then WITHHELD. Both are in
# the shipped v28 CHECK constraint — `delivery_suppressed_dnd` from migration
# 0020, `withheld_demotion` from migration 0023 — and both are terminal at
# creation, delivered by no channel. A nudge that was never shown cannot be
# returned to and cannot be wrong, so these rows must never enter the delivered
# denominator; they are partitioned out and reported on their own.
#
# Amendment, dated 2026-08-21, stating what was known at the time: the
# pre-registration written on 2026-08-09 enumerated the outcome vocabulary as it
# stood before migrations 0020 and 0023 landed, so this script silently routed
# both values to `malformed` and DROPPED the row — shrinking the denominator
# rather than reporting the withholding. That is a defect in the instrument, not
# a change of definition: the primary outcome's numerator and denominator are
# unchanged, and the recovered rows are reported in a block of their own.
WITHHELD_TERMINAL_OUTCOMES = (
    "delivery_suppressed_dnd",
    "withheld_demotion",
)

# Everything the shipped schema can store, `offered` aside (the only
# non-terminal state). Kept as one tuple so an unknown value is still caught.
TERMINAL_OUTCOMES = DELIVERED_TERMINAL_OUTCOMES + WITHHELD_TERMINAL_OUTCOMES

WITHHELD_REASONS = {
    "delivery_suppressed_dnd": "Do Not Disturb was on; no channel fired.",
    "withheld_demotion": "auto-demotion was active; Velvt had gone quiet.",
}


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
    # Decisions the gate recorded and withheld. Deliberately a separate list
    # from `offers`, so that no ratio computed over `offers` can accidentally
    # pick them up: partitioning by outcome inside a single list is one `if`
    # away from counting a nudge nobody saw as a nudge somebody ignored.
    withheld: list[Offer] = field(default_factory=list)
    excluded: list[Excluded] = field(default_factory=list)
    participants: set[str] = field(default_factory=set)
    # Exports dropped whole by the founder-device exclusion. Kept apart from
    # `participants` so that no count above can include a device the
    # pre-registration excluded before any data existed.
    excluded_participants: set[str] = field(default_factory=set)
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


def _is_founder_device(participant: str, declared: set[str]) -> bool:
    if participant in declared:
        return True
    lowered = participant.lower()
    return lowered == "founder" or lowered.startswith(FOUNDER_DEVICE_PREFIXES)


def load(paths: list[Path], founder_devices: tuple[str, ...] = ()) -> Cohort:
    cohort = Cohort()
    declared = {name.strip() for name in founder_devices if name.strip()}
    for path in paths:
        participant = path.stem
        # Applied to the whole export, not row by row. A founder export with
        # zero offers is still a founder export, and admitting it as a
        # zero-offer participant would put it back into the participation
        # denominator this exclusion exists to keep it out of.
        if _is_founder_device(participant, declared):
            cohort.excluded_participants.add(participant)
            cohort.excluded.append(
                Excluded(participant, "", "the founder's own device")
            )
            continue
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

            record = Offer(
                participant=participant,
                block_id=block_id,
                outcome=outcome,
                salience=(row.get("salience") or "normal").strip() or "normal",
                offered_at=offered_at,
                outcome_at=outcome_at,
                planned_duration_seconds=planned,
                # A withheld row is terminal at creation and reached no
                # channel, so it can never have been returned to. Force the
                # flag off rather than trusting an exporter's column.
                returned_within_window=(
                    False if outcome in WITHHELD_TERMINAL_OUTCOMES else returned
                ),
            )
            if outcome in WITHHELD_TERMINAL_OUTCOMES:
                cohort.withheld.append(record)
            else:
                cohort.offers.append(record)
    return cohort


def analyse(cohort: Cohort) -> dict:
    offers = cohort.offers
    denominator = len(offers)
    withheld = cohort.withheld

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

    withheld_counts = Counter(o.outcome for o in withheld)

    return {
        "participants": {
            "exports_received": len(cohort.participants),
            "with_at_least_one_offer": len({o.participant for o in offers}),
            "with_at_least_one_withheld": len({o.participant for o in withheld}),
            "exported_zero_offers": sorted(cohort.empty_exports),
        },
        "decisions_recorded": {
            "definition": (
                "Every row the gate wrote, delivered or not. Reported so the "
                "delivered denominator can be audited against it."
            ),
            "total": denominator + len(withheld),
            "delivered": denominator,
            "withheld": len(withheld),
        },
        "power": {
            "required_decision_points": POWER_REQUIRED_DECISION_POINTS,
            "observed_decisions_here": denominator + len(withheld),
            "assumptions": POWER_ASSUMPTIONS,
            "sufficient": (
                denominator + len(withheld) >= POWER_REQUIRED_DECISION_POINTS
            ),
            "note": (
                "The requirement is stated over ELIGIBLE DECISION POINTS, which "
                "this CSV does not carry — it carries delivered interventions. "
                "The count beside it is therefore an upper bound on how close "
                "this data gets, and it is the number every ratio below is "
                "computed from."
            ),
        },
        "primary_outcome": {
            "status": (
                "DESCRIPTIVE. Pre-registered 2026-08-09, retired 2026-08-21, "
                "retained so results stay comparable across that change. Not the "
                "primary outcome and never the headline."
            ),
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
        "withheld": {
            "definition": (
                "Decisions the gate made and did not deliver. Terminal at "
                "creation, delivered by no channel, and excluded from every "
                "denominator above — a nudge nobody saw cannot be returned to "
                "and cannot be wrong."
            ),
            "total": len(withheld),
            "by_outcome": dict(sorted(withheld_counts.items())),
            "reasons": {
                outcome: WITHHELD_REASONS[outcome]
                for outcome in sorted(withheld_counts)
                if outcome in WITHHELD_REASONS
            },
            "share_of_recorded_decisions": (
                f"{len(withheld)}/{denominator + len(withheld)}"
                if (denominator + len(withheld))
                else "not computable (no decisions recorded)"
            ),
            "note": (
                "Report this beside the delivered counts, not folded into "
                "them. A high withheld count with a low delivered count is the "
                "gate choosing not to speak, which is a result in itself."
            ),
        },
        "outcome_distribution": dict(
            sorted(Counter(o.outcome for o in offers).items())
        ),
        "outcome_distribution_note": (
            "Delivered offers only. Withheld decisions are in the 'withheld' block."
        ),
        "salience_split": by_salience,
        "exclusions": {
            "declared_in_advance": [
                f"blocks shorter than {WARMUP_EXCLUSION_SECONDS}s (below the drift warm-up)",
                "the founder's own device",
                "any participant who reinstalled mid-cohort (local history resets with the database)",
            ],
            "applied_here": len(cohort.excluded),
            "founder_devices_excluded": sorted(cohort.excluded_participants),
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
    ratio = f"{numerator}/{denominator} = {numerator / denominator:.1%}"
    if denominator < POWER_REQUIRED_DECISION_POINTS:
        return f"{ratio} — underpowered, see POWER above"
    return ratio


def render(result: dict) -> str:
    out: list[str] = []
    add = out.append
    p = result["participants"]
    add("COHORT ANALYSIS")
    add("=" * 64)
    add(f"exports received:        {p['exports_received']}")
    add(f"  with >=1 offer:        {p['with_at_least_one_offer']}")
    add(f"  with >=1 withheld:     {p['with_at_least_one_withheld']}")
    add(f"  exported zero offers:  {len(p['exported_zero_offers'])} {p['exported_zero_offers'] or ''}")
    if p["exports_received"] and not p["with_at_least_one_offer"]:
        add("")
        add("  No offer was delivered to anyone. That is a result, not a failure")
        add("  of collection: the gate's thresholds were never met, or every")
        add("  decision it did make was withheld. Report it, and read the")
        add("  WITHHELD block below before concluding which.")

    decisions = result["decisions_recorded"]
    add("")
    add("DECISIONS RECORDED")
    add("-" * 64)
    add(f"  {decisions['definition']}")
    add(f"  total:     {decisions['total']}")
    add(f"  delivered: {decisions['delivered']}   <- the denominator below")
    add(f"  withheld:  {decisions['withheld']}   <- partitioned out, reported separately")

    power = result["power"]
    add("")
    add("POWER")
    add("-" * 64)
    add(f"  Detecting the pre-registered effect needs about "
        f"{power['required_decision_points']} eligible decision")
    add("  points, under assumptions none of which are measured:")
    add(f"    {power['assumptions']}.")
    add(f"  This data carries {power['observed_decisions_here']}.")
    add(f"  {power['note']}")
    if not power["sufficient"]:
        add("  Every ratio below is reported for completeness and is not a finding.")

    primary = result["primary_outcome"]
    add("")
    add("DESCRIPTIVE — RETIRED 2026-08-21, NOT THE PRIMARY OUTCOME")
    add("-" * 64)
    add("  Pre-registered 2026-08-09 and retired by the 2026-08-21 amendment for a")
    add("  ceiling effect readable in the drift gate's own source: the gate only")
    add("  fires on someone who has already returned to the anchor three times")
    add("  inside the same ten minutes, so this measures the gate's selection rule")
    add("  rather than what the intervention changed. Retained so figures stay")
    add("  comparable across the change. Never the headline.")
    add(f"  {primary['definition']}")
    add(f"  returned within {primary['window_seconds']}s: "
        f"{_ratio(primary['numerator'], primary['denominator'])}")
    if primary["unbounded_returned_numerator"] != primary["numerator"]:
        add(f"  unbounded 'returned' would report {primary['unbounded_returned_numerator']}"
            f"/{primary['denominator']} — overstated, do not use")

    add("")
    add("PRIMARY OUTCOME (replacement, 2026-08-21) — NOT COMPUTED HERE")
    add("-" * 64)
    add("  Sustained anchor engagement: at each eligible decision point, whether")
    add("  at least 600 of the following 900 seconds were spent in the anchor")
    add("  category. This script cannot compute it and does not approximate it.")
    add("  Its denominator is eligible decision points from")
    add("  intervention_decision_log and its numerator needs per-second anchor")
    add("  coverage from work_block_observation; this CSV carries neither.")
    add("  export_cohort_evidence.sh writes the decision log to a second CSV")
    add("  beside this one. That file is the denominator, not the outcome.")

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
    add("OUTCOME DISTRIBUTION (delivered only)")
    add("-" * 64)
    if result["outcome_distribution"]:
        for outcome, count in result["outcome_distribution"].items():
            add(f"  {outcome:24} {count}")
    else:
        add("  (no delivered offers)")
    add(f"  silence (no_response): {result['silence']['no_response']} — "
        "not a refusal, stays in the denominator")

    held = result["withheld"]
    add("")
    add("WITHHELD (recorded, never delivered)")
    add("-" * 64)
    add(f"  {held['definition']}")
    if held["by_outcome"]:
        for outcome, count in held["by_outcome"].items():
            reason = held["reasons"].get(outcome, "")
            add(f"  {outcome:24} {count}   {reason}")
    elif decisions["total"]:
        add("  (none — every recorded decision was delivered)")
    else:
        add("  (no decisions were recorded at all, so none could be withheld)")
    add(f"  share of recorded decisions: {held['share_of_recorded_decisions']}")
    add("  These rows are NOT in the primary-outcome or trust denominators.")

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
    add(f"  applied to this data: {exclusions['applied_here']} exclusion(s)")
    founder = exclusions["founder_devices_excluded"]
    if founder:
        add(f"  founder devices dropped whole: {', '.join(founder)}")
    else:
        add("  founder devices dropped whole: none. No export was named "
            "`founder`/`founder-*`")
        add("  and none was declared with --founder-device, so this exclusion "
            "removed nothing.")

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
    parser.add_argument(
        "--founder-device",
        action="append",
        default=[],
        metavar="NAME",
        help="drop this export whole, per the pre-registered founder-device "
             "exclusion; repeatable. Exports named `founder` or `founder-*` "
             "are dropped without the flag.",
    )
    args = parser.parse_args()

    missing = [p for p in args.csvs if not p.exists()]
    if missing:
        print(f"ERROR: no such file: {', '.join(str(p) for p in missing)}", file=sys.stderr)
        return 1

    result = analyse(load(args.csvs, tuple(args.founder_device)))
    print(json.dumps(result, indent=2) if args.json else render(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
