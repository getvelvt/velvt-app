-- The per-install key every stored identity digest is computed under, and the
-- re-key of every digest already on disk.
--
-- WHY. Until this migration the three keys in `abstraction/key.rs` were plain
-- SHA-256 over a public domain string and the raw inputs:
--
--   * the window key, over the application name and the full raw window title
--     (or the focused site, for a browser tab) -- `abstraction_map.key_hash`,
--     `personal_override.key_hash`, and the two vector stores' `key_hash`;
--   * the application key, over the application name -- `app_stable_id`,
--     `personal_app_override.app_key_hash`, `personal_override.app_key_hash`;
--   * the bundle key, over the bundle identifier -- `app_bundle_stable_id` and
--     `personal_app_override.bundle_key_hash`.
--
-- Computed identically on every install, so anyone holding the file could
-- confirm a guess -- "was `Re: offer letter` open in Mail?" -- with one hash and
-- this repository's source, and a table of guesses computed once matched every
-- Velvt database there is. The bundle key was worse: the set of macOS bundle
-- identifiers is small and public, so its digest named the application outright,
-- and the same 64 characters on every Mac let two databases be joined on it.
-- (0036's header calls `app_key_hash` "a salted SHA-256 digest". It was not
-- salted; it is from here on.)
--
-- WHAT. One 32-byte random value per install, generated here by the database,
-- and every key becomes
--
--     HMAC-SHA-256(salt, "velvt:abstraction-salted-key:v1" || old digest)
--
-- The HMAC is taken over the old digest, not over the raw inputs, and that is the
-- only way this migration can exist: the raw application name and window title
-- are discarded at the abstraction boundary, so the digest is all a stored row
-- has left to re-key from. It costs nothing. Without the salt, HMAC over a
-- SHA-256 digest is as untestable as HMAC over the input; with the salt, both
-- cost one hash per guess. The domain strings stay inside the digest, so the
-- three kinds of key still cannot collide with one another.
--
-- WHAT IT DOES NOT DO, PLAINLY. The salt sits in this file, beside the keys, and
-- has to: a correction must still match its window after a restart, so the key
-- that computed it cannot be ephemeral. Someone who has the whole database can
-- read the salt and confirm a guess exactly as before. What this removes is the
-- offline, source-only attack and the cross-install join: a guess now has to be
-- tested per device, with that device's salt in hand, and no key on one Mac
-- equals a key on another. `semantic_embedding_cache` made the same trade in
-- 0031 for the same reason. What bounds how long a window's key sits here at all
-- is the `abstraction_map` retention sweep this change registers
-- (`AbstractionMapRetentionTarget`), not the salt.
--
-- WHY NOT `embedding_salt`. That row may be re-minted when absent
-- (`AbstractionMapRepo::embedding_salt`), and re-minting it costs a cache and the
-- learned prototypes -- 0031 promises corrections are NOT touched. A key salt that
-- changed would orphan every correction the user ever made. Different failure
-- costs, different rows: re-minting this one is handled separately and removes
-- what it orphans (`AbstractionMapRepo::stable_key_salt`).
--
-- HOW. SQL cannot compute an HMAC, so this file is the schema half -- the salt,
-- minted here by the database, and one index -- and the data half is Rust:
-- `run_migrations` calls `rekey_stored_digests` (`persistence/sqlite.rs`)
-- immediately after this file, in the same transaction, and only while this
-- migration is the one being applied, so it runs exactly once per database. It
-- re-keys every column in `KEYED_COLUMNS` with `StableKeySalt::rekey_stored_digest`,
-- the function the engine keys fresh events with, so a key re-keyed here and a
-- key computed tomorrow for the same window are equal by construction.
--
-- A stored value that is not a digest as `key.rs` writes one (64 lowercase hex)
-- was never a key Velvt produced and could never have matched a lookup. It is
-- not keyed: its row is removed where the key is the row's identity, and the
-- value is set to NULL where the column is optional. Every CHECK in the schema
-- makes that set empty in practice; it is handled so an upgrade cannot fail --
-- and stop the service from starting -- over a row that never worked.
--
-- Replaying this file with `sqlite3` alone, as the scripts under
-- `scripts/tests/` do to build a schema, gives the right tables and a salt and
-- re-keys nothing -- which, on the empty tables those scripts start from, is the
-- same database.
--
-- Corrections keep working. `personal_override` and `personal_app_override` are
-- re-keyed with the same function as the rows they are looked up against, and
-- `personal_override.app_key_hash` with the same function as the rung it pairs
-- with, so every pairing 0036 recorded still joins, and a correction taught
-- under 1.0.11 applies to the same window, application and bundle after the
-- upgrade.
--
-- Nothing on the wire changes. No key has ever been in an upload payload --
-- `BatchEventPayload` serializes an event id, a timestamp, a category-scoped
-- abstraction type and its version, a tier, and a payload of a category and a
-- duration, and `dto.rs` closes that key set -- and `stable_id` (`abs_<uuid>`),
-- which `batch_event` does hold, is random rather than derived and is not
-- touched here. The salt never leaves the device: no type in `upload::dto` has a
-- field it could occupy, `StableKeySalt` implements no serializer, and its
-- `Debug` prints `StableKeySalt(redacted)`.
--
-- One index, for the sweep: `delete_expired_mappings` keeps a mapping that a
-- buffered event still points at, which is a lookup of `raw_event_buffer` by
-- `stable_id` per candidate row. The correction paths already query that column
-- by equality and were scanning the buffer to do it.

CREATE TABLE stable_key_salt (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    salt BLOB NOT NULL CHECK(length(salt) = 32),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO stable_key_salt(id, salt) VALUES (1, randomblob(32));

CREATE INDEX IF NOT EXISTS idx_raw_event_buffer_stable_id
    ON raw_event_buffer(stable_id);
