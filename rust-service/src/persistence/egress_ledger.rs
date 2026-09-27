//! The `egress_ledger` table (migration 0038): append, read, and prune.

use super::{PersistenceError, SqlitePersistence};
use crate::egress::{entry_hash, EgressCheckpoint, EgressLedgerEntry, EgressRecord, GENESIS_HASH};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

/// The ledger `ReqwestHttpClient` writes before every send.
pub trait EgressLedgerRepo: Send + Sync {
    /// Appends `record` at the head of the chain and returns the stored entry.
    ///
    /// The read of the head and the insert are one IMMEDIATE transaction, so two
    /// sends in flight at once cannot both link to the same predecessor.
    fn append(
        &self,
        record: &EgressRecord,
        recorded_at: DateTime<Utc>,
    ) -> Result<EgressLedgerEntry, PersistenceError>;

    /// Every entry still on disk, oldest first.
    fn entries(&self) -> Result<Vec<EgressLedgerEntry>, PersistenceError>;

    /// The newest checkpoint, if retention has ever pruned.
    fn checkpoint(&self) -> Result<Option<EgressCheckpoint>, PersistenceError>;

    /// Removes up to `batch_size` of the oldest entries that are older than
    /// `cutoff` or beyond the newest `max_entries`, recording a checkpoint for
    /// the last one first. Returns how many were removed.
    fn prune(
        &self,
        cutoff: DateTime<Utc>,
        max_entries: u64,
        batch_size: usize,
        now: DateTime<Utc>,
    ) -> Result<u64, PersistenceError>;
}

pub(super) struct SqliteEgressLedgerRepo(pub(super) SqlitePersistence);

impl EgressLedgerRepo for SqliteEgressLedgerRepo {
    fn append(
        &self,
        record: &EgressRecord,
        recorded_at: DateTime<Utc>,
    ) -> Result<EgressLedgerEntry, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (head_seq, head_hash) = head(&transaction)?;
        let mut entry = EgressLedgerEntry {
            seq: head_seq + 1,
            recorded_at: recorded_at.timestamp(),
            method: record.method.clone(),
            endpoint: record.endpoint.clone(),
            body_bytes: i64::try_from(record.body_bytes).unwrap_or(i64::MAX),
            body_sha256: record.body_sha256.clone(),
            body_redacted: record.body_redacted,
            bearer: record.bearer,
            prev_hash: head_hash,
            entry_hash: String::new(),
        };
        entry.entry_hash = entry_hash(
            entry.seq,
            entry.recorded_at,
            &entry.method,
            &entry.endpoint,
            entry.body_bytes,
            &entry.body_sha256,
            entry.body_redacted,
            entry.bearer,
            &entry.prev_hash,
        );
        transaction.execute(
            "INSERT INTO egress_ledger (seq, recorded_at, method, endpoint, body_bytes,
                 body_sha256, body_redacted, bearer, prev_hash, entry_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.seq,
                entry.recorded_at,
                entry.method,
                entry.endpoint,
                entry.body_bytes,
                entry.body_sha256,
                entry.body_redacted,
                entry.bearer,
                entry.prev_hash,
                entry.entry_hash,
            ],
        )?;
        transaction.commit()?;
        Ok(entry)
    }

    fn entries(&self) -> Result<Vec<EgressLedgerEntry>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT seq, recorded_at, method, endpoint, body_bytes, body_sha256,
                    body_redacted, bearer, prev_hash, entry_hash
             FROM egress_ledger ORDER BY seq",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(EgressLedgerEntry {
                seq: row.get(0)?,
                recorded_at: row.get(1)?,
                method: row.get(2)?,
                endpoint: row.get(3)?,
                body_bytes: row.get(4)?,
                body_sha256: row.get(5)?,
                body_redacted: row.get(6)?,
                bearer: row.get(7)?,
                prev_hash: row.get(8)?,
                entry_hash: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn checkpoint(&self) -> Result<Option<EgressCheckpoint>, PersistenceError> {
        let connection = self.0.connection()?;
        newest_checkpoint(&connection)
    }

    fn prune(
        &self,
        cutoff: DateTime<Utc>,
        max_entries: u64,
        batch_size: usize,
        now: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let bounds: Option<(i64, i64)> = transaction
            .query_row("SELECT MIN(seq), MAX(seq) FROM egress_ledger", [], |row| {
                Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map(|(first, last)| first.zip(last))?;
        let Some((first, last)) = bounds else {
            return Ok(0);
        };
        // Seq is contiguous from `first` (the insert trigger refuses anything
        // else), so the entries past the cap are exactly the oldest ones.
        let through_by_age: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM egress_ledger WHERE recorded_at < ?1",
            [cutoff.timestamp()],
            |row| row.get(0),
        )?;
        let through_by_count = last - i64::try_from(max_entries).unwrap_or(i64::MAX);
        let batch_limit = first - 1 + i64::try_from(batch_size).unwrap_or(i64::MAX);
        let through = through_by_age.max(through_by_count).min(batch_limit);
        if through < first {
            return Ok(0);
        }
        let through_hash: String = transaction.query_row(
            "SELECT entry_hash FROM egress_ledger WHERE seq = ?1",
            [through],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO egress_ledger_checkpoint (through_seq, through_hash, created_at)
             VALUES (?1, ?2, ?3)",
            params![through, through_hash, now.timestamp()],
        )?;
        let deleted =
            transaction.execute("DELETE FROM egress_ledger WHERE seq <= ?1", [through])?;
        transaction.execute(
            "DELETE FROM egress_ledger_checkpoint WHERE through_seq < ?1",
            [through],
        )?;
        transaction.commit()?;
        Ok(deleted as u64)
    }
}

/// The seq and hash a new entry links to: the newest entry, else the newest
/// checkpoint, else the genesis.
fn head(connection: &rusqlite::Connection) -> Result<(i64, String), PersistenceError> {
    let newest = connection
        .query_row(
            "SELECT seq, entry_hash FROM egress_ledger ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some(newest) = newest {
        return Ok(newest);
    }
    Ok(newest_checkpoint(connection)?.map_or_else(
        || (0, GENESIS_HASH.to_owned()),
        |checkpoint| (checkpoint.through_seq, checkpoint.through_hash),
    ))
}

fn newest_checkpoint(
    connection: &rusqlite::Connection,
) -> Result<Option<EgressCheckpoint>, PersistenceError> {
    connection
        .query_row(
            "SELECT through_seq, through_hash FROM egress_ledger_checkpoint
             ORDER BY through_seq DESC LIMIT 1",
            [],
            |row| {
                Ok(EgressCheckpoint {
                    through_seq: row.get(0)?,
                    through_hash: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::{sha256_hex, verify_chain};
    use chrono::TimeZone;

    fn record(path: &str) -> EgressRecord {
        EgressRecord::new("GET", &format!("https://h{path}"), &[], None, true)
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + seconds, 0).unwrap()
    }

    fn ledger() -> (SqlitePersistence, std::sync::Arc<dyn EgressLedgerRepo>) {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let repo = persistence.egress_ledger_repo();
        (persistence, repo)
    }

    #[test]
    fn appended_entries_form_a_chain_from_genesis() {
        let (_persistence, repo) = ledger();
        let first = repo.append(&record("/a"), at(0)).unwrap();
        let second = repo.append(&record("/b"), at(1)).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(first.prev_hash, GENESIS_HASH);
        assert_eq!(second.prev_hash, first.entry_hash);
        let entries = repo.entries().unwrap();
        assert_eq!(entries, vec![first, second]);
        let summary = verify_chain(None, &entries).unwrap();
        assert_eq!(summary.head_seq, 2);
    }

    #[test]
    fn the_database_refuses_to_edit_an_entry() {
        let (persistence, repo) = ledger();
        repo.append(&record("/a"), at(0)).unwrap();
        let connection = persistence.connection().unwrap();
        let error = connection
            .execute("UPDATE egress_ledger SET body_bytes = 1 WHERE seq = 1", [])
            .unwrap_err();
        assert!(error.to_string().contains("append-only"), "{error}");
    }

    #[test]
    fn the_database_refuses_to_delete_an_entry_outside_retention() {
        let (persistence, repo) = ledger();
        repo.append(&record("/a"), at(0)).unwrap();
        repo.append(&record("/b"), at(1)).unwrap();
        let connection = persistence.connection().unwrap();
        let error = connection
            .execute("DELETE FROM egress_ledger WHERE seq = 2", [])
            .unwrap_err();
        assert!(error.to_string().contains("checkpoint"), "{error}");
    }

    #[test]
    fn the_database_refuses_an_entry_that_does_not_extend_the_head() {
        let (persistence, repo) = ledger();
        let first = repo.append(&record("/a"), at(0)).unwrap();
        let connection = persistence.connection().unwrap();
        let forged = connection.execute(
            "INSERT INTO egress_ledger (seq, recorded_at, method, endpoint, body_bytes,
                 body_sha256, body_redacted, bearer, prev_hash, entry_hash)
             VALUES (2, 0, 'GET', 'https://h/x', 0, ?1, 0, 0, ?2, ?3)",
            params![sha256_hex(b""), GENESIS_HASH, sha256_hex(b"forged")],
        );
        assert!(forged.is_err(), "a second entry linked to the genesis");
        let skipped = connection.execute(
            "INSERT INTO egress_ledger (seq, recorded_at, method, endpoint, body_bytes,
                 body_sha256, body_redacted, bearer, prev_hash, entry_hash)
             VALUES (3, 0, 'GET', 'https://h/x', 0, ?1, 0, 0, ?2, ?3)",
            params![sha256_hex(b""), first.entry_hash, sha256_hex(b"forged")],
        );
        assert!(skipped.is_err(), "an entry skipped a seq");
    }

    #[test]
    fn pruning_by_age_keeps_the_chain_verifiable_from_its_checkpoint() {
        let (_persistence, repo) = ledger();
        for second in 0..5 {
            repo.append(&record("/a"), at(second)).unwrap();
        }
        let removed = repo.prune(at(2), 1_000, 500, at(10)).unwrap();
        assert_eq!(removed, 2, "entries recorded before the cutoff");
        let checkpoint = repo.checkpoint().unwrap().unwrap();
        assert_eq!(checkpoint.through_seq, 2);
        let entries = repo.entries().unwrap();
        assert_eq!(entries.first().unwrap().seq, 3);
        verify_chain(Some(&checkpoint), &entries).unwrap();

        let next = repo.append(&record("/b"), at(11)).unwrap();
        assert_eq!(next.seq, 6);
        verify_chain(Some(&checkpoint), &repo.entries().unwrap()).unwrap();
    }

    #[test]
    fn pruning_by_count_and_batch_takes_only_the_oldest() {
        let (_persistence, repo) = ledger();
        for second in 0..10 {
            repo.append(&record("/a"), at(second)).unwrap();
        }
        // Cap of 4 means 6 must go; a batch of 5 takes the oldest 5 this pass.
        assert_eq!(repo.prune(at(-1), 4, 5, at(20)).unwrap(), 5);
        assert_eq!(repo.entries().unwrap().first().unwrap().seq, 6);
        assert_eq!(repo.prune(at(-1), 4, 5, at(21)).unwrap(), 1);
        assert_eq!(repo.entries().unwrap().len(), 4);
        assert_eq!(repo.prune(at(-1), 4, 5, at(22)).unwrap(), 0);
        let checkpoint = repo.checkpoint().unwrap().unwrap();
        assert_eq!(checkpoint.through_seq, 6);
        verify_chain(Some(&checkpoint), &repo.entries().unwrap()).unwrap();
    }

    #[test]
    fn an_emptied_ledger_continues_from_its_checkpoint() {
        let (_persistence, repo) = ledger();
        let only = repo.append(&record("/a"), at(0)).unwrap();
        assert_eq!(repo.prune(at(100), 1_000, 500, at(100)).unwrap(), 1);
        assert!(repo.entries().unwrap().is_empty());
        let next = repo.append(&record("/b"), at(101)).unwrap();
        assert_eq!(next.seq, 2);
        assert_eq!(next.prev_hash, only.entry_hash);
    }

    #[test]
    fn the_newest_checkpoint_cannot_be_deleted() {
        let (persistence, repo) = ledger();
        repo.append(&record("/a"), at(0)).unwrap();
        repo.prune(at(100), 1_000, 500, at(100)).unwrap();
        let connection = persistence.connection().unwrap();
        let error = connection
            .execute("DELETE FROM egress_ledger_checkpoint", [])
            .unwrap_err();
        assert!(error.to_string().contains("anchor"), "{error}");
    }
}
