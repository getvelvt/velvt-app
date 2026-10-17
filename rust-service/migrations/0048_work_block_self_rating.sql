-- The optional question after a block: the answers, and whether it is asked.
--
-- WHY. Velvt's summary of a block (the result card's "stayed", "left and came
-- back", "ended somewhere else") is computed from broad categories. Nothing
-- records whether the person would describe the block the same way. One
-- optional question after a block ends, "How much of this block was on what
-- you meant to do?", gives an answer to compare the summary with, offline and
-- under a hashed pre-registration (workspace plan
-- `plan/attention-labels-and-model.md` § 3.3; founder decision 2026-10-17).
-- This migration only creates the storage. Nothing writes either table until
-- the code that decides when the question is asked lands in a later change.
-- No drift decision, offer, demotion, backoff or receipt reads either table.
--
-- `work_block_self_rating` is one row per block that became eligible for the
-- question. The row is written at eligibility with `shown_at` NULL, so a
-- question that was never on screen is still a recorded missing answer rather
-- than an absent row. `answer` is one of five fixed values. Not now, closing
-- the popover and silence are never stored as an answer: `not_now_at` and
-- `expired_at` record them, and `answer` stays NULL. `answered_at` is NULL
-- exactly when `answer` is. `card_seen_before_answer` is 1 when the answer was
-- given after the block's result card had been on screen.
--
-- `rating_prompt_settings` is the question's on/off switch, a singleton like
-- `initiation_settings`. No row means off: the question is asked only after the
-- person turns it on. `opted_in_at` and `opted_out_at` are the last time each
-- happened.
--
-- Every column is a key, one value from a closed list, a 0/1 flag, the
-- singleton id, a timestamp or a version (`docs/field-inventory.md`). There is
-- no column for free text, an app or site name, a window title, an address, an
-- intention or a category. Every column refuses a typed sentence: the text
-- columns by CHECK or foreign key, the integer columns by a `typeof` CHECK,
-- because SQLite stores text in an INTEGER column that has no CHECK.
--
-- Retention: an answer goes with its block. The foreign key cascades, so Clear
-- Local Work Blocks, which deletes every `work_block` row, removes every
-- answer. No sweep of its own. The setting is a choice the person made and
-- survives Clear Local Work Blocks, as `initiation_settings` does.
--
-- UNUPLOADABLE. Neither table reaches the network. `BatchEventPayload`
-- (`upload/dto.rs`) has no field an answer or a setting could occupy.
--
-- Additive: two new tables. No table rebuild, no CHECK widened, no index (one
-- row per eligible block, read by primary key).

CREATE TABLE work_block_self_rating (
    block_id TEXT PRIMARY KEY NOT NULL
        REFERENCES work_block(block_id) ON DELETE CASCADE,
    prompt_version INTEGER NOT NULL
        CHECK (typeof(prompt_version) = 'integer' AND prompt_version >= 1),
    eligible_at INTEGER NOT NULL CHECK (typeof(eligible_at) = 'integer'),
    shown_at INTEGER CHECK (typeof(shown_at) IN ('integer', 'null')),
    answer TEXT CHECK (answer IN ('all', 'most', 'half', 'less_than_half', 'little')),
    answered_at INTEGER CHECK (typeof(answered_at) IN ('integer', 'null')),
    not_now_at INTEGER CHECK (typeof(not_now_at) IN ('integer', 'null')),
    expired_at INTEGER CHECK (typeof(expired_at) IN ('integer', 'null')),
    card_seen_before_answer INTEGER NOT NULL DEFAULT 0
        CHECK (card_seen_before_answer IN (0, 1)),
    CHECK ((answer IS NULL) = (answered_at IS NULL))
);

CREATE TABLE rating_prompt_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    opted_in_at INTEGER CHECK (typeof(opted_in_at) IN ('integer', 'null')),
    opted_out_at INTEGER CHECK (typeof(opted_out_at) IN ('integer', 'null'))
);
