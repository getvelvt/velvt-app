use super::{
    AbstractionMapRepo, AbstractionMapping, BatchEvent, HistoryCacheEntry, HistoryCacheRepo,
    InsightCacheEntry, InsightCacheRepo, LocalDisplayAggregate, LocalEventMetadata, NewUploadBatch,
    PersonalOverrideRecord, RawEventEntry, RawEventRepo, UploadBatch, UploadBatchRepo,
    UploadBatchStatus, UploadQueueDiagnostics, WorkBlockCompletion, WorkBlockIntervention,
    WorkBlockInterventionOutcome, WorkBlockObservation, WorkBlockRecord, WorkBlockRepo,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, WorkBlockIntensity, WorkBlockPhase,
    WorkBlockPurpose, WorkBlockResult,
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
    #[error("SQLite persistence contains invalid safe JSON")]
    InvalidJson(#[from] serde_json::Error),
    #[error("SQLite persistence contains an invalid local semantic embedding")]
    InvalidSemanticEmbedding,
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
            let parent_existed = parent.exists();
            std::fs::create_dir_all(parent).map_err(|_| PersistenceError::PathUnavailable)?;
            #[cfg(unix)]
            if !parent_existed || parent.file_name().is_some_and(|name| name == ".velvt") {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| PersistenceError::PathUnavailable)?;
            }
        }
        let connection = Connection::open(path)?;
        #[cfg(unix)]
        if path != Path::new(":memory:") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| PersistenceError::PathUnavailable)?;
        }
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

    pub fn semantic_learning_store(&self) -> Arc<dyn crate::abstraction::SemanticLearningStore> {
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

    pub fn work_block_repo(&self) -> Arc<dyn WorkBlockRepo> {
        Arc::new(SqliteWorkBlockRepo(self.clone()))
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

    /// Sets `sent_at` for ALL current rows in `upload_batch`.
    /// Used in retention integration tests to simulate aged sent batches.
    pub fn set_all_upload_batch_sent_at_for_test(
        &self,
        unix_ts: i64,
    ) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        conn.execute("UPDATE upload_batch SET sent_at = ?1", params![unix_ts])
            .map_err(Into::into)
    }

    /// Sets `created_at` for ALL current rows in `upload_batch`.
    /// Used in retention integration tests to simulate aged rejected batches.
    pub fn set_all_upload_batch_created_at_for_test(
        &self,
        unix_ts: i64,
    ) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        conn.execute("UPDATE upload_batch SET created_at = ?1", params![unix_ts])
            .map_err(Into::into)
    }

    /// Returns the number of rows in `upload_batch`.
    pub fn count_upload_batches_for_test(&self) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        conn.query_row("SELECT COUNT(*) FROM upload_batch", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .map_err(Into::into)
    }
}

#[derive(Clone)]
struct SqliteAbstractionMapRepo(SqlitePersistence);

impl crate::abstraction::AbstractionMappingStore for SqliteAbstractionMapRepo {
    fn personal_override(
        &self,
        stable_key: &str,
    ) -> Result<Option<crate::abstraction::PersonalOverride>, crate::abstraction::StoreError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT category, activity_name FROM personal_override WHERE key_hash = ?1",
                [stable_key],
                |row| {
                    Ok(crate::abstraction::PersonalOverride {
                        category: row.get(0)?,
                        local_activity_name: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
            .map_err(Into::into)
    }

    fn resolve_id(
        &self,
        request: crate::abstraction::MappingResolution<'_>,
    ) -> Result<String, crate::abstraction::StoreError> {
        let mapping = AbstractionMapping {
            key_hash: request.stable_key.to_owned(),
            stable_id: request.fresh_id.to_owned(),
            label: request.label.to_owned(),
            category: request.category.to_owned(),
            taxonomy_version: request.taxonomy_version.to_owned(),
            classification_tier: request.classification_tier.to_owned(),
            classification_status: request.classification_status.to_owned(),
            classification_confidence: request.classification_confidence.to_owned(),
            classification_source: request.classification_source.to_owned(),
            display_name: request.local_display_label.map(str::to_owned),
        };
        self.upsert(&mapping)?;
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT stable_id FROM abstraction_map WHERE key_hash = ?1",
                [request.stable_key],
                |row| row.get(0),
            )
            .map_err(PersistenceError::from)
            .map_err(Into::into)
    }

    fn increment_classification_count(
        &self,
        taxonomy_version: &str,
        classification_tier: &str,
    ) -> Result<(), crate::abstraction::StoreError> {
        let connection = self.0.connection()?;
        connection
            .execute(
                "INSERT INTO classification_telemetry(taxonomy_version, classification_tier, event_count)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT(taxonomy_version, classification_tier) DO UPDATE SET
                    event_count = event_count + 1,
                    updated_at = unixepoch()",
                params![taxonomy_version, classification_tier],
            )
            .map(|_| ())
            .map_err(PersistenceError::from)
            .map_err(Into::into)
    }
}

impl crate::abstraction::SemanticLearningStore for SqliteAbstractionMapRepo {
    fn record_embedding(
        &self,
        key_hash: &str,
        embedding: &[f32],
    ) -> Result<(), crate::abstraction::StoreError> {
        let bytes =
            encode_embedding(embedding).ok_or(crate::abstraction::StoreError::Unavailable)?;
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO semantic_embedding_cache(key_hash, embedding, dimensions)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key_hash) DO UPDATE SET embedding = excluded.embedding,
                dimensions = excluded.dimensions, updated_at = unixepoch()",
            params![key_hash, bytes, embedding.len() as i64],
        )?;
        transaction.execute(
            "DELETE FROM semantic_embedding_cache WHERE key_hash IN (
                SELECT key_hash FROM semantic_embedding_cache
                ORDER BY updated_at DESC, key_hash ASC LIMIT -1 OFFSET 512
             )",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn embedding(
        &self,
        key_hash: &str,
    ) -> Result<Option<Vec<f32>>, crate::abstraction::StoreError> {
        let connection = self.0.connection()?;
        let value = connection
            .query_row(
                "SELECT embedding, dimensions FROM semantic_embedding_cache WHERE key_hash = ?1",
                [key_hash],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, usize>(1)?)),
            )
            .optional()?;
        value
            .map(|(bytes, dimensions)| decode_embedding(&bytes, dimensions))
            .transpose()
            .map_err(|_| crate::abstraction::StoreError::Unavailable)
    }

    fn personal_prototypes(
        &self,
    ) -> Result<Vec<crate::abstraction::PersonalSemanticPrototype>, crate::abstraction::StoreError>
    {
        let connection = self.0.connection()?;
        let now = Utc::now().timestamp();
        let mut statement = connection.prepare(
            "SELECT category, embedding, dimensions, updated_at
             FROM personal_semantic_prototype
             WHERE updated_at >= ?1
             ORDER BY updated_at DESC, key_hash ASC LIMIT 64",
        )?;
        let rows = statement.query_map([now - 90 * 86_400], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, usize>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut prototypes = Vec::new();
        for row in rows {
            let (category, bytes, dimensions, updated_at) = row?;
            let age = (now - updated_at).max(0) as f32 / (90.0 * 86_400.0);
            prototypes.push(crate::abstraction::PersonalSemanticPrototype {
                category,
                embedding: decode_embedding(&bytes, dimensions)
                    .map_err(|_| crate::abstraction::StoreError::Unavailable)?,
                weight: 1.0 - age.min(1.0) * 0.10,
            });
        }
        Ok(prototypes)
    }

    fn record_classifier_use(
        &self,
        artifact_version: &str,
    ) -> Result<(), crate::abstraction::StoreError> {
        if artifact_version.is_empty() || artifact_version.len() > 128 {
            return Err(crate::abstraction::StoreError::Unavailable);
        }
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO classifier_artifact_telemetry(artifact_version, classification_count)
             VALUES (?1, 1) ON CONFLICT(artifact_version) DO UPDATE SET
                classification_count = classification_count + 1, updated_at = unixepoch()",
            [artifact_version],
        )?;
        Ok(())
    }
}

fn encode_embedding(embedding: &[f32]) -> Option<Vec<u8>> {
    if embedding.is_empty() || embedding.len() > 1024 || embedding.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(
        embedding
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    )
}

fn decode_embedding(bytes: &[u8], dimensions: usize) -> Result<Vec<f32>, PersistenceError> {
    if dimensions == 0 || dimensions > 1024 || bytes.len() != dimensions * 4 {
        return Err(PersistenceError::InvalidSemanticEmbedding);
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let value = f32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
            value
                .is_finite()
                .then_some(value)
                .ok_or(PersistenceError::InvalidSemanticEmbedding)
        })
        .collect()
}

impl AbstractionMapRepo for SqliteAbstractionMapRepo {
    fn upsert(&self, mapping: &AbstractionMapping) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO abstraction_map(key_hash, stable_id, label, category, taxonomy_version, classification_tier, display_name, classification_status, classification_confidence, classification_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(key_hash) DO UPDATE SET
                label = excluded.label,
                category = excluded.category,
                taxonomy_version = excluded.taxonomy_version,
                classification_tier = excluded.classification_tier,
                classification_status = excluded.classification_status,
                classification_confidence = excluded.classification_confidence,
                classification_source = excluded.classification_source,
                display_name = COALESCE(excluded.display_name, abstraction_map.display_name),
                updated_at = unixepoch()",
            params![
                mapping.key_hash,
                mapping.stable_id,
                mapping.label,
                mapping.category,
                mapping.taxonomy_version,
                mapping.classification_tier,
                mapping.display_name,
                mapping.classification_status,
                mapping.classification_confidence,
                mapping.classification_source,
            ],
        )?;
        Ok(())
    }

    fn get(&self, stable_id: &str) -> Result<AbstractionMapping, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT key_hash, stable_id, label, category, taxonomy_version, classification_tier, display_name, classification_status, classification_confidence, classification_source
                 FROM abstraction_map WHERE stable_id = ?1",
                [stable_id],
                |row| {
                    Ok(AbstractionMapping {
                        key_hash: row.get(0)?,
                        stable_id: row.get(1)?,
                        label: row.get(2)?,
                        category: row.get(3)?,
                        taxonomy_version: row.get(4)?,
                        classification_tier: row.get(5)?,
                        display_name: row.get(6)?,
                        classification_status: row.get(7)?,
                        classification_confidence: row.get(8)?,
                        classification_source: row.get(9)?,
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

    fn save_personal_override(
        &self,
        stable_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "INSERT INTO personal_override(key_hash, category, activity_name)
             SELECT key_hash, ?2, ?3 FROM abstraction_map WHERE stable_id = ?1
             ON CONFLICT(key_hash) DO UPDATE SET
                category = excluded.category,
                activity_name = COALESCE(excluded.activity_name, personal_override.activity_name),
                updated_at = unixepoch()",
            params![stable_id, category, local_activity_name],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound {
                entity: "abstraction_map",
            });
        }
        transaction.execute(
            "INSERT INTO personal_semantic_prototype(key_hash, category, embedding, dimensions)
             SELECT map.key_hash, ?2, cache.embedding, cache.dimensions
             FROM abstraction_map map JOIN semantic_embedding_cache cache ON cache.key_hash = map.key_hash
             WHERE map.stable_id = ?1
             ON CONFLICT(key_hash) DO UPDATE SET category = excluded.category,
                embedding = excluded.embedding, dimensions = excluded.dimensions,
                correction_count = correction_count + 1, updated_at = unixepoch()",
            params![stable_id, category],
        )?;
        transaction.execute(
            "DELETE FROM personal_semantic_prototype WHERE key_hash IN (
                SELECT key_hash FROM personal_semantic_prototype WHERE category = ?1
                ORDER BY correction_count DESC, updated_at DESC, key_hash ASC LIMIT -1 OFFSET 12
             )",
            [category],
        )?;
        transaction.execute(
            "DELETE FROM personal_semantic_prototype WHERE key_hash IN (
                SELECT key_hash FROM personal_semantic_prototype
                ORDER BY correction_count DESC, updated_at DESC, key_hash ASC LIMIT -1 OFFSET 64
             )",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn personal_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<PersonalOverrideRecord>, PersistenceError> {
        self.search_personal_overrides(None, 0, limit)
            .map(|(records, _)| records)
    }

    fn search_personal_overrides(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<PersonalOverrideRecord>, u64), PersistenceError> {
        let limit = limit.min(20);
        if limit == 0 {
            return Ok((Vec::new(), 0));
        }
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        let connection = self.0.connection()?;
        let total = connection.query_row(
            "SELECT COUNT(*)
             FROM personal_override
             JOIN abstraction_map ON abstraction_map.key_hash = personal_override.key_hash
             WHERE ?1 IS NULL
                OR instr(lower(COALESCE(personal_override.activity_name, abstraction_map.display_name, '')), lower(?1)) > 0
                OR instr(lower(abstraction_map.label), lower(?1)) > 0
                OR instr(lower(personal_override.category), lower(?1)) > 0",
            [query],
            |row| row.get::<_, u64>(0),
        )?;
        let mut statement = connection.prepare(
            "SELECT abstraction_map.stable_id, abstraction_map.label,
                    COALESCE(personal_override.activity_name, abstraction_map.display_name),
                    personal_override.category, personal_override.updated_at
             FROM personal_override
             JOIN abstraction_map ON abstraction_map.key_hash = personal_override.key_hash
             WHERE ?1 IS NULL
                OR instr(lower(COALESCE(personal_override.activity_name, abstraction_map.display_name, '')), lower(?1)) > 0
                OR instr(lower(abstraction_map.label), lower(?1)) > 0
                OR instr(lower(personal_override.category), lower(?1)) > 0
             ORDER BY personal_override.updated_at DESC, abstraction_map.stable_id ASC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement
            .query_map(params![query, limit as i64, offset as i64], |row| {
                Ok(PersonalOverrideRecord {
                    stable_id: row.get(0)?,
                    label: row.get(1)?,
                    local_activity_name: row.get(2)?,
                    category: row.get(3)?,
                    updated_at: timestamp_from_row(row, 4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::from)?;
        Ok((rows, total))
    }

    fn remove_personal_override(&self, stable_id: &str) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "DELETE FROM personal_override WHERE key_hash = (
                SELECT key_hash FROM abstraction_map WHERE stable_id = ?1
             )",
            [stable_id],
        )?;
        transaction.execute(
            "DELETE FROM personal_semantic_prototype WHERE key_hash = (
                SELECT key_hash FROM abstraction_map WHERE stable_id = ?1
             )",
            [stable_id],
        )?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    fn reset_personal_overrides(&self) -> Result<u64, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute("DELETE FROM personal_override", [])? as u64;
        transaction.execute("DELETE FROM personal_semantic_prototype", [])?;
        transaction.commit()?;
        Ok(changed)
    }

    fn personal_override_count(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row("SELECT COUNT(*) FROM personal_override", [], |row| {
                row.get(0)
            })
            .map_err(Into::into)
    }

    fn personal_semantic_prototype_count(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT COUNT(*) FROM personal_semantic_prototype",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn classifier_artifact_count(&self, artifact_version: &str) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT classification_count FROM classifier_artifact_telemetry WHERE artifact_version = ?1",
                [artifact_version],
                |row| row.get(0),
            )
            .optional()
            .map(|count| count.unwrap_or(0))
            .map_err(Into::into)
    }

    fn display_name_for_label(&self, label: &str) -> Result<Option<String>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT display_name FROM abstraction_map
                 WHERE label = ?1 AND display_name IS NOT NULL
                 ORDER BY updated_at DESC, stable_id ASC LIMIT 1",
                [label],
                |row| row.get(0),
            )
            .optional()
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
                event_id, stable_id, label, local_display_label, local_name_suggestion, category, taxonomy_version, classification_tier, classification_status, classification_confidence, classification_source, occurred_at, duration_seconds, upload_eligible
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                event.event_id,
                event.stable_id,
                event.label,
                event.local_display_label,
                event.local_name_suggestion,
                event.category,
                event.taxonomy_version,
                event.classification_tier,
                event.classification_status,
                event.classification_confidence,
                event.classification_source,
                event.occurred_at.timestamp(),
                event.duration_seconds,
                event.upload_eligible
            ],
        )?;
        Ok(())
    }

    fn unbatched_events(&self, limit: usize) -> Result<Vec<RawEventEntry>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_id, stable_id, label, local_display_label, local_name_suggestion, category, taxonomy_version, classification_tier, classification_status, classification_confidence, classification_source, occurred_at, duration_seconds, upload_eligible
             FROM raw_event_buffer
             WHERE upload_eligible = 1
               AND NOT EXISTS (SELECT 1 FROM batch_event WHERE batch_event.event_id = raw_event_buffer.event_id)
             ORDER BY occurred_at DESC LIMIT ?1",
        )?;
        let events = statement
            .query_map([limit as i64], raw_event_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        events
    }

    fn events_before(&self, cutoff: DateTime<Utc>) -> Result<Vec<RawEventEntry>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_id, stable_id, label, local_display_label, local_name_suggestion, category, taxonomy_version, classification_tier, classification_status, classification_confidence, classification_source, occurred_at, duration_seconds, upload_eligible
             FROM raw_event_buffer WHERE occurred_at < ?1 ORDER BY occurred_at",
        )?;
        let rows = statement.query_map([cutoff.timestamp()], raw_event_from_row)?;
        rows.map(|row| row.map_err(PersistenceError::from))
            .collect()
    }

    fn events_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RawEventEntry>, PersistenceError> {
        if end <= start || limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT event_id, stable_id, label, local_display_label, local_name_suggestion, category, taxonomy_version, classification_tier, classification_status, classification_confidence, classification_source, occurred_at, duration_seconds, upload_eligible
             FROM raw_event_buffer
             WHERE occurred_at >= ?1 AND occurred_at <= ?2
             ORDER BY occurred_at ASC LIMIT ?3",
        )?;
        let rows = statement
            .query_map(
                params![start.timestamp(), end.timestamp(), limit as i64],
                raw_event_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        rows
    }

    fn local_event_metadata(
        &self,
        event_ids: &[String],
    ) -> Result<HashMap<String, LocalEventMetadata>, PersistenceError> {
        if event_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let connection = self.0.connection()?;
        let placeholders = vec!["?"; event_ids.len()].join(",");
        let query = format!(
            "SELECT event_id, local_display_label, classification_status, classification_confidence, classification_source FROM raw_event_buffer WHERE event_id IN ({placeholders})"
        );
        let mut statement = connection.prepare(&query)?;
        let rows = statement.query_map(rusqlite::params_from_iter(event_ids), |row| {
            Ok((
                row.get::<_, String>(0)?,
                LocalEventMetadata {
                    local_display_label: row.get(1)?,
                    classification_status: row.get(2)?,
                    classification_confidence: row.get(3)?,
                    classification_source: row.get(4)?,
                },
            ))
        })?;
        rows.map(|row| row.map_err(PersistenceError::from))
            .collect()
    }

    fn local_display_aggregates(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<LocalDisplayAggregate>, PersistenceError> {
        let limit = limit.min(5);
        if limit == 0 || end <= start {
            return Ok(Vec::new());
        }
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT COALESCE(local_display_label, 'Other'), SUM(duration_seconds)
             FROM raw_event_buffer
             WHERE occurred_at >= ?1 AND occurred_at < ?2
             GROUP BY local_display_label
             ORDER BY SUM(duration_seconds) DESC, COALESCE(local_display_label, 'Other') ASC",
        )?;
        let rows = statement
            .query_map(params![start.timestamp(), end.timestamp()], |row| {
                Ok(LocalDisplayAggregate {
                    label: row.get(0)?,
                    duration_seconds: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut selected = Vec::new();
        let mut other_seconds = 0_u64;
        for row in rows {
            if row.label == "Other" {
                other_seconds = other_seconds.saturating_add(row.duration_seconds);
            } else if selected.len() < limit {
                selected.push(row);
            } else {
                other_seconds = other_seconds.saturating_add(row.duration_seconds);
            }
        }
        if other_seconds > 0 {
            selected.push(LocalDisplayAggregate {
                label: "Other".to_owned(),
                duration_seconds: other_seconds,
            });
        }
        Ok(selected)
    }

    fn update_classification(
        &self,
        event_id: &str,
        label: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE raw_event_buffer
             SET label = ?2, category = ?3, classification_tier = 'exact_match',
                 classification_status = 'classified', classification_confidence = 'high',
                 classification_source = 'user_rule',
                 local_display_label = COALESCE(?4, local_display_label)
             WHERE event_id = ?1",
            params![event_id, label, category, local_activity_name],
        )?;
        Ok(())
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
            "UPDATE upload_batch
             SET status = 'sent', sent_at = unixepoch(), last_error_code = NULL
             WHERE batch_id = ?1",
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
                "SELECT event_id, stable_id, label, category, taxonomy_version, classification_tier, occurred_at, duration_seconds
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

    fn queue_diagnostics(&self) -> Result<UploadQueueDiagnostics, PersistenceError> {
        let connection = self.0.connection()?;
        let (pending_batch_count, failed_batch_count, rejected_batch_count, next_attempt_at) =
            connection.query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'rejected' THEN 1 ELSE 0 END), 0),
                    MIN(CASE
                        WHEN status IN ('pending', 'failed') AND next_attempt_at > unixepoch()
                        THEN next_attempt_at
                    END)
                 FROM upload_batch",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )?;
        let last_error_code = connection
            .query_row(
                "SELECT last_error_code
                 FROM upload_batch
                 WHERE status IN ('pending', 'failed', 'rejected')
                   AND last_error_code IS NOT NULL
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let last_successful_sync_at = connection
            .query_row(
                "SELECT MAX(sent_at) FROM upload_batch WHERE status = 'sent'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .map(|timestamp| timestamp_to_datetime(timestamp, 0))
            .transpose()?;
        Ok(UploadQueueDiagnostics {
            pending_batch_count,
            failed_batch_count,
            rejected_batch_count,
            next_attempt_at: next_attempt_at
                .map(|timestamp| timestamp_to_datetime(timestamp, 3))
                .transpose()?,
            last_error_code,
            last_successful_sync_at,
        })
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

    fn update_event_classification(
        &self,
        event_id: &str,
        label: &str,
        category: &str,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE batch_event
             SET label = ?2, category = ?3, classification_tier = 'exact_match'
             WHERE event_id = ?1",
            params![event_id, label, category],
        )?;
        Ok(())
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

#[derive(Clone)]
struct SqliteWorkBlockRepo(SqlitePersistence);

impl WorkBlockRepo for SqliteWorkBlockRepo {
    fn create(&self, block: &WorkBlockRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO work_block(
                block_id, state_version, phase, intention, purpose, intensity,
                planned_duration_seconds, started_at, paused_at, total_paused_seconds,
                ended_at, recovered_after_restart, recovery_of, intention_expires_at, updated_at
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                block.block_id,
                block.phase.as_str(),
                block.intention,
                block.purpose.map(WorkBlockPurpose::as_str),
                block.intensity.as_str(),
                block.planned_duration_seconds,
                block.started_at.timestamp(),
                block.paused_at.map(|value| value.timestamp()),
                block.total_paused_seconds,
                block.ended_at.map(|value| value.timestamp()),
                i64::from(block.recovered_after_restart),
                block.recovery_of,
                block.intention_expires_at.timestamp(),
                block.updated_at.timestamp(),
            ],
        )?;
        Ok(())
    }

    fn latest(&self) -> Result<Option<WorkBlockRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT block_id, phase, intention, purpose, intensity, planned_duration_seconds,
                        started_at, paused_at, total_paused_seconds, ended_at,
                        recovered_after_restart, recovery_of, intention_expires_at, updated_at
                 FROM work_block ORDER BY rowid DESC LIMIT 1",
                [],
                work_block_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn get(&self, block_id: &str) -> Result<WorkBlockRecord, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT block_id, phase, intention, purpose, intensity, planned_duration_seconds,
                        started_at, paused_at, total_paused_seconds, ended_at,
                        recovered_after_restart, recovery_of, intention_expires_at, updated_at
                 FROM work_block WHERE block_id = ?1",
                [block_id],
                work_block_from_row,
            )
            .optional()?
            .ok_or(PersistenceError::NotFound {
                entity: "work_block",
            })
    }

    fn set_paused(&self, block_id: &str, at: DateTime<Utc>) -> Result<(), PersistenceError> {
        update_work_block(
            &self.0,
            "UPDATE work_block SET phase = 'paused', paused_at = ?2, updated_at = ?2
             WHERE block_id = ?1 AND phase = 'active'",
            params![block_id, at.timestamp()],
        )
    }

    fn set_active(
        &self,
        block_id: &str,
        at: DateTime<Utc>,
        total_paused_seconds: u32,
    ) -> Result<(), PersistenceError> {
        update_work_block(
            &self.0,
            "UPDATE work_block SET phase = 'active', paused_at = NULL,
                    total_paused_seconds = ?3, updated_at = ?2
             WHERE block_id = ?1 AND phase = 'paused'",
            params![block_id, at.timestamp(), total_paused_seconds],
        )
    }

    fn mark_recovered(&self, block_id: &str, at: DateTime<Utc>) -> Result<(), PersistenceError> {
        update_work_block(
            &self.0,
            "UPDATE work_block SET recovered_after_restart = 1, updated_at = ?2
             WHERE block_id = ?1 AND phase IN ('active', 'paused')",
            params![block_id, at.timestamp()],
        )
    }

    fn close_open_observation(
        &self,
        block_id: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE work_block_observation
             SET ended_at = MAX(occurred_at, ?2)
             WHERE block_id = ?1 AND ended_at IS NULL",
            params![block_id, at.timestamp()],
        )?;
        Ok(())
    }

    fn append_observation(
        &self,
        block_id: &str,
        observation: &WorkBlockObservation,
    ) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO work_block_observation(
                block_id, occurred_at, ended_at, category,
                classification_status, classification_confidence
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                block_id,
                observation.occurred_at.timestamp(),
                observation.ended_at.map(|value| value.timestamp()),
                observation.category,
                observation.classification_status.as_str(),
                observation.classification_confidence.as_str(),
            ],
        )?;
        transaction.execute(
            "UPDATE work_block SET updated_at = MAX(updated_at, ?2) WHERE block_id = ?1",
            params![block_id, observation.occurred_at.timestamp()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn observations(&self, block_id: &str) -> Result<Vec<WorkBlockObservation>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT occurred_at, ended_at, category, classification_status,
                    classification_confidence
             FROM work_block_observation WHERE block_id = ?1
             ORDER BY occurred_at, id",
        )?;
        let observations = statement
            .query_map([block_id], work_block_observation_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::from)?;
        Ok(observations)
    }

    fn latest_observation(
        &self,
        block_id: &str,
    ) -> Result<Option<WorkBlockObservation>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT occurred_at, ended_at, category, classification_status,
                        classification_confidence
                 FROM work_block_observation WHERE block_id = ?1
                 ORDER BY occurred_at DESC, id DESC LIMIT 1",
                [block_id],
                work_block_observation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn finalize(
        &self,
        block_id: &str,
        completion: &WorkBlockCompletion,
    ) -> Result<WorkBlockResult, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        if let Some(payload) = transaction
            .query_row(
                "SELECT payload FROM work_block_result WHERE block_id = ?1",
                [block_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            transaction.commit()?;
            return serde_json::from_str(&payload).map_err(Into::into);
        }
        let updated = transaction.execute(
            "UPDATE work_block SET phase = ?2, ended_at = ?3, paused_at = NULL,
                    intention_expires_at = MIN(intention_expires_at, ?4), updated_at = ?3
             WHERE block_id = ?1 AND phase IN ('active', 'paused')",
            params![
                block_id,
                completion.phase.as_str(),
                completion.ended_at.timestamp(),
                (completion.ended_at + chrono::Duration::hours(24)).timestamp(),
            ],
        )?;
        if updated == 0 {
            return Err(PersistenceError::NotFound {
                entity: "active_work_block",
            });
        }
        let payload = serde_json::to_string(&completion.result)?;
        transaction.execute(
            "INSERT INTO work_block_result(block_id, payload) VALUES (?1, ?2)",
            params![block_id, payload],
        )?;
        transaction.commit()?;
        Ok(completion.result.clone())
    }

    fn result(&self, block_id: &str) -> Result<Option<WorkBlockResult>, PersistenceError> {
        let connection = self.0.connection()?;
        let payload = connection
            .query_row(
                "SELECT payload FROM work_block_result WHERE block_id = ?1",
                [block_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(PersistenceError::from))
            .transpose()
    }

    fn record_intervention(
        &self,
        block_id: &str,
        intervention: &WorkBlockIntervention,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // A second offer for the same block is a no-op rather than an error:
        // the cap is a property of the schema, not of the caller.
        connection.execute(
            "INSERT INTO work_block_intervention(
                block_id, offered_at, action_id, anchor_category,
                switch_count, window_seconds, outcome, outcome_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(block_id) DO NOTHING",
            params![
                block_id,
                intervention.offered_at.timestamp(),
                intervention.action_id,
                intervention.anchor_category,
                intervention.switch_count,
                intervention.window_seconds,
                intervention.outcome.as_str(),
                intervention.outcome_at.map(|at| at.timestamp()),
            ],
        )?;
        Ok(())
    }

    fn intervention(
        &self,
        block_id: &str,
    ) -> Result<Option<WorkBlockIntervention>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT offered_at, action_id, anchor_category, switch_count,
                        window_seconds, outcome, outcome_at
                 FROM work_block_intervention WHERE block_id = ?1",
                [block_id],
                |row| {
                    Ok(WorkBlockIntervention {
                        offered_at: timestamp_from_row(row, 0)?,
                        action_id: row.get(1)?,
                        anchor_category: row.get(2)?,
                        switch_count: row.get(3)?,
                        window_seconds: row.get(4)?,
                        outcome: WorkBlockInterventionOutcome::from_db_value(
                            &row.get::<_, String>(5)?,
                        )
                        .ok_or_else(invalid_enum)?,
                        outcome_at: row
                            .get::<_, Option<i64>>(6)?
                            .map(|value| timestamp_to_datetime(value, 6))
                            .transpose()?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn resolve_intervention(
        &self,
        block_id: &str,
        outcome: WorkBlockInterventionOutcome,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // Guarding on `outcome = 'offered'` keeps a recorded return from being
        // overwritten when the block later ends.
        let changed = connection.execute(
            "UPDATE work_block_intervention
             SET outcome = ?2, outcome_at = ?3
             WHERE block_id = ?1 AND outcome = 'offered'",
            params![block_id, outcome.as_str(), at.timestamp()],
        )?;
        Ok(changed > 0)
    }

    fn expire_intentions(&self, now: DateTime<Utc>) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute(
            "UPDATE work_block SET intention = NULL
             WHERE intention IS NOT NULL AND intention_expires_at <= ?1",
            [now.timestamp()],
        )? as u64)
    }

    fn clear_all(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute("DELETE FROM work_block", [])? as u64)
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
            batch_id, event_id, stable_id, label, category, taxonomy_version, classification_tier, occurred_at, duration_seconds
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            batch_id,
            event.event_id,
            event.stable_id,
            event.label,
            event.category,
            event.taxonomy_version,
            event.classification_tier,
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
        local_display_label: row.get(3)?,
        local_name_suggestion: row.get(4)?,
        category: row.get(5)?,
        taxonomy_version: row.get(6)?,
        classification_tier: row.get(7)?,
        classification_status: row.get(8)?,
        classification_confidence: row.get(9)?,
        classification_source: row.get(10)?,
        occurred_at: timestamp_from_row(row, 11)?,
        duration_seconds: row.get(12)?,
        upload_eligible: row.get(13)?,
    })
}

fn work_block_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkBlockRecord> {
    let phase = parse_work_block_phase(&row.get::<_, String>(1)?)?;
    let purpose = row
        .get::<_, Option<String>>(3)?
        .map(|value| parse_work_block_purpose(&value))
        .transpose()?;
    let intensity = parse_work_block_intensity(&row.get::<_, String>(4)?)?;
    Ok(WorkBlockRecord {
        block_id: row.get(0)?,
        phase,
        intention: row.get(2)?,
        purpose,
        intensity,
        planned_duration_seconds: row.get(5)?,
        started_at: timestamp_from_row(row, 6)?,
        paused_at: row
            .get::<_, Option<i64>>(7)?
            .map(|value| timestamp_to_datetime(value, 7))
            .transpose()?,
        total_paused_seconds: row.get(8)?,
        ended_at: row
            .get::<_, Option<i64>>(9)?
            .map(|value| timestamp_to_datetime(value, 9))
            .transpose()?,
        recovered_after_restart: row.get::<_, i64>(10)? != 0,
        recovery_of: row.get(11)?,
        intention_expires_at: timestamp_from_row(row, 12)?,
        updated_at: timestamp_from_row(row, 13)?,
    })
}

fn work_block_observation_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkBlockObservation> {
    Ok(WorkBlockObservation {
        occurred_at: timestamp_from_row(row, 0)?,
        ended_at: row
            .get::<_, Option<i64>>(1)?
            .map(|value| timestamp_to_datetime(value, 1))
            .transpose()?,
        category: row.get(2)?,
        classification_status: parse_classification_status_value(&row.get::<_, String>(3)?)?,
        classification_confidence: parse_classification_confidence_value(
            &row.get::<_, String>(4)?,
        )?,
    })
}

fn invalid_enum() -> rusqlite::Error {
    rusqlite::Error::InvalidQuery
}

fn parse_work_block_phase(value: &str) -> rusqlite::Result<WorkBlockPhase> {
    match value {
        "active" => Ok(WorkBlockPhase::Active),
        "paused" => Ok(WorkBlockPhase::Paused),
        "completed" => Ok(WorkBlockPhase::Completed),
        "abandoned" => Ok(WorkBlockPhase::Abandoned),
        "expired" => Ok(WorkBlockPhase::Expired),
        _ => Err(invalid_enum()),
    }
}

fn parse_work_block_purpose(value: &str) -> rusqlite::Result<WorkBlockPurpose> {
    match value {
        "deep_work" => Ok(WorkBlockPurpose::DeepWork),
        "study" => Ok(WorkBlockPurpose::Study),
        "creative_practice" => Ok(WorkBlockPurpose::CreativePractice),
        "healthy_tech_use" => Ok(WorkBlockPurpose::HealthyTechUse),
        "work_life_boundary" => Ok(WorkBlockPurpose::WorkLifeBoundary),
        _ => Err(invalid_enum()),
    }
}

fn parse_work_block_intensity(value: &str) -> rusqlite::Result<WorkBlockIntensity> {
    match value {
        "light" => Ok(WorkBlockIntensity::Light),
        "medium" => Ok(WorkBlockIntensity::Medium),
        "intense" => Ok(WorkBlockIntensity::Intense),
        _ => Err(invalid_enum()),
    }
}

fn parse_classification_status_value(value: &str) -> rusqlite::Result<ClassificationStatus> {
    match value {
        "classified" => Ok(ClassificationStatus::Classified),
        "ambiguous" => Ok(ClassificationStatus::Ambiguous),
        "unclassified" => Ok(ClassificationStatus::Unclassified),
        _ => Err(invalid_enum()),
    }
}

fn parse_classification_confidence_value(
    value: &str,
) -> rusqlite::Result<ClassificationConfidence> {
    match value {
        "high" => Ok(ClassificationConfidence::High),
        "medium" => Ok(ClassificationConfidence::Medium),
        "low" => Ok(ClassificationConfidence::Low),
        "none" => Ok(ClassificationConfidence::None),
        _ => Err(invalid_enum()),
    }
}

fn update_work_block<P: rusqlite::Params>(
    persistence: &SqlitePersistence,
    query: &str,
    params: P,
) -> Result<(), PersistenceError> {
    let connection = persistence.connection()?;
    if connection.execute(query, params)? == 0 {
        Err(PersistenceError::NotFound {
            entity: "work_block_transition",
        })
    } else {
        Ok(())
    }
}

fn batch_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BatchEvent> {
    Ok(BatchEvent {
        event_id: row.get(0)?,
        stable_id: row.get(1)?,
        label: row.get(2)?,
        category: row.get(3)?,
        taxonomy_version: row.get(4)?,
        classification_tier: row.get(5)?,
        occurred_at: timestamp_from_row(row, 6)?,
        duration_seconds: row.get(7)?,
    })
}

fn timestamp_from_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<DateTime<Utc>> {
    let timestamp = row.get(index)?;
    timestamp_to_datetime(timestamp, index)
}

fn timestamp_to_datetime(timestamp: i64, index: usize) -> rusqlite::Result<DateTime<Utc>> {
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

    #[test]
    fn migration_0011_preserves_existing_aliases_and_adds_local_suggestion_storage() {
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
        let migrations = [
            (
                1,
                include_str!("../../migrations/0001_initial_persistence.sql"),
            ),
            (
                2,
                include_str!("../../migrations/0002_harden_indexes_and_probe.sql"),
            ),
            (
                3,
                include_str!("../../migrations/0003_upload_retry_state.sql"),
            ),
            (
                4,
                include_str!("../../migrations/0004_insight_cache_negative.sql"),
            ),
            (
                5,
                include_str!("../../migrations/0005_local_queue_display_label.sql"),
            ),
            (
                6,
                include_str!("../../migrations/0006_classification_provenance.sql"),
            ),
            (
                7,
                include_str!("../../migrations/0007_personal_overrides.sql"),
            ),
            (
                8,
                include_str!("../../migrations/0008_classification_contract.sql"),
            ),
            (9, include_str!("../../migrations/0009_work_blocks.sql")),
            (
                10,
                include_str!("../../migrations/0010_personal_override_activity_name.sql"),
            ),
        ];
        for (version, sql) in migrations {
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    (version, format!("migration-{version}")),
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO abstraction_map(
                    key_hash, stable_id, label, category, taxonomy_version
                 ) VALUES (?1, 'abs_existing', 'reference:inferred', 'REFERENCE', 'mvp-1')",
                ["a".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_override(key_hash, category, activity_name)
                 VALUES (?1, 'REFERENCE', 'Existing local alias')",
                ["a".repeat(64)],
            )
            .unwrap();
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };

        database.run_migrations().unwrap();

        let connection = database.connection().unwrap();
        let alias: String = connection
            .query_row(
                "SELECT activity_name FROM personal_override WHERE key_hash = ?1",
                ["a".repeat(64)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias, "Existing local alias");
        assert!(connection
            .prepare("SELECT local_name_suggestion FROM raw_event_buffer")
            .is_ok());
    }
}
