//! Synthetic end-to-end demonstration of 0.1.6 Scope 4: auto-demotion,
//! the weekly receipts digest, and the explain-this-nudge probe, through
//! the real router, work-block manager, receipts manager, and persistence.
//!
//! The digest parity tests are the load-bearing ones: every displayed count
//! is compared against the stored aggregate it must equal, and against the
//! independently seeded ground truth. This is the automated equivalent of
//! the packaged-app manual demonstration.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration as ChronoDuration, TimeZone, Utc};
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::initiation::{InitiationManager, InvitationGates, RuntimeInvitationGates};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{
    InitiationInvitationOutcome, InitiationInvitationRecord, PersistenceError, SqlitePersistence,
    WorkBlockCompletion, WorkBlockIntervention, WorkBlockInterventionOutcome, WorkBlockOrigin,
    WorkBlockRecord,
};
use velvt_service::receipts::ReceiptsManager;
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{FocusStateSource, WorkBlockManager};
use velvt_shared_types::{
    AcknowledgeWeeklyDigest, ClientMessage, ConfidenceLevel, DemotionStateKind,
    InterventionSalience, RequestInterventionExplanation, RequestWeeklyDigest, ServerMessage,
    WorkBlockCoverage, WorkBlockIntensity, WorkBlockNextAction, WorkBlockPhase, WorkBlockResult,
};

struct OfflineHttp;

impl HttpClient for OfflineHttp {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>>
    {
        Box::pin(async { Err(AuthError::Transport) })
    }
}

struct NullIngestor;

type IngestorFuture<'a, T> = Pin<
    Box<
        dyn std::future::Future<Output = Result<T, velvt_service::upload::CoordinatorError>>
            + Send
            + 'a,
    >,
>;

impl EventIngestor for NullIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a velvt_service::abstraction::AbstractedEvent,
        _duration_seconds: u64,
        _now: chrono::DateTime<Utc>,
    ) -> IngestorFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn flush_due<'a>(&'a self, _now: chrono::DateTime<Utc>) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_shutdown<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }

    fn flush_now<'a>(&'a self) -> IngestorFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
}

/// Test gates with independent switches, so each hold can be exercised.
#[derive(Default)]
struct SwitchableGates {
    live_block: AtomicBool,
    quiet_hours: AtomicBool,
    focus: AtomicBool,
}

impl InvitationGates for SwitchableGates {
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

struct Harness {
    router: R7Router,
    persistence: SqlitePersistence,
    work_blocks: Arc<WorkBlockManager>,
    receipts: Arc<ReceiptsManager>,
    gates: Arc<SwitchableGates>,
}

fn harness() -> Harness {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let queue = PushQueue::new(50);
    let push = PushAdapter::new(Arc::clone(&queue));
    let focus = FocusManager::new(persistence.focus_repo());
    let work_blocks = Arc::new(
        WorkBlockManager::new(persistence.work_block_repo())
            .with_focus_source(Arc::clone(&focus) as Arc<dyn FocusStateSource>),
    );
    let initiation = InitiationManager::new(
        persistence.initiation_repo(),
        RuntimeInvitationGates::new(Arc::clone(&focus), persistence.work_block_repo()),
    );
    let gates = Arc::new(SwitchableGates::default());
    let receipts = ReceiptsManager::new(
        persistence.receipts_repo(),
        Arc::clone(&gates) as Arc<dyn InvitationGates>,
    );
    let abstraction_engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let account = Arc::new(AccountAuthService::new(
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(OfflineHttp) as Arc<dyn HttpClient>,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    let router = R7Router::new(
        Arc::new(FakeCacheManager::new()),
        abstraction_engine,
        persistence.raw_event_repo(),
        Arc::new(NullIngestor) as Arc<dyn EventIngestor>,
        account,
    )
    .with_work_blocks(Arc::clone(&work_blocks), push)
    .with_focus(Arc::clone(&focus))
    .with_initiation(Arc::clone(&initiation))
    .with_receipts(Arc::clone(&receipts));
    Harness {
        router,
        persistence,
        work_blocks,
        receipts,
        gates,
    }
}

/// The Monday 00:00 UTC starting the most recent completed week (offset 0).
fn previous_week_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let today = now.date_naive();
    let this_monday = today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()));
    let prev_monday = this_monday - chrono::Days::new(7);
    Utc.from_utc_datetime(&prev_monday.and_hms_opt(0, 0, 0).unwrap())
}

fn synthetic_result(recovery_count: u32) -> WorkBlockResult {
    WorkBlockResult {
        planned_duration_seconds: 1_500,
        elapsed_duration_seconds: 1_500,
        longest_uninterrupted_seconds: 900,
        switch_away_count: recovery_count,
        recovery_count,
        confidence: ConfidenceLevel::High,
        coverage: WorkBlockCoverage::Good,
        coverage_ratio: 0.9,
        safe_evidence_category: Some("DEEP_WORK".into()),
        observation: "Velvt observed one sustained category pattern.".into(),
        next_action: WorkBlockNextAction {
            action_id: "protect_next_10".into(),
            label: "Protect the next 10 minutes.".into(),
            duration_seconds: 600,
        },
        dnd_outcomes: Vec::new(),
        reconciliation: None,
    }
}

/// Seeds one terminal block inside the digest week, with a stored session
/// result carrying the given recovery count.
fn seeded_block(
    persistence: &SqlitePersistence,
    started_at: DateTime<Utc>,
    phase: WorkBlockPhase,
    recovery_count: u32,
) -> String {
    let repo = persistence.work_block_repo();
    let block_id = Uuid::new_v4().to_string();
    repo.create(&WorkBlockRecord {
        block_id: block_id.clone(),
        phase: WorkBlockPhase::Active,
        intention: None,
        purpose: None,
        intensity: WorkBlockIntensity::Medium,
        planned_duration_seconds: 1_500,
        started_at,
        paused_at: None,
        total_paused_seconds: 0,
        ended_at: None,
        recovered_after_restart: false,
        recovery_of: None,
        origin: WorkBlockOrigin::Manual,
        intention_expires_at: started_at,
        updated_at: started_at,
    })
    .unwrap();
    repo.finalize(
        &block_id,
        &WorkBlockCompletion {
            phase,
            ended_at: started_at + ChronoDuration::minutes(25),
            result: synthetic_result(recovery_count),
        },
    )
    .unwrap();
    block_id
}

fn seed_intervention(
    persistence: &SqlitePersistence,
    block_id: &str,
    outcome: WorkBlockInterventionOutcome,
    offered_at: DateTime<Utc>,
) {
    persistence
        .work_block_repo()
        .record_intervention(
            block_id,
            &WorkBlockIntervention {
                offered_at,
                action_id: "protect_next_10".into(),
                anchor_category: "DEEP_WORK".into(),
                switch_count: 4,
                window_seconds: 600,
                outcome,
                outcome_at: (outcome != WorkBlockInterventionOutcome::Offered)
                    .then(|| offered_at + ChronoDuration::seconds(10)),
                salience: InterventionSalience::Normal,
            },
        )
        .unwrap();
}

fn seed_accepted_invitation(persistence: &SqlitePersistence, at: DateTime<Utc>) {
    let repo = persistence.initiation_repo();
    let invitation_id = Uuid::new_v4().to_string();
    repo.record_invitation(&InitiationInvitationRecord {
        invitation_id: invitation_id.clone(),
        offered_at: at,
        local_date: at.format("%Y-%m-%d").to_string(),
        action_id: "soft_start_25".into(),
        policy_version: 1,
        outcome: InitiationInvitationOutcome::Offered,
        outcome_at: None,
        backoff_policy_version: 1,
    })
    .unwrap();
    repo.resolve_invitation(
        &invitation_id,
        InitiationInvitationOutcome::Accepted,
        at + ChronoDuration::minutes(1),
    )
    .unwrap();
}

/// Seeds a known ground truth inside the most recent completed week:
/// 5 blocks declared, 3 completed, 4 recoveries, 2 wrong interventions,
/// 2 invitations accepted, 2 withheld/suppressed decisions.
fn seed_digest_week(persistence: &SqlitePersistence, week_start: DateTime<Utc>) -> Vec<String> {
    let day = |days: i64, hour: i64| week_start + ChronoDuration::hours(days * 24 + hour);
    let mut blocks = Vec::new();
    for (offset, phase, recoveries) in [
        (0_i64, WorkBlockPhase::Completed, 2_u32),
        (1, WorkBlockPhase::Completed, 1),
        (2, WorkBlockPhase::Completed, 1),
        (3, WorkBlockPhase::Abandoned, 0),
        (4, WorkBlockPhase::Abandoned, 0),
    ] {
        blocks.push(seeded_block(
            persistence,
            day(offset, 10),
            phase,
            recoveries,
        ));
    }
    // Delivered interventions: two wrong, three answered otherwise.
    seed_intervention(
        persistence,
        &blocks[0],
        WorkBlockInterventionOutcome::WasFocused,
        day(0, 11),
    );
    seed_intervention(
        persistence,
        &blocks[1],
        WorkBlockInterventionOutcome::WasFocused,
        day(1, 11),
    );
    seed_intervention(
        persistence,
        &blocks[2],
        WorkBlockInterventionOutcome::Returned,
        day(2, 11),
    );
    seed_intervention(
        persistence,
        &blocks[3],
        WorkBlockInterventionOutcome::Dismissed,
        day(3, 11),
    );
    seed_intervention(
        persistence,
        &blocks[4],
        WorkBlockInterventionOutcome::NoResponse,
        day(4, 11),
    );
    // What Velvt chose not to send: one DND hold, one demotion withhold.
    //
    // Their own blocks, because one offer per block means a held decision
    // cannot share a block with a delivered one — the primary key would keep
    // only the first, and "what Velvt chose not to send" would read as zero.
    let held_dnd_block = seeded_block(persistence, day(5, 9), WorkBlockPhase::Completed, 0);
    let withheld_block = seeded_block(persistence, day(6, 9), WorkBlockPhase::Completed, 0);
    seed_intervention(
        persistence,
        &held_dnd_block,
        WorkBlockInterventionOutcome::DeliverySuppressedDnd,
        day(3, 12),
    );
    seed_intervention(
        persistence,
        &withheld_block,
        WorkBlockInterventionOutcome::WithheldDemotion,
        day(4, 12),
    );
    seed_accepted_invitation(persistence, day(0, 9));
    seed_accepted_invitation(persistence, day(2, 9));
    blocks
}

/// D6: every digest count equals both the seeded ground truth and the
/// stored aggregate it must read from — no parallel counting anywhere.
#[tokio::test]
async fn digest_counts_match_stored_aggregates_exactly() {
    let h = harness();
    let now = Utc::now();
    let week_start = previous_week_start(now);
    let week_end = week_start + ChronoDuration::days(7);
    seed_digest_week(&h.persistence, week_start);

    let response = h
        .router
        .route(ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WeeklyDigest(digest)) = response else {
        panic!("expected a weekly digest, got {response:?}");
    };

    // Ground truth.
    // Seven declared: five carrying delivered offers, plus the two that carry
    // the held decisions. Both of those completed, so five completed in total.
    assert_eq!(digest.blocks_declared, 7);
    assert_eq!(digest.blocks_completed, 5);
    assert_eq!(digest.recoveries, 4);
    assert_eq!(digest.wrong_interventions, 2);
    assert_eq!(digest.invitations_accepted, 2);
    assert_eq!(digest.withheld, 2);
    assert_eq!(digest.digest_version, 1);

    // Stored-aggregate parity: the exact same repo aggregates the metrics
    // read must reproduce every displayed count.
    let repo = h.persistence.receipts_repo();
    assert_eq!(
        u64::from(digest.blocks_declared),
        repo.declared_block_count_between(week_start, week_end)
            .unwrap()
    );
    assert_eq!(
        u64::from(digest.blocks_completed),
        repo.completed_block_count_between(week_start, week_end)
            .unwrap()
    );
    assert_eq!(
        u64::from(digest.wrong_interventions),
        u64::from(
            repo.wrong_intervention_counts_between(week_start, week_end)
                .unwrap()
                .was_focused
        )
    );
    assert_eq!(
        u64::from(digest.invitations_accepted),
        repo.accepted_invitation_count_between(week_start, week_end)
            .unwrap()
    );
    assert_eq!(
        u64::from(digest.withheld),
        repo.withheld_count_between(week_start, week_end).unwrap()
    );
    let summed: u32 = repo
        .result_payloads_between(week_start, week_end)
        .unwrap()
        .iter()
        .filter_map(|payload| serde_json::from_str::<WorkBlockResult>(payload).ok())
        .map(|result| result.recovery_count)
        .sum();
    assert_eq!(digest.recoveries, summed);

    // Recoveries and completions lead; the headline mentions no misses.
    assert_eq!(
        digest.headline,
        "You returned 4 times and completed 5 of 7 blocks this week."
    );

    // Frozen: a second request re-serves the same stored row.
    let again = h
        .router
        .route(ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WeeklyDigest(second)) = again else {
        panic!("digest disappeared");
    };
    assert_eq!(second, digest);
}

/// The digest is held — not rerouted — while quiet hours, Focus/DND, or a
/// live block gate delivery, and the same digest is delivered after.
#[tokio::test]
async fn digest_is_held_by_quiet_hours_focus_and_live_block_then_delivered() {
    let h = harness();
    let now = Utc::now();
    seed_digest_week(&h.persistence, previous_week_start(now));

    for gate in [&h.gates.quiet_hours, &h.gates.focus, &h.gates.live_block] {
        gate.store(true, Ordering::SeqCst);
        assert!(
            h.receipts.pending_digest(now, 0).unwrap().is_none(),
            "digest delivered through a closed gate"
        );
        gate.store(false, Ordering::SeqCst);
    }

    let delivered = h.receipts.pending_digest(now, 0).unwrap();
    assert!(delivered.is_some(), "digest not delivered after the hold");
}

/// Acknowledging closes the digest; there is no reopen, reply, or thread.
#[tokio::test]
async fn acknowledged_digest_stays_closed() {
    let h = harness();
    let now = Utc::now();
    seed_digest_week(&h.persistence, previous_week_start(now));

    let Some(ServerMessage::WeeklyDigest(digest)) = h
        .router
        .route(ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap()
    else {
        panic!("digest expected");
    };
    let response = h
        .router
        .route(ClientMessage::AcknowledgeWeeklyDigest(
            AcknowledgeWeeklyDigest {
                week_start_local_date: digest.week_start_local_date.clone(),
            },
        ))
        .await
        .unwrap();
    assert!(response.is_none(), "acknowledgment has no reply surface");
    assert!(h.receipts.pending_digest(now, 0).unwrap().is_none());
}

/// An empty week produces no digest: silence, not an empty report.
#[tokio::test]
async fn a_week_with_nothing_to_report_produces_no_digest() {
    let h = harness();
    assert!(h
        .router
        .route(ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap()
        .is_none());
    assert!(h
        .persistence
        .receipts_repo()
        .weekly_digest(
            &previous_week_start(Utc::now())
                .format("%Y-%m-%d")
                .to_string()
        )
        .unwrap()
        .is_none());
}

/// The explain tap through the router: one grounded sentence, one coarse
/// weekly bucket increment, and a delivered denominator derived from the
/// same stored predicate the counter uses. No user text is representable
/// on the request (enforced by the DTO contract tests).
#[tokio::test]
async fn explain_tap_returns_one_sentence_and_counts_one_coarse_bucket() {
    let h = harness();
    let now = Utc::now();
    // One delivered intervention on a block, offered exactly now so the
    // weekly bucket is unambiguous whatever the wall clock says.
    let block_id = seeded_block(
        &h.persistence,
        now - ChronoDuration::minutes(90),
        WorkBlockPhase::Completed,
        1,
    );
    seed_intervention(
        &h.persistence,
        &block_id,
        WorkBlockInterventionOutcome::Returned,
        now,
    );

    let request = ClientMessage::RequestInterventionExplanation(RequestInterventionExplanation {
        block_id: Uuid::parse_str(&block_id).unwrap(),
        utc_offset_seconds: 0,
    });
    let Some(ServerMessage::InterventionExplanation(explanation)) =
        h.router.route(request.clone()).await.unwrap()
    else {
        panic!("explanation expected");
    };
    assert_eq!(
        explanation.sentence,
        "Velvt offered this nudge because it observed 4 switches away from deep work in the \
         10 minutes before the offer."
    );
    assert_eq!(explanation.sentence.matches('.').count(), 1);

    assert_eq!(h.receipts.explain_taps_this_week(now, 0).unwrap(), 1);
    let _ = h.router.route(request).await.unwrap();
    assert_eq!(h.receipts.explain_taps_this_week(now, 0).unwrap(), 2);
    assert_eq!(
        h.receipts
            .interventions_delivered_this_week(now, 0)
            .unwrap(),
        1,
        "denominator reads the stored delivered predicate, not a parallel counter"
    );

    // A block whose only decisions were never shown explains nothing and
    // counts nothing.
    let held = seeded_block(
        &h.persistence,
        now - ChronoDuration::minutes(50),
        WorkBlockPhase::Completed,
        0,
    );
    seed_intervention(
        &h.persistence,
        &held,
        WorkBlockInterventionOutcome::DeliverySuppressedDnd,
        now - ChronoDuration::minutes(40),
    );
    assert!(h
        .router
        .route(ClientMessage::RequestInterventionExplanation(
            RequestInterventionExplanation {
                block_id: Uuid::parse_str(&held).unwrap(),
                utc_offset_seconds: 0,
            }
        ))
        .await
        .unwrap()
        .is_none());
    assert_eq!(h.receipts.explain_taps_this_week(now, 0).unwrap(), 2);
}

/// Demotion state flows through the router: inspectable while demoted, no
/// intervention path fires, invitations continue (the recorded Scope 4
/// decision: invitations are initiation help, not interventions), and the
/// manual reset resumes.
#[tokio::test]
async fn demotion_is_inspectable_resettable_and_leaves_invitations_governed_separately() {
    let h = harness();
    let now = Utc::now();
    // A demoting stream: 4 wrong of 16 delivered inside the window.
    //
    // One block per offer, because this build makes one offer per block —
    // seeding sixteen into a single block would collapse to one row on the
    // intervention table's primary key and the rate would never trigger.
    // Sixteen delivered offers is sixteen blocks, which is also what the
    // rolling window actually measures.
    for index in 0..16 {
        let block_id = seeded_block(
            &h.persistence,
            now - ChronoDuration::hours(30) + ChronoDuration::minutes(index),
            WorkBlockPhase::Completed,
            0,
        );
        seed_intervention(
            &h.persistence,
            &block_id,
            if index < 4 {
                WorkBlockInterventionOutcome::WasFocused
            } else {
                WorkBlockInterventionOutcome::Returned
            },
            now - ChronoDuration::hours(20) + ChronoDuration::minutes(index),
        );
    }

    let Some(ServerMessage::DemotionState(state)) = h
        .router
        .route(ClientMessage::RequestDemotionState(
            velvt_shared_types::RequestDemotionState {},
        ))
        .await
        .unwrap()
    else {
        panic!("demotion state expected");
    };
    assert_eq!(state.state, DemotionStateKind::Demoted);
    assert_eq!(state.wrong_count, 4);
    assert_eq!(state.delivered_count, 16);
    assert!(state.disclosure.is_some());

    // The recorded decision for item 5: invitations stay governed by their
    // own policy while demoted. Nothing in the demotion path touches the
    // invitation gates, so an invitation request routes exactly as before
    // (here: silence for cold start, not silence because of demotion).
    let invitation_response = h
        .router
        .route(ClientMessage::RequestInitiationInvitation(
            velvt_shared_types::RequestInitiationInvitation {
                utc_offset_seconds: 0,
            },
        ))
        .await
        .unwrap();
    assert!(invitation_response.is_none(), "cold start abstains");

    let Some(ServerMessage::DemotionState(reset)) = h
        .router
        .route(ClientMessage::ResetInterventionDemotion(
            velvt_shared_types::ResetInterventionDemotion {},
        ))
        .await
        .unwrap()
    else {
        panic!("reset returns the new state");
    };
    assert_eq!(reset.state, DemotionStateKind::Active);
    assert!(reset.disclosure.is_none());
}

/// Clear-all-data removes digests, probe buckets, and the demotion
/// singleton along with the record they were derived from.
#[tokio::test]
async fn clear_all_data_removes_receipts_probe_and_demotion_state() {
    let h = harness();
    let now = Utc::now();
    seed_digest_week(&h.persistence, previous_week_start(now));
    let Some(ServerMessage::WeeklyDigest(digest)) = h
        .router
        .route(ClientMessage::RequestWeeklyDigest(RequestWeeklyDigest {
            utc_offset_seconds: 0,
        }))
        .await
        .unwrap()
    else {
        panic!("digest expected");
    };
    h.receipts.record_explain_tap(now, 0).unwrap();
    assert_eq!(h.receipts.explain_taps_this_week(now, 0).unwrap(), 1);

    let response = h
        .router
        .route(ClientMessage::ClearWorkBlockData(
            velvt_shared_types::ClearWorkBlockData {},
        ))
        .await
        .unwrap();
    assert!(matches!(response, Some(ServerMessage::WorkBlockState(_))));

    assert!(h
        .persistence
        .receipts_repo()
        .weekly_digest(&digest.week_start_local_date)
        .unwrap()
        .is_none());
    assert_eq!(h.receipts.explain_taps_this_week(now, 0).unwrap(), 0);
    assert_eq!(
        h.work_blocks
            .demotion_state_payload(now)
            .unwrap()
            .delivered_count,
        0
    );
    assert_eq!(
        h.work_blocks.demotion_state_payload(now).unwrap().state,
        DemotionStateKind::Active
    );
}
