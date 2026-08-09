-- Adds `was_focused`: the user reporting that they were working the whole time.
--
-- The existing vocabulary could not express a false positive. `dismissed` means
-- "not now", `not_helpful` concedes the drift happened, and
-- `wrong_classification` disputes a label rather than the judgment. Measuring
-- how often Velvt interrupts someone who was not drifting — the wrong-
-- intervention rate — requires an outcome that says exactly that, so the offer
-- can be counted against the detector rather than against the user.
--
-- The same rebuild adds `salience`, which records how the offer was actually
-- delivered. An offer made quietly after an earlier dismissal is a different
-- event from a full-salience one, and reading an outcome without knowing which
-- it was would overstate how often the user ignored a notification they were
-- never sent.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt. No existing
-- row changes meaning: `was_focused` has never been recorded before, and every
-- offer made so far was delivered at full salience.

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
    block_id,
    offered_at,
    action_id,
    anchor_category,
    switch_count,
    window_seconds,
    outcome,
    outcome_at,
    'normal'
FROM work_block_intervention;

DROP TABLE work_block_intervention;

ALTER TABLE work_block_intervention_rebuilt RENAME TO work_block_intervention;

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);
