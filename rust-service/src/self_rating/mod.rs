//! The optional question after a block ([`RATING_PROMPT_VERSION`]; workspace
//! `plan/attention-labels-and-model.md` § 3.1, § 3.2 and § 3.4, founder
//! decision 2026-10-17): which ended blocks it covers, the words it is asked
//! in, when it expires, and what a sighting, a Not now or an answer records.
//!
//! "How much of this block was on what you meant to do?" is a measurement
//! instrument. Velvt's summary of a block is computed from broad categories,
//! and nothing records whether the person would describe the block the same
//! way; the answers are compared with the summary offline, under a hashed
//! pre-registration. The question is never asked during a block, never as a
//! notification, and never before the person turns it on.
//!
//! Every rule here is a fixed function of timestamps and the person's own
//! setting. None depends on an answer, a category or coverage, so whether a
//! block was asked about never depends on what the block looked like, and a
//! block that was eligible but never seen stays a recorded missing answer.
//!
//! The day-14 floor. A block is covered only if it started at or after the
//! end of its Mac's drift policy v6 primary period: the period starts at the
//! start of the Mac's first block after the upgrade cut, as item 1 of the
//! draft period note and `scripts/analyze_ratings.py` define it, and ends
//! 1,209,600 seconds (14 days) later. So no block that started inside the
//! period is ever rated, and the question cannot be turned on inside it
//! either. This holds even if a build with the question reaches a Mac too
//! early.
//!
//! On without a break. The question about a block is shown, and a sighting,
//! a Not now or an answer about it is taken, only while the question has
//! stayed on since before the block started: it is on, and it was last
//! turned on at or before the block's start. Turning it off hides the
//! question, and turning it on again does not bring it back, because the
//! block's result card may have been on screen in between and an answer
//! given after it would not be a first answer. The row expires on its own
//! schedule.
//!
//! The one-time invitation. Swift reports which button was tapped
//! (`respond_rating_opt_in`); Rust decides what it means. Not now is
//! recorded only while the invitation is still owed, so a tap on an
//! invitation left on screen after the question was turned on in Settings
//! changes nothing. The Settings toggle (`set_rating_prompt_enabled`) never
//! answers the invitation.
//!
//! Isolation. This module writes `work_block_self_rating` and
//! `rating_prompt_settings` through [`SelfRatingRepo`], and no other table.
//! It reads `work_block` and `intervention_decision_log` only to place a block
//! in time. No drift decision, offer, outcome, demotion, backoff, invitation
//! or receipt reads anything it writes. `tests/rating_isolation.rs` replays
//! the same blocks with the question on and answered and with no question at
//! all, and requires every other table to come out byte-identical.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;
use velvt_shared_types::{
    RatingOptIn, RatingPrompt, RatingPromptOption, RatingPromptPresentation,
    RatingPromptSettings as SettingsPayload, WorkBlockRatingAnswer,
};

use crate::persistence::{
    PersistenceError, RatingPromptSettings, SelfRating, SelfRatingAnswer, SelfRatingRepo,
};
use crate::work_block::{EndedBlock, RatingPromptSource, RatingSurfaces};

/// The version of the question's wording and of the rules below. Frozen once
/// the rating pre-registration is hashed: any change to the copy, the five
/// answers or a rule here is a new version, and rows stored under two
/// versions are never pooled. Every row records the version it was asked
/// under.
pub const RATING_PROMPT_VERSION: u32 = 1;

/// A block that ran less than this is never asked about (plan § 3.2). Read
/// from the elapsed time the block's result reports.
pub const RATING_MIN_ELAPSED_SECONDS: u32 = 5 * 60;

/// The question can be answered until this long after the block ended, or
/// until the next block starts, whichever comes first.
pub const RATING_ANSWER_WINDOW_SECONDS: i64 = 2 * 60 * 60;

/// The drift policy whose primary period the floor protects.
pub const PRIMARY_PERIOD_POLICY_VERSION: u32 = 6;

/// The length of a Mac's v6 primary period, which the founder fixed at 14
/// days on 2026-10-17 (attention plan decision 1). The analysis config's
/// `v6_primary_period_seconds` holds the same number.
pub const PRIMARY_PERIOD_SECONDS: i64 = 1_209_600;

/// The question, frozen at [`RATING_PROMPT_VERSION`] 1.
pub const QUESTION: &str = "How much of this block was on what you meant to do?";
/// "Not now", on the question and on the invitation.
pub const NOT_NOW: &str = "Not now";
pub const FOOTNOTE: &str =
    "Optional. Your answer stays on this Mac unless you send it in an export.";
/// Shown after an answer, before the result card.
pub const ACKNOWLEDGEMENT: &str = "Noted.";
/// The quiet link under the result card after Not now.
pub const LINK: &str = "Answer the question about this block";
pub const OPT_IN_BODY: &str = "When a block ends, Velvt can ask how much of it was on what \
     you meant to do. One tap, always optional, never during a block. Your answers stay on \
     this Mac unless you send them in an export. You can turn it off in Settings.";
pub const OPT_IN_TURN_ON: &str = "Turn on";
pub const SETTINGS_TITLE: &str = "Ask one question after each block";
pub const SETTINGS_SUBTITLE: &str =
    "Never during a block. Answers stay on this Mac unless you send an export.";

/// The five answers' labels, from most of the block to least.
pub fn answer_label(answer: WorkBlockRatingAnswer) -> &'static str {
    match answer {
        WorkBlockRatingAnswer::All => "All or nearly all",
        WorkBlockRatingAnswer::Most => "Most",
        WorkBlockRatingAnswer::Half => "About half",
        WorkBlockRatingAnswer::LessThanHalf => "Less than half",
        WorkBlockRatingAnswer::Little => "Little or none",
    }
}

/// Every string this module shows, for the copy checks.
pub fn every_copy_string() -> Vec<&'static str> {
    let mut copy = vec![
        QUESTION,
        NOT_NOW,
        FOOTNOTE,
        ACKNOWLEDGEMENT,
        LINK,
        OPT_IN_BODY,
        OPT_IN_TURN_ON,
        SETTINGS_TITLE,
        SETTINGS_SUBTITLE,
    ];
    copy.extend(WorkBlockRatingAnswer::ALL.into_iter().map(answer_label));
    copy
}

/// Whether an ended block is asked about, and if not, why. Every reason is a
/// fact about time or about the person's setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    /// It ran under [`RATING_MIN_ELAPSED_SECONDS`].
    TooShort,
    /// The Mac has no primary period to place it against: no block started
    /// after the upgrade cut.
    NoPrimaryPeriod,
    /// It started before the Mac's primary period ended.
    InsidePrimaryPeriod,
    /// The question is off.
    QuestionOff,
    /// The question was last turned on after the block started, so the
    /// block did not run under it.
    StartedBeforeTurnedOn,
}

/// The end of a Mac's primary period, from its start.
pub fn primary_period_end(period_start: DateTime<Utc>) -> DateTime<Utc> {
    period_start + Duration::seconds(PRIMARY_PERIOD_SECONDS)
}

/// The rules that hold whatever the setting: long enough, and started at or
/// after the end of the Mac's primary period. The invitation to turn the
/// question on follows a block that passes these.
pub fn block_qualifies(
    block: &EndedBlock,
    period_start: Option<DateTime<Utc>>,
) -> Result<(), Eligibility> {
    if block.elapsed_seconds < RATING_MIN_ELAPSED_SECONDS {
        return Err(Eligibility::TooShort);
    }
    let Some(period_start) = period_start else {
        return Err(Eligibility::NoPrimaryPeriod);
    };
    if block.started_at < primary_period_end(period_start) {
        return Err(Eligibility::InsidePrimaryPeriod);
    }
    Ok(())
}

/// Whether `block` is asked about (plan § 3.2): it ran at least five
/// minutes, it started at or after the end of its Mac's primary period, and
/// the question was on for the whole of it.
pub fn eligibility(
    block: &EndedBlock,
    period_start: Option<DateTime<Utc>>,
    settings: &RatingPromptSettings,
) -> Eligibility {
    if let Err(reason) = block_qualifies(block, period_start) {
        return reason;
    }
    if !settings.enabled {
        return Eligibility::QuestionOff;
    }
    if on_throughout(settings, block.started_at) {
        Eligibility::Eligible
    } else {
        Eligibility::StartedBeforeTurnedOn
    }
}

/// Whether the question has been on without a break since `started_at`: it
/// is on now, and it was last turned on at or before that instant. Turning it
/// off and on again moves `opted_in_at` forward, so a block that started
/// before the latest turn-on fails this for good.
pub fn on_throughout(settings: &RatingPromptSettings, started_at: DateTime<Utc>) -> bool {
    settings.enabled
        && settings
            .opted_in_at
            .is_some_and(|turned_on| turned_on <= started_at)
}

/// When the question about a block stops taking an answer: two hours after
/// the block ended, or when the next block started, whichever came first.
pub fn answer_deadline(
    ended_at: DateTime<Utc>,
    next_block_started_at: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    let window = ended_at + Duration::seconds(RATING_ANSWER_WINDOW_SECONDS);
    next_block_started_at.map_or(window, |next| next.min(window))
}

fn stored(answer: WorkBlockRatingAnswer) -> SelfRatingAnswer {
    match answer {
        WorkBlockRatingAnswer::All => SelfRatingAnswer::All,
        WorkBlockRatingAnswer::Most => SelfRatingAnswer::Most,
        WorkBlockRatingAnswer::Half => SelfRatingAnswer::Half,
        WorkBlockRatingAnswer::LessThanHalf => SelfRatingAnswer::LessThanHalf,
        WorkBlockRatingAnswer::Little => SelfRatingAnswer::Little,
    }
}

/// Never answered and never declined: the invitation is still owed.
fn invitation_pending(settings: &RatingPromptSettings) -> bool {
    settings.first_opted_in_at.is_none() && settings.opted_out_at.is_none()
}

/// Owns every rule of the question after a block and records what happened
/// through [`SelfRatingRepo`]. Swift reports facts; this decides.
pub struct RatingPromptManager {
    ratings: Arc<dyn SelfRatingRepo>,
}

impl RatingPromptManager {
    pub fn new(ratings: Arc<dyn SelfRatingRepo>) -> Self {
        Self { ratings }
    }

    /// The question and the invitation for the snapshot of an ended block at
    /// `now`. Records the block's eligibility the first time it is eligible,
    /// with `eligible_at` its end, and expires every question whose deadline
    /// has passed.
    pub fn surfaces(
        &self,
        block: &EndedBlock,
        now: DateTime<Utc>,
    ) -> Result<RatingSurfaces, PersistenceError> {
        let settings = self.ratings.settings()?;
        if settings.enabled && self.ratings.rating(&block.block_id)?.is_none() {
            let period_start = self.period_start()?;
            if eligibility(block, period_start, &settings) == Eligibility::Eligible {
                self.ratings.record_eligible(
                    &block.block_id,
                    RATING_PROMPT_VERSION,
                    block.ended_at,
                )?;
            }
        }
        self.expire_due(now)?;

        let prompt = match self.ratings.rating(&block.block_id)? {
            Some(rating) if on_throughout(&settings, block.started_at) && rating.is_open() => {
                let deadline =
                    answer_deadline(block.ended_at, self.next_block_started_at(&block.block_id)?);
                Uuid::parse_str(&block.block_id)
                    .ok()
                    .filter(|_| now < deadline)
                    .map(|block_id| prompt(block_id, &rating, deadline))
            }
            _ => None,
        };
        let opt_in = (prompt.is_none()
            && invitation_pending(&settings)
            && block_qualifies(block, self.period_start()?).is_ok())
        .then(opt_in_card);
        Ok(RatingSurfaces { prompt, opt_in })
    }

    /// The question about `block_id` was on screen at `now`. Only an open
    /// question, while the question has been on since before the block
    /// started, records it; the first one wins.
    pub fn record_shown(
        &self,
        block_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let block_id = block_id.to_string();
        if !self.prepare_report(&block_id, now)? {
            return Ok(false);
        }
        self.ratings.mark_shown(&block_id, now)
    }

    /// An answer at `now`. Only an open question, while the question has
    /// been on since before the block started, takes one, so an answer after
    /// the deadline, or after the question was turned off, is refused.
    pub fn record_answer(
        &self,
        block_id: Uuid,
        answer: WorkBlockRatingAnswer,
        via_link: bool,
        now: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let block_id = block_id.to_string();
        if !self.prepare_report(&block_id, now)? {
            return Ok(false);
        }
        self.ratings
            .record_answer(&block_id, stored(answer), via_link, now)
    }

    /// Not now at `now`. Never an answer; the question stays open.
    pub fn record_not_now(
        &self,
        block_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let block_id = block_id.to_string();
        if !self.prepare_report(&block_id, now)? {
            return Ok(false);
        }
        self.ratings.record_not_now(&block_id, now)
    }

    /// The Settings toggle, at `now`. On is refused while the Mac's primary
    /// period is not over. Off hides an open question for good: turning the
    /// question on again does not bring it back, and the row expires on its
    /// own schedule. Neither answers the invitation: off while the question
    /// was never on changes nothing.
    pub fn set_enabled(
        &self,
        enabled: bool,
        now: DateTime<Utc>,
    ) -> Result<SettingsPayload, PersistenceError> {
        if !enabled || self.period_over(now)? {
            self.ratings.set_enabled(enabled, now)?;
        }
        self.settings(now)
    }

    /// A tap on the one-time invitation at `now`: Turn on when `turn_on`,
    /// Not now otherwise. Turn on is the Settings toggle's on, under the same
    /// floor. Not now is recorded only while the invitation is still owed,
    /// so it is never shown again; a Not now on an invitation left on screen
    /// after the question was turned on, or after an earlier Not now,
    /// changes nothing. Returns whether anything was recorded.
    pub fn respond_to_invitation(
        &self,
        turn_on: bool,
        now: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        if turn_on {
            return if self.period_over(now)? {
                self.ratings.set_enabled(true, now)
            } else {
                Ok(false)
            };
        }
        if invitation_pending(&self.ratings.settings()?) {
            self.ratings.decline_opt_in(now)
        } else {
            Ok(false)
        }
    }

    /// The setting, with the Settings pane's words. The toggle is offered once
    /// the Mac's primary period is over, or while the question is on.
    pub fn settings(&self, now: DateTime<Utc>) -> Result<SettingsPayload, PersistenceError> {
        let enabled = self.ratings.settings()?.enabled;
        Ok(SettingsPayload {
            available: enabled || self.period_over(now)?,
            enabled,
            title: SETTINGS_TITLE.to_owned(),
            subtitle: SETTINGS_SUBTITLE.to_owned(),
        })
    }

    /// Expires what is due, then says whether a report about `block_id` may
    /// be recorded: only while the question has been on since before the
    /// block started, because that is the only time it is on screen.
    fn prepare_report(&self, block_id: &str, now: DateTime<Utc>) -> Result<bool, PersistenceError> {
        self.expire_due(now)?;
        let settings = self.ratings.settings()?;
        Ok(self
            .ratings
            .block_bounds(block_id)?
            .is_some_and(|bounds| on_throughout(&settings, bounds.started_at)))
    }

    /// Expires every open question whose deadline has passed by `now`, at its
    /// deadline: the instant it stopped taking an answer, not when this ran.
    fn expire_due(&self, now: DateTime<Utc>) -> Result<(), PersistenceError> {
        for rating in self.ratings.open_ratings()? {
            let Some(bounds) = self.ratings.block_bounds(&rating.block_id)? else {
                continue;
            };
            let Some(ended_at) = bounds.ended_at else {
                continue;
            };
            let deadline = answer_deadline(ended_at, bounds.next_block_started_at);
            if now >= deadline {
                self.ratings.expire(&rating.block_id, deadline)?;
            }
        }
        Ok(())
    }

    fn next_block_started_at(
        &self,
        block_id: &str,
    ) -> Result<Option<DateTime<Utc>>, PersistenceError> {
        Ok(self
            .ratings
            .block_bounds(block_id)?
            .and_then(|bounds| bounds.next_block_started_at))
    }

    fn period_start(&self) -> Result<Option<DateTime<Utc>>, PersistenceError> {
        self.ratings
            .primary_period_start(PRIMARY_PERIOD_POLICY_VERSION)
    }

    fn period_over(&self, now: DateTime<Utc>) -> Result<bool, PersistenceError> {
        Ok(self
            .period_start()?
            .is_some_and(|start| now >= primary_period_end(start)))
    }
}

impl RatingPromptSource for RatingPromptManager {
    fn surfaces_for(&self, block: &EndedBlock, now: DateTime<Utc>) -> RatingSurfaces {
        self.surfaces(block, now).unwrap_or_else(|_| {
            tracing::warn!(
                error_code = "rating_prompt_unavailable",
                "the question after a block could not be evaluated"
            );
            RatingSurfaces::default()
        })
    }
}

fn prompt(block_id: Uuid, rating: &SelfRating, deadline: DateTime<Utc>) -> RatingPrompt {
    RatingPrompt {
        block_id,
        prompt_version: rating.prompt_version,
        presentation: if rating.not_now_at.is_some() {
            RatingPromptPresentation::Link
        } else {
            RatingPromptPresentation::Question
        },
        question: QUESTION.to_owned(),
        options: WorkBlockRatingAnswer::ALL
            .into_iter()
            .map(|answer| RatingPromptOption {
                answer,
                label: answer_label(answer).to_owned(),
            })
            .collect(),
        not_now: NOT_NOW.to_owned(),
        footnote: FOOTNOTE.to_owned(),
        acknowledgement: ACKNOWLEDGEMENT.to_owned(),
        link: LINK.to_owned(),
        expires_at: deadline,
    }
}

fn opt_in_card() -> RatingOptIn {
    RatingOptIn {
        body: OPT_IN_BODY.to_owned(),
        turn_on: OPT_IN_TURN_ON.to_owned(),
        not_now: NOT_NOW.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{GateVerdict, InterventionDecision, SqlitePersistence};
    use crate::work_block::{WorkBlockManager, BANNED_COPY_TOKENS, BANNED_JARGON_TOKENS};
    use velvt_shared_types::{
        StartWorkBlock, WorkBlockIntensity, WorkBlockPhase, WorkBlockSnapshot,
    };

    const DAY: i64 = 86_400;

    /// 2027-01-15T08:00:00Z, the anchor the work-block tests use.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    struct Fixture {
        database: SqlitePersistence,
        manager: WorkBlockManager,
        rating: Arc<RatingPromptManager>,
    }

    fn fixture() -> Fixture {
        let database = SqlitePersistence::open_in_memory().unwrap();
        let rating = Arc::new(RatingPromptManager::new(database.self_rating_repo()));
        let manager = WorkBlockManager::new(database.work_block_repo())
            .with_rating_prompt(Arc::clone(&rating) as Arc<dyn RatingPromptSource>);
        Fixture {
            database,
            manager,
            rating,
        }
    }

    fn request(planned_seconds: u32) -> StartWorkBlock {
        StartWorkBlock {
            intention: None,
            planned_duration_seconds: planned_seconds,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }
    }

    /// Starts a block at `start` and ends it `run` seconds later, early when
    /// `run` is under the 30 planned minutes. Returns its id and the snapshot
    /// the end produced.
    fn block(fixture: &Fixture, start: i64, run: i64) -> (Uuid, WorkBlockSnapshot) {
        let started = fixture.manager.start(request(1_800), at(start)).unwrap();
        let id = started.block_id.unwrap();
        let ended = if run >= 1_800 {
            fixture.manager.request_state(at(start + run)).unwrap()
        } else {
            fixture.manager.end(id, at(start + run)).unwrap()
        };
        assert!(matches!(
            ended.phase,
            WorkBlockPhase::Completed | WorkBlockPhase::Abandoned
        ));
        (id, ended)
    }

    /// A Mac whose first block was at 0 and whose question was turned on at
    /// day 15.
    fn turned_on_after_day_14() -> Fixture {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let settings = fixture.rating.set_enabled(true, at(15 * DAY)).unwrap();
        assert!(settings.enabled);
        fixture
    }

    fn ended(block_id: &str, started_at: i64, ended_at: i64, elapsed: u32) -> EndedBlock {
        EndedBlock {
            block_id: block_id.to_owned(),
            phase: WorkBlockPhase::Completed,
            started_at: at(started_at),
            ended_at: at(ended_at),
            elapsed_seconds: elapsed,
        }
    }

    fn on_since(turned_on: i64) -> RatingPromptSettings {
        RatingPromptSettings {
            enabled: true,
            first_opted_in_at: Some(at(turned_on)),
            opted_in_at: Some(at(turned_on)),
            opted_out_at: None,
        }
    }

    // -----------------------------------------------------------------------
    // The rules, as pure functions
    // -----------------------------------------------------------------------

    /// Every rule of plan § 3.2 at its boundary. The inputs are timestamps,
    /// an elapsed time and the setting: there is no field a category, a
    /// coverage figure or an answer could reach the decision through.
    #[test]
    fn every_eligibility_rule_holds_at_its_boundary() {
        let period = Some(at(0));
        let floor = PRIMARY_PERIOD_SECONDS;
        let cases = [
            (
                ended("b", floor, floor + 300, 300),
                period,
                on_since(0),
                Eligibility::Eligible,
            ),
            (
                ended("b", floor, floor + 299, 299),
                period,
                on_since(0),
                Eligibility::TooShort,
            ),
            (
                ended("b", floor - 1, floor + 1_800, 1_801),
                period,
                on_since(0),
                Eligibility::InsidePrimaryPeriod,
            ),
            (
                ended("b", floor, floor + 1_800, 1_800),
                None,
                on_since(0),
                Eligibility::NoPrimaryPeriod,
            ),
            (
                ended("b", floor, floor + 1_800, 1_800),
                period,
                RatingPromptSettings::default(),
                Eligibility::QuestionOff,
            ),
            (
                ended("b", floor, floor + 1_800, 1_800),
                period,
                on_since(floor),
                Eligibility::Eligible,
            ),
            (
                ended("b", floor, floor + 1_800, 1_800),
                period,
                on_since(floor + 1),
                Eligibility::StartedBeforeTurnedOn,
            ),
            (
                ended("b", floor, floor + 1_800, 1_800),
                period,
                RatingPromptSettings {
                    enabled: false,
                    opted_out_at: Some(at(floor - 10)),
                    ..on_since(0)
                },
                Eligibility::QuestionOff,
            ),
        ];
        for (block, period, settings, expected) in cases {
            assert_eq!(
                eligibility(&block, period, &settings),
                expected,
                "{block:?} under {settings:?}"
            );
        }
    }

    #[test]
    fn the_deadline_is_two_hours_or_the_next_block_whichever_is_first() {
        assert_eq!(answer_deadline(at(0), None), at(7_200));
        assert_eq!(answer_deadline(at(0), Some(at(600))), at(600));
        assert_eq!(answer_deadline(at(0), Some(at(9_000))), at(7_200));
    }

    // -----------------------------------------------------------------------
    // The day-14 floor
    // -----------------------------------------------------------------------

    /// Inside the Mac's first 14 days nothing is asked, nothing is offered,
    /// no row is written, and the question cannot be turned on.
    #[test]
    fn nothing_is_asked_or_offered_inside_the_first_14_days() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let refused = fixture.rating.set_enabled(true, at(14 * DAY - 1)).unwrap();
        assert!(!refused.available);
        assert!(!refused.enabled);

        let (_, inside) = block(&fixture, 13 * DAY, 1_800);
        assert_eq!(inside.rating_prompt, None);
        assert_eq!(inside.rating_opt_in, None);

        assert!(
            !fixture
                .rating
                .respond_to_invitation(true, at(14 * DAY - 1))
                .unwrap(),
            "Turn on on an invitation is held to the same floor"
        );
        let accepted = fixture.rating.set_enabled(true, at(14 * DAY)).unwrap();
        assert!(accepted.available && accepted.enabled);
        let rows = fixture.database.self_rating_repo().open_ratings().unwrap();
        assert!(rows.is_empty(), "{rows:?}");
    }

    /// A block that starts on day 13 and ends on day 14 is in the period: a
    /// block counts by its start, so it is never asked about.
    #[test]
    fn a_block_that_started_inside_the_period_is_never_asked_about() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let started = fixture
            .manager
            .start(request(1_800), at(14 * DAY - 600))
            .unwrap();
        fixture.rating.set_enabled(true, at(14 * DAY)).unwrap();
        let ended = fixture
            .manager
            .end(started.block_id.unwrap(), at(14 * DAY + 900))
            .unwrap();
        assert_eq!(ended.rating_prompt, None);
        assert!(fixture
            .database
            .self_rating_repo()
            .rating(&started.block_id.unwrap().to_string())
            .unwrap()
            .is_none());
    }

    /// The period starts at the first block after the last decision row with
    /// an older policy version, so an older build's history moves the floor.
    #[test]
    fn an_older_policy_s_decisions_move_the_floor() {
        let fixture = fixture();
        let (old, _) = block(&fixture, 0, 1_500);
        fixture
            .database
            .work_block_repo()
            .record_decision(&InterventionDecision {
                decision_id: "older-policy".into(),
                occurred_at: at(600),
                block_id: Some(old.to_string()),
                policy_version: 5,
                anchor_category: None,
                switch_count: 0,
                elapsed_seconds: 600,
                remaining_seconds: 900,
                gate_verdict: GateVerdict::AbstainedNoAnchor,
                propensity: 1.0,
                anchor_seen_within_600s: None,
                outcome_at: None,
                focus_state: None,
            })
            .unwrap();
        // The first block after the cut is on day 5, so the floor is day 19.
        block(&fixture, 5 * DAY, 1_500);
        assert!(
            !fixture
                .rating
                .set_enabled(true, at(18 * DAY))
                .unwrap()
                .enabled
        );
        assert!(
            fixture
                .rating
                .set_enabled(true, at(19 * DAY))
                .unwrap()
                .enabled
        );
        let (_, after) = block(&fixture, 19 * DAY + 60, 1_800);
        assert!(after.rating_prompt.is_some());
    }

    // -----------------------------------------------------------------------
    // The invitation and the setting
    // -----------------------------------------------------------------------

    /// After the first block that qualifies, the invitation is offered, on
    /// every snapshot of it, until it is answered. Not now is recorded and it
    /// never comes back. The question stays off throughout.
    #[test]
    fn the_invitation_follows_a_qualifying_block_until_it_is_answered() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let (_, short) = block(&fixture, 15 * DAY, 240);
        assert_eq!(short.rating_opt_in, None, "a block under five minutes");

        let (_, ended) = block(&fixture, 15 * DAY + 600, 1_800);
        let card = ended
            .rating_opt_in
            .expect("the invitation follows the block");
        assert_eq!(card.body, OPT_IN_BODY);
        assert_eq!(card.turn_on, OPT_IN_TURN_ON);
        assert_eq!(card.not_now, NOT_NOW);
        assert_eq!(ended.rating_prompt, None);
        let later = fixture.manager.request_state(at(16 * DAY)).unwrap();
        assert!(later.rating_opt_in.is_some(), "silence is not an answer");

        let toggle = fixture.rating.set_enabled(false, at(16 * DAY + 2)).unwrap();
        assert!(toggle.available && !toggle.enabled);
        assert!(
            fixture
                .manager
                .request_state(at(16 * DAY + 3))
                .unwrap()
                .rating_opt_in
                .is_some(),
            "the Settings toggle's off is not the invitation's Not now"
        );
        assert!(fixture
            .rating
            .respond_to_invitation(false, at(16 * DAY + 5))
            .unwrap());
        assert!(
            !fixture
                .rating
                .respond_to_invitation(false, at(16 * DAY + 6))
                .unwrap(),
            "recorded once"
        );
        assert_eq!(
            fixture
                .database
                .self_rating_repo()
                .settings()
                .unwrap()
                .opted_out_at,
            Some(at(16 * DAY + 5))
        );
        assert_eq!(
            fixture
                .manager
                .request_state(at(16 * DAY + 10))
                .unwrap()
                .rating_opt_in,
            None
        );
        let (_, next) = block(&fixture, 17 * DAY, 1_800);
        assert_eq!(next.rating_opt_in, None);
        assert_eq!(next.rating_prompt, None, "the question is still off");
        assert!(fixture
            .database
            .self_rating_repo()
            .open_ratings()
            .unwrap()
            .is_empty());
    }

    /// Turning the question on covers blocks that start after it. The block
    /// running when it was turned on is not asked about, and the invitation
    /// is never offered once the question has been on.
    #[test]
    fn turning_it_on_covers_only_blocks_that_start_after() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let running = fixture.manager.start(request(1_800), at(15 * DAY)).unwrap();
        fixture.rating.set_enabled(true, at(15 * DAY + 60)).unwrap();
        let ended = fixture
            .manager
            .end(running.block_id.unwrap(), at(15 * DAY + 1_200))
            .unwrap();
        assert_eq!(ended.rating_prompt, None);
        assert_eq!(ended.rating_opt_in, None);

        let (id, asked) = block(&fixture, 15 * DAY + 3_600, 1_800);
        let prompt = asked.rating_prompt.expect("the next block is asked about");
        assert_eq!(prompt.block_id, id);
        assert_eq!(prompt.prompt_version, RATING_PROMPT_VERSION);
        assert_eq!(prompt.presentation, RatingPromptPresentation::Question);
        assert_eq!(prompt.question, QUESTION);
        assert_eq!(
            prompt
                .options
                .iter()
                .map(|option| (option.answer, option.label.as_str()))
                .collect::<Vec<_>>(),
            [
                (WorkBlockRatingAnswer::All, "All or nearly all"),
                (WorkBlockRatingAnswer::Most, "Most"),
                (WorkBlockRatingAnswer::Half, "About half"),
                (WorkBlockRatingAnswer::LessThanHalf, "Less than half"),
                (WorkBlockRatingAnswer::Little, "Little or none"),
            ]
        );
        assert_eq!(prompt.expires_at, at(15 * DAY + 3_600 + 1_800 + 7_200));
        assert_eq!(asked.rating_opt_in, None);

        let row = fixture
            .database
            .self_rating_repo()
            .rating(&id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(row.eligible_at, at(15 * DAY + 3_600 + 1_800));
        assert_eq!(row.shown_at, None, "eligible, not yet on screen");
    }

    /// Turn on, on the invitation, turns the question on. The block the
    /// invitation followed did not run under it, so it is not asked about;
    /// the next block is.
    #[test]
    fn turn_on_on_the_invitation_covers_the_blocks_after_it() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let (_, ended) = block(&fixture, 15 * DAY, 1_800);
        assert!(ended.rating_opt_in.is_some());
        assert!(fixture
            .rating
            .respond_to_invitation(true, at(15 * DAY + 1_900))
            .unwrap());
        let after = fixture.manager.request_state(at(15 * DAY + 1_910)).unwrap();
        assert_eq!(after.rating_opt_in, None);
        assert_eq!(after.rating_prompt, None);
        let (_, next) = block(&fixture, 15 * DAY + 3_600, 1_800);
        assert!(next.rating_prompt.is_some());
    }

    /// Only the invitation's own Not now answers it, and only while it is
    /// owed. The Settings toggle's off on an install that never turned the
    /// question on records nothing, and a Not now on an invitation left on
    /// screen after the question was turned on in Settings changes nothing.
    #[test]
    fn a_not_now_on_an_invitation_left_on_screen_changes_nothing() {
        let fixture = fixture();
        block(&fixture, 0, 1_500);
        let (_, ended) = block(&fixture, 15 * DAY, 1_800);
        assert!(ended.rating_opt_in.is_some());

        fixture
            .rating
            .set_enabled(false, at(15 * DAY + 1_850))
            .unwrap();
        assert_eq!(
            fixture.database.self_rating_repo().settings().unwrap(),
            RatingPromptSettings::default()
        );

        assert!(
            fixture
                .rating
                .set_enabled(true, at(15 * DAY + 1_900))
                .unwrap()
                .enabled
        );
        assert!(!fixture
            .rating
            .respond_to_invitation(false, at(15 * DAY + 1_910))
            .unwrap());
        let settings = fixture.database.self_rating_repo().settings().unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.opted_out_at, None);
    }

    /// A block under five minutes is never asked about; five minutes is.
    #[test]
    fn five_minutes_is_the_shortest_block_asked_about() {
        let fixture = turned_on_after_day_14();
        let (short, ended) = block(&fixture, 16 * DAY, 299);
        assert_eq!(ended.rating_prompt, None);
        let (long, ended) = block(&fixture, 16 * DAY + 600, 300);
        assert!(ended.rating_prompt.is_some());
        let ratings = fixture.database.self_rating_repo();
        assert!(ratings.rating(&short.to_string()).unwrap().is_none());
        assert!(ratings.rating(&long.to_string()).unwrap().is_some());
    }

    // -----------------------------------------------------------------------
    // Reports
    // -----------------------------------------------------------------------

    /// Shown, then Not now: the question moves under the card as a link and
    /// stays open. An answer through the link is stored with the card seen,
    /// and the question leaves the snapshot.
    #[test]
    fn not_now_moves_the_question_under_the_card_until_it_is_answered() {
        let fixture = turned_on_after_day_14();
        let (id, _) = block(&fixture, 16 * DAY, 1_800);
        let end = 16 * DAY + 1_800;
        assert!(fixture.rating.record_shown(id, at(end + 5)).unwrap());
        assert!(
            !fixture.rating.record_shown(id, at(end + 6)).unwrap(),
            "first wins"
        );
        assert!(fixture.rating.record_not_now(id, at(end + 9)).unwrap());

        let after = fixture.manager.request_state(at(end + 60)).unwrap();
        let prompt = after
            .rating_prompt
            .expect("Not now leaves the question open");
        assert_eq!(prompt.presentation, RatingPromptPresentation::Link);
        assert_eq!(prompt.link, LINK);

        assert!(fixture
            .rating
            .record_answer(id, WorkBlockRatingAnswer::Half, true, at(end + 600))
            .unwrap());
        assert!(
            !fixture
                .rating
                .record_answer(id, WorkBlockRatingAnswer::All, false, at(end + 601))
                .unwrap(),
            "an answer is final"
        );
        let row = fixture
            .database
            .self_rating_repo()
            .rating(&id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(row.shown_at, Some(at(end + 5)));
        assert_eq!(row.not_now_at, Some(at(end + 9)));
        assert_eq!(row.answer, Some(SelfRatingAnswer::Half));
        assert_eq!(row.answered_at, Some(at(end + 600)));
        assert!(row.card_seen_before_answer);
        assert_eq!(row.expired_at, None, "an answered question never expires");
        assert_eq!(
            fixture
                .manager
                .request_state(at(end + 700))
                .unwrap()
                .rating_prompt,
            None
        );
    }

    /// Every answer is stored as its own value, and an answer straight from
    /// the question is stored with the card unseen.
    #[test]
    fn every_answer_is_stored_as_given() {
        let fixture = turned_on_after_day_14();
        for (index, answer) in WorkBlockRatingAnswer::ALL.into_iter().enumerate() {
            let start = 16 * DAY + 3 * 3_600 * index as i64;
            let (id, _) = block(&fixture, start, 1_800);
            assert!(fixture
                .rating
                .record_answer(id, answer, false, at(start + 1_830))
                .unwrap());
            let row = fixture
                .database
                .self_rating_repo()
                .rating(&id.to_string())
                .unwrap()
                .unwrap();
            assert_eq!(row.answer, Some(stored(answer)));
            assert!(!row.card_seen_before_answer);
            assert_eq!(
                row.shown_at,
                Some(at(start + 1_830)),
                "the answer is the sighting"
            );
        }
    }

    /// Two hours after the block ended the question is gone, and the row
    /// records the deadline itself as its expiry: a recorded missing answer.
    #[test]
    fn the_question_expires_two_hours_after_the_block_ended() {
        let fixture = turned_on_after_day_14();
        let (id, _) = block(&fixture, 16 * DAY, 1_800);
        let end = 16 * DAY + 1_800;
        assert!(fixture.rating.record_shown(id, at(end + 10)).unwrap());
        assert!(fixture
            .manager
            .request_state(at(end + 7_199))
            .unwrap()
            .rating_prompt
            .is_some());
        assert_eq!(
            fixture
                .manager
                .request_state(at(end + 9_000))
                .unwrap()
                .rating_prompt,
            None
        );
        assert!(!fixture
            .rating
            .record_answer(id, WorkBlockRatingAnswer::Most, false, at(end + 9_001))
            .unwrap());
        let row = fixture
            .database
            .self_rating_repo()
            .rating(&id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(row.expired_at, Some(at(end + 7_200)));
        assert_eq!(row.answer, None);
    }

    /// The next block's start ends the question about the last one, at that
    /// start, and an answer for it during the next block is refused.
    #[test]
    fn the_next_block_ends_the_question_about_the_last_one() {
        let fixture = turned_on_after_day_14();
        let (id, _) = block(&fixture, 16 * DAY, 1_800);
        let end = 16 * DAY + 1_800;
        let next = fixture
            .manager
            .start(request(1_800), at(end + 600))
            .unwrap();
        assert_eq!(next.rating_prompt, None, "never during a block");
        assert_eq!(next.rating_opt_in, None);
        assert!(!fixture
            .rating
            .record_answer(id, WorkBlockRatingAnswer::Most, false, at(end + 660))
            .unwrap());
        assert_eq!(
            fixture
                .database
                .self_rating_repo()
                .rating(&id.to_string())
                .unwrap()
                .unwrap()
                .expired_at,
            Some(at(end + 600))
        );
    }

    /// A Mac asleep through a block's deadline finishes the block on waking,
    /// at the deadline. The question is judged at the waking, not at the
    /// deadline, so a question that already expired is not shown.
    #[test]
    fn a_question_that_expired_while_the_mac_slept_is_not_shown() {
        let fixture = turned_on_after_day_14();
        let started = fixture.manager.start(request(1_800), at(16 * DAY)).unwrap();
        let woke = fixture
            .manager
            .request_state(at(16 * DAY + 1_800 + 3 * 3_600))
            .unwrap();
        assert_eq!(woke.phase, WorkBlockPhase::Completed);
        assert_eq!(woke.rating_prompt, None);
        let row = fixture
            .database
            .self_rating_repo()
            .rating(&started.block_id.unwrap().to_string())
            .unwrap()
            .expect("eligible though never shown: a recorded missing answer");
        assert_eq!(row.eligible_at, at(16 * DAY + 1_800));
        assert_eq!(row.expired_at, Some(at(16 * DAY + 1_800 + 7_200)));
        assert_eq!(row.shown_at, None);
    }

    /// Off hides an open question and refuses every report about it. The row
    /// stays, and expires on its own schedule.
    #[test]
    fn turning_it_off_hides_the_question_and_refuses_reports() {
        let fixture = turned_on_after_day_14();
        let (id, _) = block(&fixture, 16 * DAY, 1_800);
        let end = 16 * DAY + 1_800;
        let settings = fixture.rating.set_enabled(false, at(end + 30)).unwrap();
        assert!(settings.available && !settings.enabled);
        let after = fixture.manager.request_state(at(end + 60)).unwrap();
        assert_eq!(after.rating_prompt, None);
        assert_eq!(
            after.rating_opt_in, None,
            "the invitation was answered by turning it on"
        );
        assert!(!fixture.rating.record_shown(id, at(end + 61)).unwrap());
        assert!(!fixture.rating.record_not_now(id, at(end + 62)).unwrap());
        assert!(!fixture
            .rating
            .record_answer(id, WorkBlockRatingAnswer::Most, false, at(end + 63))
            .unwrap());
        let row = fixture
            .database
            .self_rating_repo()
            .rating(&id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(row.shown_at, None);
        assert_eq!(row.expired_at, None);
        fixture.manager.request_state(at(end + 7_200)).unwrap();
        assert_eq!(
            fixture
                .database
                .self_rating_repo()
                .rating(&id.to_string())
                .unwrap()
                .unwrap()
                .expired_at,
            Some(at(end + 7_200))
        );
    }

    /// Off and on again inside the answer window never brings the question
    /// back. The block's result card may have been on screen while it was
    /// off, so an answer given afterwards would not be a first answer.
    /// Reports about it stay refused, its row expires at its deadline, and a
    /// block that starts after the turn-on is asked about as usual.
    #[test]
    fn turning_it_off_and_on_again_never_brings_a_question_back() {
        let fixture = turned_on_after_day_14();
        let (id, ended) = block(&fixture, 16 * DAY, 1_800);
        assert!(ended.rating_prompt.is_some());
        let end = 16 * DAY + 1_800;
        fixture.rating.set_enabled(false, at(end + 30)).unwrap();
        assert!(
            fixture
                .rating
                .set_enabled(true, at(end + 60))
                .unwrap()
                .enabled
        );

        let after = fixture.manager.request_state(at(end + 90)).unwrap();
        assert_eq!(after.rating_prompt, None);
        assert_eq!(after.rating_opt_in, None);
        assert!(!fixture.rating.record_shown(id, at(end + 91)).unwrap());
        assert!(!fixture.rating.record_not_now(id, at(end + 92)).unwrap());
        assert!(!fixture
            .rating
            .record_answer(id, WorkBlockRatingAnswer::All, false, at(end + 93))
            .unwrap());

        fixture.manager.request_state(at(end + 7_200)).unwrap();
        let row = fixture
            .database
            .self_rating_repo()
            .rating(&id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!((row.shown_at, row.answer), (None, None));
        assert_eq!(row.expired_at, Some(at(end + 7_200)));

        let (_, next) = block(&fixture, 16 * DAY + 3 * 3_600, 1_800);
        assert!(next.rating_prompt.is_some());
    }

    /// The setting's words come from Rust, and the toggle is offered while
    /// the question is on even if the period restarted under it.
    #[test]
    fn the_settings_pane_reads_its_words_and_state_from_rust() {
        let fixture = fixture();
        let none = fixture.rating.settings(at(0)).unwrap();
        assert_eq!(
            none,
            SettingsPayload {
                available: false,
                enabled: false,
                title: SETTINGS_TITLE.into(),
                subtitle: SETTINGS_SUBTITLE.into(),
            },
            "no block, no period"
        );
        let fixture = turned_on_after_day_14();
        fixture.manager.clear_data().unwrap();
        let cleared = fixture.rating.settings(at(16 * DAY)).unwrap();
        assert!(cleared.enabled && cleared.available);
    }

    // -----------------------------------------------------------------------
    // Copy
    // -----------------------------------------------------------------------

    /// Version 1's words, pinned. A change to any of them is a new
    /// `RATING_PROMPT_VERSION`, because answers given to two wordings are
    /// never pooled.
    #[test]
    fn version_1_copy_is_frozen() {
        assert_eq!(RATING_PROMPT_VERSION, 1);
        assert_eq!(
            every_copy_string(),
            [
                "How much of this block was on what you meant to do?",
                "Not now",
                "Optional. Your answer stays on this Mac unless you send it in an export.",
                "Noted.",
                "Answer the question about this block",
                "When a block ends, Velvt can ask how much of it was on what you meant to do. \
                 One tap, always optional, never during a block. Your answers stay on this Mac \
                 unless you send them in an export. You can turn it off in Settings.",
                "Turn on",
                "Ask one question after each block",
                "Never during a block. Answers stay on this Mac unless you send an export.",
                "All or nearly all",
                "Most",
                "About half",
                "Less than half",
                "Little or none",
            ]
        );
    }

    /// Words the question after a block must never use, beyond the registered
    /// lists: it never grades focus, never scores, and never claims the
    /// product learns from the answer (plan § 1.2; `FACTS.md`).
    const NEVER_IN_RATING_COPY: &[&str] = &[
        "bad focus",
        "good focus",
        "focus",
        "score",
        "grade",
        "bad",
        "good",
        "learn",
        "adapt",
        "predict",
        "smarter",
        "behavioral model",
        "behavioural model",
        "personaliz",
        "productiv",
        "attention",
        "accura",
    ];

    #[test]
    fn rating_copy_carries_no_banned_jargon_or_grading_word() {
        for copy in every_copy_string() {
            let lower = copy.to_lowercase();
            for banned in BANNED_COPY_TOKENS
                .iter()
                .chain(BANNED_JARGON_TOKENS)
                .chain(NEVER_IN_RATING_COPY)
            {
                assert!(!lower.contains(banned), "{copy:?} contains {banned:?}");
            }
        }
    }

    /// The copy fits the schema's bounds, so a frame can never fail Swift's
    /// decode on length.
    #[test]
    fn rating_copy_fits_the_schema_bounds() {
        assert!(QUESTION.len() <= 120);
        for answer in WorkBlockRatingAnswer::ALL {
            assert!(answer_label(answer).len() <= 40);
        }
        assert!(NOT_NOW.len() <= 40 && ACKNOWLEDGEMENT.len() <= 40 && OPT_IN_TURN_ON.len() <= 40);
        assert!(FOOTNOTE.len() <= 200 && SETTINGS_SUBTITLE.len() <= 200);
        assert!(LINK.len() <= 80 && SETTINGS_TITLE.len() <= 80);
        assert!(OPT_IN_BODY.len() <= 400);
    }
}
