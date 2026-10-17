//! The field and retention inventory, and exactly what each destructive action
//! reaches.
//!
//! Two things are checked here: a field inventory, and the reach of each
//! destructive action.
//!
//! **The field inventory.** `docs/field-inventory.md` lists, for the tables
//! added for the question after a block (and any later table that holds a
//! person's own answers), every column and the class of value it may hold. Its
//! classes have no free-text class.
//!
//! - A table it marks `planned` must not exist yet.
//! - A table it marks `migrated` must have exactly its columns, types and
//!   nullability.
//! - Every column of a migrated table must refuse a typed sentence. SQLite
//!   stores whatever it is given in a column with no CHECK, so a closed class
//!   is a property of the schema rather than of the code that writes it.
//!
//! **The reach of each destructive action.** Reset Corrections, Clear Local
//! Work Blocks, Log Out and Delete Account are each sent through the real
//! router against a database that holds at least one row in every table, and a
//! value in every nullable column on at least one row. The router is wired as
//! `main.rs` wires it for every component these actions touch; the network
//! client is a fake (see below). Every table is then compared before and
//! after, row by row.
//!
//! - `REACH` says what each action does to each table. It has to name every
//!   table in the migrated schema, so a new table fails here until someone
//!   decides what each of the four actions does to it.
//! - A partial reach names its rows. A table an action deletes from, or
//!   rewrites, lists the seeded rows that change, and every other row must be
//!   exactly as it was.
//! - An action that changes a table in any other way fails, whether it reaches
//!   further or falls short.
//! - `PRIVACY.md` gives each action a bullet, and that bullet has to name every
//!   table `REACH` says the action changes.
//!
//! Before this test, `PRIVACY.md`'s list of what each action removes was
//! checked by nothing. It said nothing about signing out marking an open
//! invitation expired, and it named no table for Delete Account.
//!
//! What this file does not claim:
//!
//! - **Rows that are not on disk yet.** It tests what each action does to rows
//!   already in the database. Log Out also writes to the queue whatever the
//!   departing account still held in memory, which `account_switch.rs` covers.
//!   Here nothing is held in memory, so that write is empty.
//! - **The egress ledger's own row.** The network client here is a fake. The
//!   shipped client appends an `egress_ledger` row for every request it sends:
//!   the account-deletion request, and the session revocation Log Out sends
//!   when it holds a token. Those added rows are not exercised here.
//! - **An account deleted from another Mac.** That path
//!   (`end_deleted_account_session`, run by `SessionWatch`) does not go through
//!   the router's `DeleteAccount`, and `account_switch.rs` covers it.
//! - **No in-app delete-all.** There is no in-app action that deletes
//!   everything. `PRIVACY.md`, "How to delete all local data", is the complete
//!   path.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use rusqlite::{types::Value, Connection, ErrorCode};
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::category_prompt::{CategoryPromptManager, ListedCandidates};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::initiation::{InitiationManager, InvitationGates, RuntimeInvitationGates};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{SelfRatingAnswer, SqlitePersistence};
use velvt_service::receipts::ReceiptsManager;
use velvt_service::self_rating::RatingPromptManager;
use velvt_service::upload::{
    BatchAssembler, EventIngestor, FakeBatchUploader, FakePrivacyAlertSink, SharedUploadBatcher,
    UploadBatcher, UploadCoordinator,
};
use velvt_service::work_block::{FocusStateSource, WorkBlockManager};
use velvt_shared_types::{
    ClearWorkBlockData, ClientMessage, DeleteAccount, LogOut, ResetClassificationOverrides,
    ServerMessage,
};

const INVENTORY: &str = include_str!("../../docs/field-inventory.md");
const PRIVACY_DOCUMENT: &str = include_str!("../../PRIVACY.md");

/// What a covered column is written with to prove it refuses free text.
const TYPED_SENTENCE: &str = "Quorvane, something I typed about this block";

/// The values each class rules out besides free text, which a column of that
/// class must refuse too: a flag or the singleton id is never 2, a timestamp
/// is a whole number of seconds, and a version is a whole number of 1 or more.
fn ruled_out_by(class: &str) -> &'static [&'static str] {
    match class {
        "flag" | "singleton" => &["2"],
        "timestamp" => &["1.5"],
        "version" => &["0", "1.5"],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// The actions and their reach
// ---------------------------------------------------------------------------

/// The app's destructive actions, in the order of `REACH`'s arrays and of the
/// inventory's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    ResetCorrections,
    ClearLocalWorkBlocks,
    LogOut,
    DeleteAccount,
}

impl Action {
    const ALL: [Self; 4] = [
        Self::ResetCorrections,
        Self::ClearLocalWorkBlocks,
        Self::LogOut,
        Self::DeleteAccount,
    ];

    /// The button's name, and the inventory's column heading.
    fn name(self) -> &'static str {
        match self {
            Self::ResetCorrections => "Reset Corrections",
            Self::ClearLocalWorkBlocks => "Clear Local Work Blocks",
            Self::LogOut => "Log Out",
            Self::DeleteAccount => "Delete Account",
        }
    }

    /// How `PRIVACY.md`'s list of destructive actions opens this action's
    /// bullet.
    fn privacy_bullet(self) -> &'static str {
        match self {
            Self::ResetCorrections => "- **Reset Corrections**",
            Self::ClearLocalWorkBlocks => "- **Clear Local Work Blocks**",
            Self::LogOut => "- **Signing out**",
            Self::DeleteAccount => "- **Delete Account**",
        }
    }

    fn message(self) -> ClientMessage {
        match self {
            Self::ResetCorrections => {
                ClientMessage::ResetClassificationOverrides(ResetClassificationOverrides {})
            }
            Self::ClearLocalWorkBlocks => ClientMessage::ClearWorkBlockData(ClearWorkBlockData {}),
            Self::LogOut => ClientMessage::LogOut(LogOut {}),
            Self::DeleteAccount => ClientMessage::DeleteAccount(DeleteAccount {}),
        }
    }

    /// The positive control: the router answered as it does when the action
    /// ran. Without it, a router missing a component would "keep" every table.
    fn ran(self, response: &Option<ServerMessage>) -> bool {
        match (self, response) {
            (Self::ResetCorrections, Some(ServerMessage::MenuStatus(status))) => {
                status.correction_acknowledgment.is_some()
            }
            (Self::ClearLocalWorkBlocks, Some(ServerMessage::WorkBlockState(_))) => true,
            (Self::LogOut, None) => true,
            (Self::DeleteAccount, Some(ServerMessage::AccountDeletionAccepted(_))) => true,
            _ => false,
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|action| *action == self)
            .expect("every action is in ALL")
    }
}

/// What one action does to the rows already in one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// Every row is exactly as it was.
    Kept,
    /// No row is left.
    Removed,
    /// Exactly these seeded rows are gone. Every other row is exactly as it
    /// was.
    RemovedInPart(Rows),
    /// No row is gone. Exactly these columns change, on exactly these seeded
    /// rows, and every other row is exactly as it was.
    Rewritten(&'static [&'static str], Rows),
}

/// Seeded rows, named by the values they hold in one column: `Rows("batch_id",
/// &["batch-seed-waiting"])`. Every value must name at least one seeded row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rows(&'static str, &'static [&'static str]);

use Reach::{Kept, Removed, RemovedInPart, Rewritten};

/// The two invitations in the seed: one already answered, one still open.
const ANSWERED_INVITATION: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8aa1";
const OPEN_INVITATION: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8aa2";

/// What Log Out and Delete Account do to invitations: only the open one is
/// marked expired. The answered one keeps its outcome.
const EXPIRES_THE_OPEN_INVITATION: Reach = Rewritten(
    &["outcome", "outcome_at"],
    Rows("invitation_id", &[OPEN_INVITATION]),
);

/// What each action does to each table, in `Action::ALL` order: Reset
/// Corrections, Clear Local Work Blocks, Log Out, Delete Account.
///
/// Every table in the migrated schema has a row. A new table gets one, and a
/// seeded row in `seed_sql`, in the migration's commit. For a table that
/// `docs/field-inventory.md` covers, the inventory states the same reach.
const REACH: &[(&str, [Reach; 4])] = &[
    (
        "abstraction_map",
        [
            Rewritten(
                &["display_name"],
                Rows("stable_id", &["abs_seed_window", "abs_seed_site"]),
            ),
            Kept,
            Kept,
            Kept,
        ],
    ),
    ("antecedent_finding", [Kept; 4]),
    (
        "batch_event",
        [
            Kept,
            Kept,
            Kept,
            // The events of the two batches waiting to be sent.
            RemovedInPart(Rows(
                "event_id",
                &["event-seed-waiting", "event-seed-pending"],
            )),
        ],
    ),
    ("block_antecedent", [Kept, Removed, Kept, Kept]),
    ("category_prompt_card_entry", [Kept; 4]),
    ("category_prompt_entry", [Kept; 4]),
    ("category_prompt_notification", [Kept; 4]),
    ("classification_telemetry", [Kept; 4]),
    ("classifier_artifact_telemetry", [Kept; 4]),
    ("egress_ledger", [Kept; 4]),
    ("egress_ledger_checkpoint", [Kept; 4]),
    ("embedding_salt", [Kept; 4]),
    ("explain_probe_week", [Kept, Removed, Kept, Kept]),
    ("focus_observer_state", [Kept, Removed, Kept, Kept]),
    ("focus_state_evidence", [Kept, Removed, Kept, Kept]),
    ("history_cache", [Kept; 4]),
    (
        "initiation_invitation",
        [
            Kept,
            Removed,
            EXPIRES_THE_OPEN_INVITATION,
            EXPIRES_THE_OPEN_INVITATION,
        ],
    ),
    ("initiation_settings", [Kept; 4]),
    ("insight_cache", [Kept; 4]),
    ("intervention_decision_log", [Kept, Removed, Kept, Kept]),
    ("intervention_demotion_state", [Kept, Removed, Kept, Kept]),
    ("local_site_name", [Kept; 4]),
    ("out_of_block_run", [Kept; 4]),
    ("persistence_migration_probe", [Kept; 4]),
    ("personal_app_override", [Removed, Kept, Kept, Kept]),
    ("personal_override", [Removed, Kept, Kept, Kept]),
    ("personal_semantic_prototype", [Removed, Kept, Kept, Kept]),
    ("personal_site_override", [Removed, Kept, Kept, Kept]),
    ("quiet_hours_offer_state", [Kept, Removed, Kept, Kept]),
    ("rating_prompt_settings", [Kept; 4]),
    (
        "raw_event_buffer",
        [
            Kept,
            Kept,
            Kept,
            // The events in the two batches waiting to be sent, and the event
            // not yet in a batch. The events of the sent and the rejected
            // batch, and the local-only event, are untouched.
            Rewritten(
                &["upload_eligible", "upload_owner_user_id"],
                Rows(
                    "event_id",
                    &[
                        "event-seed-waiting",
                        "event-seed-pending",
                        "event-seed-unbatched",
                    ],
                ),
            ),
        ],
    ),
    ("schema_migration", [Kept; 4]),
    ("semantic_embedding_cache", [Kept; 4]),
    ("stable_key_salt", [Kept; 4]),
    (
        "upload_batch",
        [
            Kept,
            Kept,
            Kept,
            // The failed and the pending batch. The sent and the rejected
            // batch stay.
            RemovedInPart(Rows(
                "batch_id",
                &["batch-seed-waiting", "batch-seed-pending"],
            )),
        ],
    ),
    ("upload_host_backoff", [Kept; 4]),
    ("velvt_quiet_hours", [Kept; 4]),
    ("weekly_digest", [Kept, Removed, Kept, Kept]),
    ("work_block", [Kept, Removed, Kept, Kept]),
    (
        "work_block_category_correction",
        [Kept, Removed, Kept, Kept],
    ),
    ("work_block_intervention", [Kept, Removed, Kept, Kept]),
    ("work_block_observation", [Kept, Removed, Kept, Kept]),
    ("work_block_result", [Kept, Removed, Kept, Kept]),
    ("work_block_self_rating", [Kept, Removed, Kept, Kept]),
];

/// A reach as `docs/field-inventory.md` writes it: which kind, and for a
/// rewrite which columns. Which seeded rows a partial reach touches is
/// `REACH`'s detail, not the inventory's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Documented {
    Kept,
    Removed,
    RemovedInPart,
    Rewritten(BTreeSet<String>),
}

impl From<Reach> for Documented {
    fn from(reach: Reach) -> Self {
        match reach {
            Reach::Kept => Self::Kept,
            Reach::Removed => Self::Removed,
            Reach::RemovedInPart(_) => Self::RemovedInPart,
            Reach::Rewritten(columns, _) => Self::Rewritten(column_set(columns)),
        }
    }
}

fn column_set(columns: &[&str]) -> BTreeSet<String> {
    columns.iter().map(|column| (*column).to_owned()).collect()
}

fn reach_of(table: &str) -> Option<[Reach; 4]> {
    REACH
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, reach)| *reach)
}

// ---------------------------------------------------------------------------
// 1 — Every table has a reach, and the inventory agrees with it
// ---------------------------------------------------------------------------

/// `REACH` names exactly the tables the migrations create, once each.
#[test]
fn every_table_has_a_reach_for_every_action() {
    let scratch = ScratchDatabase::new();
    drop(SqlitePersistence::open(&scratch.path).unwrap());
    let connection = Connection::open(&scratch.path).unwrap();

    let listed: Vec<&str> = REACH.iter().map(|(table, _)| *table).collect();
    let unique: BTreeSet<&str> = listed.iter().copied().collect();
    assert_eq!(listed.len(), unique.len(), "REACH lists a table twice");

    let present: BTreeSet<String> = table_names(&connection).into_iter().collect();
    let listed: BTreeSet<String> = unique.into_iter().map(str::to_owned).collect();
    let unlisted: Vec<&String> = present.difference(&listed).collect();
    let phantom: Vec<&String> = listed.difference(&present).collect();
    assert!(
        unlisted.is_empty() && phantom.is_empty(),
        "REACH no longer names exactly the migrated tables.\n\
         In the schema, not in REACH: {unlisted:?}\n\
         In REACH, not in the schema: {phantom:?}\n\
         A new table gets a REACH row saying what Reset Corrections, Clear Local Work \
         Blocks, Log Out and Delete Account each do to it, and a seeded row in seed_sql"
    );
}

/// For every table the inventory marks `migrated`, its reach cells are
/// `REACH`'s row, and the actions are then held to both by the tests below.
#[test]
fn the_inventory_states_the_tested_reach() {
    for table in inventory_tables() {
        match table.status {
            Status::Planned => assert!(
                reach_of(&table.name).is_none(),
                "`{}` is planned in docs/field-inventory.md but has a REACH row",
                table.name
            ),
            Status::Migrated => {
                let reach = reach_of(&table.name).unwrap_or_else(|| {
                    panic!(
                        "`{}` is migrated in docs/field-inventory.md but has no REACH row",
                        table.name
                    )
                });
                for action in Action::ALL {
                    assert_eq!(
                        table.reach[action.index()],
                        Documented::from(reach[action.index()]),
                        "docs/field-inventory.md and REACH disagree on what {} does to `{}`",
                        action.name(),
                        table.name
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2 — Each action changes exactly what REACH says
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reset_corrections_reaches_exactly_its_allow_list() {
    assert_action_reaches_exactly_its_allow_list(Action::ResetCorrections).await;
}

#[tokio::test]
async fn clear_local_work_blocks_reaches_exactly_its_allow_list() {
    assert_action_reaches_exactly_its_allow_list(Action::ClearLocalWorkBlocks).await;
}

#[tokio::test]
async fn log_out_reaches_exactly_its_allow_list() {
    assert_action_reaches_exactly_its_allow_list(Action::LogOut).await;
}

#[tokio::test]
async fn delete_account_reaches_exactly_its_allow_list() {
    assert_action_reaches_exactly_its_allow_list(Action::DeleteAccount).await;
}

async fn assert_action_reaches_exactly_its_allow_list(action: Action) {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let connection = seeded_connection(&scratch);
    let router = shipped_router(&persistence);

    let before = snapshot(&connection);
    let response = router.route(action.message()).await.unwrap();
    assert!(
        action.ran(&response),
        "{} did not run, so nothing below would mean anything: {response:?}",
        action.name()
    );
    let after = snapshot(&connection);

    let mut mismatches = Vec::new();
    for (table, before_rows) in &before {
        let Some(reach) = reach_of(table) else {
            mismatches.push(format!("`{table}` has no REACH row"));
            continue;
        };
        if let Some(departure) = departure(reach[action.index()], before_rows, &after[table]) {
            mismatches.push(format!("`{table}`: {departure}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} no longer changes exactly the tables REACH names:\n  {}\n\
         If the change is intended, update REACH, PRIVACY.md's list of what the app's \
         destructive actions remove, and docs/field-inventory.md for a covered table, \
         in the same commit",
        action.name(),
        mismatches.join("\n  ")
    );
}

/// Every row of one table, keyed by rowid.
struct TableRows {
    columns: Vec<String>,
    rows: BTreeMap<i64, Vec<Value>>,
}

fn snapshot(connection: &Connection) -> BTreeMap<String, TableRows> {
    table_names(connection)
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT rowid, * FROM \"{table}\" ORDER BY rowid"))
                .unwrap();
            let columns: Vec<String> = statement
                .column_names()
                .into_iter()
                .skip(1)
                .map(str::to_owned)
                .collect();
            let width = columns.len();
            let rows = statement
                .query_map([], |row| {
                    let rowid: i64 = row.get(0)?;
                    let values = (1..=width)
                        .map(|index| row.get::<_, Value>(index))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((rowid, values))
                })
                .unwrap()
                .collect::<Result<BTreeMap<_, _>, _>>()
                .unwrap();
            (table, TableRows { columns, rows })
        })
        .collect()
}

impl TableRows {
    /// The rowids of the seeded rows `rows` names, or why it names none.
    fn select(&self, Rows(column, values): Rows) -> Result<BTreeSet<i64>, String> {
        let index = self
            .columns
            .iter()
            .position(|name| name == column)
            .ok_or_else(|| format!("REACH names rows by `{column}`, which is not a column"))?;
        let mut selected = BTreeSet::new();
        for value in values {
            let matching: Vec<i64> = self
                .rows
                .iter()
                .filter(|(_, row)| row[index] == Value::Text((*value).to_owned()))
                .map(|(rowid, _)| *rowid)
                .collect();
            if matching.is_empty() {
                return Err(format!(
                    "REACH names the row whose `{column}` is `{value}`, and the seed holds none"
                ));
            }
            selected.extend(matching);
        }
        Ok(selected)
    }

    /// A row as a failure message names it: by the column `REACH` names its
    /// rows by, when there is one, else by rowid.
    fn label(&self, rowid: i64, column: Option<&str>) -> String {
        let value = column
            .and_then(|column| self.columns.iter().position(|name| name == column))
            .map(|index| &self.rows[&rowid][index]);
        match (column, value) {
            (Some(column), Some(Value::Text(value))) => format!("{column} = {value}"),
            _ => format!("rowid {rowid}"),
        }
    }
}

/// Why the change from `before` to `after` is not what `reach` says, or
/// `None` when it is exactly that.
///
/// Rows are matched by rowid, so a row deleted and written again counts as
/// one removed and one added.
fn departure(reach: Reach, before: &TableRows, after: &TableRows) -> Option<String> {
    let added = after
        .rows
        .keys()
        .filter(|rowid| !before.rows.contains_key(rowid))
        .count();
    let removed: BTreeSet<i64> = before
        .rows
        .keys()
        .filter(|rowid| !after.rows.contains_key(rowid))
        .copied()
        .collect();
    let mut changed: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for (rowid, after_values) in &after.rows {
        let Some(before_values) = before.rows.get(rowid) else {
            continue;
        };
        for (index, (was, is)) in before_values.iter().zip(after_values).enumerate() {
            if was != is {
                changed
                    .entry(*rowid)
                    .or_default()
                    .insert(before.columns[index].clone());
            }
        }
    }

    let (expected_removed, expected_changed, named_by) = match reach {
        Kept => (BTreeSet::new(), BTreeMap::new(), None),
        Removed => (before.rows.keys().copied().collect(), BTreeMap::new(), None),
        RemovedInPart(rows) => match before.select(rows) {
            Ok(selected) => (selected, BTreeMap::new(), Some(rows.0)),
            Err(problem) => return Some(problem),
        },
        Rewritten(columns, rows) => match before.select(rows) {
            Ok(selected) => (
                BTreeSet::new(),
                selected
                    .into_iter()
                    .map(|rowid| (rowid, column_set(columns)))
                    .collect(),
                Some(rows.0),
            ),
            Err(problem) => return Some(problem),
        },
    };
    if added == 0 && removed == expected_removed && changed == expected_changed {
        return None;
    }

    let mut did = Vec::new();
    if added > 0 {
        did.push(format!("added {added} row(s)"));
    }
    if !removed.is_empty() {
        let labels: Vec<String> = removed
            .iter()
            .map(|rowid| before.label(*rowid, named_by))
            .collect();
        did.push(format!("removed {labels:?}"));
    }
    for (rowid, columns) in &changed {
        did.push(format!(
            "rewrote {columns:?} on {}",
            before.label(*rowid, named_by)
        ));
    }
    if did.is_empty() {
        did.push("changed nothing".to_owned());
    }
    Some(format!(
        "REACH says {reach:?}, and the action {}",
        did.join("; ")
    ))
}

// ---------------------------------------------------------------------------
// 3 — The published list names what each action changes
// ---------------------------------------------------------------------------

/// `PRIVACY.md` says its list of destructive actions is "the whole of what it
/// reaches". So each action's bullet names every table `REACH` says the
/// action changes, as `table` or as `table.column`.
#[test]
fn the_privacy_document_names_every_table_each_action_changes() {
    let section = document_section("### What the app's destructive actions actually remove");
    for action in Action::ALL {
        let bullet = bullet(&section, action.privacy_bullet());
        let named: BTreeSet<String> = backticked(&bullet)
            .into_iter()
            .map(|span| span.split('.').next().unwrap_or_default().to_owned())
            .collect();
        let unnamed: Vec<&str> = REACH
            .iter()
            .filter(|(_, reach)| reach[action.index()] != Kept)
            .map(|(table, _)| *table)
            .filter(|table| !named.contains(*table))
            .collect();
        assert!(
            unnamed.is_empty(),
            "PRIVACY.md's bullet for {} does not name {unnamed:?}, which the action changes",
            action.name()
        );
    }
}

/// One bullet of a list: its first line and the lines that continue it.
fn bullet(section: &str, opening: &str) -> String {
    let mut lines = section
        .lines()
        .skip_while(|line| !line.starts_with(opening));
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("PRIVACY.md has no bullet opening `{opening}`"));
    let rest: Vec<&str> = lines
        .take_while(|line| !line.trim().is_empty() && !line.starts_with("- "))
        .collect();
    std::iter::once(first)
        .chain(rest)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text of one `### ` section of `PRIVACY.md`, up to the next heading.
fn document_section(heading: &str) -> String {
    let start = PRIVACY_DOCUMENT
        .find(heading)
        .unwrap_or_else(|| panic!("PRIVACY.md has no section `{heading}`"));
    let body = &PRIVACY_DOCUMENT[start + heading.len()..];
    let end = body.find("\n#").unwrap_or(body.len());
    body[..end].to_owned()
}

// ---------------------------------------------------------------------------
// 4 — The covered tables: closed classes, no free text
// ---------------------------------------------------------------------------

/// The classes `docs/field-inventory.md` may use. There is no free-text class,
/// and adding a class is a change to this list as well as to the document.
/// Every class is closed: a column of any of them must refuse a typed
/// sentence, and the values `ruled_out_by` its class.
const CLASSES: &[&str] = &["key", "choice", "flag", "singleton", "timestamp", "version"];

#[test]
fn the_inventory_uses_closed_classes_and_no_free_text() {
    let documented: BTreeSet<String> =
        markdown_table(INVENTORY, &["Class", "What the column may hold"])
            .iter()
            .map(|cells| single_backticked(&cells[0]))
            .collect();
    let expected: BTreeSet<String> = CLASSES.iter().map(|class| (*class).to_owned()).collect();
    assert_eq!(
        documented, expected,
        "docs/field-inventory.md's field classes are not CLASSES. A class is added in both, \
         and never one for free text"
    );

    let tables: BTreeSet<String> = inventory_tables()
        .into_iter()
        .map(|table| table.name)
        .collect();
    let columns = inventory_columns();
    let column_tables: BTreeSet<String> =
        columns.iter().map(|column| column.table.clone()).collect();
    assert_eq!(
        tables, column_tables,
        "docs/field-inventory.md lists columns for a different set of tables than it lists"
    );

    for column in &columns {
        let name = format!("{}.{}", column.table, column.column);
        assert!(
            CLASSES.contains(&column.class.as_str()),
            "`{name}` has class `{}`, which is not one of {CLASSES:?}",
            column.class
        );
        assert!(
            ["TEXT", "INTEGER"].contains(&column.sql_type.as_str()),
            "`{name}` has type `{}`; a covered column is TEXT or INTEGER",
            column.sql_type
        );
        if column.sql_type == "TEXT" {
            assert!(
                ["key", "choice"].contains(&column.class.as_str()),
                "`{name}` is TEXT with class `{}`. A covered TEXT column is a key or a choice, \
                 because anything else could hold free text",
                column.class
            );
        }
        if column.class == "choice" {
            assert!(
                column.values.len() >= 2,
                "`{name}` is a choice and lists {:?} as its values",
                column.values
            );
        }
    }
}

/// A `planned` table does not exist yet. A `migrated` one has exactly its
/// documented columns, and every one of them refuses a typed sentence.
#[test]
fn every_covered_table_is_planned_or_matches_its_inventory() {
    let scratch = ScratchDatabase::new();
    drop(SqlitePersistence::open(&scratch.path).unwrap());
    let connection = seeded_connection(&scratch);
    let present: BTreeSet<String> = table_names(&connection).into_iter().collect();
    let columns = inventory_columns();

    for table in inventory_tables() {
        match table.status {
            Status::Planned => assert!(
                !present.contains(&table.name),
                "`{}` exists in the migrated schema and docs/field-inventory.md still marks it \
                 planned. The migration that creates it marks it migrated in the same commit",
                table.name
            ),
            Status::Migrated => {
                let documented: Vec<&InventoryColumn> = columns
                    .iter()
                    .filter(|column| column.table == table.name)
                    .collect();
                let problems = column_problems(&connection, &table.name, &documented);
                assert!(
                    problems.is_empty(),
                    "`{}` does not match docs/field-inventory.md:\n  {}",
                    table.name,
                    problems.join("\n  ")
                );
            }
        }
    }
}

/// The checks above refuse a careless copy of `work_block_self_rating`, so a
/// pass against the real schema means something.
///
/// The copy has the usual mistakes: a text primary key that allows NULL and
/// references nothing, a choice with no CHECK, a timestamp with no CHECK, a
/// timestamp whose CHECK allows a fraction, a version whose CHECK allows 0,
/// and a column the inventory does not list.
#[test]
fn the_column_checks_refuse_a_careless_table() {
    let columns = inventory_columns();
    let documented: Vec<&InventoryColumn> = columns
        .iter()
        .filter(|column| column.table == "work_block_self_rating")
        .collect();
    let careless = probe_connection(
        "CREATE TABLE work_block_self_rating (
             block_id TEXT PRIMARY KEY,
             prompt_version INTEGER NOT NULL CHECK (typeof(prompt_version) = 'integer'),
             eligible_at INTEGER NOT NULL,
             shown_at INTEGER,
             answer TEXT,
             answered_at INTEGER,
             not_now_at INTEGER,
             expired_at INTEGER CHECK (expired_at >= 0),
             card_seen_before_answer INTEGER NOT NULL DEFAULT 0
                 CHECK (card_seen_before_answer IN (0, 1)),
             note TEXT
         );
         INSERT INTO work_block_self_rating(block_id, prompt_version, eligible_at)
             VALUES ('block-1', 1, 100);",
    );
    let problems = column_problems(&careless, "work_block_self_rating", &documented);
    for expected in [
        "`note` is not in the inventory",
        "`block_id` may be NULL",
        "`block_id` accepts a typed sentence",
        "`answer` accepts a typed sentence",
        "`shown_at` accepts a typed sentence",
        "`prompt_version` accepts `0`",
        "`expired_at` accepts `1.5`",
    ] {
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "the checks missed \"{expected}\" on a careless table. They reported: {problems:?}"
        );
    }
    assert!(
        !problems
            .iter()
            .any(|problem| problem.contains("`card_seen_before_answer`")),
        "the checks reported the one column the careless table gets right: {problems:?}"
    );
}

/// Every way `table` differs from its documented columns.
fn column_problems(
    connection: &Connection,
    table: &str,
    documented: &[&InventoryColumn],
) -> Vec<String> {
    let mut problems = Vec::new();
    let schema = table_info(connection, table);
    let single_column_key = schema.iter().filter(|column| column.primary_key).count() == 1;

    for column in &schema {
        if !documented.iter().any(|entry| entry.column == column.name) {
            problems.push(format!("`{}` is not in the inventory", column.name));
        }
    }
    for entry in documented {
        let Some(column) = schema.iter().find(|column| column.name == entry.column) else {
            problems.push(format!(
                "`{}` is in the inventory, not in the schema",
                entry.column
            ));
            continue;
        };
        if column.sql_type != entry.sql_type {
            problems.push(format!(
                "`{}` is {} in the schema and {} in the inventory",
                entry.column, column.sql_type, entry.sql_type
            ));
        }
        // A single-column INTEGER PRIMARY KEY is the rowid and is never NULL,
        // although `table_info` reports it as nullable.
        let rowid = column.primary_key && single_column_key && column.sql_type == "INTEGER";
        let nullable = !(column.not_null || rowid);
        if nullable && !entry.nullable {
            problems.push(format!(
                "`{}` may be NULL, and the inventory says it never is",
                entry.column
            ));
        }
        if !nullable && entry.nullable {
            problems.push(format!(
                "`{}` is NOT NULL, and the inventory says it may be NULL",
                entry.column
            ));
        }
    }

    let rows: i64 = connection
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
            row.get(0)
        })
        .unwrap();
    if rows == 0 {
        problems.push(format!(
            "`{table}` has no row to write to; give it one in the seed"
        ));
        return problems;
    }
    for entry in documented {
        if !schema.iter().any(|column| column.name == entry.column) {
            continue;
        }
        if write(connection, table, &entry.column, TYPED_SENTENCE) == Write::Accepted {
            problems.push(format!(
                "`{}` accepts a typed sentence, and its class ({}) is closed",
                entry.column, entry.class
            ));
        }
        for value in ruled_out_by(&entry.class) {
            if write(connection, table, &entry.column, value) == Write::Accepted {
                problems.push(format!(
                    "`{}` accepts `{value}`, which its class ({}) rules out",
                    entry.column, entry.class
                ));
            }
        }
        if entry.class == "choice" {
            for value in &entry.values {
                if write(connection, table, &entry.column, value) == Write::Refused {
                    problems.push(format!(
                        "`{}` refuses `{value}`, which the inventory lists",
                        entry.column
                    ));
                }
            }
        }
    }
    problems
}

#[derive(Debug, PartialEq, Eq)]
enum Write {
    Accepted,
    Refused,
}

/// Writes `value` into `column` of one row and undoes it, reporting whether
/// the database took it.
///
/// One row, because a primary key set to the same value on two rows is refused
/// as a duplicate, which would look like a closed column when it is not.
fn write(connection: &Connection, table: &str, column: &str, value: &str) -> Write {
    connection
        .execute_batch("SAVEPOINT field_inventory_probe")
        .unwrap();
    let result = connection.execute(
        &format!(
            "UPDATE \"{table}\" SET \"{column}\" = ?1 \
             WHERE rowid = (SELECT MIN(rowid) FROM \"{table}\")"
        ),
        [value],
    );
    connection
        .execute_batch("ROLLBACK TO field_inventory_probe; RELEASE field_inventory_probe")
        .unwrap();
    match result {
        Ok(1) => Write::Accepted,
        Ok(changed) => panic!("writing `{table}.{column}` changed {changed} rows, not one"),
        Err(rusqlite::Error::SqliteFailure(error, _))
            if matches!(
                error.code,
                ErrorCode::ConstraintViolation | ErrorCode::TypeMismatch
            ) =>
        {
            Write::Refused
        }
        Err(other) => panic!("writing `{table}.{column}` failed for another reason: {other}"),
    }
}

struct SchemaColumn {
    name: String,
    sql_type: String,
    not_null: bool,
    primary_key: bool,
}

fn table_info(connection: &Connection, table: &str) -> Vec<SchemaColumn> {
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap()
        .query_map([], |row| {
            Ok(SchemaColumn {
                name: row.get(1)?,
                sql_type: row.get::<_, String>(2)?.to_ascii_uppercase(),
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// An in-memory database holding `sql`, with foreign keys enforced as the
/// service enforces them.
fn probe_connection(sql: &str) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(sql).unwrap();
    connection
}

// ---------------------------------------------------------------------------
// 5 — The store for the question after a block writes its own tables only
// ---------------------------------------------------------------------------

/// The tables `SelfRatingRepo` writes.
const RATING_TABLES: [&str; 2] = ["rating_prompt_settings", "work_block_self_rating"];

/// Every write `SelfRatingRepo` has, run against a database holding a row in
/// every table, changes its own two tables and leaves every other table
/// exactly as it was. An answer must never reach the drift record: the offers,
/// the decision log, the demotion state, backoff, invitations or receipts.
#[test]
fn the_rating_store_writes_only_its_own_two_tables() {
    const ANSWERED: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a03";
    const EXPIRED: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a04";
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let connection = seeded_connection(&scratch);
    for block in [ANSWERED, EXPIRED] {
        connection
            .execute(
                "INSERT INTO work_block(block_id, phase, intensity, planned_duration_seconds,
                                        started_at, ended_at, total_paused_seconds,
                                        recovered_after_restart, intention_expires_at)
                 VALUES (?1, 'completed', 'medium', 1500, 1760003000, 1760004500, 0, 0,
                         1760089400)",
                [block],
            )
            .unwrap();
    }
    let ratings = persistence.self_rating_repo();
    let at = |seconds: i64| DateTime::<Utc>::from_timestamp(1_760_004_500 + seconds, 0).unwrap();

    let before = snapshot(&connection);
    let writes = [
        (
            "record_eligible",
            ratings.record_eligible(ANSWERED, 1, at(0)),
        ),
        ("mark_shown", ratings.mark_shown(ANSWERED, at(5))),
        ("record_not_now", ratings.record_not_now(ANSWERED, at(9))),
        (
            "record_answer",
            ratings.record_answer(ANSWERED, SelfRatingAnswer::LessThanHalf, true, at(60)),
        ),
        (
            "record_eligible",
            ratings.record_eligible(EXPIRED, 1, at(0)),
        ),
        ("expire", ratings.expire(EXPIRED, at(7_200))),
        ("set_enabled(false)", ratings.set_enabled(false, at(7_300))),
        ("set_enabled(true)", ratings.set_enabled(true, at(7_400))),
    ];
    for (write, result) in writes {
        assert!(
            result.unwrap(),
            "{write} changed nothing, so nothing below would mean anything"
        );
    }
    let after = snapshot(&connection);

    let mut reached = Vec::new();
    for (table, before_rows) in &before {
        let changed = departure(Kept, before_rows, &after[table]).is_some();
        if RATING_TABLES.contains(&table.as_str()) {
            assert!(
                changed,
                "the rating store's writes left `{table}` unchanged"
            );
        } else if changed {
            reached.push(table.clone());
        }
    }
    assert!(
        reached.is_empty(),
        "the store for the question after a block changed {reached:?}. It writes \
         {RATING_TABLES:?} and nothing else"
    );
}

/// The rules of the question after a block (`self_rating`), run against a
/// database holding a row in every table: the invitation's Not now, the
/// setting both ways, eligibility, a sighting, Not now, an answer through the
/// link, and an expiry found late. The question's two tables change; every
/// other table stays exactly as it was. Plan § 3.3's tripwire: the question
/// never writes an offer, a decision, demotion, backoff, an invitation,
/// receipts, a correction or an observation.
#[test]
fn the_question_s_rules_write_only_its_own_two_tables() {
    use velvt_service::work_block::{EndedBlock, RatingPromptSource};
    use velvt_shared_types::{WorkBlockPhase, WorkBlockRatingAnswer};

    const RATED: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a05";
    const FOUND_LATE: &str = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a06";
    // The seed's first block starts the period at 1760000000, so it ends at
    // 1761209600.
    const RATED_START: i64 = 1_761_300_000;
    const LATE_START: i64 = 1_761_400_000;
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let connection = seeded_connection(&scratch);
    for (block, start) in [(RATED, RATED_START), (FOUND_LATE, LATE_START)] {
        connection
            .execute(
                "INSERT INTO work_block(block_id, phase, intensity, planned_duration_seconds,
                                        started_at, ended_at, total_paused_seconds,
                                        recovered_after_restart, intention_expires_at)
                 VALUES (?1, 'completed', 'medium', 1800, ?2, ?2 + 1800, 0, 0, ?2 + 86400)",
                rusqlite::params![block, start],
            )
            .unwrap();
    }
    // The rated block holds an offer and a decision of its own, so a rule
    // that touched its block's drift record would change a row here.
    connection
        .execute_batch(&format!(
            "INSERT INTO work_block_intervention(block_id, offered_at, action_id,
                 anchor_category, switch_count, window_seconds, outcome, outcome_at,
                 salience)
             VALUES ('{RATED}', {offered}, 'protect_next_10', 'FOCUS_WORK', 3, 600,
                     'returned', {returned}, 'normal');
             INSERT INTO intervention_decision_log(decision_id, occurred_at, block_id,
                 policy_version, anchor_category, switch_count, elapsed_seconds,
                 remaining_seconds, gate_verdict, propensity, focus_state)
             VALUES ('decision-rated-block', {offered}, '{RATED}', 6, 'FOCUS_WORK', 3,
                     600, 1200, 'offered', 1.0, 'off');",
            offered = RATED_START + 600,
            returned = RATED_START + 700,
        ))
        .unwrap();
    // Start from an install that never answered the invitation.
    connection
        .execute("DELETE FROM rating_prompt_settings", [])
        .unwrap();
    let rating = RatingPromptManager::new(persistence.self_rating_repo());
    let at = |seconds: i64| DateTime::<Utc>::from_timestamp(seconds, 0).unwrap();
    let ended = |block: &str, start: i64| EndedBlock {
        block_id: block.to_owned(),
        phase: WorkBlockPhase::Completed,
        started_at: at(start),
        ended_at: at(start + 1_800),
        elapsed_seconds: 1_800,
    };
    let rated = Uuid::parse_str(RATED).unwrap();

    let before = snapshot(&connection);
    let declined = rating.set_enabled(false, at(RATED_START - 3_000)).unwrap();
    let turned_on = rating.set_enabled(true, at(RATED_START - 2_000)).unwrap();
    let asked = rating.surfaces_for(&ended(RATED, RATED_START), at(RATED_START + 1_810));
    let writes = [
        (
            "the invitation's Not now",
            !declined.enabled && declined.available,
        ),
        ("turning it on", turned_on.enabled),
        ("eligibility", asked.prompt.is_some()),
        (
            "a sighting",
            rating.record_shown(rated, at(RATED_START + 1_815)).unwrap(),
        ),
        (
            "Not now",
            rating
                .record_not_now(rated, at(RATED_START + 1_820))
                .unwrap(),
        ),
        (
            "an answer through the link",
            rating
                .record_answer(
                    rated,
                    WorkBlockRatingAnswer::LessThanHalf,
                    true,
                    at(RATED_START + 1_900),
                )
                .unwrap(),
        ),
        (
            "an expiry found late",
            rating
                .surfaces_for(&ended(FOUND_LATE, LATE_START), at(LATE_START + 20_000))
                .prompt
                .is_none(),
        ),
        (
            "turning it off",
            !rating
                .set_enabled(false, at(LATE_START + 20_100))
                .unwrap()
                .enabled,
        ),
    ];
    for (write, happened) in writes {
        assert!(
            happened,
            "{write} did not happen, so nothing below would mean anything"
        );
    }
    let after = snapshot(&connection);

    let mut reached = Vec::new();
    for (table, before_rows) in &before {
        let changed = departure(Kept, before_rows, &after[table]).is_some();
        if RATING_TABLES.contains(&table.as_str()) {
            assert!(changed, "the question's rules left `{table}` unchanged");
        } else if changed {
            reached.push(table.clone());
        }
    }
    assert!(
        reached.is_empty(),
        "the rules of the question after a block changed {reached:?}. They write \
         {RATING_TABLES:?} and nothing else"
    );
    let expired: Option<i64> = connection
        .query_row(
            "SELECT expired_at FROM work_block_self_rating WHERE block_id = ?1",
            [FOUND_LATE],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(expired, Some(LATE_START + 1_800 + 7_200));
}

// ---------------------------------------------------------------------------
// The inventory document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Planned,
    Migrated,
}

struct InventoryTable {
    name: String,
    status: Status,
    reach: Vec<Documented>,
}

struct InventoryColumn {
    table: String,
    column: String,
    sql_type: String,
    nullable: bool,
    class: String,
    values: Vec<String>,
}

fn inventory_tables() -> Vec<InventoryTable> {
    let mut header = vec!["Table", "Status", "Retention"];
    header.extend(Action::ALL.iter().map(|action| action.name()));
    let rows = markdown_table(INVENTORY, &header);
    assert!(!rows.is_empty(), "docs/field-inventory.md lists no table");
    rows.iter()
        .map(|cells| {
            let name = single_backticked(&cells[0]);
            let status = match cells[1].as_str() {
                "planned" => Status::Planned,
                "migrated" => Status::Migrated,
                other => panic!("`{name}` has status `{other}`, not planned or migrated"),
            };
            assert!(
                !cells[2].is_empty(),
                "`{name}` states no retention in docs/field-inventory.md"
            );
            let reach = cells[3..]
                .iter()
                .map(|cell| parse_reach(&name, cell))
                .collect();
            InventoryTable {
                name,
                status,
                reach,
            }
        })
        .collect()
}

fn parse_reach(table: &str, cell: &str) -> Documented {
    match cell {
        "kept" => Documented::Kept,
        "removed" => Documented::Removed,
        "removed in part" => Documented::RemovedInPart,
        _ if cell.starts_with("rewritten:") => {
            let columns: BTreeSet<String> = backticked(cell).into_iter().collect();
            assert!(!columns.is_empty(), "`{table}`: `{cell}` names no column");
            Documented::Rewritten(columns)
        }
        _ => panic!(
            "`{table}`: `{cell}` is not kept, removed, removed in part, or rewritten: `column`"
        ),
    }
}

fn inventory_columns() -> Vec<InventoryColumn> {
    markdown_table(
        INVENTORY,
        &["Table", "Column", "Type", "Null", "Class", "Holds"],
    )
    .iter()
    .map(|cells| {
        let table = single_backticked(&cells[0]);
        let column = single_backticked(&cells[1]);
        let nullable = match cells[3].as_str() {
            "yes" => true,
            "no" => false,
            other => panic!("`{table}.{column}` has Null `{other}`, not yes or no"),
        };
        InventoryColumn {
            values: if cells[4] == "choice" {
                backticked(&cells[5])
            } else {
                Vec::new()
            },
            sql_type: cells[2].clone(),
            class: cells[4].clone(),
            table,
            column,
            nullable,
        }
    })
    .collect()
}

/// The body rows of the Markdown table whose header row is exactly `header`.
fn markdown_table(document: &str, header: &[&str]) -> Vec<Vec<String>> {
    let mut lines = document
        .lines()
        .map(str::trim)
        .skip_while(|line| !(line.starts_with('|') && table_cells(line) == header));
    assert!(
        lines.next().is_some(),
        "no table headed {header:?} in the document"
    );
    let separator = lines.next().unwrap_or_default();
    assert!(
        separator.starts_with('|') && separator.contains("---"),
        "the table headed {header:?} has no separator row"
    );
    lines
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            let cells = table_cells(line);
            assert_eq!(
                cells.len(),
                header.len(),
                "a row of the table headed {header:?} has {} cells: {line}",
                cells.len()
            );
            cells
        })
        .collect()
}

fn table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn backticked(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn single_backticked(cell: &str) -> String {
    let spans = backticked(cell);
    assert_eq!(spans.len(), 1, "expected one backticked name in `{cell}`");
    spans.into_iter().next().unwrap()
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A second connection to the scratch database, after a row has been written
/// into every table the migrations leave empty. Foreign keys are enforced, so
/// every seeded row is one the schema allows.
fn seeded_connection(scratch: &ScratchDatabase) -> Connection {
    let connection = Connection::open(&scratch.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(&seed_sql()).unwrap();
    let empty: Vec<String> = table_names(&connection)
        .into_iter()
        .filter(|table| {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
                == 0
        })
        .collect();
    assert!(
        empty.is_empty(),
        "seed_sql leaves {empty:?} empty, so no action could be seen to reach them"
    );
    // A column that is NULL on every row would read the same after an action
    // cleared it, so the seed gives each nullable column a value somewhere.
    let always_null: Vec<String> = table_names(&connection)
        .into_iter()
        .flat_map(|table| {
            table_info(&connection, &table)
                .into_iter()
                .filter(|column| !column.not_null && !column.primary_key)
                .filter(|column| {
                    connection
                        .query_row(
                            &format!(
                                "SELECT COUNT(*) FROM \"{table}\" WHERE \"{}\" IS NOT NULL",
                                column.name
                            ),
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .unwrap()
                        == 0
                })
                .map(|column| format!("{table}.{}", column.name))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        always_null.is_empty(),
        "seed_sql leaves {always_null:?} NULL on every row, so an action that cleared \
         them would go unseen. Give each a value on at least one seeded row"
    );
    connection
}

/// One row in every table the migrations leave empty, a value in every
/// nullable column on at least one row, and more than one row where an action
/// treats rows differently:
///
/// - **Upload batches and their events.** There are two batches waiting to be
///   sent (one failed, one pending), a sent one and a rejected one, each with
///   an event. There is also an event waiting for a batch, and an event that
///   is local-only.
/// - **Invitations.** There is an open invitation and an answered one.
/// - **Decisions.** There is a decision logged in a block and one logged
///   without a block.
/// - **Display names.** Both `abstraction_map` rows carry a `display_name`, and
///   two events carry the `local_display_label` a name was applied to.
/// - **Blocks.** A second block is the recovery of the first.
/// - **Answers to the question after a block.** The first block's question
///   was answered after a Not now, and the second block's expired unanswered.
///   The answered row is the first, because the inventory checks write each
///   listed answer into the first row and the schema refuses an answer with no
///   `answered_at`.
///
/// `REACH` names the rows a partial reach touches by the ids given here.
/// `embedding_salt`, `stable_key_salt` and `schema_migration` are written by
/// the migrations and are not seeded. Nothing here is drawn from a real Mac.
fn seed_sql() -> String {
    let window = "a".repeat(64);
    let site_window = "9".repeat(64);
    let app = "b".repeat(64);
    let bundle = "c".repeat(64);
    let site = "d".repeat(64);
    let prompt = "e".repeat(64);
    let genesis = "0".repeat(64);
    let first_entry = "1".repeat(64);
    let second_entry = "2".repeat(64);
    let body = "f".repeat(64);
    let block = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a01";
    let recovered_block = "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a02";
    format!(
        "
INSERT INTO abstraction_map(key_hash, stable_id, label, category, taxonomy_version,
                            classification_tier, display_name, classification_status,
                            classification_confidence, classification_source)
    VALUES ('{window}', 'abs_seed_window', 'document:code', 'FOCUS_WORK', 'mvp-2',
            'seed', 'Seed editor', 'classified', 'high', 'seed'),
           ('{site_window}', 'abs_seed_site', 'web:unknown', 'UNLOGGED', 'mvp-2',
            'fallback', 'Seed forum', 'unclassified', 'none', 'fallback');
INSERT INTO antecedent_finding(finding_id, candidate_id, candidate_registry_version,
                               discovered_at, discovery_window_start, discovery_window_end,
                               support_episodes, effect_size, q_value, confirmed_at,
                               confirm_support_episodes, confirm_effect_size, state,
                               surfaced_at, retracted_at, retraction_reason,
                               user_disputed_at)
    VALUES ('finding-seed', 'hour_bin:morning', 1, 1760000000, '2026-09-01', '2026-09-28',
            3, 0.2, 0.5, NULL, NULL, NULL, 'candidate', NULL, NULL, NULL, NULL),
           ('finding-seed-retracted', 'hour_bin:evening', 1, 1757000000, '2025-08-01',
            '2025-08-28', 4, 0.3, 0.04, 1758000000, 3, 0.25, 'retracted', 1758000100,
            1759000000, 'user_disputed', 1758999000);
INSERT INTO upload_batch(batch_id, status, sent_at, attempt_count, next_attempt_at,
                         last_error_code, owner_user_id)
    VALUES ('batch-seed-waiting', 'failed', NULL, 2, 1760000600, 'host_unreachable', 'user-seed'),
           ('batch-seed-pending', 'pending', NULL, 0, 0, NULL, 'user-seed'),
           ('batch-seed-sent', 'sent', 1760000100, 1, 0, NULL, 'user-seed'),
           ('batch-seed-rejected', 'rejected', NULL, 1, 0, 'raw_field_rejected', 'user-seed');
INSERT INTO batch_event(batch_id, event_id, stable_id, label, category, taxonomy_version,
                        classification_tier, occurred_at, duration_seconds)
    VALUES ('batch-seed-waiting', 'event-seed-waiting', 'abs_seed_window', 'document:code',
            'FOCUS_WORK', 'mvp-2', 'seed', 1760000000, 60),
           ('batch-seed-pending', 'event-seed-pending', 'abs_seed_window', 'document:code',
            'FOCUS_WORK', 'mvp-2', 'seed', 1760000030, 30),
           ('batch-seed-sent', 'event-seed-sent', 'abs_seed_window', 'document:code',
            'FOCUS_WORK', 'mvp-2', 'seed', 1759999000, 60),
           ('batch-seed-rejected', 'event-seed-rejected', 'abs_seed_window', 'document:code',
            'FOCUS_WORK', 'mvp-2', 'seed', 1759998000, 60);
INSERT INTO raw_event_buffer(event_id, stable_id, label, local_display_label, category,
                             taxonomy_version, occurred_at, duration_seconds,
                             classification_tier, classification_status,
                             classification_confidence, classification_source,
                             local_name_suggestion, upload_eligible, app_stable_id,
                             app_bundle_stable_id, declared_app_category, document_type_ids,
                             site_stable_id, upload_owner_user_id)
    VALUES ('event-seed-waiting', 'abs_seed_window', 'document:code', 'Seed editor',
            'FOCUS_WORK', 'mvp-2', 1760000000, 60, 'seed', 'classified', 'high', 'seed',
            'Seed suggestion', 1, '{app}', '{bundle}', 'public.app-category.developer-tools',
            'public.source-code public.plain-text', NULL, 'user-seed'),
           ('event-seed-pending', 'abs_seed_window', 'document:code', NULL, 'FOCUS_WORK',
            'mvp-2', 1760000030, 30, 'seed', 'classified', 'high', 'seed', NULL, 1, '{app}',
            NULL, NULL, NULL, NULL, 'user-seed'),
           ('event-seed-sent', 'abs_seed_window', 'document:code', NULL, 'FOCUS_WORK',
            'mvp-2', 1759999000, 60, 'seed', 'classified', 'high', 'seed', NULL, 1, '{app}',
            NULL, NULL, NULL, NULL, 'user-seed'),
           ('event-seed-rejected', 'abs_seed_window', 'document:code', NULL, 'FOCUS_WORK',
            'mvp-2', 1759998000, 60, 'seed', 'classified', 'high', 'seed', NULL, 1, '{app}',
            NULL, NULL, NULL, NULL, 'user-seed'),
           ('event-seed-unbatched', 'abs_seed_window', 'document:code', NULL, 'FOCUS_WORK',
            'mvp-2', 1760000060, 60, 'seed', 'classified', 'high', 'seed', NULL, 1, '{app}',
            NULL, NULL, NULL, NULL, 'user-seed'),
           ('event-seed-local', 'abs_seed_site', 'web:unknown', 'Seed forum', 'UNLOGGED',
            'mvp-2', 1760000120, 60, 'fallback', 'unclassified', 'none', 'fallback', NULL, 0,
            NULL, NULL, NULL, NULL, '{site}', NULL);
INSERT INTO work_block(block_id, phase, intention, purpose, intensity,
                       planned_duration_seconds, started_at, paused_at, ended_at,
                       recovered_after_restart, recovery_of, intention_expires_at, origin)
    VALUES ('{block}', 'completed', 'Seed intention', 'deep_work', 'medium', 1500,
            1760000000, NULL, 1760001500, 0, NULL, 1760086400, 'invitation'),
           ('{recovered_block}', 'abandoned', NULL, NULL, 'light', 600, 1760002000,
            1760002300, 1760002400, 1, '{block}', 1760088400, 'manual');
INSERT INTO work_block_observation(block_id, occurred_at, ended_at, category,
                                   classification_status, classification_confidence)
    VALUES ('{block}', 1760000000, 1760000600, 'FOCUS_WORK', 'classified', 'high');
INSERT INTO work_block_result(block_id, payload) VALUES ('{block}', '{{}}');
INSERT INTO work_block_self_rating(block_id, prompt_version, eligible_at, shown_at, answer,
                                   answered_at, not_now_at, expired_at,
                                   card_seen_before_answer)
    VALUES ('{block}', 1, 1760001500, 1760001510, 'most', 1760001530, 1760001520, NULL, 1),
           ('{recovered_block}', 1, 1760002400, NULL, NULL, NULL, NULL, 1760009600, 0);
INSERT INTO rating_prompt_settings(id, enabled, first_opted_in_at, opted_in_at, opted_out_at)
    VALUES (1, 1, 1759970000, 1759990000, 1759980000);
INSERT INTO work_block_intervention(block_id, offered_at, action_id, anchor_category,
                                    switch_count, window_seconds, outcome, outcome_at,
                                    salience, card_seen_at, delivery_channel,
                                    delivery_reported_at)
    VALUES ('{block}', 1760000700, 'protect_next_10', 'FOCUS_WORK', 3, 600, 'was_focused',
            1760000760, 'normal', 1760000701, 'notification_posted', 1760000702);
INSERT INTO work_block_category_correction(block_id, category, counts_as_category,
                                           corrected_at)
    VALUES ('{block}', 'REFERENCE', 'FOCUS_WORK', 1760000800);
INSERT INTO block_antecedent(block_id, window_seconds, categories, switch_count,
                             dominant_category, dominant_dwell_seconds, day_type,
                             hour_bucket, is_first_block_of_day, antecedent_version)
    VALUES ('{block}', 900, '[\"FOCUS_WORK\"]', 1, 'FOCUS_WORK', 600, 'weekday', 9, 1, 1);
INSERT INTO intervention_decision_log(decision_id, occurred_at, block_id, policy_version,
                                      anchor_category, switch_count, elapsed_seconds,
                                      remaining_seconds, gate_verdict, propensity,
                                      anchor_seen_within_600s, outcome_at, focus_state)
    VALUES ('decision-seed-in-block', 1760000700, '{block}', 6, 'FOCUS_WORK', 3, 700, 800,
            'offered', 1.0, 1, 1760001300, 'off'),
           ('decision-seed-no-block', 1760000900, NULL, 6, NULL, 0, 0, 0,
            'abstained_no_anchor', 1.0, NULL, NULL, 'unknown');
INSERT INTO intervention_demotion_state(id, state, demoted_at, manual_reset_at,
                                        threshold_policy_version,
                                        repromotion_policy_version, updated_at)
    VALUES (1, 'active', 1759900000, 1759950000, 1, 1, 1760000000);
INSERT INTO category_prompt_entry(entry_key, first_listed_at, last_listed_at,
                                  acknowledged_at, notified_at)
    VALUES ('application:{app}', 1760000000, 1760090000, NULL, 1760000300),
           ('site:{site}', 1759900000, 1759990000, 1759995000, NULL);
INSERT INTO category_prompt_card_entry(prompt_id, entry_key, counted)
    VALUES ('{prompt}', 'application:{app}', 1);
INSERT INTO category_prompt_notification(local_date, posted_at, entry_count, policy_version,
                                         opened_at)
    VALUES ('2026-10-09', 1760000300, 1, 1, 1760000400);
INSERT INTO classification_telemetry(taxonomy_version, classification_tier, event_count)
    VALUES ('mvp-2', 'seed', 4);
INSERT INTO classifier_artifact_telemetry(artifact_version, classification_count)
    VALUES ('builtin-hash-v1', 4);
INSERT INTO egress_ledger(seq, recorded_at, method, endpoint, body_bytes, body_sha256,
                          body_redacted, bearer, prev_hash, entry_hash)
    VALUES (1, 1760000000, 'POST', '/v1/events/batches', 512, '{body}', 0, 1, '{genesis}',
            '{first_entry}'),
           (2, 1760000100, 'GET', '/v1/auth/session', 0, '{body}', 0, 1, '{first_entry}',
            '{second_entry}');
INSERT INTO egress_ledger_checkpoint(through_seq, through_hash, created_at)
    VALUES (1, '{first_entry}', 1760000200);
INSERT INTO explain_probe_week(week_start_local_date, taps, updated_at)
    VALUES ('2026-10-05', 1, 1760000000);
INSERT INTO focus_observer_state(id, utc_offset_seconds, updated_at)
    VALUES (1, 3600, 1760000000);
INSERT INTO focus_state_evidence(active, changed_at_bucket, local_hour, local_date,
                                 recorded_at)
    VALUES (1, 1759999800, 9, '2026-10-09', 1760000000);
INSERT INTO history_cache(date, payload, ttl) VALUES ('2026-10-09', '{{}}', 600);
INSERT INTO insight_cache(date, payload, ttl, not_found)
    VALUES ('2026-10-09', '{{}}', 1800, 0);
INSERT INTO initiation_invitation(invitation_id, offered_at, local_date, action_id,
                                  policy_version, backoff_policy_version, outcome,
                                  outcome_at)
    VALUES ('{ANSWERED_INVITATION}', 1759990000, '2026-10-09', 'soft_start_25', 1, 1,
            'accepted', 1759990060),
           ('{OPEN_INVITATION}', 1760090000, '2026-10-10', 'soft_start_25', 1, 1, 'offered',
            NULL);
INSERT INTO initiation_settings(id, invitations_enabled, updated_at)
    VALUES (1, 1, 1760000000);
INSERT INTO local_site_name(site_key_hash, host, last_seen_at)
    VALUES ('{site}', 'forum.example.org', 1760000120);
INSERT INTO out_of_block_run(started_at_bucket, duration_seconds, category,
                             classification_status, classification_confidence, local_hour,
                             local_date)
    VALUES (1759999800, 600, 'SOCIAL_FEED', 'classified', 'high', 9, '2026-10-09');
INSERT INTO persistence_migration_probe(marker) VALUES ('seed');
INSERT INTO personal_override(key_hash, category, activity_name, app_key_hash)
    VALUES ('{window}', 'FOCUS_WORK', 'Seed editor', '{app}');
INSERT INTO personal_app_override(app_key_hash, category, activity_name, correction_count,
                                  bundle_key_hash, app_only)
    VALUES ('{app}', 'FOCUS_WORK', 'Seed editor', 1, '{bundle}', 0);
INSERT INTO personal_site_override(site_key_hash, category, activity_name, correction_count)
    VALUES ('{site}', 'REFERENCE', 'Seed forum', 1);
INSERT INTO personal_semantic_prototype(key_hash, category, embedding, dimensions,
                                        correction_count)
    VALUES ('{window}', 'FOCUS_WORK', zeroblob(16), 4, 1);
INSERT INTO semantic_embedding_cache(key_hash, embedding, dimensions)
    VALUES ('{window}', zeroblob(16), 4);
INSERT INTO quiet_hours_offer_state(id, rule_version, triggered_at, offered_at, response,
                                    responded_at)
    VALUES (1, 1, 1760000000, 1760000100, 'declined', 1760000200);
INSERT INTO upload_host_backoff(host, attempt_count, next_attempt_at)
    VALUES ('api.example.test', 2, 1760000600);
INSERT INTO velvt_quiet_hours(id, start_local_minutes, end_local_minutes, rule_version,
                              configured_at)
    VALUES (1, 1320, 420, 1, 1760000000);
INSERT INTO weekly_digest(week_start_local_date, generated_at, blocks_declared,
                          blocks_completed, recoveries, wrong_interventions,
                          invitations_accepted, withheld, digest_version, delivered_at,
                          acknowledged_at)
    VALUES ('2026-09-28', 1760000000, 1, 1, 0, 0, 1, 0, 1, 1760000100, 1760000200);
"
    )
}

/// The router as `main.rs` builds it, for everything the four actions touch:
/// the correction store and upload queue, the work-block manager, Focus
/// evidence, invitations, receipts and the needs-a-category prompt. A cloud
/// that accepts the account deletion, and a signed-in session.
fn shipped_router(persistence: &SqlitePersistence) -> R7Router {
    let push = PushAdapter::new(PushQueue::new(50));
    let focus = FocusManager::new(persistence.focus_repo());
    let work_blocks = Arc::new(
        WorkBlockManager::new(persistence.work_block_repo())
            .with_focus_source(Arc::clone(&focus) as Arc<dyn FocusStateSource>),
    );
    let gates = RuntimeInvitationGates::new(Arc::clone(&focus), persistence.work_block_repo());
    let initiation = InitiationManager::new(
        persistence.initiation_repo(),
        Arc::clone(&gates) as Arc<dyn InvitationGates>,
    );
    let receipts = ReceiptsManager::new(
        persistence.receipts_repo(),
        Arc::clone(&gates) as Arc<dyn InvitationGates>,
    );
    let category_prompt = CategoryPromptManager::new(
        persistence.category_prompt_repo(),
        ListedCandidates::new(persistence.raw_event_repo()),
        gates as Arc<dyn InvitationGates>,
    );
    let ingestor: Arc<dyn EventIngestor> = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-field-inventory", 50, StdDuration::from_secs(3_600)),
        UploadCoordinator::new(
            persistence.upload_batch_repo(),
            FakeBatchUploader::default(),
            FakePrivacyAlertSink::default(),
        ),
    )));
    let account = Arc::new(AccountAuthService::new(
        Arc::new(AcceptingCloud),
        Arc::new(AcceptingCloud),
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Authenticated {
            device_id: "device-field-inventory".into(),
        })),
    ));
    let (_sender, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: "device-field-inventory".into(),
    });

    R7Router::new(
        Arc::new(FakeCacheManager::new()),
        Arc::new(
            AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap(),
        ),
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
    .with_classification_corrections(
        persistence.abstraction_map_repo(),
        persistence.upload_batch_repo(),
        Arc::new(AcceptingCloud),
    )
    .with_work_blocks(work_blocks, Arc::clone(&push))
    .with_focus(focus)
    .with_initiation(initiation)
    .with_receipts(receipts)
    .with_category_prompt(category_prompt)
    .with_auth_state(auth_state)
    .with_delivery_push(push)
}

/// Answers every request `202 Accepted`, which is what the account-deletion
/// endpoint answers when it accepts. No other request is made by these
/// actions with no session tokens held.
struct AcceptingCloud;

impl HttpClient for AcceptingCloud {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async {
            Ok(HttpResponse {
                status: 202,
                error_code: None,
                tokens: None,
                retry_after: None,
                message: None,
                raw_body: None,
                user_id: None,
                device_id: None,
            })
        })
    }
}

fn table_names(connection: &Connection) -> Vec<String> {
    connection
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// A database file in its own directory, removed when the test ends.
struct ScratchDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl ScratchDatabase {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("velvt-field-inventory-{}", Uuid::new_v4()));
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
