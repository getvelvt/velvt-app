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
//! The four tests here are the machine-checked replacements:
//!
//! 1. `privacy_document_retention_cells_match_the_shipped_horizons` reads
//!    `PRIVACY.md`'s storage table out of the compiled binary and compares every
//!    number in every retention cell to the horizon the service actually runs
//!    on. Prose may be rewritten freely; a digit may not move on one side alone.
//! 2. `no_column_holds_the_sentinels_outside_the_documented_exceptions` drives a
//!    sentinel application name, window title, bundle identifier, declared
//!    category and declared document types through the real router and then reads
//!    back every value of every column of every table — by value, not by name,
//!    and including BLOBs.
//! 3. `no_declared_fact_reaches_an_upload_payload` drives the same event through
//!    the real upload path and reads the batch the uploader was handed, which is
//!    the last thing before the network. Invariant 1 of the Classification v2
//!    contract says the bundle identifier, the declared category and the declared
//!    document types are device-local; this is where that stops being prose.
//! 4. `migrated_schema_holds_exactly_the_documented_tables` closes the table
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
use velvt_service::abstraction::{
    app_bundle_key_for, AbstractionEngine, EmbeddingSimilarityPlugin, Taxonomy,
};
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
    BatchAssembler, BatchPayload, BatchUploadError, BatchUploader, EventIngestor,
    FakeBatchUploader, FakePrivacyAlertSink, SharedUploadBatcher, UploadBatcher, UploadCoordinator,
    UploadOutcome,
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

/// A sentinel bundle identifier, shaped like a real one and belonging to no real
/// application.
///
/// Migration 0033 claims the raw identifier is never stored: only
/// `app_bundle_key_for`'s digest of it reaches `raw_event_buffer`. That is a
/// claim about a value, so it is checked with a value. The digest is the positive
/// control — it must be on disk, or the walk proves nothing about a fact that
/// never arrived.
const SENTINEL_BUNDLE_ID: &str = "com.thraceline.ledger";
/// The distinctive token of the sentinel bundle identifier, lowercased. A column
/// storing `thraceline` alone, or the identifier normalized or truncated, is
/// still storing the identifier.
const SENTINEL_BUNDLE_TOKEN: &str = "thraceline";

/// A sentinel `LSApplicationCategoryType`, shaped like the Apple constants the
/// whitelist matches and on no whitelist, so the classifier abstains exactly as
/// it does for an application that declares nothing.
///
/// Unlike the bundle identifier this one is stored raw, in the single column
/// migration 0033 declares for it, and only there.
const SENTINEL_DECLARED_CATEGORY: &str = "public.app-category.glimberly";
const SENTINEL_DECLARED_CATEGORY_TOKEN: &str = "glimberly";

/// Sentinel declared document types, already deduplicated and sorted the way the
/// client sends them, and mapping to no category — so this event classifies as
/// it would with no declared types at all.
///
/// Two of them, because the column holds a set joined into one string: one
/// identifier would not show that both survive the encoding, nor that the
/// delimiter is the single ASCII space migration 0033 documents.
const SENTINEL_DOCUMENT_TYPE_IDS: [&str; 2] =
    ["com.wexcombe.sentinel-note", "public.wexcombe-draft"];
/// The token shared by both sentinel document types, lowercased.
const SENTINEL_DOCUMENT_TYPE_TOKEN: &str = "wexcombe";

fn sentinel_document_type_ids() -> Vec<String> {
    SENTINEL_DOCUMENT_TYPE_IDS
        .iter()
        .map(|identifier| (*identifier).to_owned())
        .collect()
}

/// The raw event the two value-level tests below drive, carrying every fact
/// Classification v2 added.
///
/// One constructor for both, so the upload-payload test and the column walk can
/// never drift into asserting about different inputs — the two halves of one
/// claim ("this fact lands here on disk, and nowhere on the wire") are only
/// joined if the fact is the same fact.
fn sentinel_raw_event(event_id: Uuid) -> RawEvent {
    RawEvent {
        event_id,
        occurred_at: Utc::now(),
        app_name: SENTINEL_APP_NAME.into(),
        window_title: SENTINEL_WINDOW_TITLE.into(),
        bundle_id: Some(SENTINEL_BUNDLE_ID.into()),
        declared_app_category: Some(SENTINEL_DECLARED_CATEGORY.into()),
        document_type_ids: sentinel_document_type_ids(),
        focused_document_url: None,
        duration_seconds: 300,
    }
}

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

/// The one column disclosed to hold the raw `LSApplicationCategoryType` the
/// application declares about itself, as migration 0033 names it.
///
/// It is on its own list rather than added to `DEVICE_LOCAL_EXCEPTION_COLUMNS`
/// because the two claims are different and merging them would weaken both: this
/// column may hold the declared category, and it may not hold the application
/// name or the window title. One list of permitted columns shared by every
/// sentinel would say only "one of these facts is allowed in one of these
/// columns", which is not a claim anyone published.
const DECLARED_CATEGORY_COLUMNS: &[&str] = &["raw_event_buffer.declared_app_category"];

/// The one column disclosed to hold the declared `LSItemContentTypes`, as
/// migration 0033 names it. Same reasoning as above.
const DOCUMENT_TYPE_COLUMNS: &[&str] = &["raw_event_buffer.document_type_ids"];

/// The one column disclosed to hold the bundle *digest*, as migration 0033 names
/// it.
///
/// This list is the positive control for the bundle identifier, not an exception
/// for it: the raw identifier is permitted in no column at all, and the digest
/// must be in exactly this one. A migration that stored the identifier instead of
/// its hash would satisfy neither.
const BUNDLE_DIGEST_COLUMNS: &[&str] = &["raw_event_buffer.app_bundle_stable_id"];

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

/// A sentinel application name, window title, bundle identifier, declared
/// category and declared document-type list, driven through the real router, must
/// not come back out of any column of any table except the ones this project has
/// published as exceptions.
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
///
/// Classification v2 added three facts to the raw event, so all three are
/// sentinel-bearing inputs here. Each one has a positive control of its own,
/// because the failure that matters is silent: a fact the router never recorded
/// is a fact this walk cannot find, and "not found" reads identically to "never
/// stored". The bundle identifier's control is its digest — the raw identifier is
/// permitted in no column, and the hash of it is required in exactly one, which
/// together are migration 0033's claim that a digest is stored *instead of* the
/// identifier rather than beside it.
#[tokio::test]
async fn no_column_holds_the_sentinels_outside_the_documented_exceptions() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let router = sentinel_router(&persistence);

    let acknowledgement = router
        .route(ClientMessage::RawEvent(sentinel_raw_event(Uuid::new_v4())))
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

    // The key the router is supposed to have stored in place of the bundle
    // identifier, computed here from the same public function the engine uses --
    // so this test asserts the stored value is that digest rather than merely
    // something 64 characters long.
    let bundle_digest = app_bundle_key_for(SENTINEL_BUNDLE_ID);

    let mut app_sightings = BTreeSet::new();
    let mut title_sightings = BTreeSet::new();
    let mut bundle_id_sightings = BTreeSet::new();
    let mut bundle_digest_sightings = BTreeSet::new();
    let mut declared_category_sightings = BTreeSet::new();
    let mut document_type_sightings = BTreeSet::new();
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
            title_sightings.insert(qualified.clone());
        }
        if holds_token(value, SENTINEL_BUNDLE_TOKEN) {
            bundle_id_sightings.insert(qualified.clone());
        }
        if holds_token(value, &bundle_digest) {
            bundle_digest_sightings.insert(qualified.clone());
        }
        if holds_token(value, SENTINEL_DECLARED_CATEGORY_TOKEN) {
            declared_category_sightings.insert(qualified.clone());
        }
        if holds_token(value, SENTINEL_DOCUMENT_TYPE_TOKEN) {
            document_type_sightings.insert(qualified);
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

    // The bar for appearing on any disclosure list in this file is being named in
    // PRIVACY.md, and until now that bar was a comment. A column may be excepted
    // here only if the published document says it exists, so the document is read
    // rather than trusted -- by the bare column name, because the document
    // describes columns under their table's heading rather than as `table.column`.
    for column in DEVICE_LOCAL_EXCEPTION_COLUMNS
        .iter()
        .chain(BUNDLE_DIGEST_COLUMNS)
        .chain(DECLARED_CATEGORY_COLUMNS)
        .chain(DOCUMENT_TYPE_COLUMNS)
    {
        let qualified: &str = column;
        let bare = qualified
            .split_once('.')
            .map(|(_, bare)| bare)
            .unwrap_or(qualified);
        assert!(
            PRIVACY_DOCUMENT.contains(bare),
            "`{qualified}` is excepted by this test and named nowhere in PRIVACY.md. A \
             column this build permits to hold a device-local fact is a column the \
             published document has to describe -- otherwise the disclosure lives only \
             in a test nobody outside this repository can read"
        );
    }

    // The bundle identifier, in both directions at once. The digest must be in
    // exactly the column migration 0033 declares for it -- that is the positive
    // control, and it is what makes the next assertion mean something -- and the
    // identifier it was computed from must be in no column at all. A schema that
    // stored `com.thraceline.ledger` beside its hash would pass the first
    // assertion and fail the second, which is the point of having both.
    assert_eq!(
        bundle_digest_sightings,
        column_set(BUNDLE_DIGEST_COLUMNS),
        "the bundle key belongs in exactly {BUNDLE_DIGEST_COLUMNS:?}. An empty left \
         side means the router never recorded the bundle identifier the client sent, \
         so every bundle assertion here is vacuous; an extra column means a second \
         store learned an application identity nothing discloses"
    );
    assert!(
        bundle_id_sightings.is_empty(),
        "the raw bundle identifier reached {bundle_id_sightings:?}. Migration 0033 and \
         `app_bundle_key_for` both state that only the digest is persisted, and no \
         column is excepted -- a bundle identifier is the application's identity in \
         plain text"
    );

    // The two declared facts are stored raw, each in the one column migration
    // 0033 declares for it. Equality rather than a subset: a fact missing from its
    // own column means the walk never saw it, and a fact in a second column is an
    // undisclosed store of what the user runs.
    assert_eq!(
        declared_category_sightings,
        column_set(DECLARED_CATEGORY_COLUMNS),
        "the declared application category belongs in exactly \
         {DECLARED_CATEGORY_COLUMNS:?}. A missing column means the declaration never \
         reached disk and this assertion proves nothing; an extra one belongs in \
         PRIVACY.md and in DECLARED_CATEGORY_COLUMNS -- in that order, and in one commit"
    );
    assert_eq!(
        document_type_sightings,
        column_set(DOCUMENT_TYPE_COLUMNS),
        "the declared document types belong in exactly {DOCUMENT_TYPE_COLUMNS:?}. A \
         missing column means the declaration never reached disk and this assertion \
         proves nothing; an extra one belongs in PRIVACY.md and in \
         DOCUMENT_TYPE_COLUMNS -- in that order, and in one commit"
    );

    // Where it landed, exactly. Migration 0033 documents the encoding as the
    // sorted identifiers joined by one ASCII space, and publishes an `instr`
    // recipe that is only correct against that encoding. A reader following the
    // recipe against a JSON array, a comma-separated list, or a truncated set
    // would silently match nothing, so the published spelling is asserted rather
    // than described.
    let stored_document_types: Option<String> = connection
        .query_row(
            "SELECT document_type_ids FROM raw_event_buffer",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let expected_document_types = SENTINEL_DOCUMENT_TYPE_IDS.join(" ");
    assert_eq!(
        stored_document_types.as_deref(),
        Some(expected_document_types.as_str()),
        "`raw_event_buffer.document_type_ids` does not hold the encoding migration \
         0033 publishes: one line of sorted identifiers joined by a single space"
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

/// One of the `table.column` disclosure lists above, as a set.
fn column_set(columns: &[&str]) -> BTreeSet<String> {
    columns.iter().map(|column| (*column).to_owned()).collect()
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
// 3 — No declared fact on the wire, by value
// ---------------------------------------------------------------------------

/// The last thing before the network, holding what would have been POSTed.
///
/// `FakeBatchUploader` counts uploads and keeps the payloads to itself, and the
/// claim under test is about their contents, so this double keeps them. It is the
/// real `BatchUploader` seam the HTTP uploader sits in, reached through the real
/// router, assembler and coordinator: nothing here rebuilds a payload by hand,
/// because a payload built by the test is a payload the test cannot be surprised
/// by.
#[derive(Clone, Default)]
struct RecordingUploader(Arc<Mutex<Vec<BatchPayload>>>);

impl RecordingUploader {
    fn captured(&self) -> Vec<BatchPayload> {
        self.0.lock().unwrap().clone()
    }
}

impl BatchUploader for RecordingUploader {
    fn upload<'a>(
        &'a self,
        batch: &'a BatchPayload,
    ) -> Pin<Box<dyn Future<Output = Result<UploadOutcome, BatchUploadError>> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(batch.clone());
            // Refused, like the fake the sentinel walk uses, so the batch stays
            // queued on disk and the two tests observe the same state.
            Ok(UploadOutcome::Retryable {
                code: "host_unreachable".into(),
            })
        })
    }
}

/// No value Classification v2 added to a raw event may appear in the batch this
/// device would have uploaded.
///
/// Invariant 1 of the implementation contract says the bundle identifier, the
/// declared category and the declared document types are device-local and reach
/// no DTO, and requires this test by name — twice. `upload/dto.rs` makes the
/// claim structurally true by hand-writing `Serialize`, and `dto.rs`'s own
/// `serialized_batch_holds_exactly_the_documented_keys` closes the key set. This
/// test is the end-to-end half: a real event carrying all three facts, driven
/// through the real router, engine, assembler and coordinator, and the payload
/// read back at the seam the HTTP uploader occupies.
///
/// The assertions are about **values**, not field names. A field renamed, a fact
/// folded into an existing string field, or a digest smuggled into
/// `classification_tier` all still fail here, and none of them would fail a test
/// that listed forbidden keys. The bundle *digest* is forbidden too: it is not the
/// identifier, but it is a stable per-application identity, and the contract
/// admits nothing new to the wire at all.
#[tokio::test]
async fn no_declared_fact_reaches_an_upload_payload() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let uploader = RecordingUploader::default();
    let router = sentinel_router_with_uploader(&persistence, uploader.clone());

    let event_id = Uuid::new_v4();
    let acknowledgement = router
        .route(ClientMessage::RawEvent(sentinel_raw_event(event_id)))
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
        "the sentinel event was not accepted, so no batch was assembled to inspect: \
         {acknowledgement:?}"
    );

    // The positive control, and the reason the batch threshold is one event: an
    // uploader that was handed nothing satisfies every assertion below it.
    let batches = uploader.captured();
    assert_eq!(
        batches.len(),
        1,
        "the sentinel event did not reach the uploader, so this test asserts nothing \
         about an upload payload. The router uploads only when authenticated and the \
         assembler flushes at its event threshold -- both are set up above"
    );
    let wire = serde_json::to_string(&batches[0]).unwrap();
    let wire_lowercase = wire.to_lowercase();
    assert!(
        wire.contains(&event_id.to_string()),
        "the captured batch does not carry the event that was driven through it, so \
         the absences below are absences of the wrong event: {wire}"
    );

    for (fact, value) in [
        ("the bundle identifier", SENTINEL_BUNDLE_ID.to_owned()),
        (
            "a fragment of the bundle identifier",
            SENTINEL_BUNDLE_TOKEN.to_owned(),
        ),
        (
            "the bundle key digest",
            app_bundle_key_for(SENTINEL_BUNDLE_ID),
        ),
        (
            "the declared application category",
            SENTINEL_DECLARED_CATEGORY.to_owned(),
        ),
        (
            "a fragment of the declared application category",
            SENTINEL_DECLARED_CATEGORY_TOKEN.to_owned(),
        ),
        (
            "a declared document type",
            SENTINEL_DOCUMENT_TYPE_IDS[0].to_owned(),
        ),
        (
            "a declared document type",
            SENTINEL_DOCUMENT_TYPE_IDS[1].to_owned(),
        ),
        (
            "a fragment of the declared document types",
            SENTINEL_DOCUMENT_TYPE_TOKEN.to_owned(),
        ),
        ("the raw application name", SENTINEL_APP_NAME.to_owned()),
        (
            "a fragment of the application name",
            SENTINEL_APP_TOKEN.to_owned(),
        ),
        ("the window title", SENTINEL_WINDOW_TITLE.to_owned()),
        (
            "a fragment of the window title",
            SENTINEL_TITLE_TOKEN.to_owned(),
        ),
    ] {
        assert!(
            !wire_lowercase.contains(&value.to_lowercase()),
            "{fact} ({value}) appears in the batch this device would have POSTed. \
             Invariant 1 of the Classification v2 contract admits nothing new across \
             the wire: the field it arrived in does not matter, and renaming that \
             field does not fix it.\nThe payload was: {wire}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4 — The table inventory is closed
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
    // One outcome, and a refusal: the batch is persisted and attempted, and the
    // rows stay on disk for the walk to read.
    sentinel_router_with_uploader(
        persistence,
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Retryable {
            code: "host_unreachable".into(),
        }]),
    )
}

/// The same router, with the uploader chosen by the caller, so one test can read
/// the payload it would have sent without the other losing the fake it wants.
fn sentinel_router_with_uploader<U>(persistence: &SqlitePersistence, uploader: U) -> R7Router
where
    U: BatchUploader + 'static,
{
    let taxonomy = Taxonomy::from_builtin().unwrap();
    let embedding = EmbeddingSimilarityPlugin::builtin(taxonomy.version())
        .unwrap()
        .with_learning_store(persistence.semantic_learning_store());
    let engine = AbstractionEngine::builder(persistence.abstraction_mapping_store(), taxonomy)
        .register_builtin_plugins_with_embedding(Some(embedding))
        .build()
        .unwrap();

    // One event per batch, so the batch is assembled, persisted, and attempted
    // within this test rather than waiting on a flush interval.
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader,
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
