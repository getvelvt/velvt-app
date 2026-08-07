-- Block-scoped classification corrections (roadmap invariant 3).
--
-- When the user answers a drift offer with "wrong classification", the
-- disputed broad category counts as the block's focus work for this block:
-- instantly, visibly, and deterministically. The correction is keyed to the
-- block and dies with it — durable per-activity training stays in the
-- existing personal-override correction path.
--
-- Contains broad taxonomy categories only: no app identity, title, URL, or
-- intention text.

CREATE TABLE work_block_category_correction (
    block_id TEXT NOT NULL,
    category TEXT NOT NULL CHECK(length(category) > 0),
    counts_as_category TEXT NOT NULL CHECK(length(counts_as_category) > 0),
    corrected_at INTEGER NOT NULL,
    PRIMARY KEY(block_id, category),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);
