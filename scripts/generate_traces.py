#!/usr/bin/env python3
"""Generates SYNTHETIC behavioural traces for the drift-gate replay harness.

Everything this writes is synthetic. No real user has ever touched it, and the
word SYNTHETIC is in every filename and in the header record of every file so
that a number lifted out of here cannot reach a slide unlabelled.

The traces are replayed by `rust-service/tests/trace_replay.rs` through the
REAL `WorkBlockManager::observe_safe_category` against a fresh in-memory
database per trace. Nothing is injected below the ingestion layer and nothing
is stubbed, so what the harness scores is the shipped drift gate.

Two suites, and they answer different questions.

  A — recovery.  Hand-authored patterns with known ground truth. Half of them
      are planted drift the gate SHOULD detect; the other half sit one step
      outside each of the gate's thresholds and it should abstain. A suite of
      positives alone would be passed by a gate that always fires, which is
      the failure mode that matters.

  B — null.  Pure noise: category drawn i.i.d. from a fixed distribution,
      dwell drawn from a fixed log-normal, no dependence on time of day, day
      of week, prior state, or any outcome. Acceptance: ZERO offers.

      B also ships a second arm, `null-compressed`, which is the same
      structureless generator with the dwell distribution scaled down. It must
      produce SOME offers. Without it, "zero offers on noise" is unfalsifiable
      — a harness that never calls the gate would also report zero.

============================================================================
THE CLOCK RULE — read before changing the emitter
============================================================================

`WorkBlockManager` stores each observation at the timestamp it is given
(`append_observation`, `persistence/sqlite.rs`), and `effective_now(record,
now) = now.max(record.updated_at)` is a MONOTONIC FLOOR, not a clock read.
`append_observation` also does `updated_at = MAX(updated_at, occurred_at)`.

That floor enforces exactly two rules, and traces must respect both:

  1. No observation may start before the block's `started_at`. An earlier
     timestamp is pulled forward to it.
  2. No observation may go backwards relative to the previous one.

A trace whose offsets START AFTER the block start and INCREASE MONOTONICALLY
is therefore recorded at its true timestamps, through the real path, at any
speed. A backdated or shuffled trace collapses to a single instant: every
dwell computes to zero, `dominant_category` finds no anchor, and the gate
correctly abstains — producing a silent, meaningless pass.

So the emitter below refuses to write a non-monotonic trace, and suite A
carries one deliberately-backdated trace (`A-TRAP-BACKDATED`) whose expected
result is "no offer, for the wrong reason", so the trap is demonstrated rather
than described.

Usage:
    ./scripts/generate_traces.py                    # writes scripts/traces/
    ./scripts/generate_traces.py --out /tmp/traces
    ./scripts/generate_traces.py --null-traces 100 --seed 20260821
    ./scripts/generate_traces.py --check            # regenerate and diff
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import random
import sys
from pathlib import Path

GENERATOR_VERSION = 1
TRACE_SCHEMA = "velvt-trace/1"
DEFAULT_SEED = 20260821

# ---------------------------------------------------------------------------
# The shipped gate's constants, copied here so the expectations below can be
# read without opening the Rust. They are FROZEN in `work_block/mod.rs`; this
# file must never be the thing that changes them.
# ---------------------------------------------------------------------------
DRIFT_WINDOW_SECONDS = 600
DRIFT_MIN_SWITCHES = 4
DRIFT_MIN_ELAPSED_SECONDS = 300
DRIFT_MIN_REMAINING_SECONDS = 120

# From `rust-service/resources/abstraction-taxonomy-mvp-1.json`. Do not invent
# categories: a fixture exercising a category the product cannot produce
# validates nothing.
TAXONOMY = (
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    "UNLOGGED",
)

# `is_confident_evidence` in `work_block/mod.rs` drops these three categories
# whatever their status, so they can never anchor a block or count as a switch.
NON_EVIDENCE_CATEGORIES = ("SYSTEM", "UNLOGGED", "UNCLASSIFIED")

# ---------------------------------------------------------------------------
# ASSUMPTIONS. Every number below is an assumption, not a measurement, except
# where marked MEASURED. Revisit all of them the moment real cohort data
# exists — an engine validated only against assumed dynamics has been
# validated against its author's beliefs.
# ---------------------------------------------------------------------------

# MEASURED, n=1, founder's own Mac, raw_event_buffer, 23,026 events over 8
# local days, read on 2026-08-21. Event-count marginals, NOT dwell-weighted.
# One person is not a population; this is a plausible shape, not a prior.
CATEGORY_WEIGHTS = {
    "FOCUS_WORK": 0.534,
    "REFERENCE": 0.241,
    "COMMUNICATION": 0.099,
    "PASSIVE_CONSUMPTION": 0.079,
    "UNLOGGED": 0.031,
    "SOCIAL_FEED": 0.010,
    "SYSTEM": 0.006,
    "TASK_MANAGEMENT": 0.0003,
}

# ASSUMPTION. Log-normal dwell, medians per the fixtures README's stated
# ranges (COMMUNICATION 2-4 min, FOCUS_WORK 8-20 min). No real data supports
# the shape or the spread.
DWELL_MEDIAN_SECONDS = {
    "FOCUS_WORK": 840,
    "REFERENCE": 300,
    "COMMUNICATION": 180,
    "PASSIVE_CONSUMPTION": 420,
    "SOCIAL_FEED": 150,
    "TASK_MANAGEMENT": 120,
    "SYSTEM": 60,
    "UNLOGGED": 120,
}
DWELL_SIGMA = 0.9

# MEASURED, same source and caveat. Status and confidence are not independent
# in the shipped classifier — every `classified` row is high or medium, every
# `ambiguous` row is low, every `unclassified` row is none — so they are drawn
# as one joint distribution rather than two marginals.
CLASSIFICATION_MIX = (
    (("classified", "high"), 11653 / 23026),
    (("classified", "medium"), 6373 / 23026),
    (("ambiguous", "low"), 4738 / 23026),
    (("unclassified", "none"), 262 / 23026),
)

# ASSUMPTION. Within the shipped bounds of 300-10800 seconds.
NULL_BLOCK_SECONDS = 3600


class NonMonotonicTrace(ValueError):
    """The clock rule above, enforced instead of documented."""


def observation(t: int, category: str, status: str = "classified",
                confidence: str = "high") -> dict:
    return {"t": t, "category": category, "status": status, "confidence": confidence}


def block(
    observations: list[dict],
    planned_duration_seconds: int = 3600,
    purpose: str = "deep_work",
    intensity: str = "medium",
    allow_non_monotonic: bool = False,
    where: str = "block",
) -> dict:
    """One declared block and the observations made inside it.

    Refuses a non-monotonic sequence unless explicitly told otherwise. See THE
    CLOCK RULE at the top of this file: a backdated observation is pulled
    forward by the monotonic floor, every dwell collapses to zero, no anchor is
    found, and the gate abstains for a reason that has nothing to do with the
    pattern. That is a silent, meaningless pass, and it is the exact failure
    this guard exists to prevent.
    """
    if not allow_non_monotonic:
        previous = 0
        for entry in observations:
            if entry["t"] <= previous:
                raise NonMonotonicTrace(
                    f"{where}: offset {entry['t']} does not advance past {previous}"
                )
            previous = entry["t"]
        if observations and observations[0]["t"] < 1:
            raise NonMonotonicTrace(
                f"{where}: the first offset must be at least 1 second after the "
                "block start, or it is pulled forward to it"
            )
    for entry in observations:
        if entry["category"] not in TAXONOMY:
            raise ValueError(f"{where}: {entry['category']} is not in the shipped taxonomy")

    return {
        "planned_duration_seconds": planned_duration_seconds,
        "purpose": purpose,
        "intensity": intensity,
        "observations": observations,
    }


def trace(
    trace_id: str,
    family: str,
    blocks: list[dict],
    expect_offer: bool,
    expect_reason: str,
    seed: int | None = None,
) -> dict:
    return {
        "kind": "trace",
        "schema": TRACE_SCHEMA,
        "synthetic": True,
        "trace_id": trace_id,
        "family": family,
        "seed": seed,
        "blocks": blocks,
        "expect_offer": expect_offer,
        "expect_reason": expect_reason,
    }


def single_block_trace(
    trace_id: str,
    family: str,
    observations: list[dict],
    expect_offer: bool,
    expect_reason: str,
    planned_duration_seconds: int = 3600,
    allow_non_monotonic: bool = False,
) -> dict:
    """Suite A's shape: one hand-authored block whose ground truth is known."""
    return trace(
        trace_id,
        family,
        [block(
            observations,
            planned_duration_seconds=planned_duration_seconds,
            allow_non_monotonic=allow_non_monotonic,
            where=trace_id,
        )],
        expect_offer,
        expect_reason,
    )


# ---------------------------------------------------------------------------
# Suite A — recovery. Hand-authored, so the ground truth is known by
# construction rather than by trusting a sampler.
# ---------------------------------------------------------------------------

def _drift_burst(start: int, anchor: str, away: list[str], step: int = 20) -> list[dict]:
    """Alternates anchor and non-anchor, one departure per `away` entry.

    A departure is a confident non-anchor observation whose previous confident
    observation was the anchor — that is the gate's own definition, and it is
    why the anchor has to be re-observed between departures.
    """
    entries: list[dict] = []
    t = start
    for index, category in enumerate(away):
        entries.append(observation(t, category))
        t += step
        if index != len(away) - 1:
            entries.append(observation(t, anchor))
            t += step
    return entries


def suite_a() -> list[dict]:
    traces: list[dict] = []

    # ---- planted drift the gate must detect --------------------------------

    # Anchor dwell 10 -> 400 gives FOCUS_WORK 390 confident seconds before any
    # departure, so `dominant_category` cannot pick anything else. Four
    # departures land inside one 600s window, elapsed at the last one is 520
    # (>= 300), remaining is 3080 (>= 120), and the last confident observation
    # is not the anchor.
    traces.append(single_block_trace(
        "A-PLANT-4SWITCH-FOCUS",
        "PLANTED",
        [observation(10, "FOCUS_WORK")] + _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 4),
        True,
        "4 departures from FOCUS_WORK inside 600s, ending away from the anchor",
    ))

    traces.append(single_block_trace(
        "A-PLANT-6SWITCH-FOCUS",
        "PLANTED",
        [observation(10, "FOCUS_WORK")] + _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 6),
        True,
        "6 departures, comfortably past DRIFT_MIN_SWITCHES",
    ))

    traces.append(single_block_trace(
        "A-PLANT-REFERENCE-ANCHOR",
        "PLANTED",
        [observation(10, "REFERENCE")] + _drift_burst(400, "REFERENCE", ["SOCIAL_FEED"] * 4),
        True,
        "the anchor is whatever holds the most confident time, not a fixed category",
    ))

    traces.append(single_block_trace(
        "A-PLANT-MIXED-DEPARTURES",
        "PLANTED",
        [observation(10, "FOCUS_WORK")] + _drift_burst(
            400, "FOCUS_WORK",
            ["COMMUNICATION", "SOCIAL_FEED", "PASSIVE_CONSUMPTION", "COMMUNICATION"],
        ),
        True,
        "departures need not be to the same category; leaving the anchor is the signal",
    ))

    traces.append(single_block_trace(
        "A-PLANT-MEDIUM-CONFIDENCE",
        "PLANTED",
        [observation(10, "FOCUS_WORK")] + [
            observation(entry["t"], entry["category"], "classified", "medium")
            for entry in _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 4)
        ],
        True,
        "medium confidence is still confident evidence",
    ))

    # The burst starts late in a long block. Nothing about the gate is
    # positional; this exists because a threshold that only fires early would
    # pass every test above.
    traces.append(single_block_trace(
        "A-PLANT-LATE-BURST",
        "PLANTED",
        [observation(10, "FOCUS_WORK")] + _drift_burst(2000, "FOCUS_WORK", ["COMMUNICATION"] * 4),
        True,
        "a burst 2000s into the block fires the same as one at 400s",
    ))

    # ---- one step outside each threshold: the gate must abstain ------------

    traces.append(single_block_trace(
        "A-NEAR-3SWITCH",
        "NEAR_MISS",
        [observation(10, "FOCUS_WORK")] + _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 3),
        False,
        f"3 departures is below DRIFT_MIN_SWITCHES={DRIFT_MIN_SWITCHES}",
    ))

    # Four departures, all of them inside the warm-up, so the gate has no
    # evaluation point at which it could have fired. By the time elapsed
    # passes 300 the user is back at the anchor — and the shipped gate refuses
    # to offer at that instant, because an offer then would be untrue, would
    # self-resolve as `returned` without a fresh departure, and would invite an
    # honest `was_focused` reply that pollutes the wrong-intervention rate.
    #
    # The re-observation at t=310 carries medium confidence rather than high
    # so it is not de-duplicated against the identical row at t=170.
    _back_at_anchor = [
        observation(10, "FOCUS_WORK"),
        observation(100, "COMMUNICATION"),
        observation(110, "FOCUS_WORK"),
        observation(120, "COMMUNICATION"),
        observation(130, "FOCUS_WORK"),
        observation(140, "COMMUNICATION"),
        observation(150, "FOCUS_WORK"),
        observation(160, "COMMUNICATION"),
        observation(170, "FOCUS_WORK"),
        observation(310, "FOCUS_WORK", "classified", "medium"),
    ]
    traces.append(single_block_trace(
        "A-NEAR-ENDS-ON-ANCHOR",
        "NEAR_MISS",
        list(_back_at_anchor),
        False,
        "4 departures are on the record, but the latest confident evidence is "
        "the anchor: the user is back at the block",
    ))

    # The evidence is not discarded, only deferred. The same trace with one
    # more departure fires immediately — which is what makes the abstention
    # above a postponement rather than a miss.
    traces.append(single_block_trace(
        "A-PLANT-DEFERRED-TO-NEXT-DEPARTURE",
        "PLANTED",
        _back_at_anchor + [observation(330, "COMMUNICATION")],
        True,
        "the accumulated departures fire on the next confident non-anchor "
        "observation once the warm-up has passed",
    ))

    # Four departures, but spread over 2400s so no 600-second window holds
    # more than two of them.
    traces.append(single_block_trace(
        "A-NEAR-SPREAD-BEYOND-WINDOW",
        "NEAR_MISS",
        [observation(10, "FOCUS_WORK")]
        + _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 4, step=400),
        False,
        f"the departures do not fit inside DRIFT_WINDOW_SECONDS={DRIFT_WINDOW_SECONDS}",
    ))

    # Everything happens inside the warm-up and the trace then stops, so the
    # gate never gets an evaluation point at which elapsed >= 300.
    traces.append(single_block_trace(
        "A-NEAR-INSIDE-WARMUP",
        "NEAR_MISS",
        [observation(5, "FOCUS_WORK")] + _drift_burst(100, "FOCUS_WORK", ["COMMUNICATION"] * 4, step=20),
        False,
        f"every evaluation point is below DRIFT_MIN_ELAPSED_SECONDS={DRIFT_MIN_ELAPSED_SECONDS}",
    ))

    # A 660s block: by the time the fourth departure lands at 560, only 100
    # seconds remain, and a return with 100 seconds left means nothing.
    traces.append(single_block_trace(
        "A-NEAR-NO-TIME-LEFT",
        "NEAR_MISS",
        [observation(10, "FOCUS_WORK")] + _drift_burst(500, "FOCUS_WORK", ["COMMUNICATION"] * 4, step=20),
        False,
        f"remaining time is below DRIFT_MIN_REMAINING_SECONDS={DRIFT_MIN_REMAINING_SECONDS}",
        planned_duration_seconds=660,
    ))

    traces.append(single_block_trace(
        "A-NEAR-LOW-CONFIDENCE",
        "NEAR_MISS",
        [observation(10, "FOCUS_WORK")] + [
            observation(entry["t"], entry["category"], "ambiguous", "low")
            for entry in _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 4)
        ],
        False,
        "low-confidence rows are not confident evidence, so nothing switched",
    ))

    traces.append(single_block_trace(
        "A-NEAR-UNLOGGED-DEPARTURES",
        "NEAR_MISS",
        [observation(10, "FOCUS_WORK")] + _drift_burst(400, "FOCUS_WORK", ["UNLOGGED"] * 4),
        False,
        f"{', '.join(NON_EVIDENCE_CATEGORIES)} are excluded from evidence by is_confident_evidence",
    ))

    # A slow rotation through five categories. An anchor exists — the gate
    # still picks whichever category holds the most confident seconds — but no
    # 600s window holds four departures from it, so nothing fires.
    traces.append(single_block_trace(
        "A-NEAR-SLOW-ROTATION",
        "NEAR_MISS",
        [observation(t, category) for t, category in [
            (10, "FOCUS_WORK"), (400, "COMMUNICATION"), (790, "REFERENCE"),
            (1180, "SOCIAL_FEED"), (1570, "PASSIVE_CONSUMPTION"),
        ]],
        False,
        "an anchor exists, but switching this slowly never reaches "
        f"DRIFT_MIN_SWITCHES={DRIFT_MIN_SWITCHES} inside one window",
    ))

    # Nothing here is confident evidence, so `dominant_category` returns None
    # and the gate abstains before it ever counts a switch. This is the one
    # trace where the abstention is genuinely "I have no anchor".
    traces.append(single_block_trace(
        "A-NEAR-NO-EVIDENCE",
        "NEAR_MISS",
        [observation(t, category) for t, category in [
            (10, "UNLOGGED"), (400, "SYSTEM"), (450, "UNLOGGED"), (500, "SYSTEM"),
            (550, "UNLOGGED"), (600, "SYSTEM"), (650, "UNLOGGED"), (700, "SYSTEM"),
        ]],
        False,
        "no confident evidence at all; dominant_category finds no anchor",
    ))

    # ---- the trap, demonstrated rather than described ----------------------
    burst = [observation(10, "FOCUS_WORK")] + _drift_burst(400, "FOCUS_WORK", ["COMMUNICATION"] * 4)
    traces.append(single_block_trace(
        "A-TRAP-BACKDATED",
        "TRAP",
        list(reversed(burst)),
        False,
        "IDENTICAL pattern to A-PLANT-4SWITCH-FOCUS, emitted backwards. The "
        "monotonic floor pulls every observation to the first instant, every "
        "dwell computes to zero, no anchor is found, and the gate abstains — "
        "for a reason that has nothing to do with the pattern. A harness that "
        "generated traces this way would report a silent, meaningless pass.",
        allow_non_monotonic=True,
    ))

    return traces


# ---------------------------------------------------------------------------
# Suite B — null. Structureless by construction.
# ---------------------------------------------------------------------------

def _weighted_choice(rng: random.Random, weights: dict[str, float]) -> str:
    total = sum(weights.values())
    target = rng.random() * total
    cumulative = 0.0
    for key, weight in weights.items():
        cumulative += weight
        if target <= cumulative:
            return key
    return next(reversed(list(weights)))


def _classification(rng: random.Random) -> tuple[str, str]:
    target = rng.random()
    cumulative = 0.0
    for pair, probability in CLASSIFICATION_MIX:
        cumulative += probability
        if target <= cumulative:
            return pair
    return CLASSIFICATION_MIX[-1][0]


def _null_block(rng: random.Random, dwell_scale: float, duration: int) -> dict:
    """One block of pure noise.

    Nothing here reads the clock, the day, the position in the block, or any
    earlier draw. Category is i.i.d. from a fixed distribution and dwell is
    i.i.d. log-normal given that category. A repeated draw is not a
    "self-transition": it means the same activity continued, and the manager
    de-duplicates it, which is the honest representation of a longer dwell.
    """
    observations: list[dict] = []
    t = rng.randint(1, 30)
    while t < duration:
        category = _weighted_choice(rng, CATEGORY_WEIGHTS)
        status, confidence = _classification(rng)
        observations.append(observation(t, category, status, confidence))
        median = DWELL_MEDIAN_SECONDS[category] * dwell_scale
        dwell = median * math.exp(rng.gauss(0.0, DWELL_SIGMA))
        t += max(1, int(round(dwell)))
    return block(observations, planned_duration_seconds=duration)


def null_trace(
    index: int,
    seed: int,
    dwell_scale: float,
    family: str,
    blocks_per_trace: int,
    block_seconds: int,
) -> dict:
    """One simulated user: several declared blocks, all of them noise.

    The blocks are replayed into ONE database, in order, exactly as a real
    user's would be. That matters: `backoff_state` and the demotion policy both
    read every earlier block, so a multi-block trace exercises the parts of the
    gate a single-block trace cannot reach. Traces remain independent of each
    other — each one gets its own database.
    """
    rng = random.Random(seed)
    blocks = [_null_block(rng, dwell_scale, block_seconds) for _ in range(blocks_per_trace)]

    expect_offer = family == "NULL_COMPRESSED"
    if expect_offer:
        reason = (
            "same structureless generator with dwell scaled down; the gate is a "
            "threshold on switching RATE, so compressed noise can cross it. "
            "Scored in aggregate, not per trace."
        )
    else:
        reason = "no structure exists to detect"

    return trace(
        f"{family}-{index:04d}",
        family,
        blocks,
        expect_offer,
        reason,
        seed=seed,
    )


# ---------------------------------------------------------------------------
# Writing
# ---------------------------------------------------------------------------

def header_record(suite: str, description: str, acceptance: str, traces: list[dict],
                  extra: dict | None = None) -> dict:
    record = {
        "kind": "header",
        "schema": TRACE_SCHEMA,
        "synthetic": True,
        "label": "SYNTHETIC — generated with known ground truth. No real user "
                 "has ever touched this data. Any figure derived from it must "
                 "carry the word SYNTHETIC.",
        "suite": suite,
        "description": description,
        "acceptance": acceptance,
        "generator": "scripts/generate_traces.py",
        "generator_version": GENERATOR_VERSION,
        "traces": len(traces),
        "gate_constants": {
            "DRIFT_WINDOW_SECONDS": DRIFT_WINDOW_SECONDS,
            "DRIFT_MIN_SWITCHES": DRIFT_MIN_SWITCHES,
            "DRIFT_MIN_ELAPSED_SECONDS": DRIFT_MIN_ELAPSED_SECONDS,
            "DRIFT_MIN_REMAINING_SECONDS": DRIFT_MIN_REMAINING_SECONDS,
        },
        "injection_method": (
            "fast-forward through the real ingestion path: "
            "WorkBlockManager::observe_safe_category on a fresh in-memory "
            "database per trace, monotone timestamps after block start, "
            "delivered as fast as the process allows. Nothing is injected "
            "below the stamping layer and nothing is stubbed."
        ),
    }
    if extra:
        record.update(extra)
    return record


def write_suite(path: Path, header: dict, traces: list[dict]) -> str:
    lines = [json.dumps(header, sort_keys=True)]
    lines += [json.dumps(entry, sort_keys=True) for entry in traces]
    body = "\n".join(lines) + "\n"
    path.write_text(body)
    return hashlib.sha256(body.encode()).hexdigest()


ASSUMPTIONS = """\
# ASSUMPTIONS — SYNTHETIC trace generator

Everything `generate_traces.py` produces rests on the values below. They are
recorded here because an engine validated only against assumed dynamics has
been validated against its author's beliefs. Revisit every one of them the
moment real cohort data exists.

## Measured, and still not evidence about users

Read from `~/.velvt/velvt-service.sqlite3` on 2026-08-21: 23,026 rows in
`raw_event_buffer` across 8 local days, one Mac, one person.

- Category marginals (event counts, not dwell-weighted): FOCUS_WORK 0.534,
  REFERENCE 0.241, COMMUNICATION 0.099, PASSIVE_CONSUMPTION 0.079,
  UNLOGGED 0.031, SOCIAL_FEED 0.010, SYSTEM 0.006, TASK_MANAGEMENT 0.0003.
- Classification mix, drawn as a joint distribution because status and
  confidence are not independent in the shipped classifier:
  (classified, high) 0.506, (classified, medium) 0.277, (ambiguous, low)
  0.206, (unclassified, none) 0.011.

n = 1. This is a plausible shape, not a prior.

## Assumed, with no data behind them

- Dwell is log-normal with sigma 0.9 and per-category medians of 840s
  (FOCUS_WORK), 420s (PASSIVE_CONSUMPTION), 300s (REFERENCE), 180s
  (COMMUNICATION), 150s (SOCIAL_FEED), 120s (TASK_MANAGEMENT, UNLOGGED),
  60s (SYSTEM). The fixtures README states the ranges; nothing measures them.
- One 3600-second block per trace, within the shipped bounds of 300-10800.
- Event-count marginals are reused as draw probabilities alongside separate
  dwell medians. Those two quantities are not independent in reality, so the
  generator over-represents short-dwell categories relative to their true
  share of wall time.

## The assumption the null result actually rests on

Suite B's zero-offer result is driven by the DWELL distribution, not by
anything clever in the gate. Crossing `DRIFT_MIN_SWITCHES = 4` inside
`DRIFT_WINDOW_SECONDS = 600` requires roughly eight observations in ten
minutes — a mean dwell near 75 seconds. The assumed medians are 120-840
seconds, so noise cannot reach the threshold.

That is why the `null-compressed` arm exists. Same generator, same absence of
structure, dwell scaled down: the gate fires. The pair of results says
something precise and falsifiable — **the shipped gate is a threshold on
switching rate, and whether real users cross it is an empirical question the
cohort answers, not one these fixtures can.**

## What these fixtures cannot tell you

- Whether real people behave like this. They do not, in ways nobody can predict.
- Whether an intervention changes what a person does.
- Whether the drift-gate constants are right.
- Whether anyone wants this.
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path,
                        default=Path(__file__).resolve().parent / "traces")
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--null-traces", type=int, default=100)
    parser.add_argument("--compressed-traces", type=int, default=30,
                        help="traces in the inversion-control arm; fewer are "
                             "needed because it only has to be non-zero")
    parser.add_argument("--blocks-per-trace", type=int, default=5,
                        help="declared blocks per null trace, replayed in order "
                             "into one database")
    parser.add_argument("--block-seconds", type=int, default=NULL_BLOCK_SECONDS,
                        help="planned duration of each null block; the shipped "
                             "bounds are 300-10800")
    parser.add_argument("--compressed-dwell-scale", type=float, default=0.05,
                        help="dwell multiplier for the null-compressed arm")
    parser.add_argument("--check", action="store_true",
                        help="regenerate into a temporary directory and fail if "
                             "anything differs from what is on disk")
    args = parser.parse_args()

    if args.null_traces < 1:
        print("ERROR: --null-traces must be at least 1", file=sys.stderr)
        return 1
    if not 0 < args.compressed_dwell_scale <= 1:
        print("ERROR: --compressed-dwell-scale must be in (0, 1]", file=sys.stderr)
        return 1
    if args.blocks_per_trace < 1:
        print("ERROR: --blocks-per-trace must be at least 1", file=sys.stderr)
        return 1
    if not 300 <= args.block_seconds <= 10800:
        print("ERROR: --block-seconds must be within the shipped bounds "
              "of 300-10800", file=sys.stderr)
        return 1

    target = args.out
    if args.check:
        import tempfile
        target = Path(tempfile.mkdtemp()) / "traces"
    target.mkdir(parents=True, exist_ok=True)

    recovery = suite_a()
    null_traces = [
        null_trace(index, args.seed + index, 1.0, "NULL",
                   args.blocks_per_trace, args.block_seconds)
        for index in range(args.null_traces)
    ]
    # The inversion arm is deliberately shorter and shallower: it only has to
    # be non-zero, and at this dwell scale each block carries twenty times the
    # observations, so a matching size would be megabytes of fixture for no
    # additional claim.
    compressed = [
        null_trace(index, args.seed + 500_000 + index, args.compressed_dwell_scale,
                   "NULL_COMPRESSED", 2, args.block_seconds)
        for index in range(args.compressed_traces)
    ]

    files = {}
    files["SYNTHETIC-suite-a-recovery.jsonl"] = (
        header_record(
            "A — recovery",
            "Hand-authored patterns with known ground truth: planted drift the "
            "shipped gate must detect, and near-misses one step outside each of "
            "its four thresholds where it must abstain.",
            "every trace's observed offer matches its expect_offer field",
            recovery,
            {"expect_offers": sum(1 for t in recovery if t["expect_offer"])},
        ),
        recovery,
    )
    files["SYNTHETIC-suite-b-null.jsonl"] = (
        header_record(
            "B — null",
            "Pure noise. Category i.i.d. from a fixed distribution, dwell i.i.d. "
            "log-normal, no dependence on time of day, day of week, prior state, "
            "or any outcome.",
            "ZERO offers across every trace. Any offer is a false discovery.",
            null_traces,
            {
                "expect_offers": 0,
                "dwell_scale": 1.0,
                "blocks_per_trace": args.blocks_per_trace,
                "block_seconds": args.block_seconds,
            },
        ),
        null_traces,
    )
    files["SYNTHETIC-suite-b-null-compressed.jsonl"] = (
        header_record(
            "B — null, compressed dwell (inversion control)",
            "The same structureless generator with dwell scaled down. It exists "
            "so that the zero on the arm above is falsifiable: a harness that "
            "never reached the gate would also report zero.",
            "at least one offer, in aggregate. Not scored per trace.",
            compressed,
            {"expect_offers_at_least": 1, "dwell_scale": args.compressed_dwell_scale},
        ),
        compressed,
    )

    digests = {}
    for name, (header, entries) in files.items():
        digests[name] = {
            "sha256": write_suite(target / name, header, entries),
            "traces": len(entries),
            "acceptance": header["acceptance"],
        }

    (target / "ASSUMPTIONS.md").write_text(ASSUMPTIONS)

    manifest = {
        "label": "SYNTHETIC — generated fixtures, no real user data",
        "generator": "scripts/generate_traces.py",
        "generator_version": GENERATOR_VERSION,
        "schema": TRACE_SCHEMA,
        "seed": args.seed,
        "null_traces": args.null_traces,
        "blocks_per_trace": args.blocks_per_trace,
        "block_seconds": args.block_seconds,
        "compressed_traces": args.compressed_traces,
        "compressed_blocks_per_trace": 2,
        "compressed_dwell_scale": args.compressed_dwell_scale,
        "gate_constants": {
            "DRIFT_WINDOW_SECONDS": DRIFT_WINDOW_SECONDS,
            "DRIFT_MIN_SWITCHES": DRIFT_MIN_SWITCHES,
            "DRIFT_MIN_ELAPSED_SECONDS": DRIFT_MIN_ELAPSED_SECONDS,
            "DRIFT_MIN_REMAINING_SECONDS": DRIFT_MIN_REMAINING_SECONDS,
        },
        "replayed_by": "rust-service/tests/trace_replay.rs",
        "files": digests,
    }
    manifest_body = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    (target / "SYNTHETIC-manifest.json").write_text(manifest_body)

    if args.check:
        problems = []
        for name in list(files) + ["ASSUMPTIONS.md", "SYNTHETIC-manifest.json"]:
            committed = args.out / name
            if not committed.exists():
                problems.append(f"{name}: missing from {args.out}")
                continue
            if committed.read_text() != (target / name).read_text():
                problems.append(f"{name}: differs from a fresh generation")
        if problems:
            print("Generated fixtures are STALE:", file=sys.stderr)
            for problem in problems:
                print(f"  {problem}", file=sys.stderr)
            print(f"\nRe-run: ./scripts/generate_traces.py --out {args.out}",
                  file=sys.stderr)
            return 1
        print(f"fixtures in {args.out} match a fresh generation")
        return 0

    print(f"wrote {len(files) + 2} file(s) to {target}")
    for name, digest in digests.items():
        print(f"  {name:44} {digest['traces']:4} traces  {digest['sha256'][:16]}")
    print(f"  {'ASSUMPTIONS.md':44}")
    print(f"  {'SYNTHETIC-manifest.json':44}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
