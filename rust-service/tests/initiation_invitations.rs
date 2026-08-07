//! Synthetic end-to-end demonstration of initiation invitations (0.1.6
//! Scope 3) through the real router, initiation manager, work-block
//! manager, focus manager, and persistence.
//!
//! The scenario: dense synthetic good-hours history makes the current hour
//! reliable, one invitation is extended, one tap starts a declared block
//! through the existing start command, the block completes, and the local
//! records distinguish the invited block from a manual one only by the
//! content-free origin marker. This is the automated equivalent of the
//! packaged-app manual demonstration.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::focus::FocusManager;
use velvt_service::initiation::{InitiationManager, RuntimeInvitationGates};
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{
    InitiationInvitationOutcome, SqlitePersistence, WorkBlockObservation, WorkBlockOrigin,
    WorkBlockRecord,
};
use velvt_service::upload::EventIngestor;
use velvt_service::work_block::{FocusStateSource, WorkBlockManager};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, ClientMessage, DismissInitiationInvitation,
    EndWorkBlock, LogOut, RequestInitiationInvitation, RequestInitiationSettings, ServerMessage,
    SetInitiationSettings, StartWorkBlock, WorkBlockIntensity, WorkBlockPhase,
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

struct Harness {
    router: R7Router,
    persistence: SqlitePersistence,
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
    .with_initiation(Arc::clone(&initiation));
    Harness {
        router,
        persistence,
    }
}

/// Seeds one completed synthetic block whose confident dwell fully covers
/// the local hour bucket that `now` falls in.
fn completed_block_covering_this_hour(
    persistence: &SqlitePersistence,
    started_at: DateTime<Utc>,
    minutes: u32,
) {
    let repo = persistence.work_block_repo();
    let block_id = Uuid::new_v4().to_string();
    let ended_at = started_at + ChronoDuration::minutes(i64::from(minutes));
    repo.create(&WorkBlockRecord {
        block_id: block_id.clone(),
        phase: WorkBlockPhase::Completed,
        intention: None,
        purpose: None,
        intensity: WorkBlockIntensity::Medium,
        planned_duration_seconds: (minutes * 60).clamp(300, 10_800),
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

/// Dense good-hours history relative to the real clock: three same-weekday
/// blocks whose dwell blankets the current hour, plus three more completed
/// blocks to satisfy the total-evidence gate.
fn seed_dense_history(persistence: &SqlitePersistence, now: DateTime<Utc>) {
    for back in [7, 14, 21] {
        completed_block_covering_this_hour(
            persistence,
            now - ChronoDuration::days(back) - ChronoDuration::minutes(60),
            150,
        );
    }
    for back in [6, 13, 20] {
        completed_block_covering_this_hour(persistence, now - ChronoDuration::days(back), 50);
    }
}

fn request_invitation() -> ClientMessage {
    ClientMessage::RequestInitiationInvitation(RequestInitiationInvitation {
        utc_offset_seconds: 0,
    })
}

/// The full accepted path: dense history, one invitation, one tap starting
/// a declared block through the existing start command, completion, and an
/// origin marker that exists only in local records.
#[tokio::test]
async fn dense_history_invitation_accept_completes_with_the_origin_marker() {
    let h = harness();
    let now = Utc::now();
    seed_dense_history(&h.persistence, now);

    let response = h.router.route(request_invitation()).await.unwrap();
    let Some(ServerMessage::InitiationInvitation(invitation)) = response else {
        panic!("dense good-hours history extends one invitation, got {response:?}");
    };
    assert_eq!(invitation.action_id, "soft_start_25");
    assert_eq!(invitation.duration_seconds, 1_500);
    assert_eq!(
        invitation.body,
        "You usually focus well around now — want a 25-minute soft start?"
    );

    // No schedule detail crosses IPC.
    let encoded =
        serde_json::to_string(&ServerMessage::InitiationInvitation(invitation.clone())).unwrap();
    for forbidden in ["hour", "weekday", "bucket", "window", "local_"] {
        assert!(
            !encoded.contains(forbidden),
            "schedule-shaped field {forbidden:?} crossed IPC: {encoded}"
        );
    }

    // Asking twice is repeat-safe: the same invitation, never a second.
    let again = h.router.route(request_invitation()).await.unwrap();
    let Some(ServerMessage::InitiationInvitation(same)) = again else {
        panic!("a live invitation re-surfaces");
    };
    assert_eq!(same.invitation_id, invitation.invitation_id);

    // One tap starts the declared block through the existing start command.
    let started = h
        .router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: invitation.duration_seconds,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            invitation_id: Some(invitation.invitation_id),
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(snapshot)) = started else {
        panic!("accepting starts a declared block");
    };
    assert_eq!(snapshot.phase, WorkBlockPhase::Active);
    let block_id = snapshot.block_id.unwrap();
    let start_encoded = serde_json::to_string(&ServerMessage::WorkBlockState(snapshot)).unwrap();
    assert!(
        !start_encoded.contains("origin"),
        "the origin marker crossed IPC: {start_encoded}"
    );

    // The invitation outcome is `accepted`, content-free.
    let record = h
        .persistence
        .initiation_repo()
        .invitation(&invitation.invitation_id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(record.outcome, InitiationInvitationOutcome::Accepted);

    // While the block is live no further invitation exists (invariant 1).
    assert!(h
        .router
        .route(request_invitation())
        .await
        .unwrap()
        .is_none());

    // The block ends and the local record carries the origin marker; a
    // manual block records `manual`. The R2 comparison needs nothing else.
    h.router
        .route(ClientMessage::EndWorkBlock(EndWorkBlock { block_id }))
        .await
        .unwrap();
    let invited_record = h
        .persistence
        .work_block_repo()
        .get(&block_id.to_string())
        .unwrap();
    assert_eq!(invited_record.origin, WorkBlockOrigin::Invitation);

    let manual = h
        .router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: 1_500,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    let Some(ServerMessage::WorkBlockState(manual_snapshot)) = manual else {
        panic!("manual start works unchanged");
    };
    let manual_record = h
        .persistence
        .work_block_repo()
        .get(&manual_snapshot.block_id.unwrap().to_string())
        .unwrap();
    assert_eq!(manual_record.origin, WorkBlockOrigin::Manual);
}

/// Sparse history: below the minimum-sample gates the policy answers with
/// silence — no invitation, no invented default window.
#[tokio::test]
async fn sparse_history_extends_no_invitation() {
    let h = harness();
    let now = Utc::now();
    for back in [7, 14] {
        completed_block_covering_this_hour(
            &h.persistence,
            now - ChronoDuration::days(back) - ChronoDuration::minutes(60),
            150,
        );
    }
    assert!(h
        .router
        .route(request_invitation())
        .await
        .unwrap()
        .is_none());
    assert!(h
        .persistence
        .initiation_repo()
        .recent_invitations(4)
        .unwrap()
        .is_empty());
}

/// A dismissal is recorded once through the router and a stale repeat tap
/// changes nothing.
#[tokio::test]
async fn dismissal_is_recorded_once_and_is_content_free() {
    let h = harness();
    let now = Utc::now();
    seed_dense_history(&h.persistence, now);
    let Some(ServerMessage::InitiationInvitation(invitation)) =
        h.router.route(request_invitation()).await.unwrap()
    else {
        panic!("invitation expected");
    };
    let response = h
        .router
        .route(ClientMessage::DismissInitiationInvitation(
            DismissInitiationInvitation {
                invitation_id: invitation.invitation_id,
            },
        ))
        .await
        .unwrap();
    assert!(response.is_none(), "dismissal is acknowledged silently");
    let record = h
        .persistence
        .initiation_repo()
        .invitation(&invitation.invitation_id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(record.outcome, InitiationInvitationOutcome::Dismissed);
    // A repeat tap is a no-op, and the daily cap holds.
    h.router
        .route(ClientMessage::DismissInitiationInvitation(
            DismissInitiationInvitation {
                invitation_id: invitation.invitation_id,
            },
        ))
        .await
        .unwrap();
    assert!(h
        .router
        .route(request_invitation())
        .await
        .unwrap()
        .is_none());
}

/// The single opt-out: turning invitations off answers with the Rust-owned
/// state, silences invitations entirely, and changes nothing else.
#[tokio::test]
async fn opt_out_silences_invitations_and_everything_else_is_unchanged() {
    let h = harness();
    let now = Utc::now();
    seed_dense_history(&h.persistence, now);

    let state = h
        .router
        .route(ClientMessage::RequestInitiationSettings(
            RequestInitiationSettings {},
        ))
        .await
        .unwrap();
    let Some(ServerMessage::InitiationSettings(settings)) = state else {
        panic!("settings request answers with the Rust-owned state");
    };
    assert!(settings.invitations_enabled, "invitations default on");

    let disabled = h
        .router
        .route(ClientMessage::SetInitiationSettings(
            SetInitiationSettings {
                invitations_enabled: false,
            },
        ))
        .await
        .unwrap();
    let Some(ServerMessage::InitiationSettings(settings)) = disabled else {
        panic!("setting replies with the persisted state");
    };
    assert!(!settings.invitations_enabled);
    assert!(h
        .router
        .route(request_invitation())
        .await
        .unwrap()
        .is_none());

    // Everything else is unchanged: a manual block starts normally.
    let started = h
        .router
        .route(ClientMessage::StartWorkBlock(StartWorkBlock {
            intention: None,
            planned_duration_seconds: 1_500,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            invitation_id: None,
        }))
        .await
        .unwrap();
    assert!(matches!(started, Some(ServerMessage::WorkBlockState(_))));
}

/// Logout expires the live invitation so it cannot outlive the session.
#[tokio::test]
async fn logout_expires_the_live_invitation() {
    let h = harness();
    let now = Utc::now();
    seed_dense_history(&h.persistence, now);
    let Some(ServerMessage::InitiationInvitation(invitation)) =
        h.router.route(request_invitation()).await.unwrap()
    else {
        panic!("invitation expected");
    };
    h.router
        .route(ClientMessage::LogOut(LogOut {}))
        .await
        .unwrap();
    let record = h
        .persistence
        .initiation_repo()
        .invitation(&invitation.invitation_id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(record.outcome, InitiationInvitationOutcome::Expired);
}

/// Clear-all-data removes the invitation record; the opt-out choice
/// survives, exactly like the quiet-hours setting.
#[tokio::test]
async fn clear_all_data_removes_invitations_and_keeps_the_explicit_choice() {
    let h = harness();
    let now = Utc::now();
    seed_dense_history(&h.persistence, now);
    let Some(ServerMessage::InitiationInvitation(_)) =
        h.router.route(request_invitation()).await.unwrap()
    else {
        panic!("invitation expected");
    };
    h.router
        .route(ClientMessage::SetInitiationSettings(
            SetInitiationSettings {
                invitations_enabled: false,
            },
        ))
        .await
        .unwrap();
    h.router
        .route(ClientMessage::ClearWorkBlockData(
            velvt_shared_types::ClearWorkBlockData {},
        ))
        .await
        .unwrap();
    let repo = h.persistence.initiation_repo();
    assert!(repo.recent_invitations(4).unwrap().is_empty());
    assert!(!repo.invitations_enabled().unwrap());
}
