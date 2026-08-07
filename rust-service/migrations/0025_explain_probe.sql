-- Explain-tap probe instrumentation (0.1.6 Scope 4; D7; roadmap Metrics 5).
--
-- The pre-registered R4 gate metric is coarse, consented, and content-free
-- by construction: one row per local week holding a single bounded tap
-- count. Which nudge was explained, what evidence was shown, and when
-- within the week a tap happened are structurally unrepresentable. The
-- denominator (interventions delivered that week) is not stored here — it
-- is read from `work_block_intervention` through the same delivered
-- predicate the wrong-intervention counter uses, so the probe cannot drift
-- from the stored intervention record.
--
-- Device-local. In this release no upload path for these counts exists;
-- any future upload is Terra's closed aggregate contract and is gated on
-- the existing telemetry consent.

CREATE TABLE explain_probe_week (
    week_start_local_date TEXT PRIMARY KEY NOT NULL
        CHECK(length(week_start_local_date) = 10),
    taps INTEGER NOT NULL DEFAULT 0 CHECK(taps >= 0),
    updated_at INTEGER NOT NULL
);
