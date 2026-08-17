//! Weekly receipts digest and the explain-tap probe bucket (0.1.6 Scope 4).
//!
//! "Report more, interrupt less" (D6): once a local week completes, Velvt
//! writes one digest row — exact bounded counts read from the same stored
//! aggregates the local metrics use — and offers it as a pull-based in-app
//! card, held during quiet hours, Focus/DND, and live blocks exactly like an
//! invitation. One digest, not a dashboard: recoveries and completions
//! lead, the wrong-intervention count appears exactly once, and no streak,
//! chain, or failure tally is representable anywhere in this module (D8;
//! roadmap invariant 6).
//!
//! Everything here is deterministic, versioned, and device-local. Nothing in
//! this module can write to the upload path.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use velvt_shared_types::{WeeklyDigest, WorkBlockResult};

use crate::{
    initiation::InvitationGates,
    persistence::{PersistenceError, ReceiptsRepo, WeeklyDigestRecord},
};

/// Versioned digest policy. Bump when the week definition, count sources,
/// generation rule, or headline selection below changes meaning.
pub const DIGEST_POLICY_VERSION: u32 = 1;
/// Digest rows and probe buckets older than this many completed weeks are
/// pruned at generation time. Deterministic retention, not analytics decay.
const RECEIPTS_RETENTION_WEEKS: u64 = 12;

#[derive(Debug, thiserror::Error)]
pub enum ReceiptsError {
    #[error("receipts persistence unavailable")]
    Persistence(#[from] PersistenceError),
}

/// Owns digest generation, the delivery hold, acknowledgment, and the
/// coarse explain-tap bucket. The delivery gates are the same ones an
/// invitation consults: a digest is opted-into reporting, but it still
/// never lands inside quiet hours, an active Focus mode, or a live block.
pub struct ReceiptsManager {
    repo: Arc<dyn ReceiptsRepo>,
    gates: Arc<dyn InvitationGates>,
}

impl ReceiptsManager {
    pub fn new(repo: Arc<dyn ReceiptsRepo>, gates: Arc<dyn InvitationGates>) -> Arc<Self> {
        Arc::new(Self { repo, gates })
    }

    /// The digest ready to show right now, if any.
    ///
    /// Lazily generates (and freezes) the row for the most recent completed
    /// local week on first request after the week ends; a week in which
    /// nothing at all was counted produces no digest rather than an empty
    /// report. The hold is a delay, never another channel: inside quiet
    /// hours, Focus/DND, or a live block the answer is silence and the same
    /// digest is offered after. An acknowledged digest stays closed.
    pub fn pending_digest(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<Option<WeeklyDigest>, ReceiptsError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let week = previous_local_week(now, offset_seconds);
        let record = match self.repo.weekly_digest(&week.start_local_date)? {
            Some(record) => record,
            None => {
                let Some(record) = self.generate(&week, now)? else {
                    return Ok(None);
                };
                record
            }
        };
        if record.acknowledged_at.is_some() {
            return Ok(None);
        }
        if self.gates.live_block_exists()?
            || self.gates.in_quiet_hours(now)
            || self.gates.focus_active(now)
        {
            return Ok(None);
        }
        self.repo
            .mark_digest_delivered(&week.start_local_date, now)?;
        Ok(Some(digest_payload(&record)))
    }

    /// Generates and stores the digest row for one completed week, reading
    /// every count from the stored aggregates. Returns `None` (and stores
    /// nothing) for a week with nothing to report.
    fn generate(
        &self,
        week: &LocalWeek,
        now: DateTime<Utc>,
    ) -> Result<Option<WeeklyDigestRecord>, ReceiptsError> {
        let blocks_declared = bounded(
            self.repo
                .declared_block_count_between(week.start_utc, week.end_utc)?,
        );
        let blocks_completed = bounded(
            self.repo
                .completed_block_count_between(week.start_utc, week.end_utc)?,
        );
        let recoveries = self
            .repo
            .result_payloads_between(week.start_utc, week.end_utc)?
            .iter()
            .filter_map(|payload| serde_json::from_str::<WorkBlockResult>(payload).ok())
            .fold(0_u32, |total, result| {
                total.saturating_add(result.recovery_count)
            });
        // The numerator of the same stored counter Metrics 2 reads, bounded
        // to the week through the shared SQL body.
        let wrong_interventions = self
            .repo
            .wrong_intervention_counts_between(week.start_utc, week.end_utc)?
            .was_focused;
        let invitations_accepted = bounded(
            self.repo
                .accepted_invitation_count_between(week.start_utc, week.end_utc)?,
        );
        let withheld = bounded(
            self.repo
                .withheld_count_between(week.start_utc, week.end_utc)?,
        );
        if blocks_declared == 0
            && blocks_completed == 0
            && recoveries == 0
            && wrong_interventions == 0
            && invitations_accepted == 0
            && withheld == 0
        {
            return Ok(None);
        }
        let record = WeeklyDigestRecord {
            week_start_local_date: week.start_local_date.clone(),
            generated_at: now,
            blocks_declared,
            blocks_completed,
            recoveries,
            wrong_interventions,
            invitations_accepted,
            withheld,
            digest_version: DIGEST_POLICY_VERSION,
            delivered_at: None,
            acknowledged_at: None,
        };
        self.repo.store_weekly_digest(&record)?;
        if let Some(cutoff) = retention_cutoff(&week.start_local_date) {
            self.repo.prune_receipts_before(&cutoff)?;
        }
        Ok(Some(record))
    }

    /// Records the user's one-tap close. Idempotent bookkeeping only.
    pub fn acknowledge(
        &self,
        week_start_local_date: &str,
        now: DateTime<Utc>,
    ) -> Result<(), ReceiptsError> {
        self.repo.acknowledge_digest(week_start_local_date, now)?;
        Ok(())
    }

    /// Increments the coarse, content-free explain-tap count for the
    /// current local week (D7; roadmap Metrics 5). One bounded integer per
    /// week; which nudge, what evidence, and when within the week are
    /// structurally unrepresentable.
    pub fn record_explain_tap(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<(), ReceiptsError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let week_start = current_local_week_start(now, offset_seconds);
        self.repo.record_explain_tap(&week_start, now)?;
        Ok(())
    }

    /// The stored tap count for the current local week. Local inspection
    /// and tests only; no upload path exists.
    pub fn explain_taps_this_week(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<u64, ReceiptsError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let week_start = current_local_week_start(now, offset_seconds);
        Ok(self.repo.explain_taps_for_week(&week_start)?)
    }

    /// Interventions delivered in the current local week, through the same
    /// delivered predicate the wrong-intervention counter uses. This is the
    /// probe denominator; it is derived from stored rows, never counted in
    /// parallel.
    pub fn interventions_delivered_this_week(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<u64, ReceiptsError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let week = current_local_week(now, offset_seconds);
        Ok(self
            .repo
            .delivered_intervention_count_between(week.start_utc, week.end_utc)?)
    }

    /// Clears digests and probe buckets with the rest of the local
    /// behavioral record (clear-all-data).
    pub fn clear_data(&self) -> Result<(), ReceiptsError> {
        self.repo.clear_receipts()?;
        Ok(())
    }
}

/// One local calendar week: its Monday key and its UTC instant bounds under
/// the client-supplied fixed offset. The bounds are half-open
/// (`start <= t < end`).
struct LocalWeek {
    start_local_date: String,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
}

fn bounded(count: u64) -> u32 {
    count.min(u64::from(u32::MAX)) as u32
}

fn local_monday(now: DateTime<Utc>, offset_seconds: i32) -> NaiveDate {
    let local = now + Duration::seconds(i64::from(offset_seconds));
    let today = local.date_naive();
    today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()))
}

fn week_from_monday(monday: NaiveDate, offset_seconds: i32) -> LocalWeek {
    let start_local = monday.and_hms_opt(0, 0, 0).expect("midnight exists");
    let end_local = (monday + chrono::Days::new(7))
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists");
    let to_utc =
        |naive| Utc.from_utc_datetime(&naive) - Duration::seconds(i64::from(offset_seconds));
    LocalWeek {
        start_local_date: monday.format("%Y-%m-%d").to_string(),
        start_utc: to_utc(start_local),
        end_utc: to_utc(end_local),
    }
}

/// The most recent completed local week (last Monday 00:00 to this Monday
/// 00:00, local time).
fn previous_local_week(now: DateTime<Utc>, offset_seconds: i32) -> LocalWeek {
    let current_monday = local_monday(now, offset_seconds);
    week_from_monday(current_monday - chrono::Days::new(7), offset_seconds)
}

fn current_local_week(now: DateTime<Utc>, offset_seconds: i32) -> LocalWeek {
    week_from_monday(local_monday(now, offset_seconds), offset_seconds)
}

fn current_local_week_start(now: DateTime<Utc>, offset_seconds: i32) -> String {
    local_monday(now, offset_seconds)
        .format("%Y-%m-%d")
        .to_string()
}

/// The prune cutoff: digests keyed strictly before this local Monday are
/// removed. Derived from the digest week itself so retention is a pure
/// function of the stored keys.
fn retention_cutoff(week_start_local_date: &str) -> Option<String> {
    let monday = NaiveDate::parse_from_str(week_start_local_date, "%Y-%m-%d").ok()?;
    let cutoff = monday - chrono::Days::new(7 * RECEIPTS_RETENTION_WEEKS);
    Some(cutoff.format("%Y-%m-%d").to_string())
}

fn digest_payload(record: &WeeklyDigestRecord) -> WeeklyDigest {
    WeeklyDigest {
        week_start_local_date: record.week_start_local_date.clone(),
        blocks_declared: record.blocks_declared,
        blocks_completed: record.blocks_completed,
        recoveries: record.recoveries,
        wrong_interventions: record.wrong_interventions,
        invitations_accepted: record.invitations_accepted,
        withheld: record.withheld,
        headline: digest_headline(
            record.recoveries,
            record.blocks_completed,
            record.blocks_declared,
        ),
        digest_version: record.digest_version,
    }
}

/// Registered analyst-voice headline (D8; roadmap invariants 6 and 7).
/// Recoveries lead as the accumulating positive stat; completions follow;
/// nothing here can render a streak, a chain, or a failure tally, and no
/// branch references what did not happen.
fn digest_headline(recoveries: u32, completed: u32, declared: u32) -> String {
    match (recoveries, declared) {
        (0, 0) => "Your week's receipts, from the evidence Velvt kept.".to_owned(),
        (0, _) => format!("You completed {completed} of {declared} blocks this week."),
        (_, 0) => format!("You returned to your work {recoveries} times this week."),
        _ => format!(
            "You returned {recoveries} times and completed {completed} of {declared} blocks this week."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn week_bounds_are_local_mondays_under_the_supplied_offset() {
        // 2026-08-05 12:00 UTC is a Wednesday; at UTC-8 the local Monday is
        // 2026-08-03, so the previous completed week is Jul 27 - Aug 3.
        let now = Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap();
        let week = previous_local_week(now, -28_800);
        assert_eq!(week.start_local_date, "2026-07-27");
        assert_eq!(
            week.start_utc,
            Utc.with_ymd_and_hms(2026, 7, 27, 8, 0, 0).unwrap()
        );
        assert_eq!(
            week.end_utc,
            Utc.with_ymd_and_hms(2026, 8, 3, 8, 0, 0).unwrap()
        );
        assert_eq!(current_local_week_start(now, -28_800), "2026-08-03");
    }

    #[test]
    fn offset_shifts_which_week_a_boundary_instant_belongs_to() {
        // Monday 2026-08-03 02:00 UTC: still Sunday locally at UTC-8, so the
        // completed week is one earlier than at UTC+0.
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 2, 0, 0).unwrap();
        assert_eq!(previous_local_week(now, 0).start_local_date, "2026-07-27");
        assert_eq!(
            previous_local_week(now, -28_800).start_local_date,
            "2026-07-20"
        );
    }

    #[test]
    fn headline_leads_with_recoveries_and_never_renders_a_tally_of_misses() {
        assert_eq!(
            digest_headline(4, 3, 5),
            "You returned 4 times and completed 3 of 5 blocks this week."
        );
        assert_eq!(
            digest_headline(0, 2, 2),
            "You completed 2 of 2 blocks this week."
        );
        assert_eq!(
            digest_headline(3, 0, 0),
            "You returned to your work 3 times this week."
        );
        for headline in [
            digest_headline(0, 0, 0),
            digest_headline(4, 3, 5),
            digest_headline(0, 2, 5),
        ] {
            let lowered = headline.to_ascii_lowercase();
            for banned in crate::work_block::BANNED_COPY_TOKENS {
                assert!(
                    !lowered.contains(banned),
                    "banned token {banned:?} in digest headline {headline:?}"
                );
            }
        }
    }

    #[test]
    fn retention_cutoff_is_a_pure_function_of_the_week_key() {
        assert_eq!(
            retention_cutoff("2026-07-27").as_deref(),
            Some("2026-05-04")
        );
        assert_eq!(retention_cutoff("not-a-date"), None);
    }
}
