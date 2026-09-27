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
of 2026-09-27. It must not merge until the founder has added a dated, scoped
entry to `plan/README.md`. The proposed text is under
[Before this merges](#before-this-merges); only the founder writes it there.

## What exists

| Piece | Where | What it does |
|---|---|---|
| Feature contract 3 | `rust-service/src/behavior/features.rs` | Records that v5 changed what `c_t` and `q_t` observe for browser tabs. Rows cut under contracts 2 and 3 are never pooled. |
| Return ledger | `rust-service/src/behavior/returns.rs` | Per-person counts of returns to the anchor, by context cell. Details below. |
| Synthetic suite E | `scripts/generate_traces.py`, `scripts/traces/SYNTHETIC-suite-e-returns.jsonl`, `rust-service/tests/trace_replay.rs` | 136 synthetic people replayed through the real `WorkBlockManager`, then scored by the ledger. |
| Design simulator | `scripts/simulate_nudge_designs.py` | How many weeks v6 needs, and what the declared withhold candidate would do, under stated assumptions. |

### The return ledger

**Rows.** A row is a departure the gate saw and did not act on: a decision
logged with an anchor, made on a confident non-anchor observation whose previous
confident observation was that anchor. This is `s_t`, the gate's own rule.

**Label.** The pre-registered primary outcome: at least 600 of the next 900
seconds spent in confident observations of the anchor recorded on the decision
row. A row that cannot be scored is **censored**, never counted as a failure,
for one of three reasons, in this order:
- `block_ended`: the block ended inside the 900 seconds.
- `observer_gap`: the observation ledger has a gap in them. That covers a
  pause, a sleep, a restart, and the dwell a block ends in, which no row
  measures.
- `treated`: an offer, delivered or held, came at or before the departure, or
  inside the 900 seconds while the label was still open. The row is followed
  only up to the offer. If the time before the offer already decides the label
  (600 anchor seconds, or so few that the rest could not make them up), the
  row is resolved on that time alone.

The first two reasons are the pre-registered censoring rule's. `treated` is the
ledger's own: it counts departures the gate did not act on, and an offer is the
gate acting.

**The `treated` censoring is informative, and the rates most plausibly read
high.**
- v5 offers only on a departure that makes three inside ten minutes. An offer
  inside a departure's horizon therefore means the person left again before
  the fifteen minutes were up, as part of a run.
- A horizon with a run in it usually holds less anchor time. So the rows an
  offer censors are most plausibly rows heading for "did not return", and
  dropping them makes the rates read **higher** than the person's own,
  towards "came back on their own".
- The size differs per cell, because offers follow some kinds of departure
  more than others. The direction is likely, not guaranteed: someone who darts
  away and straight back three times may still have come back.
- Neither the size nor, per cell, the sign can be estimated from v5 rows.
  Every cell reports how many of its departures an offer censored
  (`CellAccounting::censored_treated`), beside its departures, its other
  censoring and its rows decided before an offer. Suite E's INFORMATIVE family
  shows the bias on planted rates (below).
- The earlier rule dropped any row with an offer up to the horizon's end,
  whether or not the time before the offer had decided it. Censoring at the
  offer keeps those decided rows, which lowers the bias but does not remove it.

**Cells.** Nine frozen cells in three dimensions:
- the kind of departure: communication, feeds and video, or work-adjacent;
- the third of the block it happened in;
- the part of the local day: morning, afternoon, or evening and night.

**Estimates.** Each cell is a Beta-Binomial posterior.
- Prior: Beta(4, 4) on the person's overall rate. Each cell's prior is four
  pseudo-departures at that overall rate.
- Rows from one block are down-weighted for an assumed within-block correlation
  of 0.3.
- Blocks the person disputed count at half weight. A dispute counts only once
  it was recorded at or before `as_of`: a "Wrong category" reply by its
  `outcome_at`, a correction by its `corrected_at`. Both can be written after
  the block closed.
- `work_block_category_correction` has no production writer. Only the
  persistence layer's own method writes it, and only tests call that. On a Mac
  it is always empty, so a "Wrong category" reply is the only dispute the
  ledger can see today.

**Part of the day.** A block is read at its own UTC offset when the caller has
one, and at the latest known offset (`focus_observer_state`) otherwise. No
table stores a per-block offset today, so every block is read at the current
offset.
- Limitation: across a daylight-saving change or travel inside the lookback,
  departures within the offset difference of 05:00, 12:00 or 17:00 are filed
  in the neighbouring part of the day.
- Storing the offset per block needs a migration of its own. Until one exists,
  the hour cells are right only for a person whose offset did not change in the
  28 days read.

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
- It inherits the upward bias above: a rate that reads high makes it fire more
  often than the person's own rate would.

**Purity.** The ledger is a set of pure functions over rows passed in. It has no
database handle, reads no clock, uses no randomness and stores no state. Nothing
recorded after `as_of` can change an answer given at `as_of`.

### Shadow only, enforced

- `nothing_in_the_shipped_path_calls_the_return_ledger` fails the build if any
  file names the module. It reads:
  - everything in `rust-service/src`, including the gate, delivery, the IPC
    router, receipts and `main.rs`;
  - the shared types;
  - the Swift client;
  - the IPC schema.
- A file names the module if it contains any of these:
  - a path through `behavior::returns` or `returns::`, or the file name
    `returns.rs`;
  - an aliased import such as `use crate::behavior::returns as r;`;
  - a Swift or JSON spelling of its outputs (`would_withhold`,
    `wouldWithhold`, `return_ledger`, and similar);
  - the name of any public item in the module. The list is read from the
    file itself, so a new item is covered the day it lands.
- Every public name is unique in the scanned tree. Seven that collided with
  other modules were renamed, for example `Support` to `WithholdSupport` and
  `HORIZON_SECONDS` to `RETURN_HORIZON_SECONDS`.
- `the_no_caller_tripwire_fires_on_planted_callers_and_only_on_them` checks the
  test itself:
  - it plants 13 callers into a copy of the scanned layout, the review's
    `use crate::behavior::returns as r; r::context_of(..)` among them;
  - the same scan must name each one, for the stated reason;
  - five pieces of prose and near misses must pass.
  An aliased call planted in the real `main.rs` also fails the test.
- `behavior` is declared in `main.rs`, so nothing in the library crate can reach
  it.
- The ledger has no user-facing strings, and `scripts/check_banned_copy.py`
  passes.
- The one change outside `behavior/` makes `is_confident` public, so the ledger
  reads `q_t` through the gate's own function. It changes no decision.

## What it can and cannot say before randomization

**It can say** how often this person spent most of the next fifteen minutes back
at their work on their own, after a departure the gate did not act on. It gives
this per cell, as raw counts ("4 of 11") beside an estimate, and beside how many
of the cell's departures an offer censored. When the evidence is thin it says
so. Because of the `treated` censoring, those counts most plausibly read high.

**It cannot say anything about what a nudge does, or what silence would do.**
Under v5 every eligible point is offered (propensity 1.0).
- So every row the ledger counts is a departure the gate did not find eligible.
  In practice that is the first or second switch inside ten minutes.
- The held rows (Focus/DND, demotion) are the only v5 points with an eligible
  departure and no delivery. They are censored as `treated` too, because the
  states that held them also shape what happens next.
- Suite E checks this on the gate's own rows:
  - every counted row is below the switch threshold;
  - at all 719 offered points, `would_withhold` reports
    `WithholdSupport::Extrapolated`.

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
| INFORMATIVE (offers follow non-returns) | communication reads 0.707 against a planted 0.398, with 244 departures censored as `treated`; feeds and video 0.588 against 0.552 (26 treated); work-adjacent 0.502 against 0.453 (38 treated). The retired rule read 0.918 for communication. Communication surfaced as *higher* in 1/12 traces, where nothing was planted. |
| GAPS | 18 pauses and 25 unmeasured final dwells: 43 rows censored as `observer_gap`, hiding 21 returns and 22 non-returns |
| Labels | 11,954 departures matched their planted label exactly: 5,162 returned, 3,954 not returned, 696 block-ended, 43 observer gap, 2,099 treated |

**Reading INFORMATIVE.**
- Every person there returns on their own at 0.55 after every kind of
  departure.
- A non-return turns into a run of three departures, which the shipped gate
  offers on: 90% of the time after a communication departure, 10% after
  anything else.
- The planted truth counts every departure in a run that the gate did not act
  on, and all of them are non-returns. That is why the planted communication
  rate is 0.398 rather than 0.55.
- The size of the bias here is a property of that assumption, not an estimate.
  Nothing measures how often real offers follow real non-returns.

The thresholds were frozen while looking at the first five families. One later
run on a fresh seed gave:
- NULL: 1 of 200 traces surfaced a cell;
- PLANTED: 2/40, 16/40 and 28/40;
- REGIME: 32/40 before the change and 0/40 after.

Even a large difference needs more than a block a day before four weeks of data
show it reliably.

**The v6 design and the declared candidate.** From
`./scripts/simulate_nudge_designs.py --scenarios`. None of the assumptions is
measured.
- **v6:** 75% of eligible points have at least 900 s left, 10% of drawn points
  are censored, the baseline under silence is 0.30, and the effect varies
  across people with SD 0.05.
- **Declared candidate:** 3 departures a block that the gate leaves alone, 30%
  of them censored, and an own-return rate of 0.55 on average (SD 0.15), the
  same in every cell. It reads only the last 28 days, so it is reported once
  per window, not by weeks of use. The simulator carries a port of its
  arithmetic, pinned to the Rust ledger by one shared test vector.
- **Two links.** Whether it is right when it fires depends on how a person's
  own-return rate relates to what a nudge does for them. Nothing has measured
  that, so precision is shown under two links:
  - *headroom*: people who come back on their own more gain less;
  - *no link*: the two are unrelated.
- **Effect-based rule.** For comparison only. It is *not* the declared
  candidate and not the v6 draft's rule: it withholds when the 80% upper bound
  of a person's own randomized effect is below 5 points.
- In both rules, "right" means the person's true effect is below 5 points.

| Testers | Blocks/wk | Eligible/block | Effect | Points/wk | Points needed | Weeks to N | One person: weeks to see the sign | Declared: abstains / fires | Declared precision, headroom / no link | Effect-based precision at 26 / 52 wk |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | 3 | 0.5 | 15 pts | 8.4 | 428 | 50.6 | 91.1 | 70% / 1.4% | 38% / 2% | 3% / 7% |
| 10 | 3 | 1.5 | 15 pts | 18.2 | 428 | 23.4 | 42.2 | 70% / 1.4% | 38% / 2% | 9% / 10% |
| 10 | 3 | 0.5 | 7.5 pts | 8.4 | 2,295 | 271.8 | 357.9 | 70% / 1.4% | 67% / 34% | 41% / 50% |
| 10 | 3 | 1.5 | 7.5 pts | 18.2 | 2,295 | 125.9 | 165.7 | 70% / 1.4% | 67% / 34% | 48% / 58% |
| 40 | 3 | 0.5 | 15 pts | 33.8 | 399 | 11.8 | 91.1 | 70% / 1.4% | 38% / 2% | 4% / 6% |
| 40 | 3 | 1.5 | 15 pts | 72.9 | 399 | 5.5 | 42.2 | 70% / 1.4% | 38% / 2% | 7% / 11% |

With everyone's effect equal, the points needed are the pre-registered 390, or
about 1,495 at 7.5 points. At 8 blocks a week, with the other inputs unchanged,
the declared candidate never abstains and fires for 8.7% of people, with
precision 33% under the headroom link and 3% with no link.

**Reading the table.**
- The average effect is reachable in months with 10 testers and in weeks with
  40.
- The declared candidate mostly declines to answer at 3 blocks a week. It needs
  30 resolved rows in 28 days, and gets about 25. It fires for about 1 person in
  70.
- When it fires, whether it is right rests on the unmeasured link:
  - if people who come back on their own more gain less, about 4 in 10 of the
    people it fires for gain less than 5 points at a 15-point average effect;
  - if the two are unrelated, about 1 in 50, which is the base rate: firing
    then says nothing about the effect.
  - More use makes it fire more often, not more precisely.
  - It also inherits the ledger's upward bias, which this simulation leaves
    out. These figures are the rule at its best.
- The effect-based rule is mostly wrong for a year at these volumes. When most
  people benefit, most of the people it flags are noise.
- The inputs that matter most are the eligible-point rate and the departures a
  block the gate leaves alone. Neither has been measured off the founder's Mac.

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
- `work_block_category_correction`, which nothing in the shipped path writes
  today

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

A per-block UTC offset, which would fix the part-of-day limitation above, is
the same kind of change.

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
`departure_rows` must agree row for row on shared test vectors, including the
`treated` reason, which the pre-registered export does not have.

## Before this merges

**A dated, scoped Gate D entry in `plan/README.md`, written by the founder.**
`AGENTS.md` (Architecture Constraints and Scope Boundary) forbids new work in
`src/behavior/` without Gate D or a dated founder decision that overrides it.
Gate D has not been met. Nothing on this branch writes the entry. The proposed
text follows, to paste as a status note after the Gate D paragraph. If it is
entered on a later day, that day is its date.

> **Gate D, dated exception — 2026-09-27: the shadow return ledger.** Gate D
> (`pivot-engineering/10-BUNDLE-ABSORPTION.md` § 5) is not met, and its
> no-engine-work rule otherwise stands. By this decision, one piece of engine
> work may merge: velvt-app branch `feat/shadow-learning-framework`. That is the
> per-person return ledger (`rust-service/src/behavior/returns.rs`), feature
> contract 3 (`behavior/features.rs`), synthetic suite E
> (`scripts/generate_traces.py`, `tests/trace_replay.rs`) and the design
> simulator (`scripts/simulate_nudge_designs.py`).
>
> **In scope:** shadow code only. Nothing in the drift gate, delivery, the IPC
> router, any copy surface, the Swift client or the IPC schema may call or name
> the ledger, and its tripwire test enforces that. It adds no migration, table,
> column, retention window or `PRIVACY.md` row. It reads only rows already on
> the Mac and sends nothing anywhere. Drift policy v5 and every decision it
> makes are unchanged.
>
> **Not in scope, each needing its own dated entry:** randomization and policy
> v6 (RUNBOOK A4 stays parked), logging the ledger's score or version on
> decision rows, storing any per-person profile or a per-block UTC offset,
> reading testers' outcome-by-context tables, and any use of the ledger's
> output in the product.
