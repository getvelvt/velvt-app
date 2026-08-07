//! Device-local Focus/DND evidence and the deterministic quiet-hours offer.
//!
//! Swift observes coarse system Focus/DND transitions and reports them over
//! IPC; this module owns the evidence record and every decision derived from
//! it (roadmap invariants 5 and 8; DECISIONS D2). Velvt never delivers
//! around, retries against, or works around Focus/DND — the only thing this
//! module can ever do with the channel is hold Velvt's own output.
//!
//! Everything here is a deterministic, versioned policy. Nothing is learned,
//! trained, or adaptive, and no Focus mode name, configuration, or schedule
//! is representable in storage, IPC, or logs.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};
use velvt_shared_types::QuietHoursOffer;

use crate::persistence::{
    FocusRepo, FocusTransition, PersistenceError, QuietHoursOfferResponse, VelvtQuietHours,
};
use crate::work_block::FocusStateSource;

/// Version of the deterministic late-night DND pattern rule and its offer
/// policy. Bump when any constant below changes meaning.
pub const DND_PATTERN_RULE_VERSION: u32 = 1;

/// Focus transition times are floored to this bucket before storage — the
/// same five-minute granularity the version-1 dashboard switching-cluster
/// rule already treats as safe. Reconciliation needs nothing finer, and a
/// DND schedule cannot be reconstructed from five-minute edges pruned after
/// the retention window.
const FOCUS_EVIDENCE_BUCKET_SECONDS: i64 = 300;
/// Focus evidence older than this is pruned on every ingest.
const FOCUS_EVIDENCE_RETENTION_DAYS: i64 = 14;
/// Rule v1 "late night": a Focus/DND activation observed in local hours
/// 22:00-01:59.
const LATE_NIGHT_HOURS: [u32; 4] = [22, 23, 0, 1];
/// Rule v1 trigger: late-night DND on at least this many distinct local
/// days...
const LATE_NIGHT_MIN_DISTINCT_DAYS: usize = 3;
/// ...within this trailing window of local days.
const LATE_NIGHT_LOOKBACK_DAYS: i64 = 7;
/// The offer surfaces only in a morning window strictly after the trigger.
const OFFER_MORNING_START_HOUR: u32 = 6;
const OFFER_MORNING_END_HOUR: u32 = 12;
/// A declined offer is not re-asked for this many days (rule v1).
const QUIET_HOURS_DECLINE_REASK_DAYS: i64 = 30;
/// The offered quiet-hours window: 22:00 to 07:00 local.
const QUIET_START_LOCAL_MINUTES: u32 = 22 * 60;
const QUIET_END_LOCAL_MINUTES: u32 = 7 * 60;

#[derive(Debug, thiserror::Error)]
pub enum FocusError {
    #[error("focus persistence unavailable")]
    Persistence(#[from] PersistenceError),
}

pub struct FocusManager {
    repo: Arc<dyn FocusRepo>,
}

impl FocusManager {
    pub fn new(repo: Arc<dyn FocusRepo>) -> Arc<Self> {
        Arc::new(Self { repo })
    }

    /// Ingests one observed Focus/DND transition. Coarse by construction:
    /// the time is floored to the five-minute bucket and reduced to local
    /// hour/date buckets before storage, old evidence is pruned, and a
    /// repeat of the current state stores nothing.
    pub fn record_transition(
        &self,
        active: bool,
        occurred_at: DateTime<Utc>,
        utc_offset_seconds: i32,
        now: DateTime<Utc>,
    ) -> Result<(), FocusError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        self.repo.set_utc_offset(offset_seconds, now)?;
        self.repo
            .prune_focus_evidence(now - Duration::days(FOCUS_EVIDENCE_RETENTION_DAYS))?;
        let bucket = floor_to_bucket(occurred_at);
        let local = to_local(occurred_at, offset_seconds);
        self.repo.record_focus_transition(&FocusTransition {
            active,
            changed_at_bucket: bucket,
            local_hour: local.hour(),
            local_date: format_local_date(&local),
            recorded_at: now,
        })?;
        self.evaluate_pattern_rule(now)?;
        Ok(())
    }

    /// Rule v1, evaluated on ingest: late-night DND activations on at least
    /// three distinct local days within the trailing seven produce one
    /// trigger. A configured quiet-hours setting, a pending offer, or a
    /// decline younger than the re-ask interval suppresses re-triggering.
    fn evaluate_pattern_rule(&self, now: DateTime<Utc>) -> Result<(), FocusError> {
        if self.repo.quiet_hours()?.is_some() {
            return Ok(());
        }
        if let Some(state) = self.repo.quiet_hours_offer_state()? {
            match state.response {
                Some(QuietHoursOfferResponse::Accepted) => return Ok(()),
                Some(QuietHoursOfferResponse::Declined) => {
                    if let Some(responded_at) = state.responded_at {
                        if now < responded_at + Duration::days(QUIET_HOURS_DECLINE_REASK_DAYS) {
                            return Ok(());
                        }
                    }
                }
                None => {
                    if state.triggered_at.is_some() {
                        // A live trigger/offer is already in flight.
                        return Ok(());
                    }
                }
            }
        }
        let Some(offset_seconds) = self.repo.utc_offset_seconds()? else {
            return Ok(());
        };
        let lookback = recent_local_dates(now, offset_seconds, LATE_NIGHT_LOOKBACK_DAYS);
        let matching_days = self
            .repo
            .focus_active_dates_in_hours(&LATE_NIGHT_HOURS)?
            .into_iter()
            .filter(|date| lookback.contains(date))
            .count();
        if matching_days >= LATE_NIGHT_MIN_DISTINCT_DAYS {
            self.repo
                .record_quiet_hours_trigger(DND_PATTERN_RULE_VERSION, now)?;
        }
        Ok(())
    }

    /// Returns the quiet-hours offer when a trigger exists, is unanswered,
    /// and `now` falls in a local morning window strictly after the trigger.
    /// The next morning, not the moment of the pattern: the offer is a calm
    /// daytime question, never a late-night interruption.
    pub fn pending_morning_offer(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<QuietHoursOffer>, FocusError> {
        let Some(state) = self.repo.quiet_hours_offer_state()? else {
            return Ok(None);
        };
        if state.response.is_some() {
            return Ok(None);
        }
        let Some(triggered_at) = state.triggered_at else {
            return Ok(None);
        };
        let Some(offset_seconds) = self.repo.utc_offset_seconds()? else {
            return Ok(None);
        };
        let local_now = to_local(now, offset_seconds);
        if !(OFFER_MORNING_START_HOUR..OFFER_MORNING_END_HOUR).contains(&local_now.hour()) {
            return Ok(None);
        }
        // Strictly after the late-night trigger, so a trigger at 00:30
        // surfaces the same local morning and one at 23:00 the next.
        if now <= triggered_at {
            return Ok(None);
        }
        let late_night_days = self
            .repo
            .focus_active_dates_in_hours(&LATE_NIGHT_HOURS)?
            .into_iter()
            .filter(|date| {
                recent_local_dates(now, offset_seconds, LATE_NIGHT_LOOKBACK_DAYS).contains(date)
            })
            .count()
            .max(1) as u32;
        self.repo.record_quiet_hours_offered(now)?;
        Ok(Some(QuietHoursOffer {
            rule_version: state.rule_version,
            late_night_days,
            start_local_minutes: QUIET_START_LOCAL_MINUTES,
            end_local_minutes: QUIET_END_LOCAL_MINUTES,
            body: quiet_hours_offer_body(late_night_days),
        }))
    }

    /// Records the one-tap reply. Accepting configures Velvt's own quiet
    /// hours; declining is remembered for the versioned re-ask interval and
    /// changes nothing else about the product.
    pub fn respond_to_offer(&self, accepted: bool, now: DateTime<Utc>) -> Result<(), FocusError> {
        let response = if accepted {
            QuietHoursOfferResponse::Accepted
        } else {
            QuietHoursOfferResponse::Declined
        };
        self.repo.record_quiet_hours_response(response, now)?;
        if accepted {
            self.repo.set_quiet_hours(&VelvtQuietHours {
                start_local_minutes: QUIET_START_LOCAL_MINUTES,
                end_local_minutes: QUIET_END_LOCAL_MINUTES,
                rule_version: DND_PATTERN_RULE_VERSION,
                configured_at: now,
            })?;
        }
        Ok(())
    }

    /// Whether `now` falls inside Velvt's own configured quiet hours. Quiet
    /// hours only ever reduce delivery; they never move, retry, or reroute
    /// anything.
    pub fn in_velvt_quiet_hours(&self, now: DateTime<Utc>) -> bool {
        let Ok(Some(quiet_hours)) = self.repo.quiet_hours() else {
            return false;
        };
        let Ok(Some(offset_seconds)) = self.repo.utc_offset_seconds() else {
            return false;
        };
        let local = to_local(now, offset_seconds);
        let minutes = local.hour() * 60 + local.minute();
        let start = quiet_hours.start_local_minutes;
        let end = quiet_hours.end_local_minutes;
        if start <= end {
            (start..end).contains(&minutes)
        } else {
            minutes >= start || minutes < end
        }
    }

    /// Clears Focus evidence, the stored offset, and offer memory. Velvt's
    /// quiet-hours setting is an explicit user choice and survives; the
    /// evidence that argued for it does not.
    pub fn clear_evidence(&self) -> Result<(), FocusError> {
        self.repo.clear_focus_evidence()?;
        Ok(())
    }
}

impl FocusStateSource for FocusManager {
    /// Coarse Focus state at `at`: the latest stored transition at or before
    /// the same five-minute bucket. Unknown means inactive — absence of
    /// evidence never suppresses or records anything.
    fn is_focus_active(&self, at: DateTime<Utc>) -> bool {
        self.repo
            .focus_state_at_bucket(floor_to_bucket(at))
            .ok()
            .flatten()
            .unwrap_or(false)
    }
}

fn floor_to_bucket(at: DateTime<Utc>) -> DateTime<Utc> {
    let seconds =
        at.timestamp().div_euclid(FOCUS_EVIDENCE_BUCKET_SECONDS) * FOCUS_EVIDENCE_BUCKET_SECONDS;
    DateTime::from_timestamp(seconds, 0).unwrap_or(at)
}

fn to_local(at: DateTime<Utc>, offset_seconds: i32) -> DateTime<FixedOffset> {
    let offset =
        FixedOffset::east_opt(offset_seconds).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    at.with_timezone(&offset)
}

fn format_local_date(local: &DateTime<FixedOffset>) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        local.year(),
        local.month(),
        local.day()
    )
}

/// The trailing `days` local calendar dates ending at `now`, inclusive.
fn recent_local_dates(now: DateTime<Utc>, offset_seconds: i32, days: i64) -> Vec<String> {
    (0..days)
        .map(|back| format_local_date(&to_local(now - Duration::days(back), offset_seconds)))
        .collect()
}

/// Registered analyst-voice copy for the quiet-hours offer. States the
/// evidence and the offer; references no failure, no history, and nothing
/// the user missed. This is a versioned deterministic policy, so the copy
/// never describes itself as learned.
fn quiet_hours_offer_body(late_night_days: u32) -> String {
    let nights = if late_night_days == 1 {
        "1 recent late night".to_owned()
    } else {
        format!("{late_night_days} recent late nights")
    };
    format!(
        "Do Not Disturb was on during {nights}. Velvt can hold its own notifications \
         from 22:00 to 07:00 — one tap turns that on, and declining changes nothing."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SqlitePersistence;

    /// 2027-01-15T08:00:00Z, a fixed anchor for deterministic local math.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn hours(value: i64) -> i64 {
        value * 3600
    }

    fn days(value: i64) -> i64 {
        value * 86_400
    }

    fn manager_with_repo() -> (Arc<FocusManager>, Arc<dyn FocusRepo>) {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let repo = db.focus_repo();
        (FocusManager::new(repo.clone()), repo)
    }

    /// Records a late-night DND on/off pair on the evening `days_back` days
    /// before the anchor, at 23:00 UTC (= 23:00 local with offset 0).
    fn late_night_dnd(manager: &FocusManager, days_back: i64) {
        let on = at(-days(days_back) + hours(15)); // 23:00 on the prior local day
        manager.record_transition(true, on, 0, on).unwrap();
        let off = at(-days(days_back) + hours(16));
        manager.record_transition(false, off, 0, off).unwrap();
    }

    #[test]
    fn transition_times_are_bucketed_and_repeats_are_not_stored() {
        let (manager, repo) = manager_with_repo();
        manager
            .record_transition(true, at(4 * 60 + 33), 0, at(4 * 60 + 33))
            .unwrap();
        // Same state re-sampled later stores nothing.
        manager
            .record_transition(true, at(20 * 60), 0, at(20 * 60))
            .unwrap();
        let stored = repo.latest_focus_transition().unwrap().unwrap();
        assert_eq!(stored.changed_at_bucket, at(0), "not floored to 300s");
        assert!(stored.active);
        assert_eq!(
            repo.focus_active_dates_in_hours(&[8]).unwrap().len(),
            1,
            "a repeated sample of an unchanged state must not add evidence"
        );
    }

    /// Coarse-only storage: the evidence table has no column that could hold
    /// a Focus mode's name, configuration, or schedule, and timestamps are
    /// stored only at bucket granularity.
    #[test]
    fn focus_evidence_schema_is_structurally_coarse() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let schema = db
            .schema_sql()
            .unwrap()
            .into_iter()
            .find(|sql| sql.contains("CREATE TABLE focus_state_evidence"))
            .expect("focus evidence table exists");
        for forbidden in ["name", "mode", "schedule", "config", "title", "label"] {
            assert!(
                !schema.to_ascii_lowercase().contains(forbidden),
                "focus evidence schema contains {forbidden:?}: {schema}"
            );
        }
    }

    #[test]
    fn focus_state_at_reads_the_latest_edge_at_or_before_the_bucket() {
        let (manager, _repo) = manager_with_repo();
        manager.record_transition(true, at(0), 0, at(0)).unwrap();
        manager
            .record_transition(false, at(1800), 0, at(1800))
            .unwrap();
        assert!(!manager.is_focus_active(at(-300)), "before any evidence");
        assert!(manager.is_focus_active(at(60)));
        assert!(manager.is_focus_active(at(1500)));
        assert!(!manager.is_focus_active(at(1900)));
    }

    #[test]
    fn three_distinct_late_nights_in_seven_days_trigger_the_offer_next_morning() {
        let (manager, repo) = manager_with_repo();
        late_night_dnd(&manager, 3);
        late_night_dnd(&manager, 2);
        assert!(
            repo.quiet_hours_offer_state()
                .unwrap()
                .and_then(|state| state.triggered_at)
                .is_none(),
            "two distinct days must not trigger rule v1"
        );
        late_night_dnd(&manager, 1);
        let state = repo.quiet_hours_offer_state().unwrap().unwrap();
        assert_eq!(state.rule_version, DND_PATTERN_RULE_VERSION);
        assert!(state.triggered_at.is_some());

        // Not offered during the night of the trigger.
        assert!(manager
            .pending_morning_offer(at(-days(1) + hours(16) + 60))
            .unwrap()
            .is_none());
        // Offered in the next local morning window (anchor is 08:00 local).
        let offer = manager
            .pending_morning_offer(at(0))
            .unwrap()
            .expect("morning after the pattern offers quiet hours");
        assert_eq!(offer.rule_version, 1);
        assert_eq!(offer.late_night_days, 3);
        assert_eq!(offer.start_local_minutes, 22 * 60);
        assert_eq!(offer.end_local_minutes, 7 * 60);
    }

    #[test]
    fn accepting_configures_velvt_quiet_hours_and_stops_reoffers() {
        let (manager, repo) = manager_with_repo();
        for back in [3, 2, 1] {
            late_night_dnd(&manager, back);
        }
        assert!(manager.pending_morning_offer(at(0)).unwrap().is_some());
        manager.respond_to_offer(true, at(60)).unwrap();

        let quiet_hours = repo.quiet_hours().unwrap().unwrap();
        assert_eq!(quiet_hours.start_local_minutes, 22 * 60);
        assert_eq!(quiet_hours.end_local_minutes, 7 * 60);
        assert!(manager.pending_morning_offer(at(120)).unwrap().is_none());
        // 23:30 local is inside the accepted window; 08:00 local is not.
        assert!(manager.in_velvt_quiet_hours(at(hours(15) + 30 * 60)));
        assert!(!manager.in_velvt_quiet_hours(at(0)));
        // The pattern rule never re-triggers once quiet hours exist.
        late_night_dnd(&manager, 0);
        assert!(manager
            .pending_morning_offer(at(days(1)))
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_decline_is_remembered_and_not_reasked_inside_the_versioned_interval() {
        let (manager, repo) = manager_with_repo();
        for back in [3, 2, 1] {
            late_night_dnd(&manager, back);
        }
        assert!(manager.pending_morning_offer(at(0)).unwrap().is_some());
        manager.respond_to_offer(false, at(60)).unwrap();
        assert!(
            repo.quiet_hours().unwrap().is_none(),
            "declining configures nothing"
        );
        assert!(manager.pending_morning_offer(at(120)).unwrap().is_none());

        // Fresh nightly pattern inside the 30-day interval: no new trigger.
        for forward in 1..=3 {
            let on = at(days(forward) + hours(15));
            manager.record_transition(true, on, 0, on).unwrap();
            let off = at(days(forward) + hours(16));
            manager.record_transition(false, off, 0, off).unwrap();
        }
        assert!(manager
            .pending_morning_offer(at(days(4)))
            .unwrap()
            .is_none());

        // Past the interval the same deterministic pattern may ask once more.
        for forward in 31..=33 {
            let on = at(days(forward) + hours(15));
            manager.record_transition(true, on, 0, on).unwrap();
            let off = at(days(forward) + hours(16));
            manager.record_transition(false, off, 0, off).unwrap();
        }
        assert!(manager
            .pending_morning_offer(at(days(34)))
            .unwrap()
            .is_some());
    }

    #[test]
    fn offer_copy_is_analyst_voice_with_no_history_references() {
        for count in [1, 3, 7] {
            let copy = quiet_hours_offer_body(count);
            let lowered = copy.to_ascii_lowercase();
            for forbidden in crate::work_block::BANNED_COPY_TOKENS {
                assert!(
                    !lowered.contains(forbidden),
                    "{forbidden:?} in offer copy {copy:?}"
                );
            }
        }
    }

    #[test]
    fn clear_evidence_removes_evidence_and_offer_memory() {
        let (manager, repo) = manager_with_repo();
        for back in [3, 2, 1] {
            late_night_dnd(&manager, back);
        }
        manager.clear_evidence().unwrap();
        assert!(repo.latest_focus_transition().unwrap().is_none());
        assert!(repo.quiet_hours_offer_state().unwrap().is_none());
        assert!(manager.pending_morning_offer(at(0)).unwrap().is_none());
    }
}
