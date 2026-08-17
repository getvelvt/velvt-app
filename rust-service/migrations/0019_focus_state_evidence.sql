-- Device-local, coarse system Focus/DND evidence (roadmap invariant 5; D2).
--
-- Swift observes transitions; Rust owns this record and every decision
-- derived from it. Storage is deliberately coarse: only active/inactive,
-- the transition time floored to the existing five-minute local
-- granularity (the same version-1 five-minute rule the dashboard's
-- switching clusters use), and coarse local hour/date buckets for the
-- deterministic late-night pattern rule. There is structurally no column
-- that could hold a Focus mode's name, configuration, or schedule, and
-- nothing in this table is representable in the upload path.
--
-- Retention is bounded: the service prunes rows older than the pattern
-- rule's lookback window on every insert.

CREATE TABLE focus_state_evidence (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    -- Unix seconds floored to the 300-second bucket.
    changed_at_bucket INTEGER NOT NULL,
    -- Coarse local buckets for the deterministic pattern rule only.
    local_hour INTEGER NOT NULL CHECK(local_hour BETWEEN 0 AND 23),
    local_date TEXT NOT NULL CHECK(length(local_date) = 10),
    recorded_at INTEGER NOT NULL
);

CREATE INDEX idx_focus_state_evidence_bucket
    ON focus_state_evidence(changed_at_bucket DESC);

-- Latest known client UTC offset, so local-hour decisions never require the
-- client's locale or identity. Singleton row.
CREATE TABLE focus_observer_state (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    utc_offset_seconds INTEGER NOT NULL CHECK(utc_offset_seconds BETWEEN -64800 AND 64800),
    updated_at INTEGER NOT NULL
);
