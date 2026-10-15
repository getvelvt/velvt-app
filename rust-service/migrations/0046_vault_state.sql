-- Sync across your Macs, v1: what this Mac keeps about the encrypted copy of
-- your choices (docs/architecture/personal-vault-and-devices.md, section 4.3;
-- PR A3).
--
-- WHY. Sync keeps one encrypted document per account on Velvt's server, which
-- the server cannot read. Each Mac checks every copy it is served against what
-- it last trusted (section 6.1): the key the copy must be signed with, the
-- highest version it has seen and that version's hash, and the reset it last
-- saw pending. That record is the trust anchor, and it outlives Log Out,
-- turning sync off on this Mac, this Mac's removal and a switch to another
-- account; otherwise a Mac that signed out for five minutes could be shown a
-- forged reset as if it had been offline for three days. The session columns
-- (whether sync is on here, the last copy opened, still encrypted, and the
-- engine's state) are cleared on each of those.
--
-- Nothing here is plaintext of the document, and nothing here names a site,
-- an app, a bundle id or a window: `vault_rule_link` holds this install's
-- salted rule keys and the person keys the document uses, which are HMACs
-- under a salt that exists on disk only inside the ciphertext. No row is
-- written until sync is turned on, and nothing in this build can turn it on.
-- Times are Unix milliseconds.

CREATE TABLE vault_state (
    user_id TEXT PRIMARY KEY CHECK(length(user_id) BETWEEN 1 AND 64),
    -- The trust anchor: kept across Log Out, Turn Off Sync, removal and an
    -- account switch.
    sync_device_id TEXT NOT NULL CHECK(length(sync_device_id) = 36),
    host_binding_salt BLOB NOT NULL CHECK(length(host_binding_salt) = 32),
    -- HMAC-SHA256(host_binding_salt, this Mac's host UUID): a copy of this
    -- database on another Mac (Migration Assistant, a restored backup) no
    -- longer matches, and that Mac takes a new sync_device_id (J8). Never
    -- leaves the Mac.
    host_binding BLOB NOT NULL CHECK(length(host_binding) = 32),
    key_id TEXT CHECK(key_id IS NULL OR length(key_id) = 16),
    write_pk BLOB CHECK(write_pk IS NULL OR length(write_pk) = 32),
    highest_seen_version INTEGER NOT NULL DEFAULT 0 CHECK(highest_seen_version >= 0),
    -- SHA-256 of the copy at highest_seen_version: a different copy at the
    -- same version is a fork.
    highest_seen_sha256 BLOB CHECK(highest_seen_sha256 IS NULL OR length(highest_seen_sha256) = 32),
    fetched_at INTEGER,
    seen_reset_request_id TEXT CHECK(seen_reset_request_id IS NULL OR length(seen_reset_request_id) = 36),
    -- The session: cleared on Log Out, Turn Off Sync, removal and an account
    -- switch.
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0, 1)),
    server_version INTEGER CHECK(server_version IS NULL OR server_version >= 0),
    -- The copy at highest_seen_version, still encrypted. Never plaintext.
    ciphertext BLOB,
    last_hlc TEXT CHECK(last_hlc IS NULL OR length(last_hlc) = 55),
    state TEXT NOT NULL DEFAULT 'off' CHECK(state IN (
        'off', 'available_elsewhere', 'waiting_for_key', 'syncing',
        'waiting_for_rotated_key', 'reset_pending', 'reset_completed',
        'missing_on_server', 'server_copy_unreadable', 'server_inconsistent',
        'read_only_newer_schema', 'clone_detected', 'error')),
    -- An error code only, never a message or content.
    last_error TEXT CHECK(last_error IS NULL OR length(last_error) BETWEEN 1 AND 64)
);

-- Which local rule became which rule in the document. Written by the mirror
-- (PR A6), read when this Mac writes its copy, and cleared with the session.
CREATE TABLE vault_rule_link (
    local_kind TEXT NOT NULL CHECK(local_kind IN ('site', 'bundle', 'app')),
    -- The local rule's key: an HMAC under this install's stable_key_salt.
    local_key_hash TEXT NOT NULL CHECK(length(local_key_hash) = 64),
    -- The document's key for the same rule: an HMAC under person_salt.
    person_key TEXT NOT NULL CHECK(length(person_key) = 64),
    category TEXT NOT NULL CHECK(category IN (
        'FOCUS_WORK', 'PASSIVE_CONSUMPTION', 'SOCIAL_FEED', 'COMMUNICATION',
        'TASK_MANAGEMENT', 'REFERENCE', 'SYSTEM', 'UNLOGGED')),
    deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0, 1)),
    hlc TEXT NOT NULL CHECK(length(hlc) = 55),
    PRIMARY KEY (local_kind, local_key_hash)
);

-- The account's devices this Mac already knows of, so a device that signs in
-- later posts one "New sign-in" notice, and turning sync on posts none (the
-- first list seeds this table without alerting). velvt-core's device ids
-- only. alerted_at is when the notice was queued (sync_notice keeps it until
-- it is posted). Cleared with the session.
CREATE TABLE sync_known_device (
    server_device_id TEXT PRIMARY KEY CHECK(length(server_device_id) = 36),
    first_seen_at INTEGER NOT NULL,
    alerted_at INTEGER
);

-- The security notices this Mac owes the person (design J6): each is posted
-- once, never during a work block, and listed until acknowledged. Kept here
-- so a notice a work block is holding when the service restarts is still
-- posted when the block ends. Ids and times only: the copy is written when
-- the notice is shown, and a Mac's name is read then from the synced copy,
-- so no name is stored. Cleared at Log Out, when another account signs in,
-- and with the account's vault_state row; an acknowledged row goes 30 days
-- after it was acknowledged.
CREATE TABLE sync_notice (
    user_id TEXT NOT NULL CHECK(length(user_id) BETWEEN 1 AND 64),
    notice_id TEXT NOT NULL CHECK(length(notice_id) BETWEEN 1 AND 128),
    kind TEXT NOT NULL CHECK(kind IN (
        'new_sign_in', 'mac_joined_sync', 'this_mac_removed', 'sync_key_changed',
        'reset_requested', 'reset_completed', 'server_inconsistent')),
    -- The id the notice is about: a velvt-core device id, a sync device id,
    -- a key id, a reset id, or an error code and a key id. Never a name.
    subject TEXT CHECK(subject IS NULL OR length(subject) BETWEEN 1 AND 96),
    -- The times the copy names (a reset's effective time is until_at), and
    -- the one switch it reads: adopted, while offline, or joined by code.
    at INTEGER NOT NULL,
    until_at INTEGER,
    flag INTEGER NOT NULL DEFAULT 0 CHECK(flag IN (0, 1)),
    queued_at INTEGER NOT NULL,
    posted_at INTEGER,
    acknowledged_at INTEGER,
    PRIMARY KEY (user_id, notice_id)
);
