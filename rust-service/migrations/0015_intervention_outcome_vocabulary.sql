-- Widens the intervention vocabulary so an explicit user response is
-- distinguishable from silence.
--
-- `offered` -> `no_response` conflated four different situations: the user never
-- saw the offer, saw it and disagreed, saw it and could not act, or ignored it.
-- Measuring whether the detector is right requires telling those apart, so the
-- user-reportable outcomes below exist.
--
-- The action registry is also aligned with the 0.1.5 Scope 4 closed registry:
-- `return_to_anchor` becomes `protect_next_10`.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. Existing rows
-- are migrated: `expired` carries the same meaning as `no_response`.

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
    block_id,
    offered_at,
    'protect_next_10',
    anchor_category,
    switch_count,
    window_seconds,
    CASE outcome WHEN 'expired' THEN 'no_response' ELSE outcome END,
    outcome_at
FROM work_block_intervention;

DROP TABLE work_block_intervention;

ALTER TABLE work_block_intervention_rebuilt RENAME TO work_block_intervention;

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);
