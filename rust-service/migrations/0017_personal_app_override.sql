-- App-scoped classification corrections.
--
-- `personal_override` is keyed on the (app name, window title) hash, so a
-- correction teaches Velvt about exactly one window. Opening a second file in
-- the same editor produces a different title, a different hash, and an
-- unclassified event again — which is why "Unclassified" dominates a real
-- user's day no matter how often they correct it.
--
-- This table is keyed on the application alone, so one correction covers every
-- window of that app. `personal_override` still wins where both exist: a
-- correction naming a specific window is a more specific statement of intent
-- than one naming the app.
--
-- `app_key_hash` is a salted SHA-256 of the raw application name under its own
-- domain separator. Like `key_hash` it carries no recoverable raw text and
-- never enters an upload DTO or a cloud correction request.
CREATE TABLE personal_app_override (
    app_key_hash TEXT PRIMARY KEY NOT NULL CHECK (length(app_key_hash) = 64),
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

-- The app identity an event was classified under, so a later correction can be
-- generalized to the app without retaining the raw application name. Null for
-- rows written before this migration: those events predate app-scoped
-- corrections and cannot be generalized retroactively.
ALTER TABLE raw_event_buffer ADD COLUMN app_stable_id TEXT
    CHECK (app_stable_id IS NULL OR length(app_stable_id) = 64);

-- Whether generalizing a correction to the whole app is meaningful.
--
-- False for a browser window that carried a site context: one Chrome tab being
-- focus work says nothing about the next tab, and generalizing there would
-- mislabel a whole browsing session from a single correction. Those events
-- keep window-scoped corrections only.
ALTER TABLE raw_event_buffer ADD COLUMN app_scope_eligible INTEGER NOT NULL DEFAULT 1
    CHECK (app_scope_eligible IN (0, 1));
