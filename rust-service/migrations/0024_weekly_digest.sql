-- Weekly receipts digest (0.1.6 Scope 4; D6): one stored row per completed
-- local week, frozen at generation time from the same stored aggregates the
-- local metrics read. Freezing makes the parity auditable: the displayed
-- digest is the stored row, and a test can regenerate every count from the
-- underlying tables and compare.
--
-- Everything here is device-local. `week_start_local_date` is the local
-- Monday and exists to key the week deterministically across UTC-offset
-- changes; like every work-block timestamp it never leaves this database.
-- The row holds bounded counts only — no categories, copy, identifiers, or
-- per-day breakdown is representable, so a streak or failure tally cannot
-- be stored here.

CREATE TABLE weekly_digest (
    week_start_local_date TEXT PRIMARY KEY NOT NULL
        CHECK(length(week_start_local_date) = 10),
    generated_at INTEGER NOT NULL,
    blocks_declared INTEGER NOT NULL CHECK(blocks_declared >= 0),
    blocks_completed INTEGER NOT NULL CHECK(blocks_completed >= 0),
    recoveries INTEGER NOT NULL CHECK(recoveries >= 0),
    wrong_interventions INTEGER NOT NULL CHECK(wrong_interventions >= 0),
    invitations_accepted INTEGER NOT NULL CHECK(invitations_accepted >= 0),
    withheld INTEGER NOT NULL CHECK(withheld >= 0),
    digest_version INTEGER NOT NULL CHECK(digest_version >= 1),
    -- When the digest was first shown. NULL while held (quiet hours,
    -- Focus/DND) — the hold is a delay, never a different channel.
    delivered_at INTEGER,
    -- The user's one-tap close. Bookkeeping only.
    acknowledged_at INTEGER
);
