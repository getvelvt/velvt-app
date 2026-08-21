-- The bounded window of activity immediately preceding a block start, recorded
-- once at block start and never updated. Categories only, from the closed
-- taxonomy. The window length is a versioned policy constant, not a user- or
-- config-widenable value, so the amount of pre-block context recorded cannot
-- grow without a migration and a privacy review.
--
-- `categories` is a set, not a sequence: a sequence would be more informative
-- to the model and more identifying. Widening it to a sequence requires a fresh
-- review, not a config change.
--
-- `window_seconds` is bounded at 30 minutes by the schema. A user cannot widen
-- it, and neither can a config file. The constraint lives here deliberately.
--
-- No label, no stable_id, no app identity, no window title, no URL, no
-- intention text. Nothing in this table is representable in the upload path.
-- Cleared with the block it belongs to.

CREATE TABLE block_antecedent (
    block_id               TEXT    PRIMARY KEY NOT NULL,
    window_seconds         INTEGER NOT NULL CHECK(window_seconds BETWEEN 300 AND 1800),
    -- JSON array of distinct categories present in the window, sorted.
    -- Closed vocabulary; no ordering information, no dwell per item.
    categories             TEXT    NOT NULL,
    switch_count           INTEGER NOT NULL CHECK(switch_count >= 0),
    dominant_category      TEXT,
    dominant_dwell_seconds INTEGER CHECK(dominant_dwell_seconds >= 0),
    day_type               TEXT    NOT NULL CHECK(day_type IN ('weekday', 'weekend')),
    hour_bucket            INTEGER NOT NULL CHECK(hour_bucket BETWEEN 0 AND 23),
    is_first_block_of_day  INTEGER NOT NULL CHECK(is_first_block_of_day IN (0, 1)),
    antecedent_version     INTEGER NOT NULL CHECK(antecedent_version >= 1),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);
