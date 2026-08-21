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
# Suite C — segmentation. Run-level, for `behavior/bocpd.rs` and
# `behavior/hmm.rs` rather than for the shipped drift gate.
#
# READ THIS BEFORE QUOTING A NUMBER FROM SUITE C.
#
# Suites A and B are replayed through `WorkBlockManager::observe_safe_category`
# on a real database. Suite C is not. It is a sequence of CLOSED RUNS —
# `(t, category, dwell)` triples, which is exactly the row the feature contract
# in `behavior/features.rs` defines — handed straight to two models. Nothing is
# ingested, nothing is stamped, no gate is consulted, and no intervention can
# be produced.
#
# So: a result from suite C is a claim about the MODEL. It is not a claim about
# the PRODUCT, and it carries none of suite A and B's evidence that the
# ingestion path behaves. Both facts belong beside every figure derived here.
#
# Emitting runs rather than raw observations is also what makes the suite
# small enough to commit: the same 12 weeks as an observation stream would be
# tens of megabytes of fixture to answer a question about a segmenter that
# never sees an observation.
# ---------------------------------------------------------------------------

SEGMENTATION_SCHEMA = "velvt-runs/1"

# `03-BEHAVIORAL-ENGINE-VALIDATION.md` § 2 sweeps a planted completion
# probability of 0.70 down to {0.40, 0.55, 0.65, 0.70}. Transposed onto the
# quantity a segmenter can observe, the baseline rate of alternating first
# blocks is 0.30 and the antecedent raises it to one of these. The last entry
# is a NULL trace in disguise and belongs in the sweep as a control.
PLANTED_BASELINE = 0.30
PLANTED_EFFECT_SIZES = (0.70, 0.55, 0.45, 0.30)

# Versioned independently of GENERATOR_VERSION so that adding to this suite
# never changes a byte of the suites `trace_replay.rs` pins by digest.
SEGMENTATION_GENERATOR_VERSION = 1

# Trace origin is 00:00 local on a Monday, so local hour is `(t // 3600) % 24`
# and day index is `t // 86400`. Stated because the HMM's time-of-day
# diagnostic derives the hour that way and a different origin would silently
# rotate every hour bin.
SECONDS_PER_DAY = 86400
SECONDS_PER_WEEK = 7 * SECONDS_PER_DAY

# ASSUMPTION, and the most consequential one in the file. Two routines, named
# for what they look like and nothing more. They are NOT the HMM's states —
# the HMM has three, they are unvalidated, and `03-BEHAVIORAL-ENGINE-SPEC.md`
# § 2.2 forbids naming them anywhere a user could see. These are generator
# parameters with descriptive keys.
ROUTINE_SUSTAINED = {
    "weights": {
        "FOCUS_WORK": 0.68,
        "REFERENCE": 0.18,
        "COMMUNICATION": 0.06,
        "PASSIVE_CONSUMPTION": 0.03,
        "TASK_MANAGEMENT": 0.02,
        "SOCIAL_FEED": 0.01,
        "SYSTEM": 0.01,
        "UNLOGGED": 0.01,
    },
    "dwell_scale": 1.0,
}
ROUTINE_ALTERNATING = {
    "weights": {
        "FOCUS_WORK": 0.24,
        "COMMUNICATION": 0.26,
        "SOCIAL_FEED": 0.18,
        "PASSIVE_CONSUMPTION": 0.14,
        "REFERENCE": 0.12,
        "TASK_MANAGEMENT": 0.03,
        "SYSTEM": 0.02,
        "UNLOGGED": 0.01,
    },
    "dwell_scale": 0.30,
}

# ASSUMPTION. Blocks per weekday: 0-4, mode 2, per the fixtures README.
BLOCKS_PER_WEEKDAY = ((0, 0.10), (1, 0.22), (2, 0.36), (3, 0.22), (4, 0.10))

# Six start slots two hours apart, so blocks never overlap without needing a
# repair pass, and so the time-of-day diagnostic has six distinct bins to work
# with. Within the shipped block bounds of 300-10800 seconds.
BLOCK_START_HOURS = (8, 10, 12, 14, 16, 18)
BLOCK_DURATIONS = (1800, 2700, 3600, 5400)

# ASSUMPTION, and deliberately a CONFOUND. Dwell runs longer mid-morning and
# shorter late in the day, independently of routine. It is here so that
# `03-BEHAVIORAL-ENGINE-SPEC.md` § 8 failure mode 1 — "the states are the
# clock, not behaviour" — is a test with something to fail on. A generator with
# no time-of-day structure at all would make that diagnostic unfalsifiable.
# NEVER applied to the NULL family.
def _hour_dwell_scale(hour: int) -> float:
    return 1.0 + 0.30 * math.cos(2.0 * math.pi * (hour - 10) / 24.0)


def _minimum_dwell() -> int:
    return 5


def _emit_runs(rng: random.Random, routine: dict, start: int, duration: int,
               block_index: int, hour_effect: bool) -> list[list[int]]:
    """One block's worth of closed runs.

    The final run is truncated at the block deadline rather than allowed to
    overrun it. That is what `finish` does to the open observation, so it is
    the honest shape — and it is the tail trap from `fixtures/README.md`
    reproduced deliberately rather than by accident.
    """
    runs: list[list[int]] = []
    t = start
    end = start + duration
    while t < end:
        category = _weighted_choice(rng, routine["weights"])
        median = DWELL_MEDIAN_SECONDS[category] * routine["dwell_scale"]
        if hour_effect:
            median *= _hour_dwell_scale((t // 3600) % 24)
        dwell = int(round(median * math.exp(rng.gauss(0.0, DWELL_SIGMA))))
        dwell = max(_minimum_dwell(), min(dwell, end - t))
        if dwell < _minimum_dwell():
            break
        runs.append([t, TAXONOMY.index(category), dwell, block_index])
        t += dwell
    return runs


def _weekday_schedule(rng: random.Random, day_origin: int,
                      blocks_per_weekday=BLOCKS_PER_WEEKDAY) -> list[tuple[int, int]]:
    """`(start, duration)` for each block on one weekday. Never overlapping."""
    target = rng.random()
    cumulative = 0.0
    count = 0
    for value, weight in blocks_per_weekday:
        cumulative += weight
        if target <= cumulative:
            count = value
            break
    if count == 0:
        return []
    hours = sorted(rng.sample(BLOCK_START_HOURS, count))
    return [
        (day_origin + hour * 3600 + rng.randrange(0, 600), rng.choice(BLOCK_DURATIONS))
        for hour in hours
    ]


def _segmentation_trace(
    trace_id: str,
    family: str,
    seed: int,
    weeks: int,
    routine_for_block,
    ground_truth: dict,
    hour_effect: bool = True,
    antecedent_probability: float = 0.0,
    blocks_per_weekday=BLOCKS_PER_WEEKDAY,
) -> dict:
    """One simulated user's run history.

    `routine_for_block(day_index, block_ordinal, antecedent, rng)` returns the
    routine dict for that block. Every family differs only in that callback and
    in whether the hour confound is applied, so the families cannot drift apart
    in any respect that is not the thing under test.
    """
    rng = random.Random(seed)
    runs: list[list[int]] = []
    block_index = 0
    blocks: list[dict] = []
    antecedent_days: list[int] = []

    for day in range(weeks * 7):
        if day % 7 >= 5:  # weekends carry no declared blocks in this generator
            continue
        day_origin = day * SECONDS_PER_DAY
        schedule = _weekday_schedule(rng, day_origin, blocks_per_weekday)
        if not schedule:
            continue
        antecedent = rng.random() < antecedent_probability
        if antecedent:
            antecedent_days.append(day)
            # The antecedent itself, as an OUT-OF-BLOCK run in the 30 minutes
            # before the day's first block. Emitted rather than only recorded
            # in ground truth, so an antecedent miner has something to mine.
            first_start = schedule[0][0]
            offset = rng.randrange(120, 1740)
            runs.append([first_start - offset, TAXONOMY.index("COMMUNICATION"),
                         min(offset, 300), -1])
        for ordinal, (start, duration) in enumerate(schedule):
            routine = routine_for_block(day, ordinal, antecedent, rng)
            emitted = _emit_runs(rng, routine, start, duration, block_index,
                                 hour_effect)
            if not emitted:
                continue
            if routine is ROUTINE_ALTERNATING:
                label = "alternating"
            elif routine is ROUTINE_SUSTAINED:
                label = "sustained"
            else:
                label = "unstructured"
            blocks.append({
                "index": block_index,
                "start": start,
                "duration": duration,
                "runs": len(emitted),
                "routine": label,
            })
            runs.extend(emitted)
            block_index += 1

    runs.sort(key=lambda entry: entry[0])
    truth = dict(ground_truth)
    truth["antecedent_days"] = antecedent_days
    truth["blocks"] = blocks
    return {
        "kind": "trace",
        "schema": SEGMENTATION_SCHEMA,
        "synthetic": True,
        "trace_id": trace_id,
        "family": family,
        "seed": seed,
        "weeks": weeks,
        "run_encoding": "[t_seconds, category_index, dwell_seconds, block_index]; "
                        "block_index -1 is an out-of-block run; local hour is "
                        "(t // 3600) % 24 with the origin at 00:00 Monday",
        "runs": runs,
        "ground_truth": truth,
    }


def planted_trace(index: int, seed: int, weeks: int,
                  alternating_given_antecedent: float,
                  alternating_given_none: float) -> dict:
    """`PLANTED` — a real antecedent at a known effect size.

    `03-BEHAVIORAL-ENGINE-VALIDATION.md` § 2 plants a drop in completion
    probability. Completion is not a quantity a segmenter observes, so the same
    antecedent is planted in the quantity a segmenter DOES observe: the day's
    first block runs the alternating routine more often after the antecedent.
    The effect sizes are the validation document's, transposed, and the
    transposition is stated because it is a substantive choice and not a
    formatting one.

    Block start hours are drawn independently of the routine, so any dependence
    the HMM finds between state and hour is an artefact and can be read as one.
    """
    def routine_for_block(day, ordinal, antecedent, rng):
        if ordinal != 0:
            return ROUTINE_ALTERNATING if rng.random() < 0.25 else ROUTINE_SUSTAINED
        probability = (alternating_given_antecedent if antecedent
                       else alternating_given_none)
        return ROUTINE_ALTERNATING if rng.random() < probability else ROUTINE_SUSTAINED

    return _segmentation_trace(
        f"PLANTED-{index:04d}",
        "PLANTED",
        seed,
        weeks,
        routine_for_block,
        {
            "planted": True,
            "alternating_given_antecedent": alternating_given_antecedent,
            "alternating_given_none": alternating_given_none,
            "antecedent": "a COMMUNICATION run in the 30 minutes before the "
                          "day's first declared block",
            "hour_confound": "dwell scaled by 1 + 0.30 cos(2 pi (h - 10) / 24), "
                             "independent of routine",
        },
        antecedent_probability=0.45,
    )


def null_runs_trace(index: int, seed: int, weeks: int) -> dict:
    """`NULL` — no exploitable structure of any kind.

    Category i.i.d. from the fixed marginals, dwell i.i.d. log-normal given
    category, blocks placed independently of everything. No routine, no
    antecedent, and — unlike every other family here — NO hour confound, so
    that a time-of-day effect found in this family is a defect in the generator
    rather than a planted one.
    """
    null_routine = {"weights": CATEGORY_WEIGHTS, "dwell_scale": 1.0}

    def routine_for_block(day, ordinal, antecedent, rng):
        return null_routine

    return _segmentation_trace(
        f"NULL-{index:04d}",
        "NULL",
        seed,
        weeks,
        routine_for_block,
        {
            "planted": False,
            "structure": "none: category i.i.d., dwell i.i.d. given category, "
                         "no time-of-day effect, no day-of-week effect, no "
                         "dependence on any prior run or block",
        },
        hour_effect=False,
    )


def regime_trace(index: int, seed: int, weeks: int, change_week: int) -> dict:
    """`REGIME` — one abrupt, permanent routine change on a known day.

    The change is BETWEEN blocks, at a day boundary, which is what "term to
    exam period" means. That matters for how it can be detected: a BOCPD that
    is reset per work block, as `03` § 2.1 specifies, cannot see a change that
    never occurs inside a block. The validation suite therefore scores it in
    both the per-block mode and a continuous mode and reports both.
    """
    change_at = change_week * SECONDS_PER_WEEK

    def routine_for_block(day, ordinal, antecedent, rng):
        return (ROUTINE_ALTERNATING if day * SECONDS_PER_DAY >= change_at
                else ROUTINE_SUSTAINED)

    trace_record = _segmentation_trace(
        f"REGIME-{index:04d}",
        "REGIME",
        seed,
        weeks,
        routine_for_block,
        {
            "planted": True,
            "change_at_seconds": change_at,
            "change_week": change_week,
            "before": "sustained",
            "after": "alternating",
        },
    )
    first_after = next(
        (position for position, run in enumerate(trace_record["runs"])
         if run[0] >= change_at),
        len(trace_record["runs"]),
    )
    trace_record["ground_truth"]["change_run_index"] = first_after
    return trace_record


def drifting_trace(index: int, seed: int, weeks: int,
                   alternating_given_antecedent: float) -> dict:
    """`DRIFTING` — a planted effect that exists in the first half and not the
    second. The engine must detect it, then stop asserting it."""
    drift_at = (weeks // 2) * SECONDS_PER_WEEK

    def routine_for_block(day, ordinal, antecedent, rng):
        if ordinal != 0:
            return ROUTINE_ALTERNATING if rng.random() < 0.25 else ROUTINE_SUSTAINED
        before_drift = day * SECONDS_PER_DAY < drift_at
        probability = (alternating_given_antecedent
                       if (antecedent and before_drift) else 0.30)
        return ROUTINE_ALTERNATING if rng.random() < probability else ROUTINE_SUSTAINED

    record = _segmentation_trace(
        f"DRIFTING-{index:04d}",
        "DRIFTING",
        seed,
        weeks,
        routine_for_block,
        {
            "planted": True,
            "drift_at_seconds": drift_at,
            "alternating_given_antecedent_before": alternating_given_antecedent,
            "alternating_given_antecedent_after": 0.30,
            "alternating_given_none": 0.30,
        },
        antecedent_probability=0.45,
    )
    record["ground_truth"]["drift_run_index"] = next(
        (position for position, run in enumerate(record["runs"])
         if run[0] >= drift_at),
        len(record["runs"]),
    )
    return record


def sparse_trace(index: int, seed: int, weeks: int) -> dict:
    """`SPARSE` — two weeks of light usage, below every support threshold.

    The expected result is ABSTENTION, and abstention with a stated reason.
    `hmm::MIN_RUNS_TO_FIT` is 200 and this family is built to sit well under
    it, so a fit that succeeds here is a threshold that is not wired in.
    """
    def routine_for_block(day, ordinal, antecedent, rng):
        return ROUTINE_SUSTAINED

    return _segmentation_trace(
        f"SPARSE-{index:04d}",
        "SPARSE",
        seed,
        weeks,
        routine_for_block,
        {
            "planted": False,
            "expectation": "abstain, with a reason distinguishable from a failure",
        },
        blocks_per_weekday=((0, 0.50), (1, 0.35), (2, 0.15)),
    )


# ---------------------------------------------------------------------------
# Suite D — antecedent mining. EPISODE-level, for `behavior/antecedents.rs`
# and `behavior/candidates.rs`.
#
# READ THIS BEFORE QUOTING A NUMBER FROM SUITE D.
#
# Suite C hands closed RUNS to two segmentation models. Suite D hands closed
# EPISODES to the antecedent miner, and an episode is one level further from
# the ingestion path than a run is: in the real pipeline an episode onset is
# produced by the nightly segmenter, whose own recovery on suite C is
# `MI(state; routine) = 0.4991 bits` against a ceiling of 1.585 — good, and
# nowhere near perfect.
#
# So suite D measures THE MINER GIVEN CORRECT EPISODES. It deliberately does
# not measure the composition of segmenter and miner, because a single number
# for the composition would hide which of the two layers lost the signal. The
# composition is measured separately and reported separately.
#
# Nothing here is replayed through the ingestion path, nothing is stamped, no
# gate is consulted, and no intervention can be produced. A result from suite D
# is a claim about a STATISTICAL PROCEDURE. It is not a claim about the
# product, and it is not evidence about people.
#
# The outcome `y` is `1` when no confident anchor observation follows the onset
# within 600 seconds. It is a BEHAVIOURAL PROXY, not a productivity label, and
# every figure derived from it inherits that.
# ---------------------------------------------------------------------------

ANTECEDENT_SCHEMA = "velvt-episodes/1"

# Versioned independently of GENERATOR_VERSION and of
# SEGMENTATION_GENERATOR_VERSION, so adding to this suite never changes a byte
# of the suites that other harnesses pin.
ANTECEDENT_GENERATOR_VERSION = 1

# `03-BEHAVIORAL-ENGINE-VALIDATION.md` § 2's sweep, kept on its own scale this
# time: the outcome IS a probability of a bad episode, so the planted drop goes
# straight in. Baseline risk 0.70; with the antecedent present the risk becomes
# one of the four below. The last is a NULL trace in disguise and belongs in
# the sweep as a control: at RD = 0 the recovery cell measures the false
# discovery rate, not power.
ANTECEDENT_BASELINE_RISK = 0.70
ANTECEDENT_RISK_GIVEN_ANTECEDENT = (0.40, 0.55, 0.65, 0.70)

# History lengths for the recovery curve. This is the axis that answers "how
# long until Velvt knows something about me", and it is the one nobody wants to
# put a number on.
ANTECEDENT_HISTORY_WEEKS = (2, 4, 6, 8, 12)

# The planted antecedent. Episode-level rather than day-level, and it is the
# founder's "you open messaging before your first work block" transposed onto
# the unit the miner actually analyses: the episode that began immediately
# after a COMMUNICATION run.
ANTECEDENT_CATEGORY = "COMMUNICATION"
ANTECEDENT_PREVALENCE = 0.35

# ASSUMPTION, and the single most consequential one in this suite. A person's
# bad days are bad ALL DAY: the outcome carries a day-level random effect on
# top of whatever the antecedent does. This is why the permutation null is a
# CIRCULAR BLOCK SHIFT over whole days rather than an i.i.d. resample, and a
# generator without it would make that control unfalsifiable — an i.i.d. null
# would be correct, and switching the block structure off would change nothing.
ANTECEDENT_DAY_EFFECT_SD = 0.10

# ASSUMPTION. Day-level structure in the FEATURES too, for the same reason:
# a person's blocks cluster in the morning on some days and the afternoon on
# others, and Focus tends to be on for a whole day or off for a whole day. Both
# make several candidates day-clustered, which is the configuration in which an
# i.i.d. test manufactures significance out of nothing.
ANTECEDENT_SCHEDULE_MODES = ((8, 12), (12, 16), (16, 20))

# ASSUMPTION. Episodes per block: 0-3, mode 1. Weekends carry a block 20% of
# the time, so `daytype_weekend` has some support instead of abstaining in
# every trace and leaving that dimension untested.
ANTECEDENT_EPISODES_PER_BLOCK = ((0, 0.22), (1, 0.40), (2, 0.26), (3, 0.12))
ANTECEDENT_WEEKEND_BLOCK_PROBABILITY = 0.20

# From `0009_work_blocks.sql` and `0020_delivery_suppressed_dnd_outcome.sql`.
# Restated here in the SAME ORDER as `behavior/candidates.rs`, because the
# fixture encodes them as indices and a reordering would silently repoint every
# planted feature at a different level.
ANTECEDENT_BLOCK_PHASES = ("active", "paused", "completed", "abandoned", "expired")
ANTECEDENT_INTERVENTION_OUTCOMES = (
    "offered", "accepted_action", "returned", "not_helpful",
    "wrong_classification", "was_focused", "dismissed",
    "delivery_suppressed_dnd", "no_response",
)
# ASSUMPTION. A block ends in one of these three terminal phases; `active` and
# `paused` are not terminal and so are never a PRIOR block's phase.
ANTECEDENT_TERMINAL_PHASES = (2, 3, 4)
# ASSUMPTION. Most blocks carry no intervention at all, which is NOT the same
# as an intervention that produced no response. The miner sees `-1` for those
# and drops them from both arms.
ANTECEDENT_INTERVENTION_PROBABILITY = 0.18

# The category a preceding run is drawn from, when it is not the planted
# antecedent. Reuses the measured marginals with COMMUNICATION removed, so the
# planted prevalence is exactly ANTECEDENT_PREVALENCE and not that plus noise.
ANTECEDENT_OTHER_CATEGORY_WEIGHTS = {
    name: weight for name, weight in CATEGORY_WEIGHTS.items()
    if name != ANTECEDENT_CATEGORY
}


def _episode_features(rng: random.Random, day: int, weekend: bool,
                      schedule: tuple[int, int], focus_day: bool,
                      prior_phase: int,
                      prior_outcome: int) -> tuple[list[int], bool]:
    """One episode's features. Returns the encoded row minus the outcome, and
    whether the planted antecedent is present."""
    start_hour = rng.randrange(schedule[0], schedule[1])
    duration = rng.choice(BLOCK_DURATIONS)
    elapsed = rng.randrange(0, duration)
    hour = (start_hour + elapsed // 3600) % 24

    antecedent = rng.random() < ANTECEDENT_PREVALENCE
    if antecedent:
        preceding = TAXONOMY.index(ANTECEDENT_CATEGORY)
    else:
        preceding = TAXONOMY.index(
            _weighted_choice(rng, ANTECEDENT_OTHER_CATEGORY_WEIGHTS))

    # The run before the preceding run. Absent for the first two runs of a
    # block, and absent means UNOBSERVED to the miner, not "not COMMUNICATION".
    run_index = rng.randrange(0, 14)
    before_preceding = -1
    if run_index >= 2:
        before_preceding = TAXONOMY.index(_weighted_choice(rng, CATEGORY_WEIGHTS))
    gap = -1
    if run_index >= 1:
        gap = int(round(math.exp(rng.uniform(math.log(5), math.log(1800)))))

    onset = day * SECONDS_PER_DAY + start_hour * 3600 + elapsed
    return ([
        day,
        onset,
        hour,
        1 if weekend else 0,
        preceding,
        before_preceding,
        elapsed,
        1 if focus_day else 0,
        prior_phase,
        prior_outcome,
        run_index,
        gap,
    ], antecedent)


def _antecedent_trace(trace_id: str, family: str, seed: int, weeks: int,
                      risk_given_antecedent: float,
                      baseline_risk: float,
                      ground_truth: dict,
                      blocks_per_weekday=BLOCKS_PER_WEEKDAY,
                      episodes_per_block=ANTECEDENT_EPISODES_PER_BLOCK) -> dict:
    """One simulated user's episode history.

    Families differ ONLY in `risk_given_antecedent` and in how much history
    they carry, so a difference between two families cannot come from anything
    else.
    """
    rng = random.Random(seed)
    episodes: list[list[int]] = []
    prior_phase = -1
    prior_outcome = -1
    present_episodes = 0
    present_events = 0
    absent_episodes = 0
    absent_events = 0

    for day in range(weeks * 7):
        weekend = day % 7 >= 5
        if weekend and rng.random() >= ANTECEDENT_WEEKEND_BLOCK_PROBABILITY:
            continue
        # Day-level structure: schedule mode, Focus, and the day's own risk
        # offset. All three are constant within the day, which is exactly the
        # autocorrelation the circular block shift has to preserve.
        schedule = rng.choice(ANTECEDENT_SCHEDULE_MODES)
        focus_day = rng.random() < 0.30
        day_effect = rng.gauss(0.0, ANTECEDENT_DAY_EFFECT_SD)

        if weekend:
            block_count = 1
        else:
            target = rng.random()
            cumulative = 0.0
            block_count = 0
            for value, weight in blocks_per_weekday:
                cumulative += weight
                if target <= cumulative:
                    block_count = value
                    break

        for _block in range(block_count):
            target = rng.random()
            cumulative = 0.0
            count = 0
            for value, weight in episodes_per_block:
                cumulative += weight
                if target <= cumulative:
                    count = value
                    break
            for _ in range(count):
                row, antecedent = _episode_features(
                    rng, day, weekend, schedule, focus_day,
                    prior_phase, prior_outcome)
                base = risk_given_antecedent if antecedent else baseline_risk
                risk = min(0.98, max(0.02, base + day_effect))
                outcome = 1 if rng.random() < risk else 0
                if antecedent:
                    present_episodes += 1
                    present_events += outcome
                else:
                    absent_episodes += 1
                    absent_events += outcome
                episodes.append(row + [outcome])
            # The block that just ended becomes the next block's prior.
            prior_phase = rng.choice(ANTECEDENT_TERMINAL_PHASES)
            prior_outcome = (
                rng.randrange(0, len(ANTECEDENT_INTERVENTION_OUTCOMES))
                if rng.random() < ANTECEDENT_INTERVENTION_PROBABILITY else -1)

    episodes.sort(key=lambda entry: (entry[0], entry[1]))
    truth = dict(ground_truth)
    truth["episodes"] = len(episodes)
    truth["days"] = len({entry[0] for entry in episodes})
    # The realised risk difference in this trace, as opposed to the one that
    # was planted. Reported so a recovery failure can be read against what the
    # sample actually contained rather than against the parameter.
    truth["realised_present_episodes"] = present_episodes
    truth["realised_absent_episodes"] = absent_episodes
    truth["realised_risk_difference"] = round(
        (present_events / present_episodes if present_episodes else 0.0)
        - (absent_events / absent_episodes if absent_episodes else 0.0), 6)
    return {
        "kind": "trace",
        "schema": ANTECEDENT_SCHEMA,
        "synthetic": True,
        "trace_id": trace_id,
        "family": family,
        "seed": seed,
        "weeks": weeks,
        "episode_encoding": (
            "[day_index, onset_seconds, local_hour, weekend, preceding_category, "
            "category_before_preceding, block_elapsed_seconds, focus_active, "
            "prior_block_phase, prior_intervention_outcome, run_index, "
            "gap_seconds, y]; -1 means UNOBSERVED, which is not the same as "
            "absent and must not be folded into the control arm; y = 1 means no "
            "confident anchor observation within 600s, a behavioural proxy and "
            "not a productivity label"
        ),
        "episodes": episodes,
        "ground_truth": truth,
    }


def _planted_ground_truth(risk_given_antecedent: float) -> dict:
    """The planted candidate, and the candidates that carry the SAME planted
    signal by construction.

    `prevtrans_X__COMMUNICATION` is not an independent false discovery: the
    transition's second element IS the preceding category, so those eight
    candidates are entailed by the plant. Counting them as false discoveries
    would understate the miner; counting them as recoveries would overstate it.
    They are counted as their own column.
    """
    entailed = [f"prevtrans_{name}__{ANTECEDENT_CATEGORY}" for name in TAXONOMY]
    return {
        "planted": risk_given_antecedent != ANTECEDENT_BASELINE_RISK,
        "planted_candidate_id": f"prevcat_{ANTECEDENT_CATEGORY}",
        "entailed_candidate_ids": entailed,
        "planted_risk_difference": round(
            risk_given_antecedent - ANTECEDENT_BASELINE_RISK, 6),
        "risk_given_antecedent": risk_given_antecedent,
        "baseline_risk": ANTECEDENT_BASELINE_RISK,
        "antecedent_prevalence": ANTECEDENT_PREVALENCE,
        "day_effect_sd": ANTECEDENT_DAY_EFFECT_SD,
    }


def antecedent_planted_trace(index: int, seed: int, weeks: int,
                             risk_given_antecedent: float) -> dict:
    """`PLANTED` — one cell of the recovery grid."""
    return _antecedent_trace(
        f"APLANTED-{index:05d}", "PLANTED", seed, weeks,
        risk_given_antecedent, ANTECEDENT_BASELINE_RISK,
        _planted_ground_truth(risk_given_antecedent))


def antecedent_null_trace(index: int, seed: int, weeks: int) -> dict:
    """`NULL` — no association between ANY registered candidate and the outcome.

    The features still carry day-level structure and the outcome still carries
    a day-level random effect. That is deliberate: a null with no
    autocorrelation would be passed by an i.i.d. test, and the permutation
    control would be unfalsifiable. Here the day effect makes several
    candidates look associated to a test that assumes independence.
    """
    return _antecedent_trace(
        f"ANULL-{index:05d}", "NULL", seed, weeks,
        ANTECEDENT_BASELINE_RISK, ANTECEDENT_BASELINE_RISK,
        {
            "planted": False,
            "planted_candidate_id": None,
            "entailed_candidate_ids": [],
            "planted_risk_difference": 0.0,
            "structure": (
                "outcome independent of every registered candidate. Day-level "
                "random effect on the outcome and day-level structure in the "
                "features are BOTH present, so an i.i.d. test has something to "
                "get wrong."
            ),
            "day_effect_sd": ANTECEDENT_DAY_EFFECT_SD,
        })


def antecedent_saturated_trace(index: int, seed: int, weeks: int) -> dict:
    """`SATURATED` — the inversion control for the whole pipeline.

    A huge effect over a long history. It must produce SOME surfaced finding,
    in aggregate. Without it, "zero findings on 100 null traces" is
    unfalsifiable: a harness that never reached the miner would also report
    zero, and so would a miner that can never confirm anything.
    """
    return _antecedent_trace(
        f"ASATURATED-{index:05d}", "SATURATED", seed, weeks, 0.20,
        ANTECEDENT_BASELINE_RISK,
        _planted_ground_truth(0.20),
        blocks_per_weekday=((1, 0.15), (2, 0.35), (3, 0.35), (4, 0.15)),
        episodes_per_block=((1, 0.35), (2, 0.40), (3, 0.25)))


def antecedent_sparse_trace(index: int, seed: int, weeks: int) -> dict:
    """`SPARSE` — light usage, below every support threshold.

    The expected result is ABSTENTION with a stated reason, distinguishable
    from "looked and found nothing".
    """
    return _antecedent_trace(
        f"ASPARSE-{index:05d}", "SPARSE", seed, weeks,
        ANTECEDENT_BASELINE_RISK, ANTECEDENT_BASELINE_RISK,
        {
            "planted": False,
            "planted_candidate_id": None,
            "entailed_candidate_ids": [],
            "planted_risk_difference": 0.0,
            "expectation": "abstain, with a reason distinguishable from a failure",
        },
        blocks_per_weekday=((0, 0.35), (1, 0.45), (2, 0.20)),
        episodes_per_block=((0, 0.25), (1, 0.55), (2, 0.20)))


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

## Suite C — the segmentation families

Suite C is run-level. It is NOT replayed through the ingestion path: it hands
`(t, category, dwell, block)` tuples — the closed-run row `behavior/features.rs`
defines — straight to `behavior/bocpd.rs` and `behavior/hmm.rs`. **A number
from suite C is a claim about the model, not about the product.** Suites A and
B carry the ingestion-path evidence; suite C carries none of it.

Additional assumptions, none of them measured:

- Two routines. `sustained`: FOCUS_WORK 0.68, REFERENCE 0.18, COMMUNICATION
  0.06, rest 0.08, dwell scale 1.0. `alternating`: COMMUNICATION 0.26,
  FOCUS_WORK 0.24, SOCIAL_FEED 0.18, PASSIVE_CONSUMPTION 0.14, REFERENCE 0.12,
  rest 0.06, dwell scale 0.30. Nothing measures either mixture, and the dwell
  scale of 0.30 is the single number that most affects how detectable a regime
  change is.
- Blocks on weekdays only, 0-4 per day with mode 2, starting at one of six
  slots two hours apart between 08:00 and 18:00, durations drawn from
  {1800, 2700, 3600, 5400} seconds.
- A deliberate time-of-day CONFOUND: dwell is scaled by
  `1 + 0.30 cos(2 pi (h - 10) / 24)`, independent of routine. It exists so that
  `03-BEHAVIORAL-ENGINE-SPEC.md` § 8 failure mode 1 — the HMM states are the
  clock rather than behaviour — is a test with something to fail on. It is
  **never** applied to the NULL family.
- The planted antecedent from `03-BEHAVIORAL-ENGINE-VALIDATION.md` § 2 is a
  drop in *completion probability*. Completion is not a quantity a segmenter
  observes, so the same antecedent is transposed onto one that it does: the
  day's first block runs the alternating routine at rate
  {0.70, 0.55, 0.45, 0.30} after the antecedent against a baseline of 0.30.
  The transposition is a substantive choice, not a formatting one.
- The final run of every block is truncated at the block deadline rather than
  allowed to overrun it, which reproduces the tail behaviour of `finish`
  closing the open observation.

## Suite D — the antecedent-mining families

Suite D is EPISODE-level, and that is one level further from the product than
suite C. In the real pipeline an episode onset comes out of the nightly
segmenter; here it is handed to the miner directly, with correct boundaries and
a correct outcome. **A suite D number is a claim about a statistical procedure
given correct episodes.** It is not a claim about the composition of segmenter
and miner, and it is not a claim about people.

Additional assumptions, none of them measured:

- Baseline risk `P(Y=1 | antecedent absent) = 0.70`, and with the antecedent
  present one of `{0.40, 0.55, 0.65, 0.70}`. Risk differences of `-0.30`,
  `-0.15`, `-0.05` and `0.00`. The last cell is a null in disguise and measures
  the false-discovery rate rather than power.
- The planted antecedent is `prevcat_COMMUNICATION` — the episode began
  immediately after a COMMUNICATION run — at a prevalence of 0.35. It is the
  founder's "you open messaging before your first work block" transposed onto
  the unit the miner analyses.
- **A day-level random effect on the outcome**, `N(0, 0.10)` on the probability
  scale, plus day-level structure in the features: the day's schedule mode and
  whether Focus was on are constant within a day. This is the single most
  consequential assumption in the suite. It exists so the circular block-shift
  permutation has something to be right about: with an i.i.d. null and i.i.d.
  data, switching the block structure off would change nothing and the control
  would be unfalsifiable.
- Episodes per block 0-3 with mode 1, on top of suite C's 0-4 blocks per
  weekday. Weekends carry one block 20% of the time, so `daytype_weekend` has
  support in some traces instead of abstaining in all of them.
- Blocks end in one of three terminal phases and carry an intervention 18% of
  the time. Most blocks have NO prior intervention outcome, encoded as `-1`,
  and `-1` means UNOBSERVED — the miner drops those episodes from both arms
  rather than counting them as "the intervention did not succeed".
- `y = 1` means no confident anchor observation within 600 seconds. It is a
  BEHAVIOURAL PROXY. Nothing in this file, and nothing derived from it, may be
  read as a claim that the time was unproductive.

## What these fixtures cannot tell you

- Whether real people behave like this. They do not, in ways nobody can predict.
- Whether an intervention changes what a person does.
- Whether the drift-gate constants are right.
- Whether anyone wants this.
- Whether either segmentation model would find anything in a real run history.
  Suite C shows what the models do on data whose truth was known in advance.
  That is how you demonstrate a model does not hallucinate. It is not evidence
  about people.
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
    parser.add_argument("--planted-seeds", type=int, default=8,
                        help="seeds per PLANTED effect size in suite C")
    parser.add_argument("--planted-weeks", type=int, default=8)
    parser.add_argument("--segmentation-null-traces", type=int, default=100,
                        help="NULL traces in suite C; this is the family the "
                             "false-alarm rate is measured on")
    parser.add_argument("--segmentation-null-weeks", type=int, default=4)
    parser.add_argument("--regime-traces", type=int, default=24)
    parser.add_argument("--regime-weeks", type=int, default=8)
    parser.add_argument("--regime-change-week", type=int, default=4)
    parser.add_argument("--drifting-traces", type=int, default=16)
    parser.add_argument("--drifting-weeks", type=int, default=12)
    parser.add_argument("--sparse-traces", type=int, default=16)
    parser.add_argument("--sparse-weeks", type=int, default=2)
    parser.add_argument("--antecedent-null-traces", type=int, default=100,
                        help="NULL traces in suite D; this is the family the "
                             "zero-findings acceptance is measured on")
    parser.add_argument("--antecedent-null-weeks", type=int, default=8)
    parser.add_argument("--antecedent-planted-seeds", type=int, default=16,
                        help="seeds per cell of the recovery grid; the grid is "
                             "4 effect sizes x 5 history lengths, so this is "
                             "the resolution of every fraction in the curve")
    parser.add_argument("--antecedent-saturated-traces", type=int, default=8,
                        help="traces in the inversion-control arm; it only has "
                             "to be non-zero")
    parser.add_argument("--antecedent-saturated-weeks", type=int, default=26)
    parser.add_argument("--antecedent-sparse-traces", type=int, default=16)
    parser.add_argument("--antecedent-sparse-weeks", type=int, default=2)
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
    if not 0 < args.regime_change_week < args.regime_weeks:
        print("ERROR: --regime-change-week must fall strictly inside "
              "--regime-weeks, or there is no change point to recover",
              file=sys.stderr)
        return 1
    if args.drifting_weeks < 2:
        print("ERROR: --drifting-weeks must be at least 2", file=sys.stderr)
        return 1
    if args.antecedent_planted_seeds < 1:
        print("ERROR: --antecedent-planted-seeds must be at least 1",
              file=sys.stderr)
        return 1
    if args.antecedent_saturated_weeks <= max(ANTECEDENT_HISTORY_WEEKS):
        print("ERROR: --antecedent-saturated-weeks must exceed the longest "
              "history in the recovery grid, or the inversion control is not "
              "a control", file=sys.stderr)
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

    # Suite C. Seed offsets are disjoint per family so that adding traces to
    # one family never renumbers another and invalidates a recorded result.
    segmentation: list[dict] = []
    for effect_index, given_antecedent in enumerate(PLANTED_EFFECT_SIZES):
        for index in range(args.planted_seeds):
            segmentation.append(planted_trace(
                effect_index * 1000 + index,
                args.seed + 1_000_000 + effect_index * 10_000 + index,
                args.planted_weeks,
                given_antecedent,
                PLANTED_BASELINE,
            ))
    segmentation += [
        null_runs_trace(index, args.seed + 2_000_000 + index,
                        args.segmentation_null_weeks)
        for index in range(args.segmentation_null_traces)
    ]
    segmentation += [
        regime_trace(index, args.seed + 3_000_000 + index, args.regime_weeks,
                     args.regime_change_week)
        for index in range(args.regime_traces)
    ]
    segmentation += [
        drifting_trace(index, args.seed + 4_000_000 + index,
                       args.drifting_weeks, PLANTED_EFFECT_SIZES[0])
        for index in range(args.drifting_traces)
    ]
    segmentation += [
        sparse_trace(index, args.seed + 5_000_000 + index, args.sparse_weeks)
        for index in range(args.sparse_traces)
    ]

    # Suite D. Seed offsets disjoint from every other suite for the same
    # reason: adding traces to one family must never renumber another and
    # invalidate a recorded result.
    antecedents: list[dict] = []
    for effect_index, risk in enumerate(ANTECEDENT_RISK_GIVEN_ANTECEDENT):
        for week_index, weeks in enumerate(ANTECEDENT_HISTORY_WEEKS):
            for index in range(args.antecedent_planted_seeds):
                antecedents.append(antecedent_planted_trace(
                    (effect_index * 100 + week_index) * 100 + index,
                    args.seed + 6_000_000 + effect_index * 100_000
                    + week_index * 10_000 + index,
                    weeks,
                    risk,
                ))
    antecedents += [
        antecedent_null_trace(index, args.seed + 7_000_000 + index,
                              args.antecedent_null_weeks)
        for index in range(args.antecedent_null_traces)
    ]
    antecedents += [
        antecedent_saturated_trace(index, args.seed + 8_000_000 + index,
                                   args.antecedent_saturated_weeks)
        for index in range(args.antecedent_saturated_traces)
    ]
    antecedents += [
        antecedent_sparse_trace(index, args.seed + 9_000_000 + index,
                                args.antecedent_sparse_weeks)
        for index in range(args.antecedent_sparse_traces)
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

    files["SYNTHETIC-suite-c-segmentation.jsonl"] = (
        header_record(
            "C — segmentation",
            "Run-level traces for behavior/bocpd.rs and behavior/hmm.rs. Five "
            "families with known ground truth: PLANTED, NULL, REGIME, "
            "DRIFTING, SPARSE.",
            "NULL: the change-point detector's false-alarm rate is reported at "
            "every threshold, not asserted to be zero. REGIME: the planted "
            "change point is recovered, with the lag reported in runs and in "
            "minutes. SPARSE: both models abstain, with a stated reason.",
            segmentation,
            {
                "schema": SEGMENTATION_SCHEMA,
                "segmentation_generator_version": SEGMENTATION_GENERATOR_VERSION,
                "injection_method": (
                    "NONE. Suite C is not replayed through the ingestion path. "
                    "It is a sequence of closed runs handed straight to two "
                    "models, so every result derived from it is a claim about "
                    "the MODEL and not about the PRODUCT. Suites A and B carry "
                    "the ingestion-path evidence; this suite carries none of it."
                ),
                "families": sorted({trace["family"] for trace in segmentation}),
                "planted_effect_sizes": list(PLANTED_EFFECT_SIZES),
                "planted_baseline": PLANTED_BASELINE,
                "regime_change_week": args.regime_change_week,
                "total_runs": sum(len(trace["runs"]) for trace in segmentation),
            },
        ),
        segmentation,
    )

    files["SYNTHETIC-suite-d-antecedents.jsonl"] = (
        header_record(
            "D - antecedent mining",
            "Episode-level traces for behavior/antecedents.rs and "
            "behavior/candidates.rs. Four families with known ground truth: "
            "PLANTED (a 4 x 5 recovery grid over effect size and history "
            "length), NULL, SATURATED, SPARSE.",
            "NULL: ZERO surfaced findings across every trace. Any surfaced "
            "finding is a false discovery and the acceptance fails. SATURATED: "
            "at least one surfaced finding in aggregate, so the NULL zero is "
            "falsifiable. PLANTED: the recovery fraction is REPORTED at every "
            "cell, not asserted. SPARSE: abstain, with a stated reason.",
            antecedents,
            {
                "schema": ANTECEDENT_SCHEMA,
                "antecedent_generator_version": ANTECEDENT_GENERATOR_VERSION,
                "injection_method": (
                    "NONE. Suite D is not replayed through the ingestion path "
                    "and its episodes are not produced by the nightly "
                    "segmenter. It is a sequence of closed EPISODES handed "
                    "straight to the miner, so every result derived from it is "
                    "a claim about a STATISTICAL PROCEDURE given correct "
                    "episodes -- not about the product, and not about the "
                    "segmenter that would have to produce those episodes in "
                    "reality."
                ),
                "families": sorted({trace["family"] for trace in antecedents}),
                "outcome": (
                    "y = 1 iff no confident anchor observation within 600s. A "
                    "BEHAVIOURAL PROXY, not a productivity label."
                ),
                "planted_candidate_id": f"prevcat_{ANTECEDENT_CATEGORY}",
                "baseline_risk": ANTECEDENT_BASELINE_RISK,
                "risk_given_antecedent": list(ANTECEDENT_RISK_GIVEN_ANTECEDENT),
                "history_weeks": list(ANTECEDENT_HISTORY_WEEKS),
                "antecedent_prevalence": ANTECEDENT_PREVALENCE,
                "day_effect_sd": ANTECEDENT_DAY_EFFECT_SD,
                "total_episodes": sum(len(trace["episodes"])
                                      for trace in antecedents),
            },
        ),
        antecedents,
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
        "segmentation": {
            "schema": SEGMENTATION_SCHEMA,
            "generator_version": SEGMENTATION_GENERATOR_VERSION,
            "planted_seeds_per_effect": args.planted_seeds,
            "planted_effect_sizes": list(PLANTED_EFFECT_SIZES),
            "planted_baseline": PLANTED_BASELINE,
            "planted_weeks": args.planted_weeks,
            "null_traces": args.segmentation_null_traces,
            "null_weeks": args.segmentation_null_weeks,
            "regime_traces": args.regime_traces,
            "regime_weeks": args.regime_weeks,
            "regime_change_week": args.regime_change_week,
            "drifting_traces": args.drifting_traces,
            "drifting_weeks": args.drifting_weeks,
            "sparse_traces": args.sparse_traces,
            "sparse_weeks": args.sparse_weeks,
            "read_by": "rust-service/tests/behavior_segmentation.rs",
        },
        "antecedents": {
            "schema": ANTECEDENT_SCHEMA,
            "generator_version": ANTECEDENT_GENERATOR_VERSION,
            "planted_seeds_per_cell": args.antecedent_planted_seeds,
            "risk_given_antecedent": list(ANTECEDENT_RISK_GIVEN_ANTECEDENT),
            "baseline_risk": ANTECEDENT_BASELINE_RISK,
            "history_weeks": list(ANTECEDENT_HISTORY_WEEKS),
            "planted_candidate_id": f"prevcat_{ANTECEDENT_CATEGORY}",
            "antecedent_prevalence": ANTECEDENT_PREVALENCE,
            "day_effect_sd": ANTECEDENT_DAY_EFFECT_SD,
            "null_traces": args.antecedent_null_traces,
            "null_weeks": args.antecedent_null_weeks,
            "saturated_traces": args.antecedent_saturated_traces,
            "saturated_weeks": args.antecedent_saturated_weeks,
            "sparse_traces": args.antecedent_sparse_traces,
            "sparse_weeks": args.antecedent_sparse_weeks,
            "read_by": "rust-service/tests/behavior_antecedents.rs",
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
