-- Allows more than one drift offer per block so backoff-on-dismissal is real.
--
-- The previous schema made one-offer-per-block a property of the primary key.
-- The trust patches replace that blanket cap with a versioned deterministic
-- backoff policy: a re-offer needs materially new evidence and a cooldown that
-- every negative reply multiplies. That requires offers to be rows, not a row.
--
-- `backoff_policy_version` records which policy constants produced the offer.
-- 0 marks rows that predate the policy.
--
-- SQLite cannot alter a primary key, so the table is rebuilt. Existing single
-- offers become offer_seq 1.

CREATE TABLE work_block_intervention_rebuilt (
    block_id TEXT NOT NULL,
    offer_seq INTEGER NOT NULL CHECK(offer_seq >= 1),
    offered_at INTEGER NOT NULL,
    action_id TEXT NOT NULL CHECK(action_id IN ('protect_next_10')),
    anchor_category TEXT NOT NULL CHECK(length(anchor_category) > 0),
    switch_count INTEGER NOT NULL CHECK(switch_count >= 0),
    window_seconds INTEGER NOT NULL CHECK(window_seconds > 0),
    backoff_policy_version INTEGER NOT NULL DEFAULT 0 CHECK(backoff_policy_version >= 0),
    -- `offered` is the only non-terminal state.
    outcome TEXT NOT NULL DEFAULT 'offered'
        CHECK(outcome IN (
            'offered',
            'accepted_action',
            'returned',
            'not_helpful',
            'wrong_classification',
            'dismissed',
            'dismissed_was_focused',
            'no_response'
        )),
    outcome_at INTEGER,
    PRIMARY KEY(block_id, offer_seq),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

INSERT INTO work_block_intervention_rebuilt (
    block_id, offer_seq, offered_at, action_id, anchor_category,
    switch_count, window_seconds, backoff_policy_version, outcome, outcome_at
)
SELECT
    block_id, 1, offered_at, action_id, anchor_category,
    switch_count, window_seconds, 0, outcome, outcome_at
FROM work_block_intervention;

DROP TABLE work_block_intervention;

ALTER TABLE work_block_intervention_rebuilt RENAME TO work_block_intervention;

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);
