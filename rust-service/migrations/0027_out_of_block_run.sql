-- Durable out-of-block behavioural history, at run granularity.
--
-- Folded from `raw_event_buffer` runs that fall outside any declared work
-- block, before the buffer's 7-day TTL reaches the source rows. The point of
-- the fold is that the durable store knows *less* than the expiring store it
-- derives from: `raw_event_buffer` carries `stable_id`, `label`, and
-- `local_name_suggestion` (the raw application name); this table structurally
-- cannot hold any of them. There is no label column, no stable_id column, no
-- app identity, no window title, no URL, and no intention text. Broad category
-- from the closed shipped taxonomy, and coarse time.
--
-- Privacy posture is therefore identical to `work_block_observation`, and
-- strictly less informative than the buffer it is derived from. Nothing in this
-- table is representable in the upload path.
--
-- Start time is floored to the same 300-second bucket `focus_state_evidence`
-- (0019) already established, so no new precision class is introduced.
-- Retention: 90 days, enforced by `OutOfBlockRunRetentionTarget`.

CREATE TABLE out_of_block_run (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Unix seconds floored to the 300-second bucket.
    started_at_bucket         INTEGER NOT NULL,
    duration_seconds          INTEGER NOT NULL
        CHECK(duration_seconds BETWEEN 0 AND 1800),
    category                  TEXT    NOT NULL CHECK(length(category) > 0),
    -- `classification_status` is not decoration. `is_confident_evidence`
    -- (`work_block/mod.rs`) is `status = classified` AND `confidence IN
    -- (high, medium)` AND the category is not SYSTEM/UNCLASSIFIED/UNLOGGED.
    -- Without the status column that predicate is not reconstructible for an
    -- out-of-block run, and the feature layer would be forced to invent its own
    -- notion of "confident" -- which would let the model and the shipped gate
    -- disagree about what counts as evidence. They must never disagree.
    classification_status     TEXT    NOT NULL
        CHECK(classification_status IN ('classified','ambiguous','unclassified')),
    classification_confidence TEXT    NOT NULL
        CHECK(classification_confidence IN ('high','medium','low','none')),
    local_hour                INTEGER NOT NULL CHECK(local_hour BETWEEN 0 AND 23),
    local_date                TEXT    NOT NULL CHECK(length(local_date) = 10)
);

CREATE INDEX idx_out_of_block_run_bucket
    ON out_of_block_run(started_at_bucket DESC);
CREATE INDEX idx_out_of_block_run_local_date
    ON out_of_block_run(local_date, local_hour);
