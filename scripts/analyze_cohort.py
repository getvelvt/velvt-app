#!/usr/bin/env python3
"""Computes the pre-registered cohort outcomes from tester exports.

Input is whatever `export_cohort_evidence.sh` wrote on each tester's Mac: a
per-offer CSV and, beside it, the companion files that share its name stem
(`-decisions.csv`, `-blocks.csv`, `-invitations.csv`, `-explain.csv`,
`-corrections.csv`, `-meta.csv`). Pass one folder per participant, or the files
themselves.
Output is the numbers named in `pitch-deck-inputs/evidence/traction-summary.md`,
each with its numerator, denominator, window and exclusions stated, because a
ratio on its own is not reportable evidence.

This script computes. It does not decide. Every threshold and definition here
is read from that file's pre-registration (2026-08-09), its additions
(2026-08-17), the amendment that replaced the primary outcome (2026-08-21), the
correction of 2026-08-31, the drift policy v2 amendment and 0.1.6 metrics
(both 2026-09-25), and the drift policy v3 note (2026-09-26), all written
before any cohort data existed. Nothing may be
added after seeing results. If a definition turns out to be wrong, amend it in
a dated note stating what was known at the time.

What it does not compute: the replacement primary outcome's NUMERATOR
(sustained anchor engagement, at least 600 of the 900 seconds after a decision
point in the anchor category). That needs per-second anchor coverage from
`work_block_observation`, which no export carries. The denominator, the
censoring that the export can see, and the power verdict are computed and
reported. The retired 2026-08-09 figure is reported as descriptive, never as
the headline.

Usage:
    python3 scripts/analyze_cohort.py ~/cohort/*/          # one folder per participant
    python3 scripts/analyze_cohort.py p01.csv p02.csv      # companions found beside each
    python3 scripts/analyze_cohort.py --json ~/cohort/*/
    python3 scripts/analyze_cohort.py --founder-device my-mac ~/cohort/*/
    python3 scripts/analyze_cohort.py --reinstalled p07 ~/cohort/*/
    python3 scripts/analyze_cohort.py --cohort-start 2026-10-05 --cohort-weeks 1 ~/cohort/*/

Must run on the python3 that ships with macOS (3.9).
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
import textwrap
from collections import Counter
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path

# ---------------------------------------------------------------------------
# Pre-registered constants
# ---------------------------------------------------------------------------

# Pre-registered 2026-08-09 as the primary outcome's window, RETIRED as the
# primary outcome by the 2026-08-21 amendment (a ceiling effect readable in the
# gate's own source), and kept only as a descriptive figure. The state machine
# records `returned` whenever the anchor category reappears while an offer is
# unanswered, with no time bound, so the bound is applied here.
RETURN_WINDOW_SECONDS = 600

# The replacement primary outcome, 2026-08-21: at each eligible decision
# point, whether at least 600 of the following 900 seconds were spent in the
# anchor category recorded on the decision row.
PRIMARY_HORIZON_SECONDS = 900
PRIMARY_THRESHOLD_SECONDS = 600

# Pre-registered 2026-08-09. `was_focused` disputes the judgment; the offer
# should never have fired. `wrong_classification` disputes a label. They are
# reported separately as well as combined, because they are different failures.
WRONG_INTERVENTION_OUTCOMES = ("was_focused", "wrong_classification")

# Blocks too short for the drift gate's warm-up cannot produce an offer, so
# they cannot inform the metric. Declared in advance as an exclusion.
#
# Amendment, dated 2026-09-25: drift policy v2 (velvt-app PR #40, 1.0.9 and
# later) fires after 180 s, not 300 s, so the exclusion is 180 s. It is applied
# to the block's ELAPSED time, computed with the gate's own arithmetic as
# `ended_at - started_at - total_paused_seconds`. Until this date the script
# compared `planned_duration_seconds` against 300, which the schema's own CHECK
# (planned_duration_seconds BETWEEN 300 AND 10800) made impossible to trigger:
# the exclusion was declared and applied to nothing.
WARMUP_EXCLUSION_SECONDS = 180

# Amendment, dated 2026-09-25: rows from drift policy v1 and v2 are never
# pooled, and every cohort result uses policy_version 2 only.
#
# Note, dated 2026-09-26: drift policy v3 (velvt-app PR #56, protocol 32)
# keeps every v2 constant, so the warm-up exclusion above stands, but each
# dwell now reaches the gate when it begins instead of when it ends. The set of
# decision points differs from v2, and an offer now reaches the person while
# they are away instead of being withdrawn as they come back. Rows from v1, v2
# and v3 are never pooled, and every cohort result uses policy_version 3 only.
# The note is `pitch-deck-inputs/evidence/drafts/2026-09-26-policy-v3-note.md`
# until the founder appends it to traction-summary.md.
ANALYSED_POLICY_VERSION = 3

# A `work_block_intervention` row has no policy column. It takes the
# policy_version of the decision-log row for the same block_id whose verdict is
# one of these: the three verdicts that write an intervention row.
ATTRIBUTING_VERDICTS = ("offered", "withheld_demotion", "suppressed_dnd")

# The founder's own device, declared in advance as an exclusion (2026-08-09),
# and applied here rather than printed. A participant's identity is the folder
# name or the per-offer CSV's filename stem (the export carries no device
# identifier and must not grow one), so a founder export is named `founder`,
# `founder-<something>` or `founder_<something>`, and `--founder-device NAME`
# names any export that is not. Until 2026-09-25 this exclusion was rendered in
# the report's footer and enforced nowhere, which excluded exactly nothing.
FOUNDER_DEVICE_PREFIXES = ("founder-", "founder_")

# The power requirement from the 2026-08-21 amendment. Every input to it is an
# assumption, none of them measured. Since 2026-09-25 it counts policy_version
# 2 decision points only. It is reported beside every ratio because a
# percentage computed from two rows is not a finding, and printing one without
# this number beside it is how it becomes read as one.
POWER_REQUIRED_DECISION_POINTS = 390
POWER_ASSUMPTIONS = (
    "0.30 baseline under silence, a 15-point effect worth detecting, "
    "alpha 0.05 two-sided, 80% power, 70/30 allocation"
)

# Terminal outcomes for an offer that was actually DELIVERED to a person: a
# banner or an in-app card reached them and this is how it resolved.
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
# the shipped CHECK constraint (`delivery_suppressed_dnd` from migration 0020,
# `withheld_demotion` from migration 0023) and both are terminal at creation,
# delivered by no channel. A nudge that was never shown cannot be returned to
# and cannot be wrong, so these rows never enter the delivered denominator;
# they are partitioned out and reported on their own.
#
# Amendment, dated 2026-08-21: this script used to route both values to
# `malformed` and DROP the row, shrinking the denominator rather than reporting
# the withholding. A defect in the instrument, not a change of definition.
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

# `intervention_decision_log.gate_verdict`, migration 0026. Closed set.
GATE_VERDICTS = (
    "offered",
    "abstained_warmup",
    "abstained_remaining",
    "abstained_block_cap",
    "abstained_backoff",
    "abstained_no_anchor",
    "abstained_min_switches",
    "abstained_at_anchor",
    "withheld_demotion",
    "suppressed_dnd",
)

# `card_seen` as the exporter derives it from `card_seen_at` (migration 0032):
# seen = the in-app card was drawn; unseen = NULL on a row offered after the Mac
# applied 0032; unknown = NULL before that, or a database without the column
# (every 1.0.9 database). Unknown is never folded into unseen.
CARD_SEEN_BUCKETS = ("seen", "unseen", "unknown")

BLOCK_TERMINAL_PHASES = ("completed", "abandoned", "expired")
BLOCK_OPEN_PHASES = ("active", "paused")
BLOCK_ORIGINS = ("manual", "invitation")

INVITATION_TERMINAL_OUTCOMES = ("accepted", "dismissed", "no_response", "expired")
INVITATION_OPEN_OUTCOMES = ("offered",)

# The corrections file (2026-08-17 measure 3), one row per rule scope and broad
# category. `app` is `personal_app_override`, whose summed correction_count is
# the pre-registered count; `window` is `personal_override`, which keeps no
# count. The exporter writes any category outside the service's accepted set
# as `unrecognized`.
CORRECTION_SCOPES = ("app", "window")
CORRECTION_CATEGORIES = (
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    "UNLOGGED",
    "unrecognized",
)

COMPANION_SUFFIXES = {
    "decisions": "-decisions.csv",
    "blocks": "-blocks.csv",
    "invitations": "-invitations.csv",
    "explain": "-explain.csv",
    "corrections": "-corrections.csv",
    "meta": "-meta.csv",
}
FILE_KINDS = ("offers",) + tuple(COMPANION_SUFFIXES)

# Meta keys naming which companion tables the tester's database had.
META_TABLE_KEYS = {
    "decisions": "decision_log",
    "invitations": "invitations",
    "explain": "explain_probe",
    "corrections": "corrections",
}

WEEK_SECONDS = 7 * 24 * 3600

WARMUP_CATEGORY = f"block elapsed under the {WARMUP_EXCLUSION_SECONDS}s warm-up"


# ---------------------------------------------------------------------------
# Records
# ---------------------------------------------------------------------------


@dataclass
class Offer:
    participant: str
    block_id: str
    outcome: str
    salience: str
    offered_at: int
    outcome_at: int | None
    returned_within_window: bool
    card_seen: str


@dataclass
class Excluded:
    participant: str
    row: str
    reason: str
    # The declared exclusion this falls under, for counting.
    category: str


@dataclass
class Participant:
    name: str
    files: dict = field(default_factory=dict)
    meta: dict = field(default_factory=dict)
    offer_rows: list = field(default_factory=list)
    decision_rows: list = field(default_factory=list)
    block_rows: list | None = None
    invitation_rows: list | None = None
    explain_rows: list | None = None
    correction_rows: list | None = None


@dataclass
class Cohort:
    offers: list[Offer] = field(default_factory=list)
    # Decisions the gate recorded and withheld. Deliberately a separate list
    # from `offers`, so that no ratio computed over `offers` can pick them up.
    withheld: list[Offer] = field(default_factory=list)
    excluded: list[Excluded] = field(default_factory=list)
    founder_devices: set[str] = field(default_factory=set)
    exports_received: set[str] = field(default_factory=set)
    excluded_whole: dict = field(default_factory=dict)
    analysed: list[Participant] = field(default_factory=list)
    empty_exports: list[str] = field(default_factory=list)
    malformed: list[str] = field(default_factory=list)
    # Policy bookkeeping (2026-09-25 amendment).
    decisions_by_policy: Counter = field(default_factory=Counter)
    offers_by_attribution: Counter = field(default_factory=Counter)
    analysed_decisions: list = field(default_factory=list)
    eligible: list = field(default_factory=list)
    era_excluded: Counter = field(default_factory=Counter)
    non_monotonic_policy: list[str] = field(default_factory=list)
    # Block-level rows kept per participant after the policy-era filter.
    blocks: dict = field(default_factory=dict)
    invitations: dict = field(default_factory=dict)
    explain: dict = field(default_factory=dict)
    # Correction counts per participant. They carry no timestamp, so they
    # cannot be split at a policy upgrade; `corrections_span_policy_change`
    # names the participants whose counts include time before the analysed
    # policy.
    corrections: dict = field(default_factory=dict)
    corrections_span_policy_change: list[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Parsing helpers
# ---------------------------------------------------------------------------


def _int(row: dict, key: str) -> int | None:
    value = (row.get(key) or "").strip()
    if not value:
        return None
    try:
        return int(value)
    except ValueError:
        try:
            as_float = float(value)
        except ValueError:
            return None
        return int(as_float) if as_float.is_integer() else None


def _float(row: dict, key: str) -> float | None:
    value = (row.get(key) or "").strip()
    if not value:
        return None
    try:
        return float(value)
    except ValueError:
        return None


def _text(row: dict, key: str) -> str:
    return (row.get(key) or "").strip()


def _read_csv(path: Path) -> tuple[list[dict] | None, list[str] | None, str | None]:
    try:
        text = path.read_text()
    except OSError as error:
        return None, None, f"unreadable ({error})"
    reader = csv.DictReader(text.splitlines())
    rows = list(reader)
    return rows, list(reader.fieldnames or []), None


def _elapsed(started: int | None, ended: int | None, paused: int | None) -> int | None:
    """The gate's elapsed-time arithmetic, on a finished block. None if open."""
    if started is None or ended is None:
        return None
    return ended - started - (paused or 0)


def _is_founder_device(participant: str, declared: set[str]) -> bool:
    if participant in declared:
        return True
    lowered = participant.lower()
    return lowered == "founder" or lowered.startswith(FOUNDER_DEVICE_PREFIXES)


def _classify(path: Path) -> tuple[str, str]:
    """(kind, stem) for one export file."""
    name = path.name
    for kind, suffix in COMPANION_SUFFIXES.items():
        if name.endswith(suffix):
            return kind, name[: -len(suffix)]
    return "offers", path.stem


# ---------------------------------------------------------------------------
# Finding each participant's files
# ---------------------------------------------------------------------------


def discover(paths: list[Path]) -> tuple[dict[str, Participant], list[str], set[str]]:
    """Groups files into participants: (participants, problems, ambiguous names).

    A name is ambiguous when two different files of one kind claim it, which
    is what happens when two testers' exports keep the exporter's default,
    date-stamped name. Such a participant is excluded whole rather than
    analysed from a mix of two people's files.
    """
    participants: dict[str, Participant] = {}
    problems: list[str] = []
    ambiguous: set[str] = set()

    def claim(name: str, kind: str, path: Path) -> None:
        participant = participants.setdefault(name, Participant(name=name))
        existing = participant.files.get(kind)
        if existing is not None and existing.resolve() != path.resolve():
            problems.append(
                f"{name}: two {kind} files ({existing} and {path}); "
                "two exports share one participant name"
            )
            ambiguous.add(name)
            return
        participant.files[kind] = path

    for path in paths:
        if path.is_dir():
            name = path.resolve().name
            by_kind: dict[str, list[Path]] = {}
            for child in sorted(path.glob("*.csv")):
                kind, _ = _classify(child)
                by_kind.setdefault(kind, []).append(child)
            participants.setdefault(name, Participant(name=name))
            for kind, found in by_kind.items():
                if len(found) > 1:
                    problems.append(
                        f"{name}: {len(found)} {kind} files in {path}; one folder "
                        "holds one participant's export"
                    )
                    ambiguous.add(name)
                    continue
                claim(name, kind, found[0])
            continue

        kind, stem = _classify(path)
        claim(stem, kind, path)
        # The files beside it that share its stem are the same export.
        for other_kind in FILE_KINDS:
            if other_kind in participants[stem].files:
                continue
            suffix = ".csv" if other_kind == "offers" else COMPANION_SUFFIXES[other_kind]
            sibling = path.with_name(stem + suffix)
            if sibling.is_file():
                claim(stem, other_kind, sibling)
    return participants, problems, ambiguous


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------


def _read_meta(path: Path) -> tuple[dict, str | None]:
    rows, fields, error = _read_csv(path)
    if error:
        return {}, error
    if fields[:2] != ["key", "value"]:
        return {}, "meta file has no key,value header"
    return {_text(r, "key"): _text(r, "value") for r in rows}, None


def load(
    paths: list[Path],
    founder_devices: tuple[str, ...] = (),
    reinstalled: tuple[str, ...] = (),
) -> Cohort:
    cohort = Cohort()
    declared_founder = {n.strip() for n in founder_devices if n.strip()}
    declared_reinstalled = {n.strip() for n in reinstalled if n.strip()}
    participants, problems, ambiguous = discover(paths)
    cohort.malformed.extend(problems)

    for name in sorted(participants):
        participant = participants[name]
        # Applied to the whole export, not row by row. A founder export with
        # zero offers is still a founder export, and admitting it as a
        # zero-offer participant would put it back into the participation
        # denominator this exclusion exists to keep it out of.
        if _is_founder_device(name, declared_founder):
            cohort.founder_devices.add(name)
            cohort.excluded.append(
                Excluded(name, "", "the founder's own device", "founder device (whole export)")
            )
            continue
        cohort.exports_received.add(name)

        if name in declared_reinstalled:
            cohort.excluded_whole[name] = (
                "reinstalled mid-cohort; local history resets with the database"
            )
            continue
        if name in ambiguous:
            cohort.excluded_whole[name] = (
                "more than one export under this name; rename or separate them"
            )
            continue

        offers_path = participant.files.get("offers")
        if offers_path is None:
            cohort.malformed.append(f"{name}: no per-offer CSV")
            cohort.excluded_whole[name] = "no per-offer CSV received"
            continue
        rows, fields, error = _read_csv(offers_path)
        if error:
            cohort.malformed.append(f"{name}: {error}")
            cohort.excluded_whole[name] = "per-offer CSV unreadable"
            continue
        if "outcome" not in fields:
            cohort.malformed.append(f"{name}: no 'outcome' column")
            cohort.excluded_whole[name] = "per-offer CSV has no 'outcome' column"
            continue
        participant.offer_rows = rows

        if "meta" in participant.files:
            meta, error = _read_meta(participant.files["meta"])
            if error:
                cohort.malformed.append(f"{name}: meta file {error}")
            participant.meta = meta

        # Amendment, 2026-09-25: exports that do not carry the decision log are
        # reported as counts with the reason and excluded. Without it no
        # intervention can be attributed to a policy version.
        declared = participant.meta.get("decision_log")
        if "decisions" not in participant.files:
            if declared == "present":
                cohort.malformed.append(
                    f"{name}: the export wrote a decisions file and it was not received"
                )
                cohort.excluded_whole[name] = (
                    "decision-log file missing from what was received"
                )
            elif declared == "absent":
                cohort.excluded_whole[name] = (
                    "no decision log: the database predates migration 0026 "
                    "(a build older than policy v2)"
                )
            else:
                cohort.excluded_whole[name] = (
                    "no decision log received (export predates the decision-log CSV)"
                )
            continue
        rows, fields, error = _read_csv(participant.files["decisions"])
        if error or "policy_version" not in fields or "gate_verdict" not in fields:
            cohort.malformed.append(
                f"{name}: decisions file {error or 'lacks policy_version/gate_verdict'}"
            )
            cohort.excluded_whole[name] = "decision-log file unreadable"
            continue
        participant.decision_rows = rows

        for kind in ("blocks", "invitations", "explain", "corrections"):
            path = participant.files.get(kind)
            if path is None:
                if participant.meta.get(META_TABLE_KEYS.get(kind, ""), "") == "present":
                    cohort.malformed.append(
                        f"{name}: the export wrote a {kind} file and it was not received"
                    )
                continue
            rows, _, error = _read_csv(path)
            if error:
                cohort.malformed.append(f"{name}: {kind} file {error}")
                continue
            if kind == "blocks":
                participant.block_rows = rows
            elif kind == "invitations":
                participant.invitation_rows = rows
            elif kind == "explain":
                participant.explain_rows = rows
            else:
                participant.correction_rows = rows

        cohort.analysed.append(participant)

    for participant in cohort.analysed:
        _load_participant(cohort, participant)
    return cohort


def _load_participant(cohort: Cohort, participant: Participant) -> None:
    name = participant.name

    # --- Decisions, and the attribution map --------------------------------
    attribution: dict[str, set[int]] = {}
    other_policy_times: list[int] = []
    analysed_times: list[int] = []
    for row in participant.decision_rows:
        version = _int(row, "policy_version")
        verdict = _text(row, "gate_verdict")
        occurred = _int(row, "occurred_at")
        if version is None or verdict not in GATE_VERDICTS or occurred is None:
            cohort.malformed.append(
                f"{name}: decision {_text(row, 'decision_id') or '?'} has "
                f"policy_version={_text(row, 'policy_version')!r}, "
                f"gate_verdict={verdict!r}"
            )
            continue
        cohort.decisions_by_policy[version] += 1
        block_id = _text(row, "block_id")
        if verdict in ATTRIBUTING_VERDICTS and block_id:
            attribution.setdefault(block_id, set()).add(version)
        if version != ANALYSED_POLICY_VERSION:
            other_policy_times.append(occurred)
            continue
        analysed_times.append(occurred)
        record = {"participant": name, "row": row}
        cohort.analysed_decisions.append(record)
        if verdict != "offered":
            continue
        elapsed = _elapsed(
            _int(row, "block_started_at"),
            _int(row, "block_ended_at"),
            _int(row, "block_total_paused_seconds"),
        )
        if elapsed is not None and elapsed < WARMUP_EXCLUSION_SECONDS:
            cohort.excluded.append(
                Excluded(name, f"decision {_text(row, 'decision_id')}",
                         f"block elapsed {elapsed}s < {WARMUP_EXCLUSION_SECONDS}s warm-up",
                         WARMUP_CATEGORY)
            )
            continue
        cohort.eligible.append({"participant": name, "row": row, "meta": participant.meta})

    # Rows with no policy column of their own (blocks, invitations, explain
    # weeks) are counted under the analysed policy only if they come after this
    # Mac's last decision under any other policy. Conservative on purpose: a row
    # from before an upgrade cannot be told apart from one just after it.
    other_policy_until = max(other_policy_times) if other_policy_times else None
    if other_policy_times and analysed_times and max(other_policy_times) > min(analysed_times):
        cohort.non_monotonic_policy.append(name)

    def in_analysed_era(timestamp: int | None) -> bool:
        if other_policy_until is None:
            return True
        return timestamp is not None and timestamp > other_policy_until

    # --- Interventions -------------------------------------------------------
    if not participant.offer_rows:
        cohort.empty_exports.append(name)
    for row in participant.offer_rows:
        block_id = _text(row, "block_id")
        outcome = _text(row, "outcome")
        if outcome not in TERMINAL_OUTCOMES and outcome != "offered":
            cohort.malformed.append(f"{name}: unknown outcome {outcome!r}")
            continue

        versions = attribution.get(block_id, set())
        if not versions:
            cohort.offers_by_attribution["unattributed"] += 1
            cohort.excluded.append(
                Excluded(name, f"offer {block_id}",
                         "no decision-log row for this block with verdict "
                         "offered/withheld_demotion/suppressed_dnd; policy unknown",
                         "intervention not attributable to a policy version")
            )
            continue
        if len(versions) > 1:
            cohort.offers_by_attribution["conflicting"] += 1
            cohort.excluded.append(
                Excluded(name, f"offer {block_id}",
                         f"decision-log rows for this block disagree on policy {sorted(versions)}",
                         "intervention not attributable to a policy version")
            )
            continue
        version = next(iter(versions))
        cohort.offers_by_attribution[str(version)] += 1
        if version != ANALYSED_POLICY_VERSION:
            cohort.excluded.append(
                Excluded(name, f"offer {block_id}",
                         f"policy_version {version}, never pooled with {ANALYSED_POLICY_VERSION}",
                         f"intervention under policy_version {version}")
            )
            continue

        elapsed = _elapsed(
            _int(row, "started_at"), _int(row, "ended_at"), _int(row, "total_paused_seconds")
        )
        if elapsed is not None and elapsed < WARMUP_EXCLUSION_SECONDS:
            cohort.excluded.append(
                Excluded(name, f"offer {block_id}",
                         f"block elapsed {elapsed}s < {WARMUP_EXCLUSION_SECONDS}s warm-up",
                         WARMUP_CATEGORY)
            )
            continue

        offered_at = _int(row, "offered_at") or 0
        outcome_at = _int(row, "outcome_at")
        # Prefer the exporter's own bounded column when present; fall back to
        # computing it, so an older export still analyses correctly.
        flag = _text(row, "returned_within_10min")
        if flag in ("0", "1"):
            returned = flag == "1"
        else:
            returned = (
                outcome == "returned"
                and outcome_at is not None
                and (outcome_at - offered_at) <= RETURN_WINDOW_SECONDS
            )
        card_seen = _text(row, "card_seen")
        if card_seen not in CARD_SEEN_BUCKETS:
            # An export without the derived column: a timestamp is still proof
            # of a sighting, and anything else is unknown, never unseen.
            card_seen = "seen" if _text(row, "card_seen_at") else "unknown"

        record = Offer(
            participant=name,
            block_id=block_id,
            outcome=outcome,
            salience=_text(row, "salience") or "normal",
            offered_at=offered_at,
            outcome_at=outcome_at,
            # A withheld row is terminal at creation and reached no channel, so
            # it can never have been returned to. Force the flag off rather
            # than trusting an exporter's column.
            returned_within_window=(
                False if outcome in WITHHELD_TERMINAL_OUTCOMES else returned
            ),
            card_seen=card_seen,
        )
        if outcome in WITHHELD_TERMINAL_OUTCOMES:
            cohort.withheld.append(record)
        else:
            cohort.offers.append(record)

    # --- Blocks ---------------------------------------------------------------
    if participant.block_rows is not None:
        kept = []
        for row in participant.block_rows:
            phase = _text(row, "phase")
            origin = _text(row, "origin")
            started = _int(row, "started_at")
            if phase not in BLOCK_TERMINAL_PHASES + BLOCK_OPEN_PHASES or started is None:
                cohort.malformed.append(
                    f"{name}: block {_text(row, 'block_id')} phase={phase!r} started_at="
                    f"{_text(row, 'started_at')!r}"
                )
                continue
            if origin and origin not in BLOCK_ORIGINS:
                cohort.malformed.append(f"{name}: block origin {origin!r}")
                continue
            if not in_analysed_era(started):
                cohort.era_excluded["blocks"] += 1
                continue
            kept.append({"phase": phase, "origin": origin, "started_at": started})
        cohort.blocks[name] = kept

    # --- Invitations ------------------------------------------------------------
    if participant.invitation_rows is not None:
        kept = []
        for row in participant.invitation_rows:
            outcome = _text(row, "outcome")
            offered = _int(row, "offered_at")
            if outcome not in INVITATION_TERMINAL_OUTCOMES + INVITATION_OPEN_OUTCOMES:
                cohort.malformed.append(f"{name}: invitation outcome {outcome!r}")
                continue
            if not in_analysed_era(offered):
                cohort.era_excluded["invitations"] += 1
                continue
            kept.append({"outcome": outcome, "policy_version": _text(row, "policy_version")})
        cohort.invitations[name] = kept

    # --- Explain probe weeks ----------------------------------------------------
    if participant.explain_rows is not None:
        # A local Monday key cannot be placed on the UTC axis exactly, so the
        # week holding the last decision under another policy and the day
        # after it are both treated as possibly mixed.
        cutoff = None
        if other_policy_until is not None:
            cutoff = (
                datetime.fromtimestamp(other_policy_until, tz=timezone.utc).date()
                + timedelta(days=1)
            ).isoformat()
        kept = []
        for row in participant.explain_rows:
            week = _text(row, "week_start_local_date")
            taps = _int(row, "taps")
            delivered = _int(row, "delivered_interventions")
            declared = _int(row, "blocks_declared")
            if len(week) != 10 or None in (taps, delivered, declared):
                cohort.malformed.append(f"{name}: explain week row {row!r}")
                continue
            if cutoff is not None and week <= cutoff:
                cohort.era_excluded["explain_weeks"] += 1
                continue
            kept.append(
                {"week": week, "taps": taps, "delivered": delivered, "blocks": declared}
            )
        cohort.explain[name] = kept

    # --- Corrections (2026-08-17 measure 3) -------------------------------------
    if participant.correction_rows is not None:
        app: dict[str, tuple[int, int]] = {}
        window: dict[str, int] = {}
        for row in participant.correction_rows:
            scope = _text(row, "scope")
            category = _text(row, "category")
            rules = _int(row, "rules")
            corrections = _int(row, "corrections")
            problem = None
            if scope not in CORRECTION_SCOPES or category not in CORRECTION_CATEGORIES:
                problem = "unknown scope or category"
            elif rules is None or rules < 1:
                problem = "rules is not a positive count"
            elif scope == "app" and (corrections is None or corrections < rules):
                # Every app rule is written by at least one correction.
                problem = "app corrections missing or fewer than its rules"
            elif category in (app if scope == "app" else window):
                problem = "a second row for one scope and category"
            if problem:
                cohort.malformed.append(
                    f"{name}: corrections row scope={scope!r} category={category!r}: {problem}"
                )
                continue
            if scope == "app":
                app[category] = (rules, corrections)
            else:
                window[category] = rules
        cohort.corrections[name] = {"app": app, "window": window}
        if other_policy_until is not None:
            cohort.corrections_span_policy_change.append(name)


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


def _share(numerator: int, denominator: int) -> str:
    return f"{numerator}/{denominator}" if denominator else "not computable (denominator is 0)"


def _primary(cohort: Cohort) -> dict:
    censored = Counter()
    uncensored = 0
    for point in cohort.eligible:
        row, meta = point["row"], point["meta"]
        occurred = _int(row, "occurred_at") or 0
        horizon_end = occurred + PRIMARY_HORIZON_SECONDS
        ended = _int(row, "block_ended_at")
        exported = _int(meta, "exported_at")
        if ended is not None:
            if ended < horizon_end:
                censored["block ended before the horizon elapsed"] += 1
                continue
        elif exported is None:
            censored["block still open and export time unknown"] += 1
            continue
        elif exported < horizon_end:
            censored["the export ends inside the horizon"] += 1
            continue
        uncensored += 1
    eligible = len(cohort.eligible)
    return {
        "status": (
            "NOT COMPUTED. Pre-registered 2026-08-21. The numerator needs "
            "per-second anchor coverage from work_block_observation, which no "
            "export carries, and the script does not approximate it."
        ),
        "definition": (
            "Sustained anchor engagement: at each eligible decision point, whether "
            f"at least {PRIMARY_THRESHOLD_SECONDS} of the following "
            f"{PRIMARY_HORIZON_SECONDS} seconds were spent in the anchor category "
            "recorded on the decision row."
        ),
        "eligible_decision_points": eligible,
        "eligible_definition": (
            f"policy_version {ANALYSED_POLICY_VERSION} decision-log rows with gate_verdict 'offered', after "
            "the warm-up exclusion. With propensity fixed at 1.0 the shipped "
            "gate_verdict CHECK has no eligible-but-silent value, so the eligible "
            "set and the offered set are the same (2026-08-31 correction)."
        ),
        "censored_visible_in_export": dict(sorted(censored.items())),
        "not_censored_by_block_or_export_end": uncensored,
        "censoring_not_derivable": (
            "the service stopped, slept or lost the Accessibility observer inside "
            "the horizon: needs observation coverage the export does not carry"
        ),
        "numerator": None,
        "secondary_outcomes_not_computed": [
            "departure-free interval (no further anchor departure within 600 s)",
            "time to sustained return (first anchor run of at least 300 s)",
        ],
    }


def _blocks_per_week(cohort: Cohort, start: datetime | None, weeks: int) -> dict:
    per_participant = {}
    outside = 0
    for name in sorted(cohort.blocks):
        blocks = cohort.blocks[name]
        entry: dict = {"blocks_declared": len(blocks)}
        if start is not None:
            origin = int(start.timestamp())
            buckets = [0] * weeks
            for block in blocks:
                index = (block["started_at"] - origin) // WEEK_SECONDS
                if 0 <= index < weeks:
                    buckets[index] += 1
                else:
                    outside += 1
            entry["by_cohort_week"] = buckets
        per_participant[name] = entry
    measured = sorted(per_participant)
    result: dict = {
        "definition": (
            "Gate D measure 1: distinct declared work blocks per participant per "
            "cohort week, from the per-block CSV. Every declared block counts, "
            "whether or not it produced an offer; no warm-up exclusion applies."
        ),
        "participants_measured": len(measured),
        "participants_with_zero_blocks": [n for n in measured if not per_participant[n]["blocks_declared"]],
        "per_participant": per_participant,
        "not_measurable_for": sorted(
            p.name for p in cohort.analysed if p.name not in cohort.blocks
        ),
    }
    if start is not None:
        in_window = sum(sum(e["by_cohort_week"]) for e in per_participant.values())
        result["cohort_start_utc"] = start.date().isoformat()
        result["cohort_weeks"] = weeks
        result["blocks_outside_cohort_window"] = outside
        result["blocks_in_window"] = in_window
        result["participant_weeks"] = len(measured) * weeks
        result["blocks_per_participant_week"] = (
            f"{in_window}/{len(measured) * weeks}" if measured else "not computable (no participants)"
        )
    else:
        result["note"] = "Pass --cohort-start (and --cohort-weeks) to bucket by cohort week."
    return result


def _completion_by_origin(cohort: Cohort) -> dict:
    by_origin: dict = {}
    open_blocks = 0
    for blocks in cohort.blocks.values():
        for block in blocks:
            if block["phase"] in BLOCK_OPEN_PHASES:
                open_blocks += 1
                continue
            origin = block["origin"] or "not_recorded"
            entry = by_origin.setdefault(
                origin, {"completed": 0, "terminal": 0, "abandoned": 0, "expired": 0}
            )
            entry["terminal"] += 1
            entry[block["phase"]] += 1
    for origin in BLOCK_ORIGINS:
        by_origin.setdefault(origin, {"completed": 0, "terminal": 0, "abandoned": 0, "expired": 0})
    return {
        "definition": (
            "Per origin: blocks with phase 'completed' over blocks in a terminal "
            "phase (completed, abandoned, expired). No warm-up exclusion: a block "
            "abandoned in its first three minutes is the initiation failure this "
            "measures. Invitations are not randomized and acceptors self-select, "
            "so any difference between origins is ASSOCIATIONAL."
        ),
        "by_origin": dict(sorted(by_origin.items())),
        "open_at_export_excluded": open_blocks,
    }


def _invitations(cohort: Cohort) -> dict:
    counts = Counter()
    by_policy: dict = {}
    for rows in cohort.invitations.values():
        for row in rows:
            counts[row["outcome"]] += 1
            entry = by_policy.setdefault(row["policy_version"] or "?", Counter())
            entry[row["outcome"]] += 1
    terminal = sum(counts[o] for o in INVITATION_TERMINAL_OUTCOMES)
    turned_off = sorted(
        p.name for p in cohort.analysed if p.meta.get("invitations_enabled") == "0"
    )
    measured = sorted(cohort.invitations)
    return {
        "definition": (
            "Invitations with outcome 'accepted' over invitations in a terminal "
            "outcome (accepted, dismissed, no_response, expired). 'offered' is "
            "unresolved: counted and excluded."
        ),
        "measurable": bool(measured),
        "participants_measured": len(measured),
        "accepted": counts["accepted"],
        "terminal": terminal,
        "by_outcome": dict(sorted(counts.items())),
        "unresolved_offered_excluded": counts["offered"],
        "by_invitation_policy_version": {
            version: dict(sorted(c.items())) for version, c in sorted(by_policy.items())
        },
        "participants_with_invitations_off": turned_off,
        "not_measurable_for": sorted(
            p.name for p in cohort.analysed if p.name not in cohort.invitations
        ),
    }


def _explain(cohort: Cohort) -> dict:
    weeks = []
    for name in sorted(cohort.explain):
        for row in cohort.explain[name]:
            weeks.append(dict(row, participant=name))
    taps = sum(w["taps"] for w in weeks)
    delivered = sum(w["delivered"] for w in weeks)
    active = [w for w in weeks if w["blocks"] >= 1]
    active_tapped = [w for w in active if w["taps"] >= 1]
    return {
        "definition": (
            "Per participant per local week: explain_probe_week.taps over "
            "interventions delivered that week, counted on the tester's Mac with "
            "the app's own delivered predicate. R4 framing: weekly-active "
            "participant-weeks (at least one declared block) with at least one tap."
        ),
        "measurable": bool(cohort.explain),
        "participant_weeks": [
            {"participant": w["participant"], "week_start_local_date": w["week"],
             "taps": w["taps"], "delivered_interventions": w["delivered"],
             "blocks_declared": w["blocks"]}
            for w in weeks
        ],
        "taps": taps,
        "delivered_interventions": delivered,
        "weekly_active_participant_weeks": len(active),
        "weekly_active_participant_weeks_with_a_tap": len(active_tapped),
        "not_measurable_for": sorted(
            p.name for p in cohort.analysed if p.name not in cohort.explain
        ),
    }


def _corrections(cohort: Cohort) -> dict:
    per_participant = {}
    by_category = Counter()
    for name in sorted(cohort.corrections):
        app = cohort.corrections[name]["app"]
        window = cohort.corrections[name]["window"]
        per_participant[name] = {
            "app_scoped_corrections": sum(c for _, c in app.values()),
            "applications_with_an_app_rule": sum(r for r, _ in app.values()),
            "window_rules": sum(window.values()),
            "app_scoped_corrections_by_category": {
                category: c for category, (_, c) in sorted(app.items())
            },
        }
        for category, (_, c) in app.items():
            by_category[category] += c
    measured = sorted(per_participant)
    return {
        "definition": (
            "2026-08-17 addition, measure 3: the count of app-scoped classification "
            "corrections per participant. Reported as a count only. It measures how "
            "far the seed dictionary missed that person's apps: not engagement, and "
            "not to be presented as such."
        ),
        "how_counted": (
            "The sum of correction_count over the participant's app-scoped rules "
            "(personal_app_override, migration 0017), from the corrections CSV. A "
            "rule starts at 1 and every later correction that lands on the same "
            "application adds 1, whether it came from correcting an activity or "
            "from the list of apps Velvt could not read. Applications with a rule "
            "and window rules (personal_override, which keeps no count) are "
            "descriptive."
        ),
        "lower_bound": (
            "Only rules that exist at export are counted. A correction the "
            "participant removed, or a Reset, takes its count with it."
        ),
        "not_counted": (
            "The 'Wrong category' reply to a drift offer is block-scoped "
            "(work_block_category_correction), not a classification rule. It is "
            "counted in the trust figure as wrong_classification. A correction of a "
            "browser tab is window-scoped only and appears under window rules."
        ),
        "measurable": bool(measured),
        "participants_measured": len(measured),
        "per_participant": per_participant,
        "participants_with_zero_app_scoped_corrections": [
            n for n in measured if not per_participant[n]["app_scoped_corrections"]
        ],
        "total_app_scoped_corrections": sum(by_category.values()),
        "app_scoped_corrections_by_category": dict(sorted(by_category.items())),
        "includes_history_before_analysed_policy": sorted(cohort.corrections_span_policy_change),
        "not_measurable_for": sorted(
            p.name for p in cohort.analysed if p.name not in cohort.corrections
        ),
    }


def _integrity(cohort: Cohort) -> dict:
    verdicts = Counter()
    deviations = 0
    anchor_seen = Counter()
    for record in cohort.analysed_decisions:
        row = record["row"]
        verdicts[_text(row, "gate_verdict")] += 1
        propensity = _float(row, "propensity")
        if propensity is None or abs(propensity - 1.0) > 1e-9:
            deviations += 1
        value = _text(row, "anchor_seen_within_600s")
        anchor_seen[value if value in ("0", "1") else "NULL"] += 1
    return {
        "definition": (
            f"Every policy_version {ANALYSED_POLICY_VERSION} decision-log row of the analysed participants. "
            "No warm-up exclusion: this describes the log, not an outcome."
        ),
        "rows": len(cohort.analysed_decisions),
        "by_gate_verdict": dict(sorted(verdicts.items())),
        "propensity_not_1_0_protocol_deviations": deviations,
        "anchor_seen_within_600s": {
            "1": anchor_seen["1"], "0": anchor_seen["0"], "NULL": anchor_seen["NULL"],
        },
        "anchor_seen_note": "NULL is unresolved. It is never imputed and never counted as a failure.",
    }


def analyse(cohort: Cohort, cohort_start: datetime | None = None, cohort_weeks: int = 1) -> dict:
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
    eligible = len(cohort.eligible)
    excluded_reasons = Counter(e.category for e in cohort.excluded)
    if cohort.excluded_whole:
        excluded_reasons["participant excluded whole"] += len(cohort.excluded_whole)
    if cohort.decisions_by_policy:
        other_policy = sum(
            n for v, n in cohort.decisions_by_policy.items() if v != ANALYSED_POLICY_VERSION
        )
        if other_policy:
            excluded_reasons[
                f"decision rows not under policy_version {ANALYSED_POLICY_VERSION}"
            ] += other_policy

    return {
        "participants": {
            "exports_received": len(cohort.exports_received),
            "analysed": len(cohort.analysed),
            "excluded_whole": dict(sorted(cohort.excluded_whole.items())),
            "with_at_least_one_offer": len({o.participant for o in offers}),
            "with_at_least_one_withheld": len({o.participant for o in withheld}),
            "exported_zero_offers": sorted(cohort.empty_exports),
        },
        "policy": {
            "analysed_policy_version": ANALYSED_POLICY_VERSION,
            "rule": (
                "2026-09-25 amendment and 2026-09-26 note: policy_version 1, 2 and 3 "
                "are never pooled and every result uses policy_version "
                f"{ANALYSED_POLICY_VERSION} only. An intervention row takes "
                "the policy_version of the decision-log row for the same block_id "
                "whose gate_verdict is offered, withheld_demotion or suppressed_dnd; "
                "one with no such row is counted and excluded."
            ),
            "decisions_by_policy_version": {
                str(k): v for k, v in sorted(cohort.decisions_by_policy.items())
            },
            "interventions_by_attribution": dict(sorted(cohort.offers_by_attribution.items())),
            "rows_before_last_other_policy_decision_excluded": dict(
                sorted(cohort.era_excluded.items())
            ),
            "non_monotonic_policy_history": sorted(cohort.non_monotonic_policy),
        },
        "decisions_recorded": {
            "definition": (
                f"Every policy-{ANALYSED_POLICY_VERSION} intervention row the gate wrote, delivered or not, "
                "after exclusions. Reported so the delivered denominator can be "
                "audited against it."
            ),
            "total": denominator + len(withheld),
            "delivered": denominator,
            "withheld": len(withheld),
        },
        "power": {
            "required_decision_points": POWER_REQUIRED_DECISION_POINTS,
            "observed_eligible_decision_points": eligible,
            "assumptions": POWER_ASSUMPTIONS,
            "sufficient": eligible >= POWER_REQUIRED_DECISION_POINTS,
            "note": (
                f"Counts policy_version {ANALYSED_POLICY_VERSION} eligible decision points only, before "
                "censoring, so it is an upper bound on how close this data gets."
            ),
        },
        "primary_outcome": _primary(cohort),
        "retired_return_within_10min": {
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
        "card_seen": {
            "definition": (
                "From card_seen_at (migration 0032). seen: the in-app card was on "
                "screen. unseen: NULL on a row offered after the Mac had 0032. "
                "unknown: NULL before that, including every 1.0.9 row. A delivery "
                "diagnostic, not an outcome."
            ),
            "no_response": {b: sum(1 for o in silent if o.card_seen == b) for b in CARD_SEEN_BUCKETS},
            "delivered": {b: sum(1 for o in offers if o.card_seen == b) for b in CARD_SEEN_BUCKETS},
        },
        "withheld": {
            "definition": (
                "Decisions the gate made and did not deliver. Terminal at "
                "creation, delivered by no channel, and excluded from every "
                "delivered denominator: a nudge nobody saw cannot be returned to "
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
        },
        "outcome_distribution": dict(sorted(Counter(o.outcome for o in offers).items())),
        "outcome_distribution_note": (
            "Delivered offers only. Withheld decisions are in the 'withheld' block."
        ),
        "salience_split": by_salience,
        "blocks_per_participant": _blocks_per_week(cohort, cohort_start, cohort_weeks),
        "completion_by_origin": _completion_by_origin(cohort),
        "invitation_acceptance": _invitations(cohort),
        "explain_tap_rate": _explain(cohort),
        "corrections_per_participant": _corrections(cohort),
        "decision_log_integrity": _integrity(cohort),
        "exclusions": {
            "declared_in_advance": [
                f"blocks whose elapsed time (ended_at - started_at - total_paused_seconds) "
                f"is under {WARMUP_EXCLUSION_SECONDS}s, below the policy v2 and v3 warm-up "
                "(2026-09-25; was 300 s)",
                "the founder's own device",
                "any participant who reinstalled mid-cohort (local history resets with the database)",
                f"rows not attributable to policy_version {ANALYSED_POLICY_VERSION}, and "
                "exports without the decision log (2026-09-25, 2026-09-26)",
            ],
            "applied_here": sum(excluded_reasons.values()),
            "by_reason": dict(sorted(excluded_reasons.items())),
            "founder_devices_excluded": sorted(cohort.founder_devices),
            "participants_excluded_whole": dict(sorted(cohort.excluded_whole.items())),
            "detail": [
                {"participant": e.participant, "row": e.row, "reason": e.reason}
                for e in cohort.excluded
            ],
        },
        "data_quality": {"malformed": cohort.malformed},
    }


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def _ratio(numerator: int, denominator: int) -> str:
    if denominator == 0:
        return "not computable (denominator is 0)"
    ratio = f"{numerator}/{denominator} = {numerator / denominator:.1%}"
    if denominator < POWER_REQUIRED_DECISION_POINTS:
        return f"{ratio} — underpowered, see POWER above"
    return f"{ratio} — see POWER above"


def render(result: dict) -> str:
    out: list[str] = []
    add = out.append

    def heading(title: str) -> None:
        add("")
        add(title)
        add("-" * 64)

    def para(text: str, indent: str = "  ") -> None:
        add(textwrap.fill(text, width=76, initial_indent=indent, subsequent_indent=indent))

    p = result["participants"]
    add(f"COHORT ANALYSIS (drift policy v{ANALYSED_POLICY_VERSION} only)")
    add("=" * 64)
    add(f"exports received:        {p['exports_received']}")
    add(f"  analysed:              {p['analysed']}")
    for name, reason in p["excluded_whole"].items():
        add(f"  excluded whole:        {name}: {reason}")
    add(f"  with >=1 offer:        {p['with_at_least_one_offer']}")
    add(f"  with >=1 withheld:     {p['with_at_least_one_withheld']}")
    add(f"  exported zero offers:  {len(p['exported_zero_offers'])} {p['exported_zero_offers'] or ''}")
    if p["analysed"] and not p["with_at_least_one_offer"]:
        add("")
        add("  No offer was delivered to anyone. That is a result, not a failure")
        add("  of collection: the gate's thresholds were never met, or every")
        add("  decision it did make was withheld. Read BLOCKS and WITHHELD below")
        add("  before concluding which.")

    policy = result["policy"]
    heading("POLICY")
    para(policy["rule"])
    add(f"  decision rows by policy_version: {policy['decisions_by_policy_version'] or '{}'}")
    add(f"  interventions by attribution:    {policy['interventions_by_attribution'] or '{}'}")
    if policy["rows_before_last_other_policy_decision_excluded"]:
        add(f"  rows before a Mac's last decision under another policy, excluded: "
            f"{policy['rows_before_last_other_policy_decision_excluded']}")
    if policy["non_monotonic_policy_history"]:
        add(f"  ! policy history goes back and forth on: {policy['non_monotonic_policy_history']}")

    power = result["power"]
    heading("POWER")
    add(f"  Detecting the pre-registered effect needs about "
        f"{power['required_decision_points']} eligible decision")
    add("  points, under assumptions none of which are measured:")
    para(f"{power['assumptions']}.", "    ")
    add(f"  This data carries {power['observed_eligible_decision_points']} "
        f"(policy_version {ANALYSED_POLICY_VERSION}, before censoring).")
    if not power["sufficient"]:
        add("  Every ratio below is reported for completeness and is not a finding.")

    primary = result["primary_outcome"]
    heading("PRIMARY OUTCOME: sustained anchor engagement (2026-08-21) — NOT COMPUTED HERE")
    para(primary["definition"])
    add(f"  eligible decision points (denominator before censoring): "
        f"{primary['eligible_decision_points']}")
    for reason, count in primary["censored_visible_in_export"].items():
        add(f"  censored, {reason}: {count}")
    add(f"  not censored by block end or export end: "
        f"{primary['not_censored_by_block_or_export_end']}")
    para(f"censoring the export cannot see: {primary['censoring_not_derivable']}.")
    add("  NUMERATOR NOT COMPUTED: it needs per-second anchor coverage from")
    add("  work_block_observation, which no export carries. This script does not")
    add("  approximate it, and anchor_seen_within_600s is not a substitute.")

    retired = result["retired_return_within_10min"]
    heading("DESCRIPTIVE — RETIRED 2026-08-21, NOT THE PRIMARY OUTCOME")
    add("  Pre-registered 2026-08-09 and retired by the 2026-08-21 amendment for a")
    add("  ceiling effect readable in the drift gate's own source: the gate only")
    add("  fires on someone who has already been returning to the anchor inside")
    add("  the same ten minutes, so this measures the gate's selection rule")
    add("  rather than what the intervention changed. Never the headline.")
    para(retired["definition"])
    add(f"  returned within {retired['window_seconds']}s: "
        f"{_ratio(retired['numerator'], retired['denominator'])}")
    if retired["unbounded_returned_numerator"] != retired["numerator"]:
        add(f"  unbounded 'returned' would report {retired['unbounded_returned_numerator']}"
            f"/{retired['denominator']}: overstated, do not use")

    trust = result["trust"]
    heading("TRUST (wrong-intervention rate)")
    add(f"  was_focused:          {_ratio(trust['was_focused'], trust['denominator'])}")
    add(f"  wrong_classification: {_ratio(trust['wrong_classification'], trust['denominator'])}")
    add(f"  combined:             {_ratio(trust['combined_numerator'], trust['denominator'])}")
    if trust["denominator"]:
        rate = trust["combined_numerator"] / trust["denominator"]
        if rate > trust["auto_demotion_threshold"]:
            add(f"  ABOVE the {trust['auto_demotion_threshold']:.0%} auto-demotion threshold.")

    heading("OUTCOME DISTRIBUTION (delivered only)")
    if result["outcome_distribution"]:
        for outcome, count in result["outcome_distribution"].items():
            add(f"  {outcome:24} {count}")
    else:
        add("  (no delivered offers)")
    add(f"  silence (no_response): {result['silence']['no_response']}, "
        "not a refusal, stays in the denominator")

    seen = result["card_seen"]
    heading("NO_RESPONSE BY CARD_SEEN_AT (delivery diagnostic)")
    add("  seen: the card was on screen. unseen: it never reached them.")
    add("  unknown: recorded before migration 0032 (every 1.0.9 row). Never unseen.")
    for bucket in CARD_SEEN_BUCKETS:
        add(f"  {bucket:8} no_response={seen['no_response'][bucket]:4} "
            f"of delivered={seen['delivered'][bucket]:4}")

    held = result["withheld"]
    heading("WITHHELD (recorded, never delivered)")
    para(held["definition"])
    if held["by_outcome"]:
        for outcome, count in held["by_outcome"].items():
            reason = held["reasons"].get(outcome, "")
            add(f"  {outcome:24} {count}   {reason}")
    elif result["decisions_recorded"]["total"]:
        add("  (none: every recorded decision was delivered)")
    else:
        add("  (no decisions were recorded at all, so none could be withheld)")
    add(f"  share of recorded decisions: {held['share_of_recorded_decisions']}")
    add("  These rows are NOT in the retired-figure or trust denominators.")

    heading("SALIENCE SPLIT")
    add("  A quiet offer rendered the in-app card and sent no notification, so an")
    add("  ignored quiet offer never rang. Pooling understates responsiveness.")
    for salience, stats in result["salience_split"].items():
        add(f"  {salience:8} offers={stats['offers']:4} "
            f"returned={stats['returned_within_10min']:4} "
            f"no_response={stats['no_response']:4}")

    blocks = result["blocks_per_participant"]
    heading("BLOCKS DECLARED PER PARTICIPANT (Gate D measure 1)")
    for name, entry in blocks["per_participant"].items():
        weekly = f"  by cohort week {entry['by_cohort_week']}" if "by_cohort_week" in entry else ""
        add(f"  {name:24} {entry['blocks_declared']:4} block(s){weekly}")
    add(f"  participants with zero blocks: {len(blocks['participants_with_zero_blocks'])} "
        f"{blocks['participants_with_zero_blocks'] or ''}")
    if "blocks_per_participant_week" in blocks:
        add(f"  blocks per participant-week: {blocks['blocks_per_participant_week']} "
            f"(cohort from {blocks['cohort_start_utc']} UTC, {blocks['cohort_weeks']} week(s); "
            f"{blocks['blocks_outside_cohort_window']} outside the window)")
    else:
        add(f"  {blocks['note']}")
    if blocks["not_measurable_for"]:
        add(f"  no per-block CSV, not measurable: {blocks['not_measurable_for']}")

    completion = result["completion_by_origin"]
    heading("INVITED VERSUS SELF-DECLARED COMPLETION (associational)")
    for origin, entry in completion["by_origin"].items():
        add(f"  {origin:12} completed {_ratio(entry['completed'], entry['terminal'])}")
        add(f"  {'':12} abandoned={entry['abandoned']} expired={entry['expired']}")
    add(f"  still active or paused at export, excluded: {completion['open_at_export_excluded']}")

    invitations = result["invitation_acceptance"]
    heading("INVITATION ACCEPTANCE")
    if invitations["measurable"]:
        add(f"  accepted: {_ratio(invitations['accepted'], invitations['terminal'])}")
        add(f"  by outcome: {invitations['by_outcome'] or '{}'}")
        add(f"  unresolved ('offered'), excluded: {invitations['unresolved_offered_excluded']}")
        add(f"  by invitation policy_version: {invitations['by_invitation_policy_version'] or '{}'}")
        add(f"  participants who turned invitations off: "
            f"{len(invitations['participants_with_invitations_off'])} "
            f"{invitations['participants_with_invitations_off'] or ''}")
    else:
        add("  not measurable from the cohort export (no invitations file)")
    if invitations["not_measurable_for"]:
        add(f"  no invitations file: {invitations['not_measurable_for']}")

    explain = result["explain_tap_rate"]
    heading("EXPLAIN-TAP RATE (R4 gate probe)")
    if explain["measurable"]:
        for week in explain["participant_weeks"]:
            add(f"  {week['participant']:24} week of {week['week_start_local_date']}: "
                f"taps={week['taps']} delivered={week['delivered_interventions']} "
                f"blocks={week['blocks_declared']}")
        add(f"  all participant-weeks: taps {explain['taps']} over "
            f"{explain['delivered_interventions']} delivered")
        add(f"  weekly-active participant-weeks with a tap: "
            f"{_ratio(explain['weekly_active_participant_weeks_with_a_tap'], explain['weekly_active_participant_weeks'])}")
    else:
        add("  not measurable from the cohort export (no explain file)")
    if explain["not_measurable_for"]:
        add(f"  no explain file: {explain['not_measurable_for']}")

    corrections = result["corrections_per_participant"]
    heading("CORRECTIONS PER PARTICIPANT (2026-08-17 measure 3; a count, not engagement)")
    para(corrections["definition"])
    if corrections["measurable"]:
        for name, entry in corrections["per_participant"].items():
            add(f"  {name:24} {entry['app_scoped_corrections']:4} app-scoped correction(s) "
                f"on {entry['applications_with_an_app_rule']} app(s); "
                f"{entry['window_rules']} window rule(s)")
        add(f"  app-scoped corrections by category: "
            f"{corrections['app_scoped_corrections_by_category'] or '{}'}")
        para(f"Each count is a lower bound. {corrections['lower_bound']}")
    else:
        add("  not measurable from the cohort export (no corrections file)")
    if corrections["not_measurable_for"]:
        add(f"  no corrections file: {corrections['not_measurable_for']}")
    if corrections["includes_history_before_analysed_policy"]:
        add(f"  ! counts include time before policy v{ANALYSED_POLICY_VERSION}, not separable: "
            f"{corrections['includes_history_before_analysed_policy']}")

    integrity = result["decision_log_integrity"]
    heading(f"DECISION-LOG INTEGRITY (policy_version {ANALYSED_POLICY_VERSION} rows)")
    add(f"  rows: {integrity['rows']}")
    for verdict, count in integrity["by_gate_verdict"].items():
        add(f"  {verdict:24} {count}")
    add(f"  propensity other than 1.0 (protocol deviation): "
        f"{integrity['propensity_not_1_0_protocol_deviations']}")
    seen_600 = integrity["anchor_seen_within_600s"]
    add(f"  anchor_seen_within_600s: 1={seen_600['1']} 0={seen_600['0']} "
        f"NULL={seen_600['NULL']} (NULL is unresolved, never imputed)")

    exclusions = result["exclusions"]
    heading("EXCLUSIONS (declared in advance)")
    for line in exclusions["declared_in_advance"]:
        add(textwrap.fill(line, width=76, initial_indent="  - ", subsequent_indent="    "))
    add(f"  applied to this data: {exclusions['applied_here']} exclusion(s)")
    for reason, count in exclusions["by_reason"].items():
        add(f"    {count:4}  {reason}")
    founder = exclusions["founder_devices_excluded"]
    if founder:
        add(f"  founder devices dropped whole: {', '.join(founder)}")
    else:
        add("  founder devices dropped whole: none. No export was named "
            "`founder`/`founder-*`")
        add("  and none was declared with --founder-device, so this exclusion "
            "removed nothing.")

    if result["data_quality"]["malformed"]:
        heading("DATA QUALITY")
        for issue in result["data_quality"]["malformed"]:
            add(f"  ! {issue}")

    add("")
    add("Report numerator and denominator, never the ratio alone.")
    return "\n".join(out)


def _date(value: str) -> datetime:
    try:
        return datetime.strptime(value, "%Y-%m-%d").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"expected YYYY-MM-DD, got {value!r}") from error


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "exports", nargs="+", type=Path,
        help="one folder per participant, or the exported CSV files themselves",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a report")
    parser.add_argument(
        "--founder-device", action="append", default=[], metavar="NAME",
        help="drop this export whole, per the pre-registered founder-device "
             "exclusion; repeatable. Exports named `founder` or `founder-*` "
             "are dropped without the flag.",
    )
    parser.add_argument(
        "--reinstalled", action="append", default=[], metavar="NAME",
        help="drop this participant whole: they reinstalled mid-cohort, so "
             "their local history reset (pre-registered exclusion); repeatable.",
    )
    parser.add_argument(
        "--cohort-start", type=_date, default=None, metavar="YYYY-MM-DD",
        help="UTC date the cohort week(s) start, for blocks per participant per week",
    )
    parser.add_argument(
        "--cohort-weeks", type=int, default=1, metavar="N",
        help="number of cohort weeks from --cohort-start (default 1)",
    )
    args = parser.parse_args()

    missing = [p for p in args.exports if not p.exists()]
    if missing:
        print(f"ERROR: no such file: {', '.join(str(p) for p in missing)}", file=sys.stderr)
        return 1
    if args.cohort_weeks < 1:
        print("ERROR: --cohort-weeks must be at least 1", file=sys.stderr)
        return 1

    cohort = load(args.exports, tuple(args.founder_device), tuple(args.reinstalled))
    result = analyse(cohort, args.cohort_start, args.cohort_weeks)
    print(json.dumps(result, indent=2) if args.json else render(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
