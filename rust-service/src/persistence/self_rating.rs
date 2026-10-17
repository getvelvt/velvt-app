//! The optional question after a block (migration 0048): one row per block
//! that became eligible for it, in `work_block_self_rating`, and whether the
//! question is asked, in `rating_prompt_settings`.
//!
//! This store writes those two tables and no other.
//! `tests/field_inventory.rs` holds it to that against a database with a row
//! in every table. It decides nothing: when a block is eligible, when the
//! question is on screen and when it expires are the caller's facts to report.
//! What it enforces are the rules of the record itself:
//!
//! - A block is eligible once. A second eligibility changes nothing.
//! - An answer is final, and it is never given after the question expired.
//! - Not now, closing and silence are never an answer. Not now and expiry are
//!   their own timestamps, and `answer` stays NULL.
//! - The first time each thing happened wins. A later report of the same
//!   thing changes nothing.
//! - An answer and a Not now are given on screen. If no sighting was reported
//!   before one of them, its time is the sighting.
//! - The result card appears after Not now, so an answer that follows a Not
//!   now was given with the card seen.
//! - The first time the question was turned on is kept. Turning it off and on
//!   again never changes it.
//! - Not now on the one-time invitation is recorded once, on an install that
//!   never turned the question on, as the off row with its time.
//!
//! It also reads, and never writes, the two facts `crate::self_rating`
//! places a block in time by: when a block ended and when the block after it
//! started, and when the Mac's drift policy v6 primary period started.

use super::sqlite::{optional_timestamp_from_row, timestamp_from_row};
use super::{PersistenceError, SqlitePersistence};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

/// One answer to "How much of this block was on what you meant to do?", from
/// the closed list migration 0048's CHECK allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfRatingAnswer {
    /// "All or nearly all"
    All,
    /// "Most"
    Most,
    /// "About half"
    Half,
    /// "Less than half"
    LessThanHalf,
    /// "Little or none"
    Little,
}

impl SelfRatingAnswer {
    /// Every answer, from the most of the block to the least.
    pub const EVERY: [Self; 5] = [
        Self::All,
        Self::Most,
        Self::Half,
        Self::LessThanHalf,
        Self::Little,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Most => "most",
            Self::Half => "half",
            Self::LessThanHalf => "less_than_half",
            Self::Little => "little",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        Self::EVERY
            .into_iter()
            .find(|answer| answer.as_str() == value)
    }
}

/// One row of `work_block_self_rating`: what happened to the question about
/// one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfRating {
    pub block_id: String,
    /// The version of the question's wording the block was offered under.
    pub prompt_version: u32,
    /// The block's end, when the question became available. The row may be
    /// written later, when the service first sees the ended block (after
    /// sleep or a restart), but this is always the end.
    pub eligible_at: DateTime<Utc>,
    /// When the question was first on screen. `None` if it never was. An
    /// answer or a Not now with no sighting before it sets it to its own time.
    pub shown_at: Option<DateTime<Utc>>,
    /// `None` if there is no answer. Not now and expiry are never an answer.
    pub answer: Option<SelfRatingAnswer>,
    /// `Some` exactly when `answer` is (the schema's CHECK).
    pub answered_at: Option<DateTime<Utc>>,
    pub not_now_at: Option<DateTime<Utc>>,
    /// When the question stopped being available without an answer: always
    /// the deadline itself, though the row may be stamped later, when the
    /// service next builds an ended block's snapshot or takes a report.
    pub expired_at: Option<DateTime<Utc>>,
    /// The answer was given after the block's result card had been on screen.
    /// Always true for an answer after a Not now, and false with no answer.
    pub card_seen_before_answer: bool,
}

impl SelfRating {
    /// Whether the question can still be answered: it has neither an answer
    /// nor an expiry. A Not now leaves it open.
    pub fn is_open(&self) -> bool {
        self.answer.is_none() && self.expired_at.is_none()
    }
}

/// The question's on/off switch. The default, and the value with no row, is
/// off: the question is asked only after the person turns it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RatingPromptSettings {
    pub enabled: bool,
    /// The first time the question was ever turned on. Turning it off and on
    /// again never changes it. The export's `rating_prompt_enabled_at`.
    pub first_opted_in_at: Option<DateTime<Utc>>,
    /// The last time the question was turned on.
    pub opted_in_at: Option<DateTime<Utc>>,
    /// The last time the question was turned off.
    pub opted_out_at: Option<DateTime<Utc>>,
}

/// When a block started and ended and when the block after it started, read
/// from `work_block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatedBlockBounds {
    pub started_at: DateTime<Utc>,
    /// `None` while the block has not ended.
    pub ended_at: Option<DateTime<Utc>>,
    /// The start of the block created next, in the order blocks were created,
    /// which is the order `WorkBlockRepo::latest` reads. `None` while no block
    /// has followed it.
    pub next_block_started_at: Option<DateTime<Utc>>,
}

/// Storage for the question after a block. Writes `work_block_self_rating`
/// and `rating_prompt_settings`, and nothing else.
///
/// Every write returns whether it changed a row. `false` means the record
/// already said what the write would have said, or the write would have broken
/// one of the module's rules, and nothing was changed.
pub trait SelfRatingRepo: Send + Sync {
    /// Records that `block_id` became eligible for the question at `at`,
    /// under `prompt_version`. The row is written now, with nothing shown, so
    /// a question that is never on screen is still a recorded missing answer.
    ///
    /// A block is eligible once: a second call changes nothing. A block that
    /// is not on record is refused by the foreign key, as an error.
    fn record_eligible(
        &self,
        block_id: &str,
        prompt_version: u32,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError>;

    /// The row for `block_id`, if the block was ever eligible.
    fn rating(&self, block_id: &str) -> Result<Option<SelfRating>, PersistenceError>;

    /// Every row that can still be answered, oldest eligibility first.
    fn open_ratings(&self) -> Result<Vec<SelfRating>, PersistenceError>;

    /// Records that the question was on screen at `at`. The first sighting
    /// wins, and only an open question is marked. An answer or a Not now
    /// already counts as a sighting.
    fn mark_shown(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError>;

    /// Records `answer` at `at`. Only an open question takes an answer, so an
    /// answer is never replaced and never follows expiry. A Not now before it
    /// stays recorded beside it, and makes `card_seen_before_answer` true
    /// whatever the caller passed, because the card appears after Not now. With
    /// no sighting recorded, `at` is also the sighting.
    fn record_answer(
        &self,
        block_id: &str,
        answer: SelfRatingAnswer,
        card_seen_before_answer: bool,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError>;

    /// Records a tap on Not now at `at`. It is not an answer, and the question
    /// stays open. The first tap wins, and only an open question is marked.
    /// With no sighting recorded, `at` is also the sighting.
    fn record_not_now(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError>;

    /// Records that the question stopped being available at `at`. Only an
    /// open question expires: an answered one never does.
    fn expire(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError>;

    /// Whether the question is asked, when it was first turned on, and when it
    /// was last turned on and off.
    fn settings(&self) -> Result<RatingPromptSettings, PersistenceError>;

    /// Turns the question on or off at `at`. Turning it on stamps
    /// `opted_in_at`, and `first_opted_in_at` the first time only. Turning it
    /// off stamps `opted_out_at`. Setting the state it already has changes
    /// nothing, so the stamps say when the state changed.
    fn set_enabled(&self, enabled: bool, at: DateTime<Utc>) -> Result<bool, PersistenceError>;

    /// Records Not now on the one-time invitation to turn the question on, at
    /// `at`: the off row, with `opted_out_at` as its time and no opt-in. Only
    /// an install with no row is changed. One that ever turned the question
    /// on, or answered the invitation before, keeps its record.
    fn decline_opt_in(&self, at: DateTime<Utc>) -> Result<bool, PersistenceError>;

    /// Reads, never writes: when `block_id` started and ended and when the
    /// next block started. `None` for a block not on record.
    fn block_bounds(&self, block_id: &str) -> Result<Option<RatedBlockBounds>, PersistenceError>;

    /// Reads, never writes: the start of the Mac's primary period for drift
    /// policy `policy_version`. That is the `started_at` of its first block
    /// that started after its last decision row with an older policy version
    /// (the upgrade cut), or of its first block if it has no such row, as item
    /// 1 of the draft v6 primary-period note and `scripts/analyze_ratings.py`'s
    /// `v6_period_start` define it. `None` when no block started after the
    /// cut.
    fn primary_period_start(
        &self,
        policy_version: u32,
    ) -> Result<Option<DateTime<Utc>>, PersistenceError>;
}

pub(super) struct SqliteSelfRatingRepo(pub(super) SqlitePersistence);

const RATING_COLUMNS: &str = "block_id, prompt_version, eligible_at, shown_at, answer, \
     answered_at, not_now_at, expired_at, card_seen_before_answer";

/// The condition of a question that can still be answered.
const OPEN: &str = "answer IS NULL AND expired_at IS NULL";

impl SelfRatingRepo for SqliteSelfRatingRepo {
    fn record_eligible(
        &self,
        block_id: &str,
        prompt_version: u32,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let inserted = connection.execute(
            "INSERT INTO work_block_self_rating(block_id, prompt_version, eligible_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(block_id) DO NOTHING",
            params![block_id, i64::from(prompt_version), at.timestamp()],
        )?;
        Ok(inserted == 1)
    }

    fn rating(&self, block_id: &str) -> Result<Option<SelfRating>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                &format!("SELECT {RATING_COLUMNS} FROM work_block_self_rating WHERE block_id = ?1"),
                [block_id],
                rating_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn open_ratings(&self) -> Result<Vec<SelfRating>, PersistenceError> {
        let connection = self.0.connection()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {RATING_COLUMNS} FROM work_block_self_rating
             WHERE {OPEN}
             ORDER BY eligible_at, block_id"
        ))?;
        let rows = statement.query_map([], rating_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn mark_shown(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        update_open(
            &connection,
            "shown_at = ?2",
            "shown_at IS NULL",
            block_id,
            at,
        )
    }

    fn record_answer(
        &self,
        block_id: &str,
        answer: SelfRatingAnswer,
        card_seen_before_answer: bool,
        at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let updated = connection.execute(
            &format!(
                "UPDATE work_block_self_rating
                 SET answer = ?2, answered_at = ?3, shown_at = COALESCE(shown_at, ?3),
                     card_seen_before_answer = (?4 OR not_now_at IS NOT NULL)
                 WHERE block_id = ?1 AND {OPEN}"
            ),
            params![
                block_id,
                answer.as_str(),
                at.timestamp(),
                i64::from(card_seen_before_answer)
            ],
        )?;
        Ok(updated == 1)
    }

    fn record_not_now(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        update_open(
            &connection,
            "not_now_at = ?2, shown_at = COALESCE(shown_at, ?2)",
            "not_now_at IS NULL",
            block_id,
            at,
        )
    }

    fn expire(&self, block_id: &str, at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        update_open(&connection, "expired_at = ?2", "1", block_id, at)
    }

    fn settings(&self) -> Result<RatingPromptSettings, PersistenceError> {
        let connection = self.0.connection()?;
        settings(&connection)
    }

    fn set_enabled(&self, enabled: bool, at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let mut connection = self.0.connection()?;
        let transaction = connection.transaction()?;
        if settings(&transaction)?.enabled == enabled {
            return Ok(false);
        }
        if enabled {
            transaction.execute(
                "INSERT INTO rating_prompt_settings(id, enabled, first_opted_in_at, opted_in_at)
                 VALUES (1, 1, ?1, ?1)
                 ON CONFLICT(id) DO UPDATE SET
                    enabled = 1,
                    first_opted_in_at = COALESCE(
                        rating_prompt_settings.first_opted_in_at,
                        excluded.first_opted_in_at
                    ),
                    opted_in_at = excluded.opted_in_at",
                [at.timestamp()],
            )?;
        } else {
            // Off with no row is the default, returned above, so the row is
            // there to turn off.
            transaction.execute(
                "UPDATE rating_prompt_settings SET enabled = 0, opted_out_at = ?1 WHERE id = 1",
                [at.timestamp()],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }

    fn decline_opt_in(&self, at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let connection = self.0.connection()?;
        let inserted = connection.execute(
            "INSERT INTO rating_prompt_settings(id, enabled, opted_out_at)
             VALUES (1, 0, ?1)
             ON CONFLICT(id) DO NOTHING",
            [at.timestamp()],
        )?;
        Ok(inserted == 1)
    }

    fn block_bounds(&self, block_id: &str) -> Result<Option<RatedBlockBounds>, PersistenceError> {
        let connection = self.0.connection()?;
        connection
            .query_row(
                "SELECT block.started_at, block.ended_at,
                        (SELECT next.started_at FROM work_block AS next
                         WHERE next.rowid > block.rowid
                         ORDER BY next.rowid LIMIT 1)
                 FROM work_block AS block WHERE block.block_id = ?1",
                [block_id],
                |row| {
                    Ok(RatedBlockBounds {
                        started_at: timestamp_from_row(row, 0)?,
                        ended_at: optional_timestamp_from_row(row, 1)?,
                        next_block_started_at: optional_timestamp_from_row(row, 2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn primary_period_start(
        &self,
        policy_version: u32,
    ) -> Result<Option<DateTime<Utc>>, PersistenceError> {
        let connection = self.0.connection()?;
        // Not a hot path: it runs only while the snapshot of an ended block is
        // built and the block has no row yet, or the setting is read.
        connection
            .query_row(
                "SELECT MIN(started_at) FROM work_block
                 WHERE started_at > COALESCE(
                     (SELECT MAX(occurred_at) FROM intervention_decision_log
                      WHERE policy_version < ?1),
                     -9223372036854775808
                 )",
                [i64::from(policy_version)],
                |row| optional_timestamp_from_row(row, 0),
            )
            .map_err(Into::into)
    }
}

/// Sets one timestamp on `block_id`'s row when the question is open and
/// `unset` holds. The SQL fragments are this module's constants, never input.
fn update_open(
    connection: &Connection,
    assignment: &str,
    unset: &str,
    block_id: &str,
    at: DateTime<Utc>,
) -> Result<bool, PersistenceError> {
    let updated = connection.execute(
        &format!(
            "UPDATE work_block_self_rating SET {assignment}
             WHERE block_id = ?1 AND {OPEN} AND {unset}"
        ),
        params![block_id, at.timestamp()],
    )?;
    Ok(updated == 1)
}

fn settings(connection: &Connection) -> Result<RatingPromptSettings, PersistenceError> {
    let stored = connection
        .query_row(
            "SELECT enabled, first_opted_in_at, opted_in_at, opted_out_at
             FROM rating_prompt_settings WHERE id = 1",
            [],
            |row| {
                Ok(RatingPromptSettings {
                    enabled: row.get::<_, i64>(0)? != 0,
                    first_opted_in_at: optional_timestamp_from_row(row, 1)?,
                    opted_in_at: optional_timestamp_from_row(row, 2)?,
                    opted_out_at: optional_timestamp_from_row(row, 3)?,
                })
            },
        )
        .optional()?;
    Ok(stored.unwrap_or_default())
}

fn rating_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SelfRating> {
    let answer = row
        .get::<_, Option<String>>(4)?
        .map(|value| SelfRatingAnswer::from_db_value(&value).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()?;
    Ok(SelfRating {
        block_id: row.get(0)?,
        prompt_version: row.get(1)?,
        eligible_at: timestamp_from_row(row, 2)?,
        shown_at: optional_timestamp_from_row(row, 3)?,
        answer,
        answered_at: optional_timestamp_from_row(row, 5)?,
        not_now_at: optional_timestamp_from_row(row, 6)?,
        expired_at: optional_timestamp_from_row(row, 7)?,
        card_seen_before_answer: row.get::<_, i64>(8)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{RatingPromptSettings, SelfRatingAnswer, SelfRatingRepo};
    use crate::persistence::SqlitePersistence;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;

    /// 2027-01-15T08:00:00Z, the anchor the initiation tests use.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn store(blocks: &[&str]) -> (SqlitePersistence, Arc<dyn SelfRatingRepo>) {
        let database = SqlitePersistence::open_in_memory().unwrap();
        {
            let connection = database.connection().unwrap();
            for block in blocks {
                connection
                    .execute(
                        "INSERT INTO work_block(
                            block_id, phase, intensity, planned_duration_seconds, started_at,
                            total_paused_seconds, recovered_after_restart,
                            intention_expires_at, updated_at
                         ) VALUES (?1, 'completed', 'medium', 1800, 0, 0, 0, 0, 0)",
                        [block],
                    )
                    .unwrap();
            }
        }
        let repo = database.self_rating_repo();
        (database, repo)
    }

    fn row_count(database: &SqlitePersistence, table: &str) -> i64 {
        database
            .connection()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// The row is written at eligibility with nothing shown, so a question
    /// that never reached the screen is a recorded missing answer. A block is
    /// eligible once.
    #[test]
    fn a_block_is_eligible_once_and_starts_as_a_missing_answer() {
        let (_database, ratings) = store(&["block-a"]);
        assert!(ratings.record_eligible("block-a", 1, at(0)).unwrap());
        assert!(!ratings.record_eligible("block-a", 2, at(60)).unwrap());

        let rating = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(
            rating.prompt_version, 1,
            "the second eligibility changed it"
        );
        assert_eq!(rating.eligible_at, at(0));
        assert_eq!(rating.shown_at, None);
        assert_eq!(rating.answer, None);
        assert_eq!(rating.answered_at, None);
        assert_eq!(rating.not_now_at, None);
        assert_eq!(rating.expired_at, None);
        assert!(!rating.card_seen_before_answer);
        assert!(rating.is_open());
        assert_eq!(ratings.rating("block-b").unwrap(), None);
    }

    /// A rating is always about a block on record.
    #[test]
    fn a_rating_needs_a_block_on_record() {
        let (database, ratings) = store(&[]);
        assert!(ratings.record_eligible("no-such-block", 1, at(0)).is_err());
        assert_eq!(row_count(&database, "work_block_self_rating"), 0);
    }

    /// Not now is not an answer. The question stays open, the answer stays
    /// missing, and an answer given later through the link sits beside it.
    #[test]
    fn not_now_is_never_an_answer() {
        let (_database, ratings) = store(&["block-a"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings.mark_shown("block-a", at(5)).unwrap());
        assert!(ratings.record_not_now("block-a", at(9)).unwrap());

        let after_not_now = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(after_not_now.answer, None);
        assert_eq!(after_not_now.answered_at, None);
        assert_eq!(after_not_now.not_now_at, Some(at(9)));
        assert!(after_not_now.is_open());
        assert_eq!(ratings.open_ratings().unwrap(), vec![after_not_now]);

        assert!(ratings
            .record_answer("block-a", SelfRatingAnswer::Half, true, at(300))
            .unwrap());
        let answered = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(answered.answer, Some(SelfRatingAnswer::Half));
        assert_eq!(answered.answered_at, Some(at(300)));
        assert!(answered.card_seen_before_answer);
        assert_eq!(answered.not_now_at, Some(at(9)), "the Not now was erased");
        assert_eq!(answered.shown_at, Some(at(5)));
    }

    /// The first time each thing happened is what the record keeps: the popover
    /// reopening is the same question on screen, and a second Not now is the
    /// same decline.
    #[test]
    fn the_first_sighting_and_the_first_not_now_win() {
        let (_database, ratings) = store(&["block-a"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings.mark_shown("block-a", at(5)).unwrap());
        assert!(!ratings.mark_shown("block-a", at(50)).unwrap());
        assert!(ratings.record_not_now("block-a", at(9)).unwrap());
        assert!(!ratings.record_not_now("block-a", at(90)).unwrap());

        let rating = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(rating.shown_at, Some(at(5)));
        assert_eq!(rating.not_now_at, Some(at(9)));
    }

    /// An answer is final. Nothing after it changes the row: not a second
    /// answer, a late sighting, a Not now, or the expiry that would have come.
    #[test]
    fn an_answer_is_final() {
        let (_database, ratings) = store(&["block-a"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings
            .record_answer("block-a", SelfRatingAnswer::Most, false, at(10))
            .unwrap());
        let answered = ratings.rating("block-a").unwrap().unwrap();

        assert!(!ratings
            .record_answer("block-a", SelfRatingAnswer::Little, true, at(20))
            .unwrap());
        assert!(!ratings.mark_shown("block-a", at(30)).unwrap());
        assert!(!ratings.record_not_now("block-a", at(40)).unwrap());
        assert!(!ratings.expire("block-a", at(7_200)).unwrap());

        assert_eq!(ratings.rating("block-a").unwrap().unwrap(), answered);
        assert!(!answered.is_open());
        assert!(ratings.open_ratings().unwrap().is_empty());
    }

    /// An expired question takes nothing more, and its answer stays missing.
    #[test]
    fn an_expired_question_takes_no_answer() {
        let (_database, ratings) = store(&["block-a"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings.expire("block-a", at(7_200)).unwrap());
        assert!(!ratings.expire("block-a", at(7_300)).unwrap());
        assert!(!ratings
            .record_answer("block-a", SelfRatingAnswer::All, false, at(7_400))
            .unwrap());
        assert!(!ratings.mark_shown("block-a", at(7_500)).unwrap());
        assert!(!ratings.record_not_now("block-a", at(7_600)).unwrap());

        let rating = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(rating.expired_at, Some(at(7_200)));
        assert_eq!(rating.answer, None);
        assert_eq!(rating.shown_at, None);
        assert_eq!(rating.not_now_at, None);
        assert!(!rating.is_open());
    }

    /// A write about a block that was never eligible changes nothing.
    #[test]
    fn a_block_that_was_never_eligible_takes_nothing() {
        let (database, ratings) = store(&["block-a"]);
        assert!(!ratings.mark_shown("block-a", at(5)).unwrap());
        assert!(!ratings
            .record_answer("block-a", SelfRatingAnswer::Most, false, at(10))
            .unwrap());
        assert!(!ratings.record_not_now("block-a", at(15)).unwrap());
        assert!(!ratings.expire("block-a", at(20)).unwrap());
        assert_eq!(row_count(&database, "work_block_self_rating"), 0);
    }

    /// Open questions come back oldest first, without the answered and the
    /// expired ones.
    #[test]
    fn open_ratings_are_the_unanswered_unexpired_rows_oldest_first() {
        let (_database, ratings) = store(&["answered", "expired", "later", "earlier"]);
        ratings.record_eligible("answered", 1, at(0)).unwrap();
        ratings.record_eligible("expired", 1, at(10)).unwrap();
        ratings.record_eligible("later", 1, at(30)).unwrap();
        ratings.record_eligible("earlier", 1, at(20)).unwrap();
        ratings
            .record_answer("answered", SelfRatingAnswer::All, false, at(40))
            .unwrap();
        ratings.expire("expired", at(50)).unwrap();
        ratings.record_not_now("later", at(60)).unwrap();

        let open: Vec<String> = ratings
            .open_ratings()
            .unwrap()
            .into_iter()
            .map(|rating| rating.block_id)
            .collect();
        assert_eq!(open, vec!["earlier".to_owned(), "later".to_owned()]);
    }

    /// Every answer the type has is one the schema accepts, and reads back as
    /// itself.
    #[test]
    fn every_answer_is_stored_and_read_back() {
        let blocks: Vec<String> = SelfRatingAnswer::EVERY
            .iter()
            .map(|answer| format!("block-{}", answer.as_str()))
            .collect();
        let block_ids: Vec<&str> = blocks.iter().map(String::as_str).collect();
        let (_database, ratings) = store(&block_ids);
        for (answer, block) in SelfRatingAnswer::EVERY.iter().zip(&block_ids) {
            ratings.record_eligible(block, 1, at(0)).unwrap();
            assert!(ratings.record_answer(block, *answer, false, at(1)).unwrap());
            assert_eq!(
                ratings.rating(block).unwrap().unwrap().answer,
                Some(*answer)
            );
            assert_eq!(
                SelfRatingAnswer::from_db_value(answer.as_str()),
                Some(*answer)
            );
        }
        assert_eq!(SelfRatingAnswer::from_db_value("none"), None);
    }

    /// An answer is given on screen. When it arrives before any report that the
    /// question was shown, its own time is the sighting, so the row never says
    /// "answered, never shown". The late report then changes nothing.
    #[test]
    fn an_answer_with_no_sighting_before_it_is_the_sighting() {
        let (_database, ratings) = store(&["block-a", "block-b"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings
            .record_answer("block-a", SelfRatingAnswer::Most, false, at(10))
            .unwrap());
        assert!(!ratings.mark_shown("block-a", at(11)).unwrap());
        let answered = ratings.rating("block-a").unwrap().unwrap();
        assert_eq!(answered.shown_at, Some(at(10)));
        assert_eq!(answered.answered_at, Some(at(10)));

        // A sighting reported first stays the sighting.
        ratings.record_eligible("block-b", 1, at(0)).unwrap();
        assert!(ratings.mark_shown("block-b", at(4)).unwrap());
        assert!(ratings
            .record_answer("block-b", SelfRatingAnswer::Most, false, at(10))
            .unwrap());
        assert_eq!(
            ratings.rating("block-b").unwrap().unwrap().shown_at,
            Some(at(4))
        );
    }

    /// Not now is tapped on screen too. With no sighting before it, its time
    /// is the sighting, and an answer after it keeps that sighting.
    #[test]
    fn a_not_now_with_no_sighting_before_it_is_the_sighting() {
        let (_database, ratings) = store(&["block-a"]);
        ratings.record_eligible("block-a", 1, at(0)).unwrap();
        assert!(ratings.record_not_now("block-a", at(9)).unwrap());
        assert!(!ratings.mark_shown("block-a", at(12)).unwrap());
        assert_eq!(
            ratings.rating("block-a").unwrap().unwrap().shown_at,
            Some(at(9))
        );

        ratings
            .record_answer("block-a", SelfRatingAnswer::Little, true, at(300))
            .unwrap();
        assert_eq!(
            ratings.rating("block-a").unwrap().unwrap().shown_at,
            Some(at(9))
        );
    }

    /// The card appears after Not now, so an answer after a Not now was given
    /// with the card seen, whatever the caller reports. Without a Not now the
    /// caller's report stands.
    #[test]
    fn an_answer_after_not_now_was_given_with_the_card_seen() {
        let (_database, ratings) = store(&["after-not-now", "card-unseen", "card-seen"]);
        for block in ["after-not-now", "card-unseen", "card-seen"] {
            ratings.record_eligible(block, 1, at(0)).unwrap();
            ratings.mark_shown(block, at(5)).unwrap();
        }
        ratings.record_not_now("after-not-now", at(9)).unwrap();
        for (block, card_seen) in [
            ("after-not-now", false),
            ("card-unseen", false),
            ("card-seen", true),
        ] {
            assert!(ratings
                .record_answer(block, SelfRatingAnswer::Half, card_seen, at(60))
                .unwrap());
        }
        let card = |block| {
            ratings
                .rating(block)
                .unwrap()
                .unwrap()
                .card_seen_before_answer
        };
        assert!(
            card("after-not-now"),
            "an answer after Not now was stored as given before the card"
        );
        assert!(!card("card-unseen"));
        assert!(card("card-seen"));
    }

    /// The question is off until the person turns it on. The stamps say when
    /// the state changed, so asking for the state it already has changes
    /// nothing. The first time it was turned on is kept through turning it off
    /// and on again: the analysis counts from it.
    #[test]
    fn the_question_is_off_until_turned_on() {
        let (database, ratings) = store(&[]);
        assert_eq!(ratings.settings().unwrap(), RatingPromptSettings::default());
        assert!(!ratings.settings().unwrap().enabled);

        assert!(!ratings.set_enabled(false, at(0)).unwrap());
        assert_eq!(
            row_count(&database, "rating_prompt_settings"),
            0,
            "turning off a question that was never on wrote a row"
        );

        assert!(ratings.set_enabled(true, at(10)).unwrap());
        assert!(!ratings.set_enabled(true, at(20)).unwrap());
        assert_eq!(
            ratings.settings().unwrap(),
            RatingPromptSettings {
                enabled: true,
                first_opted_in_at: Some(at(10)),
                opted_in_at: Some(at(10)),
                opted_out_at: None,
            }
        );

        assert!(ratings.set_enabled(false, at(30)).unwrap());
        assert_eq!(
            ratings.settings().unwrap(),
            RatingPromptSettings {
                enabled: false,
                first_opted_in_at: Some(at(10)),
                opted_in_at: Some(at(10)),
                opted_out_at: Some(at(30)),
            }
        );
        assert!(ratings.set_enabled(true, at(40)).unwrap());
        assert!(ratings.set_enabled(false, at(50)).unwrap());
        assert!(ratings.set_enabled(true, at(60)).unwrap());
        assert_eq!(
            ratings.settings().unwrap(),
            RatingPromptSettings {
                enabled: true,
                first_opted_in_at: Some(at(10)),
                opted_in_at: Some(at(60)),
                opted_out_at: Some(at(50)),
            }
        );
        assert_eq!(row_count(&database, "rating_prompt_settings"), 1);
    }

    /// An answer goes with its block: one block's deletion takes its own row
    /// and no other, and Clear Local Work Blocks takes every row. The setting
    /// is a choice the person made, and survives the clear.
    #[test]
    fn an_answer_goes_with_its_block_and_the_setting_stays() {
        let (database, ratings) = store(&["block-a", "block-b", "block-c"]);
        for block in ["block-a", "block-b", "block-c"] {
            ratings.record_eligible(block, 1, at(0)).unwrap();
        }
        ratings
            .record_answer("block-a", SelfRatingAnswer::Most, false, at(10))
            .unwrap();
        ratings.set_enabled(true, at(5)).unwrap();

        database
            .connection()
            .unwrap()
            .execute("DELETE FROM work_block WHERE block_id = 'block-a'", [])
            .unwrap();
        assert_eq!(ratings.rating("block-a").unwrap(), None);
        assert!(ratings.rating("block-b").unwrap().is_some());

        assert_eq!(database.work_block_repo().clear_all().unwrap(), 2);
        assert_eq!(row_count(&database, "work_block_self_rating"), 0);
        assert_eq!(
            ratings.settings().unwrap(),
            RatingPromptSettings {
                enabled: true,
                first_opted_in_at: Some(at(5)),
                opted_in_at: Some(at(5)),
                opted_out_at: None,
            }
        );
    }

    /// The schema keeps an answer and its time together, so a row can never
    /// say there was an answer without when, or a time with no answer.
    #[test]
    fn the_schema_keeps_an_answer_and_its_time_together() {
        let (database, _ratings) = store(&["block-a"]);
        let connection = database.connection().unwrap();
        for (answer, answered_at) in [("'most'", "NULL"), ("NULL", "1800000010")] {
            assert!(
                connection
                    .execute(
                        &format!(
                            "INSERT INTO work_block_self_rating(block_id, prompt_version,
                                 eligible_at, shown_at, answer, answered_at)
                             VALUES ('block-a', 1, 1800000000, 1800000005, {answer},
                                     {answered_at})"
                        ),
                        [],
                    )
                    .is_err(),
                "answer {answer} with answered_at {answered_at} was stored"
            );
        }
        assert!(
            connection
                .execute(
                    "INSERT INTO work_block_self_rating(block_id, prompt_version, eligible_at)
                     VALUES ('block-a', 0, 1800000000)",
                    [],
                )
                .is_err(),
            "a prompt version of 0 was stored"
        );
    }

    /// The schema refuses every row the pre-registered analysis would discard
    /// as malformed, so a row written by anything but this store cannot be
    /// one either: an answer or a Not now never shown, a card seen with no
    /// answer, an answer after Not now with the card unseen, and an opt-in
    /// time without the first one.
    #[test]
    fn the_schema_refuses_the_rows_the_analysis_discards() {
        let (database, _ratings) = store(&["block-a"]);
        let connection = database.connection().unwrap();
        for (columns, values, what) in [
            (
                "answer, answered_at",
                "'most', 1800000010",
                "an answer never shown",
            ),
            ("not_now_at", "1800000010", "a Not now never shown"),
            (
                "shown_at, card_seen_before_answer",
                "1800000005, 1",
                "a card seen with no answer",
            ),
            (
                "shown_at, not_now_at, answer, answered_at",
                "1800000005, 1800000009, 'most', 1800000010",
                "an answer after Not now with the card unseen",
            ),
        ] {
            assert!(
                connection
                    .execute(
                        &format!(
                            "INSERT INTO work_block_self_rating(block_id, prompt_version,
                                 eligible_at, {columns})
                             VALUES ('block-a', 1, 1800000000, {values})"
                        ),
                        [],
                    )
                    .is_err(),
                "{what} was stored"
            );
        }
        connection
            .execute(
                "INSERT INTO work_block_self_rating(block_id, prompt_version, eligible_at,
                     shown_at, not_now_at, answer, answered_at, card_seen_before_answer)
                 VALUES ('block-a', 1, 1800000000, 1800000005, 1800000009, 'most',
                         1800000010, 1)",
                [],
            )
            .expect("a well-formed late answer was refused");

        for (first, last) in [("NULL", "1800000010"), ("1800000010", "NULL")] {
            assert!(
                connection
                    .execute(
                        &format!(
                            "INSERT INTO rating_prompt_settings(id, enabled, first_opted_in_at,
                                 opted_in_at)
                             VALUES (1, 1, {first}, {last})"
                        ),
                        [],
                    )
                    .is_err(),
                "first_opted_in_at {first} with opted_in_at {last} was stored"
            );
        }
    }

    fn insert_block(
        database: &SqlitePersistence,
        block: &str,
        started_at: i64,
        ended_at: Option<i64>,
    ) {
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO work_block(
                    block_id, phase, intensity, planned_duration_seconds, started_at, ended_at,
                    total_paused_seconds, recovered_after_restart, intention_expires_at,
                    updated_at
                 ) VALUES (?1, 'completed', 'medium', 1800, ?2, ?3, 0, 0, 0, 0)",
                rusqlite::params![block, started_at, ended_at],
            )
            .unwrap();
    }

    fn insert_decision(
        database: &SqlitePersistence,
        decision: &str,
        occurred_at: i64,
        block: Option<&str>,
        policy_version: u32,
    ) {
        database
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO intervention_decision_log(
                    decision_id, occurred_at, block_id, policy_version, switch_count,
                    elapsed_seconds, remaining_seconds, gate_verdict
                 ) VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 'abstained_warmup')",
                rusqlite::params![decision, occurred_at, block, policy_version],
            )
            .unwrap();
    }

    /// Not now on the invitation is recorded once, as the off row, and only
    /// on an install that has no row: it never overwrites a setting.
    #[test]
    fn not_now_on_the_invitation_is_recorded_once_and_never_over_a_setting() {
        let (database, ratings) = store(&[]);
        assert!(ratings.decline_opt_in(at(10)).unwrap());
        assert!(!ratings.decline_opt_in(at(20)).unwrap());
        assert_eq!(
            ratings.settings().unwrap(),
            RatingPromptSettings {
                enabled: false,
                first_opted_in_at: None,
                opted_in_at: None,
                opted_out_at: Some(at(10)),
            }
        );
        assert_eq!(row_count(&database, "rating_prompt_settings"), 1);

        // Turning it on afterwards works as from the default.
        assert!(ratings.set_enabled(true, at(30)).unwrap());
        assert_eq!(ratings.settings().unwrap().first_opted_in_at, Some(at(30)));
        assert!(!ratings.decline_opt_in(at(40)).unwrap());
        assert!(ratings.settings().unwrap().enabled);

        let (_, ratings) = store(&[]);
        ratings.set_enabled(true, at(0)).unwrap();
        ratings.set_enabled(false, at(5)).unwrap();
        assert!(!ratings.decline_opt_in(at(10)).unwrap());
        assert_eq!(ratings.settings().unwrap().opted_out_at, Some(at(5)));
    }

    /// The next block is the next one created, which is the order the manager
    /// reads its latest block in, whatever the clock said at its start.
    #[test]
    fn a_block_is_bounded_by_its_end_and_the_next_block_created() {
        let (database, ratings) = store(&[]);
        insert_block(&database, "first", 1_000, Some(2_500));
        insert_block(&database, "second", 3_000, Some(4_500));
        // The clock moved back before the third block started.
        insert_block(&database, "third", 2_000, None);

        assert_eq!(
            ratings.block_bounds("first").unwrap(),
            Some(super::RatedBlockBounds {
                started_at: DateTime::from_timestamp(1_000, 0).unwrap(),
                ended_at: Some(DateTime::from_timestamp(2_500, 0).unwrap()),
                next_block_started_at: Some(DateTime::from_timestamp(3_000, 0).unwrap()),
            })
        );
        assert_eq!(
            ratings
                .block_bounds("second")
                .unwrap()
                .unwrap()
                .next_block_started_at,
            Some(DateTime::from_timestamp(2_000, 0).unwrap())
        );
        assert_eq!(
            ratings.block_bounds("third").unwrap(),
            Some(super::RatedBlockBounds {
                started_at: DateTime::from_timestamp(2_000, 0).unwrap(),
                ended_at: None,
                next_block_started_at: None,
            })
        );
        assert_eq!(ratings.block_bounds("missing").unwrap(), None);
    }

    /// The period starts at the first block after the last decision row with
    /// an older policy version, or at the first block when there is none. It
    /// never waits for a decision row of the current version.
    #[test]
    fn the_primary_period_starts_at_the_first_block_after_the_upgrade_cut() {
        let (database, ratings) = store(&[]);
        assert_eq!(ratings.primary_period_start(6).unwrap(), None);

        insert_block(&database, "a", 5_000, Some(6_000));
        insert_block(&database, "b", 1_000, Some(2_000));
        assert_eq!(
            ratings.primary_period_start(6).unwrap(),
            Some(DateTime::from_timestamp(1_000, 0).unwrap()),
            "no older decision row: the first block by start"
        );

        insert_decision(&database, "v5-in-b", 1_500, Some("b"), 5);
        insert_decision(&database, "v6-in-b", 1_600, Some("b"), 6);
        assert_eq!(
            ratings.primary_period_start(6).unwrap(),
            Some(DateTime::from_timestamp(5_000, 0).unwrap()),
            "the block holding the last v5 row is before the cut"
        );

        // A v5 row with no block still moves the cut.
        insert_decision(&database, "v5-no-block", 5_000, None, 5);
        assert_eq!(ratings.primary_period_start(6).unwrap(), None);
        insert_block(&database, "c", 7_000, None);
        assert_eq!(
            ratings.primary_period_start(6).unwrap(),
            Some(DateTime::from_timestamp(7_000, 0).unwrap())
        );
    }
}
