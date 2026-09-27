-- Site rules, and the one place a hostname is stored.
--
-- WHY. A browser tab's window key is (browser, host) (`abstraction/key.rs`), so
-- a correction made on a site in Safari never reached the same site in Chrome,
-- and no column recorded which site an event was on -- only that it was on one
-- (`app_scope_eligible = 0`, 0017). Browser time Velvt could not classify had
-- no identity it could be asked about or taught under. This migration gives a
-- site one identity in every browser, a table for a rule taught about it, and a
-- table for the name Velvt shows when it asks about it.
--
-- `raw_event_buffer.site_stable_id` is the site key: HMAC-SHA-256 under this
-- install's `stable_key_salt` (0037) of the site's normalized host, in a domain
-- of its own (`velvt:abstraction-site-key:v1`), so it never equals an
-- application, bundle or window key. The host is normalized first
-- (`sites::site_identity`): one leading `www.` is stripped, and an address, a
-- name only a private network answers (`localhost`, `.local`, `home.arpa`,
-- `.internal`, `.lan`, `.localdomain`), or a name without a dot has no
-- identity at all. NULL on every event that is not a browser window on such a site, and on
-- every row written before this migration: the host was discarded at
-- abstraction and a key cannot be reversed, so there is nothing to backfill
-- from. A holder of the whole file can hash a list of popular hosts under the
-- salt stored beside it and read off which sites these keys are, so the column
-- is disclosed in PRIVACY.md as naming the site.
--
-- The index serves the list of sites Velvt could not categorize, which groups
-- the buffered events of a window of days by site the way 0033's index serves
-- the application list.
--
-- `personal_site_override` is one rule taught about one site: its site key,
-- the category, the activity name typed for it (under the CHECK 0017 puts on
-- `personal_app_override.activity_name`), and a correction count. The engine
-- consults it after the window rule and before either application rule. Like
-- the other rule tables it is kept until the rule is removed or Reset
-- Corrections runs; no sweep expires it.
--
-- `local_site_name` is the ONLY column that stores a hostname, and a named
-- exception to 0001's invariant, disclosed in PRIVACY.md. One row per site
-- key: the normalized host and when it was last seen. A row is written, or its
-- `last_seen_at` moved, on a visit whose classification was not confident (the
-- drift gate's `is_confident` rule, except that a confident SYSTEM visit counts
-- as categorized) and was not decided by one of the user's own rules, to a site
-- that has no rule; any other visit writes nothing. It is there so Velvt can name the site when it asks
-- what the site is, and it is deleted when a rule is saved for the site, when
-- `stable_key_salt` is re-minted, and 14 days after `last_seen_at` -- the
-- horizon of the raw events it was seen in. The CHECK admits exactly what the
-- normalizer produces: 1 to 253 characters of lowercase letters, digits, dots
-- and hyphens. The second index is the retention sweep's path.
--
-- UNUPLOADABLE. None of this reaches the network. `BatchEventPayload`
-- (`upload/dto.rs`) serializes an event id, a timestamp, a category-scoped
-- abstraction type and its version, a tier, and a payload of a category and a
-- duration; it has no field a key or a host could occupy.
--
-- Additive: one nullable column, two new tables, two indexes. No table rebuild
-- and no CHECK widened. Nothing here is in `KEYED_COLUMNS`: on a fresh database
-- 0037's re-key runs before these columns exist, and nothing here holds a
-- digest written before the salt.

ALTER TABLE raw_event_buffer ADD COLUMN site_stable_id TEXT
    CHECK (site_stable_id IS NULL OR length(site_stable_id) = 64);

CREATE INDEX IF NOT EXISTS idx_raw_event_buffer_site_stable_id
    ON raw_event_buffer(site_stable_id, occurred_at);

CREATE TABLE personal_site_override (
    site_key_hash TEXT PRIMARY KEY NOT NULL CHECK (length(site_key_hash) = 64),
    category TEXT NOT NULL,
    activity_name TEXT
        CHECK(activity_name IS NULL OR (
            length(trim(activity_name)) BETWEEN 1 AND 48
            AND instr(activity_name, char(10)) = 0
            AND instr(activity_name, char(13)) = 0
        )),
    correction_count INTEGER NOT NULL DEFAULT 1 CHECK (correction_count > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE local_site_name (
    site_key_hash TEXT PRIMARY KEY NOT NULL CHECK (length(site_key_hash) = 64),
    host TEXT NOT NULL CHECK (
        length(host) BETWEEN 1 AND 253
        AND host NOT GLOB '*[^a-z0-9.-]*'
    ),
    last_seen_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_local_site_name_last_seen_at
    ON local_site_name(last_seen_at);
