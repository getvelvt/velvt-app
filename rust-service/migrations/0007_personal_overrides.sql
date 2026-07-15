-- Local-only rehydration data. This column is never selected into upload DTOs,
-- serialized to cloud APIs, or emitted in Debug/log output.
ALTER TABLE abstraction_map ADD COLUMN display_name TEXT;

CREATE TABLE personal_override (
    key_hash TEXT PRIMARY KEY NOT NULL,
    category TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE classification_telemetry (
    taxonomy_version TEXT NOT NULL,
    classification_tier TEXT NOT NULL,
    event_count INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (taxonomy_version, classification_tier)
);
