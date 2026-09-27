-- The needs-a-category prompt's memory: which entries of the list have been
-- shown, answered and announced, and which local days a reminder was posted.
--
-- WHY. The list of applications and sites Velvt could not categorize
-- (`unclassified_triage`, protocol 30; sites since protocol 33) was pulled only
-- while its Settings pane was on screen, so nobody learned it existed. Protocol
-- 33 adds a card and at most one reminder a day, and both must speak only when
-- something NEW needs a category: a card answered with "Not now" that came
-- back the next minute, or a reminder repeated every morning for the same two
-- sites, is noise the person has already declined. Nothing recorded what had
-- been shown or answered, so nothing could tell new from old.
--
-- `category_prompt_entry` is one row per entry of the list, keyed
-- `application:<key>` or `site:<key>`, where the key is the salted digest the
-- list itself carries (`raw_event_buffer.app_stable_id` or `.site_stable_id`,
-- HMAC-SHA-256 under `stable_key_salt`, 0037). It records when the entry was
-- first and last on the list, the card it was last shown on (`prompt_id`, a
-- SHA-256 over the sorted keys of that card, so the same list is the same
-- card), when an answer to that card reached it, and when a reminder counted
-- it. The key is the only thing here drawn from the Mac, and it is disclosed
-- in PRIVACY.md as naming the application or site, because a holder of the
-- whole file can hash guesses under the salt beside it. No name, no hostname,
-- no category, no time observed.
--
-- `category_prompt_notification` is one row per local calendar day on which a
-- reminder was handed to the app to post: the day (the primary key is what
-- makes "at most one a day" a property of the table rather than of a check
-- that could race), when, how many entries it counted, the policy version it
-- was decided under, and when the person next opened the list from a prompt,
-- which the policy's backoff reads. A reminder is recorded when it is handed
-- over, whether or not macOS shows it; it is never handed over twice.
--
-- Retention (registered targets, constants in `retention/targets.rs`): an
-- entry 14 days after it was last on the list, the horizon of the events that
-- put it there; a reminder row 30 days after it was posted. The salt re-mint
-- deletes every entry, since a key under a lost salt names nothing.
--
-- UNUPLOADABLE. None of this reaches the network. `BatchEventPayload`
-- (`upload/dto.rs`) has no field a key, a date or a count could occupy.
--
-- Additive: two new tables and three indexes. No table rebuild and no CHECK
-- widened. Nothing here is in `KEYED_COLUMNS`: on a fresh database 0037's
-- re-key runs before these tables exist, and nothing here holds a digest
-- written before the salt.

CREATE TABLE category_prompt_entry (
    entry_key TEXT PRIMARY KEY NOT NULL CHECK (
        (
            (substr(entry_key, 1, 12) = 'application:' AND length(entry_key) = 76)
            OR (substr(entry_key, 1, 5) = 'site:' AND length(entry_key) = 69)
        )
        AND substr(entry_key, instr(entry_key, ':') + 1) NOT GLOB '*[^0-9a-f]*'
    ),
    first_listed_at INTEGER NOT NULL,
    last_listed_at INTEGER NOT NULL,
    prompt_id TEXT CHECK (
        prompt_id IS NULL
        OR (length(prompt_id) = 64 AND prompt_id NOT GLOB '*[^0-9a-f]*')
    ),
    acknowledged_at INTEGER,
    notified_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_category_prompt_entry_last_listed_at
    ON category_prompt_entry(last_listed_at);

CREATE INDEX IF NOT EXISTS idx_category_prompt_entry_prompt_id
    ON category_prompt_entry(prompt_id);

CREATE TABLE category_prompt_notification (
    local_date TEXT PRIMARY KEY NOT NULL CHECK (
        local_date GLOB '[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]'
    ),
    posted_at INTEGER NOT NULL,
    entry_count INTEGER NOT NULL CHECK (entry_count BETWEEN 1 AND 8),
    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),
    opened_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_category_prompt_notification_posted_at
    ON category_prompt_notification(posted_at);
