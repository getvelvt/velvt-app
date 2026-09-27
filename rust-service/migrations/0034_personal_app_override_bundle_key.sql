-- The bundle identity an app-scoped rule also answers to.
--
-- `personal_app_override` (0017) is keyed on a hash of the application's NAME,
-- which is the rung almost every correction lands on -- and the name is the part
-- that moves. It is localized, so the same application teaches Velvt one thing
-- in English and nothing at all in French; it changes between releases; and
-- `normalize_classifier_text` folds only ASCII, so an accented name never
-- matched a taxonomy entry to begin with. A rule the user typed once should not
-- quietly stop applying because macOS started reporting a different string.
--
-- Additive and nullable, and the primary key is deliberately unchanged. A
-- bundle-keyed rule is the SAME rule as the name-keyed one, not a second row:
-- one correction writes both keys where a bundle identifier is known, and the
-- engine consults window scope, then this column, then `app_key_hash`. NULL
-- means "this rule predates bundle keying, or was taught for an application
-- macOS reported no bundle identifier for" -- those rows keep matching by name
-- exactly as they do today, which is why nothing is backfilled: the raw
-- application name is discarded at the abstraction boundary, so there is no
-- honest way to recompute a bundle key for an existing row.
--
-- Like `app_key_hash` this is a SHA-256 digest under its own domain separator
-- (`velvt:abstraction-app-bundle-key:v1`), so it carries no recoverable raw text
-- and can collide with neither the name key nor a window key. It never enters an
-- upload DTO or a cloud correction request; no type in `upload/dto.rs` has a
-- field it could occupy.
ALTER TABLE personal_app_override ADD COLUMN bundle_key_hash TEXT
    CHECK (bundle_key_hash IS NULL OR length(bundle_key_hash) = 64);

-- The bundle rung is read on the classification path of every event that misses
-- the window rung, so it is a lookup that has to be indexed. UNIQUE because two
-- rows claiming the same bundle identity would make which rule applies depend on
-- scan order: an application has one bundle identifier, so one rule may claim
-- it. Partial, so the many NULL rows -- every pre-0034 correction -- are exempt
-- rather than colliding with each other.
--
-- This index is NOT an ON CONFLICT target of any upsert, and it cannot be one:
-- the upserts target the primary key, and an application rename arrives as a new
-- NAME key carrying an already-claimed bundle key -- a second constraint, on a
-- row the conflict clause never looks at. Left there, the write aborted with
-- SQLITE_CONSTRAINT on precisely the case bundle keying exists to solve. The
-- writer resolves it before inserting instead: `upsert_app_scope_rule`
-- (`persistence/sqlite.rs`) folds the stale name-keyed alias of the same bundle
-- into the name the application now reports, so at most one row ever claims a
-- bundle by the time the upsert runs. Any new writer of this table must go
-- through that function; a bare INSERT of a name key plus an existing bundle key
-- will fail here, by design.
CREATE UNIQUE INDEX IF NOT EXISTS idx_personal_app_override_bundle_key
    ON personal_app_override(bundle_key_hash)
    WHERE bundle_key_hash IS NOT NULL;
