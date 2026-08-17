-- Adds `delivery_suppressed_dnd` to the intervention outcome vocabulary
-- (roadmap invariant 5; DND is data, not defiance).
--
-- When the delivery path would fire while DND is active, the decision is
-- recorded with this outcome, held, and delivered by no channel — no OS
-- notification, no in-app takeover, no mid-block retry against the user's
-- setting. The row is terminal at creation: a nudge that was never shown
-- cannot be answered. Held decisions reconcile after the block as counts
-- only, never as late nudges. A suppressed row starts the same re-offer
-- cooldown an offered row does, so suppression can never shorten a wait,
-- raise salience, or increase future frequency.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. Existing
-- rows are copied unchanged.
--
-- Written against the shipped schema rather than the branch this outcome was
-- authored on. That branch carried `offer_seq` and `backoff_policy_version`
-- for multiple offers per block and a versioned cooldown multiplier; neither
-- shipped. This table keeps `block_id` as the primary key — one offer per
-- block, which the pre-registered primary metric's denominator depends on —
-- and keeps `salience`, which the branch predates.

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
            'was_focused',
            'dismissed',
            'delivery_suppressed_dnd',
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
