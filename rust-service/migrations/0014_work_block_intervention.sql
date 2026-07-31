-- Device-local record of in-session drift interventions and their outcomes.
--
-- One row per work block: the PRIMARY KEY enforces the "at most one
-- intervention per block" cap structurally rather than in application code.
--
-- `anchor_category` is a broad taxonomy category, never app identity, a window
-- title, a URL, a local display label, a stable mapping key, or intention text.
-- Nothing in this table is representable in the upload path.
CREATE TABLE work_block_intervention (
    block_id TEXT PRIMARY KEY NOT NULL,
    offered_at INTEGER NOT NULL,
    action_id TEXT NOT NULL CHECK(action_id IN ('return_to_anchor')),
    anchor_category TEXT NOT NULL CHECK(length(anchor_category) > 0),
    switch_count INTEGER NOT NULL CHECK(switch_count >= 0),
    window_seconds INTEGER NOT NULL CHECK(window_seconds > 0),
    -- `offered` is terminal only if the block ends before the user returns.
    outcome TEXT NOT NULL DEFAULT 'offered'
        CHECK(outcome IN ('offered', 'returned', 'expired')),
    outcome_at INTEGER,
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

CREATE INDEX idx_work_block_intervention_outcome
    ON work_block_intervention(outcome, offered_at DESC);
