-- The egress ledger: one row for every HTTP request the helper makes, written
-- before the request is sent.
--
-- WHY. PRIVACY.md lists what leaves this Mac and the upload DTO
-- (`upload/dto.rs`) is the ground truth for what an event batch carries. A
-- reader could check both against the source, but nothing on disk recorded what
-- was actually sent. `ReqwestHttpClient` (`auth/http.rs`) holds the only network
-- client in rust-service, and its constructor takes this ledger: every request
-- is appended here first, and a request whose row cannot be written is not sent.
--
-- WHAT A ROW HOLDS. When (`recorded_at`, unix seconds), the method and full URL,
-- the number of body bytes, the SHA-256 of those exact bytes, whether a bearer
-- credential was attached, the previous row's hash, and this row's hash. The
-- body itself is not stored. A queued upload batch is on disk already in
-- `upload_batch`/`batch_event`, and `velvt-service --dry-run-egress` prints it
-- byte for byte with the same SHA-256 this table will record when it is sent.
--
-- Two bodies carry a secret: sign-up and log-in send a password, and a token
-- refresh sends the refresh token. An unsalted SHA-256 of a password is
-- something a holder of this file could test guesses against, so for those
-- bodies `body_sha256` is the hash of the body with each secret value replaced
-- by the string "[redacted]", and `body_redacted` is 1. `body_bytes` is still
-- the length of what was sent.
--
-- THE CHAIN. `entry_hash` is the SHA-256 of the UTF-8 line
--
--   velvt-egress-v1|seq|recorded_at|method|endpoint|body_bytes|body_sha256|body_redacted|bearer|prev_hash
--
-- and `prev_hash` is the `entry_hash` of `seq - 1`, or 64 zeros for seq 1.
-- `scripts/prove_egress.sh` recomputes every hash with sqlite3 and perl. The
-- URL is the only free-text field, so it may not contain `|` or a line break
-- (the CHECK below; `egress::ledger_endpoint` percent-encodes them).
--
-- WHAT THE CHAIN PROVES, AND WHAT IT DOES NOT. Editing or deleting a row in the
-- middle breaks every hash after it, and the triggers below refuse an UPDATE, a
-- DELETE outside retention, and an INSERT that does not extend the head. It does
-- not stop someone with write access to this file from dropping the triggers
-- and rewriting the whole chain, or from cutting entries off the end: the file
-- is on your disk and holds its own anchor. Writing down the head hash the
-- verifier prints, and checking a later run still contains it, is the external
-- anchor. It also records requests, not what the network saw: DNS lookups and
-- TLS handshakes are not rows, and a request that failed to connect still is.
--
-- RETENTION. 30 days or 100,000 rows, whichever is tighter
-- (`egress::EGRESS_LEDGER_RETENTION_DAYS`, `EGRESS_LEDGER_MAX_ENTRIES`), pruned
-- oldest first by `EgressLedgerRetentionTarget`. Pruning takes the oldest rows
-- only. Before it deletes, it writes `egress_ledger_checkpoint` with the seq and
-- hash of the last row it removes, so the first surviving row still links to
-- something. Only the newest checkpoint is kept; the triggers refuse to delete
-- it, and refuse to delete any ledger row it does not cover.

CREATE TABLE egress_ledger (
    seq INTEGER PRIMARY KEY CHECK (seq >= 1),
    recorded_at INTEGER NOT NULL,
    method TEXT NOT NULL CHECK (method IN ('GET', 'POST', 'PATCH', 'DELETE')),
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

CREATE INDEX idx_egress_ledger_recorded_at ON egress_ledger(recorded_at);

CREATE TABLE egress_ledger_checkpoint (
    through_seq INTEGER PRIMARY KEY CHECK (through_seq >= 1),
    through_hash TEXT NOT NULL CHECK (length(through_hash) = 64),
    created_at INTEGER NOT NULL
);

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

CREATE TRIGGER trg_egress_ledger_checkpoint_no_update
BEFORE UPDATE ON egress_ledger_checkpoint
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger_checkpoint is append-only');
END;

CREATE TRIGGER trg_egress_ledger_checkpoint_keeps_newest
BEFORE DELETE ON egress_ledger_checkpoint
WHEN OLD.through_seq >= (SELECT MAX(through_seq) FROM egress_ledger_checkpoint)
BEGIN
    SELECT RAISE(ABORT, 'egress_ledger_checkpoint: the newest checkpoint is the chain anchor');
END;
