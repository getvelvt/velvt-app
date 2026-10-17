#!/usr/bin/env python3
"""Computes the pre-registered cohort outcomes from tester exports.

Input is whatever `export_cohort_evidence.sh` wrote on each tester's Mac: a
per-offer CSV and, beside it, the companion files that share its name stem
(`-decisions.csv`, `-outcomes.csv`, `-blocks.csv`, `-invitations.csv`,
`-explain.csv`, `-corrections.csv`, `-ratings.csv`, `-blockfeatures.csv`,
`-meta.csv`). Pass one folder per participant, or the files themselves. The
ratings and block-features files (export format 7) are analysed by
`analyze_ratings.py`; this script reads the ratings file only to count a rated
block inside a Mac's v6 primary period as a protocol deviation.
Output is the numbers named in `pitch-deck-inputs/evidence/traction-summary.md`,
each with its numerator, denominator, window and exclusions stated, because a
ratio on its own is not reportable evidence.

This script computes. It does not decide. Every threshold and definition here
is read from that file's pre-registration (2026-08-09), its additions
(2026-08-17), the amendment that replaced the primary outcome (2026-08-21), the
correction of 2026-08-31, the drift policy v2 amendment and 0.1.6 metrics
(both 2026-09-25), the drift policy v3 and v4 amendments (both 2026-09-26),
the drift policy v5 amendment (2026-09-27), the drift policy v6 amendment
(drafted 2026-10-07; it must be in that file before any v6 build reaches a
tester), the v6 primary-period amendment (drafted 2026-10-17; it must be in
that file before the first wave-1 link is sent), and the instrument addendum
that fixes how the outcomes are computed (drafted 2026-09-27; it must be in
that file before any export is used), all written before any cohort data
existed.

Every figure below counts only rows inside each Mac's v6 primary period: its
first 14 days, from the start of its first block after the upgrade cut. Rows
after day 14, and rows at or after the moment the Mac first turned on the
question after a block, are reported as two separate descriptive strata and
never pooled with the figures or with each other (`V6_PRIMARY_PERIOD_SECONDS`).
Nothing may be added after seeing results. If a definition turns out to be
wrong, amend it in a dated note stating what was known at the time.

The replacement primary outcome (sustained anchor engagement, at least 600 of
the 900 seconds after a decision point in the anchor category) needs the
observation ledger, which never leaves the tester's Mac. The exporter works
out each decision's labels there (`-outcomes.csv`, export format 4), and this
script counts them on the eligible points, with censoring by reason. An export
from before format 4 has no labels, and its points are reported as not
measurable, never approximated. The retired 2026-08-09 figure is reported as
descriptive, never as the headline.

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
import math
import sys
import textwrap
import time
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
# unanswered, with no time bound, so the bound is applied here. An answered
# offer is terminal: someone who tapped "Back to work" and then returned has
# outcome `accepted_action`, never `returned`, so this counts only returns
# recorded while the offer was still unanswered.
RETURN_WINDOW_SECONDS = 600

# The replacement primary outcome, 2026-08-21: at each eligible decision
# point, whether at least 600 of the following 900 seconds were spent in the
# anchor category recorded on the decision row.
PRIMARY_HORIZON_SECONDS = 900
PRIMARY_THRESHOLD_SECONDS = 600

# The two secondary outcomes of 2026-08-21, on the same horizon: no further
# departure from the anchor within 600 s, and the time to the first anchor run
# of at least 300 s.
DEPARTURE_FREE_SECONDS = 600
SUSTAINED_RUN_SECONDS = 300

# The 2026-08-21 censoring rule: above 10% of eligible points censored, the
# censored count is reported beside every result; above 25%, the result is
# reported as insufficient rather than as an estimate.
CENSORED_REPORT_SHARE = 0.10
CENSORED_INSUFFICIENT_SHARE = 0.25

# The 2026-08-21 rule: until randomization is enabled, every comparison this
# outcome supports is associational and must be labelled as such. Policy v6
# offers every eligible point (propensity 1.0), so there is no silence arm and
# no rate here is an effect of the nudge. The tag goes on every rendered
# primary-outcome result, so a line copied on its own keeps it.
ASSOCIATIONAL_TAG = "associational, not an effect of the nudge"
ASSOCIATIONAL_STATEMENT = (
    "Associational, not an effect of the nudge: every eligible point was offered "
    "at propensity 1.0, so there is no silence arm and nothing to compare these "
    "rates with. 2026-08-21: until randomization is enabled, every result from "
    "this outcome is associational and labelled as such."
)

# The per-decision outcomes file (`-outcomes.csv`, export format 4). The
# exporter computes it on the tester's Mac from `work_block_observation`, which
# never leaves; `label_decision` below restates the exporter's SQL, and both are
# held to `scripts/tests/fixtures/outcome-label-vectors.json`.
#
# Censoring reasons, the closed set of 2026-08-21 as the export can see it:
#   block_ended   the block ended before the horizon elapsed;
#   export_ended  the block was still open when the export ran, and the
#                 horizon had not elapsed or ends inside the open dwell, whose
#                 length is not known yet;
#   observer_gap  some second of the horizon is covered by no closed
#                 observation row of the block (a pause, sleep, restart, or a
#                 final dwell the ledger never measured).
# `none` means the horizon was fully observed and the labels are set.
#
# What observer_gap does NOT catch: ledger rows tile the block by
# construction (a closed report for the row still open is dropped, and the row
# closes where the next one begins), so only a pause (sleep pauses), a restart
# or the block's end leaves a gap. A stretch where collection stopped while the
# block stayed active, such as Accessibility revoked and restored inside the
# block, counts as observed time in the category of the row open before it.
# How often that happens is unknown.
OUTCOME_CENSOR_REASONS = ("block_ended", "export_ended", "observer_gap")
# Why an observer_gap point is unobserved: a second closed set, beside the
# reason and not a reason of its own, so the 2026-08-21 set stays as it is.
#   final_dwell  the block ended, and every unobserved second comes after its
#                last ledger row began: the dwell the person was in when the
#                block ended, which the ledger never measures (it can also
#                hold a pause or restart after which nothing was reported);
#   pause        any other observer_gap point (an unobserved second before a
#                later row, or a block still open) whose block recorded paused
#                time (the person's own pause, or sleep, which pauses) and no
#                restart: only a pause or a restart opens such a gap;
#   other        the same, but the block also restarted, or recorded neither:
#                a gap the export cannot attribute to a pause.
OBSERVER_GAP_CAUSES = ("pause", "final_dwell", "other")
OUTCOME_DEFINITION_VERSION = 1
OUTCOME_COLUMNS = (
    "decision_id",
    "censor_reason",
    "observer_gap_cause",
    "sustained_anchor_900s",
    "departure_free_600s",
    "seconds_to_sustained_return",
    "departure_category",
)

# The eight categories the service writes, and the only values the exporter
# lets out in `departure_category`. Anything else becomes `unrecognized`.
SERVICE_CATEGORIES = (
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    "UNLOGGED",
)

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
# The founder appended the note to traction-summary.md on 2026-09-26.
#
# Note, dated 2026-09-26: drift policy v4 (no protocol change) keeps every v3
# constant and timing, so the warm-up exclusion above stands, but the gate now
# receives an application the Mac client cannot observe at window level as a
# dwell of its own. Under v3 such an application reported nothing and its time
# went to the dwell before it, so a departure to it was never a decision point,
# never a switch, and never took time from the anchor. Rows from v1 to v4 are
# never pooled, and every cohort result uses policy_version 4 only. The founder
# appended the note to traction-summary.md as its 2026-09-26 amendment.
#
# Note, dated 2026-09-27: drift policy v5 (no protocol change) keeps every v4
# constant and timing, so the warm-up exclusion above stands, but a browser tab
# is now classified by its site. A tab on a host in the compiled-in site table
# is confident evidence in that host's category whatever its title says, and a
# tab on a site the table does not name is confident evidence when its own
# hostname's signals agree; a tab filed as SYSTEM (a sign-in or account page)
# is never evidence, as under v4. Under v4 such a tab counted only when a
# keyword rule (or, rarely, Tier 2) named it; otherwise it was the ambiguous
# browser prior, or UNLOGGED where two rules disagreed, and the gate counted
# neither. The change also runs the other way: under v5 the title-keyword rules
# stand aside for a tab whose site can be read, so a tab they made confident
# under v4 on a site the table neither names nor can infer (a Jira ticket on
# *.atlassian.net, the LinkedIn feed) is, short of a rare Tier 2 match, the
# ambiguous prior under v5, and the v4 switch or return there disappears.
# Browser time moves both ways, mostly from unclear to a category. The anchor,
# the switch counts and the decision points differ wherever a browser was
# open. Rows from v1 to v5 are never pooled, and every cohort result uses
# policy_version 5 only. The founder appended the note to traction-summary.md
# as its 2026-09-27 amendment.
#
# Note, dated 2026-10-07: drift policy v6 (no protocol change) keeps every v5
# constant and timing, so the warm-up exclusion above stands, but two sites are
# classified differently. A YouTube tab was PASSIVE_CONSUMPTION whatever video
# it showed; under v6 its video title decides it (a learning marker is
# REFERENCE, an entertainment marker PASSIVE_CONSUMPTION, both at medium
# confidence, and a title with neither or both is the ambiguous prior, never
# evidence), and TikTok's web app is SOCIAL_FEED instead of
# PASSIVE_CONSUMPTION. Anchors, switches and decision points differ wherever
# either was open. Rows from v1 to v6 are never pooled, and every cohort result
# uses policy_version 6 only. [Pending: the founder appends the v6 note to
# traction-summary.md before any v6 build reaches a tester.]
#
# Note, dated 2026-10-15, part of the same v6 (the founder's anchor rule, made
# before any v6 build was distributed): the anchor on a decision row is the
# work category (FOCUS_WORK, REFERENCE, TASK_MANAGEMENT or COMMUNICATION) with
# the most confident, closed time in the block, never PASSIVE_CONSUMPTION or
# SOCIAL_FEED. Through v5 it was any category with the most such time, so after
# a long drift the anchor became the drift: the return to work was the
# departure, `sustained_anchor_900s` scored time back at work as a failure and
# time on the drift as a success, and a return to the drift closed the offer
# as `returned`. A v6 block with no work time yet has no anchor and gets no
# offer: past the warm-up it records `abstained_no_anchor`, with no anchor and
# so no outcome row. The labels are computed exactly as before, against the
# anchor the row records, and an eligible (offered) point always has a work
# anchor. [Pending: the v6 note in traction-summary.md describes this rule
# before any v6 build reaches a tester.]
ANALYSED_POLICY_VERSION = 6

# The anchors that rule allows. A build cut from develop between the YouTube
# change (#78) and the anchor rule (#83) writes policy_version 6 with the old
# most-time anchor, and its rows can be told apart only where that anchor was
# not a work category, on a decision row or on the offer a decision made. No
# such build may reach a tester; an export holding one is malformed and
# excluded whole, because its rows with a work anchor are indistinguishable
# from a later build's.
WORK_ANCHOR_POLICY_VERSION = 6
WORK_ANCHOR_CATEGORIES = ("FOCUS_WORK", "REFERENCE", "TASK_MANAGEMENT", "COMMUNICATION")

# Note, dated 2026-10-17, the v6 primary-period amendment (founder decision
# 2026-10-17, "I agree with all 1-7": decision 1 of the attention plan fixes
# each Mac's v6 primary period at its first 14 days). Drafted in
# `pitch-deck-inputs/evidence/drafts/2026-10-17-v6-primary-period-note.md`;
# its items are cited by number below. [Pending: the founder appends the note
# to traction-summary.md, unchanged or as edited, before the first wave-1 link
# is sent. If its text changes, this code changes to match before any wave-1
# result is read.]
#
#   1. A Mac's period starts at the started_at of its first block after the
#      upgrade cut (its last decision row with a policy_version below 6), or
#      of its first block if it has no such row, and lasts 1,209,600 s. A Mac
#      whose export holds no such block has no period and counts in no figure
#      but blocks declared (item 6). An export that states an earlier start
#      of its own (meta `v6_period_start`, export format 7) had its history
#      cleared and falls under the reinstall exclusion; one that states a
#      later start is malformed, and the computed start is used.
#   2. Every figure counts in-period rows only. A block is placed by its
#      started_at, a decision point and an offer by their block's, an
#      invitation by its offered_at. A local week is placed only when the
#      week, widened by a day at each end, lies on one side of the period's
#      start, its end and rating_prompt_enabled_at; one that straddles any of
#      them counts in no figure and is counted. Correction counts carry no
#      time: reported whole, and flagged when the export holds a block outside
#      the period or a row at or after rating_prompt_enabled_at. Rows the
#      upgrade cut keeps but that fall before the period's start count in no
#      figure and are counted.
#   3. The power count counts in-period decision points only.
#   4. Rows at or after the period's end form the "after day 14" stratum,
#      reported under the same definitions and never pooled.
#   5. Rows at or after rating_prompt_enabled_at (meta, export format 7: the
#      first time the question after a block was ever turned on) form the
#      "question on" stratum, kept out of the figures and of "after day 14".
#      Such a row inside the period, and any block inside the period with a
#      row in the export's ratings file, is a protocol deviation, counted.
#   6. Gate D measure 1 counts in-period blocks only, with the blocks outside
#      the period reported beside it; its denominator does not change.
V6_PRIMARY_PERIOD_SECONDS = 1209600  # 14 days
V6_PERIOD_POLICY_VERSION = 6
DAY_SECONDS = 24 * 3600
# Where a row lands, in the order the report lists them. Only the first three
# are reported as figures or strata; the rest count in no figure.
PLACEMENTS = (
    "in_period",
    "after_day_14",
    "question_on",
    "before_period",
    "no_period",
    "no_time",
)
STRATA = ("after_day_14", "question_on")
PLACEMENT_LABELS = {
    "in_period": "inside its Mac's v6 primary period (counted)",
    "after_day_14": "after day 14 (the 'after day 14' stratum)",
    "question_on": "at or after rating_prompt_enabled_at (the 'question on' stratum)",
    "before_period": "after the upgrade cut and before the period's start (in no figure)",
    "no_period": "on a Mac with no v6 period: no block after the upgrade cut (in no figure)",
    "no_time": "with no time to place it by (in no figure)",
    "straddles": "straddling a period boundary, widened by a day (in no figure)",
}
# The kinds of row placed, for the report's table.
PLACED_KINDS = ("decisions", "offers", "blocks", "invitations", "explain_weeks")

# ---------------------------------------------------------------------------
# Export format 7: the optional question after a block (migration 0048).
# `analyze_ratings.py` analyses these files; the column lists are its
# contract with the exporter, and `block_features` below restates how the
# exporter computes each block's summary so the two can be held to
# scripts/tests/fixtures/block-feature-vectors.json.
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
BLOCK_FEATURE_VERSION = 1
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
FEED_CATEGORIES = ("SOCIAL_FEED", "PASSIVE_CONSUMPTION")
NEEDS_A_CATEGORY = ("UNLOGGED", "UNCLASSIFIED")
COVERAGE_CLASSES = ("insufficient", "partial", "good")
# Six-hour parts of the local day, by the hour the block started in.
HOUR_BUCKETS = ("night", "morning", "afternoon", "evening")

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

# `notification_delivery` as the exporter derives it from `delivery_channel`
# (migration 0043, protocol 35): what Swift reported became of the offer's OS
# notification. The first eight are the channels Swift can report;
# not_reported = NULL on a row offered after the Mac applied 0043; unknown =
# NULL before that, or an export without the column (every export before
# format 5). Unknown is never folded into not_reported. Descriptive only: no
# pre-registered denominator reads it.
DELIVERY_CHANNELS = (
    "notification_posted",
    "alert_style_none",
    "permission_denied",
    "permission_unknown",
    "rejected",
    "withdrawn",
    "quiet",
    "stale",
)
DELIVERY_BUCKETS = DELIVERY_CHANNELS + ("not_reported", "unknown")
# The one channel posted under an alert style that puts it on screen. Posted
# is not seen: a Focus mode Velvt could not detect (no Focus-status
# permission) holds a posted notification in Notification Center, and it is
# still recorded as notification_posted. An upper bound on offers seen.
DELIVERY_POSTED_VISIBLE = ("notification_posted",)

# `focus_state` on the decisions file (migration 0044, protocol 36): what the
# service knew about Focus when the gate decided. on / off: the app could read
# Focus status and the record said a Focus was / was not on. unknown: it could
# not (a build without Apple's Communication Notifications entitlement, Focus
# status denied or never answered), had not yet said whether it could since the
# helper started, or no Focus state was recorded yet (for example after Clear
# Local Work Blocks), so a Focus may have silenced an offer that is counted as
# delivered. not_recorded = an empty value: the decision predates migration
# 0044, or the export has no such column (every export before format 6). Never
# folded into off. Each offer takes the value on the decision-log row
# that attributes it (same block, verdict offered / withheld_demotion /
# suppressed_dnd). Descriptive only: no pre-registered denominator reads it.
FOCUS_STATES = ("on", "off", "unknown")
FOCUS_STATE_BUCKETS = FOCUS_STATES + ("not_recorded",)

BLOCK_TERMINAL_PHASES = ("completed", "abandoned", "expired")
BLOCK_OPEN_PHASES = ("active", "paused")
BLOCK_ORIGINS = ("manual", "invitation")

INVITATION_TERMINAL_OUTCOMES = ("accepted", "dismissed", "no_response", "expired")
INVITATION_OPEN_OUTCOMES = ("offered",)

# The corrections file (2026-08-17 measure 3), one row per rule scope and broad
# category. `app` is `personal_app_override` and `site` is
# `personal_site_override` (migration 0040, taught from 1.0.14); the sum of
# their correction_count is the pre-registered count. `window` is
# `personal_override`, which keeps no count. The exporter writes any category
# outside the service's accepted set as `unrecognized`, and writes `site` rows
# only from a database that has the table (meta `site_rules,present`).
CORRECTION_SCOPES = ("app", "site", "window")
# Scopes whose rows carry a correction count.
COUNTED_CORRECTION_SCOPES = ("app", "site")
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
    "outcomes": "-outcomes.csv",
    # Export format 7. Named here so a folder holding them is one export, not
    # two per-offer files.
    "ratings": "-ratings.csv",
    "blockfeatures": "-blockfeatures.csv",
    "meta": "-meta.csv",
}
FILE_KINDS = ("offers",) + tuple(COMPANION_SUFFIXES)

# Meta keys naming which companion tables the tester's database had.
META_TABLE_KEYS = {
    "decisions": "decision_log",
    "invitations": "invitations",
    "explain": "explain_probe",
    "corrections": "corrections",
    "outcomes": "outcomes",
    "ratings": "ratings",
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
    notification_delivery: str = "unknown"
    focus_state: str = "not_recorded"


@dataclass
class Excluded:
    participant: str
    row: str
    reason: str
    # The declared exclusion this falls under, for counting.
    category: str


@dataclass
class Period:
    """One Mac's v6 primary period (items 1 and 5 of the amendment)."""

    start: int | None = None
    upgrade_cut: int | None = None
    stated_start: int | None = None
    rating_prompt_enabled_at: int | None = None

    @property
    def end(self) -> int | None:
        return None if self.start is None else self.start + V6_PRIMARY_PERIOD_SECONDS

    def inside(self, at: int | None) -> bool:
        return self.start is not None and at is not None and self.start <= at < self.end

    def place(self, at: int | None) -> str:
        """Where a row placed at `at` counts: one of PLACEMENTS."""
        if self.start is None:
            return "no_period"
        if at is None:
            return "no_time"
        if at < self.start:
            return "before_period"
        if self.rating_prompt_enabled_at is not None and at >= self.rating_prompt_enabled_at:
            return "question_on"
        if at < self.end:
            return "in_period"
        return "after_day_14"

    def place_week(self, week: str) -> str:
        """A local week (its Monday's date), widened by a day at each end. A
        local date cannot be placed on the UTC axis exactly, so a week placed
        this way has no boundary strictly inside it, or it straddles."""
        if self.start is None:
            return "no_period"
        try:
            monday = datetime.strptime(week, "%Y-%m-%d").replace(tzinfo=timezone.utc)
        except ValueError:
            return "no_time"
        low = int(monday.timestamp()) - DAY_SECONDS
        high = int(monday.timestamp()) + 8 * DAY_SECONDS
        boundaries = [self.start, self.end]
        if self.rating_prompt_enabled_at is not None:
            boundaries.append(self.rating_prompt_enabled_at)
        if any(low < boundary < high for boundary in boundaries):
            return "straddles"
        return self.place(low)


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
    # None when the export carried no usable outcomes file; the reason is in
    # `outcomes_unusable`.
    outcome_rows: list | None = None
    outcomes_unusable: str = ""
    # Export format 7: the blocks the ratings file has a row for.
    rated_block_ids: set = field(default_factory=set)
    period: Period = field(default_factory=Period)


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
    # The v6 primary period (2026-10-17 amendment). Everything above holds
    # in-period rows only; `strata` holds a Cohort per descriptive stratum.
    strata: dict = field(default_factory=dict)
    periods: dict = field(default_factory=dict)
    placed: dict = field(default_factory=dict)
    question_on_inside_period: Counter = field(default_factory=Counter)
    rated_blocks_inside_period: Counter = field(default_factory=Counter)
    blocks_outside_period: Counter = field(default_factory=Counter)
    corrections_with_rows_outside_period: list[str] = field(default_factory=list)
    period_problems: list[str] = field(default_factory=list)


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
# Per-decision outcome labels
#
# The exporter's SQL computes these on the tester's Mac, because the
# observation rows they need never leave it. This is the same definition,
# written second by second so it can be read against the pre-registration
# rather than against SQL. The analysis itself reads the exported labels; this
# function exists so the two can be held to one set of test vectors.
# ---------------------------------------------------------------------------


def _is_confident(category: str, status: str, confidence: str) -> bool:
    """The drift gate's bar for evidence (`is_confident` in work_block/mod.rs)."""
    return (
        status == "classified"
        and confidence in ("high", "medium")
        and category.lower() not in ("system", "unclassified", "unlogged")
    )


def label_decision(
    occurred_at: int,
    anchor_category: str,
    block_ended_at: int | None,
    exported_at: int,
    observations: list[dict],
    *,
    block_phase: str = "completed",
    block_total_paused_seconds: int = 0,
    block_recovered_after_restart: bool = False,
) -> dict:
    """The outcome labels for one decision row with an anchor.

    `observations` are the block's `work_block_observation` rows as dicts with
    `occurred_at`, `ended_at` (None while open), `category`,
    `classification_status` and `classification_confidence`, in id order.
    They are read in the ledger's order, occurred_at then id, as the gate
    reads them. Times are epoch seconds; a row covers [occurred_at, ended_at).
    The `block_*` facts are the `work_block` row's, and are read only to say
    why an observer_gap point is unobserved (`OBSERVER_GAP_CAUSES`).
    """
    t = occurred_at
    horizon_end = t + PRIMARY_HORIZON_SECONDS
    anchor = anchor_category.upper()
    ledger = sorted(enumerate(observations), key=lambda pair: (pair[1]["occurred_at"], pair[0]))
    rows = [
        dict(
            start=o["occurred_at"],
            end=o["ended_at"],
            category=o["category"].upper(),
            confident=_is_confident(
                o["category"], o["classification_status"], o["classification_confidence"]
            ),
        )
        for _, o in ledger
    ]

    # Where the gate's evidence had the person at the decision: the latest
    # confident row that began at or before it, when that is not the anchor.
    departure_category = None
    latest = [r for r in rows if r["confident"] and r["start"] <= t]
    if latest and latest[-1]["category"] != anchor:
        category = latest[-1]["category"]
        departure_category = category if category in SERVICE_CATEGORIES else "unrecognized"

    labels = {
        "censor_reason": "none",
        "observer_gap_cause": None,
        "sustained_anchor_900s": None,
        "departure_free_600s": None,
        "seconds_to_sustained_return": None,
        "departure_category": departure_category,
    }
    seconds = range(t, horizon_end)
    closed = [r for r in rows if r["end"] is not None]
    covered = [any(r["start"] <= s < r["end"] for r in closed) for s in seconds]
    in_anchor = [
        any(r["confident"] and r["category"] == anchor and r["start"] <= s < r["end"] for r in closed)
        for s in seconds
    ]

    if block_ended_at is not None and block_ended_at < horizon_end:
        labels["censor_reason"] = "block_ended"
    elif block_ended_at is None and (
        horizon_end > exported_at
        or any(r["end"] is None and r["start"] < horizon_end for r in rows)
    ):
        labels["censor_reason"] = "export_ended"
    elif not all(covered):
        labels["censor_reason"] = "observer_gap"
        # A second after the block's last row began has no later row: it is
        # the final stretch. Any unobserved second before it was followed by
        # more observation, so something stopped the ledger and restarted it.
        last_start = max((r["start"] for r in rows), default=None)
        unobserved = [s for s, seen in zip(seconds, covered) if not seen]
        paused = block_total_paused_seconds > 0 or block_phase == "paused"
        if (
            block_ended_at is not None
            and last_start is not None
            and all(s >= last_start for s in unobserved)
        ):
            labels["observer_gap_cause"] = "final_dwell"
        elif paused and not block_recovered_after_restart:
            labels["observer_gap_cause"] = "pause"
        else:
            labels["observer_gap_cause"] = "other"
    if labels["censor_reason"] != "none":
        return labels

    labels["sustained_anchor_900s"] = int(sum(in_anchor) >= PRIMARY_THRESHOLD_SECONDS)

    # A departure is the gate's: a confident non-anchor row whose previous
    # confident row was the anchor. The decision's own row is not "further".
    departures = 0
    previous = None
    for r in rows:
        if not r["confident"]:
            continue
        if (
            r["category"] != anchor
            and previous == anchor
            and t < r["start"] <= t + DEPARTURE_FREE_SECONDS
        ):
            departures += 1
        previous = r["category"]
    labels["departure_free_600s"] = int(departures == 0)

    run_start = None
    for index, anchored in enumerate(in_anchor):
        if not anchored:
            run_start = None
            continue
        if run_start is None:
            run_start = index
        if index - run_start + 1 >= SUSTAINED_RUN_SECONDS:
            labels["seconds_to_sustained_return"] = run_start
            break
    return labels


# ---------------------------------------------------------------------------
# Per-block summary of a rated block (export format 7, BLOCK_FEATURE_VERSION 1)
#
# The exporter computes it in SQL on the tester's Mac, from the block, its
# observation rows, the stored end-of-block result and its decision and offer
# rows. This is the same definition, written as plain code. Nothing in this
# script reads it; `analyze_ratings.py` reads the exported file. It exists so
# the exporter and this statement are held to one set of vectors, which the
# app's own result is held to as well
# (`rust-service/src/work_block/block_feature_vectors.rs`).
# ---------------------------------------------------------------------------


def _share6(part: float, whole: float) -> float:
    return round(part / whole, 6)


def block_features(
    block: dict,
    observations: list[dict],
    result: dict | None,
    policy_versions: list[int],
    offer_outcome: str | None,
) -> dict:
    """The block-features row for one block, as `{column: value}` (None is an
    empty cell).

    `block` holds the `work_block` columns (`block_id`, `started_at`,
    `ended_at`, `total_paused_seconds`, `purpose`, `intensity`,
    `planned_duration_seconds`, `origin`). `observations` are its
    `work_block_observation` rows as `label_decision` takes them. `result` is
    the stored result payload, decoded, or None when there is none.
    `policy_versions` are those of the block's decision-log rows, and
    `offer_outcome` is its `work_block_intervention` outcome or None. The hour
    bucket and weekend use this process's local time zone, as the exporter's
    `localtime` does on the tester's Mac.
    """
    started = block["started_at"]
    ended = block.get("ended_at")
    paused = block.get("total_paused_seconds") or 0
    elapsed = None if ended is None else max(0, max(0, ended - started) - paused)

    confident_seconds = 0
    feed_seconds = 0
    unlogged_seconds = 0
    work_seconds: dict[str, int] = {}
    for row in observations:
        if row["ended_at"] is None:
            continue
        seconds = max(0, row["ended_at"] - row["occurred_at"])
        category = row["category"]
        status = row["classification_status"]
        if _is_confident(category, status, row["classification_confidence"]):
            confident_seconds += seconds
            if category.upper() in FEED_CATEGORIES:
                feed_seconds += seconds
            # Grouped by the category as stored, as the gate's work_anchor
            # groups it; only the largest is read.
            if category.upper() in WORK_ANCHOR_CATEGORIES:
                work_seconds[category] = work_seconds.get(category, 0) + seconds
        if category.upper() in NEEDS_A_CATEGORY or status != "classified":
            unlogged_seconds += seconds

    result = result or {}
    coverage = result.get("coverage")
    ratio = result.get("coverage_ratio")
    result_elapsed = result.get("elapsed_duration_seconds")
    switch_aways = result.get("switch_away_count")
    recoveries = result.get("recovery_count")
    longest = result.get("longest_uninterrupted_seconds")
    evidence = result.get("safe_evidence_category")
    # SQLite's ROUND on a non-negative value: half away from zero.
    covered = (
        None if ratio is None or result_elapsed is None
        else int(math.floor(ratio * result_elapsed + 0.5))
    )

    if coverage is None:
        branch = None
    elif coverage == "insufficient" or evidence is None:
        branch = "low_coverage"
    elif switch_aways == 0:
        branch = "stayed"
    elif recoveries == 0:
        branch = "ended_elsewhere"
    else:
        branch = "came_back"

    if switch_aways is None or covered is None:
        per_hour = None
    elif covered > 0:
        per_hour = round(switch_aways * 3600.0 / covered, 6)
    else:
        per_hour = 0.0

    versions = set(policy_versions)
    delivered = offer_outcome is not None and offer_outcome not in WITHHELD_TERMINAL_OUTCOMES
    local = time.localtime(started)
    return {
        "block_id": block.get("block_id"),
        "block_feature_version": BLOCK_FEATURE_VERSION,
        "policy_version": next(iter(versions)) if len(versions) == 1 else None,
        "card_branch": branch,
        "elapsed_seconds": elapsed,
        "covered_seconds": covered,
        "coverage_class": (
            None if coverage is None
            else coverage if coverage in COVERAGE_CLASSES
            else "unrecognized"
        ),
        "work_anchor_share": (
            _share6(max(work_seconds.values(), default=0), confident_seconds)
            if confident_seconds > 0 else 0.0
        ),
        "switch_aways_per_covered_hour": per_hour,
        "longest_run_share": (
            _share6(longest, elapsed) if longest is not None and elapsed else None
        ),
        "recovery_ratio": (
            _share6(recoveries, max(switch_aways, 1))
            if recoveries is not None and switch_aways is not None else None
        ),
        "feed_share": (
            _share6(feed_seconds, confident_seconds) if confident_seconds > 0 else 0.0
        ),
        "unlogged_share": _share6(unlogged_seconds, elapsed) if elapsed else None,
        "offer_delivered": int(delivered),
        "offer_outcome": offer_outcome if delivered else None,
        "purpose": block.get("purpose"),
        "intensity": block.get("intensity"),
        "planned_minutes": (block["planned_duration_seconds"] + 30) // 60,
        "origin": block.get("origin"),
        "hour_bucket": HOUR_BUCKETS[local.tm_hour // 6],
        "weekend": int(local.tm_wday in (5, 6)),
    }


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

        for kind in ("blocks", "invitations", "explain", "corrections", "outcomes", "ratings",
                     "blockfeatures"):
            path = participant.files.get(kind)
            meta_key = META_TABLE_KEYS.get(kind, "ratings" if kind == "blockfeatures" else "")
            if path is None:
                if participant.meta.get(meta_key, "") == "present":
                    cohort.malformed.append(
                        f"{name}: the export wrote a {kind} file and it was not received"
                    )
                    if kind == "outcomes":
                        participant.outcomes_unusable = "outcomes file written and not received"
                continue
            if kind == "blockfeatures":
                # Read by analyze_ratings.py, not here.
                continue
            rows, fields, error = _read_csv(path)
            if error:
                cohort.malformed.append(f"{name}: {kind} file {error}")
                if kind == "outcomes":
                    participant.outcomes_unusable = "outcomes file unreadable"
                continue
            if kind == "ratings":
                if "block_id" not in fields:
                    cohort.malformed.append(f"{name}: ratings file has no block_id column")
                    continue
                participant.rated_block_ids = {_text(r, "block_id") for r in rows if _text(r, "block_id")}
                continue
            if kind == "outcomes":
                version = participant.meta.get("outcomes_definition_version", "")
                if tuple(fields) != OUTCOME_COLUMNS:
                    participant.outcomes_unusable = "outcomes file has other columns"
                elif version and version != str(OUTCOME_DEFINITION_VERSION):
                    participant.outcomes_unusable = (
                        f"outcomes computed under definition version {version}; "
                        f"this script reads version {OUTCOME_DEFINITION_VERSION}"
                    )
                if participant.outcomes_unusable:
                    cohort.malformed.append(f"{name}: {participant.outcomes_unusable}")
                    continue
                participant.outcome_rows = rows
            elif kind == "blocks":
                participant.block_rows = rows
            elif kind == "invitations":
                participant.invitation_rows = rows
            elif kind == "explain":
                participant.explain_rows = rows
            else:
                participant.correction_rows = rows

        non_work = _non_work_anchors(participant)
        if non_work:
            shown = ", ".join(non_work[:3]) + (", ..." if len(non_work) > 3 else "")
            cohort.malformed.append(
                f"{name}: {len(non_work)} policy_version {WORK_ANCHOR_POLICY_VERSION} "
                f"row(s) with an anchor that is not a work category: {shown}"
            )
            cohort.excluded_whole[name] = (
                f"policy_version {WORK_ANCHOR_POLICY_VERSION} rows with a non-work anchor: "
                "a v6 build from before the 2026-10-15 anchor rule"
            )
            continue

        period = v6_period(participant)
        if period.start is not None and period.stated_start is not None:
            if period.stated_start < period.start:
                cohort.excluded_whole[name] = (
                    f"history cleared: the export states a v6 period start ({period.stated_start}) "
                    f"earlier than its blocks give ({period.start}); the reinstall exclusion "
                    "applies (v6 primary-period amendment, item 1)"
                )
                continue
            if period.stated_start > period.start:
                problem = (
                    f"{name}: the meta's v6_period_start {period.stated_start} is later than the "
                    f"start its blocks give ({period.start}); the computed start is used"
                )
                cohort.malformed.append(problem)
                cohort.period_problems.append(problem)
        elif period.stated_start is not None:
            problem = (
                f"{name}: the meta states a v6_period_start ({period.stated_start}) and the export "
                "holds no block after the upgrade cut; the Mac has no period"
            )
            cohort.malformed.append(problem)
            cohort.period_problems.append(problem)
        participant.period = period
        cohort.analysed.append(participant)

    cohort.strata = {stratum: Cohort() for stratum in STRATA}
    for participant in cohort.analysed:
        _load_participant(cohort, participant)
    return cohort


def v6_period(participant: Participant) -> Period:
    """Item 1 of the v6 primary-period amendment, from the export's own rows.

    The upgrade cut is the occurred_at of the Mac's last decision row with a
    policy_version below 6. The period starts at the earliest block start
    after it, read from the blocks file, and from the decisions and offers
    files, which carry their block's start (they add a block only to an export
    with no blocks file). The meta's stated start is returned beside it, never
    used in its place.
    """
    older = []
    for row in participant.decision_rows:
        version, occurred = _int(row, "policy_version"), _int(row, "occurred_at")
        if version is not None and occurred is not None and version < V6_PERIOD_POLICY_VERSION:
            older.append(occurred)
    cut = max(older) if older else None
    starts = [_int(row, "started_at") for row in participant.block_rows or []]
    starts += [_int(row, "block_started_at") for row in participant.decision_rows]
    starts += [_int(row, "started_at") for row in participant.offer_rows]
    after_cut = [s for s in starts if s is not None and (cut is None or s > cut)]
    return Period(
        start=min(after_cut) if after_cut else None,
        upgrade_cut=cut,
        stated_start=_int(participant.meta, "v6_period_start"),
        rating_prompt_enabled_at=_int(participant.meta, "rating_prompt_enabled_at"),
    )


def _non_work_anchors(participant: Participant) -> list[str]:
    """Policy-6 decision and offer rows whose anchor is not a work category."""
    found = []
    blocks = set()
    for row in participant.decision_rows:
        if _int(row, "policy_version") != WORK_ANCHOR_POLICY_VERSION:
            continue
        if _text(row, "gate_verdict") in ATTRIBUTING_VERDICTS:
            blocks.add(_text(row, "block_id"))
        anchor = _text(row, "anchor_category")
        if anchor and anchor.upper() not in WORK_ANCHOR_CATEGORIES:
            found.append(f"decision {_text(row, 'decision_id') or '?'} {anchor!r}")
    for row in participant.offer_rows:
        anchor = _text(row, "anchor_category")
        if _text(row, "block_id") in blocks and anchor and anchor.upper() not in WORK_ANCHOR_CATEGORIES:
            found.append(f"offer {_text(row, 'block_id')} {anchor!r}")
    return found


def _outcome_problem(row: dict) -> str | None:
    """Why one row of an outcomes file cannot be used, or None."""
    reason = _text(row, "censor_reason")
    labels = [
        _text(row, key)
        for key in ("sustained_anchor_900s", "departure_free_600s", "seconds_to_sustained_return")
    ]
    category = _text(row, "departure_category")
    if category and category not in SERVICE_CATEGORIES + ("unrecognized",):
        return f"departure_category {category!r} is not one of the service's categories"
    cause = _text(row, "observer_gap_cause")
    if reason == "observer_gap" and cause not in OBSERVER_GAP_CAUSES:
        return f"observer_gap_cause {cause!r} is not one of {', '.join(OBSERVER_GAP_CAUSES)}"
    if reason != "observer_gap" and cause:
        return f"observer_gap_cause {cause!r} on a point censored as {reason!r}"
    if reason in OUTCOME_CENSOR_REASONS:
        return None if labels == ["", "", ""] else f"censored ({reason}) but labelled"
    if reason != "none":
        return f"censor_reason {reason!r}"
    if labels[0] not in ("0", "1") or labels[1] not in ("0", "1"):
        return "uncensored without both 0/1 labels"
    if labels[2]:
        latest = PRIMARY_HORIZON_SECONDS - SUSTAINED_RUN_SECONDS
        value = _int(row, "seconds_to_sustained_return")
        if value is None or not 0 <= value <= latest:
            return f"seconds_to_sustained_return {labels[2]!r} is not 0 to {latest}"
    return None


def _outcome_disagreement(decision: dict, outcome: dict, exported_at: int | None) -> str | None:
    """Where an outcome row contradicts what the decisions file says on its own."""
    occurred = _int(decision, "occurred_at") or 0
    horizon_end = occurred + PRIMARY_HORIZON_SECONDS
    ended = _int(decision, "block_ended_at")
    reason = _text(outcome, "censor_reason")
    block_ended = ended is not None and ended < horizon_end
    if block_ended != (reason == "block_ended"):
        return f"censor_reason {reason!r}, but the block ended at {ended}"
    if reason == "export_ended" and ended is not None:
        return "export_ended on a block that had ended"
    if _text(outcome, "observer_gap_cause") == "final_dwell" and ended is None:
        return "an unmeasured final dwell in a block that had not ended"
    if ended is None and exported_at is not None and exported_at < horizon_end and reason != "export_ended":
        return f"censor_reason {reason!r} on a horizon still running at export"
    return None


def _load_participant(cohort: Cohort, participant: Participant) -> None:
    name = participant.name
    period = participant.period
    cohort.periods[name] = period
    placed_here: Counter = Counter()

    def route(kind: str, at: int | None) -> Cohort | None:
        """Where a row placed at `at` counts (amendment item 2): this cohort
        when it is inside the period, a stratum's cohort, or None for a row
        that counts in no figure. Every row routed is counted by placement."""
        placement = period.place(at)
        cohort.placed.setdefault(kind, Counter())[placement] += 1
        placed_here[(kind, placement)] += 1
        if placement == "question_on" and period.inside(at):
            cohort.question_on_inside_period[kind] += 1
        if placement == "in_period":
            return cohort
        return cohort.strata.get(placement)

    # --- Outcome labels, by decision ----------------------------------------
    outcomes: dict[str, dict] = {}
    unusable: dict[str, str] = {}
    for row in participant.outcome_rows or []:
        decision_id = _text(row, "decision_id")
        problem = _outcome_problem(row)
        if decision_id in outcomes or decision_id in unusable:
            problem = "a second row for this decision"
        if problem:
            cohort.malformed.append(f"{name}: outcome for decision {decision_id or '?'}: {problem}")
            outcomes.pop(decision_id, None)
            unusable[decision_id] = problem
            continue
        outcomes[decision_id] = row
    exported_at = _int(participant.meta, "exported_at")

    # --- Decisions, and the attribution map --------------------------------
    attribution: dict[str, set[int]] = {}
    focus_by_block: dict[str, set[str]] = {}
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
        focus_state = _text(row, "focus_state")
        if focus_state and focus_state not in FOCUS_STATES:
            cohort.malformed.append(
                f"{name}: decision {_text(row, 'decision_id') or '?'} has "
                f"focus_state={focus_state!r}"
            )
            focus_state = ""
        if verdict in ATTRIBUTING_VERDICTS and block_id:
            attribution.setdefault(block_id, set()).add(version)
            focus_by_block.setdefault(block_id, set()).add(focus_state or "not_recorded")
        if version != ANALYSED_POLICY_VERSION:
            other_policy_times.append(occurred)
            continue
        analysed_times.append(occurred)
        # A decision point is placed by its block's start; one logged without
        # a block, by its own time.
        block_started = _int(row, "block_started_at")
        target = route("decisions", occurred if block_started is None else block_started)
        if target is None:
            continue
        record = {"participant": name, "row": row}
        target.analysed_decisions.append(record)
        if verdict != "offered":
            continue
        elapsed = _elapsed(
            _int(row, "block_started_at"),
            _int(row, "block_ended_at"),
            _int(row, "block_total_paused_seconds"),
        )
        if elapsed is not None and elapsed < WARMUP_EXCLUSION_SECONDS:
            target.excluded.append(
                Excluded(name, f"decision {_text(row, 'decision_id')}",
                         f"block elapsed {elapsed}s < {WARMUP_EXCLUSION_SECONDS}s warm-up",
                         WARMUP_CATEGORY)
            )
            continue
        point = {"participant": name, "row": row, "meta": participant.meta,
                 "outcome": None, "no_outcome": None}
        decision_id = _text(row, "decision_id")
        if participant.outcome_rows is None:
            point["no_outcome"] = (
                participant.outcomes_unusable
                or "no outcomes file (an export from before export format 4)"
            )
        elif decision_id in unusable:
            point["no_outcome"] = "outcome row malformed"
        elif decision_id not in outcomes:
            point["no_outcome"] = "no row for this decision in the outcomes file"
            cohort.malformed.append(f"{name}: eligible decision {decision_id} has no outcome row")
        else:
            disagreement = _outcome_disagreement(row, outcomes[decision_id], exported_at)
            if disagreement:
                point["no_outcome"] = "outcome row contradicts the decisions file"
                cohort.malformed.append(f"{name}: outcome for decision {decision_id}: {disagreement}")
            else:
                point["outcome"] = outcomes[decision_id]
        target.eligible.append(point)

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

        # An offer is placed by its block's start.
        target = route("offers", _int(row, "started_at"))
        if target is None:
            continue
        elapsed = _elapsed(
            _int(row, "started_at"), _int(row, "ended_at"), _int(row, "total_paused_seconds")
        )
        if elapsed is not None and elapsed < WARMUP_EXCLUSION_SECONDS:
            target.excluded.append(
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
        delivery = _text(row, "notification_delivery")
        if delivery not in DELIVERY_BUCKETS:
            # An export without the derived column: a reported channel is still
            # a fact, and anything else is unknown, never not_reported.
            channel = _text(row, "delivery_channel")
            delivery = channel if channel in DELIVERY_CHANNELS else "unknown"
        # One offer per block, so one attributing decision; two that disagree
        # cannot be told apart and are not recorded rather than guessed.
        focus_states = focus_by_block.get(block_id, set())
        focus_state = next(iter(focus_states)) if len(focus_states) == 1 else "not_recorded"

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
            notification_delivery=delivery,
            focus_state=focus_state,
        )
        if outcome in WITHHELD_TERMINAL_OUTCOMES:
            target.withheld.append(record)
        else:
            target.offers.append(record)

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
            if period.inside(started) and _text(row, "block_id") in participant.rated_block_ids:
                cohort.rated_blocks_inside_period[name] += 1
            if period.start is not None and not period.inside(started):
                cohort.blocks_outside_period[name] += 1
            target = route("blocks", started)
            if target is None:
                continue
            entry = {"phase": phase, "origin": origin, "started_at": started}
            if target is cohort:
                kept.append(entry)
            else:
                target.blocks.setdefault(name, []).append(entry)
        # Gate D measure 1 counts every participant with a blocks file, a Mac
        # with no period as zero (item 6).
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
            target = route("invitations", offered)
            if target is None:
                continue
            entry = {"outcome": outcome, "policy_version": _text(row, "policy_version")}
            if target is cohort:
                kept.append(entry)
            else:
                target.invitations.setdefault(name, []).append(entry)
        # A Mac with no period counts in no figure but blocks declared (item 1).
        if period.start is not None:
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
            placement = period.place_week(week)
            cohort.placed.setdefault("explain_weeks", Counter())[placement] += 1
            placed_here[("explain_weeks", placement)] += 1
            if placement == "question_on":
                monday = datetime.strptime(week, "%Y-%m-%d").replace(tzinfo=timezone.utc)
                if period.inside(int(monday.timestamp()) - DAY_SECONDS):
                    cohort.question_on_inside_period["explain_weeks"] += 1
            entry = {"week": week, "taps": taps, "delivered": delivered, "blocks": declared}
            if placement == "in_period":
                kept.append(entry)
            elif placement in STRATA:
                cohort.strata[placement].explain.setdefault(name, []).append(entry)
        if period.start is not None:
            cohort.explain[name] = kept

    # --- Corrections (2026-08-17 measure 3) -------------------------------------
    # They carry no time, so they are reported whole (item 2), and only for a
    # Mac with a period (item 1).
    if participant.correction_rows is not None and period.start is not None:
        counted: dict[str, dict[str, tuple[int, int]]] = {
            scope: {} for scope in COUNTED_CORRECTION_SCOPES
        }
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
            elif scope in COUNTED_CORRECTION_SCOPES and (
                corrections is None or corrections < rules
            ):
                # Every app or site rule is written by at least one correction.
                problem = f"{scope} corrections missing or fewer than its rules"
            elif category in counted.get(scope, window):
                problem = "a second row for one scope and category"
            if problem:
                cohort.malformed.append(
                    f"{name}: corrections row scope={scope!r} category={category!r}: {problem}"
                )
                continue
            if scope in counted:
                counted[scope][category] = (rules, corrections)
            else:
                window[category] = rules
        cohort.corrections[name] = {
            "app": counted["app"],
            "site": counted["site"],
            "window": window,
            # A database from before migration 0040 cannot hold a site rule,
            # and an export from before the exporter read them has no key.
            "site_rules_recordable": participant.meta.get("site_rules") == "present"
            or bool(counted["site"]),
        }
        if other_policy_until is not None:
            cohort.corrections_span_policy_change.append(name)
        outside = any(
            count and (placement == "question_on"
                       or (kind == "blocks" and placement in ("after_day_14", "before_period")))
            for (kind, placement), count in placed_here.items()
        )
        if outside:
            cohort.corrections_with_rows_outside_period.append(name)


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


def _share(numerator: int, denominator: int) -> str:
    return f"{numerator}/{denominator}" if denominator else "not computable (denominator is 0)"


def _censoring_verdict(censored: int, measured: int) -> tuple[str, bool]:
    """(verdict, reportable) under the 2026-08-21 thresholds."""
    if measured == 0:
        return "not computable: no eligible point has an outcome row", False
    share = censored / measured
    if share > CENSORED_INSUFFICIENT_SHARE:
        return (
            f"above {CENSORED_INSUFFICIENT_SHARE:.0%} of eligible points censored: "
            "INSUFFICIENT, reported as counts and not as an estimate",
            False,
        )
    if share > CENSORED_REPORT_SHARE:
        return (
            f"above {CENSORED_REPORT_SHARE:.0%} of eligible points censored: the censored "
            "count is reported beside every result",
            True,
        )
    return f"at or below {CENSORED_REPORT_SHARE:.0%} of eligible points censored", True


def _time_to_return(seconds: list[int | None], estimate: bool) -> dict:
    """Time to sustained return on uncensored points. None is no run inside the
    horizon, which ends follow-up at 600 s: right-censored, never a failure.
    Without `estimate` (censoring above 25%) only the counts are given."""
    horizon = PRIMARY_HORIZON_SECONDS - SUSTAINED_RUN_SECONDS
    reached = sorted(s for s in seconds if s is not None)
    n = len(seconds)
    if not estimate:
        return {
            "points": n,
            "reached_within_horizon": len(reached),
            "not_reached_within_horizon": n - len(reached),
            "median_seconds": None,
            "restricted_mean_seconds": None,
            "withheld": "censoring above 25%: counts only, no estimate",
        }
    median = None
    # Kaplan-Meier median: every point is followed to the same 600 s, so it is
    # the first time by which half of the points have returned.
    for count, value in enumerate(reached, start=1):
        if 2 * count >= n:
            median = value
            break
    return {
        "definition": (
            "Seconds from the decision point to the start of the first unbroken run "
            f"of at least {SUSTAINED_RUN_SECONDS} confident anchor seconds inside the "
            f"horizon (so it starts within {horizon} s). A point with no such run is "
            f"followed to {horizon} s and has not returned by then."
        ),
        "points": n,
        "reached_within_horizon": len(reached),
        "not_reached_within_horizon": n - len(reached),
        "median_seconds": median,
        "median_note": (
            None if median is not None or n == 0
            else f"fewer than half returned within {horizon} s; the median is above {horizon} s"
        ),
        "restricted_mean_seconds": (
            round(sum(horizon if s is None else s for s in seconds) / n, 1) if n else None
        ),
        "restricted_mean_note": f"the mean of min(time, {horizon} s)",
    }


def _primary(cohort: Cohort) -> dict:
    # What the decisions file alone shows: a floor on censoring, and the check
    # the outcome rows are held to (`_outcome_disagreement`).
    visible = Counter()
    not_visible = 0
    for point in cohort.eligible:
        row, meta = point["row"], point["meta"]
        horizon_end = (_int(row, "occurred_at") or 0) + PRIMARY_HORIZON_SECONDS
        ended = _int(row, "block_ended_at")
        exported = _int(meta, "exported_at")
        if ended is not None:
            if ended < horizon_end:
                visible["block ended before the horizon elapsed"] += 1
                continue
        elif exported is None:
            visible["block still open and export time unknown"] += 1
            continue
        elif exported < horizon_end:
            visible["the export ends inside the horizon"] += 1
            continue
        not_visible += 1

    eligible = len(cohort.eligible)
    not_measurable = Counter(p["no_outcome"] for p in cohort.eligible if p["no_outcome"])
    measured = [p["outcome"] for p in cohort.eligible if p["outcome"] is not None]
    censored = Counter(_text(o, "censor_reason") for o in measured)
    gap_causes = Counter(
        _text(o, "observer_gap_cause") for o in measured
        if _text(o, "censor_reason") == "observer_gap"
    )
    uncensored = [o for o in measured if _text(o, "censor_reason") == "none"]
    censored_total = len(measured) - len(uncensored)
    verdict, reportable = _censoring_verdict(censored_total, len(measured))
    departure_categories = Counter(
        _text(o, "departure_category") or "none (at the anchor, or no evidence yet)" for o in measured
    )
    return {
        "status": (
            "COMPUTED from the per-decision outcomes file, which the exporter works out "
            "on each tester's Mac (export format 4)."
            if measured else
            "NOT MEASURABLE from these exports: no eligible point has an outcome row. "
            "Exports from before export format 4 carry no outcomes file."
        ),
        "definition": (
            "Sustained anchor engagement: at each eligible decision point, whether "
            f"at least {PRIMARY_THRESHOLD_SECONDS} of the following "
            f"{PRIMARY_HORIZON_SECONDS} seconds were spent in the anchor category "
            "recorded on the decision row, counting confident seconds only."
        ),
        "eligible_decision_points": eligible,
        "eligible_definition": (
            f"policy_version {ANALYSED_POLICY_VERSION} decision-log rows with gate_verdict 'offered', in "
            "blocks inside their Mac's v6 primary period, after "
            "the warm-up exclusion. With propensity fixed at 1.0 the shipped "
            "gate_verdict CHECK has no eligible-but-silent value, so the eligible "
            "set and the offered set are the same (2026-08-31 correction)."
        ),
        "not_measurable": dict(sorted(not_measurable.items())),
        "with_outcome_row": len(measured),
        "censored": {reason: censored[reason] for reason in OUTCOME_CENSOR_REASONS},
        "observer_gap_by_cause": {cause: gap_causes[cause] for cause in OBSERVER_GAP_CAUSES},
        "observer_gap_by_cause_note": (
            "Sub-counts of observer_gap, not censoring reasons of their own: the "
            "2026-08-21 closed set is unchanged. final_dwell is the dwell a block ended "
            "in, which the ledger never measures; pause is the person's pause or sleep; "
            "other is a restart, or a gap the export cannot attribute to a pause. A "
            "stretch where collection stopped while the block stayed active and nothing "
            "paused, slept, restarted or ended it is not a gap at all: it counts as "
            "observed, and how often that happens is unknown."
        ),
        "associational": ASSOCIATIONAL_STATEMENT,
        "censored_total": censored_total,
        "censored_share": _share(censored_total, len(measured)),
        "censoring_verdict": verdict,
        "reportable": reportable and bool(uncensored),
        "numerator": (
            sum(1 for o in uncensored if _text(o, "sustained_anchor_900s") == "1")
            if measured else None
        ),
        "denominator": len(uncensored),
        "secondary": {
            "departure_free_600s": {
                "definition": (
                    "No further departure from the anchor, by the gate's own rule, "
                    f"beginning within {DEPARTURE_FREE_SECONDS} s of the decision point. "
                    "A person who left and never came back has no further departure: "
                    "read it beside the primary outcome, never alone."
                ),
                "numerator": sum(1 for o in uncensored if _text(o, "departure_free_600s") == "1"),
                "denominator": len(uncensored),
            },
            "time_to_sustained_return": _time_to_return(
                [_int(o, "seconds_to_sustained_return") for o in uncensored],
                estimate=reportable,
            ),
        },
        "departure_category_counts": dict(sorted(departure_categories.items())),
        "departure_category_note": (
            "Where the gate's evidence had the person at each eligible decision point "
            "with an outcome row, censored or not. Marginal counts only: this script "
            "prints no outcome broken down by category or any other context."
        ),
        "censored_visible_in_export": dict(sorted(visible.items())),
        "not_censored_by_block_or_export_end": not_visible,
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
        if cohort.blocks_outside_period.get(name):
            entry["blocks_outside_their_v6_period"] = cohort.blocks_outside_period[name]
    measured = sorted(per_participant)
    result: dict = {
        "definition": (
            "Gate D measure 1: distinct declared work blocks per participant per "
            "cohort week, from the per-block CSV. Every declared block counts, "
            "whether or not it produced an offer; no warm-up exclusion applies. "
            "Only blocks inside their Mac's v6 primary period count (the v6 "
            "primary-period amendment, item 6); a block outside it is not counted "
            "and is reported beside the figure, and weeks after a Mac's period "
            "still count in the denominator."
        ),
        "blocks_outside_their_v6_period": sum(cohort.blocks_outside_period.values()),
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
            p.name for p in cohort.analysed
            if p.name not in cohort.invitations and p.period.start is not None
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
            p.name for p in cohort.analysed
            if p.name not in cohort.explain and p.period.start is not None
        ),
    }


def _corrections(cohort: Cohort) -> dict:
    per_participant = {}
    by_category = Counter()
    site_by_category = Counter()
    for name in sorted(cohort.corrections):
        app = cohort.corrections[name]["app"]
        site = cohort.corrections[name]["site"]
        window = cohort.corrections[name]["window"]
        app_count = sum(c for _, c in app.values())
        site_count = sum(c for _, c in site.values())
        per_participant[name] = {
            "corrections": app_count + site_count,
            "app_scoped_corrections": app_count,
            "site_scoped_corrections": site_count,
            "site_rules_recordable": cohort.corrections[name]["site_rules_recordable"],
            "applications_with_an_app_rule": sum(r for r, _ in app.values()),
            "sites_with_a_site_rule": sum(r for r, _ in site.values()),
            "window_rules": sum(window.values()),
            "app_scoped_corrections_by_category": {
                category: c for category, (_, c) in sorted(app.items())
            },
            "site_scoped_corrections_by_category": {
                category: c for category, (_, c) in sorted(site.items())
            },
        }
        for category, (_, c) in app.items():
            by_category[category] += c
        for category, (_, c) in site.items():
            site_by_category[category] += c
    measured = sorted(per_participant)
    return {
        "definition": (
            "2026-08-17 addition, measure 3: the count of classification corrections "
            "per participant that a rule keeps a count of, app-scoped and, from "
            "1.0.14, site-scoped. Reported as a count only. It measures how far the "
            "seed dictionary missed that person's apps and sites: not engagement, "
            "and not to be presented as such."
        ),
        "how_counted": (
            "The sum of correction_count over the participant's app-scoped rules "
            "(personal_app_override, migration 0017), from the corrections CSV. A "
            "rule starts at 1 and every later correction that lands on the same "
            "application adds 1, whether it came from correcting an activity or "
            "from the list of apps Velvt could not read. Plus the same sum over the "
            "site-scoped rules (personal_site_override, migration 0040), which "
            "count the same way for a site in every browser. Applications and "
            "sites with a rule, and window rules (personal_override, which keeps "
            "no count), are descriptive. An export from a database before 0040 "
            "cannot hold a site rule; its site count is 0 by construction and it "
            "is listed under site_rules_not_recordable."
        ),
        "lower_bound": (
            "Only rules that exist at export are counted. A correction the "
            "participant removed, or a Reset, takes its count with it."
        ),
        "not_counted": (
            "The 'Wrong category' reply to a drift offer is block-scoped "
            "(work_block_category_correction), not a classification rule. It is "
            "counted in the trust figure as wrong_classification. Before 1.0.14 a "
            "correction of a browser tab was window-scoped only and appears under "
            "window rules."
        ),
        "measurable": bool(measured),
        "participants_measured": len(measured),
        "per_participant": per_participant,
        "participants_with_zero_app_scoped_corrections": [
            n for n in measured if not per_participant[n]["app_scoped_corrections"]
        ],
        "participants_with_zero_corrections": [
            n for n in measured if not per_participant[n]["corrections"]
        ],
        "total_corrections": sum(by_category.values()) + sum(site_by_category.values()),
        "total_app_scoped_corrections": sum(by_category.values()),
        "app_scoped_corrections_by_category": dict(sorted(by_category.items())),
        "total_site_scoped_corrections": sum(site_by_category.values()),
        "site_scoped_corrections_by_category": dict(sorted(site_by_category.items())),
        "site_rules_not_recordable": [
            n for n in measured if not per_participant[n]["site_rules_recordable"]
        ],
        "includes_history_before_analysed_policy": sorted(cohort.corrections_span_policy_change),
        "includes_rows_outside_the_v6_period": sorted(cohort.corrections_with_rows_outside_period),
        "not_measurable_for": sorted(
            p.name for p in cohort.analysed
            if p.name not in cohort.corrections and p.period.start is not None
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


def _period_report(cohort: Cohort) -> dict:
    per_participant = {}
    for name, period in sorted(cohort.periods.items()):
        per_participant[name] = {
            "start": period.start,
            "end": period.end,
            "upgrade_cut": period.upgrade_cut,
            "stated_start": period.stated_start,
            "rating_prompt_enabled_at": period.rating_prompt_enabled_at,
        }
    by_kind = {
        kind: {
            placement: cohort.placed.get(kind, Counter())[placement]
            for placement in PLACEMENTS + ("straddles",)
            if cohort.placed.get(kind, Counter())[placement]
        }
        for kind in PLACED_KINDS
    }
    question_on_inside = {
        kind: cohort.question_on_inside_period[kind]
        for kind in PLACED_KINDS if cohort.question_on_inside_period[kind]
    }
    rated_inside = dict(sorted(cohort.rated_blocks_inside_period.items()))
    return {
        "status": (
            "The v6 primary-period amendment, drafted 2026-10-17 after the founder fixed "
            "the period at 14 days. Pending: it applies once the founder appends it to "
            "traction-summary.md, before the first wave-1 link."
        ),
        "definition": (
            "Each Mac's v6 primary period starts at the started_at of its first block "
            "after the upgrade cut (its last decision row with a policy_version below 6), "
            f"and lasts {V6_PRIMARY_PERIOD_SECONDS} s (14 days). Every figure in this "
            "report counts rows inside it: a block by its start, a decision point and an "
            "offer by their block's start, an invitation by its offered_at, and a local "
            "week only when the week widened by a day at each end lies on one side of "
            "every boundary. Rows after day 14, and rows at or after the Mac's "
            "rating_prompt_enabled_at, are separate descriptive strata, never pooled."
        ),
        "period_seconds": V6_PRIMARY_PERIOD_SECONDS,
        "per_participant": per_participant,
        "participants_without_a_period": sorted(
            name for name, period in cohort.periods.items() if period.start is None
        ),
        "rows_by_placement": by_kind,
        "placement_labels": PLACEMENT_LABELS,
        "protocol_deviations": {
            "definition": (
                "Item 5: a row at or after the Mac's rating_prompt_enabled_at that is "
                "inside its period, and a block inside its period that has a row in the "
                "export's ratings file. The app refuses both (the question cannot be "
                "turned on, or asked about a block, before day 14), so each one is "
                "counted and reported."
            ),
            "rows_at_or_after_rating_prompt_enabled_at_inside_the_period": question_on_inside,
            "rated_blocks_inside_the_period": rated_inside,
            "total": sum(question_on_inside.values()) + sum(rated_inside.values()),
        },
        "problems": list(cohort.period_problems),
    }


STRATUM_DEFINITIONS = {
    "after_day_14": (
        "Rows at or after the end of their Mac's v6 primary period, and before its "
        "rating_prompt_enabled_at if it has one (amendment item 4)."
    ),
    "question_on": (
        "Rows at or after the moment their Mac first turned on the question after a "
        "block, inside the period or after it (amendment item 5)."
    ),
}


def _stratum_summary(stratum: Cohort, name: str) -> dict:
    """A descriptive stratum, under the same definitions as the figures."""
    result = analyse(stratum)
    primary = result["primary_outcome"]
    free = primary["secondary"]["departure_free_600s"]
    invitations = result["invitation_acceptance"]
    explain = result["explain_tap_rate"]
    return {
        "status": (
            "DESCRIPTIVE STRATUM. Counted under the same definitions as the figures "
            "above, and never pooled with them, with the other stratum, or into the "
            "power count."
        ),
        "definition": STRATUM_DEFINITIONS[name],
        "decision_log_rows": result["decision_log_integrity"]["rows"],
        "eligible_decision_points": primary["eligible_decision_points"],
        "with_outcome_row": primary["with_outcome_row"],
        "censored": primary["censored"],
        "sustained_anchor_900s": {
            "numerator": primary["numerator"], "denominator": primary["denominator"],
        },
        "departure_free_600s": {
            "numerator": free["numerator"], "denominator": free["denominator"],
        },
        "decisions_recorded": {
            "delivered": result["decisions_recorded"]["delivered"],
            "withheld": result["decisions_recorded"]["withheld"],
        },
        "trust": {
            key: result["trust"][key]
            for key in ("was_focused", "wrong_classification", "combined_numerator", "denominator")
        },
        "retired_return_within_10min": {
            "numerator": result["retired_return_within_10min"]["numerator"],
            "denominator": result["retired_return_within_10min"]["denominator"],
        },
        "withheld_by_outcome": result["withheld"]["by_outcome"],
        "blocks_declared": {
            participant: entry["blocks_declared"]
            for participant, entry in result["blocks_per_participant"]["per_participant"].items()
        },
        "completion_by_origin": result["completion_by_origin"]["by_origin"],
        "invitations": {
            "accepted": invitations["accepted"],
            "terminal": invitations["terminal"],
            "by_outcome": invitations["by_outcome"],
        },
        "explain": {
            "participant_weeks": len(explain["participant_weeks"]),
            "taps": explain["taps"],
            "delivered_interventions": explain["delivered_interventions"],
        },
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

    by_delivery: dict[str, dict] = {}
    for bucket in DELIVERY_BUCKETS:
        subset = [o for o in offers if o.notification_delivery == bucket]
        by_delivery[bucket] = {
            "offers": len(subset),
            "returned_within_10min": sum(1 for o in subset if o.returned_within_window),
            "no_response": sum(1 for o in subset if o.outcome == "no_response"),
        }
    posted_visible = sum(by_delivery[b]["offers"] for b in DELIVERY_POSTED_VISIBLE)
    unrecorded = by_delivery["not_reported"]["offers"] + by_delivery["unknown"]["offers"]

    by_focus: dict[str, dict] = {}
    for bucket in FOCUS_STATE_BUCKETS:
        subset = [o for o in offers if o.focus_state == bucket]
        by_focus[bucket] = {
            "offers": len(subset),
            "posted_with_visible_alert_style": sum(
                1 for o in subset if o.notification_delivery in DELIVERY_POSTED_VISIBLE
            ),
            "returned_within_10min": sum(1 for o in subset if o.returned_within_window),
            "no_response": sum(1 for o in subset if o.outcome == "no_response"),
            "withheld": dict(sorted(Counter(
                o.outcome for o in withheld if o.focus_state == bucket
            ).items())),
        }

    withheld_counts = Counter(o.outcome for o in withheld)
    eligible = len(cohort.eligible)
    primary = _primary(cohort)
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
    # Rows outside each Mac's v6 primary period (the 2026-10-17 amendment).
    for kind in PLACED_KINDS:
        for placement, count in cohort.placed.get(kind, Counter()).items():
            if placement != "in_period" and count:
                excluded_reasons[
                    f"{kind.replace('_', ' ')} {PLACEMENT_LABELS[placement]}"
                ] += count

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
                "2026-09-25 amendment and 2026-09-26/27/10-07 notes: policy_version 1 to "
                "6 are never pooled and every result uses policy_version "
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
        "v6_primary_period": _period_report(cohort),
        "power": {
            "required_decision_points": POWER_REQUIRED_DECISION_POINTS,
            "observed_eligible_decision_points": eligible,
            "observed_uncensored_decision_points": primary["denominator"],
            "assumptions": POWER_ASSUMPTIONS,
            "sufficient": eligible >= POWER_REQUIRED_DECISION_POINTS,
            "note": (
                f"Counts policy_version {ANALYSED_POLICY_VERSION} eligible decision points inside "
                "each Mac's v6 primary period only (amendment item 3), before censoring, so it "
                "is an upper bound on how close this data gets."
            ),
        },
        "primary_outcome": primary,
        "retired_return_within_10min": {
            "status": (
                "DESCRIPTIVE. Pre-registered 2026-08-09, retired 2026-08-21, "
                "retained so results stay comparable across that change. Not the "
                "primary outcome and never the headline."
            ),
            "definition": (
                "Of drift interventions delivered, the fraction whose recorded "
                "outcome is a return to the anchor category within 10 minutes, "
                "recorded while the offer was still unanswered. An answered offer "
                "is terminal and never records a return, so a person who answered "
                "(for example 'Back to work') and then returned is not counted."
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
        "notification_delivery": {
            "status": (
                "DESCRIPTIVE. An additional breakdown of the delivered offers; it "
                "changes no pre-registered denominator, which still counts every "
                "delivered offer whatever became of its notification."
            ),
            "definition": (
                "From delivery_channel (migration 0043, protocol 35): what the app "
                "reported became of each offer's OS notification. "
                "notification_posted: posted with an alert style that shows it (banner "
                "or alert). Posted is not seen: a Focus mode Velvt could not detect "
                "still holds it in Notification Center, so this is an upper bound on "
                "offers seen. alert_style_none: posted, listed in Notification Center, "
                "never shown. permission_denied / permission_unknown: notifications off "
                "or never answered, nothing posted. rejected: the notification centre "
                "refused it. withdrawn: the offer was over before the notification "
                "centre had it. quiet: in-app card only, by design. stale: not posted, "
                "because the app first saw the offer more than 2 minutes after the "
                "evidence it was decided on. not_reported: no report on a Mac that "
                "records them. unknown: recorded before migration 0043. Never folded "
                "into not_reported."
            ),
            "by_channel": by_delivery,
            "posted_with_visible_alert_style": posted_visible,
            "not_posted_with_visible_alert_style": denominator - posted_visible - unrecorded,
            "not_recorded": unrecorded,
            "delivered": denominator,
        },
        "focus_state_at_decision": {
            "status": (
                "DESCRIPTIVE. An additional breakdown of the delivered and withheld "
                "offers; it changes no pre-registered denominator, which still counts "
                "every delivered offer whatever Velvt knew about Focus."
            ),
            "definition": (
                "From focus_state on the decision-log row that attributes each offer "
                "(migration 0044, protocol 36): what the service knew about Focus when "
                "the gate decided. on / off: the app could read Focus status and the "
                "record said a Focus was / was not on. unknown: the app could not read "
                "it (no Communication Notifications entitlement, Focus status denied "
                "or never answered), had not yet said whether it could since the "
                "helper started, or no Focus state was recorded yet, so a Focus may "
                "have silenced a delivered offer: "
                "its delivery is unknown, and posted_with_visible_alert_style there is "
                "an upper bound on offers seen. not_recorded: decided before migration "
                "0044, or an export without the column. Never folded into off."
            ),
            "by_focus_state": by_focus,
            "delivered": denominator,
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
                f"is under {WARMUP_EXCLUSION_SECONDS}s, below the policy v2 to v6 warm-up "
                "(2026-09-25; was 300 s)",
                "the founder's own device",
                "any participant who reinstalled mid-cohort (local history resets with the database)",
                f"rows not attributable to policy_version {ANALYSED_POLICY_VERSION}, and "
                "exports without the decision log (2026-09-25, 2026-09-26)",
                "rows outside each Mac's v6 primary period, its first 14 days (the v6 "
                "primary-period amendment, pending): rows after day 14 and rows at or after "
                "rating_prompt_enabled_at are reported as separate strata, never pooled",
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
        "strata": {
            name: _stratum_summary(stratum, name) for name, stratum in cohort.strata.items()
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

    period = result["v6_primary_period"]
    heading("V6 PRIMARY PERIOD (each Mac's first 14 days)")
    para(period["definition"])
    para(period["status"])
    for name, entry in period["per_participant"].items():
        if entry["start"] is None:
            add(f"  {name:24} no period: no block after the upgrade cut")
            continue
        enabled = entry["rating_prompt_enabled_at"]
        add(f"  {name:24} {_when(entry['start'])} to {_when(entry['end'])}; question first on: "
            f"{_when(enabled) if enabled is not None else 'never'}")
    if period["participants_without_a_period"]:
        para(f"No period, so in no figure but blocks declared (Gate D measure 1, as zero): "
             f"{', '.join(period['participants_without_a_period'])}.")
    add(f"  rows by where they count ({', '.join(PLACEMENTS[:3])}; the rest in no figure):")
    for kind, counts in period["rows_by_placement"].items():
        shown = ", ".join(f"{placement} {count}" for placement, count in counts.items())
        add(f"    {kind.replace('_', ' '):16} {shown or 'none'}")
    deviations = period["protocol_deviations"]
    add(f"  protocol deviations (item 5): {deviations['total']}")
    if deviations["rows_at_or_after_rating_prompt_enabled_at_inside_the_period"]:
        add(f"    rows at or after rating_prompt_enabled_at inside the period: "
            f"{deviations['rows_at_or_after_rating_prompt_enabled_at_inside_the_period']}")
    if deviations["rated_blocks_inside_the_period"]:
        add(f"    blocks inside the period with a rating row: "
            f"{deviations['rated_blocks_inside_the_period']}")
    for problem in period["problems"]:
        add(f"  ! {problem}")

    power = result["power"]
    heading("POWER")
    add(f"  Detecting the pre-registered effect needs about "
        f"{power['required_decision_points']} eligible decision")
    add("  points, under assumptions none of which are measured:")
    para(f"{power['assumptions']}.", "    ")
    add(f"  This data carries {power['observed_eligible_decision_points']} "
        f"(policy_version {ANALYSED_POLICY_VERSION}, before censoring), "
        f"{power['observed_uncensored_decision_points']} with an uncensored outcome.")
    if not power["sufficient"]:
        add("  Every ratio below is reported for completeness and is not a finding.")

    primary = result["primary_outcome"]
    heading("PRIMARY OUTCOME: sustained anchor engagement (2026-08-21)")
    para(primary["definition"])
    add(f"  eligible decision points (policy_version {ANALYSED_POLICY_VERSION}, before censoring): "
        f"{primary['eligible_decision_points']}")
    for reason, count in primary["not_measurable"].items():
        add(f"  not measurable, {reason}: {count}")
    add(f"  with an outcome row: {primary['with_outcome_row']}")
    if not primary["with_outcome_row"]:
        para(f"NOT MEASURABLE. {primary['status']} Nothing is approximated, and "
             "anchor_seen_within_600s is not a substitute.")
    else:
        censored = primary["censored"]
        add(f"  censored: {primary['censored_share']} "
            f"(block_ended {censored['block_ended']}, observer_gap {censored['observer_gap']}, "
            f"export_ended {censored['export_ended']}); never counted as 0, never imputed")
        causes = primary["observer_gap_by_cause"]
        add(f"    observer_gap by cause: final dwell never measured {causes['final_dwell']}, "
            f"pause (yours, or sleep) {causes['pause']}, other (a restart, or not "
            f"attributable) {causes['other']}")
        para("A sub-count, not a censoring reason: the closed set is unchanged. Collection "
             "that stopped while the block stayed active and nothing paused, slept, "
             "restarted or ended it leaves no gap and counts as observed; how often is "
             "unknown.", "    ")
        para(primary["censoring_verdict"] + ".")
        beside = ""
        if primary["censored_total"] and primary["censoring_verdict"].startswith("above"):
            beside = f"; censored {primary['censored_share']}"
        # Beside every result, after the censored count: 2026-08-21.
        beside += f"; {ASSOCIATIONAL_TAG}"
        secondary = primary["secondary"]
        free = secondary["departure_free_600s"]
        ret = secondary["time_to_sustained_return"]
        if primary["reportable"]:
            add(f"  sustained: {_ratio(primary['numerator'], primary['denominator'])}{beside}")
            add(f"  secondary, departure-free 600 s: "
                f"{_ratio(free['numerator'], free['denominator'])}{beside}")
        else:
            add("  INSUFFICIENT, not an estimate:")
            add(f"    sustained at {primary['numerator']} of {primary['denominator']} "
                f"uncensored point(s){beside}")
            add(f"    departure-free 600 s at {free['numerator']} of {free['denominator']}"
                f"{beside}")
        para(free["definition"], "    ")
        if "withheld" in ret:
            add(f"    returned to a sustained run within the horizon at "
                f"{ret['reached_within_horizon']} of {ret['points']}{beside}")
        else:
            median = (f"{ret['median_seconds']} s" if ret["median_seconds"] is not None
                      else (ret["median_note"] or "none"))
            add(f"  secondary, time to sustained return: returned within the horizon at "
                f"{ret['reached_within_horizon']} of {ret['points']}; median {median}; "
                f"restricted mean {ret['restricted_mean_seconds']} s{beside}")
        para(primary["associational"])
        add(f"  departure category at each point (marginal counts only): "
            f"{primary['departure_category_counts'] or '{}'}")
    visible = primary["censored_visible_in_export"]
    add(f"  censoring the decisions file shows on its own: {visible or '{}'}; "
        f"not censored by block or export end: {primary['not_censored_by_block_or_export_end']}")

    retired = result["retired_return_within_10min"]
    heading("DESCRIPTIVE — RETIRED 2026-08-21, NOT THE PRIMARY OUTCOME")
    add("  Pre-registered 2026-08-09 and retired by the 2026-08-21 amendment for a")
    add("  ceiling effect readable in the drift gate's own source: the gate only")
    add("  fires on someone who has already been returning to the anchor inside")
    add("  the same ten minutes, so this measures the gate's selection rule")
    add("  rather than what the intervention changed. Never the headline.")
    para(retired["definition"])
    add(f"  returned within {retired['window_seconds']}s, unanswered offers only: "
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

    delivery = result["notification_delivery"]
    heading("NOTIFICATION DELIVERY (descriptive, protocol 35)")
    add("  What became of each delivered offer's OS notification. Changes no")
    add("  denominator above: every delivered offer stays in them.")
    for bucket in DELIVERY_BUCKETS:
        stats = delivery["by_channel"][bucket]
        add(f"  {bucket:20} offers={stats['offers']:4} "
            f"returned_unanswered={stats['returned_within_10min']:4} "
            f"no_response={stats['no_response']:4}")
    add(f"  posted with a visible alert style: {delivery['posted_with_visible_alert_style']}, "
        f"not: {delivery['not_posted_with_visible_alert_style']}, "
        f"not recorded: {delivery['not_recorded']}, of delivered={delivery['delivered']}")
    add("  Posted is not seen: a Focus mode Velvt could not detect holds a posted")
    add("  notification in Notification Center, so the first figure is an upper")
    add("  bound on offers seen.")

    focus = result["focus_state_at_decision"]
    heading("OFFERS BY FOCUS STATE AT DECISION (descriptive, protocol 36)")
    add("  What Velvt knew about Focus when each offer was decided. Changes no")
    add("  denominator above. unknown: Velvt could not read Focus status, so a")
    add("  Focus may have silenced the offer: treat its delivery as unknown.")
    for bucket in FOCUS_STATE_BUCKETS:
        stats = focus["by_focus_state"][bucket]
        withheld_here = sum(stats["withheld"].values())
        add(f"  {bucket:12} delivered={stats['offers']:4} "
            f"posted_visible={stats['posted_with_visible_alert_style']:4} "
            f"returned_unanswered={stats['returned_within_10min']:4} "
            f"no_response={stats['no_response']:4} withheld={withheld_here:4}")

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
            f"returned_unanswered={stats['returned_within_10min']:4} "
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
    add(f"  blocks outside their Mac's v6 period, not counted: "
        f"{blocks['blocks_outside_their_v6_period']}")
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
            site_line = (
                f"{entry['site_scoped_corrections']} site-scoped correction(s) "
                f"on {entry['sites_with_a_site_rule']} site(s)"
                if entry["site_rules_recordable"]
                else "site rules not recordable (database before migration 0040)"
            )
            add(f"  {'':24} {site_line}; {entry['corrections']} correction(s) in all")
        add(f"  app-scoped corrections by category: "
            f"{corrections['app_scoped_corrections_by_category'] or '{}'}")
        add(f"  site-scoped corrections by category: "
            f"{corrections['site_scoped_corrections_by_category'] or '{}'}")
        if corrections["site_rules_not_recordable"]:
            add(f"  site rules not recordable: {corrections['site_rules_not_recordable']}")
        para(f"Each count is a lower bound. {corrections['lower_bound']}")
    else:
        add("  not measurable from the cohort export (no corrections file)")
    if corrections["not_measurable_for"]:
        add(f"  no corrections file: {corrections['not_measurable_for']}")
    if corrections["includes_history_before_analysed_policy"]:
        add(f"  ! counts include time before policy v{ANALYSED_POLICY_VERSION}, not separable: "
            f"{corrections['includes_history_before_analysed_policy']}")
    if corrections["includes_rows_outside_the_v6_period"]:
        add(f"  ! counts carry no time and are reported whole; these exports hold rows "
            f"outside the v6 period: {corrections['includes_rows_outside_the_v6_period']}")

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

    for name, stratum in result["strata"].items():
        title = {"after_day_14": "AFTER DAY 14", "question_on": "QUESTION ON"}[name]
        heading(f"{title} (descriptive stratum, never pooled)")
        para(stratum["definition"])
        para(stratum["status"])
        censored = stratum["censored"]
        sustained = stratum["sustained_anchor_900s"]
        free = stratum["departure_free_600s"]
        recorded = stratum["decisions_recorded"]
        trust = stratum["trust"]
        retired = stratum["retired_return_within_10min"]
        invitations = stratum["invitations"]
        explain = stratum["explain"]
        add(f"  decision-log rows: {stratum['decision_log_rows']}; eligible decision points: "
            f"{stratum['eligible_decision_points']}, {stratum['with_outcome_row']} with an outcome row")
        add(f"  censored: block_ended {censored['block_ended']}, observer_gap "
            f"{censored['observer_gap']}, export_ended {censored['export_ended']}")
        add(f"  sustained anchor 900 s: {sustained['numerator'] or 0} of {sustained['denominator']} "
            f"uncensored point(s); departure-free 600 s: {free['numerator']} of {free['denominator']}; "
            f"{ASSOCIATIONAL_TAG}")
        add(f"  offers delivered: {recorded['delivered']}, withheld: {recorded['withheld']} "
            f"{stratum['withheld_by_outcome'] or ''}".rstrip())
        add(f"  was_focused {trust['was_focused']}, wrong_classification "
            f"{trust['wrong_classification']}, of {trust['denominator']} delivered; returned "
            f"within 600 s unanswered: {retired['numerator']} of {retired['denominator']}")
        add(f"  blocks declared: {sum(stratum['blocks_declared'].values())} "
            f"{stratum['blocks_declared'] or ''}".rstrip())
        add(f"  invitations accepted: {invitations['accepted']} of {invitations['terminal']} "
            f"terminal; explain weeks: {explain['participant_weeks']} (taps {explain['taps']} "
            f"over {explain['delivered_interventions']} delivered)")

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


def _when(timestamp: int) -> str:
    return datetime.fromtimestamp(timestamp, tz=timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


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
