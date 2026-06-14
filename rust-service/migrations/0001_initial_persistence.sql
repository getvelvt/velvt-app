-- Privacy invariant: no schema column may store raw app names, window titles,
-- URLs, bundle IDs, paths, filenames, contacts, or other raw user content.
-- Persist only stable identifiers, abstract labels/categories, timestamps,
-- and ready-to-display local cache payloads.

CREATE TABLE abstraction_map (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key_hash TEXT NOT NULL UNIQUE CHECK(length(key_hash) = 64),
    stable_id TEXT NOT NULL UNIQUE CHECK(length(stable_id) > 0),
    label TEXT NOT NULL CHECK(length(label) > 0),
    category TEXT NOT NULL CHECK(length(category) > 0),
    taxonomy_version TEXT NOT NULL CHECK(length(taxonomy_version) > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE raw_event_buffer (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) > 0),
    stable_id TEXT NOT NULL CHECK(length(stable_id) > 0),
    label TEXT NOT NULL CHECK(length(label) > 0),
    category TEXT NOT NULL CHECK(length(category) > 0),
    taxonomy_version TEXT NOT NULL CHECK(length(taxonomy_version) > 0),
    occurred_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_raw_event_buffer_occurred_at
    ON raw_event_buffer(occurred_at);

CREATE TABLE upload_batch (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL UNIQUE CHECK(length(batch_id) > 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'sent')),
    sent_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_upload_batch_created_at
    ON upload_batch(created_at);

CREATE TABLE batch_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL CHECK(length(batch_id) > 0),
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) > 0),
    stable_id TEXT NOT NULL CHECK(length(stable_id) > 0),
    label TEXT NOT NULL CHECK(length(label) > 0),
    category TEXT NOT NULL CHECK(length(category) > 0),
    taxonomy_version TEXT NOT NULL CHECK(length(taxonomy_version) > 0),
    occurred_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(batch_id) REFERENCES upload_batch(batch_id) ON DELETE CASCADE
);

CREATE INDEX idx_batch_event_batch_id
    ON batch_event(batch_id);

CREATE TABLE history_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL UNIQUE CHECK(date GLOB '????-??-??'),
    payload TEXT NOT NULL CHECK(length(payload) > 0),
    ttl INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_history_cache_date
    ON history_cache(date);

CREATE TABLE insight_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL UNIQUE CHECK(date GLOB '????-??-??'),
    payload TEXT NOT NULL CHECK(length(payload) > 0),
    ttl INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_insight_cache_date
    ON insight_cache(date);
