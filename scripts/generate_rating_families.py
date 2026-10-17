#!/usr/bin/env python3
"""Generates SYNTHETIC self-rating exports with known truth, and checks
`analyze_ratings.py` against them.

Everything this writes is synthetic. No real person has touched it. The word
SYNTHETIC is in every participant name and block id, and every meta file says
`synthetic,1`, which `analyze_ratings.py` requires before it will read the
file and stamps on every line it prints. These are contract tests of the
analysis code, not evidence about anyone (FIX-1, 10-BUNDLE-ABSORPTION.md).

What the families are for (plan § 5.6)
--------------------------------------
Each family is a JSON spec in `scripts/tests/fixtures/rating-families/`. A
spec fixes the generating process, the number of people and blocks, a seed,
and what the analysis must do on it. The four the plan names:

  NULL     ratings independent of behaviour, with a person offset in the
           ratings and person-level differences in the features. H1 must not
           pass at the planned n beyond its nominal rate, and the held-out
           AUROC must come out near 0.5. The same spec carries the leakage
           trap: scoring the same rows with each person's own fitted intercept
           (what a fold that leaked the held-out person would do) gives an
           AUROC well above 0.5, so a NULL that stays at 0.5 is evidence the
           folds do not leak, rather than evidence the check cannot fail.
  PLANTED  ratings from the H2 model itself with known slopes. At the
           planned n, H1 must pass; at a large n, the fitted slopes and tau
           must land near the planted ones.
  MNAR     the same planted process, but people skip their worst blocks. It
           shows how far the answered rows bias rho against the rows nobody
           answered (the generator knows those answers), and checks that the
           readout's answer-rate table flags the skew.
  SCALE    people use the scale differently: large person offsets. The person
           intercept must absorb them (slopes recovered, tau recovered), and
           the same fit without the intercept must give visibly smaller
           slopes, so the intercept is shown to be doing the work.

The generating process
----------------------
Each block has a latent on-task share s (a logistic-normal draw around the
person's own mean). The features the export would carry are noisy functions
of s: the anchor share is s plus noise, inflated in some blocks by time away
from the desk with the work app in front (the blind spot in plan § 2);
switch-aways per covered hour fall with s; the longest run grows with s; an
offer is likelier when s is low. Coverage, unlogged share, purpose and the
card branch follow the export's own rules. The rating is drawn from one of:

  independent  a person offset and logistic noise, nothing else;
  planted      the proportional-odds model on the four standardized features
               with the spec's slopes, plus a person offset.

What this does NOT do. Plan § 5.6 extends the #72 workspace generator, whose
traces replay through the real `WorkBlockManager`. That generator is not on
develop (#72 is held), and export format 7 does not exist yet (PR-5). So these
families draw block features directly, in the shape the format-7 export will
have, and they test the analysis contract only. The replay extension, and the
parity vectors that pin the exporter's feature definitions, belong to PR-5.

Held-out fixtures (FIX-1)
-------------------------
At least 25% of the fixtures must be written by someone who did not write the
analysis. They go in `rating-families/held-out/`, with `"held_out": true` and
the author's name. `--check --require-held-out` fails until they exist; run it
before the code is hashed.

Usage:
    ./scripts/generate_rating_families.py --check
    ./scripts/generate_rating_families.py --check --require-held-out
    ./scripts/generate_rating_families.py --check --family NULL
    ./scripts/generate_rating_families.py --out /tmp/families    # replicate 0 as CSV exports

Must run on the python3 that ships with macOS (3.9), standard library only.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import analyze_ratings as ar  # noqa: E402

GENERATOR_VERSION = 1
FAMILY_DIR = Path(__file__).resolve().parent / "tests" / "fixtures" / "rating-families"
HELD_OUT_DIR = FAMILY_DIR / "held-out"
HELD_OUT_SHARE_REQUIRED = 0.25

FAMILIES = ("NULL", "PLANTED", "MNAR", "SCALE")
RATING_MODES = ("independent", "planted")
ANSWERING_MODES = ("random", "by_level")

# Every key a spec may carry. A spec with a key this does not know is refused,
# so a typo cannot silently turn an expectation off.
SPEC_KEYS = {
    "family", "held_out", "author", "description", "seed", "replicates", "people",
    "blocks_per_person", "latent", "rating", "answering", "check", "expect",
}
EXPECT_KEYS = {
    "h1_pass_share_at_most",
    "h1_pass_share_at_least",
    "held_out_auroc_between",
    "h2_pass_share_at_least",
    "in_sample_auroc_with_own_intercepts_at_least",
    "slopes_within",
    "tau_between",
    "slope_smaller_without_person_intercept",
    "rho_answered_minus_all_at_most",
    "answered_minus_unanswered_median_anchor_share_at_least",
}

BASE_TIME = 1_790_000_000
SYNTHETIC_TAG = "SYNTHETIC"


class SpecError(Exception):
    pass


# ---------------------------------------------------------------------------
# Specs
# ---------------------------------------------------------------------------


def load_spec(path: Path) -> dict:
    try:
        spec = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SpecError(f"{path.name}: {error}") from error
    unknown = set(spec) - SPEC_KEYS
    missing = SPEC_KEYS - set(spec) - {"description"}
    if unknown or missing:
        raise SpecError(f"{path.name}: unknown keys {sorted(unknown)}, missing keys {sorted(missing)}")
    if spec["family"] not in FAMILIES:
        raise SpecError(f"{path.name}: family must be one of {FAMILIES}")
    if spec["rating"].get("mode") not in RATING_MODES:
        raise SpecError(f"{path.name}: rating.mode must be one of {RATING_MODES}")
    if spec["answering"].get("mode") not in ANSWERING_MODES:
        raise SpecError(f"{path.name}: answering.mode must be one of {ANSWERING_MODES}")
    unknown_expect = set(spec["expect"]) - EXPECT_KEYS
    if unknown_expect:
        raise SpecError(f"{path.name}: unknown expectations {sorted(unknown_expect)}")
    if not spec["expect"]:
        raise SpecError(f"{path.name}: a spec with no expectation tests nothing")
    if bool(spec["held_out"]) != (path.parent.name == HELD_OUT_DIR.name):
        raise SpecError(f"{path.name}: held_out must be true exactly for specs in held-out/")
    spec["_path"] = path
    return spec


def load_specs(include_held_out: bool = True) -> list:
    paths = sorted(FAMILY_DIR.glob("*.json"))
    if include_held_out and HELD_OUT_DIR.is_dir():
        paths += sorted(HELD_OUT_DIR.glob("*.json"))
    return [load_spec(path) for path in paths]


# ---------------------------------------------------------------------------
# Generation
# ---------------------------------------------------------------------------


def _clamp(value: float, low: float = 0.0, high: float = 1.0) -> float:
    return min(high, max(low, value))


def _logistic(rng: random.Random) -> float:
    u = rng.random()
    while u <= 0.0:
        u = rng.random()
    return math.log(u / (1.0 - u))


def _poisson(rng: random.Random, rate: float) -> int:
    limit = math.exp(-rate)
    count, product = 0, rng.random()
    while product > limit:
        count += 1
        product *= rng.random()
    return count


def _weighted(rng: random.Random, options: list) -> str:
    total = sum(weight for _, weight in options)
    pick = rng.random() * total
    for value, weight in options:
        pick -= weight
        if pick <= 0:
            return value
    return options[-1][0]


def _block(rng: random.Random, s: float) -> dict:
    """One block's export features, drawn from its latent on-task share s."""
    elapsed = int(300 + (rng.random() ** 1.5) * 5100)
    if rng.random() < 0.08:
        coverage_ratio = rng.uniform(0.05, 0.24)
    else:
        coverage_ratio = rng.uniform(0.5, 1.0)
    covered = max(1, int(coverage_ratio * elapsed))
    coverage_class = (
        "insufficient" if coverage_ratio < 0.25 else "partial" if coverage_ratio < 0.75 else "good"
    )
    away = rng.random() < 0.15
    anchor = _clamp(s + rng.gauss(0.0, 0.10) + (0.25 * (1.0 - s) if away else 0.0))
    hours = covered / 3600.0
    switches = _poisson(rng, (1.5 + 9.0 * (1.0 - s)) * hours)
    recoveries = sum(1 for _ in range(switches) if rng.random() < 0.35 + 0.55 * s)
    if switches == 0:
        longest = coverage_ratio * rng.uniform(0.9, 1.0)
    else:
        longest = coverage_ratio * (s ** 1.3) * rng.uniform(0.55, 1.0)
    unlogged = rng.uniform(0.31, 0.6) if rng.random() < 0.08 else rng.uniform(0.0, 0.25)
    offered = rng.random() < 0.1 + 0.5 * (1.0 - s)
    if offered:
        outcome = _weighted(rng, [
            ("was_focused", 0.1 + 0.5 * s), ("accepted_action", 0.3), ("returned", 0.3),
            ("no_response", 0.3), ("not_helpful", 0.05),
        ])
    else:
        outcome = ""
    if coverage_class == "insufficient":
        branch = "low_coverage"
    elif switches == 0:
        branch = "stayed"
    elif recoveries == 0:
        branch = "ended_elsewhere"
    else:
        branch = "came_back"
    return {
        "elapsed_seconds": elapsed,
        "covered_seconds": covered,
        "coverage_class": coverage_class,
        "card_branch": branch,
        "work_anchor_share": anchor,
        "switch_aways_per_covered_hour": switches / hours,
        "longest_run_share": _clamp(longest),
        "recovery_ratio": recoveries / max(switches, 1),
        "feed_share": _clamp((1.0 - s) * rng.uniform(0.0, 0.7)),
        "unlogged_share": unlogged,
        "offer_delivered": 1 if offered else 0,
        "offer_outcome": outcome,
        "purpose": _weighted(rng, [
            ("deep_work", 0.5), ("study", 0.2), ("creative_practice", 0.15),
            ("healthy_tech_use", 0.1), ("work_life_boundary", 0.05),
        ]),
        "intensity": _weighted(rng, [("light", 0.3), ("medium", 0.5), ("intense", 0.2)]),
        "planned_minutes": _weighted(rng, [("25", 0.4), ("45", 0.3), ("60", 0.2), ("90", 0.1)]),
        "origin": _weighted(rng, [("manual", 0.8), ("invitation", 0.2)]),
        "hour_bucket": _weighted(rng, [("morning", 0.4), ("afternoon", 0.4), ("evening", 0.2)]),
        "weekend": 1 if rng.random() < 0.2 else 0,
    }


def _would_be_primary(block: dict, config: dict) -> bool:
    population = config["population"]
    return (
        block["purpose"] not in population["descriptive_only_purposes"]
        and block["coverage_class"] not in population["excluded_coverage_classes"]
        and block["unlogged_share"] <= population["maximum_unlogged_share"]
    )


def _feature_vector(block: dict, config: dict) -> list:
    floor = config["h2"]["longest_run_share_floor"]
    return [
        block["work_anchor_share"],
        block["switch_aways_per_covered_hour"],
        math.log(max(block["longest_run_share"], floor)),
        float(block["offer_delivered"]),
    ]


def generate(spec: dict, replicate: int, config: dict) -> tuple[dict, dict]:
    """One replicate: (exports by participant name, the truth the analysis cannot see)."""
    seed = f"{spec['seed']}:{replicate}"
    rng = ar._stream(GENERATOR_VERSION, seed)
    family = spec["family"]
    latent = spec["latent"]
    rating = spec["rating"]
    answering = spec["answering"]
    sizes = spec["blocks_per_person"]
    levels = config["answer_levels"]

    people = []
    for j in range(spec["people"]):
        count = int(round(math.exp(math.log(sizes["median"]) + rng.gauss(0.0, sizes["log_sd"]))))
        people.append({
            "name": f"{SYNTHETIC_TAG}-{family}-r{replicate:03d}-p{j:02d}",
            "mean_logit": rng.gauss(latent["mean_logit"], latent["person_sd"]),
            "offset": rng.gauss(0.0, rating["person_offset_sd"]),
            "count": min(sizes["max"], max(sizes["min"], count)),
            "v6_start": BASE_TIME + int(rng.uniform(0, 21) * ar.DAY_SECONDS),
        })

    blocks = []
    for person in people:
        for i in range(person["count"]):
            s = 1.0 / (1.0 + math.exp(-(person["mean_logit"] + rng.gauss(0.0, latent["block_sd"]))))
            block = _block(rng, s)
            block["person"] = person
            block["index"] = i
            block["latent"] = s
            blocks.append(block)

    # The planted slopes are on features standardized over the blocks that
    # would reach the primary if answered, which is what the analysis
    # standardizes over when answering is at random.
    reference = [_feature_vector(b, config) for b in blocks if _would_be_primary(b, config)]
    scale = ar.standardizer(reference)
    slopes = [rating.get("slopes", {}).get(name, 0.0) for name in ar.MODEL_FEATURES]
    thresholds = rating["thresholds"]
    for block in blocks:
        eta = block["person"]["offset"]
        if rating["mode"] == "planted":
            z = ar.standardize(_feature_vector(block, config), scale)
            eta += sum(b * v for b, v in zip(slopes, z))
        noisy = eta + _logistic(rng)
        block["would_answer"] = levels[sum(1 for t in thresholds if t < noisy)]

    exports = {}
    for person in people:
        enabled = person["v6_start"] + config["eligibility"]["v6_primary_period_seconds"] + int(
            rng.uniform(1, 4) * ar.DAY_SECONDS
        )
        mine = [b for b in blocks if b["person"] is person]
        times = sorted(enabled + int(rng.uniform(0.0, 15.0 * ar.WEEK_SECONDS)) for _ in mine)
        ratings, features, declared = [], [], []
        for block, eligible_at in zip(mine, times):
            block_id = f"{SYNTHETIC_TAG}-{family}-r{replicate:03d}-{person['name'][-3:]}-b{block['index']:03d}"
            block["block_id"] = block_id
            if answering["mode"] == "random":
                answers = rng.random() < answering["answer_probability"]
            else:
                answers = rng.random() < answering["by_level"][block["would_answer"]]
            row = {
                "block_id": block_id, "prompt_version": "1", "eligible_at": str(eligible_at),
                "shown_at": "", "answered_at": "", "not_now_at": "", "expired_at": "",
                "card_seen_before_answer": "0", "answer": "",
            }
            if answers:
                row["shown_at"] = str(eligible_at + 30)
                row["answered_at"] = str(eligible_at + 45)
                row["answer"] = block["would_answer"]
                if rng.random() < answering["late_probability"]:
                    row["card_seen_before_answer"] = "1"
                    row["answered_at"] = str(eligible_at + 900)
            elif rng.random() < 0.75:
                row["shown_at"] = str(eligible_at + 30)
                if rng.random() < 0.5:
                    row["not_now_at"] = str(eligible_at + 40)
                row["expired_at"] = str(eligible_at + 7200)
            else:
                row["expired_at"] = str(eligible_at + 7200)
            block["answered"] = bool(row["answer"])
            ratings.append(row)
            features.append({
                "block_id": block_id,
                "block_feature_version": "1",
                "policy_version": str(config["analysed_policy_version"]),
                **{key: _cell(block[key]) for key in ar.BLOCK_FEATURE_COLUMNS
                   if key not in ("block_id", "block_feature_version", "policy_version")},
            })
            declared.append({"block_id": block_id, "started_at": str(eligible_at - block["elapsed_seconds"])})
        # The Mac's first block starts its v6 primary period (it never ran an
        # older build, so there is no upgrade cut); more blocks follow before
        # the question is turned on.
        declared.append({
            "block_id": f"{SYNTHETIC_TAG}-{family}-r{replicate:03d}-{person['name'][-3:]}-first",
            "started_at": str(person["v6_start"]),
        })
        for k in range(_poisson(rng, 6.0)):
            declared.append({
                "block_id": f"{SYNTHETIC_TAG}-{family}-r{replicate:03d}-{person['name'][-3:]}-pre{k:02d}",
                "started_at": str(enabled - int(rng.uniform(0, 14) * ar.DAY_SECONDS)),
            })
        exported_at = (times[-1] if times else enabled) + 3 * ar.DAY_SECONDS
        meta = {
            "export_format": "7",
            "exported_at": str(exported_at),
            "rating_prompt_version": "1",
            "rating_prompt_enabled_at": str(enabled),
            "v6_period_start": str(person["v6_start"]),
            "block_feature_version": "1",
            ar.SYNTHETIC_META_KEY: "1",
            "generator_version": str(GENERATOR_VERSION),
            "family": family,
        }
        exports[person["name"]] = ar.Export(
            name=person["name"], meta=meta, ratings=ratings, features=features, blocks=declared,
            decisions=[],
        )
    truth = {"slopes": slopes, "tau": rating["person_offset_sd"], "blocks": blocks}
    return exports, truth


def _cell(value) -> str:
    if isinstance(value, float):
        return repr(round(value, 6))
    return str(value)


def write_exports(exports: dict, folder: Path) -> None:
    for name, export in exports.items():
        target = folder / name
        target.mkdir(parents=True, exist_ok=True)
        _write_csv(target / f"{name}-ratings.csv", ar.RATINGS_COLUMNS, export.ratings)
        _write_csv(target / f"{name}-blockfeatures.csv", ar.BLOCK_FEATURE_COLUMNS, export.features)
        _write_csv(target / f"{name}-blocks.csv", ar.BLOCK_COLUMNS, export.blocks or [])
        _write_csv(target / f"{name}-decisions.csv", ar.DECISION_COLUMNS, export.decisions or [])
        _write_csv(target / f"{name}-meta.csv", ("key", "value"),
                   [{"key": k, "value": v} for k, v in export.meta.items()])


def _write_csv(path: Path, columns, rows: list) -> None:
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(columns), lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({c: row.get(c, "") for c in columns})


# ---------------------------------------------------------------------------
# Checking
# ---------------------------------------------------------------------------


def _cohort(exports: dict, config: dict) -> ar.Cohort:
    return ar.build_cohort(exports, config, synthetic=True)


def in_sample_auroc_with_own_intercepts(rows: list, config: dict) -> float | None:
    """What a leaking fold would do: score each row with its own person's fitted intercept."""
    fit = ar.all_rows_fit(rows, config)
    if fit is None:
        return None
    spec = config["h2"]
    raw = ar._design(rows, spec["features"], spec["longest_run_share_floor"])
    scale = ar.standardizer(raw)
    groups = ar._group_indices([r.person for r in rows])
    scores = [
        fit.eta(ar.standardize(x, scale)) + (fit.intercepts[g] if fit.intercepts else 0.0)
        for x, g in zip(raw, groups)
    ]
    return ar.auroc(scores, [r.answer in config["mostly_on"] for r in rows])


def measure(spec: dict, config: dict) -> dict:
    """Runs the analysis over the spec's replicates and returns what the expectations read."""
    expect = spec["expect"]
    check = spec["check"]
    replicates = spec["replicates"]
    boot = check["bootstrap_replicates"]
    metrics: dict = {}
    h1_passes, rho_gaps, anchor_gaps = [], [], []
    needs_h1 = any(k.startswith("h1_") for k in expect)
    needs_missing = "rho_answered_minus_all_at_most" in expect or \
        "answered_minus_unanswered_median_anchor_share_at_least" in expect
    for replicate in range(replicates):
        exports, truth = generate(spec, replicate, config)
        cohort = _cohort(exports, config)
        if needs_h1:
            result = ar.h1(cohort.primary, config, boot)
            h1_passes.append(result["passed"])
        if needs_missing:
            answered = ar.spearman([r.features.work_anchor_share for r in cohort.primary],
                                   [float(r.code) for r in cohort.primary])
            everyone = [b for b in truth["blocks"] if _would_be_primary(b, config)]
            levels = config["answer_levels"]
            complete = ar.spearman([b["work_anchor_share"] for b in everyone],
                                   [float(levels.index(b["would_answer"])) for b in everyone])
            rho_gaps.append(answered - complete)
            medians = ar.answer_rate(cohort, config)["feature_medians"]["work_anchor_share"]
            anchor_gaps.append(medians["difference"])
        if replicate == 0:
            metrics["primary_ratings_replicate_0"] = len(cohort.primary)
            metrics["people_replicate_0"] = len({r.participant for r in cohort.primary})
            first_cohort, first_truth = cohort, truth
    if h1_passes:
        metrics["h1_pass_share"] = sum(h1_passes) / len(h1_passes)
        metrics["h1_replicates"] = len(h1_passes)
    if rho_gaps:
        metrics["rho_answered_minus_all"] = sum(rho_gaps) / len(rho_gaps)
        metrics["answered_minus_unanswered_median_anchor_share"] = sum(anchor_gaps) / len(anchor_gaps)

    lopo = check.get("lopo_replicates", 0)
    if lopo:
        aurocs, passes = [], []
        for replicate in range(lopo):
            exports, _ = generate(spec, replicate, config)
            result = ar.h2(_cohort(exports, config).primary, config, boot)
            aurocs.append(result.get("held_out_auroc_mostly_on_model"))
            passes.append(bool(result.get("passed")))
        defined = [a for a in aurocs if a is not None]
        metrics["held_out_auroc"] = sum(defined) / len(defined) if defined else None
        metrics["h2_pass_share"] = sum(passes) / len(passes)
        metrics["lopo_replicates"] = lopo

    if "in_sample_auroc_with_own_intercepts_at_least" in expect:
        metrics["in_sample_auroc_with_own_intercepts"] = in_sample_auroc_with_own_intercepts(
            first_cohort.primary, config
        )
    if {"slopes_within", "tau_between", "slope_smaller_without_person_intercept"} & set(expect):
        fit = ar.all_rows_fit(first_cohort.primary, config)
        metrics["slopes"] = dict(zip(config["h2"]["features"], fit.beta)) if fit else None
        metrics["tau"] = fit.tau if fit else None
        metrics["planted_slopes"] = dict(zip(ar.MODEL_FEATURES, first_truth["slopes"]))
        if "slope_smaller_without_person_intercept" in expect:
            plain = ar.all_rows_fit(first_cohort.primary, config, grid=[0.0])
            metrics["slopes_without_person_intercept"] = (
                dict(zip(config["h2"]["features"], plain.beta)) if plain else None
            )
    return metrics


def evaluate(spec: dict, metrics: dict) -> list:
    """(passed, line) per expectation."""
    results = []
    expect = spec["expect"]

    def record(ok: bool, text: str) -> None:
        results.append((bool(ok), text))

    def number(value) -> str:
        return "undefined" if value is None else f"{value:.3f}"

    if "h1_pass_share_at_most" in expect:
        value, bound = metrics["h1_pass_share"], expect["h1_pass_share_at_most"]
        record(value <= bound, f"H1 passes in {number(value)} of {metrics['h1_replicates']} "
                               f"replicates; at most {bound}")
    if "h1_pass_share_at_least" in expect:
        value, bound = metrics["h1_pass_share"], expect["h1_pass_share_at_least"]
        record(value >= bound, f"H1 passes in {number(value)} of {metrics['h1_replicates']} "
                               f"replicates; at least {bound}")
    if "held_out_auroc_between" in expect:
        value = metrics["held_out_auroc"]
        low, high = expect["held_out_auroc_between"]
        record(value is not None and low <= value <= high,
               f"held-out AUROC for mostly on {number(value)} (mean of {metrics['lopo_replicates']}); "
               f"within [{low}, {high}]")
    if "h2_pass_share_at_least" in expect:
        value, bound = metrics["h2_pass_share"], expect["h2_pass_share_at_least"]
        record(value >= bound, f"H2 passes in {number(value)} of {metrics['lopo_replicates']}; at least {bound}")
    if "in_sample_auroc_with_own_intercepts_at_least" in expect:
        value = metrics["in_sample_auroc_with_own_intercepts"]
        bound = expect["in_sample_auroc_with_own_intercepts_at_least"]
        record(value is not None and value >= bound,
               f"leakage trap: scored with each person's own intercept, AUROC {number(value)}; "
               f"at least {bound} (the check can fail)")
    if "slopes_within" in expect:
        slopes = metrics["slopes"] or {}
        for name, (low, high) in sorted(expect["slopes_within"].items()):
            value = slopes.get(name)
            record(value is not None and low <= value <= high,
                   f"slope {name} {number(value)} (planted {metrics['planted_slopes'].get(name)}); "
                   f"within [{low}, {high}]")
    if "tau_between" in expect:
        value = metrics["tau"]
        low, high = expect["tau_between"]
        record(value is not None and low <= value <= high,
               f"tau {number(value)}; within [{low}, {high}]")
    if "slope_smaller_without_person_intercept" in expect:
        name = expect["slope_smaller_without_person_intercept"]
        with_value = (metrics["slopes"] or {}).get(name)
        without = (metrics["slopes_without_person_intercept"] or {}).get(name)
        record(with_value is not None and without is not None and abs(without) < abs(with_value),
               f"slope {name} without the person intercept {number(without)}, with it "
               f"{number(with_value)}; smaller without")
    if "rho_answered_minus_all_at_most" in expect:
        value, bound = metrics["rho_answered_minus_all"], expect["rho_answered_minus_all_at_most"]
        record(value <= bound, f"rho on answered rows minus rho on every would-be primary block "
                               f"{number(value)}; at most {bound} (the bias MNAR can cause)")
    if "answered_minus_unanswered_median_anchor_share_at_least" in expect:
        value = metrics["answered_minus_unanswered_median_anchor_share"]
        bound = expect["answered_minus_unanswered_median_anchor_share_at_least"]
        record(value >= bound, f"the readout's answered-minus-unanswered median anchor share "
                               f"{number(value)}; at least {bound} (the skew is visible)")
    return results


def held_out_status(specs: list) -> tuple[bool, str]:
    held_out = sum(1 for s in specs if s["held_out"])
    share = held_out / len(specs) if specs else 0.0
    met = share >= HELD_OUT_SHARE_REQUIRED
    return met, (f"FIX-1 held-out share: {held_out} of {len(specs)} specs = {share:.0%}; "
                 f"{HELD_OUT_SHARE_REQUIRED:.0%} required before the freeze: "
                 + ("met" if met else "NOT MET"))


def run_check(specs: list, config: dict, require_held_out: bool) -> int:
    met, status = held_out_status(specs)
    if require_held_out and not met:
        print(status)
        print("held-out fixtures are missing; nothing else was run", file=sys.stderr)
        return 1
    failures = 0
    for spec in specs:
        metrics = measure(spec, config)
        origin = f"held out, by {spec['author']}" if spec["held_out"] else f"by {spec['author']}"
        print(f"[{SYNTHETIC_TAG}] {spec['family']} ({spec['_path'].name}, {origin}): "
              f"replicate 0 has {metrics['primary_ratings_replicate_0']} primary ratings from "
              f"{metrics['people_replicate_0']} people")
        for ok, line in evaluate(spec, metrics):
            print(f"  {'ok  ' if ok else 'FAIL'} {line}")
            failures += 0 if ok else 1
    print(status)
    if failures:
        print(f"{failures} check(s) FAILED", file=sys.stderr)
        return 1
    print("all family checks passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--check", action="store_true", help="run every spec and its expectations")
    parser.add_argument("--require-held-out", action="store_true",
                        help="also fail unless at least 25%% of the specs are held out (FIX-1)")
    parser.add_argument("--family", choices=FAMILIES, default=None, help="only this family")
    parser.add_argument("--out", type=Path, default=None,
                        help="write replicate 0 of each spec as CSV exports under this folder")
    parser.add_argument("--config", type=Path, default=ar.DEFAULT_CONFIG_PATH)
    args = parser.parse_args()
    if not args.check and args.out is None:
        parser.error("nothing to do: pass --check and/or --out")
    try:
        config = ar.load_config(args.config)
        specs = load_specs()
    except (ar.ConfigError, SpecError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    if args.check and args.require_held_out:
        met, status = held_out_status(specs)
        if not met:
            print(status)
            print("held-out fixtures are missing; nothing else was run", file=sys.stderr)
            return 1
    if args.family:
        specs = [s for s in specs if s["family"] == args.family]
    if args.out is not None:
        for spec in specs:
            exports, _ = generate(spec, 0, config)
            write_exports(exports, args.out / spec["_path"].stem)
        print(f"wrote replicate 0 of {len(specs)} spec(s) under {args.out}")
    if args.check:
        return run_check(specs, config, args.require_held_out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
