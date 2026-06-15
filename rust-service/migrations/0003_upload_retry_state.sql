-- Upload retry state is privacy-safe control metadata only. Rebuild the two
-- related tables because SQLite cannot extend an existing CHECK constraint.
ALTER TABLE batch_event RENAME TO batch_event_v2_backup;
ALTER TABLE upload_batch RENAME TO upload_batch_v2_backup;
ALTER TABLE raw_event_buffer ADD COLUMN duration_seconds INTEGER NOT NULL DEFAULT 0;

CREATE TABLE upload_batch (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL UNIQUE CHECK(length(batch_id) > 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'sent', 'failed', 'rejected')),
    sent_at INTEGER,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error_code TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO upload_batch(id, batch_id, status, sent_at, created_at)
SELECT id, batch_id, status, sent_at, created_at FROM upload_batch_v2_backup;

CREATE TABLE batch_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL CHECK(length(batch_id) > 0),
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) > 0),
    stable_id TEXT NOT NULL CHECK(length(stable_id) > 0),
    label TEXT NOT NULL CHECK(length(label) > 0),
    category TEXT NOT NULL CHECK(length(category) > 0),
    taxonomy_version TEXT NOT NULL CHECK(length(taxonomy_version) > 0),
    occurred_at INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(batch_id) REFERENCES upload_batch(batch_id) ON DELETE CASCADE
);

INSERT INTO batch_event(
    id, batch_id, event_id, stable_id, label, category, taxonomy_version, occurred_at, created_at
)
SELECT id, batch_id, event_id, stable_id, label, category, taxonomy_version, occurred_at, created_at
FROM batch_event_v2_backup;

DROP TABLE batch_event_v2_backup;
DROP TABLE upload_batch_v2_backup;

CREATE INDEX idx_upload_batch_retry_due
    ON upload_batch(status, next_attempt_at, created_at);
CREATE INDEX idx_upload_batch_created_at ON upload_batch(created_at);
CREATE INDEX idx_upload_batch_sent_at ON upload_batch(sent_at);
CREATE INDEX idx_batch_event_batch_id ON batch_event(batch_id);
CREATE INDEX idx_batch_event_occurred_at ON batch_event(occurred_at);
CREATE INDEX idx_batch_event_created_at ON batch_event(created_at);

CREATE TABLE upload_host_backoff (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    host TEXT NOT NULL UNIQUE CHECK(length(host) > 0),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_upload_host_backoff_next_attempt_at
    ON upload_host_backoff(next_attempt_at);
