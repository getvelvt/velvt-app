# The personal return ledger, and the path to a personal nudge rule

Status, 2026-09-27: **shadow only.** Nothing described here changes what a
tester sees or when a nudge fires. Drift policy v5 ships unchanged.

The founder asked for a model built from each person's own habits that shapes
the focus-block nudge. The honest order is: record the right data now, count it
per person, randomize (v6), and let a personal rule act only if it beats the
fixed one on randomized data (v7). This page describes what exists for the first
two steps and what the last two need.

**Governance.** `AGENTS.md` (Scope Boundary) gates new work in `src/behavior/`
on Gate D or a dated founder decision. This work follows the founder's request
of 2026-09-27. Before it merges, the founder still needs to add a dated, scoped
entry to `plan/README.md`: shadow-only code, no caller from the gate, delivery,
IPC or copy, and A4 randomization still parked.

## What exists

| Piece | Where | What it does |
|---|---|---|
| Feature contract 3 | `rust-service/src/behavior/features.rs` | Records that v5 changed what `c_t` and `q_t` observe for browser tabs. Rows cut under contracts 2 and 3 are never pooled. |
| Return ledger | `rust-service/src/behavior/returns.rs` | Per-person counts of returns to the anchor, by context cell. Details below. |
| Synthetic suite E | `scripts/generate_traces.py`, `scripts/traces/SYNTHETIC-suite-e-returns.jsonl`, `rust-service/tests/trace_replay.rs` | 116 synthetic people replayed through the real `WorkBlockManager`, then scored by the ledger. |
| Design simulator | `scripts/simulate_nudge_designs.py` | How many weeks v6 and a per-person rule need, under stated assumptions. |

### The return ledger

**Rows.** A row is a departure the gate saw and did not act on: a decision
logged with an anchor, made on a confident non-anchor observation whose previous
confident observation was that anchor. This is `s_t`, the gate's own rule.

**Label.** The pre-registered primary outcome: at least 600 of the next 900
seconds spent in confident observations of the anchor recorded on the decision
row.
- **Censored**, never counted as a failure: the block ended inside the 900
  seconds, or the observation ledger has a gap in them.
- **Treated**, and excluded: an offer, delivered or held, falls at or before the
  end of the 900 seconds.

**Cells.** Nine frozen cells in three dimensions:
- the kind of departure: communication, feeds and video, or work-adjacent;
- the third of the block it happened in;
- the part of the local day: morning, afternoon, or evening and night.

**Estimates.** Each cell is a Beta-Binomial posterior.
- Prior: Beta(4, 4) on the person's overall rate. Each cell's prior is four
  pseudo-departures at that overall rate.
- Rows from one block are down-weighted for an assumed within-block correlation
  of 0.3.
- Blocks the person disputed ("Wrong category") count at half weight.

**Abstention.** The ledger declines to answer below 6 contributing blocks or 30
resolved rows. It reads only blocks that closed in the 28 days before `as_of`,
and only rows from one drift policy version.

**When a cell is surfaced.** A cell is reported as different from the rest of
its dimension only if both of these hold:
- on the earlier 60% of blocks, a family-wise 80% interval (Bonferroni over all
  nine cells) excludes no difference;
- on the later 40% of blocks, the same direction repeats at one-sided 90%.

**Versioning.** Every result carries `RETURN_LEDGER_MODEL_VERSION = 1`, the
feature contract version and the policy version.

**`would_withhold`.** A candidate rule declared in advance, for offline
evaluation only.
- It fires only where, in each of the point's three cells, the lower end of the
  80% interval is at least 0.60.
- It can only ever describe removing a nudge, never adding one.

**Purity.** The ledger is a set of pure functions over rows passed in. It has no
database handle, reads no clock, uses no randomness and stores no state.

### Shadow only, enforced

- `nothing_in_the_shipped_path_calls_the_return_ledger` fails the build if any
  file names the module:
  - anything in `rust-service/src`, including the gate, delivery, the IPC
    router, receipts and `main.rs`;
  - the shared types;
  - the Swift client;
  - the IPC schema.
- `behavior` is declared in `main.rs`, so nothing in the library crate can reach
  it.
- The ledger has no user-facing strings, and `scripts/check_banned_copy.py`
  passes.
- The one change outside `behavior/` makes `is_confident` public, so the ledger
  reads `q_t` through the gate's own function. It changes no decision.

## What it can and cannot say before randomization

**It can say** how often this person spent most of the next fifteen minutes back
at their work on their own, after a departure the gate did not act on. It gives
this per cell, as raw counts ("4 of 11") beside an estimate. When the evidence is
thin it says so.

**It cannot say anything about what a nudge does, or what silence would do.**
Under v5 every eligible point is offered (propensity 1.0).
- So every row the ledger counts is a departure the gate did not find eligible.
  In practice that is the first or second switch inside ten minutes.
- The held rows (Focus/DND, demotion) are the only v5 points with an eligible
  departure and no delivery. They are excluded, because the states that held
  them also shape what happens next.
- Suite E checks this on the gate's own rows:
  - every counted row is below the switch threshold;
  - at all 597 offered points, `would_withhold` reports
    `Support::Extrapolated`.

`plan/06` already says rows like these can never support an offer-versus-silence
comparison, "at any sample size". Nothing here changes that. The withhold
candidate is written down now so it can be evaluated honestly later, on
randomized rows. It must never be applied at an offered point.

**No causal copy.** The ledger's counts describe associations. Turning one into
advice ("switch off chat") is a causal claim (`03-BEHAVIORAL-ENGINE-SPEC.md`,
lines 174-193). If counts ever reach a surface, they are shown as counts, without
"because".

## How much data it needs

The figures below are **synthetic**. They measure the ledger on planted rates,
not any person.

**Suite E, on the committed fixtures.** The planted pattern is 0.25 against
0.70, inside the 28-day lookback.

| Family | Result |
|---|---|
| PLANTED at 3 / 5 / 8 blocks a week | found in 3/12, 4/12, 7/12 |
| NULL (no structure) | 0/40 with the ledger's controls; 28/40 with every control off |
| SPARSE | 16/16 abstain: 8 on blocks, 8 on rows |
| REGIME | found before the change in 8/12; still found 4 weeks after it in 0/12 |
| CORRECTED | disputes counted and down-weighted; the hidden pattern found in 0/12 before the correction and 3/12 after |
| Labels | 10,864 departures matched their planted label exactly |

The thresholds were frozen while looking at this suite. One later run on a fresh
seed gave:
- NULL: 1 of 200 traces surfaced a cell;
- PLANTED: 2/40, 16/40 and 28/40;
- REGIME: 32/40 before the change and 0/40 after.

Even a large difference needs more than a block a day before four weeks of data
show it reliably.

**The v6 design.** From `./scripts/simulate_nudge_designs.py --scenarios`.

Shared assumptions, none of them measured:
- 75% of eligible points have at least 900 s left;
- 10% of drawn points are censored;
- baseline 0.30 under silence;
- the effect varies across people with SD 0.05;
- a person's nudge is withheld when the 80% upper bound of their own effect is
  below 5 points.

| Testers | Blocks/wk | Eligible/block | Effect | Points/wk | Points needed | Weeks to N | One person: weeks to see the sign | Rule precision at 26 / 52 wk |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | 3 | 0.5 | 15 pts | 8.4 | 428 | 50.6 | 91.1 | 3% / 7% |
| 10 | 3 | 1.5 | 15 pts | 18.2 | 428 | 23.4 | 42.2 | 9% / 10% |
| 10 | 3 | 0.5 | 7.5 pts | 8.4 | 2,295 | 271.8 | 357.9 | 41% / 50% |
| 10 | 3 | 1.5 | 7.5 pts | 18.2 | 2,295 | 125.9 | 165.7 | 48% / 58% |
| 40 | 3 | 0.5 | 15 pts | 33.8 | 399 | 11.8 | 91.1 | 4% / 6% |
| 40 | 3 | 1.5 | 15 pts | 72.9 | 399 | 5.5 | 42.2 | 7% / 11% |

With everyone's effect equal, the points needed are the pre-registered 390, or
about 1,495 at 7.5 points.

**Reading the table.**
- The average effect is reachable in months with 10 testers and in weeks with
  40.
- A per-person withhold rule is mostly wrong for a year at these volumes. When
  most people benefit, most of the people it flags are noise.
- The eligible-point rate is the input that matters most, and it has never been
  measured off the founder's Mac.

## The path: v5, then v6, then v7

**v5 (now, wave 1).**
- Nothing changes for testers. The ledger stays shadow.
- Wave 1 is read for rates only: eligible points per user-week, the censoring
  share, reply rates, and clustering within blocks. Those rates replace the
  simulator's assumptions.
- Nobody reads outcome-by-context tables from testers until the v6 amendment is
  on main.

**v6 (drafted, not enabled).**
- The v5 gate decides eligibility. A fixed draw decides: offer with p = 0.7,
  silence with p = 0.3.
- At most one draw per block, and only when at least 900 s remain. The seed and
  the propensity are stored on the row.
- It needs, in order:
  1. a Gate D override;
  2. a consent sentence;
  3. a hashed pre-registration amendment, appended before any v6 row exists,
     including on the founder's Macs;
  4. migration 0042 or later;
  5. a decision log that fails closed;
  6. disclosure and an opt-out.
- Planned alongside it: log the ledger's `model_version` and the withhold
  candidate's score on each decision row, still acting on nothing. That is a
  migration and a `PRIVACY.md` row of its own.

**v7 (only if v6 shows an average effect above zero).**
- The declared withhold rule is evaluated offline on v6 rows, with
  self-normalized inverse-propensity weighting and an effective-sample-size
  floor. Below the floor the answer is "not estimable".
- It ships only if all of these hold:
  - it beats fixed p;
  - the wrong-intervention rate is no worse;
  - it passes `pivot-engineering/testing/03`.
- If it ships:
  - it is off by default, with a kill switch and a per-person reset;
  - it can only lower the offer probability at points v5 already finds eligible;
  - it is a new policy version, never pooled with v5 or v6.
- A bandit would need a separate, dated reversal of the existing "do not build".

If the average effect is null or negative, the right move is fewer nudges by a
fixed rule, and no personalization.

## Privacy

**Everything stays on the Mac.** The ledger reads four tables that already
exist:
- `work_block_observation`
- `intervention_decision_log`
- `work_block_intervention`
- `work_block_category_correction`

It never reads `raw_event_buffer`, site names, application names, window titles
or intention text.

**It writes nothing.** No table, column or retention window is added, so
`PRIVACY.md` is unchanged. Clear Local Work Blocks deletes every input, so a
person's ledger is gone with them.

**Adding storage later needs its own change.** A stored shadow score, for
example, needs:
- a migration;
- a `PRIVACY.md` row, which `tests/published_claims.rs` enforces;
- a cascade on its block;
- export only under the consent clause.

**A return profile is an inference about a person.** That is why it stays local
and is described here. A per-person profile never leaves the Mac. A population
prior would need separate consent and would ship as constants in the binary.

## Using the founder's own data for development

The founder's Macs are excluded from every cohort result by the 2026-08-09
pre-registration. That rule stands. His data can still help build the framework,
within these limits.

**Allowed:**
- Check that the ledger runs on real rows, is deterministic, and abstains. At
  the founder's recorded pace of about 0.17 blocks a week it will abstain.
- Measure **rates**, not outcomes: eligible points per block, the share with
  900 s left, and censoring. Use them as simulator inputs, labelled "n = 1,
  founder, not a cohort figure".
- Run the export (`scripts/export_cohort_evidence.sh`) on his own Mac to
  rehearse the tester flow. The founder-device exclusion keeps that file out of
  every cohort result.

**Not allowed:**
- Tuning a threshold, cell or prior on the founder's outcome-by-context counts.
  Any such change is a new `RETURN_LEDGER_MODEL_VERSION`, justified from code
  and synthetic suites, as every policy change so far has been.
- Quoting a founder count as evidence in a deck or a pitch.

**Nothing leaves the Mac** in any of this. Agents never read `~/.velvt`. The
founder runs anything that reads his own database himself.

**Not built yet.** There is no developer harness that opens a copy of the
founder's database and prints his ledger. The per-departure outcomes file
planned for the export will carry the same labels. When it lands, the export and
`departure_rows` must agree row for row on shared test vectors.
