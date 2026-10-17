#!/usr/bin/env python3
"""Computes the self-rating readout and its two hypotheses from tester exports.

The question after a block ("How much of this block was on what you meant to
do?") is the person's judgement of a whole block. This script asks one thing
of it: whether Velvt's factual block summaries agree with what people say about
their own blocks. It reports agreement. It never grades a person or a block, and
nothing it computes runs in the app.

The plan is `plan/attention-labels-and-model.md` § 5 in the private workspace.
The dated pre-registration in `pitch-deck-inputs/evidence/traction-summary.md`
names the commit and the SHA-256 of this file and of
`scripts/rating-analysis-config.json`, and it is hashed before any rating row
exists, the founder's Macs included. Until that entry exists this file is a
draft. After it exists, a change here is a dated amendment, written before the
data it would touch, or it is not made. `--provenance` prints the three values
the entry records, and every report repeats them.

This script computes. It does not decide. Every threshold is read from the
frozen config, which this file validates key by key.

Inputs (export format 7; the exporter is PR-5)
----------------------------------------------
One folder per participant, or the files themselves, sharing a name stem:

  <stem>-ratings.csv        every rating row, answered or not:
                            block_id, prompt_version, eligible_at, shown_at,
                            answered_at, not_now_at, expired_at,
                            card_seen_before_answer, answer
  <stem>-blockfeatures.csv  one row per block that has a rating row, at
                            BLOCK_FEATURE_VERSION 1: block_id,
                            block_feature_version, policy_version,
                            card_branch, elapsed_seconds, covered_seconds,
                            coverage_class, work_anchor_share,
                            switch_aways_per_covered_hour, longest_run_share,
                            recovery_ratio, feed_share, unlogged_share,
                            offer_delivered, offer_outcome, purpose,
                            intensity, planned_minutes, origin, hour_bucket,
                            weekend
  <stem>-meta.csv           key,value: export_format (7 or later),
                            exported_at, rating_prompt_version,
                            rating_prompt_enabled_at (the first time the
                            question was ever turned on), v6_period_start,
                            block_feature_version
  <stem>-blocks.csv         as export_cohort_evidence.sh writes it today: at
                            least block_id and started_at for every declared
                            block. The v6 period start and each rated block's
                            start are read from it, and the reactivity proxy
                            counts its rows.
  <stem>-decisions.csv      as export_cohort_evidence.sh writes it today (the
                            columns in DECISION_COLUMNS). It gives the upgrade
                            cut, the anchor check and the eligible decision
                            points of readout item 4.
  <stem>-outcomes.csv       optional, as export_cohort_evidence.sh writes it
                            today. It gives the sustained-anchor labels of
                            readout item 4. A missing file is "not
                            available", never zero.

Populations
-----------
An export is dropped whole, in this order, if it:

  - is missing its ratings, block-feature, meta, blocks or decisions file, or
    one of them lacks a column this script reads;
  - is the founder's (named `founder`, `founder-*` or `founder_*`, or
    `--founder-device`). It is still parsed and counted, as a pipeline check
    labelled "n = 1, founder", and no statistic is computed from it;
  - is a declared reinstall (`--reinstalled`);
  - is older than format 7, or its meta lacks a key above;
  - holds a policy_version 6 decision row whose anchor is not a work category
    (item 5 of the 2026-10-17 v6 amendment: a v6 build from before the anchor
    rule);
  - states a v6_period_start earlier than the computed one (below): its
    history was cleared, and it falls under the reinstall exclusion.

The unit is the person, not the Mac. Each export is one person unless
`--same-person A,B` names exports that are one tester's Macs; every bootstrap,
every leave-one-out fold and every count of testers uses the person. A name
that matches no export, a name in two groups and a group of one are refused.

The v6 primary period (item 1 of the v6 primary-period amendment). It starts
at the started_at of the Mac's first block after the upgrade cut: its first
block that started after its last decision row with a policy_version below 6
(by that row's occurred_at), or its first block if it has no such row. It does
not wait for a decision row. It lasts 14 days. An export that states a later
v6_period_start than the computed one is reported as malformed with that
reason, and the computed start is used. A stated start is never used in the
computed one's place.

Each rating row is then placed by the first rule that applies:

  1. malformed (a CHECK the table enforces does not hold, a value outside its
     closed set, or an answer after Not now with card_seen_before_answer 0),
     or its block has no block-feature row: excluded and listed;
  2. rating prompt version other than 1, or block feature version other than
     1: excluded;
  3. policy_version other than 6: excluded, never pooled;
  4. no v6 period start (the export holds no block after the upgrade cut), or
     no start for the block in the blocks file: excluded (the period cannot
     be checked);
  5. the block started before the period's start (before the upgrade cut):
     excluded;
  6. the block started inside its Mac's v6 primary period, ran under 5
     minutes, or became eligible before the question was first turned on (or
     the export states no rating_prompt_enabled_at): a PROTOCOL DEVIATION (the
     service forbids each), excluded and counted;
  7. otherwise the row is ELIGIBLE. Every eligible row counts in the answer
     rate, answered or not.

Eligible rows that were not answered are missing. Not now, closing and silence
are never counted as an answer, and nothing is imputed. Answered rows:

  8. answered after the card was seen (card_seen_before_answer = 1): the late
     stratum, outside the primary, reported on its own;
  9. purpose healthy_tech_use or work_life_boundary: reported descriptively;
 10. coverage class insufficient: excluded from the primary;
 11. unlogged or needs-a-category share above 0.30: excluded from the primary;
 12. otherwise PRIMARY.

The card table counts rows that reach rule 10, so its "low coverage" row is not
empty by construction.

What it reports, and when
-------------------------
Every report places rows "as of" a moment when it has one: a row counts only
if it was eligible by then, and it is answered, Not now, shown or expired only
if that happened by then. Every timestamp is in the export, so the same rows
give the same report whatever later exports add.

Default: PROGRESS only, at any count. Exports, exclusions and deviations by
count, eligible rows by status, primary ratings and testers, and the stop
rule. It uses each row's status, card_seen_before_answer, purpose, policy
version, period, coverage class and unlogged share, and whose Mac it came
from. It reads the answer and the block features only to check that they are
well-formed and to apply the 300-second floor; no answer value, H1 or H2
feature or summary of either is computed or printed.

`--readout`: the DESCRIPTIVE READOUT, taken once. Refused below 100 primary
ratings, and refused when the stop rule was met first (the readout is then
reported at the one look). It is computed as of the answered_at of the 100th
primary rating (in answered_at order, ties by person, Mac and block), so
running it again reproduces the same numbers. It is the only reading of
answers before the look:

  1. the result card's branch against the person's tier, in counts, over
     rows that reach rule 10; a linearly weighted kappa of the three ordered
     branches against the tiers on primary ratings, with a person-bootstrap
     95% interval;
  2. tau, the between-person SD of the rating on the logit scale, and the
     ICC, from the ordinal model with a person intercept and no feature, each
     with a person-bootstrap 95% interval;
  3. the answer rate, overall and per person, and the median of each block
     feature in answered against unanswered eligible blocks;
  4. in primary blocks with a delivered offer, the tier by the offer's reply
     (was_focused, any other reply, no reply); in primary blocks with at least
     one measured eligible decision point, the tier by the block's share of
     those points with sustained_anchor_900s = 1. An eligible decision point
     is one analyze_cohort.py counts: policy_version 6, gate_verdict offered,
     and a block elapsed time of at least the 180-second warm-up. Measured
     means its outcome row has censor_reason none;
  5. the reactivity proxy, per person: blocks declared in the 14 days before
     and after rating_prompt_enabled_at, summed over the person's Macs;
  6. the late answers and the two excluded purposes, described apart.

Anything else it prints is labelled exploratory. No model of features is
fitted here.

`--final-look` (the one look). Refused until the stop rule is met: 220 primary
ratings from at least 12 testers, or 16 weeks after the earliest
rating_prompt_enabled_at, measured to the latest exported_at, whichever comes
first. The look is computed as of the moment the rule was first met: the
answered_at of the rating that first brought the primary to 220 ratings from
at least 12 testers, or the earliest enablement plus 16 weeks, whichever is
earlier. It prints that cutoff, so a later look is the same look, and it
reports the descriptives (items 1 to 6) at the cutoff, then:

  H1 (primary). Spearman rho between work_anchor_share and the rating coded
     0-4 (little .. all), one-sided, with a person-clustered bootstrap that
     resamples people. Passes if the 5th percentile of the bootstrap is above
     0. Reported beside it: the AUROC of work_anchor_share for "mostly on"
     (all or most) against the rest, and the within-person rho. Reported only
     at 100 or more primary ratings at the cutoff.
  H2 (secondary). A proportional-odds ordinal logistic model on four features
     (work_anchor_share, switch-aways per covered hour, log of the longest
     run's share of elapsed time, offer delivered), standardized slopes under
     a ridge of lambda = 1 (penalty lambda/2 times the sum of squared slopes),
     and a person random intercept, set to 0 for the held-out person. Scored
     by leave-one-person-out log loss against the same procedure on
     work_anchor_share alone. Passes if the 95% person-bootstrap interval on
     the mean improvement lies above 0 AND the calibration slope lies in
     [0.8, 1.2]. A reliability table with at most 5 bins is reported.
     Reported only at 200 or more primary ratings at the cutoff.

Below 100 primary ratings at the cutoff: descriptives only, no claim about any
feature.

Choices the plan left open, fixed here
--------------------------------------
- Ranks with ties take the average rank. Percentiles interpolate linearly
  between order statistics. Every bootstrap draws people with replacement, as
  many as there are, from a stream seeded by the config's seed and the name
  of the statistic, and recomputes the statistic on the pooled rows. A draw
  where the statistic is undefined is skipped and counted.
- Within-person rho: each value minus its person's mean, for people with at
  least 2 primary ratings, then Spearman on the pooled centred values.
- The person intercept SD (tau) is the value on a fixed grid that maximizes the
  Laplace approximation to the marginal likelihood at the penalized joint mode,
  fitted again inside every training fold. tau = 0 is the model without the
  intercept. The readout's tau is the same procedure with no feature.
  ICC = tau^2 / (tau^2 + pi^2 / 3).
- Every fit adds, at each of the five levels, one pseudo-row of weight 0.5 with
  every standardized feature at 0 and no person. It keeps the thresholds
  finite and ordered when a training fold, or the held-out ratings of the
  calibration fit, have no rating at some level. The calibration fit is a fit
  too, and carries them.
- Standardization uses the training rows of each fold (mean, sample SD). A
  feature with no spread in a fold is set to 0 for that fold.
- The longest run's share of elapsed time is floored at 0.01 before the log:
  log(max(longest_run_share, 0.01)).
- The tau and ICC intervals refit the no-feature model on each bootstrap
  draw, with a person drawn twice counted as two people.
- The H2 interval resamples people over the held-out losses. The models are
  not refitted inside the bootstrap.
- The calibration slope is the slope of a proportional-odds fit of the rating
  on the pooled held-out linear term, with free thresholds, no ridge and
  no person intercept.
- The held-out AUROC for "mostly on" ranks the held-out linear term
  (x . beta), not the held-out probability. Each fold fits its thresholds
  without the held-out person, so they shift against that person's own rating
  level, and pooled probabilities then rank people below chance: on the NULL
  family, with people who differ in how they use the scale, the probability
  ranking gave 0.40 where nothing is there. The linear term carries no
  fold threshold: the NULL then gives about 0.49 (a leave-one-out estimate
  runs slightly low when people differ; the NULL spec records the measured
  value), and a leaked intercept or leaked rows raise it well above 0.5.
- Reliability bins split the held-out probability of "mostly on" into equal
  counts: 5 bins, fewer if a bin would hold under 10 rows.
- The stop rule's clock runs from the earliest rating_prompt_enabled_at among
  the analysed exports to the latest exported_at among them.

Wording
-------
Results describe "agreement between Velvt's block summaries and people's own
ratings of their blocks". The report, text or JSON, refuses to print words
that describe anything else (an attention model, accuracy, prediction,
learning); see BANNED_REPORT_PATTERNS.

Synthetic data
--------------
`--synthetic` is for the contract tests in
`scripts/generate_rating_families.py`. Every input must carry `synthetic,1` in
its meta, every line of the report says SYNTHETIC, and only then may the stop
rule be bypassed (the look is then taken on every row, with no cutoff) or the
bootstrap shortened. Real and synthetic exports never mix, in either
direction. A synthetic result is a test of this code, not evidence about
anyone.

Usage:
    python3 scripts/analyze_ratings.py ~/ratings/*/
    python3 scripts/analyze_ratings.py --json ~/ratings/*/
    python3 scripts/analyze_ratings.py --readout ~/ratings/*/
    python3 scripts/analyze_ratings.py --final-look ~/ratings/*/
    python3 scripts/analyze_ratings.py --founder-device my-mac ~/ratings/*/
    python3 scripts/analyze_ratings.py --same-person p04-laptop,p04-desktop ~/ratings/*/
    python3 scripts/analyze_ratings.py --provenance

Must run on the python3 that ships with macOS (3.9), standard library only.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import hashlib
import json
import math
import random
import re
import subprocess
import sys
import textwrap
from collections import Counter
from dataclasses import dataclass, field, replace
from pathlib import Path

SCRIPT_PATH = Path(__file__).resolve()
DEFAULT_CONFIG_PATH = SCRIPT_PATH.with_name("rating-analysis-config.json")

# ---------------------------------------------------------------------------
# The export contract (format 7). PR-5's exporter writes exactly these.
# ---------------------------------------------------------------------------

RATINGS_COLUMNS = (
    "block_id",
    "prompt_version",
    "eligible_at",
    "shown_at",
    "answered_at",
    "not_now_at",
    "expired_at",
    "card_seen_before_answer",
    "answer",
)
BLOCK_FEATURE_COLUMNS = (
    "block_id",
    "block_feature_version",
    "policy_version",
    "card_branch",
    "elapsed_seconds",
    "covered_seconds",
    "coverage_class",
    "work_anchor_share",
    "switch_aways_per_covered_hour",
    "longest_run_share",
    "recovery_ratio",
    "feed_share",
    "unlogged_share",
    "offer_delivered",
    "offer_outcome",
    "purpose",
    "intensity",
    "planned_minutes",
    "origin",
    "hour_bucket",
    "weekend",
)
REQUIRED_META_KEYS = (
    "export_format",
    "exported_at",
    "rating_prompt_version",
    "rating_prompt_enabled_at",
    "v6_period_start",
    "block_feature_version",
)
COVERAGE_CLASSES = ("insufficient", "partial", "good")
CARD_BRANCHES = ("stayed", "came_back", "ended_elsewhere", "low_coverage")
PURPOSES = ("deep_work", "study", "creative_practice", "healthy_tech_use", "work_life_boundary")
INTENSITIES = ("light", "medium", "intense")
TIER_ORDER = ("low", "good", "high")

COMPANION_SUFFIXES = {
    "ratings": "-ratings.csv",
    "blockfeatures": "-blockfeatures.csv",
    "meta": "-meta.csv",
    "blocks": "-blocks.csv",
    "decisions": "-decisions.csv",
    "outcomes": "-outcomes.csv",
}
REQUIRED_FILES = ("ratings", "blockfeatures", "meta", "blocks", "decisions")
# The columns of the companion files that this script reads. The exporter
# writes more; these must be present.
BLOCK_COLUMNS = ("block_id", "started_at")
DECISION_COLUMNS = (
    "decision_id",
    "occurred_at",
    "block_id",
    "policy_version",
    "anchor_category",
    "gate_verdict",
    "block_started_at",
    "block_ended_at",
    "block_total_paused_seconds",
)
OUTCOME_COLUMNS = ("decision_id", "censor_reason", "sustained_anchor_900s")

# Item 5 of the 2026-10-17 v6 amendment: under policy_version 6 every anchor
# is a work category or empty. The same set as analyze_cohort.py.
WORK_ANCHOR_CATEGORIES = ("FOCUS_WORK", "REFERENCE", "TASK_MANAGEMENT", "COMMUNICATION")
# An eligible decision point, as analyze_cohort.py counts it: gate_verdict
# offered, in a block whose elapsed time is at least the warm-up.
ELIGIBLE_VERDICT = "offered"
WARMUP_EXCLUSION_SECONDS = 180

# An offer's stored outcome, grouped by whether the person replied. The five
# replies are `outcome_for` in rust-service/src/work_block/mod.rs; returned and
# no_response are the two ways an offer ends without one.
OFFER_REPLY_GROUPS = {
    "was_focused": ("was_focused",),
    "other_reply": ("accepted_action", "not_helpful", "wrong_classification", "dismissed"),
    "no_reply": ("returned", "no_response"),
}

# The same founder rule as analyze_cohort.py (2026-08-09): identity is the
# folder name or the file stem, because the export carries no device id.
FOUNDER_DEVICE_PREFIXES = ("founder-", "founder_")
SYNTHETIC_META_KEY = "synthetic"

DAY_SECONDS = 86400
WEEK_SECONDS = 7 * DAY_SECONDS

# The model's features, by the names the config uses.
MODEL_FEATURES = (
    "work_anchor_share",
    "switch_aways_per_covered_hour",
    "log_longest_run_share",
    "offer_delivered",
)

# Words that would describe something this analysis is not. Checked against
# every rendered report before it is printed.
BANNED_REPORT_PATTERNS = (
    ("accuracy", re.compile(r"\baccura(?:cy|te|tely)\b", re.IGNORECASE)),
    ("attention model", re.compile(r"\battention\s+model", re.IGNORECASE)),
    ("predict*", re.compile(r"\bpredict\w*", re.IGNORECASE)),
    ("learn*", re.compile(r"\blearn\w*", re.IGNORECASE)),
    ("adapt*", re.compile(r"\badapt\w*", re.IGNORECASE)),
    ("smarter", re.compile(r"\bsmarter\b", re.IGNORECASE)),
    ("behavioral model", re.compile(r"\bbehaviou?ral\s+model", re.IGNORECASE)),
    ("bad focus", re.compile(r"\bbad\s+focus\b", re.IGNORECASE)),
)

AGREEMENT_STATEMENT = (
    "Agreement between Velvt's block summaries and people's own ratings of their "
    "blocks. The rating is a person's judgement of a whole block, not a measure of "
    "momentary attention, and nothing here grades a person."
)
SYNTHETIC_STAMP = "SYNTHETIC contract test, not evidence"


class ConfigError(Exception):
    pass


class WordingError(Exception):
    pass


# ---------------------------------------------------------------------------
# Config: a closed schema, so a key cannot be added or dropped unnoticed.
# ---------------------------------------------------------------------------

_LIST = "list"
_NUMBER = "number"
_INT = "int"
_STR = "str"

CONFIG_SHAPE = {
    "config_version": _INT,
    "rating_prompt_version": _INT,
    "block_feature_version": _INT,
    "minimum_export_format": _INT,
    "analysed_policy_version": _INT,
    "answer_levels": _LIST,
    "tiers": {"high": _LIST, "good": _LIST, "low": _LIST},
    "mostly_on": _LIST,
    "eligibility": {"minimum_elapsed_seconds": _INT, "v6_primary_period_seconds": _INT},
    "population": {
        "excluded_coverage_classes": _LIST,
        "maximum_unlogged_share": _NUMBER,
        "descriptive_only_purposes": _LIST,
    },
    "card_branches": {"ordered_low_to_high": _LIST, "unordered": _LIST, "kappa_weights": _STR},
    "stop_rule": {
        "primary_ratings": _INT,
        "testers": _INT,
        "weeks_after_first_enablement": _INT,
        "descriptive_minimum_ratings": _INT,
        "h2_minimum_ratings": _INT,
    },
    "bootstrap": {"seed": _INT, "replicates": _INT, "quantile_method": _STR},
    "h1": {
        "feature": _STR,
        "one_sided_lower_quantile": _NUMBER,
        "two_sided_interval": _LIST,
        "within_person_minimum_ratings": _INT,
    },
    "h2": {
        "features": _LIST,
        "baseline_features": _LIST,
        "longest_run_share_floor": _NUMBER,
        "ridge_lambda": _NUMBER,
        "person_intercept_sd_grid": _LIST,
        "pseudo_weight_per_level": _NUMBER,
        "improvement_interval": _LIST,
        "calibration_slope_range": _LIST,
        "reliability_bins_max": _INT,
        "reliability_bin_minimum_rows": _INT,
    },
    "missingness": {"anchor_share_quantile_groups": _INT},
    "reactivity": {"window_days": _INT},
}


def _check_shape(value, shape, path: str) -> None:
    if isinstance(shape, dict):
        if not isinstance(value, dict):
            raise ConfigError(f"{path or 'config'} must be an object")
        missing = sorted(set(shape) - set(value))
        extra = sorted(set(value) - set(shape))
        if missing or extra:
            raise ConfigError(f"{path or 'config'}: missing keys {missing}, unexpected keys {extra}")
        for key, sub in shape.items():
            _check_shape(value[key], sub, f"{path}.{key}" if path else key)
        return
    ok = {
        _INT: isinstance(value, int) and not isinstance(value, bool),
        _NUMBER: isinstance(value, (int, float)) and not isinstance(value, bool),
        _STR: isinstance(value, str),
        _LIST: isinstance(value, list),
    }[shape]
    if not ok:
        raise ConfigError(f"{path} must be of type {shape}")


def load_config(path: Path) -> dict:
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ConfigError(f"cannot read {path}: {error}") from error
    _check_shape(config, CONFIG_SHAPE, "")
    if config["config_version"] != 1:
        raise ConfigError("config_version must be 1")
    levels = config["answer_levels"]
    if len(levels) != 5 or len(set(levels)) != 5:
        raise ConfigError("answer_levels must name five distinct levels")
    tiered = config["tiers"]["high"] + config["tiers"]["good"] + config["tiers"]["low"]
    if sorted(tiered) != sorted(levels):
        raise ConfigError("tiers must partition answer_levels")
    if not set(config["mostly_on"]) <= set(levels):
        raise ConfigError("mostly_on must be answer levels")
    if config["card_branches"]["kappa_weights"] != "linear":
        raise ConfigError("kappa_weights must be linear")
    branches = config["card_branches"]["ordered_low_to_high"] + config["card_branches"]["unordered"]
    if sorted(branches) != sorted(CARD_BRANCHES):
        raise ConfigError("card_branches must partition the four card branches")
    for key in ("features", "baseline_features"):
        unknown = set(config["h2"][key]) - set(MODEL_FEATURES)
        if unknown or not config["h2"][key]:
            raise ConfigError(f"h2.{key} names unknown features {sorted(unknown)}")
    if config["h1"]["feature"] != "work_anchor_share":
        raise ConfigError("h1.feature must be work_anchor_share")
    grid = config["h2"]["person_intercept_sd_grid"]
    if not grid or any(not isinstance(t, (int, float)) or t < 0 for t in grid):
        raise ConfigError("h2.person_intercept_sd_grid must be non-negative numbers")
    if not 0.0 < config["h2"]["longest_run_share_floor"] < 1.0:
        raise ConfigError("h2.longest_run_share_floor must lie strictly between 0 and 1")
    return config


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def provenance(config_path: Path) -> dict:
    """What the pre-registration records, recomputed from the files on disk."""
    commit, dirty = "unknown", "unknown"
    try:
        repo = SCRIPT_PATH.parent
        commit = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "-C", str(repo), "status", "--porcelain", "--",
             str(SCRIPT_PATH), str(config_path.resolve())],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        dirty = "yes" if status else "no"
    except (OSError, subprocess.CalledProcessError):
        pass
    return {
        "analysis_sha256": sha256_file(SCRIPT_PATH),
        "config_sha256": sha256_file(config_path),
        "config_path": str(config_path),
        "git_commit": commit,
        "uncommitted_changes_to_these_files": dirty,
    }


# ---------------------------------------------------------------------------
# Parsing helpers (the same conventions as analyze_cohort.py)
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
        number = float(value)
    except ValueError:
        return None
    return number if math.isfinite(number) else None


def _text(row: dict, key: str) -> str:
    return (row.get(key) or "").strip()


def _read_csv(path: Path) -> tuple[list[dict] | None, list[str], str | None]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        return None, [], f"unreadable ({error})"
    reader = csv.DictReader(text.splitlines())
    rows = list(reader)
    return rows, list(reader.fieldnames or []), None


def _is_founder_device(name: str, declared: set) -> bool:
    if name in declared:
        return True
    lowered = name.lower()
    return lowered == "founder" or lowered.startswith(FOUNDER_DEVICE_PREFIXES)


# ---------------------------------------------------------------------------
# Records
# ---------------------------------------------------------------------------


@dataclass
class Export:
    name: str
    files: dict = field(default_factory=dict)
    meta: dict = field(default_factory=dict)
    ratings: list = field(default_factory=list)
    features: list = field(default_factory=list)
    blocks: list | None = None
    decisions: list | None = None
    outcomes: list | None = None
    unusable: str = ""


@dataclass
class BlockFeatures:
    block_id: str
    block_feature_version: int
    policy_version: int
    card_branch: str
    elapsed_seconds: int
    covered_seconds: int
    coverage_class: str
    work_anchor_share: float
    switch_aways_per_covered_hour: float
    longest_run_share: float
    recovery_ratio: float
    feed_share: float
    unlogged_share: float
    offer_delivered: int
    offer_outcome: str
    purpose: str
    intensity: str
    planned_minutes: int
    origin: str
    hour_bucket: str
    weekend: int


@dataclass
class Rating:
    participant: str
    person: str
    block_id: str
    prompt_version: int
    eligible_at: int
    shown_at: int | None
    answered_at: int | None
    not_now_at: int | None
    expired_at: int | None
    card_seen_before_answer: int
    answer: str
    code: int | None
    features: BlockFeatures | None = None

    @property
    def state(self) -> str:
        if self.answer:
            return "answered"
        if self.not_now_at is not None:
            return "not_now"
        if self.shown_at is None:
            return "expired_never_shown" if self.expired_at is not None else "open_never_shown"
        return "expired_after_shown" if self.expired_at is not None else "open_shown"

    def as_of(self, cutoff: int) -> "Rating | None":
        """This row as it stood at `cutoff`, or None if it was not yet eligible.
        Every timestamp is in the export, so this is the same on any later
        export of the same Mac."""
        if self.eligible_at > cutoff:
            return None

        def by(at: int | None) -> int | None:
            return at if at is not None and at <= cutoff else None

        answered = by(self.answered_at) is not None
        return replace(
            self,
            shown_at=by(self.shown_at),
            answered_at=by(self.answered_at),
            not_now_at=by(self.not_now_at),
            expired_at=by(self.expired_at),
            answer=self.answer if answered else "",
            code=self.code if answered else None,
            card_seen_before_answer=self.card_seen_before_answer if answered else 0,
        )


STATES = (
    "answered",
    "not_now",
    "expired_after_shown",
    "expired_never_shown",
    "open_shown",
    "open_never_shown",
)


@dataclass
class Cohort:
    synthetic: bool
    exports_received: list = field(default_factory=list)
    excluded_whole: dict = field(default_factory=dict)
    founder_checks: dict = field(default_factory=dict)
    analysed: list = field(default_factory=list)
    exports: dict = field(default_factory=dict)
    malformed: list = field(default_factory=list)
    people: dict = field(default_factory=dict)
    excluded_rows: Counter = field(default_factory=Counter)
    deviations: Counter = field(default_factory=Counter)
    eligible: list = field(default_factory=list)
    late: list = field(default_factory=list)
    descriptive_purpose: list = field(default_factory=list)
    card_population: list = field(default_factory=list)
    primary_exclusions: Counter = field(default_factory=Counter)
    primary: list = field(default_factory=list)
    period_starts: dict = field(default_factory=dict)
    export_problems: Counter = field(default_factory=Counter)
    declared: dict = field(default_factory=dict)
    cutoff: int | None = None


# ---------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------


def _classify(path: Path) -> tuple[str | None, str]:
    for kind, suffix in COMPANION_SUFFIXES.items():
        if path.name.endswith(suffix):
            return kind, path.name[: -len(suffix)]
    return None, path.stem


def discover(paths: list) -> dict:
    """Groups export files by participant: a folder's name, or a file's stem."""
    exports: dict = {}

    def add(name: str, kind: str, path: Path) -> None:
        export = exports.setdefault(name, Export(name=name))
        if kind in export.files:
            export.unusable = f"two {COMPANION_SUFFIXES[kind]} files"
        export.files[kind] = path

    for path in paths:
        if path.is_dir():
            for child in sorted(path.iterdir()):
                kind, _ = _classify(child)
                if kind is not None and child.is_file():
                    add(path.name, kind, child)
            exports.setdefault(path.name, Export(name=path.name))
        else:
            kind, stem = _classify(path)
            if kind is not None:
                add(stem, kind, path)
    return exports


def read_files(export: Export) -> None:
    """Fills an Export from its files; sets `unusable` when it cannot be read."""
    if export.unusable:
        return
    missing = [COMPANION_SUFFIXES[k] for k in REQUIRED_FILES if k not in export.files]
    if missing:
        export.unusable = f"no {', '.join(missing)} file (an export before format 7 has none)"
        return
    for kind in COMPANION_SUFFIXES:
        if kind not in export.files:
            continue
        rows, header, error = _read_csv(export.files[kind])
        if error:
            export.unusable = f"{COMPANION_SUFFIXES[kind]} {error}"
            return
        if kind == "meta":
            if header[:2] != ["key", "value"]:
                export.unusable = "meta file has no key,value header"
                return
            export.meta = {_text(r, "key"): _text(r, "value") for r in rows}
        elif kind == "ratings":
            absent = [c for c in RATINGS_COLUMNS if c not in header]
            if absent:
                export.unusable = f"ratings file lacks columns {absent}"
                return
            export.ratings = rows
        elif kind == "blockfeatures":
            absent = [c for c in BLOCK_FEATURE_COLUMNS if c not in header]
            if absent:
                export.unusable = f"block-feature file lacks columns {absent}"
                return
            export.features = rows
        else:
            columns = {"blocks": BLOCK_COLUMNS, "decisions": DECISION_COLUMNS,
                       "outcomes": OUTCOME_COLUMNS}[kind]
            absent = [c for c in columns if c not in header]
            if absent:
                export.unusable = f"{COMPANION_SUFFIXES[kind]} file lacks columns {absent}"
                return
            setattr(export, kind, rows)


def _parse_rating(name: str, person: str, row: dict, levels: list) -> tuple[Rating | None, str | None]:
    block_id = _text(row, "block_id")
    if not block_id:
        return None, "no block_id"
    prompt_version = _int(row, "prompt_version")
    eligible_at = _int(row, "eligible_at")
    if prompt_version is None or eligible_at is None:
        return None, "no prompt_version or eligible_at"
    card = _int(row, "card_seen_before_answer")
    if card not in (0, 1):
        return None, "card_seen_before_answer is not 0 or 1"
    answer = _text(row, "answer")
    answered_at = _int(row, "answered_at")
    shown_at = _int(row, "shown_at")
    if answer and answer not in levels:
        return None, f"answer {answer!r} is not one of the five levels"
    if bool(answer) != (answered_at is not None):
        return None, "answer and answered_at disagree"
    if answer and shown_at is None:
        return None, "answered but never shown"
    if card == 1 and not answer:
        return None, "card_seen_before_answer is 1 on an unanswered row"
    # After Not now the card appears, so a later answer is a late one.
    if answer and card == 0 and _int(row, "not_now_at") is not None:
        return None, "answered after Not now with card_seen_before_answer 0"
    return (
        Rating(
            participant=name,
            person=person,
            block_id=block_id,
            prompt_version=prompt_version,
            eligible_at=eligible_at,
            shown_at=shown_at,
            answered_at=answered_at,
            not_now_at=_int(row, "not_now_at"),
            expired_at=_int(row, "expired_at"),
            card_seen_before_answer=card,
            answer=answer,
            code=levels.index(answer) if answer else None,
        ),
        None,
    )


def _parse_features(row: dict) -> tuple[BlockFeatures | None, str | None]:
    ints = {}
    for key in ("block_feature_version", "policy_version", "elapsed_seconds",
                "covered_seconds", "offer_delivered", "planned_minutes", "weekend"):
        value = _int(row, key)
        if value is None:
            return None, f"{key} is not an integer"
        ints[key] = value
    floats = {}
    for key in ("work_anchor_share", "switch_aways_per_covered_hour", "longest_run_share",
                "recovery_ratio", "feed_share", "unlogged_share"):
        value = _float(row, key)
        if value is None:
            return None, f"{key} is not a number"
        floats[key] = value
    for key in ("work_anchor_share", "longest_run_share", "recovery_ratio",
                "feed_share", "unlogged_share"):
        if not 0.0 <= floats[key] <= 1.0:
            return None, f"{key} is outside [0, 1]"
    if floats["switch_aways_per_covered_hour"] < 0:
        return None, "switch_aways_per_covered_hour is negative"
    if ints["offer_delivered"] not in (0, 1) or ints["weekend"] not in (0, 1):
        return None, "offer_delivered or weekend is not 0 or 1"
    card_branch = _text(row, "card_branch")
    coverage = _text(row, "coverage_class")
    purpose = _text(row, "purpose")
    intensity = _text(row, "intensity")
    offer_outcome = _text(row, "offer_outcome")
    if card_branch not in CARD_BRANCHES:
        return None, f"card_branch {card_branch!r} is not one of {CARD_BRANCHES}"
    if coverage not in COVERAGE_CLASSES:
        return None, f"coverage_class {coverage!r} is not one of {COVERAGE_CLASSES}"
    if purpose not in PURPOSES:
        return None, f"purpose {purpose!r} is not one of {PURPOSES}"
    if intensity not in INTENSITIES:
        return None, f"intensity {intensity!r} is not one of {INTENSITIES}"
    if ints["elapsed_seconds"] <= 0:
        return None, "elapsed_seconds is not positive"
    if (coverage == "insufficient") != (card_branch == "low_coverage"):
        return None, "coverage_class and card_branch disagree about low coverage"
    if ints["offer_delivered"] == 0 and offer_outcome:
        return None, "an offer outcome on a block with no offer delivered"
    return (
        BlockFeatures(
            block_id=_text(row, "block_id"),
            card_branch=card_branch,
            coverage_class=coverage,
            offer_outcome=offer_outcome,
            purpose=purpose,
            intensity=intensity,
            origin=_text(row, "origin"),
            hour_bucket=_text(row, "hour_bucket"),
            **ints,
            **floats,
        ),
        None,
    )


def _people(names: list, same_person: tuple) -> dict:
    """Export name -> person. `same_person` holds comma-separated groups of
    exports that are one tester's Macs; the person is the group's first name in
    sorted order, so the mapping does not depend on argument order.

    A name that matches no export, a name in two groups and a group of one are
    refused. A typo would otherwise leave one tester's Macs counted as two
    people in every bootstrap, fold and count of testers."""
    known = set(names)
    people = {name: name for name in names}
    grouped: dict = {}
    for group in same_person:
        members = sorted({n.strip() for n in group.split(",") if n.strip()})
        if len(members) < 2:
            raise ConfigError(f"--same-person {group!r} names fewer than two exports")
        unknown = [m for m in members if m not in known]
        if unknown:
            raise ConfigError(f"--same-person {group!r}: no export is named {', '.join(unknown)}")
        for member in members:
            if member in grouped:
                raise ConfigError(
                    f"--same-person: {member} is in two groups ({grouped[member]!r} and {group!r})"
                )
            grouped[member] = group
            people[member] = members[0]
    return people


def v6_period_start(export: Export, policy_version: int) -> dict:
    """Item 1 of the v6 primary-period amendment: the started_at of the Mac's
    first block after the upgrade cut, from the blocks and decisions files.

    The cut is the occurred_at of the Mac's last decision row with a
    policy_version below `policy_version`; a Mac with none has no cut, and its
    period starts at its first block. The start never waits for a decision row,
    and the meta's stated start is returned beside it, never in its place."""
    older = [
        occurred for occurred, version in (
            (_int(row, "occurred_at"), _int(row, "policy_version")) for row in export.decisions or []
        )
        if occurred is not None and version is not None and version < policy_version
    ]
    cut = max(older) if older else None
    starts = [
        start for start in (_int(row, "started_at") for row in export.blocks or [])
        if start is not None and (cut is None or start > cut)
    ]
    return {
        "computed": min(starts) if starts else None,
        "upgrade_cut": cut,
        "stated": _int(export.meta, "v6_period_start"),
    }


def _non_work_anchors(export: Export, policy_version: int) -> list:
    """Item 5 of the v6 amendment: decision rows of this policy whose anchor is
    not a work category. An offer copies its decision's anchor, so the decision
    rows are enough here."""
    found = []
    for row in export.decisions or []:
        if _int(row, "policy_version") != policy_version:
            continue
        anchor = _text(row, "anchor_category")
        if anchor and anchor.upper() not in WORK_ANCHOR_CATEGORIES:
            found.append(f"decision {_text(row, 'decision_id') or '?'} {anchor!r}")
    return found


def _partition(cohort: Cohort, eligible: list, config: dict) -> None:
    """Rules 8 to 12 of the module docstring, over eligible rows."""
    excluded_coverage = set(config["population"]["excluded_coverage_classes"])
    unlogged_limit = config["population"]["maximum_unlogged_share"]
    descriptive_purposes = set(config["population"]["descriptive_only_purposes"])
    cohort.eligible = list(eligible)
    cohort.late = []
    cohort.descriptive_purpose = []
    cohort.card_population = []
    cohort.primary_exclusions = Counter()
    cohort.primary = []
    for rating in cohort.eligible:
        feature = rating.features
        if rating.state != "answered":
            continue
        if rating.card_seen_before_answer == 1:
            cohort.late.append(rating)
            continue
        if feature.purpose in descriptive_purposes:
            cohort.descriptive_purpose.append(rating)
            continue
        cohort.card_population.append(rating)
        if feature.coverage_class in excluded_coverage:
            cohort.primary_exclusions[f"coverage class {feature.coverage_class}"] += 1
            continue
        if feature.unlogged_share > unlogged_limit:
            cohort.primary_exclusions[
                f"unlogged or needs-a-category share above {unlogged_limit:.2f}"
            ] += 1
            continue
        cohort.primary.append(rating)


def restrict(cohort: Cohort, cutoff: int, config: dict) -> Cohort:
    """The cohort as it stood at `cutoff`: each eligible row as of then (see
    Rating.as_of), placed again by rules 8 to 12. The original is unchanged."""
    view = replace(cohort, cutoff=cutoff)
    rows = [r.as_of(cutoff) for r in cohort.eligible]
    _partition(view, [r for r in rows if r is not None], config)
    return view


def build_cohort(
    exports: dict,
    config: dict,
    *,
    founder_devices: tuple = (),
    reinstalled: tuple = (),
    same_person: tuple = (),
    synthetic: bool = False,
) -> Cohort:
    """Applies the population rules in the module docstring, in their order."""
    cohort = Cohort(synthetic=synthetic)
    declared_founder = {n.strip() for n in founder_devices if n.strip()}
    declared_reinstalled = {n.strip() for n in reinstalled if n.strip()}
    people = _people(sorted(exports), same_person)
    cohort.declared = {
        "same_person": sorted(
            sorted({n.strip() for n in g.split(",") if n.strip()}) for g in same_person
        ),
        "reinstalled": sorted(declared_reinstalled),
        "founder_device": sorted(declared_founder),
        "not_among_the_exports": sorted((declared_founder | declared_reinstalled) - set(exports)),
    }
    levels = config["answer_levels"]
    policy_version = config["analysed_policy_version"]
    period = config["eligibility"]["v6_primary_period_seconds"]
    minimum_elapsed = config["eligibility"]["minimum_elapsed_seconds"]
    eligible: list = []

    for name in sorted(exports):
        export = exports[name]
        cohort.exports_received.append(name)
        is_synthetic = export.meta.get(SYNTHETIC_META_KEY) == "1"
        if export.unusable:
            cohort.excluded_whole[name] = export.unusable
            continue
        if is_synthetic != synthetic:
            raise ConfigError(
                f"{name}: synthetic and real exports never mix; "
                + ("pass --synthetic for synthetic exports" if is_synthetic
                   else "--synthetic was given and this export is not synthetic")
            )
        if _is_founder_device(name, declared_founder):
            cohort.excluded_whole[name] = "the founder's own device (pipeline check only)"
            check = Counter()
            for row in export.ratings:
                rating, error = _parse_rating(name, name, row, levels)
                check["malformed" if error else rating.state] += 1
            check["block_feature_rows"] = len(export.features)
            cohort.founder_checks[name] = dict(sorted(check.items()))
            continue
        if name in declared_reinstalled:
            cohort.excluded_whole[name] = "reinstalled mid-cohort; local history resets with the database"
            continue
        export_format = _int(export.meta, "export_format")
        if export_format is None or export_format < config["minimum_export_format"]:
            cohort.excluded_whole[name] = (
                f"export format {export_format} is older than {config['minimum_export_format']}"
            )
            continue
        absent = [k for k in REQUIRED_META_KEYS if k not in export.meta]
        if absent:
            cohort.excluded_whole[name] = f"meta lacks {absent}"
            continue
        non_work = _non_work_anchors(export, policy_version)
        if non_work:
            shown = ", ".join(non_work[:3]) + (", ..." if len(non_work) > 3 else "")
            cohort.malformed.append(
                f"{name}: {len(non_work)} policy_version {policy_version} decision row(s) with an "
                f"anchor that is not a work category: {shown}"
            )
            cohort.excluded_whole[name] = (
                f"policy_version {policy_version} decision rows with a non-work anchor: a v6 build "
                "from before the 2026-10-15 anchor rule (item 5 of the v6 amendment)"
            )
            continue
        start = v6_period_start(export, policy_version)
        computed, stated = start["computed"], start["stated"]
        if computed is not None and stated is not None and stated < computed:
            cohort.excluded_whole[name] = (
                f"history cleared: the export states a v6 period start ({stated}) earlier than its "
                f"blocks give ({computed}); the reinstall exclusion applies"
            )
            continue
        if computed is not None and stated is not None and stated > computed:
            cohort.malformed.append(
                f"{name}: the meta's v6_period_start {stated} is later than the start its blocks "
                f"give ({computed}); the computed start is used"
            )
            cohort.export_problems[
                "v6_period_start in the meta later than the computed start (malformed; the "
                "computed start is used)"
            ] += 1

        cohort.analysed.append(name)
        cohort.exports[name] = export
        cohort.people[name] = people[name]
        cohort.period_starts[name] = start
        features: dict = {}
        for row in export.features:
            parsed, error = _parse_features(row)
            block_id = _text(row, "block_id")
            if error:
                features[block_id] = error
                continue
            if block_id in features:
                features[block_id] = "two block-feature rows"
                continue
            features[block_id] = parsed
        period_start = computed
        enabled_at = _int(export.meta, "rating_prompt_enabled_at")
        block_starts = {_text(row, "block_id"): _int(row, "started_at") for row in export.blocks or []}
        seen: set = set()
        for row in export.ratings:
            rating, error = _parse_rating(name, people[name], row, levels)
            if rating is not None and rating.block_id in seen:
                rating, error = None, "two rating rows for one block"
            if error:
                cohort.malformed.append(f"{name}: rating row: {error}")
                cohort.excluded_rows["malformed rating row"] += 1
                continue
            seen.add(rating.block_id)
            feature = features.get(rating.block_id)
            if feature is None:
                cohort.malformed.append(f"{name}: block {rating.block_id} has no block-feature row")
                cohort.excluded_rows["no block-feature row"] += 1
                continue
            if isinstance(feature, str):
                cohort.malformed.append(f"{name}: block-feature row: {feature}")
                cohort.excluded_rows["malformed block-feature row"] += 1
                continue
            rating.features = feature
            if rating.prompt_version != config["rating_prompt_version"]:
                cohort.excluded_rows["rating prompt version is not "
                                     f"{config['rating_prompt_version']}"] += 1
                continue
            if feature.block_feature_version != config["block_feature_version"]:
                cohort.excluded_rows["block feature version is not "
                                     f"{config['block_feature_version']}"] += 1
                continue
            if feature.policy_version != policy_version:
                cohort.excluded_rows[f"policy_version is not {policy_version} (never pooled)"] += 1
                continue
            if period_start is None:
                cohort.excluded_rows[
                    "no v6 period start (the export holds no block after the upgrade cut)"
                ] += 1
                continue
            started_at = block_starts.get(rating.block_id)
            if started_at is None:
                cohort.excluded_rows["no start for the block in the blocks file"] += 1
                continue
            if started_at < period_start:
                cohort.excluded_rows[
                    "block started before the v6 period start (before the upgrade cut)"
                ] += 1
                continue
            if started_at < period_start + period:
                cohort.deviations["rated block started inside the v6 primary period (before day 14)"] += 1
                continue
            if feature.elapsed_seconds < minimum_elapsed:
                cohort.deviations[f"block shorter than {minimum_elapsed} s"] += 1
                continue
            if enabled_at is None:
                cohort.deviations["rating row, but the export states no rating_prompt_enabled_at"] += 1
                continue
            if rating.eligible_at < enabled_at:
                cohort.deviations[
                    "rating row for a block that ended before the question was first turned on"
                ] += 1
                continue
            eligible.append(rating)
    _partition(cohort, eligible, config)
    return cohort


def load(
    paths: list, config: dict, *, founder_devices=(), reinstalled=(), same_person=(), synthetic=False
) -> Cohort:
    exports = discover(paths)
    for export in exports.values():
        read_files(export)
    return build_cohort(
        exports, config, founder_devices=founder_devices, reinstalled=reinstalled,
        same_person=same_person, synthetic=synthetic,
    )


# ---------------------------------------------------------------------------
# Statistics. Standard library only, every tie and edge case decided here.
# ---------------------------------------------------------------------------


def _mean(values: list) -> float | None:
    return sum(values) / len(values) if values else None


def _median(values: list) -> float | None:
    return quantile(sorted(values), 0.5) if values else None


def quantile(sorted_values: list, q: float) -> float | None:
    """Linear interpolation between order statistics (type 7)."""
    n = len(sorted_values)
    if n == 0:
        return None
    h = (n - 1) * q
    lower = math.floor(h)
    upper = min(lower + 1, n - 1)
    return sorted_values[lower] + (h - lower) * (sorted_values[upper] - sorted_values[lower])


def ranks(values: list) -> list:
    """1-based ranks; ties take the average rank."""
    order = sorted(range(len(values)), key=values.__getitem__)
    result = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        average = (i + j) / 2.0 + 1.0
        for t in range(i, j + 1):
            result[order[t]] = average
        i = j + 1
    return result


def pearson(x: list, y: list) -> float | None:
    n = len(x)
    if n < 2:
        return None
    mx = sum(x) / n
    my = sum(y) / n
    sxx = sxy = syy = 0.0
    for a, b in zip(x, y):
        da = a - mx
        db = b - my
        sxx += da * da
        syy += db * db
        sxy += da * db
    if sxx <= 0 or syy <= 0:
        return None
    return sxy / math.sqrt(sxx * syy)


def spearman(x: list, y: list) -> float | None:
    if len(x) < 2:
        return None
    return pearson(ranks(x), ranks(y))


def auroc(scores: list, labels: list) -> float | None:
    """Mann-Whitney AUROC; a tie between a positive and a negative counts 0.5."""
    positives = sum(1 for label in labels if label)
    negatives = len(labels) - positives
    if positives == 0 or negatives == 0:
        return None
    rank = ranks(scores)
    rank_sum = sum(r for r, label in zip(rank, labels) if label)
    return (rank_sum - positives * (positives + 1) / 2.0) / (positives * negatives)


def weighted_kappa(first: list, second: list, categories: int) -> float | None:
    """Cohen's kappa with linear weights over ordered categories 0..categories-1."""
    n = len(first)
    if n == 0 or categories < 2:
        return None
    observed = [[0.0] * categories for _ in range(categories)]
    for a, b in zip(first, second):
        observed[a][b] += 1.0 / n
    rows = [sum(observed[i]) for i in range(categories)]
    cols = [sum(observed[i][j] for i in range(categories)) for j in range(categories)]
    agree_observed = agree_expected = 0.0
    for i in range(categories):
        for j in range(categories):
            weight = 1.0 - abs(i - j) / (categories - 1)
            agree_observed += weight * observed[i][j]
            agree_expected += weight * rows[i] * cols[j]
    if agree_expected >= 1.0:
        return None
    return (agree_observed - agree_expected) / (1.0 - agree_expected)


def _stream(seed: int, label: str) -> random.Random:
    digest = hashlib.sha256(f"{seed}:{label}".encode("utf-8")).digest()
    return random.Random(int.from_bytes(digest[:8], "big"))


def person_bootstrap(groups: list, statistic, replicates: int, rng: random.Random) -> tuple[list, int]:
    """Resamples people with replacement; returns sorted defined values and the undefined count."""
    values = []
    undefined = 0
    count = len(groups)
    if count == 0:
        return values, replicates
    for _ in range(replicates):
        pooled: list = []
        for _ in range(count):
            pooled.extend(groups[rng.randrange(count)])
        value = statistic(pooled)
        if value is None:
            undefined += 1
        else:
            values.append(value)
    values.sort()
    return values, undefined


def _interval(values: list, low: float, high: float) -> list | None:
    if not values:
        return None
    return [quantile(values, low), quantile(values, high)]


# ---------------------------------------------------------------------------
# The ordinal model: proportional odds, ridge on slopes, a Gaussian person
# intercept, fitted by Newton's method with a backtracking line search.
#
#   P(y <= k | x, person j) = sigmoid(theta_k - (x . beta + u_j)),  k = 0..3
#
# minimizing  -sum log P(y_i)  +  ridge/2 * |beta|^2  +  sum_j u_j^2 / (2 tau^2)
# plus the pseudo-rows described in the module docstring. The objective is
# convex where the thresholds are ordered, and the line search never leaves
# that region.
# ---------------------------------------------------------------------------


def _sigmoid(t: float) -> float:
    if t >= 0:
        return 1.0 / (1.0 + math.exp(-t))
    z = math.exp(t)
    return z / (1.0 + z)


def _cell(upper: float | None, lower: float | None) -> float:
    """sigmoid(upper) - sigmoid(lower), with None as +inf / -inf, without cancellation."""
    if upper is None:
        return 1.0 if lower is None else _sigmoid(-lower)
    if lower is None:
        return _sigmoid(upper)
    if lower > 0:
        return _sigmoid(-lower) - _sigmoid(-upper)
    return _sigmoid(upper) - _sigmoid(lower)


def level_probabilities(thresholds: list, eta: float) -> list:
    count = len(thresholds) + 1
    probabilities = []
    for k in range(count):
        upper = thresholds[k] - eta if k < count - 1 else None
        lower = thresholds[k - 1] - eta if k > 0 else None
        probabilities.append(_cell(upper, lower))
    return probabilities


@dataclass
class OrdinalFit:
    thresholds: list
    beta: list
    intercepts: list
    tau: float
    objective: float
    log_marginal: float
    iterations: int
    converged: bool

    def eta(self, x: list) -> float:
        return sum(b * v for b, v in zip(self.beta, x))


class _OrdinalProblem:
    def __init__(self, X, y, groups, levels, ridge, tau, pseudo_weight):
        self.T = levels - 1
        self.p = len(X[0]) if X else 0
        self.G = (max(groups) + 1) if (tau > 0 and groups) else 0
        self.D = self.T + self.p + self.G
        self.ridge = ridge
        self.inverse_tau2 = 1.0 / (tau * tau) if self.G else 0.0
        rows = [(X[i], y[i], groups[i] if self.G else -1, 1.0) for i in range(len(y))]
        if pseudo_weight > 0:
            zero = [0.0] * self.p
            rows.extend((zero, k, -1, pseudo_weight) for k in range(levels))
        self.rows = rows

    def _eta(self, params: list, x: list, g: int) -> float:
        offset = self.T
        eta = 0.0
        for m in range(self.p):
            eta += x[m] * params[offset + m]
        if g >= 0:
            eta += params[offset + self.p + g]
        return eta

    def _penalty(self, params: list) -> float:
        T, p = self.T, self.p
        total = 0.5 * self.ridge * sum(b * b for b in params[T:T + p])
        if self.G:
            total += 0.5 * self.inverse_tau2 * sum(u * u for u in params[T + p:])
        return total

    def objective(self, params: list) -> float:
        T = self.T
        for t in range(1, T):
            if not params[t] > params[t - 1]:
                return math.inf
        total = 0.0
        for x, k, g, w in self.rows:
            eta = self._eta(params, x, g)
            probability = _cell(params[k] - eta if k < T else None, params[k - 1] - eta if k > 0 else None)
            if not probability > 0.0:
                return math.inf
            total -= w * math.log(probability)
        return total + self._penalty(params)

    def derivatives(self, params: list) -> tuple[list, list]:
        T, p, D = self.T, self.p, self.D
        grad = [0.0] * D
        hess = [[0.0] * D for _ in range(D)]
        for x, k, g, w in self.rows:
            eta = self._eta(params, x, g)
            upper = params[k] - eta if k < T else None
            lower = params[k - 1] - eta if k > 0 else None
            probability = _cell(upper, lower)
            if upper is None:
                fu = du = 0.0
            else:
                su = _sigmoid(upper)
                fu = su * (1.0 - su)
                du = fu * (1.0 - 2.0 * su)
            if lower is None:
                fl = dl = 0.0
            else:
                sl = _sigmoid(lower)
                fl = sl * (1.0 - sl)
                dl = fl * (1.0 - 2.0 * sl)
            la = fu / probability
            lb = -fl / probability
            laa = du / probability - la * la
            lbb = -dl / probability - lb * lb
            lab = -la * lb
            # The objective carries -w * log P.
            ga, gb = -w * la, -w * lb
            haa, hbb, hab = -w * laa, -w * lbb, -w * lab
            a_index = k if upper is not None else -1
            b_index = k - 1 if lower is not None else -1
            shared_index = [T + m for m in range(p)]
            shared_value = [-v for v in x]
            if g >= 0:
                shared_index.append(T + p + g)
                shared_value.append(-1.0)
            if a_index >= 0:
                grad[a_index] += ga
                hess[a_index][a_index] += haa
            if b_index >= 0:
                grad[b_index] += gb
                hess[b_index][b_index] += hbb
            if a_index >= 0 and b_index >= 0:
                hess[a_index][b_index] += hab
                hess[b_index][a_index] += hab
            g_shared = ga + gb
            ca = haa + hab
            cb = hbb + hab
            cc = haa + hbb + 2.0 * hab
            for i1, v1 in zip(shared_index, shared_value):
                grad[i1] += g_shared * v1
                if a_index >= 0:
                    hess[a_index][i1] += ca * v1
                    hess[i1][a_index] += ca * v1
                if b_index >= 0:
                    hess[b_index][i1] += cb * v1
                    hess[i1][b_index] += cb * v1
                row = hess[i1]
                scaled = cc * v1
                for i2, v2 in zip(shared_index, shared_value):
                    row[i2] += scaled * v2
        for m in range(p):
            grad[T + m] += self.ridge * params[T + m]
            hess[T + m][T + m] += self.ridge
        for j in range(self.G):
            index = T + p + j
            grad[index] += self.inverse_tau2 * params[index]
            hess[index][index] += self.inverse_tau2
        return grad, hess


def _cholesky_solve(matrix: list, rhs: list) -> list | None:
    n = len(matrix)
    lower = [[0.0] * n for _ in range(n)]
    for i in range(n):
        li = lower[i]
        for j in range(i + 1):
            lj = lower[j]
            total = matrix[i][j]
            for k in range(j):
                total -= li[k] * lj[k]
            if i == j:
                if total <= 0.0:
                    return None
                li[i] = math.sqrt(total)
            else:
                li[j] = total / lj[j]
    z = [0.0] * n
    for i in range(n):
        total = rhs[i]
        li = lower[i]
        for k in range(i):
            total -= li[k] * z[k]
        z[i] = total / li[i]
    solution = [0.0] * n
    for i in reversed(range(n)):
        total = z[i]
        for k in range(i + 1, n):
            total -= lower[k][i] * solution[k]
        solution[i] = total / lower[i][i]
    return solution


def _solve_spd(matrix: list, rhs: list) -> list | None:
    solution = _cholesky_solve(matrix, rhs)
    if solution is not None:
        return solution
    n = len(matrix)
    scale = max((abs(matrix[i][i]) for i in range(n)), default=1.0) or 1.0
    damping = 1e-10 * scale
    while damping < 1e6 * scale:
        damped = [row[:] for row in matrix]
        for i in range(n):
            damped[i][i] += damping
        solution = _cholesky_solve(damped, rhs)
        if solution is not None:
            return solution
        damping *= 10.0
    return None


def _initial_params(problem: _OrdinalProblem, y: list, levels: int, start: OrdinalFit | None) -> list:
    if start is not None and len(start.thresholds) == problem.T and len(start.beta) == problem.p:
        intercepts = start.intercepts if len(start.intercepts) == problem.G else [0.0] * problem.G
        params = list(start.thresholds) + list(start.beta) + list(intercepts)
        if math.isfinite(problem.objective(params)):
            return params
    counts = [0.5] * levels
    for value in y:
        counts[value] += 1.0
    total = sum(counts)
    thresholds = []
    cumulative = 0.0
    for k in range(levels - 1):
        cumulative += counts[k]
        share = cumulative / total
        thresholds.append(math.log(share / (1.0 - share)))
    return thresholds + [0.0] * problem.p + [0.0] * problem.G


def fit_ordinal(
    X: list,
    y: list,
    groups: list | None,
    levels: int,
    *,
    ridge: float,
    tau: float,
    pseudo_weight: float,
    start: OrdinalFit | None = None,
    max_iterations: int = 100,
) -> OrdinalFit:
    problem = _OrdinalProblem(X, y, groups or [], levels, ridge, tau, pseudo_weight)
    params = _initial_params(problem, y, levels, start)
    value = problem.objective(params)
    converged = False
    iterations = 0
    for iterations in range(1, max_iterations + 1):
        grad, hess = problem.derivatives(params)
        step = _solve_spd(hess, [-g for g in grad])
        if step is None:
            break
        decrement = -sum(g * s for g, s in zip(grad, step))
        if decrement < 1e-10:
            converged = True
            break
        t = 1.0
        accepted = None
        while t > 1e-12:
            candidate = [a + t * s for a, s in zip(params, step)]
            candidate_value = problem.objective(candidate)
            if candidate_value <= value - 1e-4 * t * decrement:
                accepted = (candidate, candidate_value)
                break
            t *= 0.5
        if accepted is None:
            converged = decrement < 1e-6
            break
        params, value = accepted
    T, p = problem.T, problem.p
    log_marginal = -value
    if problem.G:
        _, hess = problem.derivatives(params)
        for j in range(problem.G):
            index = T + p + j
            log_marginal -= math.log(tau) + 0.5 * math.log(hess[index][index])
    return OrdinalFit(
        thresholds=params[:T],
        beta=params[T:T + p],
        intercepts=params[T + p:],
        tau=tau if problem.G else 0.0,
        objective=value,
        log_marginal=log_marginal,
        iterations=iterations,
        converged=converged,
    )


def fit_ordinal_tau(
    X: list, y: list, groups: list, levels: int, *, ridge: float, grid: list, pseudo_weight: float
) -> tuple[OrdinalFit | None, list]:
    """The fit at the grid value of tau with the largest Laplace marginal likelihood."""
    best = None
    profile = []
    previous = None
    for tau in grid:
        fit = fit_ordinal(
            X, y, groups, levels, ridge=ridge, tau=float(tau), pseudo_weight=pseudo_weight,
            start=previous,
        )
        profile.append({"tau": float(tau), "log_marginal": fit.log_marginal, "converged": fit.converged})
        if fit.converged and (best is None or fit.log_marginal > best.log_marginal + 1e-12):
            best = fit
        previous = fit
    return best, profile


def _group_indices(names: list) -> list:
    index: dict = {}
    return [index.setdefault(name, len(index)) for name in names]


def _model_value(features: BlockFeatures, name: str, share_floor: float) -> float:
    if name == "work_anchor_share":
        return features.work_anchor_share
    if name == "switch_aways_per_covered_hour":
        return features.switch_aways_per_covered_hour
    if name == "log_longest_run_share":
        return math.log(max(features.longest_run_share, share_floor))
    if name == "offer_delivered":
        return float(features.offer_delivered)
    raise KeyError(name)


def standardizer(rows: list) -> list:
    """(mean, sd) per column from these rows; sd 0 marks a column with no spread."""
    if not rows:
        return []
    columns = len(rows[0])
    result = []
    for c in range(columns):
        values = [r[c] for r in rows]
        mean = sum(values) / len(values)
        if len(values) < 2:
            result.append((mean, 0.0))
            continue
        variance = sum((v - mean) ** 2 for v in values) / (len(values) - 1)
        result.append((mean, math.sqrt(variance)))
    return result


def standardize(row: list, scale: list) -> list:
    return [((v - m) / s) if s > 0 else 0.0 for v, (m, s) in zip(row, scale)]


# ---------------------------------------------------------------------------
# The readout
# ---------------------------------------------------------------------------


def tier_of(answer: str, config: dict) -> str:
    for tier in TIER_ORDER:
        if answer in config["tiers"][tier]:
            return tier
    raise KeyError(answer)


def _share(numerator: int, denominator: int) -> float | None:
    return numerator / denominator if denominator else None


def accounting(cohort: Cohort) -> dict:
    """Progress by status only: no answer and no feature value is read here."""
    states = Counter(r.state for r in cohort.eligible)
    shown = sum(1 for r in cohort.eligible if r.shown_at is not None)
    return {
        "eligible": len(cohort.eligible),
        "by_state": {s: states.get(s, 0) for s in STATES},
        "shown": shown,
        "late_answers": len(cohort.late),
        "descriptive_purpose_answers": len(cohort.descriptive_purpose),
        "card_population": len(cohort.card_population),
        "primary_exclusions": dict(sorted(cohort.primary_exclusions.items())),
        "primary": len(cohort.primary),
        "primary_testers": len({r.person for r in cohort.primary}),
        "statement": "Not now, closing and silence are never counted as an answer. "
                     "Missing stays missing; nothing is imputed.",
    }


# The block features compared between answered and unanswered blocks.
MEDIAN_FEATURES = (
    "work_anchor_share",
    "switch_aways_per_covered_hour",
    "longest_run_share",
    "recovery_ratio",
    "feed_share",
    "unlogged_share",
    "covered_seconds",
    "elapsed_seconds",
    "planned_minutes",
)


def answer_rate(cohort: Cohort, config: dict) -> dict:
    """Readout item 3: is a rating missing at random with respect to the block?"""
    rows = cohort.eligible

    def rate(subset: list) -> dict:
        answered = sum(1 for r in subset if r.state == "answered")
        return {"eligible": len(subset), "answered": answered, "rate": _share(answered, len(subset))}

    answered = [r for r in rows if r.state == "answered"]
    unanswered = [r for r in rows if r.state != "answered"]
    medians = {}
    for name in MEDIAN_FEATURES:
        a = _median([float(getattr(r.features, name)) for r in answered])
        u = _median([float(getattr(r.features, name)) for r in unanswered])
        medians[name] = {
            "answered": a,
            "not_answered": u,
            "difference": (a - u) if a is not None and u is not None else None,
        }
    offer_share = {
        "answered": _mean([float(r.features.offer_delivered) for r in answered]),
        "not_answered": _mean([float(r.features.offer_delivered) for r in unanswered]),
    }
    latencies = [
        r.answered_at - r.shown_at for r in answered if r.card_seen_before_answer == 0
    ]

    exploratory: dict = {
        "by_coverage_class": {
            c: rate([r for r in rows if r.features.coverage_class == c]) for c in COVERAGE_CLASSES
        },
        "by_card_branch": {
            b: rate([r for r in rows if r.features.card_branch == b]) for b in CARD_BRANCHES
        },
    }
    groups = config["missingness"]["anchor_share_quantile_groups"]
    values = sorted(r.features.work_anchor_share for r in rows)
    cuts = [quantile(values, i / groups) for i in range(1, groups)] if values else []
    by_group = []
    for i in range(groups):
        low = cuts[i - 1] if i > 0 and cuts else None
        high = cuts[i] if i < len(cuts) else None
        subset = [
            r for r in rows
            if (low is None or r.features.work_anchor_share > low)
            and (high is None or r.features.work_anchor_share <= high)
        ]
        by_group.append({"lower_exclusive": low, "upper_inclusive": high, **rate(subset)})
    exploratory["by_work_anchor_share_group"] = by_group
    return {
        "overall": rate(rows),
        "rate_when_shown": _share(len(answered), sum(1 for r in rows if r.shown_at is not None)),
        "by_person": {person: rate(group) for person, group in _people_groups(rows)},
        "median_seconds_shown_to_answer": _median(latencies),
        "feature_medians": medians,
        "offer_delivered_share": offer_share,
        "exploratory": exploratory,
    }


def rating_distribution(rows: list, config: dict) -> dict:
    answers = Counter(r.answer for r in rows)
    tiers = Counter(tier_of(r.answer, config) for r in rows)
    return {
        "n": len(rows),
        "by_answer": {level: answers.get(level, 0) for level in config["answer_levels"]},
        "by_tier": {tier: tiers.get(tier, 0) for tier in TIER_ORDER},
    }


def card_table(cohort: Cohort, config: dict, replicates: int) -> dict:
    """Readout item 1: the result card's branch against the person's tier."""
    table = {branch: {tier: 0 for tier in TIER_ORDER} for branch in CARD_BRANCHES}
    for r in cohort.card_population:
        table[r.features.card_branch][tier_of(r.answer, config)] += 1
    ordered = config["card_branches"]["ordered_low_to_high"]

    def kappa(subset: list) -> float | None:
        pairs = [
            (ordered.index(r.features.card_branch), TIER_ORDER.index(tier_of(r.answer, config)))
            for r in subset
            if r.features.card_branch in ordered
        ]
        if not pairs:
            return None
        return weighted_kappa([a for a, _ in pairs], [b for _, b in pairs], len(ordered))

    boot, undefined = person_bootstrap(
        _by_person(cohort.primary), kappa, replicates,
        _stream(config["bootstrap"]["seed"], "card_kappa"),
    )
    return {
        "population": len(cohort.card_population),
        "table": table,
        "branch_order_low_to_high": ordered,
        "tier_order_low_to_high": list(TIER_ORDER),
        "kappa_rows": sum(1 for r in cohort.primary if r.features.card_branch in ordered),
        "weighted_kappa": kappa(cohort.primary),
        "weighted_kappa_interval_95": _interval(boot, 0.025, 0.975),
        "bootstrap_undefined": undefined,
        "note": "The table counts rows answered before the card, purposes other than "
                "healthy_tech_use and work_life_boundary, before the coverage and unlogged "
                "exclusions. Kappa uses primary ratings in the three ordered branches (ended "
                "elsewhere < came back < stayed) against low < good < high, with linear weights.",
    }


def _tau(rows: list, persons: list, config: dict) -> float | None:
    fit, _ = fit_ordinal_tau(
        [[] for _ in rows], [r.code for r in rows], _group_indices(persons),
        len(config["answer_levels"]), ridge=config["h2"]["ridge_lambda"],
        grid=config["h2"]["person_intercept_sd_grid"],
        pseudo_weight=config["h2"]["pseudo_weight_per_level"],
    )
    return fit.tau if fit else None


def _icc(tau: float | None) -> float | None:
    return tau * tau / (tau * tau + math.pi ** 2 / 3.0) if tau is not None else None


def person_variation(rows: list, config: dict, replicates: int) -> dict:
    """Readout item 2: tau and ICC from the ordinal model with a person intercept
    and no feature, with person-bootstrap intervals."""
    if not rows:
        return {"people": 0, "tau": None, "icc_latent": None}
    tau = _tau(rows, [r.person for r in rows], config)
    groups = _by_person(rows)
    rng = _stream(config["bootstrap"]["seed"], "person_variation")
    draws = []
    undefined = 0
    for _ in range(replicates):
        pooled, persons = [], []
        for slot in range(len(groups)):
            group = groups[rng.randrange(len(groups))]
            pooled.extend(group)
            persons.extend([slot] * len(group))
        value = _tau(pooled, persons, config)
        if value is None:
            undefined += 1
        else:
            draws.append(value)
    draws.sort()
    counts = sorted(len(g) for g in groups)
    interval = _interval(draws, 0.025, 0.975)
    return {
        "people": len(groups),
        "ratings_per_person": {"min": counts[0], "median": _median(counts), "max": counts[-1]},
        "tau": tau,
        "tau_interval_95": interval,
        "icc_latent": _icc(tau),
        "icc_interval_95": [_icc(interval[0]), _icc(interval[1])] if interval else None,
        "bootstrap_undefined": undefined,
        "tau_at_grid_edge": tau is not None and tau == max(config["h2"]["person_intercept_sd_grid"]),
    }


def _reply_group(outcome: str) -> str:
    for group, outcomes in OFFER_REPLY_GROUPS.items():
        if outcome in outcomes:
            return group
    return "unrecorded"


def offer_replies(rows: list, config: dict) -> dict:
    """Readout item 4: the tier by the offer's reply, in blocks with a delivered offer."""
    offered = [r for r in rows if r.features.offer_delivered == 1]
    table = {g: {tier: 0 for tier in TIER_ORDER} for g in tuple(OFFER_REPLY_GROUPS) + ("unrecorded",)}
    for r in offered:
        table[_reply_group(r.features.offer_outcome)][tier_of(r.answer, config)] += 1
    return {"offered_blocks": len(offered), "by_reply": table}


def _block_elapsed(row: dict) -> int | None:
    """The gate's elapsed-time arithmetic on a decision row's block, as
    analyze_cohort.py computes it. None while the block is open."""
    started = _int(row, "block_started_at")
    ended = _int(row, "block_ended_at")
    if started is None or ended is None:
        return None
    return ended - started - (_int(row, "block_total_paused_seconds") or 0)


def sustained_anchor(cohort: Cohort, rows: list, config: dict) -> dict:
    """Readout item 4: the tier by the block's share of measured eligible
    decision points with sustained_anchor_900s = 1.

    Eligible as analyze_cohort.py counts it: policy_version 6, gate_verdict
    offered, block elapsed time at least the 180-second warm-up. Measured: the
    outcome row's censor_reason is none. Abstentions, held and withheld
    decisions have outcome rows too (the export writes one per anchored
    decision), and they are not eligible points."""
    available = [
        name for name, export in cohort.exports.items()
        if export.decisions is not None and export.outcomes is not None
    ]
    by_block: dict = {}
    for name in available:
        export = cohort.exports[name]
        labels = {}
        for row in export.outcomes:
            if _text(row, "censor_reason") == "none" and _text(row, "sustained_anchor_900s") in ("0", "1"):
                labels[_text(row, "decision_id")] = int(_text(row, "sustained_anchor_900s"))
        for row in export.decisions:
            if _int(row, "policy_version") != config["analysed_policy_version"]:
                continue
            if _text(row, "gate_verdict") != ELIGIBLE_VERDICT:
                continue
            elapsed = _block_elapsed(row)
            if elapsed is not None and elapsed < WARMUP_EXCLUSION_SECONDS:
                continue
            label = labels.get(_text(row, "decision_id"))
            if label is not None:
                by_block.setdefault((name, _text(row, "block_id")), []).append(label)
    matched = [
        (r, sum(by_block[(r.participant, r.block_id)]) / len(by_block[(r.participant, r.block_id)]))
        for r in rows
        if (r.participant, r.block_id) in by_block
    ]
    by_tier = {}
    for tier in TIER_ORDER:
        shares = [share for r, share in matched if tier_of(r.answer, config) == tier]
        by_tier[tier] = {"blocks": len(shares), "mean_share": _mean(shares), "median_share": _median(shares)}
    return {
        "exports_with_labels": len(available),
        "exports_without_labels": sorted(set(cohort.exports) - set(available)),
        "blocks_with_a_measured_eligible_point": len(matched),
        "by_tier": by_tier,
    }


def reactivity(cohort: Cohort, config: dict) -> dict:
    """Readout item 5, per person: blocks declared in the window before and
    after each Mac's rating_prompt_enabled_at, summed over the person's Macs.

    A Mac counts only when its window after has closed by the export and by the
    cutoff; a person is measured only when every Mac of theirs on which the
    question was turned on counts."""
    window = config["reactivity"]["window_days"] * DAY_SECONDS
    macs = []
    for name in sorted(cohort.exports):
        export = cohort.exports[name]
        enabled = _int(export.meta, "rating_prompt_enabled_at")
        exported = _int(export.meta, "exported_at")
        entry = {"mac": name, "person": cohort.people[name]}
        if enabled is None:
            macs.append(dict(entry, status="question never enabled"))
            continue
        if exported is None or exported < enabled + window:
            macs.append(dict(entry, status="export ends inside the window after"))
            continue
        if cohort.cutoff is not None and cohort.cutoff < enabled + window:
            macs.append(dict(entry, status="the window after ends after the cutoff"))
            continue
        starts = [_int(row, "started_at") for row in export.blocks or []]
        before = sum(1 for s in starts if s is not None and enabled - window <= s < enabled)
        after = sum(1 for s in starts if s is not None and enabled <= s < enabled + window)
        macs.append(dict(entry, status="measured", before=before, after=after))
    people = []
    for person in sorted({m["person"] for m in macs}):
        mine = [m for m in macs if m["person"] == person and m["status"] != "question never enabled"]
        if not mine:
            continue
        open_macs = [m["mac"] for m in mine if m["status"] != "measured"]
        if open_macs:
            people.append({"person": person, "status": "not measured: " + ", ".join(
                f"{m['mac']} ({m['status']})" for m in mine if m["status"] != "measured")})
            continue
        people.append({
            "person": person,
            "status": "measured",
            "macs": [m["mac"] for m in mine],
            "before": sum(m["before"] for m in mine),
            "after": sum(m["after"] for m in mine),
        })
    measured = [p for p in people if p["status"] == "measured"]
    return {
        "window_days": config["reactivity"]["window_days"],
        "people": people,
        "macs": macs,
        "median_change": _median([p["after"] - p["before"] for p in measured]),
        "note": "Descriptive, per person. Confounded with time since install, and labelled so.",
    }


def _people_groups(rows: list) -> list:
    groups: dict = {}
    for r in rows:
        groups.setdefault(r.person, []).append(r)
    return [(person, groups[person]) for person in sorted(groups)]


def _by_person(rows: list) -> list:
    return [group for _, group in _people_groups(rows)]


# ---------------------------------------------------------------------------
# Stop rule
# ---------------------------------------------------------------------------


def primary_in_order(rows: list) -> list:
    """Primary ratings in the order they were answered; ties by person, Mac and
    block, so the order never depends on the order of the files."""
    return sorted(rows, key=lambda r: (r.answered_at, r.person, r.participant, r.block_id))


def first_reached(rows: list, count: int, testers: int = 0) -> int | None:
    """The answered_at at which the primary ratings first numbered `count`
    from at least `testers` people, or None if they never did."""
    seen: set = set()
    for index, r in enumerate(primary_in_order(rows), 1):
        seen.add(r.person)
        if index >= count and len(seen) >= testers:
            return r.answered_at
    return None


def stop_rule(cohort: Cohort, config: dict) -> dict:
    """Where the cohort stands against the stop rule, and the two moments the
    report is computed at: the readout's (the 100th primary rating) and the
    look's (the first moment the stop rule held)."""
    rule = config["stop_rule"]
    enabled = [
        _int(e.meta, "rating_prompt_enabled_at") for e in cohort.exports.values()
        if _int(e.meta, "rating_prompt_enabled_at") is not None
    ]
    exported = [
        _int(e.meta, "exported_at") for e in cohort.exports.values()
        if _int(e.meta, "exported_at") is not None
    ]
    first = min(enabled) if enabled else None
    as_of = max(exported) if exported else None
    weeks = (as_of - first) / WEEK_SECONDS if first is not None and as_of is not None else None
    n = len(cohort.primary)
    testers = len({r.person for r in cohort.primary})
    count_point = first_reached(cohort.primary, rule["primary_ratings"], rule["testers"])
    time_point = first + rule["weeks_after_first_enablement"] * WEEK_SECONDS if first is not None else None
    by_count = count_point is not None
    by_time = time_point is not None and as_of is not None and as_of >= time_point
    points = ([count_point] if by_count else []) + ([time_point] if by_time else [])
    look_cutoff = min(points) if points else None
    met_by = None
    if look_cutoff is not None:
        met_by = " and ".join(
            label for label, point in (("count", count_point if by_count else None),
                                       ("time", time_point if by_time else None))
            if point == look_cutoff
        )
    readout_cutoff = first_reached(cohort.primary, rule["descriptive_minimum_ratings"])
    readout_at_the_look = look_cutoff is not None and (
        readout_cutoff is None or look_cutoff <= readout_cutoff
    )
    return {
        "primary_ratings": n,
        "testers": testers,
        "first_enabled_at": first,
        "as_of_exported_at": as_of,
        "weeks_since_first_enabled": weeks,
        "met_by_count": by_count,
        "met_by_time": by_time,
        "met": look_cutoff is not None,
        "met_by": met_by,
        "count_met_at": count_point,
        "time_point": time_point,
        "look_cutoff": look_cutoff,
        "rule": (
            f"one look, at {rule['primary_ratings']} primary ratings from at least "
            f"{rule['testers']} testers, or {rule['weeks_after_first_enablement']} weeks after "
            f"the first graduate enabled the question, whichever comes first, computed as of "
            f"the moment it was first met"
        ),
        "descriptive_minimum_ratings": rule["descriptive_minimum_ratings"],
        "h2_minimum_ratings": rule["h2_minimum_ratings"],
        "readout_cutoff": readout_cutoff,
        "readout_due": readout_cutoff is not None and not readout_at_the_look,
        "readout_at_the_look": readout_at_the_look,
    }


def exports_before(cohort: Cohort, cutoff: int) -> list:
    """Exports taken before `cutoff`. Rows of theirs between their exported_at
    and the cutoff are not in them, so a later export could add some."""
    return sorted(
        name for name, export in cohort.exports.items()
        if (_int(export.meta, "exported_at") or 0) < cutoff
    )


# ---------------------------------------------------------------------------
# H1 and H2
# ---------------------------------------------------------------------------


def h1(rows: list, config: dict, replicates: int) -> dict:
    spec = config["h1"]
    seed = config["bootstrap"]["seed"]
    mostly_on = set(config["mostly_on"])

    def rho(subset: list) -> float | None:
        return spearman([r.features.work_anchor_share for r in subset], [float(r.code) for r in subset])

    def area(subset: list) -> float | None:
        return auroc([r.features.work_anchor_share for r in subset], [r.answer in mostly_on for r in subset])

    groups = _by_person(rows)
    boot, undefined = person_bootstrap(groups, rho, replicates, _stream(seed, "h1_rho"))
    estimate = rho(rows)
    lower = quantile(boot, spec["one_sided_lower_quantile"]) if boot else None
    passed = estimate is not None and lower is not None and lower > 0.0

    area_boot, area_undefined = person_bootstrap(groups, area, replicates, _stream(seed, "h1_auroc"))
    low, high = spec["two_sided_interval"]

    minimum = spec["within_person_minimum_ratings"]
    centred_groups = []
    for group in groups:
        if len(group) < minimum:
            continue
        mx = sum(r.features.work_anchor_share for r in group) / len(group)
        my = sum(r.code for r in group) / len(group)
        centred_groups.append([(r.features.work_anchor_share - mx, r.code - my) for r in group])

    def within(pairs: list) -> float | None:
        return spearman([a for a, _ in pairs], [b for _, b in pairs])

    pooled = [pair for group in centred_groups for pair in group]
    within_boot, within_undefined = person_bootstrap(
        centred_groups, within, replicates, _stream(seed, "h1_within")
    )
    return {
        "n": len(rows),
        "people": len(groups),
        "rho": estimate,
        "one_sided_lower_bound_95": lower,
        "passed": passed,
        "bootstrap_replicates": replicates,
        "bootstrap_undefined": undefined,
        "auroc_mostly_on": area(rows),
        "auroc_interval_95": _interval(area_boot, low, high),
        "auroc_bootstrap_undefined": area_undefined,
        "within_person_rho": within(pooled),
        "within_person_interval_95": _interval(within_boot, low, high),
        "within_person_people": len(centred_groups),
        "within_person_bootstrap_undefined": within_undefined,
        "rule": "Spearman rho between work_anchor_share and the rating coded 0-4; passes if the "
                "5th percentile of the person-clustered bootstrap is above 0.",
    }


def _design(rows: list, names: list, floor: float) -> list:
    return [[_model_value(r.features, name, floor) for name in names] for r in rows]


def h2(rows: list, config: dict, replicates: int) -> dict:
    spec = config["h2"]
    levels = len(config["answer_levels"])
    mostly_on_codes = {config["answer_levels"].index(a) for a in config["mostly_on"]}
    floor = spec["longest_run_share_floor"]
    fit_args = {
        "ridge": spec["ridge_lambda"],
        "grid": spec["person_intercept_sd_grid"],
        "pseudo_weight": spec["pseudo_weight_per_level"],
    }
    people = sorted({r.person for r in rows})
    held_out = []
    failures = []
    for person in people:
        train = [r for r in rows if r.person != person]
        test = [r for r in rows if r.person == person]
        y_train = [r.code for r in train]
        groups = _group_indices([r.person for r in train])
        fold = {}
        for label, names in (("model", spec["features"]), ("baseline", spec["baseline_features"])):
            raw = _design(train, names, floor)
            scale = standardizer(raw)
            X = [standardize(row, scale) for row in raw]
            fit, _ = fit_ordinal_tau(X, y_train, groups, levels, **fit_args)
            if fit is None:
                failures.append(f"{label} fit did not converge without {person}")
                break
            fold[label] = (fit, scale, names)
        if len(fold) < 2:
            continue
        for r in test:
            entry = {"person": person, "code": r.code}
            for label, (fit, scale, names) in fold.items():
                x = standardize(_design([r], names, floor)[0], scale)
                eta = fit.eta(x)
                probabilities = level_probabilities(fit.thresholds, eta)
                entry[f"{label}_loss"] = -math.log(max(probabilities[r.code], 1e-300))
                entry[f"{label}_eta"] = eta
                entry[f"{label}_mostly_on"] = sum(probabilities[k] for k in mostly_on_codes)
            held_out.append(entry)
    if failures or not held_out:
        return {"reported": False, "failures": failures, "passed": False}

    for entry in held_out:
        entry["improvement"] = entry["baseline_loss"] - entry["model_loss"]
    groups = _by_person_entries(held_out)

    def mean_improvement(entries: list) -> float | None:
        return _mean([e["improvement"] for e in entries])

    boot, undefined = person_bootstrap(
        groups, mean_improvement, replicates, _stream(config["bootstrap"]["seed"], "h2_improvement")
    )
    low, high = spec["improvement_interval"]
    interval = _interval(boot, low, high)

    etas = [e["model_eta"] for e in held_out]
    slope = None
    if len(set(etas)) > 1:
        calibration = fit_ordinal(
            [[v] for v in etas], [e["code"] for e in held_out], None, levels,
            ridge=0.0, tau=0.0, pseudo_weight=spec["pseudo_weight_per_level"],
        )
        slope = calibration.beta[0] if calibration.converged else None
    slope_low, slope_high = spec["calibration_slope_range"]
    passed = (
        interval is not None and interval[0] > 0.0
        and slope is not None and slope_low <= slope <= slope_high
    )

    labels = [e["code"] in mostly_on_codes for e in held_out]
    reliability = _reliability(
        [e["model_mostly_on"] for e in held_out], labels,
        spec["reliability_bins_max"], spec["reliability_bin_minimum_rows"],
    )

    full = all_rows_fit(rows, config)
    return {
        "reported": True,
        "n": len(held_out),
        "people": len(people),
        "mean_log_loss_model": _mean([e["model_loss"] for e in held_out]),
        "mean_log_loss_baseline": _mean([e["baseline_loss"] for e in held_out]),
        "mean_improvement": mean_improvement(held_out),
        "improvement_interval_95": interval,
        "bootstrap_replicates": replicates,
        "bootstrap_undefined": undefined,
        "calibration_slope": slope,
        "calibration_slope_range": [slope_low, slope_high],
        "passed": passed,
        "held_out_auroc_mostly_on_model": auroc([e["model_eta"] for e in held_out], labels),
        "held_out_auroc_mostly_on_baseline": auroc([e["baseline_eta"] for e in held_out], labels),
        "reliability": reliability,
        "all_rows_fit": {
            "features": spec["features"],
            "standardized_slopes": full.beta if full else None,
            "thresholds": full.thresholds if full else None,
            "tau": full.tau if full else None,
            "note": "Fitted once on every primary rating, for description. The test above is "
                    "the held-out comparison, not this fit.",
        },
        "rule": "Leave-one-person-out log loss of the four-feature model against "
                "work_anchor_share alone; passes if the 95% person-bootstrap interval on the "
                "mean improvement lies above 0 and the calibration slope lies in "
                f"[{slope_low}, {slope_high}].",
    }


def all_rows_fit(rows: list, config: dict, *, grid: list | None = None) -> OrdinalFit | None:
    """The four-feature model fitted once on every row, for description only.

    `grid` replaces the tau grid; the families check passes [0.0] to show what
    the fit does without the person intercept.
    """
    spec = config["h2"]
    raw = _design(rows, spec["features"], spec["longest_run_share_floor"])
    scale = standardizer(raw)
    fit, _ = fit_ordinal_tau(
        [standardize(row, scale) for row in raw], [r.code for r in rows],
        _group_indices([r.person for r in rows]), len(config["answer_levels"]),
        ridge=spec["ridge_lambda"],
        grid=spec["person_intercept_sd_grid"] if grid is None else grid,
        pseudo_weight=spec["pseudo_weight_per_level"],
    )
    return fit


def _by_person_entries(entries: list) -> list:
    groups: dict = {}
    for e in entries:
        groups.setdefault(e["person"], []).append(e)
    return [groups[name] for name in sorted(groups)]


def _reliability(probabilities: list, labels: list, bins_max: int, minimum: int) -> list:
    n = len(probabilities)
    bins = max(1, min(bins_max, n // minimum)) if n else 0
    order = sorted(range(n), key=lambda i: (probabilities[i], i))
    result = []
    for b in range(bins):
        members = order[b * n // bins:(b + 1) * n // bins]
        result.append({
            "rows": len(members),
            "mean_held_out_probability": _mean([probabilities[i] for i in members]),
            "observed_share": _mean([1.0 if labels[i] else 0.0 for i in members]),
        })
    return result


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


def readout(cohort: Cohort, config: dict, replicates: int) -> dict:
    """The descriptive readout, items 1 to 6 of the module docstring, on the
    cohort as given (restricted to a cutoff by the caller)."""
    result = {
        "cutoff": cohort.cutoff,
        "exports_taken_before_the_cutoff": (
            exports_before(cohort, cohort.cutoff) if cohort.cutoff is not None else []
        ),
        "distribution": {
            "primary": rating_distribution(cohort.primary, config),
            "late": rating_distribution(cohort.late, config),
            "descriptive_purposes": rating_distribution(cohort.descriptive_purpose, config),
        },
        "card": card_table(cohort, config, replicates),
        "person_variation": person_variation(cohort.primary, config, replicates),
        "answer_rate": answer_rate(cohort, config),
        "offers": offer_replies(cohort.primary, config),
        "sustained_anchor": sustained_anchor(cohort, cohort.primary, config),
        "reactivity": reactivity(cohort, config),
    }
    return result


def analyse(
    cohort: Cohort,
    config: dict,
    *,
    take_readout: bool = False,
    final_look: bool = False,
    ignore_stop_rule: bool = False,
    bootstrap_replicates: int | None = None,
    skip_h2: bool = False,
    provenance_record: dict | None = None,
) -> dict:
    if take_readout and final_look:
        raise ConfigError("--readout and --final-look are separate runs")
    if (ignore_stop_rule or bootstrap_replicates is not None or skip_h2) and not cohort.synthetic:
        raise ConfigError("the stop rule, the bootstrap and H2 are fixed for real exports")
    replicates = bootstrap_replicates or config["bootstrap"]["replicates"]
    rule = stop_rule(cohort, config)
    result: dict = {
        "synthetic": cohort.synthetic,
        "provenance": provenance_record or {},
        "statement": AGREEMENT_STATEMENT,
        "mode": "final_look" if final_look else ("readout" if take_readout else "progress"),
        "exports": {
            "received": cohort.exports_received,
            "analysed": cohort.analysed,
            "people": dict(sorted(cohort.people.items())),
            "declared": cohort.declared,
            "excluded_whole": cohort.excluded_whole,
            "problems": dict(sorted(cohort.export_problems.items())),
            "founder_pipeline_check": cohort.founder_checks,
            "v6_period_starts": dict(sorted(cohort.period_starts.items())),
        },
        "rows": {
            "excluded": dict(sorted(cohort.excluded_rows.items())),
            "protocol_deviations": dict(sorted(cohort.deviations.items())),
            "malformed": cohort.malformed,
        },
        "progress": accounting(cohort),
        "stop_rule": rule,
    }
    minimum = config["stop_rule"]["descriptive_minimum_ratings"]
    if take_readout:
        if rule["readout_at_the_look"]:
            result["readout_refused"] = (
                "the stop rule was met before primary ratings reached "
                f"{minimum}; the descriptives are reported at the one look (--final-look)"
            )
        elif rule["readout_cutoff"] is None:
            result["readout_refused"] = (
                f"{len(cohort.primary)} primary ratings; the readout is taken once there are {minimum}"
            )
        else:
            result["readout"] = readout(restrict(cohort, rule["readout_cutoff"], config), config, replicates)
        return result
    if not final_look:
        return result
    met = rule["met"] or ignore_stop_rule
    result["final_look"] = {
        "stop_rule_met": rule["met"],
        "stop_rule_bypassed_for_synthetic_test": ignore_stop_rule and not rule["met"],
    }
    if not met:
        result["final_look"]["refused"] = "the stop rule is not met; the one look has not been taken"
        return result
    if rule["met"] and not ignore_stop_rule:
        look = restrict(cohort, rule["look_cutoff"], config)
    else:
        look = cohort
    result["final_look"]["cutoff"] = look.cutoff
    result["final_look"]["primary_ratings_at_the_cutoff"] = len(look.primary)
    result["final_look"]["testers_at_the_cutoff"] = len({r.person for r in look.primary})
    result["readout"] = readout(look, config, replicates)
    n = len(look.primary)
    h2_minimum = config["stop_rule"]["h2_minimum_ratings"]
    if n < minimum and not ignore_stop_rule:
        result["final_look"]["h1"] = f"not reported: {n} primary ratings, below {minimum}"
    else:
        result["final_look"]["h1"] = h1(look.primary, config, replicates)
    if skip_h2:
        result["final_look"]["h2"] = "skipped (synthetic run)"
    elif n < h2_minimum and not ignore_stop_rule:
        result["final_look"]["h2"] = f"not reported: {n} primary ratings, below {h2_minimum}"
    else:
        result["final_look"]["h2"] = h2(look.primary, config, replicates)
    return result


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def _number(value, digits: int = 3) -> str:
    if value is None:
        return "undefined"
    if isinstance(value, bool):
        return "yes" if value else "no"
    if isinstance(value, int):
        return str(value)
    return f"{value:.{digits}f}"


def _when(timestamp) -> str:
    """A Unix time as the export stores it, with its UTC date for the reader."""
    if timestamp is None:
        return "undefined"
    moment = datetime.datetime.fromtimestamp(timestamp, tz=datetime.timezone.utc)
    return f"{timestamp} ({moment:%Y-%m-%d %H:%M} UTC)"


def _pair(interval) -> str:
    if not interval:
        return "undefined"
    return f"[{_number(interval[0])}, {_number(interval[1])}]"


def check_wording(text: str, names: tuple = ()) -> None:
    """Refuses a report that uses a banned word. Participant names and paths are
    the reader's own, not the report's words, so they are masked first."""
    for name in sorted((n for n in names if n), key=len, reverse=True):
        text = text.replace(name, "<name>")
    for label, pattern in BANNED_REPORT_PATTERNS:
        match = pattern.search(text)
        if match:
            raise WordingError(f"the report would say {match.group(0)!r} ({label})")


def render(result: dict) -> str:
    out: list = []
    add = out.append

    def heading(title: str) -> None:
        add("")
        add(title)
        add("-" * 72)

    def para(text: str, indent: str = "  ") -> None:
        add(textwrap.fill(text, width=76, initial_indent=indent, subsequent_indent=indent))

    add("SELF-RATING ANALYSIS (drift policy v6 rows only)")
    add("=" * 72)
    para(result["statement"], "")
    provenance_record = result["provenance"]
    if provenance_record:
        add(f"analysis sha256: {provenance_record['analysis_sha256']}")
        add(f"config sha256:   {provenance_record['config_sha256']}")
        add(f"git commit:      {provenance_record['git_commit']} (uncommitted changes to "
            f"these files: {provenance_record['uncommitted_changes_to_these_files']})")
    add(f"mode: {result['mode']}")

    exports = result["exports"]
    heading("EXPORTS")
    add(f"  received: {len(exports['received'])}   analysed: {len(exports['analysed'])}")
    for name, reason in sorted(exports["excluded_whole"].items()):
        add(f"  excluded whole: {name}: {reason}")
    for name, counts in sorted(exports["founder_pipeline_check"].items()):
        add(f"  founder pipeline check (n = 1, founder; not a result): {name}: "
            + ", ".join(f"{k} {v}" for k, v in counts.items()))
    for problem, count in exports["problems"].items():
        add(f"  {problem}: {count} export(s)")
    declared = exports["declared"]
    add("  declared on the command line (from the tester registry, fixed before the look):")
    add("    one person, several Macs: " + ("; ".join(", ".join(g) for g in declared["same_person"])
                                            or "none"))
    add("    reinstalled: " + (", ".join(declared["reinstalled"]) or "none"))
    add("    founder devices: " + (", ".join(declared["founder_device"]) or "none"))
    if declared["not_among_the_exports"]:
        add("    declared but not among the exports: " + ", ".join(declared["not_among_the_exports"]))

    rows = result["rows"]
    heading("ROWS")
    if not rows["excluded"] and not rows["protocol_deviations"]:
        add("  no rating row excluded before eligibility")
    for reason, count in rows["excluded"].items():
        add(f"  excluded, {reason}: {count}")
    for reason, count in rows["protocol_deviations"].items():
        add(f"  PROTOCOL DEVIATION, {reason}: {count} (excluded)")
    for message in rows["malformed"][:20]:
        add(f"    {message}")
    if len(rows["malformed"]) > 20:
        add(f"    ... and {len(rows['malformed']) - 20} more")

    progress = result["progress"]
    heading("PROGRESS (status only)")
    add(f"  eligible rating rows: {progress['eligible']}, shown {progress['shown']}")
    for state, count in progress["by_state"].items():
        add(f"    {state:<22} {count}")
    add(f"  answered after seeing the card (late stratum, outside the primary): "
        f"{progress['late_answers']}")
    add(f"  answered, purpose healthy_tech_use or work_life_boundary (descriptive): "
        f"{progress['descriptive_purpose_answers']}")
    for reason, count in progress["primary_exclusions"].items():
        add(f"  answered, excluded from the primary, {reason}: {count}")
    add(f"  PRIMARY ratings: {progress['primary']} from {progress['primary_testers']} tester(s)")
    para(progress["statement"])

    rule = result["stop_rule"]
    heading("STOP RULE")
    para(rule["rule"] + ".")
    add(f"  primary ratings {rule['primary_ratings']}, testers {rule['testers']}, weeks since "
        f"the first enablement {_number(rule['weeks_since_first_enabled'], 1)}")
    add(f"  met: {_number(rule['met'])} (by count {_number(rule['met_by_count'])}, "
        f"by time {_number(rule['met_by_time'])})")
    if rule["met"]:
        add(f"  first met by {rule['met_by']} at {_when(rule['look_cutoff'])}: the look is "
            "computed as of then")

    data = result.get("readout")
    if data is None and "final_look" not in result:
        heading("DESCRIPTIVE READOUT")
        minimum = rule["descriptive_minimum_ratings"]
        if "readout_refused" in result:
            add(f"  REFUSED: {result['readout_refused']}")
        elif rule["readout_at_the_look"]:
            para(f"The stop rule was met before primary ratings reached {minimum}, so the "
                 "descriptives are reported at the one look.")
        elif rule["readout_due"]:
            para(f"Due: primary ratings reached {minimum} at {_when(rule['readout_cutoff'])}. Take it "
                 "once with --readout. It is computed as of that moment, so running it again "
                 "gives the same numbers. This run read no answer.")
        else:
            para(f"Not yet: it is taken once, with --readout, when there are {minimum} primary "
                 "ratings. This run read no answer.")
    if data is not None:
        heading("DESCRIPTIVE READOUT" if result["mode"] == "readout" else "DESCRIPTIVES AT THE LOOK")
        if data["cutoff"] is not None:
            add(f"  as of {_when(data['cutoff'])}: rows eligible, shown, answered, Not now or "
                "expired by then")
        else:
            add("  every row received (no cutoff: synthetic run with the stop rule bypassed)")
        if data["exports_taken_before_the_cutoff"]:
            add("  exports taken before the cutoff (rows after their export are missing): "
                + ", ".join(data["exports_taken_before_the_cutoff"]))
        heading("1. DOES THE BLOCK SUMMARY MATCH WHAT PEOPLE SAID? (card branch against tier)")
        card = data["card"]
        add(f"  rows: {card['population']}")
        add(f"  {'branch':<18}" + "".join(f"{t:>8}" for t in card["tier_order_low_to_high"]))
        for branch, cells in card["table"].items():
            add(f"  {branch:<18}" + "".join(f"{cells[t]:>8}" for t in card["tier_order_low_to_high"]))
        add(f"  weighted kappa on {card['kappa_rows']} primary ratings (linear, three ordered "
            f"branches): {_number(card['weighted_kappa'])} {_pair(card['weighted_kappa_interval_95'])}")
        para(card["note"])

        variation = data["person_variation"]
        heading("2. BETWEEN-PERSON VARIATION IN THE RATING")
        if variation["people"]:
            per = variation["ratings_per_person"]
            add(f"  people {variation['people']}, primary ratings per person {per['min']}-{per['max']} "
                f"(median {_number(per['median'], 1)})")
            add(f"  tau (logit scale): {_number(variation['tau'])} {_pair(variation['tau_interval_95'])}"
                + ("; at the edge of the grid" if variation["tau_at_grid_edge"] else ""))
            add(f"  ICC: {_number(variation['icc_latent'])} {_pair(variation['icc_interval_95'])}")
        else:
            add("  no primary rating")

        rates = data["answer_rate"]
        heading("3. IS A RATING MISSING AT RANDOM? (answered against unanswered eligible blocks)")
        overall = rates["overall"]
        add(f"  answer rate: {overall['answered']}/{overall['eligible']} = {_number(overall['rate'])} "
            f"of eligible; {_number(rates['rate_when_shown'])} of shown")
        add(f"  median seconds from shown to answer: {_number(rates['median_seconds_shown_to_answer'], 0)}")
        for person, cell in rates["by_person"].items():
            add(f"    {person}: {cell['answered']}/{cell['eligible']} = {_number(cell['rate'])}")
        add("  medians, answered against not answered:")
        for name, cell in rates["feature_medians"].items():
            add(f"    {name:<30} {_number(cell['answered'])} against {_number(cell['not_answered'])}")
        offer = rates["offer_delivered_share"]
        add(f"    {'offer delivered (share)':<30} {_number(offer['answered'])} against "
            f"{_number(offer['not_answered'])}")
        add("  exploratory, answer rate by:")
        explore = rates["exploratory"]
        for label, key in (("coverage class", "by_coverage_class"), ("card branch", "by_card_branch")):
            for value, cell in explore[key].items():
                add(f"    {label} {value:<16} {cell['answered']}/{cell['eligible']} = "
                    f"{_number(cell['rate'])}")
        for cell in explore["by_work_anchor_share_group"]:
            low, high = cell["lower_exclusive"], cell["upper_inclusive"]
            if low is None:
                span = f"at or below {_number(high)}" if high is not None else "any value"
            elif high is None:
                span = f"above {_number(low)}"
            else:
                span = f"above {_number(low)}, at or below {_number(high)}"
            add(f"    work_anchor_share {span}: {cell['answered']}/{cell['eligible']} = "
                f"{_number(cell['rate'])}")

        heading("4. THE RATING AGAINST OFFER REPLIES AND THE SUSTAINED-ANCHOR OUTCOME")
        offers = data["offers"]
        add(f"  primary blocks with a delivered offer: {offers['offered_blocks']}")
        for group, cells in offers["by_reply"].items():
            add(f"    {group:<14}" + ", ".join(f"{t} {cells[t]}" for t in TIER_ORDER))
        sustained = data["sustained_anchor"]
        add(f"  exports with outcome labels: {sustained['exports_with_labels']}; without "
            f"(not available, never zero): {len(sustained['exports_without_labels'])}")
        if sustained["exports_with_labels"]:
            add(f"  primary blocks with a measured eligible decision point (offered, block at "
                f"least {WARMUP_EXCLUSION_SECONDS} s): {sustained['blocks_with_a_measured_eligible_point']}")
            for tier, cell in sustained["by_tier"].items():
                add(f"    {tier:<6} blocks {cell['blocks']}, share sustained: mean "
                    f"{_number(cell['mean_share'])}, median {_number(cell['median_share'])}")

        react = data["reactivity"]
        heading(f"5. REACTIVITY PROXY (blocks declared {react['window_days']} days before and "
                "after enabling, per person)")
        for person in react["people"]:
            if person["status"] == "measured":
                add(f"  {person['person']}: before {person['before']}, after {person['after']}"
                    + (f" (Macs {', '.join(person['macs'])})" if len(person["macs"]) > 1 else ""))
            else:
                add(f"  {person['person']}: {person['status']}")
        add(f"  median change: {_number(react['median_change'], 1)}")
        para(react["note"])

        distribution = data["distribution"]
        heading("6. RATINGS, AND THE STRATA DESCRIBED APART")
        for label, key in (("primary", "primary"), ("late stratum", "late"),
                           ("descriptive purposes", "descriptive_purposes")):
            cell = distribution[key]
            add(f"  {label}: n {cell['n']}; "
                + ", ".join(f"{k} {v}" for k, v in cell["by_answer"].items())
                + "; tiers " + ", ".join(f"{k} {v}" for k, v in cell["by_tier"].items()))

    look = result.get("final_look")
    if look is not None:
        heading("THE ONE LOOK")
        if look.get("stop_rule_bypassed_for_synthetic_test"):
            add("  stop rule bypassed for a synthetic contract test")
        if "refused" in look:
            add(f"  REFUSED: {look['refused']}")
        else:
            if look["cutoff"] is not None:
                add(f"  as of {_when(look['cutoff'])}: {look['primary_ratings_at_the_cutoff']} primary "
                    f"ratings from {look['testers_at_the_cutoff']} tester(s)")
            first = look["h1"]
            if isinstance(first, str):
                add(f"  H1: {first}")
            else:
                add(f"  H1 (primary): rho {_number(first['rho'])}, one-sided 95% lower bound "
                    f"{_number(first['one_sided_lower_bound_95'])}: "
                    + ("PASSES" if first["passed"] else "does not pass"))
                add(f"    n {first['n']} from {first['people']} people; bootstrap "
                    f"{first['bootstrap_replicates']} draws, {first['bootstrap_undefined']} undefined")
                add(f"    AUROC of work_anchor_share for mostly on: {_number(first['auroc_mostly_on'])} "
                    f"{_pair(first['auroc_interval_95'])}")
                add(f"    within-person rho ({first['within_person_people']} people): "
                    f"{_number(first['within_person_rho'])} {_pair(first['within_person_interval_95'])}")
                para(first["rule"], "    ")
            second = look["h2"]
            if isinstance(second, str):
                add(f"  H2: {second}")
            elif not second.get("reported"):
                add("  H2: not reported: " + "; ".join(second.get("failures", [])))
            else:
                add(f"  H2 (secondary): mean held-out improvement in log loss "
                    f"{_number(second['mean_improvement'], 4)} {_pair(second['improvement_interval_95'])}, "
                    f"calibration slope {_number(second['calibration_slope'])}: "
                    + ("PASSES" if second["passed"] else "does not pass"))
                add(f"    held-out log loss: four features {_number(second['mean_log_loss_model'], 4)}, "
                    f"work_anchor_share alone {_number(second['mean_log_loss_baseline'], 4)}")
                add(f"    held-out AUROC for mostly on: four features "
                    f"{_number(second['held_out_auroc_mostly_on_model'])}, work_anchor_share alone "
                    f"{_number(second['held_out_auroc_mostly_on_baseline'])}")
                add("    reliability (held-out probability of mostly on against the observed share):")
                for cell in second["reliability"]:
                    add(f"      rows {cell['rows']:>4}: {_number(cell['mean_held_out_probability'])} "
                        f"against {_number(cell['observed_share'])}")
                fit = second["all_rows_fit"]
                if fit["standardized_slopes"] is not None:
                    add("    all-rows fit, standardized slopes: " + ", ".join(
                        f"{name} {_number(value)}" for name, value in zip(fit["features"], fit["standardized_slopes"])
                    ) + f"; tau {_number(fit['tau'])}")
                para(second["rule"], "    ")

    text = "\n".join(out) + "\n"
    if result["synthetic"]:
        text = "\n".join(f"[{SYNTHETIC_STAMP}] {line}" for line in text.splitlines()) + "\n"
    check_wording(text, _reader_names(result))
    return text


def _reader_names(result: dict) -> tuple:
    """Names the reader chose (folders, files, the config path), which the
    wording guard masks: they are not the report's own words."""
    exports = result["exports"]
    declared = exports.get("declared", {})
    return (
        tuple(exports["received"]) + tuple(exports["people"].values())
        + tuple(declared.get("not_among_the_exports", ()))
        + (result["provenance"].get("config_path", ""),)
    )


def render_json(result: dict) -> str:
    """The result as JSON, under the same wording guard as the text report."""
    text = json.dumps(result, indent=2)
    check_wording(text, _reader_names(result))
    return text


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("exports", nargs="*", type=Path,
                        help="one folder per participant, or the exported CSV files themselves")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of a report")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--readout", action="store_true",
                       help="take the descriptive readout, once, as of the 100th primary rating "
                            "(refused below 100)")
    modes.add_argument("--final-look", action="store_true",
                       help="take the one pre-registered look (refused until the stop rule is met)")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG_PATH,
                        help="the frozen config (default: the one beside this script)")
    parser.add_argument("--founder-device", action="append", default=[], metavar="NAME",
                        help="drop this export whole (pipeline check only); repeatable")
    parser.add_argument("--reinstalled", action="append", default=[], metavar="NAME",
                        help="drop this participant whole: reinstalled mid-cohort; repeatable")
    parser.add_argument("--same-person", action="append", default=[], metavar="A,B",
                        help="these exports are one tester's Macs: one person in every "
                             "bootstrap, fold and count of testers; repeatable")
    parser.add_argument("--provenance", action="store_true",
                        help="print the SHA-256 of this file and the config, and the commit, then exit")
    parser.add_argument("--synthetic", action="store_true",
                        help="inputs are SYNTHETIC contract-test exports (see the docstring)")
    parser.add_argument("--ignore-stop-rule", action="store_true",
                        help="synthetic only: take the look whatever the stop rule says")
    parser.add_argument("--bootstrap-replicates", type=int, default=None, metavar="N",
                        help="synthetic only: fewer bootstrap draws than the config's")
    args = parser.parse_args()

    try:
        config = load_config(args.config)
    except ConfigError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    record = provenance(args.config)
    if args.provenance:
        print(json.dumps(record, indent=2))
        return 0
    if not args.exports:
        parser.error("no exports given")
    if (args.ignore_stop_rule or args.bootstrap_replicates is not None) and not args.synthetic:
        print("ERROR: --ignore-stop-rule and --bootstrap-replicates are for --synthetic runs only",
              file=sys.stderr)
        return 2
    missing = [str(p) for p in args.exports if not p.exists()]
    if missing:
        print(f"ERROR: no such file: {', '.join(missing)}", file=sys.stderr)
        return 1
    try:
        cohort = load(args.exports, config, founder_devices=tuple(args.founder_device),
                      reinstalled=tuple(args.reinstalled), same_person=tuple(args.same_person),
                      synthetic=args.synthetic)
        result = analyse(
            cohort, config, take_readout=args.readout, final_look=args.final_look,
            ignore_stop_rule=args.ignore_stop_rule,
            bootstrap_replicates=args.bootstrap_replicates, provenance_record=record,
        )
        text = render_json(result) if args.json else render(result)
    except (ConfigError, WordingError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    print(text, end="" if text.endswith("\n") else "\n")
    if args.final_look and "refused" in result.get("final_look", {}):
        return 3
    if args.readout and "readout_refused" in result:
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
