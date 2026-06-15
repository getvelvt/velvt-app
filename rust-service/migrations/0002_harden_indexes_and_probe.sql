-- Privacy invariant: schema columns may contain only stable IDs, abstract labels,
-- categories, taxonomy versions, timestamps, and ready-to-display cache payloads.
-- Raw app names, window titles, URLs, bundle IDs, paths, and filenames are forbidden.

CREATE INDEX idx_schema_migration_created_at
    ON schema_migration(created_at);

CREATE INDEX idx_abstraction_map_created_at
    ON abstraction_map(created_at);
CREATE INDEX idx_abstraction_map_updated_at
    ON abstraction_map(updated_at);

CREATE INDEX idx_raw_event_buffer_created_at
    ON raw_event_buffer(created_at);

CREATE INDEX idx_upload_batch_sent_at
    ON upload_batch(sent_at);

CREATE INDEX idx_batch_event_occurred_at
    ON batch_event(occurred_at);
CREATE INDEX idx_batch_event_created_at
    ON batch_event(created_at);

CREATE INDEX idx_history_cache_ttl
    ON history_cache(ttl);
CREATE INDEX idx_history_cache_created_at
    ON history_cache(created_at);

CREATE INDEX idx_insight_cache_ttl
    ON insight_cache(ttl);
CREATE INDEX idx_insight_cache_created_at
    ON insight_cache(created_at);

-- This harmless probe proves a new migration file is embedded and applied without
-- modifying the migration runner.
CREATE TABLE persistence_migration_probe (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    marker TEXT NOT NULL UNIQUE CHECK(length(marker) > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_persistence_migration_probe_created_at
    ON persistence_migration_probe(created_at);
