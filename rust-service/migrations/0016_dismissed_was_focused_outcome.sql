-- Adds `dismissed_was_focused` to the intervention outcome vocabulary.
--
-- A plain dismissal says "not now". `dismissed_was_focused` says the
-- interruption itself was wrong regardless of classification — the user was
-- focused. It is the ground-truth false-positive signal for the detector and
-- must stay distinct from both `dismissed` and `wrong_classification` in
-- storage and metrics, so it is a first-class stored value rather than a
-- rendering of one of the existing ones.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. Existing
-- rows carry their existing meanings and are copied unchanged.

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
            'dismissed_was_focused',
            'no_response'
        )),
    outcome_at INTEGER,
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

INSERT INTO work_block_intervention_rebuilt (
    block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at
)
SELECT
    block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at
FROM work_block_intervention;

DROP TABLE work_block_intervention;

ALTER TABLE work_block_intervention_rebuilt RENAME TO work_block_intervention;

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);
