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
