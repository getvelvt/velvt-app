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
