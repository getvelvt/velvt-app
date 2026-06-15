use super::{
    AbstractionMapRepo, AbstractionMapping, BatchEvent, HistoryCacheEntry, HistoryCacheRepo,
    InsightCacheEntry, InsightCacheRepo, NewUploadBatch, RawEventEntry, RawEventRepo, UploadBatch,
    UploadBatchRepo, UploadBatchStatus,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/embedded_migrations.rs"));

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("SQLite persistence unavailable")]
    Sqlite(#[from] rusqlite::Error),
    #[error("SQLite persistence lock unavailable")]
    LockUnavailable,
    #[error("SQLite persistence path unavailable")]
    PathUnavailable,
    #[error("SQLite persistence contains an invalid timestamp")]
    InvalidTimestamp,
    #[error("SQLite persistence row not found: {entity}")]
    NotFound { entity: &'static str },
}

#[derive(Clone)]
pub struct SqlitePersistence {
    connection: Arc<Mutex<Connection>>,
}

impl SqlitePersistence {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            let parent = path.parent().ok_or(PersistenceError::PathUnavailable)?;
            std::fs::create_dir_all(parent).map_err(|_| PersistenceError::PathUnavailable)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let persistence = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        persistence.run_migrations()?;
        Ok(persistence)
    }

    pub fn open_in_memory() -> Result<Self, PersistenceError> {
        Self::open(":memory:")
    }

    pub fn run_migrations(&self) -> Result<(), PersistenceError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migration (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                version INTEGER NOT NULL UNIQUE,
                name TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );",
        )?;
        for migration in EMBEDDED_MIGRATIONS {
            let applied = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migration WHERE version = ?1)",
                [migration.version],
                |row| row.get::<_, bool>(0),
            )?;
            if !applied {
                transaction.execute_batch(migration.sql)?;
                transaction.execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    params![migration.version, migration.name],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn abstraction_map_repo(&self) -> Arc<dyn AbstractionMapRepo> {
        Arc::new(SqliteAbstractionMapRepo(self.clone()))
    }

    pub fn abstraction_mapping_store(
        &self,
    ) -> Arc<dyn crate::abstraction::AbstractionMappingStore> {
        Arc::new(SqliteAbstractionMapRepo(self.clone()))
    }

    pub fn upload_batch_repo(&self) -> Arc<dyn UploadBatchRepo> {
        Arc::new(SqliteUploadBatchRepo(self.clone()))
    }

    pub fn history_cache_repo(&self) -> Arc<dyn HistoryCacheRepo> {
        Arc::new(SqliteHistoryCacheRepo(self.clone()))
    }

    pub fn insight_cache_repo(&self) -> Arc<dyn InsightCacheRepo> {
        Arc::new(SqliteInsightCacheRepo(self.clone()))
    }

    pub fn raw_event_repo(&self) -> Arc<dyn RawEventRepo> {
        Arc::new(SqliteRawEventRepo(self.clone()))
    }

    fn insert_batch_with_events(
        &self,
        batch: &NewUploadBatch,
        events: &[BatchEvent],
    ) -> Result<(), PersistenceError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        insert_batch(&transaction, batch)?;
        for event in events {
            add_event_to_batch(&transaction, &batch.batch_id, event)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn schema_snapshot(&self) -> Result<Vec<String>, PersistenceError> {
        self.schema_values(
            "SELECT name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )
    }

    pub fn schema_sql(&self) -> Result<Vec<String>, PersistenceError> {
        self.schema_values(
            "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
    }

    pub fn raw_event_query_plan(&self) -> Result<String, PersistenceError> {
        let connection = self.connection()?;
        let plan = connection.query_row(
            "EXPLAIN QUERY PLAN SELECT event_id, stable_id, label, category, taxonomy_version, occurred_at
             FROM raw_event_buffer WHERE occurred_at < ?1 ORDER BY occurred_at",
            [i64::MAX],
            |row| row.get::<_, String>(3),
        )?;
        Ok(plan)
    }

    fn schema_values(&self, query: &str) -> Result<Vec<String>, PersistenceError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(query)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, PersistenceError> {
        self.connection
            .lock()
            .map_err(|_| PersistenceError::LockUnavailable)
    }

    // ------------------------------------------------------------------
    // Test-setup helpers — public for integration tests, never call in
    // production code; name suffix "_for_test" documents intent.
    // ------------------------------------------------------------------

    /// Sets `created_at` for ALL current rows in `raw_event_buffer`.
    /// Used in retention integration tests to simulate aged rows.
    pub fn set_all_raw_event_created_at_for_test(
        &self,
        unix_ts: i64,
    ) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        conn.execute(
            "UPDATE raw_event_buffer SET created_at = ?1",
            params![unix_ts],
        )
        .map_err(Into::into)
    }

    /// Returns the number of rows in `raw_event_buffer`.
    pub fn count_raw_events_for_test(&self) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        conn.query_row("SELECT COUNT(*) FROM raw_event_buffer", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .map_err(Into::into)
    }
}

#[derive(Clone)]
struct SqliteAbstractionMapRepo(SqlitePersistence);

impl crate::abstraction::AbstractionMappingStore for SqliteAbstractionMapRepo {
    fn resolve_id(
        &self,
        stable_key: &str,
        fresh_id: &str,
        label: &str,
        category: &str,
        taxonomy_version: &str,
    ) -> Result<String, crate::abstraction::StoreError> {
        let mapping = AbstractionMapping {
            key_hash: stable_key.to_owned(),
            stable_id: fresh_id.to_owned(),
            label: label.to_owned(),
            category: category.to_owned(),
            taxonomy_version: taxonomy_version.to_owned(),
        };
        self.upsert(&mapping)?;
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT stable_id FROM abstraction_map WHERE key_hash = ?1",
                [stable_key],
                |row| row.get(0),
            )
            .map_err(PersistenceError::from)
            .map_err(Into::into)
    }
}

impl AbstractionMapRepo for SqliteAbstractionMapRepo {
    fn upsert(&self, mapping: &AbstractionMapping) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO abstraction_map(key_hash, stable_id, label, category, taxonomy_version)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key_hash) DO UPDATE SET
                label = excluded.label,
                category = excluded.category,
                taxonomy_version = excluded.taxonomy_version,
                updated_at = unixepoch()",
            params![
                mapping.key_hash,
                mapping.stable_id,
                mapping.label,
                mapping.category,
                mapping.taxonomy_version
            ],
        )?;
        Ok(())
    }

    fn get(&self, stable_id: &str) -> Result<AbstractionMapping, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT key_hash, stable_id, label, category, taxonomy_version
                 FROM abstraction_map WHERE stable_id = ?1",
                [stable_id],
                |row| {
                    Ok(AbstractionMapping {
                        key_hash: row.get(0)?,
                        stable_id: row.get(1)?,
                        label: row.get(2)?,
                        category: row.get(3)?,
                        taxonomy_version: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)?
            .ok_or(PersistenceError::NotFound {
                entity: "abstraction_map",
            })
    }

    fn exists(&self, key_hash: &str) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM abstraction_map WHERE key_hash = ?1)",
                [key_hash],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
}

#[derive(Clone)]
struct SqliteRawEventRepo(SqlitePersistence);

impl RawEventRepo for SqliteRawEventRepo {
    fn insert(&self, event: &RawEventEntry) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO raw_event_buffer(
                event_id, stable_id, label, category, taxonomy_version, occurred_at, duration_seconds
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.event_id,
                event.stable_id,
                event.label,
                event.category,
                event.taxonomy_version,
                event.occurred_at.timestamp(),
                event.duration_seconds
            ],
        )?;
        Ok(())
    }

    fn events_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<RawEventEntry>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_id, stable_id, label, category, taxonomy_version, occurred_at, duration_seconds
             FROM raw_event_buffer WHERE occurred_at < ?1 ORDER BY occurred_at",
        )?;
        let rows = statement.query_map([cutoff.timestamp()], raw_event_from_row)?;
        rows.map(|row| row.map_err(PersistenceError::from))
            .collect()
    }

    fn delete_before(&self, cutoff: DateTime<Utc>) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM raw_event_buffer WHERE occurred_at < ?1",
            [cutoff.timestamp()],
        )?;
        Ok(deleted as u64)
    }

    fn delete_expired_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM raw_event_buffer WHERE id IN (
                 SELECT id FROM raw_event_buffer WHERE created_at < ?1 LIMIT ?2
             )",
            params![cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }
}

#[derive(Clone)]
struct SqliteUploadBatchRepo(SqlitePersistence);

impl UploadBatchRepo for SqliteUploadBatchRepo {
    fn insert_batch(&self, batch: &NewUploadBatch) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        insert_batch(&connection, batch)
    }

    fn insert_batch_with_events(
        &self,
        batch: &NewUploadBatch,
        events: &[BatchEvent],
    ) -> Result<(), PersistenceError> {
        self.0.insert_batch_with_events(batch, events)
    }

    fn mark_sent(&self, batch_id: &str) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        let updated = connection.execute(
            "UPDATE upload_batch SET status = 'sent', sent_at = unixepoch() WHERE batch_id = ?1",
            [batch_id],
        )?;
        if updated == 0 {
            Err(PersistenceError::NotFound {
                entity: "upload_batch",
            })
        } else {
            Ok(())
        }
    }

    fn pending_batches(&self) -> Result<Vec<UploadBatch>, PersistenceError> {
        self.resumable_batches(DateTime::<Utc>::MAX_UTC)
    }

    fn resumable_batches(&self, now: DateTime<Utc>) -> Result<Vec<UploadBatch>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut batch_statement = connection.prepare(
            "SELECT batch_id, status, attempt_count, next_attempt_at
             FROM upload_batch
             WHERE status IN ('pending', 'failed') AND next_attempt_at <= ?1
             ORDER BY created_at, id",
        )?;
        let batch_ids = batch_statement
            .query_map([now.timestamp()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    upload_status_from_str(&row.get::<_, String>(1)?)?,
                    row.get::<_, u32>(2)?,
                    timestamp_from_row(row, 3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut batches = Vec::with_capacity(batch_ids.len());
        for (batch_id, status, attempt_count, next_attempt_at) in batch_ids {
            let mut event_statement = connection.prepare(
                "SELECT event_id, stable_id, label, category, taxonomy_version, occurred_at, duration_seconds
                 FROM batch_event WHERE batch_id = ?1 ORDER BY id",
            )?;
            let events = event_statement
                .query_map([&batch_id], batch_event_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            batches.push(UploadBatch {
                batch_id,
                status,
                attempt_count,
                next_attempt_at,
                events,
            });
        }
        Ok(batches)
    }

    fn mark_failed(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError> {
        update_batch_state(
            &self.0,
            "UPDATE upload_batch SET status = 'failed', attempt_count = attempt_count + 1,
             next_attempt_at = ?2, last_error_code = ?3 WHERE batch_id = ?1",
            batch_id,
            next_attempt_at.timestamp(),
            error_code,
        )
    }

    fn mark_pending_retry(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError> {
        update_batch_state(
            &self.0,
            "UPDATE upload_batch SET status = 'pending', attempt_count = attempt_count + 1,
             next_attempt_at = ?2, last_error_code = ?3 WHERE batch_id = ?1",
            batch_id,
            next_attempt_at.timestamp(),
            error_code,
        )
    }

    fn mark_rejected(&self, batch_id: &str, error_code: &str) -> Result<(), PersistenceError> {
        update_batch_state(
            &self.0,
            "UPDATE upload_batch SET status = 'rejected', last_error_code = ?3 WHERE batch_id = ?1",
            batch_id,
            0,
            error_code,
        )
    }

    fn discard_batch(&self, batch_id: &str) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        let deleted =
            connection.execute("DELETE FROM upload_batch WHERE batch_id = ?1", [batch_id])?;
        if deleted == 0 {
            Err(PersistenceError::NotFound {
                entity: "upload_batch",
            })
        } else {
            Ok(())
        }
    }

    fn batch_status(&self, batch_id: &str) -> Result<UploadBatchStatus, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT status FROM upload_batch WHERE batch_id = ?1",
                [batch_id],
                |row| upload_status_from_str(&row.get::<_, String>(0)?),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound {
                entity: "upload_batch",
            })
    }

    fn host_backoff_attempt(&self, host: &str) -> Result<u32, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT attempt_count FROM upload_host_backoff WHERE host = ?1",
                [host],
                |row| row.get(0),
            )
            .optional()
            .map(|attempt| attempt.unwrap_or(0))
            .map_err(Into::into)
    }

    fn host_backoff_until(&self, host: &str) -> Result<Option<DateTime<Utc>>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT next_attempt_at FROM upload_host_backoff WHERE host = ?1",
                [host],
                |row| timestamp_from_row(row, 0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn set_host_backoff(
        &self,
        host: &str,
        attempt_count: u32,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO upload_host_backoff(host, attempt_count, next_attempt_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(host) DO UPDATE SET
                attempt_count = excluded.attempt_count,
                next_attempt_at = excluded.next_attempt_at,
                updated_at = unixepoch()",
            params![host, attempt_count, next_attempt_at.timestamp()],
        )?;
        Ok(())
    }

    fn clear_host_backoff(&self, host: &str) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute("DELETE FROM upload_host_backoff WHERE host = ?1", [host])?;
        Ok(())
    }

    fn add_event_to_batch(
        &self,
        batch_id: &str,
        event: &BatchEvent,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        add_event_to_batch(&connection, batch_id, event)
    }

    fn delete_sent_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM upload_batch WHERE id IN (
                 SELECT id FROM upload_batch WHERE status = 'sent' AND sent_at < ?1 LIMIT ?2
             )",
            params![cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }

    fn delete_rejected_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM upload_batch WHERE id IN (
                 SELECT id FROM upload_batch WHERE status = 'rejected' AND created_at < ?1 LIMIT ?2
             )",
            params![cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }
}

#[derive(Clone)]
struct SqliteHistoryCacheRepo(SqlitePersistence);

impl HistoryCacheRepo for SqliteHistoryCacheRepo {
    fn upsert(&self, entry: &HistoryCacheEntry) -> Result<(), PersistenceError> {
        upsert_cache(
            &self.0,
            "INSERT INTO history_cache(date, payload, ttl) VALUES (?1, ?2, ?3)
             ON CONFLICT(date) DO UPDATE SET payload = excluded.payload, ttl = excluded.ttl",
            &entry.date,
            &entry.payload,
            entry.expires_at,
        )
    }

    fn get(&self, date: &str) -> Result<Option<HistoryCacheEntry>, PersistenceError> {
        get_history_cache(&self.0, date)
    }

    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError> {
        invalidate_cache(&self.0, "DELETE FROM history_cache WHERE date = ?1", date)
    }

    fn invalidate_all(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute("DELETE FROM history_cache", [])? as u64)
    }

    fn delete_expired_batch(
        &self,
        grace_cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM history_cache WHERE id IN (
                 SELECT id FROM history_cache WHERE ttl < ?1 LIMIT ?2
             )",
            params![grace_cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }
}

#[derive(Clone)]
struct SqliteInsightCacheRepo(SqlitePersistence);

impl InsightCacheRepo for SqliteInsightCacheRepo {
    fn upsert(&self, entry: &InsightCacheEntry) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO insight_cache(date, payload, ttl, not_found) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(date) DO UPDATE SET
                payload = excluded.payload,
                ttl = excluded.ttl,
                not_found = 0",
            params![entry.date, entry.payload, entry.expires_at.timestamp()],
        )?;
        Ok(())
    }

    fn upsert_negative(
        &self,
        date: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO insight_cache(date, payload, ttl, not_found) VALUES (?1, 'null', ?2, 1)
             ON CONFLICT(date) DO UPDATE SET
                payload = 'null',
                ttl = excluded.ttl,
                not_found = 1",
            params![date, expires_at.timestamp()],
        )?;
        Ok(())
    }

    fn get(&self, date: &str) -> Result<Option<InsightCacheEntry>, PersistenceError> {
        get_insight_cache(&self.0, date)
    }

    fn invalidate(&self, date: &str) -> Result<u64, PersistenceError> {
        invalidate_cache(&self.0, "DELETE FROM insight_cache WHERE date = ?1", date)
    }

    fn invalidate_all(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute("DELETE FROM insight_cache", [])? as u64)
    }

    fn delete_expired_batch(
        &self,
        grace_cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM insight_cache WHERE id IN (
                 SELECT id FROM insight_cache WHERE ttl < ?1 LIMIT ?2
             )",
            params![grace_cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }
}

fn insert_batch(connection: &Connection, batch: &NewUploadBatch) -> Result<(), PersistenceError> {
    connection.execute(
        "INSERT INTO upload_batch(batch_id) VALUES (?1)",
        [&batch.batch_id],
    )?;
    Ok(())
}

fn add_event_to_batch(
    connection: &Connection,
    batch_id: &str,
    event: &BatchEvent,
) -> Result<(), PersistenceError> {
    connection.execute(
        "INSERT INTO batch_event(
            batch_id, event_id, stable_id, label, category, taxonomy_version, occurred_at, duration_seconds
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            batch_id,
            event.event_id,
            event.stable_id,
            event.label,
            event.category,
            event.taxonomy_version,
            event.occurred_at.timestamp(),
            event.duration_seconds
        ],
    )?;
    Ok(())
}

fn raw_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEventEntry> {
    Ok(RawEventEntry {
        event_id: row.get(0)?,
        stable_id: row.get(1)?,
        label: row.get(2)?,
        category: row.get(3)?,
        taxonomy_version: row.get(4)?,
        occurred_at: timestamp_from_row(row, 5)?,
        duration_seconds: row.get(6)?,
    })
}

fn batch_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BatchEvent> {
    Ok(BatchEvent {
        event_id: row.get(0)?,
        stable_id: row.get(1)?,
        label: row.get(2)?,
        category: row.get(3)?,
        taxonomy_version: row.get(4)?,
        occurred_at: timestamp_from_row(row, 5)?,
        duration_seconds: row.get(6)?,
    })
}

fn timestamp_from_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let timestamp = row.get(index)?;
    DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(PersistenceError::InvalidTimestamp),
        )
    })
}

fn upload_status_from_str(status: &str) -> rusqlite::Result<UploadBatchStatus> {
    match status {
        "pending" => Ok(UploadBatchStatus::Pending),
        "sent" => Ok(UploadBatchStatus::Sent),
        "failed" => Ok(UploadBatchStatus::Failed),
        "rejected" => Ok(UploadBatchStatus::Rejected),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn update_batch_state(
    persistence: &SqlitePersistence,
    query: &str,
    batch_id: &str,
    next_attempt_at: i64,
    error_code: &str,
) -> Result<(), PersistenceError> {
    let connection = persistence.connection()?;
    let updated = connection.execute(query, params![batch_id, next_attempt_at, error_code])?;
    if updated == 0 {
        Err(PersistenceError::NotFound {
            entity: "upload_batch",
        })
    } else {
        Ok(())
    }
}

fn upsert_cache(
    persistence: &SqlitePersistence,
    query: &str,
    date: &str,
    payload: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), PersistenceError> {
    let connection = persistence.connection()?;
    connection.execute(query, params![date, payload, expires_at.timestamp()])?;
    Ok(())
}

fn get_history_cache(
    persistence: &SqlitePersistence,
    date: &str,
) -> Result<Option<HistoryCacheEntry>, PersistenceError> {
    let connection = persistence.connection()?;
    connection
        .query_row(
            "SELECT date, payload, ttl FROM history_cache WHERE date = ?1 AND ttl > unixepoch()",
            [date],
            |row| {
                Ok(HistoryCacheEntry {
                    date: row.get(0)?,
                    payload: row.get(1)?,
                    expires_at: timestamp_from_row(row, 2)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn get_insight_cache(
    persistence: &SqlitePersistence,
    date: &str,
) -> Result<Option<InsightCacheEntry>, PersistenceError> {
    let connection = persistence.connection()?;
    connection
        .query_row(
            "SELECT date, payload, ttl, not_found
             FROM insight_cache WHERE date = ?1 AND ttl > unixepoch()",
            [date],
            |row| {
                Ok(InsightCacheEntry {
                    date: row.get(0)?,
                    payload: row.get(1)?,
                    expires_at: timestamp_from_row(row, 2)?,
                    is_negative: row.get::<_, i32>(3)? != 0,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn invalidate_cache(
    persistence: &SqlitePersistence,
    query: &str,
    date: &str,
) -> Result<u64, PersistenceError> {
    let connection = persistence.connection()?;
    Ok(connection.execute(query, [date])? as u64)
}

#[cfg(test)]
mod tests {
    use super::SqlitePersistence;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    #[test]
    fn newly_added_migration_applies_after_initial_schema_deploy() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migration (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    version INTEGER NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                );",
            )
            .unwrap();
        connection
            .execute_batch(include_str!(
                "../../migrations/0001_initial_persistence.sql"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO schema_migration(version, name) VALUES (1, '0001_initial_persistence.sql')",
                [],
            )
            .unwrap();
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };

        database.run_migrations().unwrap();

        assert!(database
            .schema_snapshot()
            .unwrap()
            .iter()
            .any(|name| name == "persistence_migration_probe"));
    }

    #[test]
    fn migration_0004_not_found_column_is_functional() {
        use chrono::Utc;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migration (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    version INTEGER NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                );",
            )
            .unwrap();
        connection
            .execute_batch(include_str!(
                "../../migrations/0001_initial_persistence.sql"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO schema_migration(version, name) VALUES (1, '0001_initial_persistence.sql')",
                [],
            )
            .unwrap();
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.run_migrations().unwrap();

        // After migration 0004, upsert_negative must succeed and round-trip.
        let repo = database.insight_cache_repo();
        let expires_at = Utc::now() + chrono::Duration::hours(1);
        repo.upsert_negative("2026-01-01", expires_at).unwrap();
        let entry = repo
            .get("2026-01-01")
            .unwrap()
            .expect("negative entry not found");
        assert!(entry.is_negative, "is_negative flag not set");
        assert_eq!(entry.date, "2026-01-01");
    }
}
