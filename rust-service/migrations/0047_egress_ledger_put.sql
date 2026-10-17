-- The egress ledger records PUT requests too.
--
-- WHY. Sync across your Macs writes its encrypted copy with
-- `PUT /v1/vault/blobs/prefs` (docs/architecture/personal-vault-and-devices.md,
-- section 8.3), and migration 0038 let the ledger hold only GET, POST, PATCH
-- and DELETE. `ReqwestHttpClient` sends nothing it cannot record first, so
-- without this every vault write would fail closed. Nothing in this build
-- sends a PUT; this only lets the ledger record one.
--
-- HOW. SQLite cannot change a CHECK constraint, so the table is rebuilt: the
-- same columns and constraints with PUT added to `method`, every row copied
-- as it is (so the hash chain, and the checkpoint behind it, verify exactly as
-- before), and the index and triggers of migration 0038 made again. The one
-- checkpoint trigger that reads `egress_ledger` is dropped first and made
-- again last, because SQLite will not rename a table while a trigger names
-- one that does not exist. Dropping a table fires no DELETE trigger, so the
-- append-only rules are not bypassed for any row: every row is in the new
-- table before the old one goes, and the migration runs in one transaction.

DROP TRIGGER trg_egress_ledger_checkpoint_names_live_entry;

CREATE TABLE egress_ledger_next (
    seq INTEGER PRIMARY KEY CHECK (seq >= 1),
    recorded_at INTEGER NOT NULL,
    method TEXT NOT NULL CHECK (method IN ('GET', 'POST', 'PUT', 'PATCH', 'DELETE')),
    endpoint TEXT NOT NULL CHECK (
        length(endpoint) > 0
        AND instr(endpoint, '|') = 0
        AND instr(endpoint, char(10)) = 0
        AND instr(endpoint, char(13)) = 0
    ),
    body_bytes INTEGER NOT NULL CHECK (body_bytes >= 0),
    body_sha256 TEXT NOT NULL CHECK (length(body_sha256) = 64),
    body_redacted INTEGER NOT NULL CHECK (body_redacted IN (0, 1)),
    bearer INTEGER NOT NULL CHECK (bearer IN (0, 1)),
    prev_hash TEXT NOT NULL CHECK (length(prev_hash) = 64),
    entry_hash TEXT NOT NULL UNIQUE CHECK (length(entry_hash) = 64)
);

INSERT INTO egress_ledger_next (
    seq, recorded_at, method, endpoint, body_bytes, body_sha256, body_redacted,
    bearer, prev_hash, entry_hash
)
SELECT seq, recorded_at, method, endpoint, body_bytes, body_sha256, body_redacted,
       bearer, prev_hash, entry_hash
FROM egress_ledger
ORDER BY seq;

DROP TABLE egress_ledger;

ALTER TABLE egress_ledger_next RENAME TO egress_ledger;

CREATE INDEX idx_egress_ledger_recorded_at ON egress_ledger(recorded_at);

CREATE TRIGGER trg_egress_ledger_insert_extends_head
BEFORE INSERT ON egress_ledger
WHEN NEW.seq != 1 + COALESCE(
        (SELECT MAX(seq) FROM egress_ledger),
        (SELECT MAX(through_seq) FROM egress_ledger_checkpoint),
        0)
  OR NEW.prev_hash != COALESCE(
        (SELECT entry_hash FROM egress_ledger ORDER BY seq DESC LIMIT 1),
        (SELECT through_hash FROM egress_ledger_checkpoint ORDER BY through_seq DESC LIMIT 1),
        '0000000000000000000000000000000000000000000000000000000000000000')
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger: an entry must extend the chain head');
END;

CREATE TRIGGER trg_egress_ledger_no_update
BEFORE UPDATE ON egress_ledger
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger is append-only');
END;

CREATE TRIGGER trg_egress_ledger_delete_behind_checkpoint
BEFORE DELETE ON egress_ledger
WHEN OLD.seq > COALESCE((SELECT MAX(through_seq) FROM egress_ledger_checkpoint), 0)
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger: only entries behind a checkpoint may be pruned');
END;

CREATE TRIGGER trg_egress_ledger_checkpoint_names_live_entry
BEFORE INSERT ON egress_ledger_checkpoint
WHEN NOT EXISTS (
        SELECT 1 FROM egress_ledger
        WHERE seq = NEW.through_seq AND entry_hash = NEW.through_hash)
  OR NEW.through_seq <= COALESCE((SELECT MAX(through_seq) FROM egress_ledger_checkpoint), 0)
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger_checkpoint must name a live entry past the last checkpoint');
END;
