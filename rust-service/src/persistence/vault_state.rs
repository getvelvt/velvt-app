//! The Sync v1 tables (migration 0046): `vault_state`, `vault_rule_link`,
//! `sync_known_device` and `sync_notice`, and the two local settings the
//! document mirrors.
//!
//! `vault::sync` decides what the rows mean; this layer stores and reads them.
//! Nothing here is document plaintext: `ciphertext` is the sealed copy, and
//! the rule links hold salted keys only.

use std::fmt;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::{PersistenceError, SqlitePersistence, VelvtQuietHours};

/// One account's row in `vault_state`: the trust anchor, then the session.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultStateRecord {
    pub user_id: String,
    pub sync_device_id: String,
    pub host_binding_salt: Vec<u8>,
    pub host_binding: Vec<u8>,
    pub key_id: Option<String>,
    pub write_pk: Option<Vec<u8>>,
    pub highest_seen_version: u64,
    pub highest_seen_sha256: Option<Vec<u8>>,
    pub fetched_at_ms: Option<i64>,
    pub seen_reset_request_id: Option<String>,
    /// The key id of this Mac's own create that velvt-core has not yet shown
    /// settled (no answer, or refused). Its Keychain item is kept, and is
    /// this Mac's own create after a restart too, not a live item of unknown
    /// origin. Not a session column: Log Out and Turn Off keep it.
    pub pending_create_key_id: Option<String>,
    pub enabled: bool,
    pub server_version: Option<u64>,
    pub ciphertext: Option<Vec<u8>>,
    pub last_hlc: Option<String>,
    pub state: String,
    pub last_error: Option<String>,
}

impl VaultStateRecord {
    /// The session columns as Log Out, Turn Off Sync, removal and an account
    /// switch leave them. The anchor is untouched.
    pub fn clear_session(&mut self) {
        self.enabled = false;
        self.server_version = None;
        self.ciphertext = None;
        self.last_hlc = None;
        self.state = "off".to_owned();
        self.last_error = None;
    }
}

impl fmt::Debug for VaultStateRecord {
    /// The sealed copy and the host binding are printed by length only: the
    /// first is opaque and large, and the second identifies this Mac.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultStateRecord")
            .field("user_id", &self.user_id)
            .field("sync_device_id", &self.sync_device_id)
            .field("host_binding", &"[32 bytes]")
            .field("key_id", &self.key_id)
            .field("highest_seen_version", &self.highest_seen_version)
            .field("fetched_at_ms", &self.fetched_at_ms)
            .field("seen_reset_request_id", &self.seen_reset_request_id)
            .field("pending_create_key_id", &self.pending_create_key_id)
            .field("enabled", &self.enabled)
            .field("server_version", &self.server_version)
            .field(
                "ciphertext",
                &self.ciphertext.as_ref().map(|bytes| bytes.len()),
            )
            .field("last_hlc", &self.last_hlc)
            .field("state", &self.state)
            .field("last_error", &self.last_error)
            .finish()
    }
}

/// One row of `vault_rule_link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRuleLinkRecord {
    pub local_kind: String,
    pub local_key_hash: String,
    pub person_key: String,
    pub category: String,
    pub deleted: bool,
    pub hlc: String,
}

/// One row of `sync_known_device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDeviceRecord {
    pub server_device_id: String,
    pub first_seen_at_ms: i64,
    pub alerted_at_ms: Option<i64>,
}

/// One row of `sync_notice`: a security notice owed to the account, by ids
/// and times only (`vault::sync::notices::NoticeEvent` says what each column
/// means for each kind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncNoticeRecord {
    pub user_id: String,
    pub notice_id: String,
    pub kind: String,
    pub subject: Option<String>,
    pub at_ms: i64,
    pub until_ms: Option<i64>,
    pub flag: bool,
    pub queued_at_ms: i64,
    pub posted_at_ms: Option<i64>,
    pub acknowledged_at_ms: Option<i64>,
}

/// The two settings the document mirrors and never applies (§10.1): this
/// Mac's quiet hours, and its invitations switch with when it was set. `None`
/// when this Mac never set one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalSyncSettings {
    pub quiet_hours: Option<VelvtQuietHours>,
    pub invitations: Option<(bool, DateTime<Utc>)>,
}

pub trait VaultStateRepo: Send + Sync {
    fn vault_state(&self, user_id: &str) -> Result<Option<VaultStateRecord>, PersistenceError>;
    fn vault_states(&self) -> Result<Vec<VaultStateRecord>, PersistenceError>;
    /// Inserts or replaces the account's whole row.
    fn save_vault_state(&self, record: &VaultStateRecord) -> Result<(), PersistenceError>;
    /// Clears the session of every account but `keep`: their session columns,
    /// and `vault_rule_link` and `sync_known_device`, which belong to whichever
    /// account's session was last open. With `keep` `None`, every session.
    /// The tables are cleared only when some other account still had a
    /// session, so a sign-in back to the same account keeps them. Returns
    /// whether any session was cleared.
    fn clear_sessions(&self, keep: Option<&str>) -> Result<bool, PersistenceError>;
    /// Deletes the account's row, anchor included, its notices, and the
    /// session tables.
    fn delete_vault_state(&self, user_id: &str) -> Result<(), PersistenceError>;
    fn rule_links(&self) -> Result<Vec<VaultRuleLinkRecord>, PersistenceError>;
    fn upsert_rule_link(&self, link: &VaultRuleLinkRecord) -> Result<(), PersistenceError>;
    /// Deletes each tombstone in `pushed` that is still the one pushed (same
    /// kind, key and HLC): a link edited again since is kept.
    fn delete_pushed_tombstones(
        &self,
        pushed: &[VaultRuleLinkRecord],
    ) -> Result<u64, PersistenceError>;
    fn clear_rule_links(&self) -> Result<(), PersistenceError>;
    fn known_devices(&self) -> Result<Vec<KnownDeviceRecord>, PersistenceError>;
    /// Adds each device not already known; a known one is left as it is.
    fn insert_known_devices(&self, devices: &[KnownDeviceRecord]) -> Result<(), PersistenceError>;
    fn local_sync_settings(&self) -> Result<LocalSyncSettings, PersistenceError>;
    /// The account's notices, oldest first.
    fn sync_notices(&self, user_id: &str) -> Result<Vec<SyncNoticeRecord>, PersistenceError>;
    /// Inserts or replaces the notice's row.
    fn save_sync_notice(&self, notice: &SyncNoticeRecord) -> Result<(), PersistenceError>;
    /// Deletes every account's notices but `keep`'s; with `keep` `None`,
    /// every notice.
    fn delete_sync_notices(&self, keep: Option<&str>) -> Result<(), PersistenceError>;
    /// Deletes notices acknowledged before `before_ms`.
    fn prune_sync_notices(&self, before_ms: i64) -> Result<u64, PersistenceError>;
}

pub(super) struct SqliteVaultStateRepo(pub(super) SqlitePersistence);

const STATE_COLUMNS: &str = "user_id, sync_device_id, host_binding_salt, host_binding, key_id,
    write_pk, highest_seen_version, highest_seen_sha256, fetched_at, seen_reset_request_id,
    enabled, server_version, ciphertext, last_hlc, state, last_error, pending_create_key_id";

const CLEARED_SESSION: &str = "enabled = 0, server_version = NULL, ciphertext = NULL,
    last_hlc = NULL, state = 'off', last_error = NULL";

fn state_from_row(row: &Row<'_>) -> rusqlite::Result<VaultStateRecord> {
    let version = |index: usize| -> rusqlite::Result<u64> {
        let value: i64 = row.get(index)?;
        u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
    };
    let optional_version = |index: usize| -> rusqlite::Result<Option<u64>> {
        let value: Option<i64> = row.get(index)?;
        value
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
            })
            .transpose()
    };
    Ok(VaultStateRecord {
        user_id: row.get(0)?,
        sync_device_id: row.get(1)?,
        host_binding_salt: row.get(2)?,
        host_binding: row.get(3)?,
        key_id: row.get(4)?,
        write_pk: row.get(5)?,
        highest_seen_version: version(6)?,
        highest_seen_sha256: row.get(7)?,
        fetched_at_ms: row.get(8)?,
        seen_reset_request_id: row.get(9)?,
        enabled: row.get::<_, i64>(10)? != 0,
        server_version: optional_version(11)?,
        ciphertext: row.get(12)?,
        last_hlc: row.get(13)?,
        state: row.get(14)?,
        last_error: row.get(15)?,
        pending_create_key_id: row.get(16)?,
    })
}

/// velvt-core's versions are a Postgres BIGINT, so a real one always fits.
fn to_sql_version(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl VaultStateRepo for SqliteVaultStateRepo {
    fn vault_state(&self, user_id: &str) -> Result<Option<VaultStateRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                &format!("SELECT {STATE_COLUMNS} FROM vault_state WHERE user_id = ?1"),
                [user_id],
                state_from_row,
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn vault_states(&self) -> Result<Vec<VaultStateRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {STATE_COLUMNS} FROM vault_state ORDER BY user_id"
        ))?;
        let rows = statement.query_map([], state_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn save_vault_state(&self, record: &VaultStateRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            &format!(
                "INSERT OR REPLACE INTO vault_state({STATE_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                         ?17)"
            ),
            params![
                record.user_id,
                record.sync_device_id,
                record.host_binding_salt,
                record.host_binding,
                record.key_id,
                record.write_pk,
                to_sql_version(record.highest_seen_version),
                record.highest_seen_sha256,
                record.fetched_at_ms,
                record.seen_reset_request_id,
                i64::from(record.enabled),
                record.server_version.map(to_sql_version),
                record.ciphertext,
                record.last_hlc,
                record.state,
                record.last_error,
                record.pending_create_key_id,
            ],
        )?;
        Ok(())
    }

    fn clear_sessions(&self, keep: Option<&str>) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let keep = keep.unwrap_or("");
        let open: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM vault_state
             WHERE user_id != ?1
               AND (enabled = 1 OR ciphertext IS NOT NULL OR server_version IS NOT NULL
                    OR last_hlc IS NOT NULL OR state != 'off' OR last_error IS NOT NULL)",
            [keep],
            |row| row.get(0),
        )?;
        let cleared = open > 0;
        if cleared || keep.is_empty() {
            transaction.execute(
                &format!("UPDATE vault_state SET {CLEARED_SESSION} WHERE user_id != ?1"),
                [keep],
            )?;
            transaction.execute("DELETE FROM vault_rule_link", [])?;
            transaction.execute("DELETE FROM sync_known_device", [])?;
        }
        transaction.commit()?;
        Ok(cleared)
    }

    fn delete_vault_state(&self, user_id: &str) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM vault_state WHERE user_id = ?1", [user_id])?;
        transaction.execute("DELETE FROM sync_notice WHERE user_id = ?1", [user_id])?;
        transaction.execute("DELETE FROM vault_rule_link", [])?;
        transaction.execute("DELETE FROM sync_known_device", [])?;
        transaction.commit()?;
        Ok(())
    }

    fn rule_links(&self) -> Result<Vec<VaultRuleLinkRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT local_kind, local_key_hash, person_key, category, deleted, hlc
             FROM vault_rule_link ORDER BY local_kind, local_key_hash",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(VaultRuleLinkRecord {
                local_kind: row.get(0)?,
                local_key_hash: row.get(1)?,
                person_key: row.get(2)?,
                category: row.get(3)?,
                deleted: row.get::<_, i64>(4)? != 0,
                hlc: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn upsert_rule_link(&self, link: &VaultRuleLinkRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO vault_rule_link(local_kind, local_key_hash, person_key, category, deleted, hlc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(local_kind, local_key_hash) DO UPDATE SET
                person_key = excluded.person_key,
                category = excluded.category,
                deleted = excluded.deleted,
                hlc = excluded.hlc",
            params![
                link.local_kind,
                link.local_key_hash,
                link.person_key,
                link.category,
                i64::from(link.deleted),
                link.hlc,
            ],
        )?;
        Ok(())
    }

    fn delete_pushed_tombstones(
        &self,
        pushed: &[VaultRuleLinkRecord],
    ) -> Result<u64, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let mut removed = 0;
        {
            let mut statement = transaction.prepare(
                "DELETE FROM vault_rule_link
                 WHERE local_kind = ?1 AND local_key_hash = ?2 AND hlc = ?3 AND deleted = 1",
            )?;
            for link in pushed.iter().filter(|link| link.deleted) {
                removed +=
                    statement.execute(params![link.local_kind, link.local_key_hash, link.hlc])?
                        as u64;
            }
        }
        transaction.commit()?;
        Ok(removed)
    }

    fn clear_rule_links(&self) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute("DELETE FROM vault_rule_link", [])?;
        Ok(())
    }

    fn known_devices(&self) -> Result<Vec<KnownDeviceRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT server_device_id, first_seen_at, alerted_at
             FROM sync_known_device ORDER BY server_device_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(KnownDeviceRecord {
                server_device_id: row.get(0)?,
                first_seen_at_ms: row.get(1)?,
                alerted_at_ms: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn insert_known_devices(&self, devices: &[KnownDeviceRecord]) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO sync_known_device(server_device_id, first_seen_at, alerted_at)
                 VALUES (?1, ?2, ?3)",
            )?;
            for device in devices {
                statement.execute(params![
                    device.server_device_id,
                    device.first_seen_at_ms,
                    device.alerted_at_ms
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn local_sync_settings(&self) -> Result<LocalSyncSettings, PersistenceError> {
        let connection = self.0.connection()?;
        let quiet_hours = connection
            .query_row(
                "SELECT start_local_minutes, end_local_minutes, rule_version, configured_at
                 FROM velvt_quiet_hours WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(start, end, rule_version, configured_at)| {
                Ok::<_, PersistenceError>(VelvtQuietHours {
                    start_local_minutes: start,
                    end_local_minutes: end,
                    rule_version,
                    configured_at: DateTime::from_timestamp(configured_at, 0)
                        .ok_or(PersistenceError::InvalidTimestamp)?,
                })
            })
            .transpose()?;
        let invitations = connection
            .query_row(
                "SELECT invitations_enabled, updated_at FROM initiation_settings WHERE id = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
            .map(|(enabled, updated_at)| {
                DateTime::from_timestamp(updated_at, 0)
                    .map(|at| (enabled != 0, at))
                    .ok_or(PersistenceError::InvalidTimestamp)
            })
            .transpose()?;
        Ok(LocalSyncSettings {
            quiet_hours,
            invitations,
        })
    }

    fn sync_notices(&self, user_id: &str) -> Result<Vec<SyncNoticeRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT user_id, notice_id, kind, subject, at, until_at, flag, queued_at,
                    posted_at, acknowledged_at
             FROM sync_notice WHERE user_id = ?1 ORDER BY queued_at, notice_id",
        )?;
        let rows = statement.query_map([user_id], |row| {
            Ok(SyncNoticeRecord {
                user_id: row.get(0)?,
                notice_id: row.get(1)?,
                kind: row.get(2)?,
                subject: row.get(3)?,
                at_ms: row.get(4)?,
                until_ms: row.get(5)?,
                flag: row.get::<_, i64>(6)? != 0,
                queued_at_ms: row.get(7)?,
                posted_at_ms: row.get(8)?,
                acknowledged_at_ms: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn save_sync_notice(&self, notice: &SyncNoticeRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT OR REPLACE INTO sync_notice(user_id, notice_id, kind, subject, at, until_at,
                 flag, queued_at, posted_at, acknowledged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                notice.user_id,
                notice.notice_id,
                notice.kind,
                notice.subject,
                notice.at_ms,
                notice.until_ms,
                i64::from(notice.flag),
                notice.queued_at_ms,
                notice.posted_at_ms,
                notice.acknowledged_at_ms,
            ],
        )?;
        Ok(())
    }

    fn delete_sync_notices(&self, keep: Option<&str>) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "DELETE FROM sync_notice WHERE user_id != ?1",
            [keep.unwrap_or("")],
        )?;
        Ok(())
    }

    fn prune_sync_notices(&self, before_ms: i64) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let removed = connection.execute(
            "DELETE FROM sync_notice WHERE acknowledged_at IS NOT NULL AND acknowledged_at < ?1",
            [before_ms],
        )?;
        Ok(removed as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::{SyncNoticeRecord, VaultStateRecord};
    use crate::persistence::SqlitePersistence;

    fn notice(user: &str, id: &str, acknowledged_at_ms: Option<i64>) -> SyncNoticeRecord {
        SyncNoticeRecord {
            user_id: user.into(),
            notice_id: format!("new_sign_in:{id}"),
            kind: "new_sign_in".into(),
            subject: Some(id.into()),
            at_ms: 1_000,
            until_ms: None,
            flag: false,
            queued_at_ms: 1_000,
            posted_at_ms: None,
            acknowledged_at_ms,
        }
    }

    /// `sync_notice` keeps each account's notices apart, updates one in
    /// place, and forgets acknowledged ones after the cutoff only.
    #[test]
    fn notices_are_kept_per_account_and_pruned_once_acknowledged() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.vault_state_repo();
        let device = "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1";
        repo.save_sync_notice(&notice("a", device, None)).unwrap();
        repo.save_sync_notice(&notice("b", device, None)).unwrap();
        let mut posted = notice("a", device, None);
        posted.posted_at_ms = Some(2_000);
        repo.save_sync_notice(&posted).unwrap();
        assert_eq!(repo.sync_notices("a").unwrap(), vec![posted]);

        repo.delete_sync_notices(Some("a")).unwrap();
        assert!(repo.sync_notices("b").unwrap().is_empty());
        assert_eq!(repo.sync_notices("a").unwrap().len(), 1);

        let old = "45c48cce-2e2d-4fbd-b6a0-3f0bd1b6f5e2";
        repo.save_sync_notice(&notice("a", old, Some(5_000)))
            .unwrap();
        assert_eq!(repo.prune_sync_notices(5_000).unwrap(), 0);
        assert_eq!(repo.prune_sync_notices(5_001).unwrap(), 1);
        assert_eq!(
            repo.sync_notices("a").unwrap().len(),
            1,
            "unacknowledged stays"
        );

        repo.delete_sync_notices(None).unwrap();
        assert!(repo.sync_notices("a").unwrap().is_empty());
    }

    /// The table takes an id, never text: a subject the length of a sentence
    /// and a kind it does not know are refused.
    #[test]
    fn a_notice_row_refuses_what_is_not_an_id() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.vault_state_repo();
        let mut long = notice("a", "x", None);
        long.subject = Some("y".repeat(97));
        assert!(repo.save_sync_notice(&long).is_err());
        let mut unknown = notice("a", "x", None);
        unknown.kind = "marketing".into();
        assert!(repo.save_sync_notice(&unknown).is_err());
    }

    /// `pending_create_key_id` is not a session column: a cleared session
    /// (Log Out, Turn Off, another account) keeps it, so a kept item is
    /// still this Mac's own create when sync is turned on again. It takes a
    /// key id and nothing longer.
    #[test]
    fn a_pending_create_outlives_the_session() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.vault_state_repo();
        let record = VaultStateRecord {
            user_id: "a".into(),
            sync_device_id: "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1".into(),
            host_binding_salt: vec![1; 32],
            host_binding: vec![2; 32],
            key_id: None,
            write_pk: None,
            highest_seen_version: 0,
            highest_seen_sha256: None,
            fetched_at_ms: Some(1_000),
            seen_reset_request_id: None,
            pending_create_key_id: Some("0123456789abcdef".into()),
            enabled: true,
            server_version: None,
            ciphertext: None,
            last_hlc: None,
            state: "error".into(),
            last_error: Some("vault_create_refused".into()),
        };
        repo.save_vault_state(&record).unwrap();
        assert_eq!(repo.vault_state("a").unwrap().as_ref(), Some(&record));

        assert!(repo.clear_sessions(None).unwrap());
        let cleared = repo.vault_state("a").unwrap().unwrap();
        assert!(!cleared.enabled);
        assert_eq!(cleared.pending_create_key_id, record.pending_create_key_id);

        let mut key = record.clone();
        key.pending_create_key_id = Some("0123456789abcdef0123456789abcdef".into());
        assert!(repo.save_vault_state(&key).is_err());
    }

    /// Deleting an account's row deletes its notices too.
    #[test]
    fn deleting_the_row_deletes_the_account_s_notices() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.vault_state_repo();
        repo.save_sync_notice(&notice("a", "x", None)).unwrap();
        repo.delete_vault_state("a").unwrap();
        assert!(repo.sync_notices("a").unwrap().is_empty());
    }
}
