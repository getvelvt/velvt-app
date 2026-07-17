-- Device-local meaningful-work state. `intention` is the only free-form field
-- and is intentionally isolated from upload/cache tables with a short expiry.
CREATE TABLE work_block (
    block_id TEXT PRIMARY KEY NOT NULL CHECK(length(block_id) > 0),
    state_version INTEGER NOT NULL DEFAULT 1 CHECK(state_version = 1),
    phase TEXT NOT NULL CHECK(phase IN ('active', 'paused', 'completed', 'abandoned', 'expired')),
    intention TEXT CHECK(intention IS NULL OR (length(intention) <= 120 AND intention NOT LIKE '%' || char(10) || '%' AND intention NOT LIKE '%' || char(13) || '%')),
    purpose TEXT CHECK(purpose IS NULL OR purpose IN ('deep_work', 'study', 'creative_practice', 'healthy_tech_use', 'work_life_boundary')),
    intensity TEXT NOT NULL CHECK(intensity IN ('light', 'medium', 'intense')),
    planned_duration_seconds INTEGER NOT NULL CHECK(planned_duration_seconds BETWEEN 300 AND 10800),
    started_at INTEGER NOT NULL,
    paused_at INTEGER,
    total_paused_seconds INTEGER NOT NULL DEFAULT 0 CHECK(total_paused_seconds >= 0),
    ended_at INTEGER,
    recovered_after_restart INTEGER NOT NULL DEFAULT 0 CHECK(recovered_after_restart IN (0, 1)),
    recovery_of TEXT,
    intention_expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(recovery_of) REFERENCES work_block(block_id) ON DELETE SET NULL
);

CREATE INDEX idx_work_block_phase_updated
    ON work_block(phase, updated_at DESC);
CREATE INDEX idx_work_block_intention_expiry
    ON work_block(intention_expires_at) WHERE intention IS NOT NULL;

-- Safe category observations only. No app identity, title, URL, local display
-- label, stable mapping key, or intention is representable in this table.
CREATE TABLE work_block_observation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    block_id TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    ended_at INTEGER,
    category TEXT NOT NULL CHECK(length(category) > 0),
    classification_status TEXT NOT NULL CHECK(classification_status IN ('classified', 'ambiguous', 'unclassified')),
    classification_confidence TEXT NOT NULL CHECK(classification_confidence IN ('high', 'medium', 'low', 'none')),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

CREATE INDEX idx_work_block_observation_block_time
    ON work_block_observation(block_id, occurred_at, id);
CREATE UNIQUE INDEX idx_work_block_observation_one_open
    ON work_block_observation(block_id) WHERE ended_at IS NULL;

-- Rust-authored safe result JSON. The result DTO structurally excludes
-- intention and every raw/local identity field. UNIQUE block_id makes
-- completion idempotent across retries and restart recovery.
CREATE TABLE work_block_result (
    block_id TEXT PRIMARY KEY NOT NULL,
    payload TEXT NOT NULL CHECK(length(payload) > 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);
