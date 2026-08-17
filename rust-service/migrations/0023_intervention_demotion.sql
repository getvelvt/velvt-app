-- Auto-demotion (0.1.6 Scope 4; roadmap invariant 4; D5): when the rolling
-- wrong-intervention rate rises above the versioned threshold, interventions
-- pause and Velvt observes quietly. The demotion rule is a deterministic
-- versioned policy on the existing 0.1.5 wrong-intervention counter — it is
-- not learned, trained, or adaptive.
--
-- `withheld_demotion` records a decision the drift gate would have offered
-- but held because the demotion state machine was in `demoted`. Like
-- `delivery_suppressed_dnd` it is terminal at creation — a nudge that was
-- never shown cannot be answered — and it is delivered by no channel. It is
-- excluded from the wrong-intervention denominator (a nudge that was never
-- shown cannot be wrong), and it starts the same re-offer cooldown and
-- consumes the same per-block cap an offered row does, so re-promotion can
-- never produce a burst of catch-up nudges. These rows are what the weekly
-- receipts digest counts as "what Velvt chose not to send".
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt (same
-- pattern as migrations 0015-0017 and 0020). Existing rows are copied
-- unchanged.

CREATE TABLE work_block_intervention_rebuilt (
    block_id TEXT PRIMARY KEY NOT NULL,
    offered_at INTEGER NOT NULL,
    action_id TEXT NOT NULL CHECK(action_id IN ('protect_next_10')),
    anchor_category TEXT NOT NULL CHECK(length(anchor_category) > 0),
    switch_count INTEGER NOT NULL CHECK(switch_count >= 0),
    window_seconds INTEGER NOT NULL CHECK(window_seconds > 0),
    -- `offered` is the only non-terminal state.
    outcome TEXT NOT NULL DEFAULT 'offered'
        CHECK(outcome IN (
            'offered',
            'accepted_action',
            'returned',
            'not_helpful',
            'wrong_classification',
            'dismissed',
            'was_focused',
            'delivery_suppressed_dnd',
            'withheld_demotion',
            'no_response'
        )),
    outcome_at INTEGER,
    salience TEXT NOT NULL DEFAULT 'normal'
        CHECK(salience IN ('normal', 'quiet')),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

INSERT INTO work_block_intervention_rebuilt (
    block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at, salience
)
SELECT
    block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at, salience
FROM work_block_intervention;

DROP TABLE work_block_intervention;

ALTER TABLE work_block_intervention_rebuilt RENAME TO work_block_intervention;

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);

-- The demotion state machine's persisted state. A singleton row like
-- `velvt_quiet_hours`. The state is derived deterministically from the
-- intervention rows above plus `manual_reset_at`; this row exists so the
-- entered-at instant can be disclosed ("quiet since ...") and so a manual
-- reset is remembered. It records the current state only — never a
-- transition history or timeline. Behavioral evidence, so it is cleared by
-- clear-all-data (unlike the `initiation_settings` preference).
CREATE TABLE intervention_demotion_state (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    state TEXT NOT NULL DEFAULT 'active' CHECK(state IN ('active', 'demoted')),
    demoted_at INTEGER,
    manual_reset_at INTEGER,
    threshold_policy_version INTEGER NOT NULL CHECK(threshold_policy_version >= 1),
    repromotion_policy_version INTEGER NOT NULL CHECK(repromotion_policy_version >= 1),
    updated_at INTEGER NOT NULL
);
