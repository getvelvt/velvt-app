-- A terminal status for an upload batch that has exhausted its retry ceiling.
--
-- `pending` and `failed` were the two statuses no sweep collected, and the
-- unbounded path into them is not revocation -- `upload_eligible()` gates
-- ingestion, so a revoked device stops minting batches. It is sustained
-- transport failure: a transport error restores the authenticated state rather
-- than degrading it, so while the host is unreachable the device stays
-- upload-eligible, keeps minting batches, and every attempt lands back in the
-- retry queue. A transient outage self-heals. A retired host does not, and the
-- queue grows for as long as the service runs.
--
-- The ceiling is 288 attempts (`UPLOAD_BATCH_ATTEMPT_CEILING`), and it is set
-- from observed recovery rather than from the backoff arithmetic. `mark_sent`
-- does not reset `attempt_count`, so a delivered batch carries the cumulative
-- cost of every outage it sat through: across 2,604 delivered batches on the
-- development device the counts read 2,489 at zero, 39 at 1-9, 15 at 10-47, 60
-- at 48-95, and one at 116, that last one a ~30-hour outage on 2026-08-21 that
-- the device fully recovered from. The 76 batches with ten or more attempts
-- retried at 3.53-3.87 attempts an hour. 288 is 74-82 hours at that rate, so
-- the margin over the longest recovery actually observed is 2.5x. A first draft
-- of this ceiling was 96, below that observed maximum, which would have
-- abandoned a batch the device went on to deliver.
--
-- `abandoned` is deliberately not `rejected`. `rejected` means the backend
-- refused the batch on a privacy check, and the menu bar renders that status as
-- "Privacy check failed" -- reusing it for a network outage would put a
-- sentence in front of the user that is not true. `abandoned` is terminal at
-- creation: nothing retries it, `resumable_batches` does not return it, and the
-- age sweep collects it on the same horizon as a sent batch.
--
-- SQLite cannot alter a CHECK constraint, so the table is rebuilt -- the same
-- pattern as migrations 0003, 0015, 0016, 0020 and 0023. `upload_batch` is the
-- parent of `batch_event`, so both are renamed before either is dropped and the
-- child is dropped first: with foreign keys enabled a DROP of the parent
-- performs an implicit DELETE, which would cascade every queued event away.
-- Existing rows are copied unchanged.

ALTER TABLE batch_event RENAME TO batch_event_v30_backup;
ALTER TABLE upload_batch RENAME TO upload_batch_v30_backup;

CREATE TABLE upload_batch (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL UNIQUE CHECK(length(batch_id) > 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'sent', 'failed', 'rejected', 'abandoned')),
    sent_at INTEGER,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    last_error_code TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO upload_batch(
    id, batch_id, status, sent_at, attempt_count, next_attempt_at, last_error_code, created_at
)
SELECT id, batch_id, status, sent_at, attempt_count, next_attempt_at, last_error_code, created_at
FROM upload_batch_v30_backup;

CREATE TABLE batch_event (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    batch_id TEXT NOT NULL CHECK(length(batch_id) > 0),
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) > 0),
    stable_id TEXT NOT NULL CHECK(length(stable_id) > 0),
    label TEXT NOT NULL CHECK(length(label) > 0),
    category TEXT NOT NULL CHECK(length(category) > 0),
    taxonomy_version TEXT NOT NULL CHECK(length(taxonomy_version) > 0),
    classification_tier TEXT NOT NULL DEFAULT 'fallback',
    occurred_at INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(batch_id) REFERENCES upload_batch(batch_id) ON DELETE CASCADE
);

INSERT INTO batch_event(
    id, batch_id, event_id, stable_id, label, category, taxonomy_version,
    classification_tier, occurred_at, duration_seconds, created_at
)
SELECT id, batch_id, event_id, stable_id, label, category, taxonomy_version,
       classification_tier, occurred_at, duration_seconds, created_at
FROM batch_event_v30_backup;

DROP TABLE batch_event_v30_backup;
DROP TABLE upload_batch_v30_backup;

CREATE INDEX idx_upload_batch_retry_due
    ON upload_batch(status, next_attempt_at, created_at);
CREATE INDEX idx_upload_batch_created_at ON upload_batch(created_at);
CREATE INDEX idx_upload_batch_sent_at ON upload_batch(sent_at);
CREATE INDEX idx_batch_event_batch_id ON batch_event(batch_id);
CREATE INDEX idx_batch_event_occurred_at ON batch_event(occurred_at);
CREATE INDEX idx_batch_event_created_at ON batch_event(created_at);
