//! The published claims, asserted rather than written down.
//!
//! Velvt's privacy story is prose, and prose does not fail a build. An audit on
//! 2026-08-31 found nine statements across seven files that the database or the
//! binary falsified, and not one of them was covered by a test. The guards that
//! did exist were structurally incapable of catching what they were written to
//! catch: the retention pin in `config` compared the TTL to a window derived
//! from the same constant, and the schema guard in `persistence_contract`
//! forbade five column *names* as substrings of `sqlite_master` while the column
//! holding 9,073 raw macOS application names is called `local_name_suggestion`
//! and matched none of them.
//!
//! The three tests here are the machine-checked replacements:
//!
//! 1. `privacy_document_retention_cells_match_the_shipped_horizons` reads
//!    `PRIVACY.md`'s storage table out of the compiled binary and compares every
//!    number in every retention cell to the horizon the service actually runs
//!    on. Prose may be rewritten freely; a digit may not move on one side alone.
//! 2. `no_column_holds_the_sentinels_outside_the_documented_exceptions` drives a
//!    sentinel application name and window title through the real router and
//!    then reads back every value of every column of every table — by value, not
//!    by name, and including BLOBs.
//! 3. `migrated_schema_holds_exactly_the_documented_tables` closes the table
//!    inventory, so a new migration cannot add a store the document does not
//!    mention.
//!
//! `persistence_contract::schema_has_no_forbidden_raw_content_columns` still
//! exists and still checks column names. It is kept: a forbidden name is worth
//! catching early and cheaply. Test 2 here is the assertion that would have
//! failed, and it deliberately overlaps rather than replaces.
//!
//! # Why the module include
//!
//! `behavior` is declared in `src/main.rs`, not `src/lib.rs`, so an integration
//! test cannot reach `OUT_OF_BLOCK_RUN_RETENTION_DAYS` through `velvt_service`.
//! Until someone who owns `lib.rs` moves it, the file is included by path,
//! exactly as `behavior_antecedents.rs` and `behavior_segmentation.rs` include
//! theirs. The cost is that its two unit tests compile and run in two crates.

#[path = "../src/behavior/retention.rs"]
mod behavior_retention;

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use rusqlite::{types::Value, Connection};
use uuid::Uuid;
use velvt_service::abstraction::{AbstractionEngine, EmbeddingSimilarityPlugin, Taxonomy};
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::config::ServiceConfig;
use velvt_service::delivery::FakeCacheManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{AbstractionMapping, SqlitePersistence};
use velvt_service::retention::SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS;
use velvt_service::upload::{
    BatchAssembler, EventIngestor, FakeBatchUploader, FakePrivacyAlertSink, SharedUploadBatcher,
    UploadBatcher, UploadCoordinator, UploadOutcome,
};
use velvt_service::work_block::WorkBlockManager;
use velvt_shared_types::{
    ClientMessage, RawEvent, RawEventAck, RawEventStatus, ServerMessage, StartWorkBlock,
    WorkBlockIntensity,
};

use behavior_retention::OUT_OF_BLOCK_RUN_RETENTION_DAYS;

/// The published document, compiled into this binary.
///
/// `include_str!` rather than a runtime read: a path that stops resolving is a
/// compile error here, where a `File::open` that stops resolving is a test that
/// quietly stops checking anything.
const PRIVACY_DOCUMENT: &str = include_str!("../../PRIVACY.md");

/// A sentinel application name. Not a real product, long enough to be
/// unmistakable in a failure message, short enough to survive
/// `responsible_local_name_suggestion`'s 48-character ceiling, and not a member
/// of its generic-name list — so it is retained rather than discarded, which is
/// what makes the positive control below possible.
const SENTINEL_APP_NAME: &str = "Kervanth Ledger";
/// The distinctive token of the sentinel application name, lowercased. Matching
/// on the token rather than the whole string catches a column that stored a
/// normalized, truncated, or first-word-only derivation of it.
const SENTINEL_APP_TOKEN: &str = "kervanth";

/// A sentinel window title. `PRIVACY.md` says the literal window title is not
/// preserved anywhere, with no exception, so this token is expected in no
/// column at all — not even the device-local ones that may hold an application
/// name.
const SENTINEL_WINDOW_TITLE: &str = "Brindlow settlement draft, third revision";
const SENTINEL_TITLE_TOKEN: &str = "brindlow";

// ---------------------------------------------------------------------------
// 1 — The retention cells in PRIVACY.md, against the shipped horizons
// ---------------------------------------------------------------------------

/// Every number in a retention cell of `PRIVACY.md`'s storage table must equal
/// the horizon the shipped code enforces.
///
/// The parse is deliberately number-shaped rather than sentence-shaped: it
/// extracts the digit runs from a cell and compares them as a sorted multiset,
/// so the prose around them may be rewritten, reordered, or extended without
/// touching this test, and a single changed digit on either side fails it. The
/// row set is closed in both directions, so adding a table to the document
/// without pinning its horizon fails too.
///
/// Commit `515ccf5` doubled the raw-event horizon from seven days to fourteen
/// and touched no `.md` file. Six published statements were false for four days.
/// This is the test that would have caught it in the same commit.
#[test]
fn privacy_document_retention_cells_match_the_shipped_horizons() {
    let config = shipped_config();
    let sent_days = config.sent_batch_retention.as_secs() / 86_400;
    let rejected_days = config.rejected_batch_audit_period.as_secs() / 86_400;
    let raw_event_days = config.raw_event_ttl.as_secs() / 86_400;

    // Three of the published numbers are inline SQL literals in
    // `persistence::sqlite` rather than named constants, so they are measured
    // against the real DAL instead of imported. A measured number is a stronger
    // pin than an imported one anyway: it fails if the SQL changes even when the
    // constant beside it does not.
    let (prototype_total_cap, prototype_per_category_cap) = measured_prototype_caps();
    let expected: Vec<(&str, Vec<u64>)> = vec![
        ("abstraction_map", vec![]),
        ("raw_event_buffer", vec![raw_event_days]),
        // "pending and failed batches: 30 days, the same horizon as sent" is
        // literally true in the DAL: `delete_stale_queued_batch` takes the sent
        // cutoff and filters on `created_at` with no status predicate.
        (
            "upload_batch,batch_event",
            vec![sent_days, rejected_days, sent_days],
        ),
        ("personal_override", vec![]),
        ("personal_app_override", vec![]),
        (
            "semantic_embedding_cache",
            vec![
                measured_embedding_cache_capacity(),
                SEMANTIC_EMBEDDING_CACHE_RETENTION_DAYS,
            ],
        ),
        (
            "personal_semantic_prototype",
            vec![prototype_total_cap, prototype_per_category_cap],
        ),
        ("history_cache,insight_cache", vec![]),
        ("work_block", vec![measured_intention_retention_hours()]),
        ("work_block_observation", vec![]),
        ("work_block_result", vec![]),
        ("intervention_decision_log", vec![]),
        ("out_of_block_run", vec![OUT_OF_BLOCK_RUN_RETENTION_DAYS]),
        ("block_antecedent", vec![]),
        ("antecedent_finding", vec![]),
    ];

    // Keyed rather than zipped, so reordering the rows of the document is a
    // prose edit and not a failure. The two key sets are compared first, so a
    // row added to the document without a horizon pinned here still fails.
    let documented: Vec<StorageRow> = storage_table_rows();
    assert_eq!(
        documented
            .iter()
            .map(|row| row.key.clone())
            .collect::<BTreeSet<_>>(),
        expected
            .iter()
            .map(|(key, _)| (*key).to_owned())
            .collect::<BTreeSet<_>>(),
        "PRIVACY.md's storage table no longer lists the rows this test pins; \
         a row added there needs a shipped horizon pinned here, and a row \
         removed from there needs removing here"
    );

    for (key, shipped) in &expected {
        let row = documented
            .iter()
            .find(|row| row.key == *key)
            .expect("the key sets were just asserted equal");
        let mut documented_numbers = numbers_in(&row.retention);
        let mut shipped_numbers = shipped.clone();
        documented_numbers.sort_unstable();
        shipped_numbers.sort_unstable();
        assert_eq!(
            documented_numbers, shipped_numbers,
            "PRIVACY.md publishes {documented_numbers:?} for `{key}` and the \
             shipped code enforces {shipped_numbers:?}. The document reads: {}",
            row.retention
        );
    }
}

/// One row of the storage table in `PRIVACY.md`.
struct StorageRow {
    /// The backticked table names in the first cell, comma-joined, so a row
    /// covering two tables has one stable key.
    key: String,
    /// The verbatim final cell, retained for the failure message: the number is
    /// what fails, but the sentence is what a reader has to go and correct.
    retention: String,
}

/// The storage table, located by its header rather than by line number.
///
/// A line-number-addressed parse would silently start reading the wrong table
/// the first time a paragraph is inserted above it, which is the failure mode
/// this whole file exists to remove.
fn storage_table_rows() -> Vec<StorageRow> {
    let mut lines = PRIVACY_DOCUMENT
        .lines()
        .map(str::trim)
        .skip_while(|line| !is_storage_table_header(line));
    let header = lines.next();
    assert!(
        header.is_some(),
        "PRIVACY.md no longer contains a table whose first column is `Table` and \
         whose last is `Default retention`"
    );
    let separator = lines.next().unwrap_or_default();
    assert!(
        separator.starts_with('|') && separator.contains("---"),
        "the storage table header in PRIVACY.md is not followed by a separator row"
    );

    lines
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            let cells = table_cells(line);
            StorageRow {
                key: backticked(&cells[0]).join(","),
                retention: cells[cells.len() - 1].clone(),
            }
        })
        .collect()
}

fn is_storage_table_header(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    let cells = table_cells(line);
    cells.first().is_some_and(|cell| cell == "Table")
        && cells.last().is_some_and(|cell| cell == "Default retention")
}

/// The content cells of a markdown table row, without the empty edges that
/// splitting on the delimiter produces.
fn table_cells(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// The identifiers a cell names in backticks. `` `upload_batch` / `batch_event` ``
/// yields both, and the prose between them is discarded.
fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Every run of decimal digits in a cell, in order.
///
/// Digits rather than words, so the sentence around a number can be rewritten
/// freely. A cell that names `VELVT_RAW_EVENT_TTL_HOURS` and states no value
/// contributes nothing, which is right: a pointer to a knob is not a published
/// horizon, and only a stated number is a claim.
fn numbers_in(cell: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for character in cell.chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            numbers.push(current.parse().expect("a digit run parses"));
            current.clear();
        }
    }
    if !current.is_empty() {
        numbers.push(current.parse().expect("a digit run parses"));
    }
    numbers
}

/// The configuration the service runs on with nothing set in the environment.
///
/// The published horizons are defaults, so an ambient `VELVT_*` override in a
/// developer's shell would make this test pass for the wrong reason.
fn shipped_config() -> ServiceConfig {
    static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    std::env::remove_var("VELVT_RAW_EVENT_TTL_HOURS");
    std::env::remove_var("VELVT_SENT_BATCH_RETENTION_DAYS");
    std::env::remove_var("VELVT_REJECTED_BATCH_AUDIT_DAYS");
    ServiceConfig::load().expect("the shipped defaults load without any environment")
}

/// The `semantic_embedding_cache` row cap, measured by overflowing it.
fn measured_embedding_cache_capacity() -> u64 {
    const WRITES: u64 = 600;
    let database = SqlitePersistence::open_in_memory().unwrap();
    let store = database.semantic_learning_store();
    for index in 0..WRITES {
        store
            .record_embedding(&format!("{index:064x}"), &[1.0, index as f32])
            .unwrap();
    }
    let survivors = (0..WRITES)
        .filter(|index| store.embedding(&format!("{index:064x}")).unwrap().is_some())
        .count() as u64;
    assert!(
        survivors < WRITES,
        "the embedding cache accepted {WRITES} pairs without evicting any, so it has no cap to measure"
    );
    survivors
}

/// The two `personal_semantic_prototype` caps, as `(total, per_category)`:
/// the per-category cap by overflowing one category, the total by filling
/// several categories that each stay under it.
///
/// Both measurements assert that they overflowed. A measurement that never
/// reached the cap would report the number of writes instead of the cap and
/// would then disagree with a correct document, which is the wrong direction
/// for this test to fail in.
fn measured_prototype_caps() -> (u64, u64) {
    const CORRECTIONS_IN_ONE_CATEGORY: usize = 40;
    const CATEGORIES: &[&str] = &[
        "FOCUS_WORK",
        "COMMUNICATION",
        "PASSIVE_CONSUMPTION",
        "REFERENCE",
        "SYSTEM",
        "UNCLASSIFIED",
        "SOCIAL",
        "ADMIN",
        "PLANNING",
        "LEARNING",
    ];

    let database = SqlitePersistence::open_in_memory().unwrap();
    correct_prototypes(&database, &["FOCUS_WORK"], CORRECTIONS_IN_ONE_CATEGORY);
    let per_category = database
        .abstraction_map_repo()
        .personal_semantic_prototype_count()
        .unwrap();
    assert!(
        (per_category as usize) < CORRECTIONS_IN_ONE_CATEGORY,
        "{CORRECTIONS_IN_ONE_CATEGORY} corrections in one category evicted nothing, \
         so the per-category cap was never reached and {per_category} is a write count"
    );

    let database = SqlitePersistence::open_in_memory().unwrap();
    correct_prototypes(&database, CATEGORIES, per_category as usize);
    let total = database
        .abstraction_map_repo()
        .personal_semantic_prototype_count()
        .unwrap();
    let written = CATEGORIES.len() * per_category as usize;
    assert!(
        (total as usize) < written,
        "{written} corrections across {} categories evicted nothing, so the total cap \
         was never reached and {total} is a write count",
        CATEGORIES.len()
    );
    assert!(
        per_category < total,
        "the per-category cap ({per_category}) is not smaller than the total cap ({total}), \
         so PRIVACY.md's two numbers cannot be told apart"
    );
    (total, per_category)
}

/// Corrects `per_category` distinct window identities in each named category,
/// through the same DAL the correction path uses, so each one promotes its
/// cached sketch into a prototype.
fn correct_prototypes(database: &SqlitePersistence, categories: &[&str], per_category: usize) {
    let repository = database.abstraction_map_repo();
    let semantic = database.semantic_learning_store();
    let mut index = 0_u64;
    for category in categories {
        for _ in 0..per_category {
            let key_hash = format!("{index:064x}");
            let stable_id = format!("abs_prototype_{index}");
            repository
                .upsert(&AbstractionMapping {
                    key_hash: key_hash.clone(),
                    stable_id: stable_id.clone(),
                    label: "unlogged".into(),
                    category: "UNLOGGED".into(),
                    taxonomy_version: "mvp-1".into(),
                    classification_tier: "fallback".into(),
                    classification_status: "ambiguous".into(),
                    classification_confidence: "low".into(),
                    classification_source: "fallback".into(),
                    display_name: None,
                })
                .unwrap();
            semantic
                .record_embedding(&key_hash, &[1.0, index as f32])
                .unwrap();
            repository
                .save_personal_override(&stable_id, category, None)
                .unwrap();
            index += 1;
        }
    }
}

/// The work-block intention horizon, measured from a block the manager started.
fn measured_intention_retention_hours() -> u64 {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.work_block_repo();
    let manager = WorkBlockManager::new(repository.clone());
    let started_at = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
    manager
        .start(
            StartWorkBlock {
                intention: Some("local intention text".into()),
                planned_duration_seconds: 1_500,
                purpose: None,
                intensity: WorkBlockIntensity::Medium,
                invitation_id: None,
            },
            started_at,
        )
        .unwrap();
    let block = repository
        .latest()
        .unwrap()
        .expect("the block was declared");
    (block.intention_expires_at - block.started_at)
        .num_hours()
        .try_into()
        .expect("the intention horizon is a positive number of hours")
}

// ---------------------------------------------------------------------------
// 2 — No raw content in any column, by value
// ---------------------------------------------------------------------------

/// The columns this project has deliberately and publicly excepted from the
/// no-raw-content invariant, as `table.column`.
///
/// Being disclosed by name in `PRIVACY.md` is the bar for being on this list.
/// `local_name_suggestion` is additionally named in the headers of
/// `0001_initial_persistence.sql` and `0011_local_activity_suggestions.sql`;
/// `display_name` and the two `activity_name` columns are named in the storage
/// table as the places the name a user typed is kept.
///
/// Adding a column here is the deliberate act the audit found missing. Do not
/// add one without adding it to `PRIVACY.md` in the same commit.
const DEVICE_LOCAL_EXCEPTION_COLUMNS: &[&str] = &[
    "abstraction_map.display_name",
    "personal_app_override.activity_name",
    "personal_override.activity_name",
    "raw_event_buffer.local_display_label",
    "raw_event_buffer.local_name_suggestion",
];

/// Every column in the migrated schema declared `BLOB`.
///
/// `scripts/prove_local.sh` filtered on `TEXT`/`CHAR`/`CLOB`, so BLOB columns
/// were counted and never enumerated, and the one table that holds a sketch of
/// a window title was reported as clean. The script now names them as
/// UNINSPECTED, which is honest but is not an inspection: bash and `sqlite3`
/// cannot decode a sketch. A new BLOB column is therefore content the published
/// audit tool cannot read back, and it turns this build red until someone
/// decides in writing what it holds.
const BLOB_BEARING_COLUMNS: &[&str] = &[
    "embedding_salt.salt",
    "personal_semantic_prototype.embedding",
    "semantic_embedding_cache.embedding",
];

/// A sentinel application name and window title, driven through the real
/// router, must not come back out of any column of any table except the ones
/// this project has published as exceptions.
///
/// This is the assertion `persistence_contract::schema_has_no_forbidden_raw_content_columns`
/// was written to make and cannot: it forbids the substrings `app_name`,
/// `window_title`, `bundle_id`, `url`, and `file_path` in the schema text, and
/// the column that holds the raw macOS application name is called
/// `local_name_suggestion`. A name taboo can only catch a careless name. This
/// one reads the values.
///
/// The positive control is load-bearing. A walk that silently visits nothing
/// passes every negative assertion, which is precisely how the guard it replaces
/// stayed green over 9,073 real application names.
#[tokio::test]
async fn no_column_holds_the_sentinels_outside_the_documented_exceptions() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let router = sentinel_router(&persistence);

    let acknowledgement = router
        .route(ClientMessage::RawEvent(RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            app_name: SENTINEL_APP_NAME.into(),
            window_title: SENTINEL_WINDOW_TITLE.into(),
            bundle_id: None,
            focused_document_url: None,
            duration_seconds: 300,
        }))
        .await
        .unwrap();
    assert!(
        matches!(
            acknowledgement,
            Some(ServerMessage::RawEventAck(RawEventAck {
                status: RawEventStatus::Accepted,
                ..
            }))
        ),
        "the sentinel event was not accepted, so nothing was written to look at: {acknowledgement:?}"
    );
    drop(persistence);

    // A second connection to the same file, which is what PRIVACY.md invites a
    // reader to open. `SqlitePersistence` does not hand out its own connection,
    // and an in-memory database cannot be reached from a second handle, so this
    // test writes to disk. The migrations are still the real EMBEDDED_MIGRATIONS
    // -- `SqlitePersistence::open` ran them.
    let connection = Connection::open(&scratch.path).unwrap();

    let mut app_sightings = BTreeSet::new();
    let mut title_sightings = BTreeSet::new();
    let mut blob_columns_holding_values = BTreeSet::new();
    let mut values_read = 0_usize;
    let mut values_present = 0_usize;
    scan_every_value(&connection, |table, column, value| {
        values_read += 1;
        if matches!(value, Value::Null) {
            return;
        }
        values_present += 1;
        let qualified = format!("{table}.{column}");
        if matches!(value, Value::Blob(_)) {
            blob_columns_holding_values.insert(qualified.clone());
        }
        if holds_token(value, SENTINEL_APP_TOKEN) {
            app_sightings.insert(qualified.clone());
        }
        if holds_token(value, SENTINEL_TITLE_TOKEN) {
            title_sightings.insert(qualified);
        }
    });

    assert!(
        values_present > 0,
        "the scan read {values_read} values and every one of them was NULL, \
         so it proves nothing"
    );

    // The positive control: the raw application name is retained on purpose, so
    // it must be found. If it is not, the walk is looking in the wrong place and
    // every assertion below it is vacuous.
    assert!(
        app_sightings.contains("raw_event_buffer.local_name_suggestion"),
        "the sentinel application name was not found in the one column documented \
         to hold it, so this scan is not reaching stored values: found {app_sightings:?}"
    );

    let permitted: BTreeSet<String> = DEVICE_LOCAL_EXCEPTION_COLUMNS
        .iter()
        .map(|column| (*column).to_owned())
        .collect();
    let undisclosed: Vec<&String> = app_sightings.difference(&permitted).collect();
    assert!(
        undisclosed.is_empty(),
        "the raw application name reached {undisclosed:?}, which PRIVACY.md does not \
         disclose as a device-local exception. Either the column should not hold it, \
         or it belongs in PRIVACY.md and in DEVICE_LOCAL_EXCEPTION_COLUMNS -- in that \
         order, and in one commit"
    );

    // The window title has no exception. PRIVACY.md states that the literal
    // title is not preserved, and states separately that a lossy sketch of it
    // is -- from which individual words are partially recoverable. This
    // assertion is about the literal string only. Word-level recoverability from
    // the sketch is a different property, disclosed in the document rather than
    // fixed: migration 0031 mints a per-install salt, but `main.rs` still builds
    // the classifier with `EmbeddingSimilarityPlugin::builtin`, which runs on the
    // zero salt, so the document's "the hash is unsalted" is currently accurate.
    assert!(
        title_sightings.is_empty(),
        "the window title reached {title_sightings:?}. PRIVACY.md states the literal \
         title is preserved nowhere and names no exception to that"
    );

    let declared_blob_columns = declared_blob_columns(&connection);
    let allowlisted: BTreeSet<String> = BLOB_BEARING_COLUMNS
        .iter()
        .map(|column| (*column).to_owned())
        .collect();
    assert_eq!(
        declared_blob_columns, allowlisted,
        "the set of BLOB columns in the migrated schema changed. A BLOB is stored \
         content the published audit tool cannot read back, so each one is listed \
         here on purpose"
    );
    assert!(
        blob_columns_holding_values.is_subset(&allowlisted),
        "a column not declared BLOB is storing BLOB values: {:?}",
        blob_columns_holding_values
            .difference(&allowlisted)
            .collect::<Vec<_>>()
    );
}

/// Visits every value of every column of every table in `sqlite_master`.
///
/// `PRAGMA table_info` rather than a hand-written column list, and
/// `sqlite_master` rather than a hand-written table list, because the guard this
/// replaces enumerated what its author remembered. Values are read as
/// `rusqlite::types::Value`, which preserves BLOBs as bytes rather than
/// declining to look at them.
fn scan_every_value(connection: &Connection, mut visit: impl FnMut(&str, &str, &Value)) {
    for table in table_names(connection) {
        let columns: Vec<String> = connection
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for column in columns {
            let values: Vec<Value> = connection
                .prepare(&format!("SELECT \"{column}\" FROM \"{table}\""))
                .unwrap()
                .query_map([], |row| row.get::<_, Value>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for value in &values {
                visit(&table, &column, value);
            }
        }
    }
}

/// Whether a stored value carries the token, in text or in raw bytes.
fn holds_token(value: &Value, token: &str) -> bool {
    match value {
        Value::Text(text) => text.to_lowercase().contains(token),
        Value::Blob(bytes) => contains_ascii_ignoring_case(bytes, token.as_bytes()),
        Value::Null | Value::Integer(_) | Value::Real(_) => false,
    }
}

fn contains_ascii_ignoring_case(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn table_names(connection: &Connection) -> Vec<String> {
    connection
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn declared_blob_columns(connection: &Connection) -> BTreeSet<String> {
    let mut columns = BTreeSet::new();
    for table in table_names(connection) {
        let declared: Vec<(String, String)> = connection
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (column, declared_type) in declared {
            if declared_type.to_uppercase().contains("BLOB") {
                columns.insert(format!("{table}.{column}"));
            }
        }
    }
    columns
}

// ---------------------------------------------------------------------------
// 3 — The table inventory is closed
// ---------------------------------------------------------------------------

/// Every table the shipped migrations create.
///
/// Adding a table here without adding it to `PRIVACY.md`'s storage inventory is
/// the exact mistake this test exists to catch: migrations 0027, 0028, and 0029
/// each landed a durable behavioural store, and the document caught up with them
/// only after an outside reader opened the file and found stores it did not
/// describe.
///
/// `schema_migration` is on the list because it is on the disk. It is created by
/// `run_migrations`, not by a migration file, and a reader who opens the
/// database will see it.
const MIGRATED_TABLES: &[&str] = &[
    "abstraction_map",
    "antecedent_finding",
    "batch_event",
    "block_antecedent",
    "classification_telemetry",
    "classifier_artifact_telemetry",
    "embedding_salt",
    "explain_probe_week",
    "focus_observer_state",
    "focus_state_evidence",
    "history_cache",
    "initiation_invitation",
    "initiation_settings",
    "insight_cache",
    "intervention_decision_log",
    "intervention_demotion_state",
    "out_of_block_run",
    "persistence_migration_probe",
    "personal_app_override",
    "personal_override",
    "personal_semantic_prototype",
    "quiet_hours_offer_state",
    "raw_event_buffer",
    "schema_migration",
    "semantic_embedding_cache",
    "upload_batch",
    "upload_host_backoff",
    "velvt_quiet_hours",
    "weekly_digest",
    "work_block",
    "work_block_category_correction",
    "work_block_intervention",
    "work_block_observation",
    "work_block_result",
];

/// The migrated schema holds exactly the tables listed above, and every table
/// `PRIVACY.md` claims exists does exist.
///
/// The second half is the cheaper direction and worth having: a table renamed in
/// a migration but still described in the document is a published claim about a
/// store that is not there.
#[test]
fn migrated_schema_holds_exactly_the_documented_tables() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    drop(persistence);
    let connection = Connection::open(&scratch.path).unwrap();

    let present: BTreeSet<String> = table_names(&connection).into_iter().collect();
    let expected: BTreeSet<String> = MIGRATED_TABLES
        .iter()
        .map(|table| (*table).to_owned())
        .collect();
    assert_eq!(
        present, expected,
        "the migrated schema no longer holds exactly MIGRATED_TABLES. A new table \
         belongs in that list and in PRIVACY.md's storage inventory, in the same commit"
    );

    let documented: BTreeSet<String> = storage_table_rows()
        .iter()
        .flat_map(|row| row.key.split(',').map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let missing: Vec<&String> = documented.difference(&present).collect();
    assert!(
        missing.is_empty(),
        "PRIVACY.md describes storage for {missing:?}, which the migrated schema does not create"
    );
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A database file in its own directory, removed when the test ends.
struct ScratchDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl ScratchDatabase {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("velvt-published-claims-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("velvt-service.sqlite3");
        Self { directory, path }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The router, wired the way `main.rs` wires it for everything this test can
/// observe: the built-in Tier 2 plugin with the learning store attached, so the
/// embedding cache is written rather than left empty and trivially clean, and an
/// authenticated session, so the upload queue is written rather than skipped.
fn sentinel_router(persistence: &SqlitePersistence) -> R7Router {
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let embedding = EmbeddingSimilarityPlugin::builtin(taxonomy.version())
        .unwrap()
        .with_learning_store(persistence.semantic_learning_store());
    let engine = AbstractionEngine::builder(persistence.abstraction_mapping_store(), taxonomy)
        .register_builtin_plugins_with_embedding(Some(embedding))
        .build()
        .unwrap();

    // One event per batch, so the batch is assembled, persisted, and attempted
    // within this test rather than waiting on a flush interval. The upload
    // itself is refused so the rows stay on disk to be inspected.
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Retryable {
            code: "host_unreachable".into(),
        }]),
        FakePrivacyAlertSink::default(),
    );
    let ingestor: Arc<dyn EventIngestor> = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-published-claims", 1, StdDuration::from_secs(3_600)),
        coordinator,
    )));

    let account = Arc::new(AccountAuthService::new(
        Arc::new(UnreachableHttp),
        Arc::new(UnreachableHttp),
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    let (_sender, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: "device-published-claims".into(),
    });

    R7Router::new(
        Arc::new(FakeCacheManager::new()),
        Arc::new(engine),
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
    .with_auth_state(auth_state)
}

/// No account request is made by this test, and one that were made would be a
/// change worth noticing rather than one worth answering.
struct UnreachableHttp;

impl HttpClient for UnreachableHttp {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async { Err(AuthError::Transport) })
    }
}
