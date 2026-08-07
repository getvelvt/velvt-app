//! Deterministic good-hours windows and the capped initiation invitation
//! (0.1.6 Scope 3; `plan/05-unified-roadmap.md` invariants 1, 2, 7; D1, D4).
//!
//! The good-hours data exists to invite, not to report. Rust derives
//! per-user good-hours windows from the safe local aggregates that already
//! exist — completed work blocks and their confident category dwell —
//! bucketed by local weekday and hour. Everything here is a deterministic,
//! versioned policy with explicit minimum-sample gates: below any gate the
//! policy answers `insufficient_evidence` and extends no invitation, never
//! an invented pattern or a generic default window. Nothing is learned,
//! trained, or adaptive; the learned good-hours model is 0.2.0 and does not
//! exist here.
//!
//! Privacy: buckets, windows, and every timing column live only in the
//! local database. The invitation payload is schedule-free by construction,
//! and no field derived here reaches upload, telemetry, or log paths.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};
use uuid::Uuid;
use velvt_shared_types::InitiationInvitation;

use crate::focus::FocusManager;
use crate::persistence::{
    InitiationInvitationOutcome, InitiationInvitationRecord, InitiationRepo, PersistenceError,
    WorkBlockRepo,
};
use crate::work_block::FocusStateSource;
use velvt_shared_types::WorkBlockPhase;

/// Version of the deterministic good-hours window policy. Bump when any
/// derivation constant below changes meaning.
pub const GOOD_HOURS_POLICY_VERSION: u32 = 1;
/// Completed-block evidence older than this does not argue for a window.
const GOOD_HOURS_LOOKBACK_DAYS: i64 = 28;
/// Cold-start gate: fewer completed blocks than this in the lookback and
/// the policy returns `insufficient_evidence` for every hour.
const GOOD_HOURS_MIN_TOTAL_COMPLETED_BLOCKS: u64 = 6;
/// Per-bucket gate: a (weekday, hour) bucket is a good hour only when at
/// least this many distinct completed blocks contributed dwell to it...
const GOOD_HOURS_MIN_BUCKET_BLOCKS: usize = 3;
/// ...and their confident dwell in the bucket totals at least this much.
const GOOD_HOURS_MIN_BUCKET_DWELL_SECONDS: u32 = 45 * 60;

/// Version of the invitation backoff policy (D1: backoff, never
/// escalation). Bump when any constant below changes meaning.
pub const INVITATION_BACKOFF_POLICY_VERSION: u32 = 1;
/// Hard cap: at most this many invitations per local calendar day.
pub const INVITATION_DAILY_CAP: u64 = 1;
/// The registered invitation action and the block it declares.
const INVITATION_ACTION_ID: &str = "soft_start_25";
const INVITATION_DURATION_SECONDS: u32 = 25 * 60;
/// An unanswered invitation lapses to `no_response` after this long.
const INVITATION_RESPONSE_TTL_SECONDS: i64 = 30 * 60;
/// Minimum spacing between invitations. Every trailing dismissal multiplies
/// this by the versioned multiplier; nothing ever shortens it.
const INVITATION_BASE_COOLDOWN_SECONDS: i64 = 24 * 60 * 60;
const INVITATION_BACKOFF_COOLDOWN_MULTIPLIER: u32 = 2;
/// This many trailing dismissals (uninterrupted by an acceptance) silence
/// invitations entirely...
const INVITATION_SILENCE_DISMISSAL_THRESHOLD: u32 = 3;
/// ...for this long after the most recent dismissal.
const INVITATION_SILENCE_DAYS: i64 = 30;
/// Bounded backoff scan; also caps the cooldown exponent.
const BACKOFF_SCAN_LIMIT: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum InitiationError {
    #[error("initiation persistence unavailable")]
    Persistence(#[from] PersistenceError),
}

/// What the deterministic window policy says about one moment in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoodHoursAssessment {
    /// Below a minimum-sample gate. No invitation, no invented pattern, no
    /// generic default window.
    InsufficientEvidence,
    /// Enough evidence exists to derive windows, and the current hour is
    /// not in one.
    OutsideGoodHours,
    /// The evidence says the user reliably focuses around the current hour.
    GoodHour,
}

/// The delivery gates the invitation consults beyond its own policy: an
/// active or paused block (invariant 1), Velvt's own quiet hours, and the
/// Scope 1 Focus/DND evidence. Every gate only ever suppresses.
pub trait InvitationGates: Send + Sync {
    fn live_block_exists(&self) -> Result<bool, PersistenceError>;
    fn in_quiet_hours(&self, at: DateTime<Utc>) -> bool;
    fn focus_active(&self, at: DateTime<Utc>) -> bool;
}

/// Production gates: the work-block store and the Scope 1 Focus/DND
/// evidence owner.
pub struct RuntimeInvitationGates {
    focus: Arc<FocusManager>,
    blocks: Arc<dyn WorkBlockRepo>,
}

impl RuntimeInvitationGates {
    pub fn new(focus: Arc<FocusManager>, blocks: Arc<dyn WorkBlockRepo>) -> Arc<Self> {
        Arc::new(Self { focus, blocks })
    }
}

impl InvitationGates for RuntimeInvitationGates {
    fn live_block_exists(&self) -> Result<bool, PersistenceError> {
        Ok(self.blocks.latest()?.is_some_and(|record| {
            matches!(
                record.phase,
                WorkBlockPhase::Active | WorkBlockPhase::Paused
            )
        }))
    }

    fn in_quiet_hours(&self, at: DateTime<Utc>) -> bool {
        self.focus.in_velvt_quiet_hours(at)
    }

    fn focus_active(&self, at: DateTime<Utc>) -> bool {
        self.focus.is_focus_active(at)
    }
}

pub struct InitiationManager {
    repo: Arc<dyn InitiationRepo>,
    gates: Arc<dyn InvitationGates>,
}

impl InitiationManager {
    pub fn new(repo: Arc<dyn InitiationRepo>, gates: Arc<dyn InvitationGates>) -> Arc<Self> {
        Arc::new(Self { repo, gates })
    }

    /// Evaluates every gate and returns at most one invitation.
    ///
    /// Repeat-safe: a live unanswered invitation is returned unchanged so a
    /// reconnecting client re-renders the same card; it is never doubled.
    /// Housekeeping is lazy and one-directional — stale or invalidated
    /// invitations resolve to `no_response`/`expired`, and nothing here can
    /// shorten a wait or raise a cap.
    pub fn pending_invitation(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<Option<InitiationInvitation>, InitiationError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let enabled = self.repo.invitations_enabled()?;
        if let Some(open) = self.repo.open_invitation()? {
            if !enabled
                || open.policy_version != GOOD_HOURS_POLICY_VERSION
                || open.backoff_policy_version != INVITATION_BACKOFF_POLICY_VERSION
            {
                self.repo.resolve_invitation(
                    &open.invitation_id,
                    InitiationInvitationOutcome::Expired,
                    now,
                )?;
            } else if now >= open.offered_at + Duration::seconds(INVITATION_RESPONSE_TTL_SECONDS) {
                // Silence is a real outcome, distinct from invalidation.
                self.repo.resolve_invitation(
                    &open.invitation_id,
                    InitiationInvitationOutcome::NoResponse,
                    now,
                )?;
            } else if self.gates.live_block_exists()?
                || self.gates.in_quiet_hours(now)
                || self.gates.focus_active(now)
            {
                self.repo.resolve_invitation(
                    &open.invitation_id,
                    InitiationInvitationOutcome::Expired,
                    now,
                )?;
            } else {
                return Ok(Some(offer_payload(&open)));
            }
        }
        if !enabled {
            return Ok(None);
        }
        if self.gates.live_block_exists()?
            || self.gates.in_quiet_hours(now)
            || self.gates.focus_active(now)
        {
            return Ok(None);
        }
        let local_date = format_local_date(&to_local(now, offset_seconds));
        if self.repo.invitations_on_local_date(&local_date)? >= INVITATION_DAILY_CAP {
            return Ok(None);
        }
        let recent = self.repo.recent_invitations(BACKOFF_SCAN_LIMIT)?;
        let (trailing_dismissals, last_dismissed_at) = trailing_dismissals(&recent);
        if trailing_dismissals >= INVITATION_SILENCE_DISMISSAL_THRESHOLD {
            if let Some(dismissed_at) = last_dismissed_at {
                if now < dismissed_at + Duration::days(INVITATION_SILENCE_DAYS) {
                    return Ok(None);
                }
            }
        }
        if let Some(last) = recent.first() {
            // Backoff, never escalation: each trailing dismissal multiplies
            // the spacing by the versioned constant. Acceptance resets the
            // count; silence and invalidation change nothing.
            let cooldown = INVITATION_BASE_COOLDOWN_SECONDS.saturating_mul(i64::from(
                INVITATION_BACKOFF_COOLDOWN_MULTIPLIER
                    .saturating_pow(trailing_dismissals.min(BACKOFF_SCAN_LIMIT as u32)),
            ));
            if now < last.offered_at + Duration::seconds(cooldown) {
                return Ok(None);
            }
        }
        if self.assess_good_hours(now, offset_seconds)? != GoodHoursAssessment::GoodHour {
            return Ok(None);
        }
        let record = InitiationInvitationRecord {
            invitation_id: Uuid::new_v4().to_string(),
            offered_at: now,
            local_date,
            action_id: INVITATION_ACTION_ID.to_owned(),
            policy_version: GOOD_HOURS_POLICY_VERSION,
            backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
            outcome: InitiationInvitationOutcome::Offered,
            outcome_at: None,
        };
        self.repo.record_invitation(&record)?;
        Ok(Some(offer_payload(&record)))
    }

    /// The deterministic window policy for the moment `now`.
    ///
    /// Evidence: confident category dwell inside completed blocks from the
    /// lookback window, split across local hour boundaries and bucketed by
    /// (weekday, hour). Both gates are explicit minimum-sample gates.
    pub fn assess_good_hours(
        &self,
        now: DateTime<Utc>,
        utc_offset_seconds: i32,
    ) -> Result<GoodHoursAssessment, InitiationError> {
        let offset_seconds = utc_offset_seconds.clamp(-64_800, 64_800);
        let since = now - Duration::days(GOOD_HOURS_LOOKBACK_DAYS);
        if self.repo.completed_block_count(since)? < GOOD_HOURS_MIN_TOTAL_COMPLETED_BLOCKS {
            return Ok(GoodHoursAssessment::InsufficientEvidence);
        }
        let mut dwell_seconds = HashMap::<(u32, u32), u32>::new();
        let mut dwell_blocks = HashMap::<(u32, u32), HashSet<String>>::new();
        for span in self.repo.completed_block_dwell_spans(since)? {
            let mut cursor = span.started_at;
            while cursor < span.ended_at {
                let local = to_local(cursor, offset_seconds);
                let bucket = (local.weekday().num_days_from_monday(), local.hour());
                let next_hour = cursor
                    + Duration::seconds(3_600 - i64::from(local.minute() * 60 + local.second()));
                let piece_end = span.ended_at.min(next_hour);
                let seconds = (piece_end - cursor).num_seconds().max(0) as u32;
                let total = dwell_seconds.entry(bucket).or_default();
                *total = total.saturating_add(seconds);
                dwell_blocks
                    .entry(bucket)
                    .or_default()
                    .insert(span.block_id.clone());
                cursor = piece_end;
            }
        }
        let qualifies = |bucket: &(u32, u32)| {
            dwell_seconds.get(bucket).copied().unwrap_or(0) >= GOOD_HOURS_MIN_BUCKET_DWELL_SECONDS
                && dwell_blocks.get(bucket).map(HashSet::len).unwrap_or(0)
                    >= GOOD_HOURS_MIN_BUCKET_BLOCKS
        };
        let any_window_exists = dwell_seconds.keys().any(&qualifies);
        if !any_window_exists {
            return Ok(GoodHoursAssessment::InsufficientEvidence);
        }
        let local_now = to_local(now, offset_seconds);
        let current = (local_now.weekday().num_days_from_monday(), local_now.hour());
        if qualifies(&current) {
            Ok(GoodHoursAssessment::GoodHour)
        } else {
            Ok(GoodHoursAssessment::OutsideGoodHours)
        }
    }

    /// Records the one-tap dismissal. Only a live invitation transitions;
    /// a stale or repeated tap is a no-op.
    pub fn dismiss(&self, invitation_id: Uuid, now: DateTime<Utc>) -> Result<(), InitiationError> {
        self.repo.resolve_invitation(
            &invitation_id.to_string(),
            InitiationInvitationOutcome::Dismissed,
            now,
        )?;
        Ok(())
    }

    /// Whether `invitation_id` names the live invitation, so a start
    /// command carrying it may record the invitation origin. A stale or
    /// unknown id claims nothing and the start proceeds as manual.
    pub fn claimable(
        &self,
        invitation_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<bool, InitiationError> {
        let Some(open) = self.repo.open_invitation()? else {
            return Ok(false);
        };
        Ok(open.invitation_id == invitation_id.to_string()
            && open.policy_version == GOOD_HOURS_POLICY_VERSION
            && now < open.offered_at + Duration::seconds(INVITATION_RESPONSE_TTL_SECONDS))
    }

    /// Records that a block actually started. A validated claim becomes
    /// `accepted`; any other live invitation expires, because an active
    /// block suppresses invitations (invariant 1).
    pub fn record_block_started(
        &self,
        claimed: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<(), InitiationError> {
        match claimed {
            Some(invitation_id) => {
                self.repo.resolve_invitation(
                    &invitation_id.to_string(),
                    InitiationInvitationOutcome::Accepted,
                    now,
                )?;
            }
            None => self.expire_open(now)?,
        }
        Ok(())
    }

    /// Expires the live invitation, if any. Used on opt-out, logout and
    /// account switches, and any state that invalidates the offer.
    pub fn expire_open(&self, now: DateTime<Utc>) -> Result<(), InitiationError> {
        if let Some(open) = self.repo.open_invitation()? {
            self.repo.resolve_invitation(
                &open.invitation_id,
                InitiationInvitationOutcome::Expired,
                now,
            )?;
        }
        Ok(())
    }

    /// The single opt-out. Disabling silences invitations entirely (the
    /// live one expires); nothing else about the product changes.
    pub fn set_enabled(&self, enabled: bool, now: DateTime<Utc>) -> Result<bool, InitiationError> {
        self.repo.set_invitations_enabled(enabled, now)?;
        if !enabled {
            self.expire_open(now)?;
        }
        Ok(self.repo.invitations_enabled()?)
    }

    pub fn enabled(&self) -> Result<bool, InitiationError> {
        Ok(self.repo.invitations_enabled()?)
    }

    /// Clears the invitation record with the rest of the local behavioral
    /// data. The opt-out is an explicit user choice and survives.
    pub fn clear_data(&self) -> Result<(), InitiationError> {
        self.repo.clear_invitations()?;
        Ok(())
    }
}

fn offer_payload(record: &InitiationInvitationRecord) -> InitiationInvitation {
    InitiationInvitation {
        invitation_id: Uuid::parse_str(&record.invitation_id).unwrap_or_default(),
        action_id: record.action_id.clone(),
        body: invitation_body(),
        duration_seconds: INVITATION_DURATION_SECONDS,
        policy_version: record.policy_version,
    }
}

/// Registered analyst-voice invitation copy (D4; invariant 7). Fixed and
/// schedule-free: no window, hour, count, or history detail is rendered,
/// and it never describes the deterministic policy as learned.
fn invitation_body() -> String {
    "You usually focus well around now — want a 25-minute soft start?".to_owned()
}

/// Trailing dismissals, most recent first, stopping at the first
/// acceptance. Silence (`no_response`) and invalidation (`expired`) are not
/// "leave me alone" evidence: they neither count nor reset (D1).
fn trailing_dismissals(recent: &[InitiationInvitationRecord]) -> (u32, Option<DateTime<Utc>>) {
    let mut count = 0_u32;
    let mut last_dismissed_at = None;
    for invitation in recent {
        match invitation.outcome {
            InitiationInvitationOutcome::Dismissed => {
                count = count.saturating_add(1);
                if last_dismissed_at.is_none() {
                    last_dismissed_at =
                        Some(invitation.outcome_at.unwrap_or(invitation.offered_at));
                }
            }
            InitiationInvitationOutcome::Accepted => break,
            InitiationInvitationOutcome::NoResponse
            | InitiationInvitationOutcome::Expired
            | InitiationInvitationOutcome::Offered => {}
        }
    }
    (count, last_dismissed_at)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{
        SqlitePersistence, WorkBlockObservation, WorkBlockOrigin, WorkBlockRecord,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use velvt_shared_types::{
        ClassificationConfidence, ClassificationStatus, WorkBlockIntensity, WorkBlockPhase,
    };

    /// 2027-01-15T08:00:00Z — a Friday, 08:00 local at offset 0. The same
    /// deterministic anchor the focus tests use.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    fn hours(value: i64) -> i64 {
        value * 3_600
    }

    fn days(value: i64) -> i64 {
        value * 86_400
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

    struct Fixture {
        manager: Arc<InitiationManager>,
        repo: Arc<dyn InitiationRepo>,
        db: SqlitePersistence,
        gates: Arc<FakeGates>,
    }

    fn fixture() -> Fixture {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let repo = db.initiation_repo();
        let gates = Arc::new(FakeGates::default());
        let manager = InitiationManager::new(
            Arc::clone(&repo),
            Arc::clone(&gates) as Arc<dyn InvitationGates>,
        );
        Fixture {
            manager,
            repo,
            db,
            gates,
        }
    }

    /// Seeds one completed block whose confident dwell covers the whole
    /// block span. Synthetic fixture data only.
    fn completed_block(db: &SqlitePersistence, started_at: DateTime<Utc>, minutes: u32) {
        let repo = db.work_block_repo();
        let block_id = Uuid::new_v4().to_string();
        let ended_at = started_at + Duration::minutes(i64::from(minutes));
        repo.create(&WorkBlockRecord {
            block_id: block_id.clone(),
            phase: WorkBlockPhase::Completed,
            intention: None,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            planned_duration_seconds: (minutes * 60).max(300),
            started_at,
            paused_at: None,
            total_paused_seconds: 0,
            ended_at: Some(ended_at),
            recovered_after_restart: false,
            recovery_of: None,
            origin: WorkBlockOrigin::Manual,
            intention_expires_at: started_at,
            updated_at: started_at,
        })
        .unwrap();
        repo.append_observation(
            &block_id,
            &WorkBlockObservation {
                occurred_at: started_at,
                ended_at: Some(ended_at),
                category: "DEEP_WORK".into(),
                classification_status: ClassificationStatus::Classified,
                classification_confidence: ClassificationConfidence::High,
            },
        )
        .unwrap();
    }

    /// Dense synthetic history: three prior Fridays with 50 confident
    /// minutes starting 08:00 local, plus three prior Mondays at 14:00, all
    /// inside the lookback. Buckets (Fri, 08) and (Mon, 14) both qualify.
    fn dense_good_hours_history(db: &SqlitePersistence) {
        for back in [7, 14, 21] {
            completed_block(db, at(-days(back)), 50);
        }
        for back in [4, 11, 18] {
            completed_block(db, at(-days(back) + hours(6)), 50);
        }
    }

    /// Sparse cold-start history: two completed blocks, well under the
    /// total-evidence gate.
    fn sparse_cold_start_history(db: &SqlitePersistence) {
        completed_block(db, at(-days(7)), 50);
        completed_block(db, at(-days(14)), 50);
    }

    #[test]
    fn cold_start_returns_insufficient_evidence_and_invites_nobody() {
        let f = fixture();
        sparse_cold_start_history(&f.db);
        assert_eq!(
            f.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::InsufficientEvidence
        );
        assert!(f.manager.pending_invitation(at(0), 0).unwrap().is_none());
        assert!(
            f.repo.recent_invitations(8).unwrap().is_empty(),
            "cold start must record nothing"
        );
    }

    #[test]
    fn empty_history_returns_insufficient_evidence_not_a_default_window() {
        let f = fixture();
        for hour in 0..24 {
            assert_eq!(
                f.manager.assess_good_hours(at(hours(hour)), 0).unwrap(),
                GoodHoursAssessment::InsufficientEvidence,
                "no history must never yield a window at hour {hour}"
            );
        }
    }

    #[test]
    fn dense_history_makes_the_reliable_hour_a_good_hour() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        assert_eq!(
            f.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::GoodHour
        );
        // Same weekday, evidence-free hour: outside, not insufficient —
        // windows exist, this hour is just not in one.
        assert_eq!(
            f.manager.assess_good_hours(at(hours(12)), 0).unwrap(),
            GoodHoursAssessment::OutsideGoodHours
        );
        let invitation = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("good hour extends one invitation");
        assert_eq!(invitation.action_id, "soft_start_25");
        assert_eq!(invitation.duration_seconds, 25 * 60);
        assert_eq!(invitation.policy_version, GOOD_HOURS_POLICY_VERSION);
        assert!(f
            .manager
            .pending_invitation(at(hours(12)), 0)
            .unwrap()
            .is_none());
    }

    /// Gate boundaries, exactly: 6 total blocks with 3 in-bucket blocks and
    /// exactly the minimum dwell qualify; one block or one second less on
    /// any gate abstains.
    #[test]
    fn minimum_sample_gates_are_exact_boundaries() {
        // Exactly at every gate: 3 bucket blocks x 15 minutes = 2700s.
        let f = fixture();
        for back in [7, 14, 21] {
            completed_block(&f.db, at(-days(back)), 15);
        }
        for back in [4, 11, 18] {
            completed_block(&f.db, at(-days(back) + hours(6)), 5);
        }
        assert_eq!(
            f.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::GoodHour
        );

        // One second under the dwell gate: insufficient, never rounded up.
        let g = fixture();
        for back in [7, 14] {
            completed_block(&g.db, at(-days(back)), 15);
        }
        {
            // Third bucket block one second short of completing 2700s.
            let db = &g.db;
            let repo = db.work_block_repo();
            let started_at = at(-days(21));
            let block_id = Uuid::new_v4().to_string();
            let ended_at = started_at + Duration::seconds(15 * 60 - 1);
            repo.create(&WorkBlockRecord {
                block_id: block_id.clone(),
                phase: WorkBlockPhase::Completed,
                intention: None,
                purpose: None,
                intensity: WorkBlockIntensity::Medium,
                planned_duration_seconds: 900,
                started_at,
                paused_at: None,
                total_paused_seconds: 0,
                ended_at: Some(ended_at),
                recovered_after_restart: false,
                recovery_of: None,
                origin: WorkBlockOrigin::Manual,
                intention_expires_at: started_at,
                updated_at: started_at,
            })
            .unwrap();
            repo.append_observation(
                &block_id,
                &WorkBlockObservation {
                    occurred_at: started_at,
                    ended_at: Some(ended_at),
                    category: "DEEP_WORK".into(),
                    classification_status: ClassificationStatus::Classified,
                    classification_confidence: ClassificationConfidence::High,
                },
            )
            .unwrap();
        }
        for back in [4, 11, 18] {
            completed_block(&g.db, at(-days(back) + hours(6)), 5);
        }
        assert_eq!(
            g.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::InsufficientEvidence
        );

        // One block under the total gate: insufficient everywhere.
        let h = fixture();
        for back in [7, 14, 21] {
            completed_block(&h.db, at(-days(back)), 50);
        }
        for back in [4, 11] {
            completed_block(&h.db, at(-days(back) + hours(6)), 50);
        }
        assert_eq!(
            h.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::InsufficientEvidence
        );

        // One distinct block under the bucket gate: two strong Fridays are
        // not "reliably" — insufficient for that bucket.
        let i = fixture();
        for back in [7, 14] {
            completed_block(&i.db, at(-days(back)), 50);
        }
        for back in [4, 11, 18, 25] {
            completed_block(&i.db, at(-days(back) + hours(37)), 5);
        }
        assert_eq!(
            i.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::InsufficientEvidence
        );
    }

    /// Dwell spans split across local hour boundaries instead of crediting
    /// one bucket with the whole span.
    #[test]
    fn dwell_is_split_across_hour_boundaries() {
        let f = fixture();
        // 07:30-08:20 on three Fridays: 1800s into (Fri, 07), 1200s into
        // (Fri, 08) per block. Bucket (Fri, 07) qualifies (5400s), bucket
        // (Fri, 08) does not (3600s < 2700s? no — 3600 >= 2700, pick 08:35).
        // 07:30-08:14: 1800s into 07, 840s into 08 -> (Fri, 08) stays under.
        for back in [7, 14, 21] {
            completed_block(&f.db, at(-days(back)) - Duration::minutes(30), 44);
        }
        for back in [4, 11, 18] {
            completed_block(&f.db, at(-days(back) + hours(6)), 5);
        }
        assert_eq!(
            f.manager.assess_good_hours(at(-hours(1)), 0).unwrap(),
            GoodHoursAssessment::GoodHour,
            "the 07:00 bucket holds 3 x 30 confident minutes"
        );
        assert_eq!(
            f.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::OutsideGoodHours,
            "the 08:00 bucket holds only 3 x 14 minutes"
        );
    }

    #[test]
    fn evidence_outside_the_lookback_does_not_argue_for_a_window() {
        let f = fixture();
        for back in [29, 36, 43, 50, 57, 64] {
            completed_block(&f.db, at(-days(back)), 50);
        }
        assert_eq!(
            f.manager.assess_good_hours(at(0), 0).unwrap(),
            GoodHoursAssessment::InsufficientEvidence
        );
    }

    #[test]
    fn daily_cap_is_enforced_across_the_local_day_and_resets_on_rollover() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        // A prior invitation today (backdated past the cooldown, so the cap
        // is the binding gate) suppresses a second one.
        f.repo
            .record_invitation(&InitiationInvitationRecord {
                invitation_id: Uuid::new_v4().to_string(),
                offered_at: at(-hours(25)),
                local_date: "2027-01-15".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: GOOD_HOURS_POLICY_VERSION,
                backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
                outcome: InitiationInvitationOutcome::Accepted,
                outcome_at: Some(at(-hours(25))),
            })
            .unwrap();
        assert!(
            f.manager.pending_invitation(at(0), 0).unwrap().is_none(),
            "hard cap: at most one invitation per local day"
        );
        // The same record dated yesterday no longer binds.
        let g = fixture();
        dense_good_hours_history(&g.db);
        g.repo
            .record_invitation(&InitiationInvitationRecord {
                invitation_id: Uuid::new_v4().to_string(),
                offered_at: at(-hours(25)),
                local_date: "2027-01-14".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: GOOD_HOURS_POLICY_VERSION,
                backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
                outcome: InitiationInvitationOutcome::Accepted,
                outcome_at: Some(at(-hours(25))),
            })
            .unwrap();
        assert!(g.manager.pending_invitation(at(0), 0).unwrap().is_some());
    }

    #[test]
    fn live_block_quiet_hours_and_focus_each_suppress_and_expire() {
        for configure in [
            (|gates: &FakeGates| gates.live_block.store(true, Ordering::SeqCst)) as fn(&FakeGates),
            |gates| gates.quiet_hours.store(true, Ordering::SeqCst),
            |gates| gates.focus.store(true, Ordering::SeqCst),
        ] {
            let f = fixture();
            dense_good_hours_history(&f.db);
            // A live invitation exists, then the gate turns on: it expires.
            let invitation = f
                .manager
                .pending_invitation(at(0), 0)
                .unwrap()
                .expect("good hour extends the invitation");
            configure(&f.gates);
            assert!(f.manager.pending_invitation(at(60), 0).unwrap().is_none());
            let record = f
                .repo
                .invitation(&invitation.invitation_id.to_string())
                .unwrap()
                .unwrap();
            assert_eq!(record.outcome, InitiationInvitationOutcome::Expired);
            // And no new invitation is extended while the gate holds.
            assert!(f.manager.pending_invitation(at(120), 0).unwrap().is_none());
        }
    }

    #[test]
    fn opt_out_silences_invitations_entirely_and_survives_clear_data() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        assert!(f.manager.enabled().unwrap(), "invitations default on");
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("enabled good hour invites");
        assert!(!f.manager.set_enabled(false, at(30)).unwrap());
        let record = f
            .repo
            .invitation(&live.invitation_id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(
            record.outcome,
            InitiationInvitationOutcome::Expired,
            "disabling expires the live invitation"
        );
        assert!(f.manager.pending_invitation(at(60), 0).unwrap().is_none());
        // Clearing behavioral data keeps the explicit choice.
        f.manager.clear_data().unwrap();
        assert!(!f.manager.enabled().unwrap());
        assert!(f.repo.recent_invitations(8).unwrap().is_empty());
        // Re-enabling restores exactly the previous behavior.
        assert!(f.manager.set_enabled(true, at(90)).unwrap());
        assert!(f.manager.pending_invitation(at(120), 0).unwrap().is_some());
    }

    #[test]
    fn an_unanswered_invitation_lapses_to_no_response_after_the_ttl() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        // Inside the TTL the same invitation re-surfaces, never a second.
        let again = f
            .manager
            .pending_invitation(at(60), 0)
            .unwrap()
            .expect("repeat-safe re-surface");
        assert_eq!(again.invitation_id, live.invitation_id);
        assert_eq!(f.repo.recent_invitations(8).unwrap().len(), 1);
        // Past the TTL it lapses to no_response.
        assert!(f
            .manager
            .pending_invitation(at(INVITATION_RESPONSE_TTL_SECONDS + 1), 0)
            .unwrap()
            .is_none());
        let record = f
            .repo
            .invitation(&live.invitation_id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(record.outcome, InitiationInvitationOutcome::NoResponse);
    }

    #[test]
    fn a_dismissal_multiplies_the_next_invitations_cooldown() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        // One extra midweek block keeps the total gate satisfied even when
        // the oldest fixture blocks age out of the lookback later in the
        // scenario.
        completed_block(&f.db, at(-days(2)), 50);
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        f.manager.dismiss(live.invitation_id, at(60)).unwrap();
        // Monday 14:00 local (a qualifying bucket) 3 days later is well past
        // the base cooldown but inside the doubled one... base 24h x 2 = 48h;
        // 3 days = 72h > 48h, so pick the Saturday+Sunday gap instead: there
        // is no qualifying bucket before Monday, so assert directly at the
        // cooldown boundary using the Friday bucket one week on.
        // 47h59m after the dismissed invitation: suppressed by backoff even
        // at a good hour (next Friday 08:00 is 168h, so probe the doubled
        // cooldown with a synthetic Monday bucket at +72h).
        let monday_1400 = at(days(3) + hours(6));
        assert!(
            f.manager
                .pending_invitation(monday_1400, 0)
                .unwrap()
                .is_some(),
            "72h > doubled 48h cooldown: the next good hour may invite"
        );

        // A second consecutive dismissal quadruples the wait: 96h.
        let second = f
            .manager
            .pending_invitation(monday_1400, 0)
            .unwrap()
            .unwrap();
        f.manager
            .dismiss(second.invitation_id, monday_1400 + Duration::seconds(60))
            .unwrap();
        let friday_0800 = at(days(7));
        // Friday is 96h after Monday 08:00, exactly 90h after Monday 14:00:
        // inside the quadrupled cooldown, so it stays silent.
        assert!(
            f.manager
                .pending_invitation(friday_0800, 0)
                .unwrap()
                .is_none(),
            "two trailing dismissals: 90h < 96h cooldown"
        );
        let monday_1400_next = at(days(10) + hours(6));
        assert!(
            f.manager
                .pending_invitation(monday_1400_next, 0)
                .unwrap()
                .is_some(),
            "162h > 96h cooldown"
        );
    }

    #[test]
    fn a_dismissal_streak_silences_invitations_for_the_versioned_interval() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        // Three dismissed invitations, recorded directly with valid
        // spacing, the most recent one day ago.
        for (back, date) in [(15_i64, "2026-12-31"), (8, "2027-01-07"), (1, "2027-01-14")] {
            f.repo
                .record_invitation(&InitiationInvitationRecord {
                    invitation_id: Uuid::new_v4().to_string(),
                    offered_at: at(-days(back)),
                    local_date: date.into(),
                    action_id: INVITATION_ACTION_ID.to_owned(),
                    policy_version: GOOD_HOURS_POLICY_VERSION,
                    backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
                    outcome: InitiationInvitationOutcome::Dismissed,
                    outcome_at: Some(at(-days(back) + 60)),
                })
                .unwrap();
        }
        // Good hour, cooldown long past — still silent for the versioned
        // interval after the last dismissal.
        assert!(f.manager.pending_invitation(at(0), 0).unwrap().is_none());
        assert!(f
            .manager
            .pending_invitation(at(days(21)), 0)
            .unwrap()
            .is_none());
        // Past the interval, and with fresh in-lookback evidence, one
        // invitation may be extended once more.
        let g = fixture();
        // Weekday-aligned evidence relative to the probe at +31 days (a
        // Monday): three prior Mondays at 08:00 and three prior Fridays at
        // 14:00, all inside the lookback.
        for back in [7, 14, 21] {
            completed_block(&g.db, at(days(31) - days(back)), 50);
        }
        for back in [3, 10, 17] {
            completed_block(&g.db, at(days(31) - days(back) + hours(6)), 50);
        }
        for (back, date) in [
            (45_i64, "2026-12-01"),
            (38, "2026-12-08"),
            (31, "2026-12-15"),
        ] {
            g.repo
                .record_invitation(&InitiationInvitationRecord {
                    invitation_id: Uuid::new_v4().to_string(),
                    offered_at: at(days(31) - days(back)),
                    local_date: date.into(),
                    action_id: INVITATION_ACTION_ID.to_owned(),
                    policy_version: GOOD_HOURS_POLICY_VERSION,
                    backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
                    outcome: InitiationInvitationOutcome::Dismissed,
                    outcome_at: Some(at(days(31) - days(back) + 60)),
                })
                .unwrap();
        }
        assert!(
            g.manager
                .pending_invitation(at(days(31)), 0)
                .unwrap()
                .is_some(),
            "31 days after the last dismissal the silence interval has passed"
        );
    }

    #[test]
    fn acceptance_resets_the_dismissal_count_and_silence_never_shortens() {
        let recent = vec![
            InitiationInvitationRecord {
                invitation_id: "a".into(),
                offered_at: at(0),
                local_date: "2027-01-15".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: 1,
                backoff_policy_version: 1,
                outcome: InitiationInvitationOutcome::Dismissed,
                outcome_at: Some(at(60)),
            },
            InitiationInvitationRecord {
                invitation_id: "b".into(),
                offered_at: at(-days(1)),
                local_date: "2027-01-14".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: 1,
                backoff_policy_version: 1,
                outcome: InitiationInvitationOutcome::NoResponse,
                outcome_at: Some(at(-days(1) + 1_800)),
            },
            InitiationInvitationRecord {
                invitation_id: "c".into(),
                offered_at: at(-days(2)),
                local_date: "2027-01-13".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: 1,
                backoff_policy_version: 1,
                outcome: InitiationInvitationOutcome::Dismissed,
                outcome_at: Some(at(-days(2) + 60)),
            },
            InitiationInvitationRecord {
                invitation_id: "d".into(),
                offered_at: at(-days(3)),
                local_date: "2027-01-12".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: 1,
                backoff_policy_version: 1,
                outcome: InitiationInvitationOutcome::Accepted,
                outcome_at: Some(at(-days(3) + 60)),
            },
            InitiationInvitationRecord {
                invitation_id: "e".into(),
                offered_at: at(-days(4)),
                local_date: "2027-01-11".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: 1,
                backoff_policy_version: 1,
                outcome: InitiationInvitationOutcome::Dismissed,
                outcome_at: Some(at(-days(4) + 60)),
            },
        ];
        let (count, last) = trailing_dismissals(&recent);
        assert_eq!(
            count, 2,
            "silence skips no_response, acceptance stops the scan"
        );
        assert_eq!(last, Some(at(60)));
    }

    #[test]
    fn a_stale_policy_version_expires_the_stored_invitation() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        let stale_id = Uuid::new_v4().to_string();
        f.repo
            .record_invitation(&InitiationInvitationRecord {
                invitation_id: stale_id.clone(),
                offered_at: at(-60),
                local_date: "2027-01-15".into(),
                action_id: INVITATION_ACTION_ID.to_owned(),
                policy_version: GOOD_HOURS_POLICY_VERSION + 1,
                backoff_policy_version: INVITATION_BACKOFF_POLICY_VERSION,
                outcome: InitiationInvitationOutcome::Offered,
                outcome_at: None,
            })
            .unwrap();
        // The incompatible invitation expires; the daily cap then keeps the
        // day quiet rather than re-inviting.
        assert!(f.manager.pending_invitation(at(0), 0).unwrap().is_none());
        assert_eq!(
            f.repo.invitation(&stale_id).unwrap().unwrap().outcome,
            InitiationInvitationOutcome::Expired
        );
    }

    #[test]
    fn accepting_records_the_outcome_and_a_manual_start_expires_the_offer() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        assert!(f.manager.claimable(live.invitation_id, at(60)).unwrap());
        assert!(
            !f.manager.claimable(Uuid::new_v4(), at(60)).unwrap(),
            "an unknown id claims nothing"
        );
        f.manager
            .record_block_started(Some(live.invitation_id), at(60))
            .unwrap();
        assert_eq!(
            f.repo
                .invitation(&live.invitation_id.to_string())
                .unwrap()
                .unwrap()
                .outcome,
            InitiationInvitationOutcome::Accepted
        );

        // Manual-start path: a live offer expires when a block starts
        // without claiming it.
        let g = fixture();
        dense_good_hours_history(&g.db);
        let live = g
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        g.manager.record_block_started(None, at(60)).unwrap();
        assert_eq!(
            g.repo
                .invitation(&live.invitation_id.to_string())
                .unwrap()
                .unwrap()
                .outcome,
            InitiationInvitationOutcome::Expired
        );
    }

    #[test]
    fn a_claim_past_the_ttl_is_not_accepted_evidence() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        assert!(!f
            .manager
            .claimable(live.invitation_id, at(INVITATION_RESPONSE_TTL_SECONDS + 1))
            .unwrap());
    }

    /// Analyst voice (invariant 7; D4): the invitation copy is registered
    /// here, schedule-free, and carries none of the banned vocabulary.
    #[test]
    fn invitation_copy_is_analyst_voice_and_schedule_free() {
        let copy = invitation_body();
        let lowered = copy.to_ascii_lowercase();
        for forbidden in [
            "still",
            "dismiss",
            "failed",
            "failure",
            "ignored",
            "last time",
            "again",
            "learned",
            "adaptive",
        ] {
            assert!(
                !lowered.contains(forbidden),
                "{forbidden:?} in invitation copy {copy:?}"
            );
        }
        for digit_or_schedule in ["monday", "friday", "weekday", "o'clock", ":00"] {
            assert!(
                !lowered.contains(digit_or_schedule),
                "schedule detail {digit_or_schedule:?} in invitation copy {copy:?}"
            );
        }
    }

    /// The serialized invitation payload carries no window, weekday, hour
    /// bucket, or other schedule-reconstructing field.
    #[test]
    fn invitation_payload_is_schedule_free() {
        let f = fixture();
        dense_good_hours_history(&f.db);
        let live = f
            .manager
            .pending_invitation(at(0), 0)
            .unwrap()
            .expect("invitation");
        let encoded = serde_json::to_string(&live).unwrap();
        for forbidden in [
            "hour",
            "weekday",
            "bucket",
            "window",
            "local_",
            "offered_at",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "schedule-shaped field {forbidden:?} in {encoded}"
            );
        }
    }
}
