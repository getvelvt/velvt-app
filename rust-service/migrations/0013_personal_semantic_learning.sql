CREATE TABLE semantic_embedding_cache (
    key_hash TEXT PRIMARY KEY NOT NULL CHECK (length(key_hash) = 64),
    embedding BLOB NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions > 0 AND dimensions <= 1024),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_semantic_embedding_cache_updated_at
    ON semantic_embedding_cache(updated_at);

CREATE TABLE personal_semantic_prototype (
    key_hash TEXT PRIMARY KEY NOT NULL CHECK (length(key_hash) = 64),
    category TEXT NOT NULL,
    embedding BLOB NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions > 0 AND dimensions <= 1024),
    correction_count INTEGER NOT NULL DEFAULT 1 CHECK (correction_count > 0),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_personal_semantic_prototype_category_updated
    ON personal_semantic_prototype(category, updated_at DESC);

CREATE TABLE classifier_artifact_telemetry (
    artifact_version TEXT PRIMARY KEY NOT NULL,
    classification_count INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
