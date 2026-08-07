-- Initiation invitations (0.1.6 Scope 3): the bounded invitation store, the
-- single Rust-owned opt-out, and the content-free origin marker on blocks.
--
-- Everything here is device-local. The invitation store records only when an
-- invitation was extended and its bounded outcome; the outcome vocabulary is
-- a separate closed enum from the intervention vocabulary because the two
-- record different questions ("did the invitation help a start begin?"
-- versus "was the mid-block interruption right?") and must stay separately
-- countable. `offered` is the only non-terminal state.
--
-- `local_date` exists solely to enforce the versioned daily cap
-- deterministically across UTC-offset changes. Like every work-block
-- timestamp it never leaves this database: no IPC, upload, log, or
-- telemetry path reads this table's timing columns.

CREATE TABLE initiation_invitation (
    invitation_id TEXT PRIMARY KEY NOT NULL CHECK(length(invitation_id) > 0),
    offered_at INTEGER NOT NULL,
    local_date TEXT NOT NULL CHECK(length(local_date) = 10),
    action_id TEXT NOT NULL CHECK(action_id IN ('soft_start_25')),
    policy_version INTEGER NOT NULL CHECK(policy_version >= 1),
    backoff_policy_version INTEGER NOT NULL CHECK(backoff_policy_version >= 1),
    -- `offered` is the only non-terminal state.
    outcome TEXT NOT NULL DEFAULT 'offered'
        CHECK(outcome IN (
            'offered',
            'accepted',
            'dismissed',
            'no_response',
            'expired'
        )),
    outcome_at INTEGER
);

CREATE INDEX idx_initiation_invitation_offered
    ON initiation_invitation(offered_at DESC);
CREATE INDEX idx_initiation_invitation_local_date
    ON initiation_invitation(local_date);

-- The single opt-out (D4). A singleton row like `velvt_quiet_hours`; the
-- explicit user choice survives clearing behavioral data.
CREATE TABLE initiation_settings (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    invitations_enabled INTEGER NOT NULL DEFAULT 1 CHECK(invitations_enabled IN (0, 1)),
    updated_at INTEGER NOT NULL
);

-- Content-free origin marker (R2): an accepted invitation is a declared
-- block, distinguishable from a manual declaration only by this closed
-- two-value enum. It encodes how the block started and nothing about when
-- an invitation is extended, so the marker cannot reconstruct a schedule.
ALTER TABLE work_block ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual'
    CHECK(origin IN ('manual', 'invitation'));
