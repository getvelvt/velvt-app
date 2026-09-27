//! The needs-a-category card and the daily reminder (protocol 33;
//! [`CATEGORY_PROMPT_POLICY_VERSION`]).
//!
//! The list of applications and sites Velvt could not categorize was pulled
//! only while its Settings pane was on screen, so the people it exists for
//! never found it. This module decides when Velvt says so, and in what words:
//! an in-app card while anything on the list is unanswered, and at most one
//! notification a local day, only when something new is on the list, never
//! during a focus session, never in Velvt's quiet hours and never while macOS
//! Focus is known to be on.
//!
//! Everything here is a fixed, versioned rule, and every gate only ever
//! suppresses. Answering the card, either way, quiets it until an entry no
//! answer has reached is among the eight it counts; three reminders in a row
//! that nobody opened pause reminders for a week. Nothing shortens a wait or
//! raises a cap, and nothing adapts.
//!
//! "New" is judged against everything that needs a category, not against the
//! eight the list shows: the ledger records every entry above the list's
//! floor, so an entry that moves up into the eight after it was answered, or
//! after a reminder was posted while it was listed below them, is not new.
//!
//! Privacy: the card and the reminder are counts, never names. A reminder's
//! text is kept by macOS Notification Center, outside anything Velvt can
//! delete, so no application name or hostname may ever be in it; the names on
//! the list are dropped at [`ListedCandidates`], before this module sees an
//! entry. A card's id is random and says nothing about any entry. The ledger
//! (`category_prompt_entry`, `category_prompt_card_entry`,
//! `category_prompt_notification`, migration 0041) holds salted keys, card
//! ids, dates, times and counts, and nothing here reaches the network.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use velvt_shared_types::{
    CategoryPrompt, CategoryPromptCard, CategoryPromptNotification, CategoryPromptResponse,
    TriageEntryKind, UnclassifiedTriageEntry,
};

use crate::initiation::{format_local_date, to_local, InvitationGates};
use crate::persistence::{
    CategoryPromptNotificationRecord, CategoryPromptRepo, PersistenceError, RawEventRepo,
    UnclassifiedAppEntry, UnclassifiedSiteEntry, TRIAGE_MAX_ENTRIES, TRIAGE_MIN_SECONDS,
};
use crate::retention::CATEGORY_PROMPT_NOTIFICATION_RETENTION_DAYS;

/// Version of the card-and-reminder policy. Bump when any constant below, or
/// the meaning of an answer, changes. Each reminder row records the version it
/// was decided under.
pub const CATEGORY_PROMPT_POLICY_VERSION: u32 = 1;
/// The list the card and the reminder speak for: the last seven days, which
/// is what "you used this week" in their copy promises. The Settings list
/// takes its own window from the request.
pub const CATEGORY_PROMPT_LOOKBACK_DAYS: u32 = 7;
/// Backoff, never escalation: this many reminders in a row, each followed by
/// no `opened` answer before the next one...
pub const REMINDER_BACKOFF_UNOPENED: usize = 3;
/// ...pause reminders for this long after the latest of them. The card still
/// shows. An `opened` answer ends the run.
pub const REMINDER_BACKOFF_PAUSE_DAYS: i64 = 7;
/// A run counts only reminders posted within this many days of now: the
/// horizon the reminder rows are kept for
/// ([`CATEGORY_PROMPT_NOTIFICATION_RETENTION_DAYS`]), so whether reminders are
/// paused never depends on when the sweep last ran. Part of the policy: a
/// change to that horizon is a change to this rule.
pub const REMINDER_BACKOFF_WINDOW_DAYS: i64 = CATEGORY_PROMPT_NOTIFICATION_RETENTION_DAYS as i64;

#[derive(Debug, thiserror::Error)]
pub enum CategoryPromptError {
    #[error("category prompt persistence unavailable")]
    Persistence(#[from] PersistenceError),
}

/// The needs-a-category list: the applications and the browser sites Velvt
/// could not categorize in the last `lookback_days`, ranked together.
///
/// Each half is bounded by its own query exactly as before (at least
/// [`TRIAGE_MIN_SECONDS`] observed, at most [`TRIAGE_MAX_ENTRIES`], the window
/// clamped to the raw-event horizon). The merge ranks by observed time,
/// longest first, then applications ahead of sites, then by key, and keeps the
/// first [`TRIAGE_MAX_ENTRIES`]: a list of eight is a task, and a list of
/// sixteen is an inventory. The top eight of the union are always inside the
/// top eight of each half, so the per-half caps lose nothing.
///
/// An application Velvt holds no local name for keeps its row, with no name,
/// so the client can say "Unnamed application" without that placeholder ever
/// coming back as the name of a rule.
pub fn needs_a_category(
    raw_events: &dyn RawEventRepo,
    lookback_days: u32,
) -> Result<Vec<UnclassifiedTriageEntry>, PersistenceError> {
    let mut entries = ranked(
        raw_events.unclassified_triage(lookback_days, TRIAGE_MIN_SECONDS, TRIAGE_MAX_ENTRIES)?,
        raw_events.unclassified_site_triage(
            lookback_days,
            TRIAGE_MIN_SECONDS,
            TRIAGE_MAX_ENTRIES,
        )?,
    );
    entries.truncate(TRIAGE_MAX_ENTRIES);
    Ok(entries)
}

/// Everything that needs a category: [`needs_a_category`] with no cap, ranked
/// the same way, so its first [`TRIAGE_MAX_ENTRIES`] entries are that list.
/// For the prompt's ledger, never for a list shown to anyone.
///
/// Bounded by the floor rather than a cap: an entry needs
/// [`TRIAGE_MIN_SECONDS`] in the window. It is the list's query without its
/// LIMIT, so reading it costs what reading the list does, and the prompt then
/// writes one ledger row per entry. Measured on a 30,000-row buffer (a debug
/// build, 2026-09-27): about 45 ms to read either, and a whole
/// [`CategoryPromptManager::pending_prompt`] in about 45 ms with 80 entries
/// and about 100 ms with 4,000, about the most the floor admits in seven days.
pub fn everything_that_needs_a_category(
    raw_events: &dyn RawEventRepo,
    lookback_days: u32,
) -> Result<Vec<UnclassifiedTriageEntry>, PersistenceError> {
    Ok(ranked(
        raw_events.every_unclassified_application(lookback_days)?,
        raw_events.every_unclassified_site(lookback_days)?,
    ))
}

/// The two halves as one list: longest observed first, then applications
/// ahead of sites, then by key.
fn ranked(
    applications: Vec<UnclassifiedAppEntry>,
    sites: Vec<UnclassifiedSiteEntry>,
) -> Vec<UnclassifiedTriageEntry> {
    let mut entries: Vec<UnclassifiedTriageEntry> = applications
        .into_iter()
        .map(|application| UnclassifiedTriageEntry {
            kind: TriageEntryKind::Application,
            stable_id: application.app_stable_id,
            display_name: application.display_name,
            seconds_observed: application.seconds_observed,
            event_count: application.event_count,
        })
        .chain(sites.into_iter().map(|site| UnclassifiedTriageEntry {
            kind: TriageEntryKind::Site,
            stable_id: site.site_stable_id,
            display_name: Some(site.display_name),
            seconds_observed: site.seconds_observed,
            event_count: site.event_count,
        }))
        .collect();
    entries.sort_by(|left, right| {
        right
            .seconds_observed
            .cmp(&left.seconds_observed)
            .then(left.kind.cmp(&right.kind))
            .then_with(|| left.stable_id.cmp(&right.stable_id))
    });
    entries
}

/// One entry of the list as the prompt sees it: which kind it is and its key.
/// No name, by construction.
#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    pub kind: TriageEntryKind,
    pub stable_id: String,
}

impl Candidate {
    /// The ledger's key for this entry: `application:<key>` or `site:<key>`.
    /// The two key domains cannot collide, and the prefix keeps them apart in
    /// one column anyway.
    pub fn entry_key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.stable_id)
    }
}

impl std::fmt::Debug for Candidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Candidate")
            .field("kind", &self.kind)
            .field("stable_id", &"[local_identifier]")
            .finish()
    }
}

/// Where the prompt's candidates come from. A seam so the policy can be tested
/// without seeding a week of events.
pub trait CategoryPromptCandidates: Send + Sync {
    /// Everything that needs a category, ranked as [`needs_a_category`] ranks
    /// it and not capped: the first [`TRIAGE_MAX_ENTRIES`] are the list the
    /// card counts, and the rest are what it has below them.
    fn candidates(&self) -> Result<Vec<Candidate>, PersistenceError>;
}

/// Production candidates: [`everything_that_needs_a_category`] over the last
/// [`CATEGORY_PROMPT_LOOKBACK_DAYS`], with the names dropped here.
pub struct ListedCandidates {
    raw_events: Arc<dyn RawEventRepo>,
}

impl ListedCandidates {
    pub fn new(raw_events: Arc<dyn RawEventRepo>) -> Arc<Self> {
        Arc::new(Self { raw_events })
    }
}

impl CategoryPromptCandidates for ListedCandidates {
    fn candidates(&self) -> Result<Vec<Candidate>, PersistenceError> {
        Ok(
            everything_that_needs_a_category(&*self.raw_events, CATEGORY_PROMPT_LOOKBACK_DAYS)?
                .into_iter()
                .map(|entry| Candidate {
                    kind: entry.kind,
                    stable_id: entry.stable_id,
                })
                .collect(),
        )
    }
}

pub struct CategoryPromptManager {
    repo: Arc<dyn CategoryPromptRepo>,
    candidates: Arc<dyn CategoryPromptCandidates>,
    gates: Arc<dyn InvitationGates>,
}

impl CategoryPromptManager {
    pub fn new(
        repo: Arc<dyn CategoryPromptRepo>,
        candidates: Arc<dyn CategoryPromptCandidates>,
        gates: Arc<dyn InvitationGates>,
    ) -> Arc<Self> {
        Arc::new(Self {
            repo,
            candidates,
            gates,
        })
    }

    /// The card, and at most one reminder a local day, for the list as it is
    /// at `now`.
    ///
    /// Repeat-safe: while the entries the card counts stay the same, the card
    /// keeps its `prompt_id`, so a reconnecting client is handed the same
    /// card. A reminder is claimed in the same transaction that records it,
    /// and a claimed reminder is never handed over again, whether or not the
    /// client managed to post it.
    ///
    /// Nothing at all while a work block is active or paused. Otherwise the
    /// card shows while any entry it would count is unanswered. The reminder
    /// additionally needs: not Velvt's quiet hours, macOS Focus not known to
    /// be on, no reminder yet on this local day, no backoff pause, and an
    /// entry among the eight that no earlier reminder or answer has reached.
    pub fn pending_prompt(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<CategoryPrompt, CategoryPromptError> {
        self.evaluate(now, Some(utc_offset_seconds))
    }

    /// The card alone, for the reply to an answer: never a reminder, so an
    /// answer can never be what brings one.
    pub fn current_card(&self, now: DateTime<Utc>) -> Result<CategoryPrompt, CategoryPromptError> {
        self.evaluate(now, None)
    }

    /// Records an answer to the card `prompt_id`. Either answer stamps every
    /// entry that card covered -- the ones it counted, and every entry listed
    /// below them while it was the latest card -- even when the list has moved
    /// on since, so the card stays away until an entry no answer has reached
    /// is among the eight it counts. `Opened` also ends a run of unopened
    /// reminders. An id Velvt has no card for answers no entry.
    pub fn acknowledge(
        &self,
        prompt_id: &str,
        response: CategoryPromptResponse,
        now: DateTime<Utc>,
    ) -> Result<(), CategoryPromptError> {
        self.repo.acknowledge_prompt(
            prompt_id,
            matches!(response, CategoryPromptResponse::Opened),
            now,
        )?;
        Ok(())
    }

    /// `utc_offset_seconds` is `None` when a reminder may not be claimed.
    fn evaluate(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: Option<i32>,
    ) -> Result<CategoryPrompt, CategoryPromptError> {
        // A focus session is the one time nothing may interrupt: the card is
        // withheld as well as the reminder. The client hides it too, from the
        // block's own state, but the rule is this one.
        if self.gates.live_block_exists()? {
            return Ok(CategoryPrompt::default());
        }
        let listed = self.candidates.candidates()?;
        if listed.is_empty() {
            return Ok(CategoryPrompt::default());
        }
        // Every entry that needs a category is recorded, so the ledger knows
        // the ones below the eight as well; the card, its count and "new" are
        // the eight's.
        let keys: Vec<String> = listed.iter().map(Candidate::entry_key).collect();
        let entries = self.repo.record_listed(&keys, now)?;
        let shown = listed.len().min(TRIAGE_MAX_ENTRIES);
        let (counted_keys, uncounted_keys) = keys.split_at(shown);
        let counted = &entries[..shown];
        if counted.iter().all(|entry| entry.acknowledged_at.is_some()) {
            return Ok(CategoryPrompt::default());
        }
        let prompt_id = self.repo.card_for(counted_keys, uncounted_keys)?;
        let counts = ListCounts::of(&listed[..shown]);

        let mut notification = None;
        if let Some(utc_offset_seconds) = utc_offset_seconds {
            let utc_offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
            let something_new = counted
                .iter()
                .any(|entry| entry.notified_at.is_none() && entry.acknowledged_at.is_none());
            if something_new
                && !self.gates.in_quiet_hours(now)
                && !self.gates.focus_active(now)
                && !reminders_paused(
                    &self.repo.recent_notifications(REMINDER_BACKOFF_UNOPENED)?,
                    now,
                )
            {
                let local_date = format_local_date(&to_local(now, utc_offset_seconds));
                // The primary key on the local date is the daily cap: a second
                // claim for the same day, racing or not, changes nothing. The
                // claim stamps every listed entry, below the eight as well:
                // each needed a category when this reminder was posted, so
                // none is new to the next one.
                if self.repo.claim_notification(
                    &local_date,
                    &keys,
                    counts.total(),
                    CATEGORY_PROMPT_POLICY_VERSION,
                    now,
                )? {
                    notification = Some(notification_copy(counts));
                }
            }
        }
        Ok(CategoryPrompt {
            prompt_id: Some(prompt_id),
            card: Some(card_copy(counts)),
            notification,
        })
    }
}

/// Whether the reminder is in its backoff pause at `now`: the most recent
/// [`REMINDER_BACKOFF_UNOPENED`] reminders posted within
/// [`REMINDER_BACKOFF_WINDOW_DAYS`] were each followed by no `opened` answer
/// before the next one, and the latest was posted less than
/// [`REMINDER_BACKOFF_PAUSE_DAYS`] ago. `recent` is most recent first.
fn reminders_paused(recent: &[CategoryPromptNotificationRecord], now: DateTime<Utc>) -> bool {
    let horizon = now - Duration::days(REMINDER_BACKOFF_WINDOW_DAYS);
    let run: Vec<&CategoryPromptNotificationRecord> = recent
        .iter()
        .take_while(|reminder| reminder.posted_at >= horizon)
        .take(REMINDER_BACKOFF_UNOPENED)
        .collect();
    let Some(latest) = run.first() else {
        return false;
    };
    run.len() == REMINDER_BACKOFF_UNOPENED
        && run.iter().all(|reminder| reminder.opened_at.is_none())
        && now < latest.posted_at + Duration::days(REMINDER_BACKOFF_PAUSE_DAYS)
}

/// How many sites and how many applications the list holds: the only facts
/// about the list the copy is allowed to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListCounts {
    sites: u32,
    applications: u32,
}

impl ListCounts {
    fn of(candidates: &[Candidate]) -> Self {
        let sites = candidates
            .iter()
            .filter(|candidate| candidate.kind == TriageEntryKind::Site)
            .count() as u32;
        Self {
            sites,
            applications: candidates.len() as u32 - sites,
        }
    }

    fn total(self) -> u32 {
        self.sites + self.applications
    }
}

fn counted(count: u32, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// "2 sites and 1 app", "1 site", "3 apps".
fn counted_entries(counts: ListCounts) -> String {
    match (counts.sites, counts.applications) {
        (0, applications) => counted(applications, "app", "apps"),
        (sites, 0) => counted(sites, "site", "sites"),
        (sites, applications) => format!(
            "{} and {}",
            counted(sites, "site", "sites"),
            counted(applications, "app", "apps")
        ),
    }
}

/// "… you used this week doesn't have a category yet." — one sentence both
/// surfaces open with, agreeing in number with what it counts.
fn needs_sentence(counts: ListCounts) -> String {
    let verb = if counts.total() == 1 {
        "doesn't"
    } else {
        "don't"
    };
    format!(
        "{} you used this week {verb} have a category yet.",
        counted_entries(counts)
    )
}

/// Registered card copy. Counts only: no name, no hostname, no time observed,
/// and nothing about an earlier card or reminder.
fn card_copy(counts: ListCounts) -> CategoryPromptCard {
    let reach = match (counts.sites, counts.applications) {
        (1, 0) => "every page of that site",
        (_, 0) => "every page of each site",
        (0, 1) => "every window of that app",
        (0, _) => "every window of each app",
        _ => "every page of a site and every window of an app",
    };
    CategoryPromptCard {
        title: "Needs a category".to_owned(),
        body: format!(
            "{} Choose once and it covers {reach}.",
            needs_sentence(counts)
        ),
        primary_action: if counts.total() == 1 {
            "Choose a category"
        } else {
            "Choose categories"
        }
        .to_owned(),
        secondary_action: "Not now".to_owned(),
        entry_count: counts.total(),
    }
}

/// Registered reminder copy. macOS Notification Center keeps it, so it must
/// never carry a name: counts only, as on the card.
fn notification_copy(counts: ListCounts) -> CategoryPromptNotification {
    let title = match (counts.sites, counts.applications) {
        (1, 0) => "A site needs a category",
        (0, 1) => "An app needs a category",
        (_, 0) => "A few sites need a category",
        (0, _) => "A few apps need a category",
        _ => "A few things need a category",
    };
    CategoryPromptNotification {
        title: title.to_owned(),
        body: format!("{} Choose once in Velvt.", needs_sentence(counts)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{RawEventEntry, SqlitePersistence};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// 2027-01-15T08:00:00Z — a Friday, 08:00 local at offset 0. The anchor
    /// the initiation tests use.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn hours(value: i64) -> i64 {
        value * 3_600
    }

    fn days(value: i64) -> i64 {
        value * 86_400
    }

    fn site(seed: u8) -> Candidate {
        Candidate {
            kind: TriageEntryKind::Site,
            stable_id: format!("{seed:02x}").repeat(32),
        }
    }

    fn application(seed: u8) -> Candidate {
        Candidate {
            kind: TriageEntryKind::Application,
            stable_id: format!("{seed:02x}").repeat(32),
        }
    }

    #[derive(Default)]
    struct FakeGates {
        live_block: AtomicBool,
        quiet_hours: AtomicBool,
        focus: AtomicBool,
    }

    impl InvitationGates for FakeGates {
        fn live_block_exists(&self) -> Result<bool, PersistenceError> {
            Ok(self.live_block.load(Ordering::SeqCst))
        }

        fn in_quiet_hours(&self, _at: DateTime<Utc>) -> bool {
            self.quiet_hours.load(Ordering::SeqCst)
        }

        fn focus_active(&self, _at: DateTime<Utc>) -> bool {
            self.focus.load(Ordering::SeqCst)
        }
    }

    #[derive(Default)]
    struct FakeCandidates(Mutex<Vec<Candidate>>);

    impl FakeCandidates {
        fn set(&self, candidates: Vec<Candidate>) {
            *self.0.lock().unwrap() = candidates;
        }
    }

    impl CategoryPromptCandidates for FakeCandidates {
        fn candidates(&self) -> Result<Vec<Candidate>, PersistenceError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    struct Fixture {
        manager: Arc<CategoryPromptManager>,
        repo: Arc<dyn CategoryPromptRepo>,
        gates: Arc<FakeGates>,
        list: Arc<FakeCandidates>,
        _db: SqlitePersistence,
    }

    fn fixture() -> Fixture {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let repo = db.category_prompt_repo();
        let gates = Arc::new(FakeGates::default());
        let list = Arc::new(FakeCandidates::default());
        let manager = CategoryPromptManager::new(
            Arc::clone(&repo),
            Arc::clone(&list) as Arc<dyn CategoryPromptCandidates>,
            Arc::clone(&gates) as Arc<dyn InvitationGates>,
        );
        Fixture {
            manager,
            repo,
            gates,
            list,
            _db: db,
        }
    }

    fn prompt(fixture: &Fixture, now: DateTime<Utc>) -> CategoryPrompt {
        fixture.manager.pending_prompt(now, 0).unwrap()
    }

    /// The id of the card for `list` at `now`.
    fn prompt_after(fixture: &Fixture, now: DateTime<Utc>, list: Vec<Candidate>) -> String {
        fixture.list.set(list);
        prompt(fixture, now)
            .prompt_id
            .expect("an unanswered list has a card")
    }

    fn answer(fixture: &Fixture, prompt_id: &str, now: DateTime<Utc>) {
        fixture
            .manager
            .acknowledge(prompt_id, CategoryPromptResponse::NotNow, now)
            .unwrap();
    }

    #[test]
    fn an_empty_list_asks_nothing_and_records_nothing() {
        let f = fixture();
        assert_eq!(prompt(&f, at(0)), CategoryPrompt::default());
        assert!(f.repo.recent_notifications(8).unwrap().is_empty());
    }

    #[test]
    fn something_new_brings_the_card_and_one_reminder() {
        let f = fixture();
        f.list.set(vec![site(1), site(2), application(3)]);

        let first = prompt(&f, at(0));

        let prompt_id = first.prompt_id.clone().expect("a card has an id");
        assert_eq!(prompt_id.len(), 64);
        assert!(prompt_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        let card = first.card.expect("the list is unanswered");
        assert_eq!(card.title, "Needs a category");
        assert_eq!(
            card.body,
            "2 sites and 1 app you used this week don't have a category yet. \
             Choose once and it covers every page of a site and every window of an app."
        );
        assert_eq!(card.primary_action, "Choose categories");
        assert_eq!(card.secondary_action, "Not now");
        assert_eq!(card.entry_count, 3);
        let reminder = first.notification.expect("the list is new");
        assert_eq!(reminder.title, "A few things need a category");
        assert_eq!(
            reminder.body,
            "2 sites and 1 app you used this week don't have a category yet. Choose once in Velvt."
        );
        let recorded = f.repo.recent_notifications(8).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].local_date, "2027-01-15");
        assert_eq!(recorded[0].entry_count, 3);
        assert_eq!(recorded[0].policy_version, CATEGORY_PROMPT_POLICY_VERSION);
    }

    /// A reconnect asks again, and is handed the same card and no second
    /// reminder: the reminder was claimed when it was handed over, posted or
    /// not.
    #[test]
    fn the_card_is_repeat_safe_and_a_claimed_reminder_is_never_resent() {
        let f = fixture();
        f.list.set(vec![site(1), application(2)]);
        let first = prompt(&f, at(0));
        assert!(first.notification.is_some());

        for later in [at(1), at(60), at(hours(6))] {
            let again = prompt(&f, later);
            assert_eq!(again.prompt_id, first.prompt_id);
            assert_eq!(again.card, first.card);
            assert_eq!(again.notification, None, "a claimed reminder is consumed");
        }
        // The next day brings nothing either: nothing on the list is new.
        assert_eq!(prompt(&f, at(days(1))).notification, None);
        assert_eq!(f.repo.recent_notifications(8).unwrap().len(), 1);
    }

    /// A card's id is random: it stays while the eight the card counts stay
    /// the same, whatever happens below them, changes when they change, and
    /// never comes back for the same keys, so it says nothing about them.
    #[test]
    fn the_card_id_is_random_and_changes_only_with_what_the_card_counts() {
        let f = fixture();
        f.list.set(vec![site(1), application(2)]);
        let first = prompt(&f, at(0)).prompt_id.unwrap();
        assert_eq!(first.len(), 64);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));

        // The same two in the other order are the same card.
        f.list.set(vec![application(2), site(1)]);
        assert_eq!(
            prompt(&f, at(60)).prompt_id.as_deref(),
            Some(first.as_str())
        );

        let wider = prompt_after(&f, at(120), vec![site(1), application(2), site(3)]);
        assert_ne!(wider, first);
        let narrower = prompt_after(&f, at(180), vec![site(1), application(2)]);
        assert_ne!(narrower, first, "an id is never a function of the keys");
        assert_ne!(narrower, wider);

        // Another install with the same list draws another id.
        let g = fixture();
        g.list.set(vec![site(1), application(2)]);
        assert_ne!(prompt(&g, at(0)).prompt_id.unwrap(), first);

        // Below the eight, entries come and go under the same card.
        let full: Vec<Candidate> = (1..=9).map(site).collect();
        let under_eight = prompt_after(&g, at(60), full.clone());
        let mut churned = full[..8].to_vec();
        churned.extend([site(10), site(11)]);
        assert_eq!(prompt_after(&g, at(120), churned), under_eight);
    }

    /// At most one reminder a local day: a new entry later the same day brings
    /// the card and no reminder; the next local day may remind about it.
    #[test]
    fn one_reminder_a_local_day_and_the_day_rolls_over_locally() {
        let f = fixture();
        f.list.set(vec![site(1)]);
        assert!(prompt(&f, at(0)).notification.is_some());

        f.list.set(vec![site(1), site(2)]);
        let same_day = prompt(&f, at(hours(10)));
        assert!(same_day.card.is_some());
        assert_eq!(same_day.notification, None, "the day's reminder is spent");

        let next_day = prompt(&f, at(days(1)));
        let reminder = next_day.notification.expect("a new local day");
        assert_eq!(
            reminder.body,
            "2 sites you used this week don't have a category yet. Choose once in Velvt."
        );
        let dates: Vec<String> = f
            .repo
            .recent_notifications(8)
            .unwrap()
            .into_iter()
            .map(|reminder| reminder.local_date)
            .collect();
        assert_eq!(dates, vec!["2027-01-16", "2027-01-15"]);
    }

    /// The day is the client's local day. 08:00Z is 23:00 the day before at
    /// UTC-9, so a reminder then and one at 10:00Z (01:00 local) fall on two
    /// local days, while at UTC+0 they would share one.
    #[test]
    fn the_local_date_comes_from_the_clients_offset() {
        let f = fixture();
        f.list.set(vec![site(1)]);
        assert!(f
            .manager
            .pending_prompt(at(0), -9 * 3_600)
            .unwrap()
            .notification
            .is_some());
        f.list.set(vec![site(1), site(2)]);
        assert!(f
            .manager
            .pending_prompt(at(hours(2)), -9 * 3_600)
            .unwrap()
            .notification
            .is_some());
        let dates: Vec<String> = f
            .repo
            .recent_notifications(8)
            .unwrap()
            .into_iter()
            .map(|reminder| reminder.local_date)
            .collect();
        assert_eq!(dates, vec!["2027-01-15", "2027-01-14"]);

        // An offset past any real zone is clamped, not trusted.
        let g = fixture();
        g.list.set(vec![site(1)]);
        assert!(g
            .manager
            .pending_prompt(at(0), i32::MAX)
            .unwrap()
            .notification
            .is_some());
        assert_eq!(
            g.repo.recent_notifications(1).unwrap()[0].local_date,
            "2027-01-16"
        );
    }

    /// A live block withholds everything; quiet hours and Focus withhold the
    /// reminder only, and claim nothing, so the reminder comes once they end.
    #[test]
    fn a_live_block_quiet_hours_and_focus_each_suppress() {
        let f = fixture();
        f.list.set(vec![site(1)]);
        f.gates.live_block.store(true, Ordering::SeqCst);
        assert_eq!(prompt(&f, at(0)), CategoryPrompt::default());
        f.gates.live_block.store(false, Ordering::SeqCst);

        for gate in [&f.gates.quiet_hours, &f.gates.focus] {
            gate.store(true, Ordering::SeqCst);
            let held = prompt(&f, at(60));
            assert!(held.card.is_some(), "the card is not a notification");
            assert_eq!(held.notification, None);
            assert!(f.repo.recent_notifications(8).unwrap().is_empty());
            gate.store(false, Ordering::SeqCst);
        }

        assert!(prompt(&f, at(120)).notification.is_some());
    }

    /// A block that starts while a card is up takes the card away without
    /// answering it; the card comes back unchanged when the block ends.
    #[test]
    fn a_block_hides_a_card_it_does_not_answer() {
        let f = fixture();
        f.list.set(vec![application(1)]);
        let before = prompt(&f, at(0));

        f.gates.live_block.store(true, Ordering::SeqCst);
        assert_eq!(prompt(&f, at(60)), CategoryPrompt::default());
        f.gates.live_block.store(false, Ordering::SeqCst);

        let after = prompt(&f, at(hours(1)));
        assert_eq!(after.prompt_id, before.prompt_id);
        assert_eq!(after.card, before.card);
    }

    /// Only an entry no reminder has counted and no answer has reached is new.
    #[test]
    fn only_a_new_entry_brings_a_reminder() {
        let f = fixture();
        f.list.set(vec![site(1), application(2)]);
        assert!(prompt(&f, at(0)).notification.is_some());

        // The same two, unanswered, on later days: the card, no reminder.
        for day in 1..=3 {
            let later = prompt(&f, at(days(day)));
            assert!(later.card.is_some());
            assert_eq!(later.notification, None, "day {day}");
        }

        // A third joins: it is new, and the reminder counts the whole list.
        f.list.set(vec![site(1), application(2), site(4)]);
        let reminder = prompt(&f, at(days(4))).notification.expect("a new entry");
        assert_eq!(
            reminder.body,
            "2 sites and 1 app you used this week don't have a category yet. Choose once in Velvt."
        );
    }

    /// Either answer quiets the card until an entry the card never showed
    /// joins the list, and an answered entry is not new to the reminder.
    #[test]
    fn an_answer_hides_the_card_until_a_new_entry_arrives() {
        for response in [
            CategoryPromptResponse::NotNow,
            CategoryPromptResponse::Opened,
        ] {
            let f = fixture();
            f.list.set(vec![site(1), application(2)]);
            let shown = f.manager.pending_prompt(at(0), 0).unwrap();
            f.manager
                .acknowledge(shown.prompt_id.as_deref().unwrap(), response, at(30))
                .unwrap();

            assert_eq!(prompt(&f, at(60)), CategoryPrompt::default());
            assert_eq!(
                f.manager.current_card(at(60)).unwrap(),
                CategoryPrompt::default()
            );
            assert_eq!(
                prompt(&f, at(days(1))),
                CategoryPrompt::default(),
                "an answered list stays quiet the next day"
            );

            f.list.set(vec![site(1), application(2), site(3)]);
            let returned = prompt(&f, at(days(1) + 60));
            assert_ne!(returned.prompt_id, shown.prompt_id);
            let card = returned.card.expect("a new entry brings the card back");
            assert_eq!(card.entry_count, 3);
            assert!(returned.notification.is_some(), "and it is new");
        }
    }

    /// An answer reaches what its card covered even when it arrives after the
    /// list has moved on: a card drawn for three, answered one request late
    /// when the list is two of them, answers all three, so the card does not
    /// come back for two entries nobody has news about. An entry the answered
    /// card never covered stays unanswered, and an unknown id answers nothing.
    #[test]
    fn a_late_answer_covers_what_its_card_counted_and_an_unknown_one_nothing() {
        let f = fixture();
        let old = prompt_after(&f, at(0), vec![site(1), site(2), site(3)]);
        let current = prompt_after(&f, at(60), vec![site(1), site(2)]);
        assert_ne!(old, current);

        answer(&f, &"f".repeat(64), at(90));
        assert_eq!(
            prompt(&f, at(100)).prompt_id.as_deref(),
            Some(current.as_str()),
            "an unknown id answers nothing"
        );

        answer(&f, &old, at(120));
        assert_eq!(prompt(&f, at(180)), CategoryPrompt::default());
        f.list.set(vec![site(1), site(2), site(3)]);
        assert_eq!(
            prompt(&f, at(240)),
            CategoryPrompt::default(),
            "the late answer reached the entry that has since left the list"
        );

        // An entry that joined after the answered card was drawn is not
        // answered by it.
        let g = fixture();
        let first = prompt_after(&g, at(0), vec![site(1)]);
        let second = prompt_after(&g, at(60), vec![site(1), site(2)]);
        answer(&g, &first, at(90));
        let still_up = prompt(&g, at(120));
        assert_eq!(still_up.prompt_id.as_deref(), Some(second.as_str()));
        answer(&g, &second, at(150));
        assert_eq!(prompt(&g, at(180)), CategoryPrompt::default());
    }

    /// "New" is judged against everything that needs a category. Nine sites
    /// need one and the card counts eight; after "Not now", the ninth moving
    /// up past the eighth is not news, and brings neither the card nor a
    /// reminder. One that joins after the answer is news when it moves up.
    #[test]
    fn an_entry_that_moves_up_into_the_eight_is_not_new() {
        let f = fixture();
        let nine: Vec<Candidate> = (1..=9).map(site).collect();
        f.list.set(nine.clone());
        let shown = prompt(&f, at(0));
        assert_eq!(shown.card.as_ref().unwrap().entry_count, 8);
        assert!(shown.notification.is_some());
        answer(&f, shown.prompt_id.as_deref().unwrap(), at(30));

        let mut climbed = nine[..7].to_vec();
        climbed.extend([site(9), site(8)]);
        f.list.set(climbed.clone());
        assert_eq!(prompt(&f, at(120)), CategoryPrompt::default());
        assert_eq!(prompt(&f, at(days(1))), CategoryPrompt::default());

        // Joined below the eight after the answer, then moves up: news.
        climbed.push(site(10));
        f.list.set(climbed.clone());
        assert_eq!(prompt(&f, at(days(1) + 60)), CategoryPrompt::default());
        let mut risen = climbed[..7].to_vec();
        risen.extend([site(10), site(9), site(8)]);
        f.list.set(risen);
        let back = prompt(&f, at(days(2)));
        assert_eq!(
            back.card
                .expect("a new entry is among the eight")
                .entry_count,
            8
        );
        assert!(back.notification.is_some());
    }

    /// A reminder counts the eight and stamps everything listed, so an entry
    /// that was below the eight when it was posted is not new to the next
    /// one when it moves up. The card, unanswered, is still up.
    #[test]
    fn a_reminder_covers_the_entries_below_the_eight_too() {
        let f = fixture();
        let nine: Vec<Candidate> = (1..=9).map(site).collect();
        f.list.set(nine.clone());
        assert!(prompt(&f, at(0)).notification.is_some());

        let mut climbed = nine[..7].to_vec();
        climbed.extend([site(9), site(8)]);
        f.list.set(climbed);
        let next_day = prompt(&f, at(days(1)));
        assert!(next_day.card.is_some());
        assert_eq!(next_day.notification, None);
    }

    /// Three reminders in a row that nobody opened pause reminders for seven
    /// days after the latest; the card is unaffected.
    #[test]
    fn three_unopened_reminders_pause_the_reminder_for_a_week() {
        let f = fixture();
        for day in 0..3 {
            f.list.set((1..=day as u8 + 1).map(site).collect());
            assert!(
                prompt(&f, at(days(day))).notification.is_some(),
                "reminder {day}"
            );
        }

        f.list.set((1..=4).map(site).collect());
        let paused = prompt(&f, at(days(3)));
        assert!(paused.card.is_some(), "the pause is the reminder's only");
        assert_eq!(paused.notification, None);
        assert_eq!(prompt(&f, at(days(8) + hours(23))).notification, None);

        // Seven days after the latest (day 2), the pause is over.
        let resumed = prompt(&f, at(days(9)));
        assert!(resumed.notification.is_some());
        // That one was not opened either, so the last three are still
        // unopened: the next new entry waits another week.
        f.list.set((1..=5).map(site).collect());
        assert_eq!(prompt(&f, at(days(10))).notification, None);
        assert!(prompt(&f, at(days(16))).notification.is_some());
    }

    /// An `opened` answer ends the run: the next new entry may remind on the
    /// next local day, however many reminders went unopened before.
    #[test]
    fn opening_the_list_resets_the_run() {
        let f = fixture();
        for day in 0..2 {
            f.list.set((1..=day as u8 + 1).map(site).collect());
            assert!(prompt(&f, at(days(day))).notification.is_some());
        }
        f.list.set((1..=3).map(site).collect());
        let third = prompt(&f, at(days(2)));
        assert!(third.notification.is_some());
        f.manager
            .acknowledge(
                third.prompt_id.as_deref().unwrap(),
                CategoryPromptResponse::Opened,
                at(days(2) + 60),
            )
            .unwrap();

        f.list.set((1..=4).map(site).collect());
        assert!(
            prompt(&f, at(days(3))).notification.is_some(),
            "an opened reminder breaks the run of three"
        );

        // "Not now" is an answer, but not an open: it hides the card and does
        // not end a run.
        let g = fixture();
        for day in 0..3 {
            g.list.set((1..=day as u8 + 1).map(site).collect());
            let shown = prompt(&g, at(days(day)));
            assert!(shown.notification.is_some());
            g.manager
                .acknowledge(
                    shown.prompt_id.as_deref().unwrap(),
                    CategoryPromptResponse::NotNow,
                    at(days(day) + 60),
                )
                .unwrap();
        }
        g.list.set((1..=4).map(site).collect());
        let after = prompt(&g, at(days(3)));
        assert!(after.card.is_some());
        assert_eq!(after.notification, None);
    }

    #[test]
    fn the_backoff_reads_only_the_latest_three_and_their_opens() {
        let reminder = |day: i64, opened: bool| CategoryPromptNotificationRecord {
            local_date: format!("2027-01-{:02}", 15 + day),
            posted_at: at(days(day)),
            entry_count: 1,
            policy_version: 1,
            opened_at: opened.then(|| at(days(day) + 60)),
        };
        let now = at(days(3));
        assert!(!reminders_paused(&[], now));
        assert!(!reminders_paused(
            &[reminder(2, false), reminder(1, false)],
            now
        ));
        assert!(reminders_paused(
            &[reminder(2, false), reminder(1, false), reminder(0, false)],
            now
        ));
        assert!(!reminders_paused(
            &[reminder(2, false), reminder(1, true), reminder(0, false)],
            now
        ));
        assert!(!reminders_paused(
            &[reminder(2, false), reminder(1, false), reminder(0, false)],
            at(days(2 + REMINDER_BACKOFF_PAUSE_DAYS))
        ));
        // Only reminders inside the window count toward a run: two a month
        // before a third are not "in a row" with it.
        assert!(!reminders_paused(
            &[reminder(31, false), reminder(1, false), reminder(0, false)],
            at(days(32))
        ));
        assert!(reminders_paused(
            &[
                reminder(31, false),
                reminder(30, false),
                reminder(29, false)
            ],
            at(days(32))
        ));
    }

    /// Whether reminders pause never depends on when the sweep last ran:
    /// the same three reminders, swept or not, give the same answer.
    #[test]
    fn the_backoff_does_not_depend_on_the_sweep() {
        let run = |sweep: bool| {
            let f = fixture();
            for (index, day) in [0, 1, 31].into_iter().enumerate() {
                f.list.set((1..=index as u8 + 1).map(site).collect());
                assert!(
                    prompt(&f, at(days(day))).notification.is_some(),
                    "day {day}"
                );
            }
            if sweep {
                let cutoff = at(days(32))
                    - Duration::days(CATEGORY_PROMPT_NOTIFICATION_RETENTION_DAYS as i64);
                assert_eq!(f.repo.delete_expired_notifications(cutoff, 8).unwrap(), 2);
            }
            f.list.set((1..=4).map(site).collect());
            prompt(&f, at(days(32))).notification.is_some()
        };
        assert!(run(false), "the two a month back are outside the window");
        assert!(run(true));
    }

    /// Singular and plural, for every list the cap allows, and every rendered
    /// string clean of the registered banned vocabulary and of the capability
    /// claims `scripts/check_banned_copy.py` bans in source.
    #[test]
    fn every_rendered_string_is_counted_correctly_and_clean() {
        let claims = ["learn", "adapt", "predict", "smarter"];
        for sites in 0..=TRIAGE_MAX_ENTRIES as u32 {
            for applications in 0..=(TRIAGE_MAX_ENTRIES as u32 - sites) {
                let counts = ListCounts {
                    sites,
                    applications,
                };
                if counts.total() == 0 {
                    continue;
                }
                let card = card_copy(counts);
                let reminder = notification_copy(counts);
                assert_eq!(card.entry_count, counts.total());
                for text in [
                    &card.title,
                    &card.body,
                    &card.primary_action,
                    &card.secondary_action,
                    &reminder.title,
                    &reminder.body,
                ] {
                    let lowered = text.to_ascii_lowercase();
                    for forbidden in crate::work_block::BANNED_COPY_TOKENS {
                        assert!(!lowered.contains(forbidden), "{forbidden:?} in {text:?}");
                    }
                    for claim in claims {
                        assert!(!lowered.contains(claim), "{claim:?} in {text:?}");
                    }
                    for wrong_number in ["1 sites", "1 apps", " 0 ", "0 sites", "0 apps"] {
                        assert!(!text.contains(wrong_number), "{wrong_number:?} in {text:?}");
                    }
                    assert!(
                        text.len() <= 240,
                        "{text:?} is longer than the schema allows"
                    );
                }
                let agreement = if counts.total() == 1 {
                    "doesn't have"
                } else {
                    "don't have"
                };
                assert!(card.body.contains(agreement), "{}", card.body);
                assert!(reminder.body.contains(agreement), "{}", reminder.body);
            }
        }
        let one_site = ListCounts {
            sites: 1,
            applications: 0,
        };
        assert_eq!(
            card_copy(one_site).body,
            "1 site you used this week doesn't have a category yet. \
             Choose once and it covers every page of that site."
        );
        assert_eq!(card_copy(one_site).primary_action, "Choose a category");
        assert_eq!(notification_copy(one_site).title, "A site needs a category");
        let three_apps = ListCounts {
            sites: 0,
            applications: 3,
        };
        assert_eq!(
            card_copy(three_apps).body,
            "3 apps you used this week don't have a category yet. \
             Choose once and it covers every window of each app."
        );
        assert_eq!(
            notification_copy(ListCounts {
                sites: 0,
                applications: 1
            })
            .body,
            "1 app you used this week doesn't have a category yet. Choose once in Velvt."
        );
    }

    fn unclassified_event(
        id: &str,
        seconds: u64,
        app_key: Option<String>,
        site_key: Option<String>,
    ) -> RawEventEntry {
        RawEventEntry {
            event_id: id.to_owned(),
            stable_id: format!("abs_{id}"),
            label: "unlogged".into(),
            local_display_label: None,
            local_name_suggestion: app_key.as_ref().map(|_| format!("Qwybex {id}")),
            category: "UNLOGGED".into(),
            taxonomy_version: "mvp-2".into(),
            classification_tier: "fallback".into(),
            classification_status: "unclassified".into(),
            classification_confidence: "none".into(),
            classification_source: "fallback".into(),
            occurred_at: Utc::now() - Duration::hours(1),
            duration_seconds: seconds,
            upload_eligible: false,
            app_scope_eligible: site_key.is_none(),
            app_stable_id: app_key,
            site_stable_id: site_key,
        }
    }

    /// Applications and sites share one list: longest first, applications
    /// ahead of sites on a tie, then by key, eight at most, and a site's name
    /// is its host.
    #[test]
    fn the_list_ranks_applications_and_sites_together_and_keeps_eight() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let events = db.raw_event_repo();
        for index in 0..6_u8 {
            let seconds = 600 + u64::from(index) * 60;
            let app = unclassified_event(
                &format!("app-{index}"),
                seconds,
                Some(format!("{:02x}", 0x10 + index).repeat(32)),
                None,
            );
            events.insert(&app).unwrap();
            let visit = unclassified_event(
                &format!("site-{index}"),
                seconds,
                Some("ee".repeat(32)),
                Some(format!("{:02x}", 0x20 + index).repeat(32)),
            );
            events.insert(&visit).unwrap();
            assert!(events
                .record_local_site_name(
                    &visit.event_id,
                    &format!("site-{index}.example.org"),
                    Utc::now()
                )
                .unwrap());
        }

        let list = needs_a_category(&*events, 7).unwrap();

        assert_eq!(list.len(), TRIAGE_MAX_ENTRIES);
        let order: Vec<(TriageEntryKind, u64)> = list
            .iter()
            .map(|entry| (entry.kind, entry.seconds_observed))
            .collect();
        assert_eq!(
            order,
            vec![
                (TriageEntryKind::Application, 900),
                (TriageEntryKind::Site, 900),
                (TriageEntryKind::Application, 840),
                (TriageEntryKind::Site, 840),
                (TriageEntryKind::Application, 780),
                (TriageEntryKind::Site, 780),
                (TriageEntryKind::Application, 720),
                (TriageEntryKind::Site, 720),
            ]
        );
        assert_eq!(list[1].display_name.as_deref(), Some("site-5.example.org"));
        assert_eq!(list[0].display_name.as_deref(), Some("Qwybex app-5"));

        // Everything that needs a category is all twelve, ranked the same
        // way, and its first eight are the list.
        let everything = everything_that_needs_a_category(&*events, 7).unwrap();
        assert_eq!(everything.len(), 12);
        assert_eq!(everything[..TRIAGE_MAX_ENTRIES], list[..]);
        assert!(everything[TRIAGE_MAX_ENTRIES..]
            .iter()
            .all(|entry| entry.seconds_observed < 720));
        let candidates = ListedCandidates::new(events).candidates().unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.stable_id.as_str())
                .collect::<Vec<_>>(),
            everything
                .iter()
                .map(|entry| entry.stable_id.as_str())
                .collect::<Vec<_>>()
        );
    }
}
