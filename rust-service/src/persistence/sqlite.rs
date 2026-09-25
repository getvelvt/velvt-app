use super::{
    AbstractionMapRepo, AbstractionMapping, AntecedentFinding, AntecedentFindingRepo,
    AntecedentFindingState, AntecedentRetractionReason, AppScopeOverride, BatchEvent, BehaviorRepo,
    BlockAntecedent, CompletedBlockDwellSpan, DayType, DeclaredAppMetadata, DemotionStateRecord,
    FocusRepo, FocusTransition, GateVerdict, HistoryCacheEntry, HistoryCacheRepo,
    InitiationInvitationOutcome, InitiationInvitationRecord, InitiationRepo, InsightCacheEntry,
    InsightCacheRepo, InterventionDecision, InterventionDemotionState, LocalDisplayAggregate,
    LocalEventMetadata, NewUploadBatch, OutOfBlockRun, PersonalOverrideRecord,
    QuietHoursOfferResponse, QuietHoursOfferState, RawEventEntry, RawEventRepo, ReceiptsRepo,
    ReportedDwell, UnclassifiedAppEntry, UploadBatch, UploadBatchRepo, UploadBatchStatus,
    UploadQueueDiagnostics, VelvtQuietHours, WeeklyDigestRecord, WorkBlockCategoryCorrection,
    WorkBlockCompletion, WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockObservation,
    WorkBlockOrigin, WorkBlockRecord, WorkBlockRepo, WrongInterventionCounts,
    MAX_REPORTED_DWELL_SECONDS,
};
// Named through the defining module because `persistence::mod` re-exports types
// rather than constants; the retry ceiling is policy that belongs beside the
// status vocabulary it extends.
use super::models::UPLOAD_BATCH_ATTEMPT_CEILING;
// Same reason: the triage bounds are policy, declared once beside the trait that
// documents them.
use super::traits::{TRIAGE_MAX_ENTRIES, TRIAGE_MAX_LOOKBACK_DAYS, TRIAGE_MIN_SECONDS};
use crate::abstraction::{EmbeddingSalt, StableKeySalt};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, CorrectionScope, InterventionSalience,
    WorkBlockIntensity, WorkBlockPhase, WorkBlockPurpose, WorkBlockResult,
};

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/embedded_migrations.rs"));

/// The migration whose data half is Rust rather than SQL: 0037 mints the
/// stable-key salt in SQL, and [`rekey_stored_digests`] re-keys every stored
/// digest under it, because SQL cannot compute an HMAC.
const STABLE_KEY_SALT_MIGRATION: i64 = 37;

/// Every pending migration, in one transaction, in version order.
fn apply_embedded_migrations(connection: &mut Connection) -> Result<(), PersistenceError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migration (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL UNIQUE,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        );",
    )?;
    // Idempotence is keyed on the version number, so the recorded name is the
    // only evidence that the file applied under that number is the one this
    // build carries. A reused or renumbered migration (0010 and 0011 were each
    // allocated twice before build.rs refused duplicates) would otherwise be
    // skipped silently and leave this database on a schema no other install
    // has. Refusing to open is the loud outcome: the whole transaction rolls
    // back, nothing is applied, and startup halts naming both files.
    //
    // This compares names only. An edited migration keeps its name and passes;
    // detecting that needs a content checksum recorded per applied migration,
    // which needs a new column and so a numbered migration of its own.
    for migration in EMBEDDED_MIGRATIONS {
        let recorded = transaction
            .query_row(
                "SELECT name FROM schema_migration WHERE version = ?1",
                [migration.version],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match recorded {
            None => {
                transaction.execute_batch(migration.sql)?;
                // Only while 0037 itself is being applied, in its transaction,
                // and therefore exactly once per database: a second pass would
                // HMAC keys that are already keyed and orphan every one of them.
                if migration.version == STABLE_KEY_SALT_MIGRATION {
                    rekey_stored_digests(&transaction)?;
                }
                transaction.execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    params![migration.version, migration.name],
                )?;
            }
            Some(recorded) if recorded == migration.name => {}
            Some(recorded) => {
                return Err(PersistenceError::MigrationNameMismatch {
                    version: migration.version,
                    recorded,
                    embedded: migration.name,
                });
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

/// Every column that holds a digest `abstraction/key.rs` computed, as
/// `(table, column, optional)`. `optional` says whether the row can live
/// without the value: a key that is the row's identity cannot.
const KEYED_COLUMNS: &[(&str, &str, bool)] = &[
    ("abstraction_map", "key_hash", false),
    ("personal_override", "key_hash", false),
    ("personal_override", "app_key_hash", true),
    ("personal_app_override", "app_key_hash", false),
    ("personal_app_override", "bundle_key_hash", true),
    ("raw_event_buffer", "app_stable_id", true),
    ("raw_event_buffer", "app_bundle_stable_id", true),
    ("semantic_embedding_cache", "key_hash", false),
    ("personal_semantic_prototype", "key_hash", false),
];

/// Migration 0037's data half: every digest in [`KEYED_COLUMNS`] re-keyed
/// under the salt the migration's SQL just minted.
///
/// Through `StableKeySalt::rekey_stored_digest` and nothing else, which is the
/// function the engine keys fresh events with -- so a row re-keyed here and a
/// key computed afterwards for the same window, application or bundle are one
/// function's output, and every correction taught under 1.0.11 still matches.
/// One function over every column is also what keeps the pairings joining:
/// `personal_override.app_key_hash` and the rung it names, and
/// `raw_event_buffer.app_stable_id` and the rule it was taught under.
///
/// A value that is not a digest as `key.rs` writes one was never a key Velvt
/// produced and could never have matched a lookup. It is not keyed: the row goes
/// where the key is its identity, the value goes where it is optional. Every
/// CHECK in the schema makes that set empty in practice; handling it here keeps
/// an upgrade from failing -- and the service from starting -- over a row that
/// never worked.
fn rekey_stored_digests(transaction: &rusqlite::Transaction<'_>) -> Result<(), PersistenceError> {
    let salt =
        transaction.query_row("SELECT salt FROM stable_key_salt WHERE id = 1", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
    let salt = <[u8; StableKeySalt::LENGTH]>::try_from(salt.as_slice())
        .map(StableKeySalt::from_bytes)
        .map_err(|_| PersistenceError::InvalidStableKeySalt)?;
    for (table, column, optional) in KEYED_COLUMNS {
        let stored = transaction
            .prepare(&format!(
                "SELECT rowid, {column} FROM {table} WHERE {column} IS NOT NULL"
            ))?
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, rusqlite::types::Value>(1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut rekey = transaction.prepare(&format!(
            "UPDATE {table} SET {column} = ?2 WHERE rowid = ?1"
        ))?;
        let mut discard = transaction.prepare(&if *optional {
            format!("UPDATE {table} SET {column} = NULL WHERE rowid = ?1")
        } else {
            format!("DELETE FROM {table} WHERE rowid = ?1")
        })?;
        for (rowid, value) in stored {
            let keyed = match value {
                rusqlite::types::Value::Text(digest) => salt.rekey_stored_digest(&digest),
                _ => None,
            };
            match keyed {
                Some(keyed) => rekey.execute(params![rowid, keyed])?,
                None => discard.execute([rowid])?,
            };
        }
    }
    Ok(())
}

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
    #[error("SQLite persistence contains an invalid embedding salt")]
    InvalidEmbeddingSalt,
    #[error("SQLite persistence contains an invalid stable-key salt")]
    InvalidStableKeySalt,
    /// An app-scoped rule was asked for over an application identity whose own
    /// events say generalizing to the whole application is not meaningful -- a
    /// browser, where one tab says nothing about the next. Refused here rather
    /// than trusted from the caller: the evidence lives in the event rows, so
    /// this is the only layer that can see it (`app_identity_for_event`).
    #[error("SQLite persistence refused an app-scoped rule for an ineligible application")]
    AppScopeIneligible,
    /// The database applied migration `version` from a different file than the
    /// one this build embeds for that number. Both names are migration file
    /// names from this repository, never user data.
    #[error(
        "SQLite migration {version} was applied as {recorded:?}, but this build embeds {embedded:?} for that version"
    )]
    MigrationNameMismatch {
        version: i64,
        recorded: String,
        embedded: &'static str,
    },
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
        // Without this SQLite leaves a deleted row's bytes in free space until
        // the page is reused, so Clear Local Work Blocks, Reset Corrections and
        // every retention sweep removed text from queries but not from the
        // file (PRIVACY_AUDIT.md Audit 8). With it, deleted content is zeroed.
        connection.pragma_update(None, "secure_delete", true)?;
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

    /// Opens an existing database for reading only: no migrations, no
    /// permission changes, and SQLite itself refuses every write. For
    /// `velvt-service --dry-run-egress`, which may run beside a live service.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn run_migrations(&self) -> Result<(), PersistenceError> {
        let mut connection = self.connection()?;
        apply_embedded_migrations(&mut connection)
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

    /// The hash-chained record of every request sent (`egress_ledger`, 0038).
    pub fn egress_ledger_repo(&self) -> Arc<dyn super::EgressLedgerRepo> {
        Arc::new(super::egress_ledger::SqliteEgressLedgerRepo(self.clone()))
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

    pub fn focus_repo(&self) -> Arc<dyn FocusRepo> {
        Arc::new(SqliteFocusRepo(self.clone()))
    }

    pub fn receipts_repo(&self) -> Arc<dyn ReceiptsRepo> {
        Arc::new(SqliteReceiptsRepo(self.clone()))
    }

    pub fn initiation_repo(&self) -> Arc<dyn InitiationRepo> {
        Arc::new(SqliteInitiationRepo(self.clone()))
    }

    pub fn behavior_repo(&self) -> Arc<dyn BehaviorRepo> {
        Arc::new(SqliteBehaviorRepo(self.clone()))
    }

    /// Discovered antecedent patterns (`0029`). Has no caller in the shipped
    /// path: the miner writes findings, tests read them, and nothing surfaces.
    pub fn antecedent_finding_repo(&self) -> Arc<dyn AntecedentFindingRepo> {
        Arc::new(SqliteAntecedentFindingRepo(self.clone()))
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

    pub(super) fn connection(&self) -> Result<MutexGuard<'_, Connection>, PersistenceError> {
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

    /// Sets `updated_at` for the named `abstraction_map` rows, by stable id.
    /// Used in retention integration tests to simulate a window not observed
    /// inside the horizon; `upsert` refreshes the column on every observation,
    /// so a test cannot age a mapping by writing to it.
    pub fn set_abstraction_map_updated_at_for_test(
        &self,
        stable_ids: &[String],
        unix_ts: i64,
    ) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        let mut updated = 0;
        for stable_id in stable_ids {
            updated += conn.execute(
                "UPDATE abstraction_map SET updated_at = ?2 WHERE stable_id = ?1",
                params![stable_id, unix_ts],
            )?;
        }
        Ok(updated)
    }

    /// Sets `updated_at` for the named `semantic_embedding_cache` rows.
    /// Used in retention integration tests to simulate an entry that has not
    /// been re-observed inside the window; `record_embedding` refreshes the
    /// column on every write, so a test cannot age a row by writing to it.
    pub fn set_semantic_embedding_updated_at_for_test(
        &self,
        key_hashes: &[String],
        unix_ts: i64,
    ) -> Result<usize, PersistenceError> {
        let conn = self.connection()?;
        let mut updated = 0;
        for key_hash in key_hashes {
            updated += conn.execute(
                "UPDATE semantic_embedding_cache SET updated_at = ?2 WHERE key_hash = ?1",
                params![key_hash, unix_ts],
            )?;
        }
        Ok(updated)
    }
}

/// Every persisted personal rule, window-scoped and app-scoped, in one shape.
///
/// The history listed window rules only until protocol 30, which made an
/// app-scoped rule invisible and unremovable: the user could neither see what
/// they had taught nor undo it, and removing the window rule left the engine
/// falling through into the surviving app rule and answering exactly as before.
/// Both rungs are read here so one list can show, edit and remove either, with
/// `scope` saying which it is -- the `stable_id` column means an abstraction
/// stable id for a window rule and the application's own key hash for an app
/// rule, so a caller must read the scope before acting on the id.
///
/// The app rung carries no label of its own: `personal_app_override` holds a
/// category, a typed name and two hashes, and the raw application name is gone
/// by then. Both text columns therefore come from the most recent event of that
/// application, which is also the only place a local name for it exists.
const RULE_SOURCE: &str = "
    SELECT 'window' AS scope,
           abstraction_map.stable_id AS stable_id,
           abstraction_map.label AS label,
           COALESCE(personal_override.activity_name, abstraction_map.display_name) AS local_label,
           personal_override.category AS category,
           personal_override.updated_at AS updated_at
      FROM personal_override
      JOIN abstraction_map ON abstraction_map.key_hash = personal_override.key_hash
    UNION ALL
    SELECT 'app' AS scope,
           rule.app_key_hash AS stable_id,
           COALESCE(
               (SELECT recent.label FROM raw_event_buffer recent
                 WHERE recent.app_stable_id = rule.app_key_hash
                 ORDER BY recent.occurred_at DESC LIMIT 1),
               -- No event of this application survives the retention window.
               -- A plain word rather than a label derived from the category:
               -- mirroring `override_label_for_category` into SQL would put a
               -- second copy of that mapping a migration away from drifting.
               'application'
           ) AS label,
           COALESCE(
               rule.activity_name,
               (SELECT COALESCE(named.local_display_label, named.local_name_suggestion)
                  FROM raw_event_buffer named
                 WHERE named.app_stable_id = rule.app_key_hash
                   AND COALESCE(named.local_display_label, named.local_name_suggestion)
                       IS NOT NULL
                 ORDER BY named.occurred_at DESC LIMIT 1)
           ) AS local_label,
           rule.category AS category,
           rule.updated_at AS updated_at
      FROM personal_app_override rule
     -- Only the app rules that are a rule in their own right. A correction has
     -- written BOTH rungs since 0017, so listing this table unfiltered showed
     -- every past correction twice -- one action, two rows -- for every existing
     -- user, the moment the app rung reached this list. `app_only` (0035) records
     -- which it is at write time, because nothing can recover the pairing
     -- afterwards: the rungs are keyed in different hash domains and the only
     -- join between them, `raw_event_buffer`, holds 14 days. A paired row is
     -- represented here by its window rule, which is also the row whose removal
     -- takes both rungs with it, so what the user sees is what they can undo. A
     -- rule taught through triage has no window rung at all and appears.
     WHERE rule.app_only = 1
";

/// The one search predicate both the count and the page apply. `?1` is the
/// trimmed query or NULL, and NULL means "everything" rather than "nothing".
const RULE_FILTER: &str = "
    ?1 IS NULL
       OR instr(lower(COALESCE(local_label, '')), lower(?1)) > 0
       OR instr(lower(label), lower(?1)) > 0
       OR instr(lower(category), lower(?1)) > 0
";

fn app_scope_override_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppScopeOverride> {
    Ok(AppScopeOverride {
        app_key_hash: row.get(0)?,
        bundle_key_hash: row.get(1)?,
        category: row.get(2)?,
        activity_name: row.get(3)?,
        correction_count: row.get(4)?,
        updated_at: timestamp_from_row(row, 5)?,
    })
}

/// Writes one app-scope rule, converging on the bundle identity.
///
/// The one place this table is written, because the two keys on a row are
/// constrained in two different ways and a single upsert cannot honour both:
/// `app_key_hash` is the primary key and the conflict target, while
/// `bundle_key_hash` carries a partial UNIQUE index (0034) that no upsert can
/// target. An application rename is exactly a collision on the second one -- the
/// same bundle arriving under a new name hash -- so the upsert alone aborted with
/// SQLITE_CONSTRAINT on the case bundle keying exists to solve.
///
/// So the stale alias is folded first. The bundle identifier is the identity: a
/// row claiming this bundle under a different name is this same rule under the
/// name the application used to report, not a second rule. It is re-keyed to the
/// current name, which keeps `correction_count` -- how often the user had to
/// repeat themselves survives a rename -- or deleted when a rule under the new
/// name already exists and the re-key would collide with that in turn. After
/// either, at most one row claims the bundle and it is the row the upsert is
/// about to touch.
///
/// `app_only` is raised, never lowered (`MAX`, migration 0035): a rule taught
/// about the application itself keeps its own place in the correction history
/// even when a later window correction touches the same row, because those are
/// two things the user said.
fn upsert_app_scope_rule(
    connection: &Connection,
    app_key_hash: &str,
    bundle_key_hash: Option<&str>,
    category: &str,
    local_activity_name: Option<&str>,
    app_only: bool,
) -> rusqlite::Result<()> {
    if let Some(bundle_key_hash) = bundle_key_hash {
        connection.execute(
            "UPDATE personal_app_override
                SET app_key_hash = ?2, updated_at = unixepoch()
              WHERE bundle_key_hash = ?1
                AND app_key_hash <> ?2
                AND NOT EXISTS (
                    SELECT 1 FROM personal_app_override existing
                     WHERE existing.app_key_hash = ?2
                )",
            params![bundle_key_hash, app_key_hash],
        )?;
        // Only reachable when the re-key above could not run because a rule under
        // the new name already existed -- a rule taught while the client reported
        // no bundle identifier, say. Two rows may not claim one bundle, and the
        // row being written is the one that holds the current name, so the older
        // alias goes; the category it held is about to be restated anyway.
        connection.execute(
            "DELETE FROM personal_app_override
              WHERE bundle_key_hash = ?1 AND app_key_hash <> ?2",
            params![bundle_key_hash, app_key_hash],
        )?;
    }
    connection.execute(
        "INSERT INTO personal_app_override(
             app_key_hash, bundle_key_hash, category, activity_name, app_only
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(app_key_hash) DO UPDATE SET
            bundle_key_hash =
                COALESCE(excluded.bundle_key_hash, personal_app_override.bundle_key_hash),
            category = excluded.category,
            activity_name =
                COALESCE(excluded.activity_name, personal_app_override.activity_name),
            app_only = MAX(personal_app_override.app_only, excluded.app_only),
            correction_count = personal_app_override.correction_count + 1,
            updated_at = unixepoch()",
        params![
            app_key_hash,
            bundle_key_hash,
            category,
            local_activity_name,
            i64::from(app_only)
        ],
    )?;
    Ok(())
}

/// The application identity an event was recorded under, when that event can be
/// generalized to the whole application at all.
///
/// Sourced from the event rather than from a caller, because the raw application
/// name is discarded after abstraction: the event row is the only place the app
/// identity survives. `app_scope_eligible = 0` -- a browser window that carried a
/// site context -- yields `None`, which is what keeps one tab from recolouring an
/// entire browsing session.
///
/// The application behind the event is checked as well as the event, because a
/// browser also produces windows it read no site from: one of those is eligible
/// on its own row while the application it belongs to is not, and generalizing
/// from it writes exactly the browser-wide rule this guarantee exists to prevent
/// (`app_scope_identity_is_ineligible`).
fn app_identity_for_event(
    connection: &Connection,
    event_id: &str,
) -> rusqlite::Result<Option<(String, Option<String>)>> {
    let identity: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT app_stable_id, app_bundle_stable_id FROM raw_event_buffer
             WHERE event_id = ?1 AND app_stable_id IS NOT NULL AND app_scope_eligible = 1",
            [event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    eligible_app_identity(connection, identity)
}

/// Drops an application identity that its own events say may not carry an
/// app-scoped rule. The one place the two reads above are combined, so every
/// writer of `personal_app_override` answers the question the same way.
fn eligible_app_identity(
    connection: &Connection,
    identity: Option<(String, Option<String>)>,
) -> rusqlite::Result<Option<(String, Option<String>)>> {
    match identity {
        Some((app_key_hash, bundle_key_hash)) => {
            if app_scope_identity_is_ineligible(
                connection,
                &app_key_hash,
                bundle_key_hash.as_deref(),
            )? {
                return Ok(None);
            }
            Ok(Some((app_key_hash, bundle_key_hash)))
        }
        None => Ok(None),
    }
}

/// Whether this application's own events say it must not carry an app-scoped
/// rule at all.
///
/// The same judgement `app_identity_for_event` makes for one event, asked of an
/// application identity instead -- because the surfaces that name an application
/// hold only its hashes, never the name, so nothing above this layer can see the
/// evidence. `app_scope_eligible = 0` is recorded on a window whose identity came
/// from a site rather than from the application (`abstraction/engine.rs`), and an
/// application that has ever produced one is a browser: an app-wide rule there
/// would classify every future tab -- a video, a forum, mail -- as whatever the
/// user said about one of them, at High confidence and from
/// `ClassificationSource::UserRule`, which the engine reads BEFORE the plugins
/// and which therefore also stops `BrowserContextPlugin` ever running for that
/// browser again.
///
/// Evidence of ineligibility, not proof of eligibility: an identity with no
/// events left -- an editor whose events aged out, a rule being edited -- is not
/// refused, because "nothing is known" is the state every rule taught before
/// these columns existed is in, and refusing those would break editing a saved
/// rule.
fn app_scope_identity_is_ineligible(
    connection: &Connection,
    app_key_hash: &str,
    bundle_key_hash: Option<&str>,
) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM raw_event_buffer
              WHERE app_scope_eligible = 0
                AND (app_stable_id = ?1
                     OR (?2 IS NOT NULL AND app_bundle_stable_id = ?2))
         )",
        params![app_key_hash, bundle_key_hash],
        |row| row.get(0),
    )
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

    /// Answers for either app identity, so one call serves both rungs.
    ///
    /// The two keys live in different hash domains
    /// (`velvt:abstraction-app-key:v1` and
    /// `velvt:abstraction-app-bundle-key:v1`), so a name key can never equal a
    /// bundle key and the `OR` cannot match the wrong rule. That is what lets
    /// the bundle rung be an extra call with a different key rather than a
    /// second trait method: the engine consults the window key, then the bundle
    /// key, then the name key, and this one read resolves whichever it is given.
    fn personal_app_override(
        &self,
        app_stable_key: &str,
    ) -> Result<Option<crate::abstraction::PersonalOverride>, crate::abstraction::StoreError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT category, activity_name FROM personal_app_override
                 WHERE app_key_hash = ?1 OR bundle_key_hash = ?1",
                [app_stable_key],
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

    fn stable_key_salt(&self) -> Result<StableKeySalt, crate::abstraction::StoreError> {
        AbstractionMapRepo::stable_key_salt(self).map_err(Into::into)
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

    fn save_personal_app_override(
        &self,
        event_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        // Both identities are written where the event carried a bundle one, so
        // the rule keeps applying after a rename or under a localized name. The
        // keys are resolved first and written through `upsert_app_scope_rule`
        // rather than selected straight into the INSERT, because the rename case
        // needs the bundle key as a value: a row already claiming that bundle
        // under the application's previous name has to be folded in before the
        // insert, or the UNIQUE bundle index (0034) aborts the write.
        //
        // `app_only` is false: this write is the second rung of a correction that
        // also writes a window rule, so the correction history shows it once,
        // there.
        let Some((app_key_hash, bundle_key_hash)) = app_identity_for_event(&transaction, event_id)?
        else {
            return Ok(false);
        };
        upsert_app_scope_rule(
            &transaction,
            &app_key_hash,
            bundle_key_hash.as_deref(),
            category,
            local_activity_name,
            false,
        )?;
        transaction.commit()?;
        Ok(true)
    }

    fn save_personal_app_override_by_stable_id(
        &self,
        stable_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        // Resolved through the event rows for this mapping, the way
        // `remove_personal_override` resolves the same rung: an edit arrives
        // after its source event has left the queue, so the stable id is all the
        // client has. The most recent eligible event wins, because that is the
        // identity the next event of this rule will carry.
        let identity: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT app_stable_id, app_bundle_stable_id FROM raw_event_buffer
                 WHERE stable_id = ?1 AND app_stable_id IS NOT NULL
                   AND app_scope_eligible = 1
                 ORDER BY occurred_at DESC LIMIT 1",
                [stable_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        // And the application, not only the event, for the reason
        // `app_identity_for_event` states: a browser window Velvt read no site
        // from is eligible on its own row while the browser is not.
        let identity = eligible_app_identity(&transaction, identity)?;
        let Some((app_key_hash, bundle_key_hash)) = identity else {
            return Ok(false);
        };
        // Paired with a window rule, like every edit of a saved rule, so
        // `app_only` stays false and the history lists the pair once.
        upsert_app_scope_rule(
            &transaction,
            &app_key_hash,
            bundle_key_hash.as_deref(),
            category,
            local_activity_name,
            false,
        )?;
        transaction.commit()?;
        Ok(true)
    }

    fn save_app_scope_override(
        &self,
        app_key_hash: &str,
        bundle_key_hash: Option<&str>,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        // No event is consulted: the user is naming an application, and the key
        // came from the list Velvt itself offered. Idempotent by the conflict
        // branch; `correction_count` still advances, because how often someone
        // had to say the same thing is the signal that something upstream is
        // wrong.
        //
        // `app_only` is true, and that is the whole difference from the paired
        // path: there is no window rule behind this one, so the correction
        // history has to list this row itself or the user could never see or undo
        // what they taught here.
        //
        // The one thing that is checked, because the caller cannot: whether this
        // application may carry an app-wide rule at all. `unclassified_triage`
        // filters ineligible identities out of the list, so a well-behaved client
        // never asks -- and that filter is a query one edit away from being
        // widened, while the consequence of a Safari-wide rule is permanent and
        // invisible (every tab FOCUS_WORK at High confidence, and
        // `BrowserContextPlugin` never consulted for that browser again). The
        // guarantee `app_identity_for_event` documents belongs to the writer, not
        // to whoever happens to call it.
        if app_scope_identity_is_ineligible(&transaction, app_key_hash, bundle_key_hash)? {
            return Err(PersistenceError::AppScopeIneligible);
        }
        upsert_app_scope_rule(
            &transaction,
            app_key_hash,
            bundle_key_hash,
            category,
            local_activity_name,
            true,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn app_scope_override(
        &self,
        app_key_hash: &str,
    ) -> Result<Option<AppScopeOverride>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT app_key_hash, bundle_key_hash, category, activity_name,
                        correction_count, updated_at
                 FROM personal_app_override WHERE app_key_hash = ?1",
                [app_key_hash],
                app_scope_override_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn bundle_app_override(
        &self,
        bundle_key_hash: &str,
    ) -> Result<Option<AppScopeOverride>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT app_key_hash, bundle_key_hash, category, activity_name,
                        correction_count, updated_at
                 FROM personal_app_override WHERE bundle_key_hash = ?1",
                [bundle_key_hash],
                app_scope_override_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn remove_app_scope_override(&self, app_key_hash: &str) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "DELETE FROM personal_app_override WHERE app_key_hash = ?1",
            [app_key_hash],
        )?;
        // The typed name mirrored into every window of this application, for the
        // reason `remove_personal_override` nulls it: `display_name` records no
        // provenance, the upsert that writes it coalesces, so a name the user
        // typed would otherwise survive its own undo. A curated label is
        // deterministic and returns on the next observation of the window.
        transaction.execute(
            "UPDATE abstraction_map SET display_name = NULL
             WHERE stable_id IN (
                 SELECT stable_id FROM raw_event_buffer WHERE app_stable_id = ?1
             )",
            [app_key_hash],
        )?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    fn save_personal_override(
        &self,
        stable_id: &str,
        category: &str,
        local_activity_name: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        // `app_key_hash` (0036) records which app rung this correction also
        // wrote, so `remove_personal_override` can take both rungs away by key
        // lookup instead of re-deriving the pairing from `raw_event_buffer` --
        // which holds fourteen days, so the derivation failed silently for every
        // older correction and left an app rule nothing could list, remove or
        // re-teach. Resolved from the most recent eligible event of this mapping,
        // the same read `save_personal_app_override_by_stable_id` uses to choose
        // the rung to write, so the column names the rung that was written.
        //
        // NULL where the window is not generalizable at all (`app_scope_eligible
        // = 0`, a browser tab): no app rung was written, so there is none to
        // remove, and pointing at the browser's rung would let removing one tab's
        // rule delete a rule some other correction taught.
        //
        // COALESCE on conflict: an edit that arrives after the source events have
        // aged out resolves NULL, and must leave a pairing that was recorded when
        // they were still there rather than erase it.
        let changed = transaction.execute(
            "INSERT INTO personal_override(key_hash, category, activity_name, app_key_hash)
             SELECT map.key_hash, ?2, ?3,
                    (SELECT event.app_stable_id FROM raw_event_buffer event
                      WHERE event.stable_id = map.stable_id
                        AND event.app_stable_id IS NOT NULL
                        AND event.app_scope_eligible = 1
                        -- The application as well as the event, so this column
                        -- names an identity a rung was actually written for and
                        -- never a browser's.
                        AND NOT EXISTS (
                            SELECT 1 FROM raw_event_buffer ineligible
                             WHERE ineligible.app_scope_eligible = 0
                               AND (ineligible.app_stable_id = event.app_stable_id
                                    OR (event.app_bundle_stable_id IS NOT NULL
                                        AND ineligible.app_bundle_stable_id
                                            = event.app_bundle_stable_id))
                        )
                      ORDER BY event.occurred_at DESC LIMIT 1)
               FROM abstraction_map map WHERE map.stable_id = ?1
             ON CONFLICT(key_hash) DO UPDATE SET
                category = excluded.category,
                activity_name = COALESCE(excluded.activity_name, personal_override.activity_name),
                app_key_hash = COALESCE(excluded.app_key_hash, personal_override.app_key_hash),
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
            &format!("SELECT COUNT(*) FROM ({RULE_SOURCE}) WHERE {RULE_FILTER}"),
            [query],
            |row| row.get::<_, u64>(0),
        )?;
        let mut statement = connection.prepare(&format!(
            "SELECT scope, stable_id, label, local_label, category, updated_at
             FROM ({RULE_SOURCE})
             WHERE {RULE_FILTER}
             ORDER BY updated_at DESC, stable_id ASC
             LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = statement
            .query_map(params![query, limit as i64, offset as i64], |row| {
                Ok(PersonalOverrideRecord {
                    scope: match row.get::<_, String>(0)?.as_str() {
                        "app" => CorrectionScope::App,
                        _ => CorrectionScope::Window,
                    },
                    stable_id: row.get(1)?,
                    label: row.get(2)?,
                    local_activity_name: row.get(3)?,
                    category: row.get(4)?,
                    updated_at: timestamp_from_row(row, 5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::from)?;
        Ok((rows, total))
    }

    fn remove_personal_override(&self, stable_id: &str) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        // The app rung this correction wrote, read from the window rule itself
        // (`app_key_hash`, 0036) and read BEFORE the window rule is deleted.
        //
        // It used to be re-derived here by subquerying `raw_event_buffer`, and
        // that is a fourteen-day cache: past the TTL the subquery was empty, the
        // app rung survived, and Remove reported success having changed nothing
        // the user could see. Worse, the surviving row is `app_only = 0`, which
        // `RULE_SOURCE` hides from the correction history while
        // `unclassified_triage`'s NOT EXISTS still matches it -- so the
        // application could be neither listed, nor removed, nor taught again,
        // short of a full Reset. Recorded at write time, this is a key lookup
        // that does not read the event cache at all.
        //
        // NULL means there is no paired rung to remove: a browser tab, which
        // never wrote one, or a rule older than 0036 whose events were gone by
        // the time that migration could backfill it.
        let paired_app_key: Option<String> = transaction
            .query_row(
                "SELECT rule.app_key_hash FROM personal_override rule
                  WHERE rule.key_hash = (
                      SELECT key_hash FROM abstraction_map WHERE stable_id = ?1
                  )",
                [stable_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
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
        // Removing only the window rung left the engine falling through into the
        // surviving app rung and returning the same category and the same typed
        // name on the next event, so the undo the user asked for changed nothing
        // they could see. Hence this delete -- and hence `app_only = 0`, which is
        // every writer of that flag's condition read back in the delete
        // direction: a rule taught about the application itself through triage is
        // a rule of its own, sticky and never cleared (0035), and must survive
        // the removal of an unrelated window rule that happens to name the same
        // application. Without the predicate one Remove destroyed a rule the user
        // taught somewhere else entirely.
        if let Some(app_key_hash) = &paired_app_key {
            transaction.execute(
                "DELETE FROM personal_app_override
                  WHERE app_key_hash = ?1 AND app_only = 0",
                [app_key_hash],
            )?;
        }
        // The third place the typed name lives. Nulled rather than rewritten
        // because `display_name` records no provenance: nothing here can tell a
        // name the user typed from one `curated_display_label` produced, and the
        // upsert coalesces, so a later write can never null it. A curated label
        // is derived deterministically and comes back on the next observation of
        // that window; a typed one must not outlive its own undo. The sibling
        // windows of the same application are included because the app rung
        // mirrored the typed name into every one of them -- and they are found
        // under the app rung that was just removed (`?2`, NULL when there was
        // none), so a browser tab's undo no longer clears the local labels of
        // every other window of the browser. `raw_event_buffer` is still the only
        // place a window can be traced to its application, and that is sound
        // here: a name it cannot reach is one the next observation of that window
        // rewrites anyway.
        transaction.execute(
            "UPDATE abstraction_map SET display_name = NULL
             WHERE stable_id = ?1
                OR (?2 IS NOT NULL AND stable_id IN (
                    SELECT stable_id FROM raw_event_buffer WHERE app_stable_id = ?2
                 ))",
            params![stable_id, paired_app_key],
        )?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    fn reset_personal_overrides(&self) -> Result<u64, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute("DELETE FROM personal_override", [])? as u64;
        transaction.execute("DELETE FROM personal_semantic_prototype", [])?;
        // The app rung holds the same free-text `activity_name` under the
        // application's own hash. Without this the reset was a no-op for every
        // app-scoped correction — per migration 0017 the rung that carries
        // almost all of them — because the engine falls through the emptied
        // window rung into the app rung on the very next event.
        transaction.execute("DELETE FROM personal_app_override", [])?;
        // Every `display_name`, not only the rows a correction wrote. The column
        // records no provenance, so no query can separate a name the user typed
        // from one `curated_display_label` produced, and duplicating that
        // allowlist in SQL would put a second copy of it a migration away from
        // drifting. Clearing all of it is the only answer that is true for
        // certain: a curated label is deterministic and is rewritten on the next
        // observation of the window, so the cost is one event of a missing local
        // label, while a typed name surviving a reset the user was told was
        // destructive is a broken promise.
        transaction.execute("UPDATE abstraction_map SET display_name = NULL", [])?;
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

    fn delete_expired_semantic_embeddings(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM semantic_embedding_cache WHERE key_hash IN (
                 SELECT key_hash FROM semantic_embedding_cache
                 WHERE updated_at < ?1 LIMIT ?2
             )",
            params![cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }

    fn embedding_salt(&self) -> Result<EmbeddingSalt, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let stored = transaction
            .query_row("SELECT salt FROM embedding_salt WHERE id = 1", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()?;
        let bytes = match stored {
            Some(bytes) => <[u8; EmbeddingSalt::LENGTH]>::try_from(bytes.as_slice())
                // Unreachable while `CHECK(length(salt) = 32)` stands, which is
                // why this is an error rather than a silent re-mint: a row that
                // is present but unreadable is a corrupt database, and minting
                // over it would destroy the caches it was still keying.
                .map_err(|_| PersistenceError::InvalidEmbeddingSalt)?,
            None => {
                let minted = transaction.query_row(
                    "SELECT randomblob(?1)",
                    [EmbeddingSalt::LENGTH as i64],
                    |row| row.get::<_, Vec<u8>>(0),
                )?;
                transaction.execute(
                    "INSERT INTO embedding_salt(id, salt) VALUES (1, ?1)",
                    params![minted],
                )?;
                // Same reason migration 0031 empties both stores in the same
                // statement batch that mints the salt: every vector already on
                // disk was computed in a different space, and comparing across
                // spaces produces a similarity number that means nothing.
                transaction.execute("DELETE FROM semantic_embedding_cache", [])?;
                transaction.execute("DELETE FROM personal_semantic_prototype", [])?;
                <[u8; EmbeddingSalt::LENGTH]>::try_from(minted.as_slice())
                    .map_err(|_| PersistenceError::InvalidEmbeddingSalt)?
            }
        };
        transaction.commit()?;
        Ok(EmbeddingSalt::from_bytes(bytes))
    }

    fn stable_key_salt(&self) -> Result<StableKeySalt, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        let stored = transaction
            .query_row("SELECT salt FROM stable_key_salt WHERE id = 1", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()?;
        let bytes = match stored {
            // Unreachable while `CHECK(length(salt) = 32)` stands. An error rather
            // than a re-mint for the reason `embedding_salt` gives: a row that is
            // present but unreadable is a corrupt database, and minting over it
            // would destroy every correction it still keys.
            Some(bytes) => <[u8; StableKeySalt::LENGTH]>::try_from(bytes.as_slice())
                .map_err(|_| PersistenceError::InvalidStableKeySalt)?,
            None => {
                let minted = transaction.query_row(
                    "SELECT randomblob(?1)",
                    [StableKeySalt::LENGTH as i64],
                    |row| row.get::<_, Vec<u8>>(0),
                )?;
                transaction.execute(
                    "INSERT INTO stable_key_salt(id, salt) VALUES (1, ?1)",
                    params![minted],
                )?;
                // Every key below was computed under the salt that is gone, so
                // no lookup can reach it again. Kept, a window rule would still
                // be listed in the history and never apply, and correcting a
                // buffered event would write a new rule under the dead key --
                // both silent. Removed, the state is one the user can see: no
                // rules, and the next observation of each window keys afresh.
                transaction.execute_batch(
                    "DELETE FROM personal_override;
                     DELETE FROM personal_app_override;
                     DELETE FROM personal_semantic_prototype;
                     DELETE FROM semantic_embedding_cache;
                     DELETE FROM abstraction_map;
                     UPDATE raw_event_buffer
                        SET app_stable_id = NULL, app_bundle_stable_id = NULL
                      WHERE app_stable_id IS NOT NULL OR app_bundle_stable_id IS NOT NULL;",
                )?;
                tracing::warn!(
                    error_code = "stable_key_salt_reminted",
                    "stable-key salt was missing; corrections keyed under it were removed"
                );
                <[u8; StableKeySalt::LENGTH]>::try_from(minted.as_slice())
                    .map_err(|_| PersistenceError::InvalidStableKeySalt)?
            }
        };
        transaction.commit()?;
        Ok(StableKeySalt::from_bytes(bytes))
    }

    fn delete_expired_mappings(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        // Oldest first, on `idx_abstraction_map_updated_at`; each reference
        // check is a primary-key lookup (`personal_override.key_hash`) or an
        // index lookup (`idx_raw_event_buffer_stable_id`, migration 0037).
        let deleted = connection.execute(
            "DELETE FROM abstraction_map WHERE id IN (
                 SELECT map.id FROM abstraction_map map
                  WHERE map.updated_at < ?1
                    AND NOT EXISTS (
                        SELECT 1 FROM personal_override rule
                         WHERE rule.key_hash = map.key_hash
                    )
                    AND NOT EXISTS (
                        SELECT 1 FROM raw_event_buffer event
                         WHERE event.stable_id = map.stable_id
                    )
                  ORDER BY map.updated_at
                  LIMIT ?2
             )",
            params![cutoff.timestamp(), limit as i64],
        )?;
        Ok(deleted as u64)
    }
}

#[derive(Clone)]
struct SqliteRawEventRepo(SqlitePersistence);

impl RawEventRepo for SqliteRawEventRepo {
    fn insert(&self, event: &RawEventEntry) -> Result<(), PersistenceError> {
        // An event with no declared metadata is written exactly as it was
        // before the columns existed: three NULLs, no other difference.
        self.insert_with_declared_metadata(event, &DeclaredAppMetadata::ABSENT)
    }

    fn insert_with_declared_metadata(
        &self,
        event: &RawEventEntry,
        metadata: &DeclaredAppMetadata,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO raw_event_buffer(
                event_id, stable_id, label, local_display_label, local_name_suggestion, category, taxonomy_version, classification_tier, classification_status, classification_confidence, classification_source, occurred_at, duration_seconds, upload_eligible, app_stable_id, app_scope_eligible, app_bundle_stable_id, declared_app_category, document_type_ids
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
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
                event.upload_eligible,
                event.app_stable_id,
                event.app_scope_eligible,
                metadata.app_bundle_stable_id,
                metadata.declared_app_category,
                encode_document_type_ids(&metadata.document_type_ids),
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

    fn unclassified_triage(
        &self,
        lookback_days: u32,
        min_seconds: u64,
        limit: usize,
    ) -> Result<Vec<UnclassifiedAppEntry>, PersistenceError> {
        // Clamped here rather than trusted from the caller, the way
        // `local_display_aggregates` caps its own limit: these three bounds are
        // what keep the list a task instead of an inventory, and a caller that
        // could widen them could undo that from anywhere.
        let lookback_days = lookback_days.clamp(1, TRIAGE_MAX_LOOKBACK_DAYS);
        let min_seconds = min_seconds.max(TRIAGE_MIN_SECONDS);
        let limit = limit.min(TRIAGE_MAX_ENTRIES);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT app_stable_id,
                    -- An application Velvt holds no name for is still time the
                    -- user spent, so it is named plainly rather than dropped:
                    -- omitting the row hid real minutes from a list whose whole
                    -- claim is \"this is the time Velvt could not read\", and the
                    -- user can usually still answer -- they know what they had
                    -- open for an hour, and the row carries that hour. The
                    -- literal lives here for the reason `RULE_SOURCE`'s
                    -- 'application' does: it is a last-resort word, not a
                    -- category-derived label, so no mapping is duplicated into
                    -- SQL where it could drift.
                    COALESCE(display_name, 'Unnamed application') AS display_name,
                    seconds_observed, event_count,
                    app_bundle_stable_id
             FROM (
                 SELECT observed.app_stable_id AS app_stable_id,
                        -- The most recent name Velvt already holds for this
                        -- application. `local_name_suggestion` carries the raw
                        -- application name for exactly the events that matched
                        -- no seed and no correction (migration 0001), which is
                        -- every event in this list.
                        (SELECT COALESCE(named.local_display_label, named.local_name_suggestion)
                           FROM raw_event_buffer named
                          WHERE named.app_stable_id = observed.app_stable_id
                            AND COALESCE(named.local_display_label, named.local_name_suggestion)
                                IS NOT NULL
                          ORDER BY named.occurred_at DESC
                          LIMIT 1) AS display_name,
                        SUM(observed.duration_seconds) AS seconds_observed,
                        COUNT(*) AS event_count,
                        -- One application name resolves to one bundle
                        -- identifier, so any non-null value in the group is
                        -- that identifier; MAX is how SQLite says \"any\".
                        MAX(observed.app_bundle_stable_id) AS app_bundle_stable_id
                 FROM raw_event_buffer observed
                 WHERE observed.category = 'UNLOGGED'
                   AND observed.app_stable_id IS NOT NULL
                   AND observed.occurred_at >= ?1
                   -- Offering an application here is a claim that teaching it is
                   -- safe: the only thing the user can do with a row is write an
                   -- app-wide rule for it. So the same gate the correction path
                   -- applies per event (`app_identity_for_event`) applies to the
                   -- rows that make up a group, and to the application behind
                   -- them.
                   --
                   -- A browser window that carried a site context is not
                   -- generalizable -- one tab says nothing about the next -- and
                   -- an UNLOGGED one is exactly a site Velvt could not read, so
                   -- these rows were most of what the list offered for a browser.
                   -- Teaching one wrote an app-wide rule that classifies every
                   -- future tab, mail and video alike, at High confidence and
                   -- from `user_rule`, which the engine reads before the plugins
                   -- and which therefore also stops `BrowserContextPlugin` from
                   -- ever running for that browser again.
                   AND observed.app_scope_eligible = 1
                   -- And the application itself, not only these rows: a browser
                   -- also produces windows it read no site from, which are
                   -- eligible one row at a time while the application they belong
                   -- to is not. Any ineligible event under either identity is that
                   -- evidence, which is the same read
                   -- `save_app_scope_override` refuses on -- this filter keeps the
                   -- list honest, that check keeps the promise.
                   AND NOT EXISTS (
                       SELECT 1 FROM raw_event_buffer ineligible
                       WHERE ineligible.app_scope_eligible = 0
                         AND (ineligible.app_stable_id = observed.app_stable_id
                              OR (observed.app_bundle_stable_id IS NOT NULL
                                  AND ineligible.app_bundle_stable_id
                                      = observed.app_bundle_stable_id))
                   )
                   -- An application the user has already taught must leave the
                   -- list the moment they teach it. Past events keep their
                   -- UNLOGGED category -- nothing here rewrites history -- so
                   -- without this the app they just explained would be back at
                   -- the top of the list tomorrow.
                   AND NOT EXISTS (
                       SELECT 1 FROM personal_app_override rule
                       WHERE rule.app_key_hash = observed.app_stable_id
                          OR (rule.bundle_key_hash IS NOT NULL
                              AND rule.bundle_key_hash = observed.app_bundle_stable_id)
                   )
                 GROUP BY observed.app_stable_id
             )
             WHERE seconds_observed >= ?2
             ORDER BY seconds_observed DESC, app_stable_id ASC
             LIMIT ?3",
        )?;
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(lookback_days));
        let entries = statement
            .query_map(
                params![cutoff.timestamp(), min_seconds as i64, limit as i64],
                |row| {
                    Ok(UnclassifiedAppEntry {
                        app_stable_id: row.get(0)?,
                        display_name: row.get(1)?,
                        seconds_observed: row.get(2)?,
                        event_count: row.get(3)?,
                        app_bundle_stable_id: row.get(4)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(PersistenceError::from)?;
        Ok(entries)
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
        // Rows the upload pipeline still owes the backend are spared. The
        // predicate is character-for-character the one `unbatched_events` uses,
        // and that is the whole rule: a row is kept exactly while
        // `recover_unbatched` would re-queue it at the next start. An eligible
        // row with no `batch_event` was acked to Swift and never reached a
        // batch — the service died between the ack and the flush, or the
        // backlog was longer than one start's recovery limit — and expiring it
        // on the TTL deleted an accepted event that nothing could re-send.
        //
        // This comment used to say the rule mirrored upload-batch retention,
        // "which deliberately never deletes pending or failed batches."
        // `delete_stale_queued_batch` deletes exactly those, so that sentence
        // is gone rather than softened. What the rule actually rests on is an
        // ordering of horizons: a batched row has to be deleted here before the
        // sweep that deletes its batch cascades the `batch_event` away, because
        // a row whose `batch_event` disappears re-enters the spared set and is
        // never collected again. The raw TTL is 14 days
        // (`VELVT_RAW_EVENT_TTL_HOURS`, defaulted from `DAILY_ACTIVITY_DAYS`)
        // against a 30-day sent-and-queued batch horizon
        // (`VELVT_SENT_BATCH_RETENTION_DAYS`), so it holds by 16 days — and
        // inverts the moment the TTL is raised past 720 hours.
        // `tests/retention.rs::the_raw_event_horizon_stays_inside_the_batch_horizon`
        // is that ordering as an assertion rather than as two numbers that
        // happen to be in the right order.
        //
        // Open, found 2026-08-31, not fixed here: `delete_rejected_batch` runs
        // at 7 days (`VELVT_REJECTED_BATCH_AUDIT_DAYS`), which is inside the
        // raw TTL, so a rejected batch's rows do come back unbatched and
        // `recover_unbatched` re-queues them at the next start — against the
        // rule in `upload/coordinator.rs` that a `raw_field_rejected` batch is
        // permanently terminal and must never re-enter retry scheduling. The
        // development device has never held a rejected batch, so nothing has
        // taken that path. The fix belongs in the rejected sweep or its
        // horizon, not in this predicate, which is why it is named here instead
        // of quietly widened.
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM raw_event_buffer WHERE id IN (
                 SELECT id FROM raw_event_buffer
                 WHERE created_at < ?1
                   AND NOT (
                       upload_eligible = 1
                       AND NOT EXISTS (
                           SELECT 1 FROM batch_event
                           WHERE batch_event.event_id = raw_event_buffer.event_id
                       )
                   )
                 LIMIT ?2
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
        // `abandoned` belongs in this list even though it has no count of its
        // own: the error that ended a batch's retries is the most recent thing
        // the user has not been told, and dropping it would let a queue that
        // failed its way to terminal report no error at all.
        let last_error_code = connection
            .query_row(
                "SELECT last_error_code
                 FROM upload_batch
                 WHERE status IN ('pending', 'failed', 'rejected', 'abandoned')
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
        update_batch_retry_state(
            &self.0,
            "failed",
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
        update_batch_retry_state(
            &self.0,
            "pending",
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

    fn delete_stale_queued_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        // Deliberately no status filter. The two statuses the other sweeps
        // enumerate were the only ones ever collected, so a batch that never
        // reached one of them lived forever; naming statuses here would
        // reproduce that hole the next time the vocabulary grows. A sent batch
        // that outlived this horizon by creation date is collected too — its
        // events are already delivered, so removing the local copy early is
        // never a loss.
        let deleted = connection.execute(
            "DELETE FROM upload_batch WHERE id IN (
                 SELECT id FROM upload_batch WHERE created_at < ?1 LIMIT ?2
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
                ended_at, recovered_after_restart, recovery_of, origin,
                intention_expires_at, updated_at
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                block.origin.as_str(),
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
                        recovered_after_restart, recovery_of, origin,
                        intention_expires_at, updated_at
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
                        recovered_after_restart, recovery_of, origin,
                        intention_expires_at, updated_at
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

    fn reported_dwells(
        &self,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<ReportedDwell>, PersistenceError> {
        if until <= from {
            return Ok(Vec::new());
        }
        let connection = self.0.connection()?;
        // The lower bound on `occurred_at` is what keeps this on
        // `idx_raw_event_buffer_occurred_at`: no dwell longer than the cap is
        // ever stored, so none that overlaps `from` can have begun earlier.
        let mut statement = connection.prepare(
            "SELECT occurred_at, MIN(MAX(duration_seconds, 0), ?4), category,
                    classification_status, classification_confidence
             FROM raw_event_buffer
             WHERE occurred_at >= ?1 AND occurred_at < ?3
               AND occurred_at + MIN(MAX(duration_seconds, 0), ?4) > ?2
             ORDER BY occurred_at, id",
        )?;
        let dwells = statement
            .query_map(
                params![
                    from.timestamp() - i64::from(MAX_REPORTED_DWELL_SECONDS),
                    from.timestamp(),
                    until.timestamp(),
                    MAX_REPORTED_DWELL_SECONDS,
                ],
                |row| {
                    // Read leniently. A vocabulary token this build does not
                    // know supports no category claim, and that is all the
                    // engine needs from it; failing the read instead would
                    // leave a block that can never be finalized.
                    let status = row.get::<_, String>(3)?;
                    let confidence = row.get::<_, String>(4)?;
                    Ok(ReportedDwell {
                        occurred_at: timestamp_from_row(row, 0)?,
                        duration_seconds: row.get(1)?,
                        category: row.get(2)?,
                        classification_status: parse_classification_status_value(&status)
                            .unwrap_or(ClassificationStatus::Unclassified),
                        classification_confidence: parse_classification_confidence_value(
                            &confidence,
                        )
                        .unwrap_or(ClassificationConfidence::None),
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(dwells)
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
                switch_count, window_seconds, outcome, outcome_at, salience
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
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
                intervention.salience.as_str(),
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
                        window_seconds, outcome, outcome_at, salience, card_seen_at
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
                        salience: parse_intervention_salience(&row.get::<_, String>(7)?)?,
                        card_seen_at: row
                            .get::<_, Option<i64>>(8)?
                            .map(|value| timestamp_to_datetime(value, 8))
                            .transpose()?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn recent_interventions(
        &self,
        limit: usize,
    ) -> Result<Vec<WorkBlockIntervention>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT offered_at, action_id, anchor_category, switch_count,
                    window_seconds, outcome, outcome_at, salience, card_seen_at
             FROM work_block_intervention
             ORDER BY offered_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(WorkBlockIntervention {
                offered_at: timestamp_from_row(row, 0)?,
                action_id: row.get(1)?,
                anchor_category: row.get(2)?,
                switch_count: row.get(3)?,
                window_seconds: row.get(4)?,
                outcome: WorkBlockInterventionOutcome::from_db_value(&row.get::<_, String>(5)?)
                    .ok_or_else(invalid_enum)?,
                outcome_at: row
                    .get::<_, Option<i64>>(6)?
                    .map(|value| timestamp_to_datetime(value, 6))
                    .transpose()?,
                salience: parse_intervention_salience(&row.get::<_, String>(7)?)?,
                card_seen_at: row
                    .get::<_, Option<i64>>(8)?
                    .map(|value| timestamp_to_datetime(value, 8))
                    .transpose()?,
            })
        })?;
        rows.map(|row| row.map_err(PersistenceError::from))
            .collect()
    }

    fn mark_intervention_card_seen(
        &self,
        block_id: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // `card_seen_at IS NULL` makes the first sighting win. A card that
        // re-renders when the popover is reopened is the same delivery, and
        // moving the timestamp forward would report the offer as reaching the
        // user later than it did.
        let changed = connection.execute(
            "UPDATE work_block_intervention
             SET card_seen_at = ?2
             WHERE block_id = ?1 AND card_seen_at IS NULL",
            params![block_id, at.timestamp()],
        )?;
        Ok(changed > 0)
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

    fn record_category_correction(
        &self,
        block_id: &str,
        correction: &WorkBlockCategoryCorrection,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // The first correction for a category wins. A user correcting the same
        // category twice in one block is restating, not revising, and silently
        // overwriting the original timestamp would misreport when they were
        // first believed.
        connection.execute(
            "INSERT INTO work_block_category_correction(
                block_id, category, counts_as_category, corrected_at
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(block_id, category) DO NOTHING",
            params![
                block_id,
                correction.category,
                correction.counts_as_category,
                correction.corrected_at.timestamp()
            ],
        )?;
        Ok(())
    }

    fn category_corrections(
        &self,
        block_id: &str,
    ) -> Result<Vec<WorkBlockCategoryCorrection>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT category, counts_as_category, corrected_at
             FROM work_block_category_correction
             WHERE block_id = ?1 ORDER BY corrected_at",
        )?;
        let rows = statement.query_map([block_id], |row| {
            Ok(WorkBlockCategoryCorrection {
                category: row.get(0)?,
                counts_as_category: row.get(1)?,
                corrected_at: timestamp_from_row(row, 2)?,
            })
        })?;
        let mut corrections = Vec::new();
        for correction in rows {
            corrections.push(correction?);
        }
        Ok(corrections)
    }

    fn wrong_intervention_counts(
        &self,
        since: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, PersistenceError> {
        let connection = self.0.connection()?;
        wrong_intervention_counts_sql(&connection, since.timestamp(), i64::MAX)
    }

    fn demotion_state(&self) -> Result<Option<DemotionStateRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT state, demoted_at, manual_reset_at, threshold_policy_version,
                        repromotion_policy_version, updated_at
                 FROM intervention_demotion_state WHERE id = 1",
                [],
                |row| {
                    Ok(DemotionStateRecord {
                        state: InterventionDemotionState::from_db_value(&row.get::<_, String>(0)?)
                            .ok_or_else(invalid_enum)?,
                        demoted_at: optional_timestamp_from_row(row, 1)?,
                        manual_reset_at: optional_timestamp_from_row(row, 2)?,
                        threshold_policy_version: row.get(3)?,
                        repromotion_policy_version: row.get(4)?,
                        updated_at: timestamp_from_row(row, 5)?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn set_demotion_state(&self, record: &DemotionStateRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO intervention_demotion_state(
                id, state, demoted_at, manual_reset_at, threshold_policy_version,
                repromotion_policy_version, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                demoted_at = excluded.demoted_at,
                manual_reset_at = excluded.manual_reset_at,
                threshold_policy_version = excluded.threshold_policy_version,
                repromotion_policy_version = excluded.repromotion_policy_version,
                updated_at = excluded.updated_at",
            params![
                record.state.as_str(),
                record.demoted_at.map(|at| at.timestamp()),
                record.manual_reset_at.map(|at| at.timestamp()),
                record.threshold_policy_version,
                record.repromotion_policy_version,
                record.updated_at.timestamp(),
            ],
        )?;
        Ok(())
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
        let removed = connection.execute("DELETE FROM work_block", [])? as u64;
        // The demotion state is derived behavioral evidence, not a user
        // preference: it dies with the record it was derived from.
        connection.execute("DELETE FROM intervention_demotion_state", [])?;
        // Block-scoped decisions leave with their block by cascade. A decision
        // logged without a block (there is no such write site today, but the
        // column is nullable) would otherwise survive a clear, so it is removed
        // explicitly rather than relying on the foreign key.
        connection.execute(
            "DELETE FROM intervention_decision_log WHERE block_id IS NULL",
            [],
        )?;
        Ok(removed)
    }

    fn record_decision(&self, decision: &InterventionDecision) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // A replayed decision id is a no-op rather than an error: a retried
        // write must not double-count an evaluation in the eligibility rate.
        connection.execute(
            "INSERT INTO intervention_decision_log(
                decision_id, occurred_at, block_id, policy_version, anchor_category,
                switch_count, elapsed_seconds, remaining_seconds, gate_verdict,
                propensity, anchor_seen_within_600s, outcome_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(decision_id) DO NOTHING",
            params![
                decision.decision_id,
                decision.occurred_at.timestamp(),
                decision.block_id,
                decision.policy_version,
                decision.anchor_category,
                decision.switch_count,
                decision.elapsed_seconds,
                decision.remaining_seconds,
                decision.gate_verdict.as_str(),
                decision.propensity,
                decision.anchor_seen_within_600s.map(i64::from),
                decision.outcome_at.map(|at| at.timestamp()),
            ],
        )?;
        Ok(())
    }

    fn decisions(&self, block_id: &str) -> Result<Vec<InterventionDecision>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT decision_id, occurred_at, block_id, policy_version, anchor_category,
                    switch_count, elapsed_seconds, remaining_seconds, gate_verdict,
                    propensity, anchor_seen_within_600s, outcome_at
             FROM intervention_decision_log
             WHERE block_id = ?1
             ORDER BY occurred_at ASC, decision_id ASC",
        )?;
        let rows = statement.query_map([block_id], decision_from_row)?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(row?);
        }
        Ok(decisions)
    }

    fn recent_decisions(
        &self,
        limit: usize,
    ) -> Result<Vec<InterventionDecision>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT decision_id, occurred_at, block_id, policy_version, anchor_category,
                    switch_count, elapsed_seconds, remaining_seconds, gate_verdict,
                    propensity, anchor_seen_within_600s, outcome_at
             FROM intervention_decision_log
             ORDER BY occurred_at DESC, decision_id DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], decision_from_row)?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(row?);
        }
        Ok(decisions)
    }

    fn unresolved_decisions(
        &self,
        horizon_closed_by: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<InterventionDecision>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT decision_id, occurred_at, block_id, policy_version, anchor_category,
                    switch_count, elapsed_seconds, remaining_seconds, gate_verdict,
                    propensity, anchor_seen_within_600s, outcome_at
             FROM intervention_decision_log
             WHERE anchor_seen_within_600s IS NULL
               AND anchor_category IS NOT NULL
               AND block_id IS NOT NULL
               AND occurred_at <= ?1
             ORDER BY occurred_at ASC, decision_id ASC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![horizon_closed_by.timestamp(), limit as i64],
            decision_from_row,
        )?;
        let mut decisions = Vec::new();
        for row in rows {
            decisions.push(row?);
        }
        Ok(decisions)
    }

    fn observed_category_between(
        &self,
        block_id: &str,
        category: &str,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // `occurred_at > ?3`, not `>=`. `observe_safe_category` appends the
        // observation before it calls `evaluate_drift`, so the observation that
        // triggered a decision is already on disk carrying the decision's own
        // timestamp. Under `>=` the `AbstainedAtAnchor` verdict — which fires
        // exactly when the latest confident observation IS the anchor — would
        // read back as "the user returned" for every row of it, definitionally
        // and without a single return having happened. The bound answers what
        // happened after the decision, so the evidence the decision was made on
        // is not part of the answer.
        connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM work_block_observation
                    WHERE block_id = ?1 AND lower(category) = lower(?2)
                      AND occurred_at > ?3 AND occurred_at <= ?4
                 )",
                params![block_id, category, from.timestamp(), until.timestamp()],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn resolve_decision(
        &self,
        decision_id: &str,
        anchor_seen: bool,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // `anchor_seen_within_600s IS NULL` is the whole idempotence guarantee:
        // a decision answered once keeps the answer it was given, so rerunning
        // the resolver over history cannot move a number anyone has read.
        let updated = connection.execute(
            "UPDATE intervention_decision_log
             SET anchor_seen_within_600s = ?2, outcome_at = ?3
             WHERE decision_id = ?1 AND anchor_seen_within_600s IS NULL",
            params![decision_id, i64::from(anchor_seen), at.timestamp()],
        )?;
        Ok(updated > 0)
    }
}

/// Reads one logged decision. An unrecognised `gate_verdict` is an error, not a
/// defaulted variant: a row written by a newer binary must never read back as an
/// older meaning, because every downstream count would then be wrong and silent.
fn decision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InterventionDecision> {
    let verdict: String = row.get(8)?;
    let gate_verdict = GateVerdict::from_stored(&verdict).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unrecognised gate verdict",
            )),
        )
    })?;
    Ok(InterventionDecision {
        decision_id: row.get(0)?,
        occurred_at: timestamp_from_row(row, 1)?,
        block_id: row.get(2)?,
        policy_version: row.get(3)?,
        anchor_category: row.get(4)?,
        switch_count: row.get(5)?,
        elapsed_seconds: row.get(6)?,
        remaining_seconds: row.get(7)?,
        gate_verdict,
        propensity: row.get(9)?,
        anchor_seen_within_600s: row.get::<_, Option<i64>>(10)?.map(|value| value != 0),
        outcome_at: optional_timestamp_from_row(row, 11)?,
    })
}

/// The one delivered/withheld split, written once. `delivered` counts every
/// row that was actually shown; DND-suppressed and demotion-withheld rows
/// were delivered by no channel and are excluded. The rolling counter, the
/// digest's weekly window, and the probe denominator all read through this
/// single SQL body, so no consumer can drift from another.
fn wrong_intervention_counts_sql(
    connection: &rusqlite::Connection,
    since: i64,
    until: i64,
) -> Result<WrongInterventionCounts, PersistenceError> {
    connection
        .query_row(
            "SELECT
                COALESCE(SUM(outcome NOT IN ('delivery_suppressed_dnd', 'withheld_demotion')), 0),
                COALESCE(SUM(outcome = 'was_focused'), 0)
             FROM work_block_intervention WHERE offered_at >= ?1 AND offered_at < ?2",
            params![since, until],
            |row| {
                Ok(WrongInterventionCounts {
                    delivered: row.get(0)?,
                    was_focused: row.get(1)?,
                })
            },
        )
        .map_err(PersistenceError::from)
}

struct SqliteFocusRepo(SqlitePersistence);

impl FocusRepo for SqliteFocusRepo {
    fn record_focus_transition(
        &self,
        transition: &FocusTransition,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let current: Option<i64> = connection
            .query_row(
                "SELECT active FROM focus_state_evidence
                 ORDER BY changed_at_bucket DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if current == Some(i64::from(transition.active)) {
            return Ok(false);
        }
        connection.execute(
            "INSERT INTO focus_state_evidence(
                active, changed_at_bucket, local_hour, local_date, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                transition.active,
                transition.changed_at_bucket.timestamp(),
                transition.local_hour,
                transition.local_date,
                transition.recorded_at.timestamp(),
            ],
        )?;
        Ok(true)
    }

    fn latest_focus_transition(&self) -> Result<Option<FocusTransition>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT active, changed_at_bucket, local_hour, local_date, recorded_at
                 FROM focus_state_evidence
                 ORDER BY changed_at_bucket DESC, id DESC LIMIT 1",
                [],
                focus_transition_from_row,
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn focus_state_at_bucket(
        &self,
        bucket: DateTime<Utc>,
    ) -> Result<Option<bool>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT active FROM focus_state_evidence
                 WHERE changed_at_bucket <= ?1
                 ORDER BY changed_at_bucket DESC, id DESC LIMIT 1",
                [bucket.timestamp()],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn focus_active_dates_in_hours(&self, hours: &[u32]) -> Result<Vec<String>, PersistenceError> {
        if hours.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.0.connection()?;
        let placeholders = std::iter::repeat_n("?", hours.len())
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection.prepare(&format!(
            "SELECT DISTINCT local_date FROM focus_state_evidence
             WHERE active = 1 AND local_hour IN ({placeholders})
             ORDER BY local_date DESC"
        ))?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(hours.iter()), |row| row.get(0))?;
        let mut dates = Vec::new();
        for row in rows {
            dates.push(row?);
        }
        Ok(dates)
    }

    fn prune_focus_evidence(&self, cutoff: DateTime<Utc>) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute(
            "DELETE FROM focus_state_evidence WHERE changed_at_bucket < ?1",
            [cutoff.timestamp()],
        )? as u64)
    }

    fn set_utc_offset(&self, seconds: i32, at: DateTime<Utc>) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO focus_observer_state(id, utc_offset_seconds, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE
             SET utc_offset_seconds = excluded.utc_offset_seconds,
                 updated_at = excluded.updated_at",
            params![seconds, at.timestamp()],
        )?;
        Ok(())
    }

    fn utc_offset_seconds(&self) -> Result<Option<i32>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT utc_offset_seconds FROM focus_observer_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn quiet_hours_offer_state(&self) -> Result<Option<QuietHoursOfferState>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT rule_version, triggered_at, offered_at, response, responded_at
                 FROM quiet_hours_offer_state WHERE id = 1",
                [],
                |row| {
                    Ok(QuietHoursOfferState {
                        rule_version: row.get(0)?,
                        triggered_at: row
                            .get::<_, Option<i64>>(1)?
                            .map(|value| timestamp_to_datetime(value, 1))
                            .transpose()?,
                        offered_at: row
                            .get::<_, Option<i64>>(2)?
                            .map(|value| timestamp_to_datetime(value, 2))
                            .transpose()?,
                        response: row
                            .get::<_, Option<String>>(3)?
                            .as_deref()
                            .and_then(QuietHoursOfferResponse::from_db_value),
                        responded_at: row
                            .get::<_, Option<i64>>(4)?
                            .map(|value| timestamp_to_datetime(value, 4))
                            .transpose()?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn record_quiet_hours_trigger(
        &self,
        rule_version: u32,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO quiet_hours_offer_state(
                id, rule_version, triggered_at, offered_at, response, responded_at
             ) VALUES (1, ?1, ?2, NULL, NULL, NULL)
             ON CONFLICT(id) DO UPDATE
             SET rule_version = excluded.rule_version,
                 triggered_at = excluded.triggered_at,
                 offered_at = NULL,
                 response = NULL,
                 responded_at = NULL",
            params![rule_version, at.timestamp()],
        )?;
        Ok(())
    }

    fn record_quiet_hours_offered(&self, at: DateTime<Utc>) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE quiet_hours_offer_state SET offered_at = COALESCE(offered_at, ?1)
             WHERE id = 1",
            [at.timestamp()],
        )?;
        Ok(())
    }

    fn record_quiet_hours_response(
        &self,
        response: QuietHoursOfferResponse,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // Only an unanswered offer transitions: a reply cannot be rewritten.
        connection.execute(
            "UPDATE quiet_hours_offer_state
             SET response = ?1, responded_at = ?2
             WHERE id = 1 AND response IS NULL",
            params![response.as_str(), at.timestamp()],
        )?;
        Ok(())
    }

    fn quiet_hours(&self) -> Result<Option<VelvtQuietHours>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT start_local_minutes, end_local_minutes, rule_version, configured_at
                 FROM velvt_quiet_hours WHERE id = 1",
                [],
                |row| {
                    Ok(VelvtQuietHours {
                        start_local_minutes: row.get(0)?,
                        end_local_minutes: row.get(1)?,
                        rule_version: row.get(2)?,
                        configured_at: timestamp_from_row(row, 3)?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn set_quiet_hours(&self, quiet_hours: &VelvtQuietHours) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO velvt_quiet_hours(
                id, start_local_minutes, end_local_minutes, rule_version, configured_at
             ) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE
             SET start_local_minutes = excluded.start_local_minutes,
                 end_local_minutes = excluded.end_local_minutes,
                 rule_version = excluded.rule_version,
                 configured_at = excluded.configured_at",
            params![
                quiet_hours.start_local_minutes,
                quiet_hours.end_local_minutes,
                quiet_hours.rule_version,
                quiet_hours.configured_at.timestamp(),
            ],
        )?;
        Ok(())
    }

    fn clear_focus_evidence(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let removed = connection.execute("DELETE FROM focus_state_evidence", [])? as u64;
        connection.execute("DELETE FROM focus_observer_state", [])?;
        connection.execute("DELETE FROM quiet_hours_offer_state", [])?;
        Ok(removed)
    }
}

#[derive(Clone)]
struct SqliteInitiationRepo(SqlitePersistence);

impl InitiationRepo for SqliteInitiationRepo {
    fn record_invitation(
        &self,
        invitation: &InitiationInvitationRecord,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO initiation_invitation(
                invitation_id, offered_at, local_date, action_id,
                policy_version, backoff_policy_version, outcome, outcome_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                invitation.invitation_id,
                invitation.offered_at.timestamp(),
                invitation.local_date,
                invitation.action_id,
                invitation.policy_version,
                invitation.backoff_policy_version,
                invitation.outcome.as_str(),
                invitation.outcome_at.map(|value| value.timestamp()),
            ],
        )?;
        Ok(())
    }

    fn invitation(
        &self,
        invitation_id: &str,
    ) -> Result<Option<InitiationInvitationRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT invitation_id, offered_at, local_date, action_id,
                        policy_version, backoff_policy_version, outcome, outcome_at
                 FROM initiation_invitation WHERE invitation_id = ?1",
                [invitation_id],
                initiation_invitation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn open_invitation(&self) -> Result<Option<InitiationInvitationRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT invitation_id, offered_at, local_date, action_id,
                        policy_version, backoff_policy_version, outcome, outcome_at
                 FROM initiation_invitation WHERE outcome = 'offered'
                 ORDER BY offered_at DESC LIMIT 1",
                [],
                initiation_invitation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn recent_invitations(
        &self,
        limit: usize,
    ) -> Result<Vec<InitiationInvitationRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT invitation_id, offered_at, local_date, action_id,
                    policy_version, backoff_policy_version, outcome, outcome_at
             FROM initiation_invitation ORDER BY offered_at DESC, invitation_id DESC LIMIT ?1",
        )?;
        let rows = statement
            .query_map([limit as i64], initiation_invitation_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn resolve_invitation(
        &self,
        invitation_id: &str,
        outcome: InitiationInvitationOutcome,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let updated = connection.execute(
            "UPDATE initiation_invitation SET outcome = ?2, outcome_at = ?3
             WHERE invitation_id = ?1 AND outcome = 'offered'",
            params![invitation_id, outcome.as_str(), at.timestamp()],
        )?;
        Ok(updated > 0)
    }

    fn invitations_on_local_date(&self, local_date: &str) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM initiation_invitation WHERE local_date = ?1",
            [local_date],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn invitations_enabled(&self) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let enabled: Option<i64> = connection
            .query_row(
                "SELECT invitations_enabled FROM initiation_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(enabled.map(|value| value != 0).unwrap_or(true))
    }

    fn set_invitations_enabled(
        &self,
        enabled: bool,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO initiation_settings(id, invitations_enabled, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                invitations_enabled = excluded.invitations_enabled,
                updated_at = excluded.updated_at",
            params![i64::from(enabled), at.timestamp()],
        )?;
        Ok(())
    }

    fn completed_block_count(&self, since: DateTime<Utc>) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM work_block
             WHERE phase = 'completed' AND started_at >= ?1",
            [since.timestamp()],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn completed_block_dwell_spans(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<CompletedBlockDwellSpan>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT observation.block_id, observation.occurred_at, observation.ended_at
             FROM work_block_observation observation
             JOIN work_block block ON block.block_id = observation.block_id
             WHERE block.phase = 'completed'
               AND block.started_at >= ?1
               AND observation.ended_at IS NOT NULL
               AND observation.ended_at > observation.occurred_at
               AND observation.classification_status = 'classified'
               AND observation.classification_confidence IN ('high', 'medium')
               AND lower(observation.category) NOT IN ('system', 'unclassified', 'unlogged')
             ORDER BY observation.occurred_at, observation.id",
        )?;
        let rows = statement
            .query_map([since.timestamp()], |row| {
                Ok(CompletedBlockDwellSpan {
                    block_id: row.get(0)?,
                    started_at: timestamp_from_row(row, 1)?,
                    ended_at: timestamp_from_row(row, 2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn clear_invitations(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let removed = connection.execute("DELETE FROM initiation_invitation", [])? as u64;
        Ok(removed)
    }
}

struct SqliteReceiptsRepo(SqlitePersistence);

impl ReceiptsRepo for SqliteReceiptsRepo {
    fn declared_block_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM work_block
             WHERE started_at >= ?1 AND started_at < ?2",
            params![since.timestamp(), until.timestamp()],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn completed_block_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        // Same predicate as `InitiationRepo::completed_block_count`, bounded:
        // the digest's completed count cannot drift from the cold-start
        // gate's definition of a completed block.
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM work_block
             WHERE phase = 'completed' AND started_at >= ?1 AND started_at < ?2",
            params![since.timestamp(), until.timestamp()],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn result_payloads_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<String>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT result.payload
             FROM work_block_result result
             JOIN work_block block ON block.block_id = result.block_id
             WHERE block.started_at >= ?1 AND block.started_at < ?2
             ORDER BY block.started_at, block.block_id",
        )?;
        let rows = statement
            .query_map(params![since.timestamp(), until.timestamp()], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn accepted_invitation_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM initiation_invitation
             WHERE outcome = 'accepted' AND outcome_at >= ?1 AND outcome_at < ?2",
            params![since.timestamp(), until.timestamp()],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn withheld_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM work_block_intervention
             WHERE outcome IN ('delivery_suppressed_dnd', 'withheld_demotion')
               AND offered_at >= ?1 AND offered_at < ?2",
            params![since.timestamp(), until.timestamp()],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn wrong_intervention_counts_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<WrongInterventionCounts, PersistenceError> {
        let connection = self.0.connection()?;
        wrong_intervention_counts_sql(&connection, since.timestamp(), until.timestamp())
    }

    fn delivered_intervention_count_between(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let counts =
            wrong_intervention_counts_sql(&connection, since.timestamp(), until.timestamp())?;
        Ok(u64::from(counts.delivered))
    }

    fn weekly_digest(
        &self,
        week_start_local_date: &str,
    ) -> Result<Option<WeeklyDigestRecord>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT week_start_local_date, generated_at, blocks_declared,
                        blocks_completed, recoveries, wrong_interventions,
                        invitations_accepted, withheld, digest_version,
                        delivered_at, acknowledged_at
                 FROM weekly_digest WHERE week_start_local_date = ?1",
                [week_start_local_date],
                weekly_digest_from_row,
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn store_weekly_digest(&self, record: &WeeklyDigestRecord) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // Insert-only: a stored digest is frozen. Re-generating a week is a
        // policy violation, not an upsert.
        connection.execute(
            "INSERT INTO weekly_digest(
                week_start_local_date, generated_at, blocks_declared,
                blocks_completed, recoveries, wrong_interventions,
                invitations_accepted, withheld, digest_version,
                delivered_at, acknowledged_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.week_start_local_date,
                record.generated_at.timestamp(),
                record.blocks_declared,
                record.blocks_completed,
                record.recoveries,
                record.wrong_interventions,
                record.invitations_accepted,
                record.withheld,
                record.digest_version,
                record.delivered_at.map(|at| at.timestamp()),
                record.acknowledged_at.map(|at| at.timestamp()),
            ],
        )?;
        Ok(())
    }

    fn mark_digest_delivered(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE weekly_digest SET delivered_at = ?2
             WHERE week_start_local_date = ?1 AND delivered_at IS NULL",
            params![week_start_local_date, at.timestamp()],
        )?;
        Ok(())
    }

    fn acknowledge_digest(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "UPDATE weekly_digest SET acknowledged_at = ?2
             WHERE week_start_local_date = ?1 AND acknowledged_at IS NULL",
            params![week_start_local_date, at.timestamp()],
        )?;
        Ok(())
    }

    fn record_explain_tap(
        &self,
        week_start_local_date: &str,
        at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO explain_probe_week(week_start_local_date, taps, updated_at)
             VALUES (?1, 1, ?2)
             ON CONFLICT(week_start_local_date) DO UPDATE SET
                taps = taps + 1,
                updated_at = excluded.updated_at",
            params![week_start_local_date, at.timestamp()],
        )?;
        Ok(())
    }

    fn explain_taps_for_week(&self, week_start_local_date: &str) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let taps: Option<i64> = connection
            .query_row(
                "SELECT taps FROM explain_probe_week WHERE week_start_local_date = ?1",
                [week_start_local_date],
                |row| row.get(0),
            )
            .optional()?;
        Ok(taps.unwrap_or(0) as u64)
    }

    fn prune_receipts_before(&self, week_start_local_date: &str) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let mut removed = connection.execute(
            "DELETE FROM weekly_digest WHERE week_start_local_date < ?1",
            [week_start_local_date],
        )? as u64;
        removed += connection.execute(
            "DELETE FROM explain_probe_week WHERE week_start_local_date < ?1",
            [week_start_local_date],
        )? as u64;
        Ok(removed)
    }

    fn clear_receipts(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let mut removed = connection.execute("DELETE FROM weekly_digest", [])? as u64;
        removed += connection.execute("DELETE FROM explain_probe_week", [])? as u64;
        Ok(removed)
    }
}

fn weekly_digest_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WeeklyDigestRecord> {
    Ok(WeeklyDigestRecord {
        week_start_local_date: row.get(0)?,
        generated_at: timestamp_from_row(row, 1)?,
        blocks_declared: row.get(2)?,
        blocks_completed: row.get(3)?,
        recoveries: row.get(4)?,
        wrong_interventions: row.get(5)?,
        invitations_accepted: row.get(6)?,
        withheld: row.get(7)?,
        digest_version: row.get(8)?,
        delivered_at: optional_timestamp_from_row(row, 9)?,
        acknowledged_at: optional_timestamp_from_row(row, 10)?,
    })
}

fn initiation_invitation_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<InitiationInvitationRecord> {
    Ok(InitiationInvitationRecord {
        invitation_id: row.get(0)?,
        offered_at: timestamp_from_row(row, 1)?,
        local_date: row.get(2)?,
        action_id: row.get(3)?,
        policy_version: row.get(4)?,
        backoff_policy_version: row.get(5)?,
        outcome: InitiationInvitationOutcome::from_db_value(&row.get::<_, String>(6)?)
            .ok_or_else(invalid_enum)?,
        outcome_at: row
            .get::<_, Option<i64>>(7)?
            .map(|value| timestamp_to_datetime(value, 7))
            .transpose()?,
    })
}

fn focus_transition_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FocusTransition> {
    Ok(FocusTransition {
        active: row.get::<_, i64>(0)? != 0,
        changed_at_bucket: timestamp_from_row(row, 1)?,
        local_hour: row.get(2)?,
        local_date: row.get(3)?,
        recorded_at: timestamp_from_row(row, 4)?,
    })
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

/// Serializes declared document types for storage.
///
/// One line of space-separated identifiers, exactly as migration 0033
/// documents. A Uniform Type Identifier is reverse-DNS -- letters, digits, dots
/// and hyphens -- and cannot contain a space, so the delimiter is unambiguous
/// and a single type can be matched with
/// `instr(' ' || document_type_ids || ' ', ' public.source-code ')`.
///
/// The client has already deduplicated and sorted the list; nothing is re-sorted
/// here, because the stored string should be the declaration that arrived rather
/// than a tidied version of it.
///
/// An empty list stores NULL, not an empty string: "declared nothing" and
/// "declared, and it was empty" are the same fact and must have one
/// representation.
fn encode_document_type_ids(document_type_ids: &[String]) -> Option<String> {
    (!document_type_ids.is_empty()).then(|| document_type_ids.join(" "))
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
        // Not selected by the upload-queue reads this mapper serves: the app
        // identity is used only to generalize a correction, never uploaded.
        app_stable_id: None,
        app_scope_eligible: true,
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
        origin: parse_work_block_origin(&row.get::<_, String>(12)?)?,
        intention_expires_at: timestamp_from_row(row, 13)?,
        updated_at: timestamp_from_row(row, 14)?,
    })
}

fn parse_work_block_origin(value: &str) -> rusqlite::Result<WorkBlockOrigin> {
    WorkBlockOrigin::from_db_value(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            12,
            rusqlite::types::Type::Text,
            format!("unknown work_block origin: {value}").into(),
        )
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

/// A stored JSON column that no longer parses. Distinct from `invalid_enum` so
/// a corrupt payload cannot be mistaken for an unrecognised vocabulary token.
fn invalid_stored_json(index: usize) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stored JSON column did not parse",
        )),
    )
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

fn parse_intervention_salience(value: &str) -> rusqlite::Result<InterventionSalience> {
    match value {
        "normal" => Ok(InterventionSalience::Normal),
        "quiet" => Ok(InterventionSalience::Quiet),
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

fn optional_timestamp_from_row(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<DateTime<Utc>>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| timestamp_to_datetime(value, index))
        .transpose()
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
        "abandoned" => Ok(UploadBatchStatus::Abandoned),
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

/// Records one spent attempt and returns the batch to the retry queue under
/// `retry_status`, unless the attempt just spent was its last.
///
/// The ceiling is applied in the same statement as the increment, so there is
/// no window in which a batch past its ceiling is still resumable. Without it a
/// batch retries for as long as the service runs: nothing else in the queue
/// counts attempts, and an unreachable host leaves the device upload-eligible,
/// so the retry never stops on its own.
fn update_batch_retry_state(
    persistence: &SqlitePersistence,
    retry_status: &str,
    batch_id: &str,
    next_attempt_at: i64,
    error_code: &str,
) -> Result<(), PersistenceError> {
    let connection = persistence.connection()?;
    let updated = connection.execute(
        "UPDATE upload_batch
         SET status = CASE WHEN attempt_count + 1 >= ?5 THEN 'abandoned' ELSE ?4 END,
             attempt_count = attempt_count + 1,
             next_attempt_at = ?2,
             last_error_code = ?3
         WHERE batch_id = ?1",
        params![
            batch_id,
            next_attempt_at,
            error_code,
            retry_status,
            UPLOAD_BATCH_ATTEMPT_CEILING
        ],
    )?;
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

/// The durable behavioural substrate: out-of-block runs and the bounded
/// pre-block window.
///
/// Nothing behind this repo can hold a label, a stable id, an application name,
/// a window title, a URL, or intention text — the column list is the guarantee,
/// not a runtime filter.
struct SqliteBehaviorRepo(SqlitePersistence);

impl BehaviorRepo for SqliteBehaviorRepo {
    fn record_out_of_block_run(&self, run: &OutOfBlockRun) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO out_of_block_run(
                started_at_bucket, duration_seconds, category, classification_status,
                classification_confidence, local_hour, local_date
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run.started_at_bucket,
                run.duration_seconds,
                run.category,
                run.classification_status.as_str(),
                run.classification_confidence.as_str(),
                run.local_hour,
                run.local_date,
            ],
        )?;
        Ok(())
    }

    fn out_of_block_runs(&self, since_bucket: i64) -> Result<Vec<OutOfBlockRun>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(
            "SELECT started_at_bucket, duration_seconds, category, classification_status,
                    classification_confidence, local_hour, local_date
             FROM out_of_block_run
             WHERE started_at_bucket >= ?1
             ORDER BY started_at_bucket ASC, id ASC",
        )?;
        let rows = statement.query_map([since_bucket], |row| {
            let status: String = row.get(3)?;
            let confidence: String = row.get(4)?;
            Ok(OutOfBlockRun {
                started_at_bucket: row.get(0)?,
                duration_seconds: row.get(1)?,
                category: row.get(2)?,
                classification_status: parse_classification_status_value(&status)?,
                classification_confidence: parse_classification_confidence_value(&confidence)?,
                local_hour: row.get(5)?,
                local_date: row.get(6)?,
            })
        })?;
        let mut runs = Vec::new();
        for row in rows {
            runs.push(row?);
        }
        Ok(runs)
    }

    fn delete_out_of_block_runs_before(
        &self,
        cutoff_bucket: i64,
        batch_size: usize,
    ) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        let deleted = connection.execute(
            "DELETE FROM out_of_block_run WHERE id IN (
                SELECT id FROM out_of_block_run
                WHERE started_at_bucket < ?1
                ORDER BY started_at_bucket ASC
                LIMIT ?2
             )",
            params![cutoff_bucket, batch_size as i64],
        )?;
        Ok(deleted as u64)
    }

    fn record_block_antecedent(
        &self,
        antecedent: &BlockAntecedent,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        // Recorded once at block start and never updated: a second write is a
        // no-op, so a later evaluation cannot rewrite the window that was
        // actually observed before the block began.
        let mut categories = antecedent.categories.clone();
        categories.sort();
        categories.dedup();
        let encoded = serde_json::to_string(&categories)?;
        connection.execute(
            "INSERT INTO block_antecedent(
                block_id, window_seconds, categories, switch_count, dominant_category,
                dominant_dwell_seconds, day_type, hour_bucket, is_first_block_of_day,
                antecedent_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(block_id) DO NOTHING",
            params![
                antecedent.block_id,
                antecedent.window_seconds,
                encoded,
                antecedent.switch_count,
                antecedent.dominant_category,
                antecedent.dominant_dwell_seconds,
                antecedent.day_type.as_str(),
                antecedent.hour_bucket,
                i64::from(antecedent.is_first_block_of_day),
                antecedent.antecedent_version,
            ],
        )?;
        Ok(())
    }

    fn block_antecedent(
        &self,
        block_id: &str,
    ) -> Result<Option<BlockAntecedent>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT block_id, window_seconds, categories, switch_count, dominant_category,
                        dominant_dwell_seconds, day_type, hour_bucket, is_first_block_of_day,
                        antecedent_version
                 FROM block_antecedent WHERE block_id = ?1",
                [block_id],
                |row| {
                    let encoded: String = row.get(2)?;
                    let day_type: String = row.get(6)?;
                    Ok(BlockAntecedent {
                        block_id: row.get(0)?,
                        window_seconds: row.get(1)?,
                        // A row that cannot be parsed is an error, never an
                        // empty set: an empty antecedent is a real observation
                        // ("nothing preceded this block") and must not be
                        // manufactured by a decoding failure.
                        categories: serde_json::from_str(&encoded)
                            .map_err(|_| invalid_stored_json(2))?,
                        switch_count: row.get(3)?,
                        dominant_category: row.get(4)?,
                        dominant_dwell_seconds: row.get(5)?,
                        day_type: DayType::from_stored(&day_type).ok_or_else(invalid_enum)?,
                        hour_bucket: row.get(7)?,
                        is_first_block_of_day: row.get::<_, i64>(8)? != 0,
                        antecedent_version: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(PersistenceError::from)
    }
}

/// Discovered antecedent patterns (`0029_antecedent_findings.sql`).
///
/// Every honesty rule this table carries lives in the schema: the surfacing
/// trigger, the `surfaced_at`/`confirmed_at` CHECK, and the one-look-per-window
/// unique index. This impl deliberately adds none of its own. A rule duplicated
/// in application code is a rule that can drift from the one the database
/// actually enforces, and only one of the two is authoritative.
struct SqliteAntecedentFindingRepo(SqlitePersistence);

const ANTECEDENT_FINDING_COLUMNS: &str = "finding_id, candidate_id, candidate_registry_version, \
     discovered_at, discovery_window_start, discovery_window_end, support_episodes, effect_size, \
     q_value, confirmed_at, confirm_support_episodes, confirm_effect_size, state, surfaced_at, \
     retracted_at, retraction_reason, user_disputed_at";

fn antecedent_finding_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AntecedentFinding> {
    let state: String = row.get(12)?;
    let reason: Option<String> = row.get(15)?;
    Ok(AntecedentFinding {
        finding_id: row.get(0)?,
        candidate_id: row.get(1)?,
        candidate_registry_version: row.get(2)?,
        discovered_at: row.get(3)?,
        discovery_window_start: row.get(4)?,
        discovery_window_end: row.get(5)?,
        support_episodes: row.get(6)?,
        effect_size: row.get(7)?,
        q_value: row.get(8)?,
        confirmed_at: row.get(9)?,
        confirm_support_episodes: row.get(10)?,
        confirm_effect_size: row.get(11)?,
        state: AntecedentFindingState::from_stored(&state).ok_or_else(invalid_enum)?,
        surfaced_at: row.get(13)?,
        retracted_at: row.get(14)?,
        retraction_reason: match reason {
            None => None,
            Some(value) => {
                Some(AntecedentRetractionReason::from_stored(&value).ok_or_else(invalid_enum)?)
            }
        },
        user_disputed_at: row.get(16)?,
    })
}

impl AntecedentFindingRepo for SqliteAntecedentFindingRepo {
    fn record_antecedent_finding(
        &self,
        finding: &AntecedentFinding,
    ) -> Result<(), PersistenceError> {
        let connection = self.0.connection()?;
        connection.execute(
            "INSERT INTO antecedent_finding(
                finding_id, candidate_id, candidate_registry_version, discovered_at,
                discovery_window_start, discovery_window_end, support_episodes, effect_size,
                q_value, confirmed_at, confirm_support_episodes, confirm_effect_size, state,
                surfaced_at, retracted_at, retraction_reason, user_disputed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                finding.finding_id,
                finding.candidate_id,
                finding.candidate_registry_version,
                finding.discovered_at,
                finding.discovery_window_start,
                finding.discovery_window_end,
                finding.support_episodes,
                finding.effect_size,
                finding.q_value,
                finding.confirmed_at,
                finding.confirm_support_episodes,
                finding.confirm_effect_size,
                finding.state.as_str(),
                finding.surfaced_at,
                finding.retracted_at,
                finding.retraction_reason.map(|reason| reason.as_str()),
                finding.user_disputed_at,
            ],
        )?;
        Ok(())
    }

    fn antecedent_finding(
        &self,
        finding_id: &str,
    ) -> Result<Option<AntecedentFinding>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                &format!(
                    "SELECT {ANTECEDENT_FINDING_COLUMNS} FROM antecedent_finding \
                     WHERE finding_id = ?1"
                ),
                [finding_id],
                antecedent_finding_from_row,
            )
            .optional()
            .map_err(PersistenceError::from)
    }

    fn antecedent_findings_in_state(
        &self,
        state: AntecedentFindingState,
    ) -> Result<Vec<AntecedentFinding>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {ANTECEDENT_FINDING_COLUMNS} FROM antecedent_finding \
             WHERE state = ?1 ORDER BY discovered_at DESC, finding_id ASC"
        ))?;
        let rows = statement.query_map([state.as_str()], antecedent_finding_from_row)?;
        let mut findings = Vec::new();
        for row in rows {
            findings.push(row?);
        }
        Ok(findings)
    }

    fn confirm_antecedent_finding(
        &self,
        finding_id: &str,
        confirmed_at: i64,
        support_episodes: u32,
        effect_size: f64,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // Two statements because SQLite's `BEFORE UPDATE OF <column>` triggers
        // fire per named column, and the held-out check has to see the
        // confirmation timestamp land. Both run inside one implicit
        // transaction per statement; a failure of the second leaves a
        // confirmed-but-still-`candidate` row, which reads as unconfirmed
        // everywhere and can never be surfaced.
        let updated = connection.execute(
            "UPDATE antecedent_finding
                SET confirmed_at = ?2, confirm_support_episodes = ?3, confirm_effect_size = ?4
              WHERE finding_id = ?1",
            params![finding_id, confirmed_at, support_episodes, effect_size],
        )?;
        if updated == 0 {
            return Ok(false);
        }
        connection.execute(
            "UPDATE antecedent_finding SET state = 'confirmed' WHERE finding_id = ?1",
            [finding_id],
        )?;
        Ok(true)
    }

    fn mark_antecedent_finding_surfaced(
        &self,
        finding_id: &str,
        surfaced_at: i64,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        // No `confirmed_at IS NOT NULL` guard in this WHERE clause, on purpose.
        // The database is the thing that must refuse, and a guard here would
        // turn a loud ABORT into a quiet no-op the day someone edits the
        // schema.
        let updated = connection.execute(
            "UPDATE antecedent_finding SET state = 'surfaced', surfaced_at = ?2 \
             WHERE finding_id = ?1",
            params![finding_id, surfaced_at],
        )?;
        Ok(updated > 0)
    }

    fn retract_antecedent_finding(
        &self,
        finding_id: &str,
        retracted_at: i64,
        reason: AntecedentRetractionReason,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let updated = connection.execute(
            "UPDATE antecedent_finding
                SET state = 'retracted', retracted_at = ?2, retraction_reason = ?3
              WHERE finding_id = ?1",
            params![finding_id, retracted_at, reason.as_str()],
        )?;
        Ok(updated > 0)
    }

    fn dispute_antecedent_finding(
        &self,
        finding_id: &str,
        disputed_at: i64,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let updated = connection.execute(
            "UPDATE antecedent_finding
                SET state = 'disputed', user_disputed_at = ?2, retracted_at = ?2,
                    retraction_reason = 'user_disputed'
              WHERE finding_id = ?1",
            params![finding_id, disputed_at],
        )?;
        Ok(updated > 0)
    }

    fn clear_antecedent_findings(&self) -> Result<u64, PersistenceError> {
        let connection = self.0.connection()?;
        Ok(connection.execute("DELETE FROM antecedent_finding", [])? as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::SqlitePersistence;
    use crate::abstraction::EmbeddingSalt;
    use crate::persistence::{
        AbstractionMapping, BlockAntecedent, DayType, GateVerdict, InterventionDecision,
        OutOfBlockRun,
    };
    use chrono::Utc;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};
    use velvt_shared_types::{ClassificationConfidence, ClassificationStatus, CorrectionScope};

    /// The name the runner records for `version`. A fixture that hand-applies
    /// a prefix of the migrations has to record what the runner would have, or
    /// the runner refuses the database as renumbered.
    fn embedded_migration_name(version: i64) -> &'static str {
        super::EMBEDDED_MIGRATIONS
            .iter()
            .find(|migration| migration.version == version)
            .expect("the fixture names an embedded migration")
            .name
    }

    fn schema_migration_rows(database: &SqlitePersistence) -> Vec<(i64, String)> {
        let connection = database.connection().unwrap();
        let mut statement = connection
            .prepare("SELECT version, name FROM schema_migration ORDER BY version")
            .unwrap();
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn every_applied_migration_is_recorded_under_its_file_name() {
        let database = SqlitePersistence::open_in_memory().unwrap();

        let rows = schema_migration_rows(&database);

        assert_eq!(rows.len(), super::EMBEDDED_MIGRATIONS.len());
        for (version, name) in rows {
            assert_eq!(name, embedded_migration_name(version));
            assert!(name.starts_with(&format!("{version:04}_")) && name.ends_with(".sql"));
        }
    }

    #[test]
    fn rerunning_migrations_on_a_database_this_build_migrated_is_a_no_op() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let before = schema_migration_rows(&database);

        database.run_migrations().unwrap();

        assert_eq!(schema_migration_rows(&database), before);
    }

    /// 0010 was allocated twice in July 2026: main's
    /// `0010_personal_override_activity_name.sql` and mvp-enhancements'
    /// `0010_local_only_events.sql`, renumbered to 0012 at the merge. A
    /// database that applied the other 0010 has never run this build's, and
    /// used to be opened as though it had.
    #[test]
    fn a_migration_number_applied_from_another_file_refuses_the_database() {
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
            .execute(
                "INSERT INTO schema_migration(version, name) VALUES (10, '0010_local_only_events.sql')",
                [],
            )
            .unwrap();
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };

        let error = database
            .run_migrations()
            .expect_err("a renumbered migration must not pass as applied");

        let message = error.to_string();
        assert!(
            message.contains("0010_local_only_events.sql")
                && message.contains("0010_personal_override_activity_name.sql"),
            "the refusal names both files: {message}"
        );
        match error {
            super::PersistenceError::MigrationNameMismatch {
                version,
                recorded,
                embedded,
            } => {
                assert_eq!(version, 10);
                assert_eq!(recorded, "0010_local_only_events.sql");
                assert_eq!(embedded, "0010_personal_override_activity_name.sql");
            }
            other => panic!("expected a migration name mismatch, got {other:?}"),
        }
        // Nothing before or after 0010 was applied: the refusal rolled back
        // the whole run, so the database is exactly as it was found.
        assert_eq!(
            schema_migration_rows(&database),
            vec![(10, "0010_local_only_events.sql".to_owned())]
        );
        assert!(!database
            .schema_snapshot()
            .unwrap()
            .iter()
            .any(|name| name == "raw_event_buffer"));
    }

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

    /// Migration 0016 rebuilds `work_block_intervention` to widen a CHECK
    /// constraint, which SQLite cannot alter in place. Alpha installs already
    /// hold answered offers, and losing them would erase the only record of
    /// whether the detector was ever right.
    #[test]
    fn migration_0016_preserves_recorded_offers_and_defaults_them_to_full_salience() {
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
        for (version, sql) in [
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
            (
                11,
                include_str!("../../migrations/0011_local_activity_suggestions.sql"),
            ),
            (
                12,
                include_str!("../../migrations/0012_local_only_events.sql"),
            ),
            (
                13,
                include_str!("../../migrations/0013_personal_semantic_learning.sql"),
            ),
            (
                14,
                include_str!("../../migrations/0014_work_block_intervention.sql"),
            ),
            (
                15,
                include_str!("../../migrations/0015_intervention_outcome_vocabulary.sql"),
            ),
        ] {
            connection.execute_batch(sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    (version, embedded_migration_name(version)),
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at,
                    total_paused_seconds, recovered_after_restart,
                    intention_expires_at, updated_at
                 ) VALUES ('block-1', 'completed', 'medium', 1800, 0, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO work_block_intervention(
                    block_id, offered_at, action_id, anchor_category,
                    switch_count, window_seconds, outcome, outcome_at
                 ) VALUES ('block-1', 100, 'protect_next_10', 'DEEP_WORK', 4, 600, 'dismissed', 120)",
                [],
            )
            .unwrap();
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };

        database.run_migrations().unwrap();

        let recorded = database
            .work_block_repo()
            .intervention("block-1")
            .unwrap()
            .expect("the answered offer must survive the table rebuild");
        assert_eq!(
            recorded.outcome,
            crate::persistence::WorkBlockInterventionOutcome::Dismissed
        );
        assert_eq!(recorded.switch_count, 4);
        assert_eq!(
            recorded.salience,
            velvt_shared_types::InterventionSalience::Normal,
            "offers made before salience existed were all delivered at full salience"
        );

        // The point of the rebuild: the widened vocabulary is now accepted.
        let connection = database.connection().unwrap();
        connection
            .execute(
                "UPDATE work_block_intervention SET outcome = 'was_focused' WHERE block_id = ?1",
                ["block-1"],
            )
            .unwrap();
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
                    (version, embedded_migration_name(version)),
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

        let rekeyed = after_0037(&database, &"a".repeat(64));
        let connection = database.connection().unwrap();
        let alias: String = connection
            .query_row(
                "SELECT activity_name FROM personal_override WHERE key_hash = ?1",
                [rekeyed],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias, "Existing local alias");
        assert!(connection
            .prepare("SELECT local_name_suggestion FROM raw_event_buffer")
            .is_ok());
    }

    // -----------------------------------------------------------------------
    // The durable behavioural substrate (04-DATA-ARCHITECTURE.md §§ 2, 3).
    // -----------------------------------------------------------------------

    /// The schema is the privacy guarantee. A column that could hold an
    /// application name, a label, a stable id, a window title, a URL, or
    /// intention text would make the durable store *more* informative than the
    /// seven-day buffer it is folded from, which is the exact inversion this
    /// design exists to avoid. Asserted against `sqlite_master`, so adding such
    /// a column in a later migration fails here rather than in review.
    #[test]
    fn the_behavioural_tables_have_no_column_that_could_identify_an_application() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let schema = database.schema_sql().unwrap();
        for table in [
            "out_of_block_run",
            "block_antecedent",
            "intervention_decision_log",
        ] {
            let definition = schema
                .iter()
                .find(|sql| sql.contains(&format!("CREATE TABLE {table}")))
                .unwrap_or_else(|| panic!("{table} is missing from the schema"))
                .to_ascii_lowercase();
            for forbidden in [
                "stable_id",
                "label",
                "display_name",
                "local_display_label",
                "local_name_suggestion",
                "window_title",
                "url",
                "intention",
                "bundle",
                "app_name",
            ] {
                assert!(
                    !definition.contains(forbidden),
                    "{table} gained a `{forbidden}` column; the durable store must \
                     stay strictly less informative than the buffer it derives from"
                );
            }
        }
    }

    /// `is_confident_evidence` is `status = classified` AND `confidence IN
    /// (high, medium)` AND the category is not SYSTEM/UNCLASSIFIED/UNLOGGED.
    /// All three inputs must survive into `out_of_block_run`, or the feature
    /// layer would have to invent its own notion of confident evidence — and
    /// the model and the shipped gate would be free to disagree.
    #[test]
    fn an_out_of_block_run_round_trips_every_input_the_confidence_rule_needs() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.behavior_repo();
        let run = OutOfBlockRun {
            started_at_bucket: 1_800_000_300,
            duration_seconds: 420,
            category: "COMMUNICATION".into(),
            classification_status: ClassificationStatus::Classified,
            classification_confidence: ClassificationConfidence::Medium,
            local_hour: 9,
            local_date: "2026-08-21".into(),
        };
        repo.record_out_of_block_run(&run).unwrap();

        let stored = repo.out_of_block_runs(0).unwrap();
        assert_eq!(stored, vec![run]);
        // All three, together, are what makes the predicate reconstructible.
        assert_eq!(
            stored[0].classification_status,
            ClassificationStatus::Classified
        );
        assert_eq!(
            stored[0].classification_confidence,
            ClassificationConfidence::Medium
        );
        assert_eq!(stored[0].category, "COMMUNICATION");
    }

    /// The five-minute grid is a precision class, not a rounding detail:
    /// introducing a finer one is a privacy change. Floors downwards on both
    /// sides of the epoch.
    #[test]
    fn the_run_start_bucket_floors_onto_the_five_minute_grid() {
        use crate::persistence::{out_of_block_run_bucket, OUT_OF_BLOCK_RUN_BUCKET_SECONDS};
        assert_eq!(OUT_OF_BLOCK_RUN_BUCKET_SECONDS, 300);
        let bucket = |seconds: i64| {
            out_of_block_run_bucket(chrono::DateTime::from_timestamp(seconds, 0).unwrap())
        };
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(299), 0);
        assert_eq!(bucket(300), 300);
        assert_eq!(bucket(301), 300);
        assert_eq!(bucket(-1), -300, "pre-epoch instants floor downwards too");
    }

    /// The schema's own bounds. A duration outside 0..1800 or an hour outside
    /// 0..23 cannot be stored, so a bad fold job fails loudly instead of
    /// writing a value the feature layer would silently trust.
    #[test]
    fn out_of_block_run_bounds_are_enforced_by_the_schema() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.behavior_repo();
        let valid = OutOfBlockRun {
            started_at_bucket: 0,
            duration_seconds: 1_800,
            category: "REFERENCE".into(),
            classification_status: ClassificationStatus::Classified,
            classification_confidence: ClassificationConfidence::High,
            local_hour: 23,
            local_date: "2026-08-21".into(),
        };
        repo.record_out_of_block_run(&valid).unwrap();

        let too_long = OutOfBlockRun {
            duration_seconds: 1_801,
            ..valid.clone()
        };
        assert!(repo.record_out_of_block_run(&too_long).is_err());

        let bad_hour = OutOfBlockRun {
            local_hour: 24,
            ..valid.clone()
        };
        assert!(repo.record_out_of_block_run(&bad_hour).is_err());

        let bad_date = OutOfBlockRun {
            local_date: "2026-8-21".into(),
            ..valid
        };
        assert!(repo.record_out_of_block_run(&bad_date).is_err());
    }

    /// The antecedent is recorded once at block start and never updated: a
    /// later evaluation must not be able to rewrite the window that was
    /// actually observed before the block began.
    #[test]
    fn a_block_antecedent_is_written_once_and_never_rewritten() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at,
                    total_paused_seconds, recovered_after_restart,
                    intention_expires_at, updated_at
                 ) VALUES ('antecedent-block', 'active', 'medium', 1800, 0, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let repo = database.behavior_repo();
        let first = BlockAntecedent {
            block_id: "antecedent-block".into(),
            window_seconds: 900,
            // Deliberately unsorted with a duplicate: the set is canonicalised
            // on write, so no ordering information can leak in through the
            // caller's argument order.
            categories: vec![
                "REFERENCE".into(),
                "COMMUNICATION".into(),
                "COMMUNICATION".into(),
            ],
            switch_count: 3,
            dominant_category: Some("COMMUNICATION".into()),
            dominant_dwell_seconds: Some(540),
            day_type: DayType::Weekday,
            hour_bucket: 8,
            is_first_block_of_day: true,
            antecedent_version: 1,
        };
        repo.record_block_antecedent(&first).unwrap();

        let stored = repo
            .block_antecedent("antecedent-block")
            .unwrap()
            .expect("the antecedent was recorded");
        assert_eq!(stored.categories, vec!["COMMUNICATION", "REFERENCE"]);
        assert_eq!(stored.switch_count, 3);
        assert_eq!(stored.day_type, DayType::Weekday);
        assert!(stored.is_first_block_of_day);

        repo.record_block_antecedent(&BlockAntecedent {
            switch_count: 99,
            categories: vec!["SOCIAL_FEED".into()],
            ..first
        })
        .unwrap();
        let reread = repo.block_antecedent("antecedent-block").unwrap().unwrap();
        assert_eq!(
            reread, stored,
            "the antecedent was rewritten after the fact"
        );
    }

    /// The 30-minute ceiling lives in the schema, not in a config file. A user
    /// cannot widen it, and neither can a future constant: widening it requires
    /// a migration, which requires a privacy review.
    #[test]
    fn the_antecedent_window_cannot_exceed_thirty_minutes() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at,
                    total_paused_seconds, recovered_after_restart,
                    intention_expires_at, updated_at
                 ) VALUES ('wide-window', 'active', 'medium', 1800, 0, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let repo = database.behavior_repo();
        let over_the_ceiling = BlockAntecedent {
            block_id: "wide-window".into(),
            window_seconds: 1_801,
            categories: vec!["COMMUNICATION".into()],
            switch_count: 0,
            dominant_category: None,
            dominant_dwell_seconds: None,
            day_type: DayType::Weekend,
            hour_bucket: 0,
            is_first_block_of_day: false,
            antecedent_version: 1,
        };
        assert!(repo.record_block_antecedent(&over_the_ceiling).is_err());

        let at_the_ceiling = BlockAntecedent {
            window_seconds: 1_800,
            ..over_the_ceiling
        };
        repo.record_block_antecedent(&at_the_ceiling).unwrap();
    }

    /// A decision belongs to its block. When the block goes, so does the
    /// decision — otherwise a cleared database would still hold the behavioural
    /// evidence the user believes they deleted.
    #[test]
    fn decisions_cascade_with_the_block_they_were_made_about() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at,
                    total_paused_seconds, recovered_after_restart,
                    intention_expires_at, updated_at
                 ) VALUES ('cascading-block', 'completed', 'medium', 1800, 0, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let repo = database.work_block_repo();
        repo.record_decision(&InterventionDecision {
            decision_id: "cascade-1".into(),
            occurred_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            block_id: Some("cascading-block".into()),
            policy_version: 1,
            anchor_category: None,
            switch_count: 0,
            elapsed_seconds: 10,
            remaining_seconds: 1_790,
            gate_verdict: GateVerdict::AbstainedWarmup,
            propensity: 1.0,
            anchor_seen_within_600s: None,
            outcome_at: None,
        })
        .unwrap();
        assert_eq!(repo.decisions("cascading-block").unwrap().len(), 1);

        repo.clear_all().unwrap();
        assert!(repo.recent_decisions(16).unwrap().is_empty());
    }

    /// The propensity column is the irreversible item: it cannot be
    /// retrofitted, and a value outside (0, 1] is not a probability. The schema
    /// refuses it rather than letting an off-policy estimator divide by it.
    #[test]
    fn propensity_must_be_a_probability() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at,
                    total_paused_seconds, recovered_after_restart,
                    intention_expires_at, updated_at
                 ) VALUES ('propensity-block', 'active', 'medium', 1800, 0, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let repo = database.work_block_repo();
        let decision = |id: &str, propensity: f64| InterventionDecision {
            decision_id: id.into(),
            occurred_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            block_id: Some("propensity-block".into()),
            policy_version: 1,
            anchor_category: None,
            switch_count: 0,
            elapsed_seconds: 0,
            remaining_seconds: 1_800,
            gate_verdict: GateVerdict::AbstainedWarmup,
            propensity,
            anchor_seen_within_600s: None,
            outcome_at: None,
        };
        assert!(repo.record_decision(&decision("zero", 0.0)).is_err());
        assert!(repo.record_decision(&decision("over", 1.5)).is_err());
        repo.record_decision(&decision("deterministic", 1.0))
            .unwrap();
        repo.record_decision(&decision("randomized", 0.5)).unwrap();
        assert_eq!(repo.decisions("propensity-block").unwrap().len(), 2);
    }

    /// Migration 0030 widens a CHECK, which SQLite can only do by rebuilding.
    /// `upload_batch` is the parent of `batch_event` under `ON DELETE CASCADE`,
    /// and with foreign keys enabled a DROP of the parent performs an implicit
    /// DELETE — so a rebuild in the wrong order silently takes every queued
    /// event with it. The from-scratch path cannot catch that: the tables are
    /// empty when the migration runs. This is the upgrade path a shipped device
    /// takes, with rows on disk.
    #[test]
    fn migration_0030_rebuilds_the_parent_without_cascading_queued_events() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
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
        for migration in super::EMBEDDED_MIGRATIONS
            .iter()
            .filter(|migration| migration.version < 30)
        {
            connection.execute_batch(migration.sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    rusqlite::params![migration.version, migration.name],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO upload_batch(batch_id, status, attempt_count, last_error_code)
                 VALUES ('stranded', 'failed', 7, 'transport')",
                [],
            )
            .unwrap();
        for index in 0..3 {
            connection
                .execute(
                    "INSERT INTO batch_event(
                        batch_id, event_id, stable_id, label, category, taxonomy_version, occurred_at
                     ) VALUES ('stranded', ?1, 'abs_1', 'document:edit', 'FOCUS_WORK', 'mvp-1', 0)",
                    rusqlite::params![format!("evt-{index}")],
                )
                .unwrap();
        }

        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.run_migrations().unwrap();

        let connection = database.connection().unwrap();
        let events: i64 = connection
            .query_row("SELECT COUNT(*) FROM batch_event", [], |row| row.get(0))
            .unwrap();
        assert_eq!(events, 3, "the queued events must survive the rebuild");
        let (status, attempts): (String, i64) = connection
            .query_row(
                "SELECT status, attempt_count FROM upload_batch WHERE batch_id = 'stranded'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((status.as_str(), attempts), ("failed", 7));
        connection
            .execute(
                "UPDATE upload_batch SET status = 'abandoned' WHERE batch_id = 'stranded'",
                [],
            )
            .expect("the widened CHECK accepts the terminal status");
        assert!(
            connection
                .execute(
                    "UPDATE upload_batch SET status = 'discarded' WHERE batch_id = 'stranded'",
                    [],
                )
                .is_err(),
            "the vocabulary is still closed"
        );
    }

    /// Migration 0031 mints the salt, so a migrated database reads and never
    /// writes. Two reads must agree: a salt that moved between calls would put
    /// vectors from two spaces into one cache, and every similarity computed
    /// across them would be a number about nothing.
    #[test]
    fn embedding_salt_is_read_back_unchanged_and_is_not_the_zero_salt() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.abstraction_map_repo();

        let first = repo.embedding_salt().unwrap();
        let second = repo.embedding_salt().unwrap();

        assert_eq!(first, second, "the salt must be stable across reads");
        assert_ne!(
            first,
            EmbeddingSalt::UNSALTED,
            "0031 mints a random salt; the zero salt protects nothing"
        );
    }

    /// The create path. It exists so that a startup which cannot read a salt is
    /// never answered with `EmbeddingSalt::UNSALTED`, and it has to empty both
    /// vector stores for the reason 0031 does: a fresh salt is a fresh vector
    /// space, and a sketch computed under the old key is not comparable with
    /// one computed under the new key.
    #[test]
    fn a_minted_embedding_salt_empties_the_stores_that_hold_vectors() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        connection
            .execute_batch(
                "DELETE FROM embedding_salt;
                 INSERT INTO semantic_embedding_cache(key_hash, embedding, dimensions)
                     VALUES (hex(randomblob(32)), x'0001', 256);
                 INSERT INTO personal_semantic_prototype(key_hash, category, embedding, dimensions)
                     VALUES (hex(randomblob(32)), 'FOCUS_WORK', x'0001', 256);",
            )
            .unwrap();
        drop(connection);

        let repo = database.abstraction_map_repo();
        let minted = repo.embedding_salt().unwrap();
        assert_ne!(minted, EmbeddingSalt::UNSALTED);
        assert_eq!(
            minted,
            repo.embedding_salt().unwrap(),
            "the minted salt is persisted, not regenerated per call"
        );

        let connection = database.connection().unwrap();
        let cached: i64 = connection
            .query_row("SELECT COUNT(*) FROM semantic_embedding_cache", [], |row| {
                row.get(0)
            })
            .unwrap();
        let prototypes: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM personal_semantic_prototype",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            (cached, prototypes),
            (0, 0),
            "vectors from the previous space must not survive a new salt"
        );
    }

    /// Migration 0031's header states in the present tense that the salt this
    /// table holds is the key the shipped classifier computes under. Nothing in
    /// the test suite runs `main`, so that sentence is pinned against the
    /// startup source itself — the same technique `tests/published_claims.rs`
    /// uses on PRIVACY.md, for the same reason. 0031 shipped once with the
    /// header written and the wiring absent; this is what makes that state red
    /// instead of quiet.
    #[test]
    fn startup_builds_the_builtin_classifier_under_the_device_salt() {
        const STARTUP: &str = include_str!("../main.rs");
        assert!(
            STARTUP.contains(".embedding_salt()"),
            "startup no longer reads the device salt, so migration 0031 mints a key \
             nothing uses and pays a cache wipe for it"
        );
        assert!(
            STARTUP.contains("EmbeddingSimilarityPlugin::builtin_salted("),
            "startup no longer builds the built-in classifier under the device salt"
        );
        assert!(
            !STARTUP.contains("EmbeddingSimilarityPlugin::builtin("),
            "startup builds the built-in classifier on `EmbeddingSalt::UNSALTED`, which \
             every reader of `plugin.rs` knows. Sketches cached under it are recoverable \
             offline from published source, which is the exposure 0031 says it closed"
        );
    }

    /// The ceiling is the whole point of migration 0030, and nothing exercised
    /// it. `update_batch_retry_state` applies it in the same statement as the
    /// increment, so the boundary is where a mistake would live: an off-by-one
    /// either abandons a batch one attempt early or lets it retry forever.
    ///
    /// `>= UPLOAD_BATCH_ATTEMPT_CEILING` is checked against `attempt_count + 1`,
    /// so the batch is abandoned by the attempt that reaches the ceiling, not
    /// the one after it. Abandonment is terminal: `resumable_batches` must stop
    /// returning the row in the same transition that writes the status, or the
    /// retry loop keeps picking up a batch nothing will ever send.
    #[test]
    fn the_attempt_ceiling_abandons_a_batch_exactly_once_it_is_reached() {
        use crate::persistence::UploadBatchStatus;
        use chrono::TimeZone;

        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.upload_batch_repo();
        let past = chrono::Utc.timestamp_opt(1, 0).unwrap();

        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO upload_batch(batch_id, status, attempt_count, next_attempt_at)
                 VALUES ('ceiling', 'failed', ?1, 0)",
                rusqlite::params![super::UPLOAD_BATCH_ATTEMPT_CEILING - 2],
            )
            .unwrap();

        // One below the ceiling: still owed to the backend, still resumable.
        repo.mark_failed("ceiling", past, "transport").unwrap();
        let (status, attempts) = batch_row(&database, "ceiling");
        assert_eq!(status, "failed");
        assert_eq!(attempts, i64::from(super::UPLOAD_BATCH_ATTEMPT_CEILING) - 1);
        assert!(
            repo.resumable_batches(chrono::Utc::now())
                .unwrap()
                .iter()
                .any(|batch| batch.batch_id == "ceiling"),
            "a batch below the ceiling must still be retried"
        );

        // The attempt that reaches the ceiling is the one that abandons it.
        repo.mark_failed("ceiling", past, "transport").unwrap();
        let (status, attempts) = batch_row(&database, "ceiling");
        assert_eq!(status, "abandoned");
        assert_eq!(attempts, i64::from(super::UPLOAD_BATCH_ATTEMPT_CEILING));
        assert!(
            !repo
                .resumable_batches(chrono::Utc::now())
                .unwrap()
                .iter()
                .any(|batch| batch.batch_id == "ceiling"),
            "an abandoned batch is terminal and must never be resumed"
        );

        // The status round-trips through the parser rather than only the CHECK.
        assert_eq!(
            super::upload_status_from_str("abandoned").unwrap(),
            UploadBatchStatus::Abandoned
        );
    }

    fn batch_row(database: &SqlitePersistence, batch_id: &str) -> (String, i64) {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT status, attempt_count FROM upload_batch WHERE batch_id = ?1",
                [batch_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    // ---------------------------------------------------------------------
    // Triage: which applications Velvt could not read
    // ---------------------------------------------------------------------

    /// `key(n)` builds a distinct 64-character hex key, which is what every
    /// identity column CHECKs for.
    fn key(seed: u8) -> String {
        format!("{seed:02x}").repeat(32)
    }

    fn unlogged_event(
        event_id: &str,
        app_key: &str,
        local_name: Option<&str>,
        occurred_at: chrono::DateTime<Utc>,
        duration_seconds: u64,
    ) -> crate::persistence::RawEventEntry {
        crate::persistence::RawEventEntry {
            event_id: event_id.into(),
            stable_id: format!("abs_{event_id}"),
            label: "unlogged".into(),
            local_display_label: None,
            // The raw application name, which migration 0001 documents this
            // column as holding for exactly the events that matched no seed and
            // no correction — every event in a triage list.
            local_name_suggestion: local_name.map(str::to_owned),
            category: "UNLOGGED".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "fallback".into(),
            classification_status: "unclassified".into(),
            classification_confidence: "none".into(),
            classification_source: "fallback".into(),
            occurred_at,
            duration_seconds,
            upload_eligible: false,
            app_stable_id: Some(app_key.to_owned()),
            app_scope_eligible: true,
        }
    }

    /// Five minutes is the floor, and it is inclusive: an application observed
    /// for exactly five minutes is on the list, one observed for a second less
    /// is not. A list of one-second curiosities is not a task anyone will do.
    #[test]
    fn the_triage_floor_is_five_minutes_and_includes_the_boundary() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        // Summed across two events, so the floor is tested against observed
        // time rather than a single dwell.
        events
            .insert(&unlogged_event(
                "at-floor-a",
                &key(1),
                Some("Figma"),
                now,
                200,
            ))
            .unwrap();
        events
            .insert(&unlogged_event(
                "at-floor-b",
                &key(1),
                Some("Figma"),
                now,
                100,
            ))
            .unwrap();
        events
            .insert(&unlogged_event(
                "under-floor",
                &key(2),
                Some("Calculator"),
                now,
                299,
            ))
            .unwrap();

        let entries = events.unclassified_triage(14, 300, 8).unwrap();

        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].app_stable_id, key(1));
        assert_eq!(entries[0].display_name, "Figma");
        assert_eq!(entries[0].seconds_observed, 300);
        assert_eq!(entries[0].event_count, 2);
    }

    /// The floor cannot be lowered by a caller. It is the difference between a
    /// list of things to do and an inventory of everything installed.
    #[test]
    fn a_caller_cannot_ask_for_a_lower_floor_than_the_published_one() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        events
            .insert(&unlogged_event(
                "brief",
                &key(3),
                Some("Calculator"),
                Utc::now(),
                30,
            ))
            .unwrap();

        assert!(events.unclassified_triage(14, 0, 8).unwrap().is_empty());
    }

    /// The window is the published retention window. An event just inside it
    /// counts; one just outside does not, and asking for a longer window cannot
    /// reach evidence that no longer exists.
    #[test]
    fn the_triage_window_is_bounded_by_the_retention_window() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        events
            .insert(&unlogged_event(
                "inside",
                &key(4),
                Some("Obsidian"),
                now - chrono::Duration::days(13),
                600,
            ))
            .unwrap();
        events
            .insert(&unlogged_event(
                "outside",
                &key(5),
                Some("Sketch"),
                now - chrono::Duration::days(15),
                600,
            ))
            .unwrap();

        let fortnight = events.unclassified_triage(14, 300, 8).unwrap();
        assert_eq!(fortnight.len(), 1, "{fortnight:?}");
        assert_eq!(fortnight[0].display_name, "Obsidian");

        // Clamped, not honoured: a 90-day request returns the same fortnight.
        let asked_for_more = events.unclassified_triage(90, 300, 8).unwrap();
        assert_eq!(asked_for_more.len(), 1);

        // And a narrower request is still honoured.
        assert!(events.unclassified_triage(7, 300, 8).unwrap().is_empty());
    }

    /// Ranked by observed time, longest first, and capped at eight however many
    /// qualify.
    #[test]
    fn the_triage_list_is_ranked_by_time_and_capped_at_eight() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        for index in 0..10_u8 {
            events
                .insert(&unlogged_event(
                    &format!("ranked-{index}"),
                    &key(index + 10),
                    Some(&format!("App {index}")),
                    now,
                    300 + u64::from(index) * 60,
                ))
                .unwrap();
        }

        let entries = events.unclassified_triage(14, 300, 8).unwrap();

        assert_eq!(entries.len(), 8);
        assert_eq!(entries[0].display_name, "App 9");
        assert!(entries
            .windows(2)
            .all(|pair| pair[0].seconds_observed >= pair[1].seconds_observed));
        // Asking for more than the cap does not widen it.
        assert_eq!(events.unclassified_triage(14, 300, 50).unwrap().len(), 8);
    }

    /// UNLOGGED only. That is the state `is_confident_evidence` excludes, so it
    /// is the time that reaches neither the drift gate nor the anchor — which is
    /// what makes it worth a user's attention. A classified application is not
    /// unreadable and must never appear.
    #[test]
    fn only_unlogged_time_reaches_the_triage_list() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        let mut classified = unlogged_event("classified", &key(6), Some("Slack"), now, 3_600);
        classified.category = "COMMUNICATION".into();
        classified.classification_status = "classified".into();
        events.insert(&classified).unwrap();
        // An event with no app identity at all cannot be generalized to an app,
        // so there is nothing to teach and nothing to show.
        let mut anonymous = unlogged_event("anonymous", &key(7), Some("Ghost"), now, 3_600);
        anonymous.app_stable_id = None;
        events.insert(&anonymous).unwrap();

        assert!(events.unclassified_triage(14, 300, 8).unwrap().is_empty());
    }

    /// An hour Velvt holds no name for is still an hour the user spent, so the
    /// row is named plainly instead of dropped. Omitting it hid real time from
    /// the one list whose entire claim is that it shows the time Velvt could not
    /// read -- and the user can usually still answer, because the row carries the
    /// time observed and they know what they had open for an hour.
    #[test]
    fn an_application_velvt_holds_no_name_for_is_named_plainly() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        events
            .insert(&unlogged_event("nameless", &key(8), None, now, 3_600))
            .unwrap();

        let entries = events.unclassified_triage(14, 300, 8).unwrap();

        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].app_stable_id, key(8));
        assert_eq!(entries[0].display_name, "Unnamed application");
        assert_eq!(entries[0].seconds_observed, 3_600);
        // The floor still applies to it: unnamed does not mean exempt.
        assert!(events.unclassified_triage(14, 7_200, 8).unwrap().is_empty());
    }

    /// Teaching an application has to remove it from the list. Nothing rewrites
    /// the past events, so without this the application the user just explained
    /// would be back at the top of the list tomorrow — and the one action the
    /// surface offers would look like it did nothing.
    #[test]
    fn an_application_the_user_has_already_taught_leaves_the_list() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        let now = Utc::now();
        events
            .insert(&unlogged_event(
                "taught",
                &key(9),
                Some("Linear"),
                now,
                3_600,
            ))
            .unwrap();
        assert_eq!(events.unclassified_triage(14, 300, 8).unwrap().len(), 1);

        maps.save_app_scope_override(&key(9), None, "TASK_MANAGEMENT", Some("Tickets"))
            .unwrap();

        assert!(events.unclassified_triage(14, 300, 8).unwrap().is_empty());
    }

    /// The same, when the rule was taught under the bundle identity: the name
    /// key may differ (a rename, a localized name) while the application is the
    /// same one, and it must still drop off the list.
    #[test]
    fn a_rule_taught_under_the_bundle_identity_also_clears_the_list() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        let metadata = crate::persistence::DeclaredAppMetadata {
            app_bundle_stable_id: Some(key(0x2a)),
            declared_app_category: None,
            document_type_ids: Vec::new(),
        };
        events
            .insert_with_declared_metadata(
                &unlogged_event("bundled", &key(0x1a), Some("Code"), Utc::now(), 3_600),
                &metadata,
            )
            .unwrap();

        let entries = events.unclassified_triage(14, 300, 8).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].app_bundle_stable_id.as_deref(),
            Some(key(0x2a).as_str()),
            "the bundle identity has to reach the surface that writes the rule"
        );

        // A rule under a different NAME key but the same bundle key.
        maps.save_app_scope_override(&key(0x3a), Some(&key(0x2a)), "FOCUS_WORK", None)
            .unwrap();

        assert!(events.unclassified_triage(14, 300, 8).unwrap().is_empty());
    }

    // ---------------------------------------------------------------------
    // Declared metadata and the app rung
    // ---------------------------------------------------------------------

    /// An event with no declared metadata is written exactly as it was before
    /// the columns existed. Absent metadata must degrade to today's behaviour,
    /// and the first requirement of that is that absence stays NULL rather than
    /// becoming an empty declaration.
    #[test]
    fn an_event_without_declared_metadata_stores_nulls() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        database
            .raw_event_repo()
            .insert(&unlogged_event(
                "plain",
                &key(0x4a),
                Some("App"),
                Utc::now(),
                60,
            ))
            .unwrap();

        let stored: (Option<String>, Option<String>, Option<String>) = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT app_bundle_stable_id, declared_app_category, document_type_ids
                 FROM raw_event_buffer WHERE event_id = 'plain'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(stored, (None, None, None));
    }

    /// The serialisation migration 0033 documents: one line of space-separated
    /// identifiers, matchable with a padded `instr`.
    #[test]
    fn declared_document_types_store_as_one_space_separated_line() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let metadata = crate::persistence::DeclaredAppMetadata {
            app_bundle_stable_id: Some(key(0x5a)),
            declared_app_category: Some("public.app-category.developer-tools".into()),
            document_type_ids: vec!["public.plain-text".into(), "public.source-code".into()],
        };
        database
            .raw_event_repo()
            .insert_with_declared_metadata(
                &unlogged_event("declared", &key(0x6a), Some("Code"), Utc::now(), 60),
                &metadata,
            )
            .unwrap();

        let connection = database.connection().unwrap();
        let stored: String = connection
            .query_row(
                "SELECT document_type_ids FROM raw_event_buffer WHERE event_id = 'declared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, "public.plain-text public.source-code");

        // The padded match documented in 0033: one type is found, and a longer
        // identifier that merely starts with it is not.
        let matched: bool = connection
            .query_row(
                "SELECT instr(' ' || document_type_ids || ' ', ' public.source-code ') > 0
                 FROM raw_event_buffer WHERE event_id = 'declared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(matched);
        let over_matched: bool = connection
            .query_row(
                "SELECT instr(' ' || document_type_ids || ' ', ' public.plain ') > 0
                 FROM raw_event_buffer WHERE event_id = 'declared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!over_matched);
    }

    /// One correction writes both identities, and a later correction made from
    /// an event that carried no bundle id must not erase the bundle key the
    /// rule already had.
    #[test]
    fn an_event_sourced_app_rule_records_both_identities() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        events
            .insert_with_declared_metadata(
                &unlogged_event("with-bundle", &key(0x7a), Some("Code"), Utc::now(), 60),
                &crate::persistence::DeclaredAppMetadata {
                    app_bundle_stable_id: Some(key(0x8a)),
                    ..crate::persistence::DeclaredAppMetadata::ABSENT
                },
            )
            .unwrap();

        assert!(maps
            .save_personal_app_override("with-bundle", "FOCUS_WORK", Some("Editing"))
            .unwrap());

        let rule = maps.app_scope_override(&key(0x7a)).unwrap().unwrap();
        assert_eq!(rule.bundle_key_hash.as_deref(), Some(key(0x8a).as_str()));
        assert_eq!(rule.category, "FOCUS_WORK");
        // The same rule is reachable from either identity, which is what makes
        // the bundle rung a rung rather than a second table.
        assert_eq!(
            maps.bundle_app_override(&key(0x8a))
                .unwrap()
                .unwrap()
                .app_key_hash,
            key(0x7a)
        );

        // A second correction from an event with no bundle id at all.
        let mut later = unlogged_event("no-bundle", &key(0x7a), Some("Code"), Utc::now(), 60);
        later.event_id = "no-bundle".into();
        events.insert(&later).unwrap();
        assert!(maps
            .save_personal_app_override("no-bundle", "REFERENCE", None)
            .unwrap());

        let rule = maps.app_scope_override(&key(0x7a)).unwrap().unwrap();
        assert_eq!(rule.category, "REFERENCE");
        assert_eq!(
            rule.bundle_key_hash.as_deref(),
            Some(key(0x8a).as_str()),
            "a correction made without a bundle id must not erase the one the rule had"
        );
        assert_eq!(rule.activity_name.as_deref(), Some("Editing"));
        assert_eq!(rule.correction_count, 2);
    }

    /// Teaching the same application twice is teaching it once. The count still
    /// advances, because how often someone had to repeat themselves is the
    /// signal that something upstream is wrong.
    #[test]
    fn teaching_an_application_is_idempotent() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();

        maps.save_app_scope_override(&key(0x9a), Some(&key(0xaa)), "FOCUS_WORK", Some("Writing"))
            .unwrap();
        maps.save_app_scope_override(&key(0x9a), Some(&key(0xaa)), "FOCUS_WORK", Some("Writing"))
            .unwrap();

        let rule = maps.app_scope_override(&key(0x9a)).unwrap().unwrap();
        assert_eq!(rule.category, "FOCUS_WORK");
        assert_eq!(rule.correction_count, 2);
        assert_eq!(
            database
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM personal_app_override", [], |row| row
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
            1
        );
    }

    /// The history has to show app rules, marked as app rules: until it did, a
    /// user could neither see nor undo what they had taught at app scope, and
    /// removing the window rule left the engine falling through into the
    /// surviving app rule and answering exactly as before.
    ///
    /// Two separate things the user said here -- a window correction, and an
    /// application taught through triage -- so two rows. The two rungs of ONE
    /// correction are a different case and are one row:
    /// `a_correction_that_wrote_both_rungs_is_one_rule_in_the_history`.
    #[test]
    fn the_correction_history_shows_both_rungs_with_their_scope() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        events
            .insert(&unlogged_event(
                "shown",
                &key(0xba),
                Some("Figma"),
                Utc::now(),
                600,
            ))
            .unwrap();
        maps.upsert(&AbstractionMapping {
            key_hash: key(0xca),
            stable_id: "abs_window".into(),
            label: "reference:inferred".into(),
            category: "REFERENCE".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            classification_status: "classified".into(),
            classification_confidence: "high".into(),
            classification_source: "user_rule".into(),
            display_name: None,
        })
        .unwrap();
        maps.save_personal_override("abs_window", "REFERENCE", Some("Reading"))
            .unwrap();
        maps.save_app_scope_override(&key(0xba), None, "FOCUS_WORK", Some("Design work"))
            .unwrap();

        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();

        assert_eq!(total, 2);
        let app_rule = rules
            .iter()
            .find(|rule| rule.scope == CorrectionScope::App)
            .expect("the app rung must be listed");
        assert_eq!(
            app_rule.stable_id,
            key(0xba),
            "an app rule is addressed by its application key, which is what Remove needs"
        );
        assert_eq!(app_rule.local_activity_name.as_deref(), Some("Design work"));
        assert!(rules
            .iter()
            .any(|rule| rule.scope == CorrectionScope::Window && rule.stable_id == "abs_window"));
        // And it is searchable by the name the user typed, like any other rule.
        let (matched, count) = maps
            .search_personal_overrides(Some("design work"), 0, 20)
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(matched[0].scope, CorrectionScope::App);
    }

    /// Removing an app rule removes the typed name with it. `display_name`
    /// records no provenance and its upsert coalesces, so a name the user typed
    /// would otherwise survive its own undo.
    #[test]
    fn removing_an_app_rule_also_clears_the_mirrored_typed_name() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let mut event = unlogged_event("mirrored", &key(0xda), Some("Figma"), Utc::now(), 600);
        event.stable_id = "abs_mirrored".into();
        events.insert(&event).unwrap();
        maps.upsert(&AbstractionMapping {
            key_hash: key(0xea),
            stable_id: "abs_mirrored".into(),
            label: "document:inferred".into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            classification_status: "classified".into(),
            classification_confidence: "high".into(),
            classification_source: "user_rule".into(),
            display_name: Some("Design work".into()),
        })
        .unwrap();
        maps.save_app_scope_override(&key(0xda), None, "FOCUS_WORK", Some("Design work"))
            .unwrap();

        assert!(maps.remove_app_scope_override(&key(0xda)).unwrap());

        assert!(maps.app_scope_override(&key(0xda)).unwrap().is_none());
        assert_eq!(maps.get("abs_mirrored").unwrap().display_name, None);
        // A second removal is a no-op rather than an error: the rule is gone,
        // which is what the caller asked for.
        assert!(!maps.remove_app_scope_override(&key(0xda)).unwrap());
    }

    /// Editing a saved rule generalizes to the application, the way correcting
    /// an event does. Without it the app rung kept the previous category for
    /// every other window of that application: the window the user was looking
    /// at changed and nothing else did.
    #[test]
    fn editing_a_saved_rule_can_generalize_from_the_stable_id_alone() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let mut event = unlogged_event("edited", &key(0xfa), Some("Cursor"), Utc::now(), 600);
        event.stable_id = "abs_edited".into();
        events
            .insert_with_declared_metadata(
                &event,
                &crate::persistence::DeclaredAppMetadata {
                    app_bundle_stable_id: Some(key(0x11)),
                    ..crate::persistence::DeclaredAppMetadata::ABSENT
                },
            )
            .unwrap();

        assert!(maps
            .save_personal_app_override_by_stable_id("abs_edited", "FOCUS_WORK", Some("Editing"))
            .unwrap());

        let rule = maps.app_scope_override(&key(0xfa)).unwrap().unwrap();
        assert_eq!(rule.category, "FOCUS_WORK");
        assert_eq!(rule.bundle_key_hash.as_deref(), Some(key(0x11).as_str()));

        // A browser window carrying a site context is not generalizable: one
        // tab says nothing about the next.
        let mut browser = unlogged_event("tab", &key(0x12), Some("Chrome"), Utc::now(), 600);
        browser.stable_id = "abs_tab".into();
        browser.app_scope_eligible = false;
        events.insert(&browser).unwrap();
        assert!(!maps
            .save_personal_app_override_by_stable_id("abs_tab", "REFERENCE", None)
            .unwrap());
        assert!(maps.app_scope_override(&key(0x12)).unwrap().is_none());
    }

    /// The engine consults the bundle rung with the same read it uses for the
    /// name rung, which is only sound because the two keys live in different
    /// hash domains. This is that read, answering for both.
    #[test]
    fn the_app_rung_answers_for_either_identity() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        database
            .abstraction_map_repo()
            .save_app_scope_override(&key(0x13), Some(&key(0x14)), "FOCUS_WORK", Some("Editing"))
            .unwrap();
        let store = database.abstraction_mapping_store();

        for identity in [key(0x13), key(0x14)] {
            let found = store.personal_app_override(&identity).unwrap().unwrap();
            assert_eq!(found.category, "FOCUS_WORK");
            assert_eq!(found.local_activity_name.as_deref(), Some("Editing"));
        }
        assert!(store.personal_app_override(&key(0x15)).unwrap().is_none());
    }

    /// Counts the rows of the app rung, which is where "one application, one
    /// rule" either holds or does not.
    fn app_rule_count(database: &SqlitePersistence) -> u64 {
        database
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM personal_app_override", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// The rename bundle keying exists to solve: the same application, the same
    /// bundle identifier, a new name. The rule has to follow it.
    ///
    /// The write used to abort here with SQLITE_CONSTRAINT: the upsert conflicts
    /// on `app_key_hash`, the new name is not in conflict, and the UNIQUE partial
    /// index on `bundle_key_hash` (0034) is a second constraint no upsert can
    /// target. Converging instead of erroring is the whole feature.
    #[test]
    fn renaming_an_application_keeps_the_rule_it_was_taught() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        let store = database.abstraction_mapping_store();
        let bundle = key(0x16);
        let old_name = key(0x17);
        let new_name = key(0x18);
        let bundled = |bundle: &str| crate::persistence::DeclaredAppMetadata {
            app_bundle_stable_id: Some(bundle.to_owned()),
            ..crate::persistence::DeclaredAppMetadata::ABSENT
        };

        // Taught under the name macOS reported at the time.
        events
            .insert_with_declared_metadata(
                &unlogged_event("before-rename", &old_name, Some("Code"), Utc::now(), 600),
                &bundled(&bundle),
            )
            .unwrap();
        assert!(maps
            .save_personal_app_override("before-rename", "FOCUS_WORK", Some("Editing"))
            .unwrap());

        // The next release reports a different name for the same bundle.
        events
            .insert_with_declared_metadata(
                &unlogged_event(
                    "after-rename",
                    &new_name,
                    Some("Visual Studio Code"),
                    Utc::now(),
                    600,
                ),
                &bundled(&bundle),
            )
            .unwrap();
        assert!(
            maps.save_personal_app_override("after-rename", "FOCUS_WORK", None)
                .unwrap(),
            "the app-scope write must converge on the bundle, not abort on its UNIQUE index"
        );

        // One application, one rule, now answering to the name it reports today.
        assert_eq!(app_rule_count(&database), 1);
        let rule = maps.bundle_app_override(&bundle).unwrap().unwrap();
        assert_eq!(rule.app_key_hash, new_name);
        assert_eq!(rule.category, "FOCUS_WORK");
        assert_eq!(
            rule.activity_name.as_deref(),
            Some("Editing"),
            "the name the user typed survives the rename"
        );
        assert_eq!(
            rule.correction_count, 2,
            "the re-key keeps the count: how often someone repeated themselves is the signal"
        );
        assert!(maps.app_scope_override(&old_name).unwrap().is_none());

        // And the correction still applies to the renamed application, by either
        // identity the next event will carry.
        for identity in [&new_name, &bundle] {
            let found = store.personal_app_override(identity).unwrap().unwrap();
            assert_eq!(found.category, "FOCUS_WORK");
            assert_eq!(found.local_activity_name.as_deref(), Some("Editing"));
        }
    }

    /// The same rename, when a rule already exists under the new name -- taught
    /// while the client reported no bundle identifier, so nothing tied the two
    /// together. Two rows would then claim one bundle, which the index forbids
    /// and which would make the applied rule depend on scan order. The
    /// application has one identity, so it keeps one rule.
    #[test]
    fn a_rename_into_a_name_already_taught_converges_on_one_rule() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let bundle = key(0x19);
        let old_name = key(0x1a);
        let new_name = key(0x1b);

        maps.save_app_scope_override(&old_name, Some(&bundle), "REFERENCE", None)
            .unwrap();
        maps.save_app_scope_override(&new_name, None, "PASSIVE_CONSUMPTION", None)
            .unwrap();
        assert_eq!(app_rule_count(&database), 2);

        maps.save_app_scope_override(&new_name, Some(&bundle), "FOCUS_WORK", Some("Editing"))
            .unwrap();

        assert_eq!(app_rule_count(&database), 1);
        let rule = maps.app_scope_override(&new_name).unwrap().unwrap();
        assert_eq!(rule.bundle_key_hash.as_deref(), Some(bundle.as_str()));
        assert_eq!(rule.category, "FOCUS_WORK");
        assert_eq!(rule.activity_name.as_deref(), Some("Editing"));
        assert!(maps.app_scope_override(&old_name).unwrap().is_none());
        // Still one rule in the history, not the two it was written from.
        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 1, "{rules:?}");
        assert_eq!(rules[0].scope, CorrectionScope::App);
    }

    /// One correction is one rule in the history.
    ///
    /// `CorrectEventClassification` has written both rungs since 0017, so
    /// listing the app table unfiltered showed every past correction twice for
    /// every existing user. The window rule is the one that is shown, because it
    /// is the one whose removal takes both rungs with it.
    #[test]
    fn a_correction_that_wrote_both_rungs_is_one_rule_in_the_history() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let app = key(0x1c);
        let mut event = unlogged_event("paired", &app, Some("Code"), Utc::now(), 600);
        event.stable_id = "abs_paired".into();
        events.insert(&event).unwrap();
        maps.upsert(&AbstractionMapping {
            key_hash: key(0x1d),
            stable_id: "abs_paired".into(),
            label: "document:inferred".into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            classification_status: "classified".into(),
            classification_confidence: "high".into(),
            classification_source: "user_rule".into(),
            display_name: None,
        })
        .unwrap();

        // Exactly the pair the correction path writes, in the order it writes it.
        maps.save_personal_override("abs_paired", "FOCUS_WORK", Some("Editing"))
            .unwrap();
        assert!(maps
            .save_personal_app_override("paired", "FOCUS_WORK", Some("Editing"))
            .unwrap());

        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 1, "one action reads as one rule: {rules:?}");
        assert_eq!(rules[0].scope, CorrectionScope::Window);
        assert_eq!(rules[0].stable_id, "abs_paired");

        // Hidden from the list is not absent from the engine: the app rung is
        // still there, still answering for every other window of that
        // application.
        assert_eq!(
            maps.app_scope_override(&app).unwrap().unwrap().category,
            "FOCUS_WORK"
        );
        // And removing the row the user can see removes both rungs, which is why
        // this is the row that is shown.
        assert!(maps.remove_personal_override("abs_paired").unwrap());
        assert!(maps.app_scope_override(&app).unwrap().is_none());
    }

    /// A rule taught through triage has no window rung behind it, so it is a
    /// rule of its own and stays visible -- including after a later window
    /// correction touches the same row, because those are two separate things
    /// the user said.
    #[test]
    fn a_rule_taught_about_an_application_keeps_its_own_place_in_the_history() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let app = key(0x1e);
        let mut event =
            unlogged_event("taught-then-corrected", &app, Some("Code"), Utc::now(), 600);
        event.stable_id = "abs_taught".into();
        events.insert(&event).unwrap();

        maps.save_app_scope_override(&app, None, "FOCUS_WORK", Some("Editing"))
            .unwrap();
        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 1, "{rules:?}");
        assert_eq!(rules[0].scope, CorrectionScope::App);

        // A later correction of one window of that application writes the app
        // rung again, as the paired rung. The row the user taught must not
        // disappear underneath them.
        assert!(maps
            .save_personal_app_override("taught-then-corrected", "REFERENCE", None)
            .unwrap());
        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 1, "{rules:?}");
        assert_eq!(rules[0].scope, CorrectionScope::App);
        assert_eq!(rules[0].category, "REFERENCE");
    }

    /// The rules already on disk. Before 0035 the only writer of this table was
    /// the paired correction path, so the column's default of 0 is what makes a
    /// correction made last month stop reading as two rules -- and that can only
    /// be checked on the upgrade path, with a row written before the column
    /// existed.
    #[test]
    fn migration_0035_reads_existing_app_rules_as_the_paired_rung() {
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
        for migration in super::EMBEDDED_MIGRATIONS
            .iter()
            .filter(|migration| migration.version < 35)
        {
            connection.execute_batch(migration.sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    rusqlite::params![migration.version, migration.name],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO personal_app_override(app_key_hash, category, activity_name)
                 VALUES (?1, 'FOCUS_WORK', 'Editing')",
                [key(0x1f)],
            )
            .unwrap();

        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.run_migrations().unwrap();

        let rekeyed = after_0037(&database, &key(0x1f));
        let stored: i64 = database
            .connection()
            .unwrap()
            .query_row(
                "SELECT app_only FROM personal_app_override WHERE app_key_hash = ?1",
                [rekeyed],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, 0,
            "a row that predates the column was written paired"
        );
        let (rules, total) = database
            .abstraction_map_repo()
            .search_personal_overrides(None, 0, 20)
            .unwrap();
        assert_eq!(
            total, 0,
            "a pre-0035 app rung is the second rung of a correction, not a rule of its own: {rules:?}"
        );
    }

    // ---------------------------------------------------------------------
    // Removing a correction: both of its rungs, and only its own
    // ---------------------------------------------------------------------

    /// A key written before migration 0037, as it reads after the upgrade:
    /// re-keyed under this database's salt. For the upgrade tests of earlier
    /// migrations, which now run through 0037 on their way to the latest schema.
    fn after_0037(database: &SqlitePersistence, stored: &str) -> String {
        database
            .abstraction_map_repo()
            .stable_key_salt()
            .unwrap()
            .rekey_stored_digest(stored)
            .unwrap()
    }

    /// The mapping a window rule needs in order to exist at all.
    fn window_mapping(stable_id: &str, key_hash: &str) -> AbstractionMapping {
        AbstractionMapping {
            key_hash: key_hash.to_owned(),
            stable_id: stable_id.to_owned(),
            label: "document:inferred".into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            classification_status: "classified".into(),
            classification_confidence: "high".into(),
            classification_source: "user_rule".into(),
            display_name: None,
        }
    }

    /// The app rung recorded on a window rule, which is what makes removal a key
    /// lookup rather than a guess at a fourteen-day event cache.
    fn paired_app_key(database: &SqlitePersistence, key_hash: &str) -> Option<String> {
        database
            .connection()
            .unwrap()
            .query_row(
                "SELECT app_key_hash FROM personal_override WHERE key_hash = ?1",
                [key_hash],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// A rule the user taught about an application from the triage list is a rule
    /// of its own (0035: sticky, never cleared) and must survive the removal of
    /// an unrelated window rule that happens to name the same application.
    ///
    /// The delete carried no `app_only` predicate, so one Remove silently
    /// destroyed something the user had taught somewhere else entirely -- the
    /// 0035 invariant held on the write path and was broken on the delete path.
    #[test]
    fn removing_a_window_rule_keeps_a_rule_taught_about_the_application() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let app = key(0x20);
        let mut event = unlogged_event("triaged-app", &app, Some("Linear"), Utc::now(), 600);
        event.stable_id = "abs_window".into();
        events.insert(&event).unwrap();
        maps.upsert(&window_mapping("abs_window", &key(0x21)))
            .unwrap();

        // Taught from the triage list: there is no window behind this, so the app
        // rung is the whole rule.
        maps.save_app_scope_override(&app, None, "TASK_MANAGEMENT", Some("Tickets"))
            .unwrap();
        // Later, one window of the same application is corrected, which writes a
        // window rule and touches the app rung as its paired rung.
        maps.save_personal_override("abs_window", "FOCUS_WORK", Some("Editing"))
            .unwrap();
        assert!(maps
            .save_personal_app_override("triaged-app", "FOCUS_WORK", Some("Editing"))
            .unwrap());

        assert!(maps.remove_personal_override("abs_window").unwrap());

        assert!(
            maps.app_scope_override(&app).unwrap().is_some(),
            "removing the window rule must not destroy what the user taught about the application"
        );
        let (rules, total) = maps.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 1, "{rules:?}");
        assert_eq!(
            rules[0].scope,
            CorrectionScope::App,
            "and it is still listed, so it can still be undone on its own"
        );
    }

    /// A browser-tab correction generalizes to nothing (`app_scope_eligible = 0`,
    /// one site says nothing about the next), so it writes no app rung -- and its
    /// removal must not delete the browser's rung, which some other correction
    /// wrote. The delete used to resolve the application from the event rows
    /// regardless of eligibility and take that rule with it.
    #[test]
    fn removing_a_browser_tab_rule_leaves_the_app_rung_it_never_wrote() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let browser = key(0x22);
        // A browser-wide rung, written before the eligibility gate below existed
        // -- which is now the only way one can be here, and exactly the row the
        // old delete destroyed.
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO personal_app_override(app_key_hash, category, app_only)
                 VALUES (?1, 'REFERENCE', 0)",
                [browser.as_str()],
            )
            .unwrap();

        let mut tab = unlogged_event("tab", &browser, Some("Safari"), Utc::now(), 600);
        tab.stable_id = "abs_tab".into();
        tab.app_scope_eligible = false;
        events.insert(&tab).unwrap();
        maps.upsert(&window_mapping("abs_tab", &key(0x23))).unwrap();
        maps.save_personal_override("abs_tab", "FOCUS_WORK", Some("Reading"))
            .unwrap();

        assert!(
            paired_app_key(&database, &key(0x23)).is_none(),
            "a tab's rule records no app rung because none was written for it"
        );

        assert!(maps.remove_personal_override("abs_tab").unwrap());

        assert!(
            maps.app_scope_override(&browser).unwrap().is_some(),
            "removing one tab's rule must not delete a rule about the browser"
        );
    }

    /// Remove, on a correction older than the raw-event TTL.
    ///
    /// The pairing used to be re-derived by subquerying `raw_event_buffer`, which
    /// holds fourteen days: past that the subquery was empty, the window rule
    /// went, the app rung stayed, and Remove reported success having changed
    /// nothing the user could see. The row it left was `app_only = 0` -- hidden
    /// from the history by `RULE_SOURCE`, still matched by the triage NOT EXISTS
    /// -- so the application could be neither listed, removed nor taught again.
    /// This is not a rare case: it is every removal older than the TTL.
    #[test]
    fn removing_a_correction_older_than_the_event_ttl_still_removes_the_app_rung() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let maps = database.abstraction_map_repo();
        let events = database.raw_event_repo();
        let app = key(0x24);
        let mut event = unlogged_event("aged-out", &app, Some("Code"), Utc::now(), 600);
        event.stable_id = "abs_aged".into();
        events.insert(&event).unwrap();
        maps.upsert(&window_mapping("abs_aged", &key(0x25)))
            .unwrap();
        maps.save_personal_override("abs_aged", "FOCUS_WORK", Some("Editing"))
            .unwrap();
        assert!(maps
            .save_personal_app_override("aged-out", "FOCUS_WORK", Some("Editing"))
            .unwrap());
        assert_eq!(
            paired_app_key(&database, &key(0x25)).as_deref(),
            Some(app.as_str()),
            "the pairing is recorded while the event is still here"
        );

        // The TTL sweep, which is what makes this ordinary rather than rare.
        events
            .delete_expired_batch(Utc::now() + chrono::Duration::days(1), 100)
            .unwrap();
        assert!(events
            .events_before(Utc::now() + chrono::Duration::days(1))
            .unwrap()
            .is_empty());

        assert!(maps.remove_personal_override("abs_aged").unwrap());

        assert!(
            maps.app_scope_override(&app).unwrap().is_none(),
            "the app rung goes with the rule the user removed, event cache or no event cache"
        );
        assert_eq!(app_rule_count(&database), 0);
    }

    /// The rules already on disk. A correction made before 0036 recorded no
    /// pairing, and the migration recovers it for every one whose events are
    /// still inside the TTL -- so those become removable on upgrade rather than
    /// only from the next correction on.
    #[test]
    fn migration_0036_recovers_the_pairing_of_rules_already_on_disk() {
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
        for migration in super::EMBEDDED_MIGRATIONS
            .iter()
            .filter(|migration| migration.version < 36)
        {
            connection.execute_batch(migration.sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    rusqlite::params![migration.version, migration.name],
                )
                .unwrap();
        }
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };
        let app = key(0x30);
        let mut event = unlogged_event("legacy", &app, Some("Code"), Utc::now(), 600);
        event.stable_id = "abs_legacy".into();
        database.raw_event_repo().insert(&event).unwrap();
        database
            .abstraction_map_repo()
            .upsert(&window_mapping("abs_legacy", &key(0x31)))
            .unwrap();
        // Both rungs exactly as the correction path wrote them before this column
        // existed: written directly, because the writer now fills a column the
        // schema at this point does not have.
        {
            let connection = database.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO personal_override(key_hash, category, activity_name)
                     VALUES (?1, 'FOCUS_WORK', 'Editing')",
                    [key(0x31)],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO personal_app_override(app_key_hash, category, activity_name, app_only)
                     VALUES (?1, 'FOCUS_WORK', 'Editing', 0)",
                    [app.as_str()],
                )
                .unwrap();
        }

        database.run_migrations().unwrap();

        // 0037 re-keys both sides of the pairing with one function, so it still
        // joins; the assertions read the keys as they are after the upgrade.
        let app = after_0037(&database, &app);
        assert_eq!(
            paired_app_key(&database, &after_0037(&database, &key(0x31))).as_deref(),
            Some(app.as_str()),
            "the pairing is recoverable while the events are still inside the TTL"
        );
        // And the point of recovering it: the rule is removable afterwards, even
        // once those events are gone.
        let maps = database.abstraction_map_repo();
        database
            .raw_event_repo()
            .delete_expired_batch(Utc::now() + chrono::Duration::days(1), 100)
            .unwrap();
        assert!(maps.remove_personal_override("abs_legacy").unwrap());
        assert!(maps.app_scope_override(&app).unwrap().is_none());
    }

    // ---------------------------------------------------------------------
    // Triage offers only what it is safe to teach
    // ---------------------------------------------------------------------

    /// Writing an app-wide rule is the only thing the triage list lets a user
    /// do, so offering an application there is a claim that teaching it is safe.
    /// A browser never is: an app-wide rule would classify every future tab --
    /// video, forum, mail -- as whatever was said about one of them, at High
    /// confidence, and the engine reads that rung before the plugins, so
    /// `BrowserContextPlugin` would never run for that browser again.
    #[test]
    fn the_triage_list_never_offers_an_application_it_is_not_safe_to_teach() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let now = Utc::now();
        let browser = key(0x32);
        let bundle = key(0x33);
        let metadata = crate::persistence::DeclaredAppMetadata {
            app_bundle_stable_id: Some(bundle.clone()),
            ..crate::persistence::DeclaredAppMetadata::ABSENT
        };
        // A tab whose site Velvt read and could not classify: UNLOGGED, and not
        // generalizable to the application.
        let mut site_read = unlogged_event("site-read", &browser, Some("Safari"), now, 3_600);
        site_read.app_scope_eligible = false;
        events
            .insert_with_declared_metadata(&site_read, &metadata)
            .unwrap();
        // A window of the same browser Velvt could read no site from. Eligible on
        // its own row -- there is no site on it to say otherwise -- which is how
        // the browser reached this list as "Safari" in the first place.
        let site_unread = unlogged_event("site-unread", &browser, Some("Safari"), now, 3_600);
        events
            .insert_with_declared_metadata(&site_unread, &metadata)
            .unwrap();
        // The same browser under a second name key -- a rename, a localized name
        // -- so the bundle identity has to answer for it too.
        let renamed = unlogged_event("renamed", &key(0x34), Some("Safari"), now, 3_600);
        events
            .insert_with_declared_metadata(&renamed, &metadata)
            .unwrap();
        // An ordinary application, so this proves a filter rather than an empty
        // list.
        events
            .insert(&unlogged_event(
                "editor",
                &key(0x35),
                Some("Zed"),
                now,
                3_600,
            ))
            .unwrap();

        let entries = events.unclassified_triage(14, 300, 8).unwrap();

        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].app_stable_id, key(0x35));
    }

    /// And the writer refuses the same identity, rather than trusting whoever
    /// calls it. The filter above is a query one edit away from being widened,
    /// while a browser-wide rule is permanent and invisible once written.
    #[test]
    fn teaching_an_application_its_own_events_rule_out_is_refused() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        let browser = key(0x36);
        let bundle = key(0x37);
        let mut tab = unlogged_event("tab", &browser, Some("Safari"), Utc::now(), 3_600);
        tab.app_scope_eligible = false;
        events
            .insert_with_declared_metadata(
                &tab,
                &crate::persistence::DeclaredAppMetadata {
                    app_bundle_stable_id: Some(bundle.clone()),
                    ..crate::persistence::DeclaredAppMetadata::ABSENT
                },
            )
            .unwrap();

        assert!(matches!(
            maps.save_app_scope_override(&browser, Some(&bundle), "FOCUS_WORK", Some("Reading")),
            Err(super::PersistenceError::AppScopeIneligible)
        ));
        // Under a different name key carrying the same bundle identity, which is
        // how a rename reaches this call.
        assert!(matches!(
            maps.save_app_scope_override(&key(0x38), Some(&bundle), "FOCUS_WORK", None),
            Err(super::PersistenceError::AppScopeIneligible)
        ));
        assert_eq!(app_rule_count(&database), 0);

        // An application nothing is known about is still teachable: "no events
        // left" is the state every rule taught before these columns existed is
        // in, and refusing those would break editing a saved rule.
        maps.save_app_scope_override(&key(0x39), None, "FOCUS_WORK", None)
            .unwrap();
        assert_eq!(app_rule_count(&database), 1);
    }

    /// The paired path answers the same question the same way: a browser window
    /// with no site on it is eligible one row at a time, and generalizing from it
    /// would write the browser-wide rule the gate exists to prevent.
    #[test]
    fn a_correction_on_a_browser_window_does_not_write_an_app_rung() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let maps = database.abstraction_map_repo();
        let browser = key(0x3b);
        let mut site_read = unlogged_event("read", &browser, Some("Safari"), Utc::now(), 600);
        site_read.app_scope_eligible = false;
        events.insert(&site_read).unwrap();
        // The window the user actually corrects: same browser, no site read.
        let mut unread = unlogged_event("unread", &browser, Some("Safari"), Utc::now(), 600);
        unread.stable_id = "abs_unread".into();
        events.insert(&unread).unwrap();
        maps.upsert(&window_mapping("abs_unread", &key(0x3c)))
            .unwrap();

        assert!(!maps
            .save_personal_app_override("unread", "FOCUS_WORK", Some("Reading"))
            .unwrap());
        assert!(!maps
            .save_personal_app_override_by_stable_id("abs_unread", "FOCUS_WORK", Some("Reading"))
            .unwrap());
        maps.save_personal_override("abs_unread", "FOCUS_WORK", Some("Reading"))
            .unwrap();

        assert_eq!(app_rule_count(&database), 0);
        assert!(
            paired_app_key(&database, &key(0x3c)).is_none(),
            "the window rule records no pairing, because none was written"
        );
    }

    // ---------------------------------------------------------------------
    // Reported dwells for the work-block engine (00-GROUND-TRUTH § 6c, § 6d)
    // ---------------------------------------------------------------------

    /// The read returns exactly the dwells that overlap the window — including
    /// one that began before it and was still running — reads the stored
    /// vocabulary back as the enums the ledger uses, and treats a token it does
    /// not know as no evidence rather than failing: a failed read here would
    /// leave a block that can never be finalized.
    #[test]
    fn reported_dwells_are_the_ones_overlapping_the_window() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let events = database.raw_event_repo();
        let at =
            |seconds: i64| chrono::DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap();
        let entry = |event_id: &str, start: i64, seconds: u64, confidence: &str| {
            let mut event = unlogged_event(event_id, &key(0x51), None, at(start), seconds);
            event.category = "FOCUS_WORK".into();
            event.classification_status = "classified".into();
            event.classification_confidence = confidence.into();
            events.insert(&event).unwrap();
        };
        // Ends exactly at the window start: touches it, does not overlap.
        entry("ended-before", -100, 100, "high");
        // Began 20 minutes before the window and runs into it.
        entry("straddles-start", -1_200, 1_500, "high");
        entry("inside", 400, 60, "medium");
        entry("unknown-token", 500, 60, "certain");
        // Begins at the window end: outside a half-open window.
        entry("at-end", 1_000, 60, "high");

        let dwells = database
            .work_block_repo()
            .reported_dwells(at(0), at(1_000))
            .unwrap();
        let starts = dwells
            .iter()
            .map(|dwell| dwell.occurred_at)
            .collect::<Vec<_>>();
        assert_eq!(starts, vec![at(-1_200), at(400), at(500)]);
        assert_eq!(dwells[0].duration_seconds, 1_500);
        assert_eq!(dwells[0].ended_at(), at(300));
        assert_eq!(dwells[0].category, "FOCUS_WORK");
        assert_eq!(
            dwells[0].classification_status,
            ClassificationStatus::Classified
        );
        assert_eq!(
            dwells[1].classification_confidence,
            ClassificationConfidence::Medium
        );
        assert_eq!(
            dwells[2].classification_confidence,
            ClassificationConfidence::None
        );
        assert!(database
            .work_block_repo()
            .reported_dwells(at(10), at(10))
            .unwrap()
            .is_empty());
    }
}

/// Migration 0037 and the two reads that depend on it: the stable-key salt, and
/// the `abstraction_map` sweep.
///
/// A module of its own because the upgrade test below builds a database the way
/// a 1.0.11 install left it -- migrations 0001 to 0036 applied, rows keyed with
/// the unsalted digests -- and every assertion in it is about that one fixture.
#[cfg(test)]
mod salted_key_tests {
    use super::{SqlitePersistence, EMBEDDED_MIGRATIONS};
    use crate::abstraction::{
        app_bundle_key_for, app_stable_key_for, stable_key_for, AbstractionEngine,
        ClassificationSource,
    };
    use crate::persistence::AbstractionMapping;
    use chrono::{TimeZone, Utc};
    use rusqlite::{params, types::Value, Connection};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;
    use velvt_shared_types::{CorrectionScope, RawEvent};

    const EDITOR: &str = "Code";
    const EDITOR_BUNDLE: &str = "com.microsoft.VSCode";
    const EDITOR_TITLE: &str = "offer letter draft";
    const TAUGHT_APP: &str = "Quillard";
    const STALE_TITLE: &str = "a window from months ago";

    /// A key as 1.0.11 stored it: SHA-256 over the published domain string and
    /// the length-prefixed raw fields. Written out here rather than borrowed
    /// from `key.rs`, so the fixture is what the shipped build wrote and not
    /// whatever this build happens to compute.
    fn unsalted(domain: &str, fields: &[&str]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(domain.as_bytes());
        for field in fields {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn unsalted_window(app: &str, title: &str) -> String {
        unsalted("velvt:abstraction-key:v1", &[app, title])
    }

    fn unsalted_app(app: &str) -> String {
        unsalted("velvt:abstraction-app-key:v1", &[app])
    }

    fn unsalted_bundle(bundle_id: &str) -> String {
        unsalted("velvt:abstraction-app-bundle-key:v1", &[bundle_id])
    }

    /// A database exactly as far as 1.0.11 took it: every migration before
    /// 0037, recorded as applied, and nothing after.
    fn database_at_1_0_11() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
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
        for migration in EMBEDDED_MIGRATIONS
            .iter()
            .filter(|migration| migration.version < 37)
        {
            connection.execute_batch(migration.sql).unwrap();
            connection
                .execute(
                    "INSERT INTO schema_migration(version, name) VALUES (?1, ?2)",
                    params![migration.version, migration.name],
                )
                .unwrap();
        }
        connection
    }

    /// What a 1.0.11 user who had corrected one window of their editor, taught
    /// one application from triage, and once opened a window long ago would
    /// have on disk -- every key in its unsalted form.
    fn seed_1_0_11_rows(connection: &Connection) {
        let window = unsalted_window(EDITOR, EDITOR_TITLE);
        let editor = unsalted_app(EDITOR);
        let bundle = unsalted_bundle(EDITOR_BUNDLE);
        let taught = unsalted_app(TAUGHT_APP);
        let stale = unsalted_window(EDITOR, STALE_TITLE);
        let now = Utc::now().timestamp();
        connection
            .execute(
                "INSERT INTO abstraction_map(
                     key_hash, stable_id, label, category, taxonomy_version,
                     display_name, created_at, updated_at
                 ) VALUES (?1, 'abs_fixture_window', 'reference:inferred', 'REFERENCE',
                           'mvp-2', 'Offer letter', ?2, ?2)",
                params![window, now - 3_600],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO abstraction_map(
                     key_hash, stable_id, label, category, taxonomy_version,
                     created_at, updated_at
                 ) VALUES (?1, 'abs_fixture_stale', 'unlogged', 'UNLOGGED', 'mvp-2', ?2, ?2)",
                params![stale, now - 90 * 86_400],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_override(key_hash, category, activity_name, app_key_hash)
                 VALUES (?1, 'REFERENCE', 'Offer letter', ?2)",
                params![window, editor],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_app_override(
                     app_key_hash, bundle_key_hash, category, activity_name, app_only
                 ) VALUES (?1, ?2, 'REFERENCE', 'Offer letter', 0)",
                params![editor, bundle],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_app_override(app_key_hash, category, activity_name, app_only)
                 VALUES (?1, 'FOCUS_WORK', 'Quillard', 1)",
                params![taught],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO raw_event_buffer(
                     event_id, stable_id, label, category, taxonomy_version, occurred_at,
                     duration_seconds, app_stable_id, app_scope_eligible, app_bundle_stable_id
                 ) VALUES ('evt-fixture', 'abs_fixture_window', 'reference:inferred',
                           'REFERENCE', 'mvp-2', ?1, 300, ?2, 1, ?3)",
                params![now - 3_600, editor, bundle],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO semantic_embedding_cache(key_hash, embedding, dimensions)
                 VALUES (?1, x'0000803f', 1)",
                params![window],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_semantic_prototype(key_hash, category, embedding, dimensions)
                 VALUES (?1, 'REFERENCE', x'0000803f', 1)",
                params![window],
            )
            .unwrap();
        // Never keys Velvt wrote. `personal_override.key_hash` has no length
        // CHECK at all; the bundle column checks the length and not the case.
        connection
            .execute(
                "INSERT INTO raw_event_buffer(
                     event_id, stable_id, label, category, taxonomy_version, occurred_at,
                     app_bundle_stable_id
                 ) VALUES ('evt-uppercase', 'abs_fixture_other', 'unlogged', 'UNLOGGED',
                           'mvp-2', ?1, ?2)",
                params![now, "AB".repeat(32)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_override(key_hash, category) VALUES ('NOT-A-KEY', 'SYSTEM')",
                [],
            )
            .unwrap();
    }

    fn upgraded_from_1_0_11() -> SqlitePersistence {
        let connection = database_at_1_0_11();
        seed_1_0_11_rows(&connection);
        let database = SqlitePersistence {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.run_migrations().unwrap();
        database
    }

    fn text_values(database: &SqlitePersistence) -> Vec<(String, String)> {
        let connection = database.connection().unwrap();
        let tables: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let mut values = Vec::new();
        for table in tables {
            let columns: Vec<String> = connection
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap()
                .query_map([], |row| row.get(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for column in columns {
                let mut statement = connection
                    .prepare(&format!("SELECT \"{column}\" FROM \"{table}\""))
                    .unwrap();
                let rows = statement
                    .query_map([], |row| row.get::<_, Value>(0))
                    .unwrap()
                    .map(Result::unwrap);
                for value in rows {
                    if let Value::Text(text) = value {
                        values.push((format!("{table}.{column}"), text));
                    }
                }
            }
        }
        values
    }

    fn raw_event(app_name: &str, window_title: &str, bundle_id: Option<&str>) -> RawEvent {
        RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc.with_ymd_and_hms(2026, 9, 25, 9, 0, 0).unwrap(),
            duration_seconds: 60,
            app_name: app_name.to_owned(),
            window_title: window_title.to_owned(),
            bundle_id: bundle_id.map(str::to_owned),
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: None,
        }
    }

    /// The upgrade itself. Every key a 1.0.11 install stored becomes the key
    /// this build computes for the same raw inputs, under the salt 0037 minted,
    /// and no unsalted digest survives anywhere in the file.
    #[test]
    fn migration_0037_re_keys_every_digest_a_1_0_11_database_holds() {
        let database = upgraded_from_1_0_11();
        let salt = database.abstraction_map_repo().stable_key_salt().unwrap();
        let window = stable_key_for(&salt, EDITOR, EDITOR_TITLE);
        let editor = app_stable_key_for(&salt, EDITOR);
        let bundle = app_bundle_key_for(&salt, EDITOR_BUNDLE);
        let connection = database.connection().unwrap();

        let (map_key, updated_at): (String, i64) = connection
            .query_row(
                "SELECT key_hash, updated_at FROM abstraction_map
                 WHERE stable_id = 'abs_fixture_window'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(map_key, window);
        assert!(
            updated_at < Utc::now().timestamp() - 1_800,
            "re-keying must not look like an observation, or it restarts the sweep's clock"
        );
        let (rule_key, paired): (String, Option<String>) = connection
            .query_row(
                "SELECT key_hash, app_key_hash FROM personal_override WHERE category = 'REFERENCE'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rule_key, window);
        assert_eq!(paired.as_deref(), Some(editor.as_str()));
        let (app_rule, bundle_rule): (String, Option<String>) = connection
            .query_row(
                "SELECT app_key_hash, bundle_key_hash FROM personal_app_override
                 WHERE app_only = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(app_rule, editor);
        assert_eq!(bundle_rule.as_deref(), Some(bundle.as_str()));
        let (event_app, event_bundle): (String, String) = connection
            .query_row(
                "SELECT app_stable_id, app_bundle_stable_id FROM raw_event_buffer
                 WHERE event_id = 'evt-fixture'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_app, editor);
        assert_eq!(event_bundle, bundle);
        let uppercase: Option<String> = connection
            .query_row(
                "SELECT app_bundle_stable_id FROM raw_event_buffer
                 WHERE event_id = 'evt-uppercase'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            uppercase, None,
            "an optional value that was never a key is cleared"
        );
        for table in ["semantic_embedding_cache", "personal_semantic_prototype"] {
            let key: String = connection
                .query_row(&format!("SELECT key_hash FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(key, window, "{table}");
        }
        let malformed: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM personal_override WHERE key_hash = 'NOT-A-KEY'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            malformed, 0,
            "a value that was never a key is removed, not keyed"
        );
        drop(connection);

        let unsalted_digests = [
            unsalted_window(EDITOR, EDITOR_TITLE),
            unsalted_window(EDITOR, STALE_TITLE),
            unsalted_app(EDITOR),
            unsalted_app(TAUGHT_APP),
            unsalted_bundle(EDITOR_BUNDLE),
        ];
        let survivors: Vec<String> = text_values(&database)
            .into_iter()
            .filter(|(_, text)| unsalted_digests.iter().any(|digest| text.contains(digest)))
            .map(|(column, _)| column)
            .collect();
        assert!(
            survivors.is_empty(),
            "an unsalted digest survived the upgrade in {survivors:?}"
        );
    }

    /// What the user sees after the upgrade: every correction taught under
    /// 1.0.11 still applies, to the same window, the same application under
    /// either identity, and the application taught from triage -- and the
    /// window keeps the stable id it had, so its history is one history.
    #[test]
    fn corrections_taught_under_1_0_11_still_apply_after_the_upgrade() {
        let database = upgraded_from_1_0_11();
        let engine =
            AbstractionEngine::from_builtin_taxonomy(database.abstraction_mapping_store()).unwrap();

        let same_window = engine
            .process(raw_event(EDITOR, EDITOR_TITLE, Some(EDITOR_BUNDLE)))
            .unwrap();
        assert_eq!(same_window.category(), "REFERENCE");
        assert_eq!(
            same_window.classification_source(),
            ClassificationSource::UserRule
        );
        assert_eq!(same_window.stable_id(), "abs_fixture_window");
        assert_eq!(same_window.local_display_label(), Some("Offer letter"));

        let other_window = engine
            .process(raw_event(
                EDITOR,
                "a file never corrected",
                Some(EDITOR_BUNDLE),
            ))
            .unwrap();
        assert_eq!(other_window.category(), "REFERENCE", "the bundle rung");
        let renamed = engine
            .process(raw_event(
                "Visual Studio Code",
                "another",
                Some(EDITOR_BUNDLE),
            ))
            .unwrap();
        assert_eq!(
            renamed.category(),
            "REFERENCE",
            "the bundle rung, under a new name"
        );
        let no_bundle = engine
            .process(raw_event(EDITOR, "a third file", None))
            .unwrap();
        assert_eq!(no_bundle.category(), "REFERENCE", "the name rung");
        let taught = engine
            .process(raw_event(TAUGHT_APP, "anything", None))
            .unwrap();
        assert_eq!(taught.category(), "FOCUS_WORK");
        assert_eq!(
            taught.classification_source(),
            ClassificationSource::UserRule
        );

        let rules = database.abstraction_map_repo();
        let (listed, total) = rules.search_personal_overrides(None, 0, 20).unwrap();
        assert_eq!(total, 2, "one window rule and one triage rule: {listed:?}");
        assert!(listed
            .iter()
            .any(|rule| rule.scope == CorrectionScope::Window
                && rule.stable_id == "abs_fixture_window"));
        assert!(listed
            .iter()
            .any(|rule| rule.scope == CorrectionScope::App && rule.category == "FOCUS_WORK"));

        // The pairing 0036 recorded still joins, so removing the window rule
        // still takes the application rung it wrote with it.
        assert!(rules
            .remove_personal_override("abs_fixture_window")
            .unwrap());
        let salt = rules.stable_key_salt().unwrap();
        assert!(rules
            .app_scope_override(&app_stable_key_for(&salt, EDITOR))
            .unwrap()
            .is_none());
        assert!(rules
            .app_scope_override(&app_stable_key_for(&salt, TAUGHT_APP))
            .unwrap()
            .is_some());
    }

    /// The re-key belongs to applying 0037, not to opening the database: a
    /// second pass would HMAC keys that are already keyed and orphan every
    /// correction. Opening an upgraded database again changes no key.
    #[test]
    fn the_re_key_runs_once_per_database() {
        let database = upgraded_from_1_0_11();
        let keys = |database: &SqlitePersistence| -> Vec<(String, String)> {
            text_values(database)
                .into_iter()
                .filter(|(column, _)| {
                    super::KEYED_COLUMNS
                        .iter()
                        .any(|(table, name, _)| column == &format!("{table}.{name}"))
                })
                .collect()
        };
        let before = keys(&database);
        assert!(!before.is_empty());

        database.run_migrations().unwrap();

        assert_eq!(keys(&database), before);
    }

    /// The scripts under `scripts/tests/` build their schema by replaying the
    /// migration files with `sqlite3`, which cannot run the Rust half. The SQL
    /// half alone must still apply and leave the salt the schema expects.
    #[test]
    fn migration_0037_replays_as_plain_sql() {
        let connection = database_at_1_0_11();
        let migration = EMBEDDED_MIGRATIONS
            .iter()
            .find(|migration| migration.version == 37)
            .unwrap();

        connection.execute_batch(migration.sql).unwrap();

        let salt_length: i64 = connection
            .query_row("SELECT length(salt) FROM stable_key_salt", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(salt_length, 32);
    }

    /// 0037 mints the salt, so a migrated database only ever reads it, and two
    /// reads agree.
    #[test]
    fn the_stable_key_salt_is_read_back_unchanged() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let repo = database.abstraction_map_repo();

        assert_eq!(
            repo.stable_key_salt().unwrap(),
            repo.stable_key_salt().unwrap()
        );
        let other = SqlitePersistence::open_in_memory().unwrap();
        assert_ne!(
            repo.stable_key_salt().unwrap(),
            other.abstraction_map_repo().stable_key_salt().unwrap(),
            "two installs must not share a salt"
        );
    }

    /// The hand-deleted row. Every key on disk was computed under a salt that
    /// is gone, so the rows keyed under it go with it, in the same transaction
    /// that mints the new one -- nothing is left listed that can never apply.
    #[test]
    fn a_minted_stable_key_salt_removes_every_row_it_orphans() {
        let database = upgraded_from_1_0_11();
        let before = database.abstraction_map_repo().stable_key_salt().unwrap();
        database
            .connection()
            .unwrap()
            .execute("DELETE FROM stable_key_salt", [])
            .unwrap();

        let minted = database.abstraction_map_repo().stable_key_salt().unwrap();

        assert_ne!(minted, before);
        assert_eq!(
            minted,
            database.abstraction_map_repo().stable_key_salt().unwrap(),
            "the minted salt is persisted, not regenerated per call"
        );
        let connection = database.connection().unwrap();
        for table in [
            "personal_override",
            "personal_app_override",
            "personal_semantic_prototype",
            "semantic_embedding_cache",
            "abstraction_map",
        ] {
            let rows: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                rows, 0,
                "{table} still holds rows keyed under the lost salt"
            );
        }
        let keyed_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM raw_event_buffer
                 WHERE app_stable_id IS NOT NULL OR app_bundle_stable_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(keyed_events, 0);
    }

    fn mapping(key_hash: &str, stable_id: &str) -> AbstractionMapping {
        AbstractionMapping {
            key_hash: key_hash.to_owned(),
            stable_id: stable_id.to_owned(),
            label: "unlogged".into(),
            category: "UNLOGGED".into(),
            taxonomy_version: "mvp-2".into(),
            classification_tier: "fallback".into(),
            classification_status: "ambiguous".into(),
            classification_confidence: "low".into(),
            classification_source: "fallback".into(),
            display_name: None,
        }
    }

    /// The sweep: an unobserved mapping goes, and the two references that keep
    /// one past the horizon keep it -- a correction keyed on it, and an event
    /// still in the buffer that a correction would resolve through it.
    #[test]
    fn the_mapping_sweep_keeps_what_a_correction_or_an_event_still_needs() {
        let database = upgraded_from_1_0_11();
        let repo = database.abstraction_map_repo();
        repo.upsert(&mapping(&"1a".repeat(32), "abs_fresh"))
            .unwrap();
        let old = Utc::now().timestamp() - 30 * 86_400;
        {
            let connection = database.connection().unwrap();
            connection
                .execute(
                    "INSERT INTO abstraction_map(key_hash, stable_id, label, category,
                         taxonomy_version, created_at, updated_at)
                     VALUES (?1, 'abs_buffered', 'unlogged', 'UNLOGGED', 'mvp-2', ?2, ?2)",
                    params!["2b".repeat(32), old],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO raw_event_buffer(event_id, stable_id, label, category,
                         taxonomy_version, occurred_at)
                     VALUES ('evt-buffered', 'abs_buffered', 'unlogged', 'UNLOGGED', 'mvp-2', ?1)",
                    [old],
                )
                .unwrap();
            // The corrected window: last observed long ago, still ruled.
            connection
                .execute(
                    "UPDATE abstraction_map SET updated_at = ?1
                     WHERE stable_id = 'abs_fixture_window'",
                    [old],
                )
                .unwrap();
            connection
                .execute(
                    "DELETE FROM raw_event_buffer WHERE event_id = 'evt-fixture'",
                    [],
                )
                .unwrap();
        }
        let cutoff = Utc::now() - chrono::Duration::days(14);

        let deleted = repo.delete_expired_mappings(cutoff, 500).unwrap();

        let connection = database.connection().unwrap();
        let survivors: Vec<String> = connection
            .prepare("SELECT stable_id FROM abstraction_map ORDER BY stable_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(deleted, 1, "only the stale, unreferenced mapping goes");
        assert_eq!(
            survivors,
            vec!["abs_buffered", "abs_fixture_window", "abs_fresh"]
        );
        drop(connection);
        // Batched like every other target: a limit is a limit.
        assert_eq!(repo.delete_expired_mappings(cutoff, 0).unwrap(), 0);
    }

    /// The sweep runs on an index, not a scan of the buffer per candidate.
    #[test]
    fn the_mapping_sweep_reads_the_buffer_through_an_index() {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let connection = database.connection().unwrap();
        let plan: Vec<String> = connection
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT 1 FROM raw_event_buffer event WHERE event.stable_id = 'abs_x'",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(
            plan.iter()
                .any(|step| step.contains("idx_raw_event_buffer_stable_id")),
            "{plan:?}"
        );
    }
}
