//! Finding E5, and cross-device-03: what deleting an account destroys on this
//! Mac, and whose token may send what was queued.
//!
//! Every queued batch carries the account it was recorded under
//! (`upload_batch.owner_user_id`, migration 0042). The service's
//! `SessionOwnership` policy sends a batch only under that account's session,
//! holds everything while no session is usable, and discards a batch once
//! another account is signed in. Before owners existed, the cloud attributed a
//! batch to whichever bearer token the retry loop held when it next ran: after
//! Log Out and a sign-in to another account, or after the account was deleted
//! on another Mac, the queue was stored in the next account. On the
//! development device on 2026-08-31 the queue held 181 failed and 1 pending
//! batch carrying 2,771 `batch_event` rows.
//!
//! The router also destroys the whole queue when the cloud accepts a deletion
//! from this Mac. `a_queue_survives_a_refused_deletion` is the other half: a
//! deletion the cloud turned down leaves the user signed in, and the queue is
//! theirs. `tests/account_switch.rs` drives the same rules end to end through a
//! sign-in to a second account.
//!
//! What this file does *not* claim: the rest of the database survives account
//! deletion. `raw_event_buffer` (its rows become local-only, not deleted),
//! `abstraction_map`, both override tables, the prototype and embedding
//! stores, and the terminal (`sent`, `rejected`, `abandoned`) batch rows are
//! all untouched by it. The deletion dialog says so in those words, and
//! `PRIVACY.md`'s removal procedure is the only complete path.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use rusqlite::Connection;
use uuid::Uuid;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse, TokenStore,
};
use velvt_service::delivery::FakeCacheManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{BatchEvent, NewUploadBatch, RawEventEntry, SqlitePersistence};
use velvt_service::upload::{
    BatchAssembler, BatchEventPayload, BatchPayload, EventIngestor, FakeBatchUploader,
    FakePrivacyAlertSink, SessionOwnership, SharedUploadBatcher, UploadBatcher, UploadCoordinator,
    UploadOutcome,
};
use velvt_shared_types::{ClientMessage, DeleteAccount, ServerMessage};

const SCHEMA_VERSION: &str = "1";
const CLIENT_VERSION: &str = "test";

/// The positive control, and the reason the assertions below are worth making.
///
/// Without it "nothing was uploaded" is satisfied by a resume path that uploads
/// nothing under any circumstances. Left alone, the coordinator sends the two
/// batches whose next attempt is due.
///
/// It also fixes the horizon the purge has to use. `resume_pending` asks for
/// `resumable_batches(now)`, so `batch-scheduled` is invisible to it for
/// another six hours and survives here — which is exactly why deletion reads
/// the queue through `pending_batches`, `resumable_batches` at an unbounded
/// horizon. A purge on the retry loop's horizon would leave that batch behind
/// and it would come due under the next account's token. On the development
/// device on 2026-08-31 that was not an edge case: all 182 queued batches
/// carried the same `next_attempt_at`, 2026-08-29 11:18 UTC, and every one of
/// them was scheduled past the moment the service last ran.
#[tokio::test]
async fn a_queue_left_alone_resumes_the_batches_that_are_due() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    seed_queue(&persistence);

    let uploader =
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted, UploadOutcome::Accepted]);
    let resumed = resume_queue(&persistence, uploader.clone()).await;

    assert_eq!(resumed, 2, "the two due batches must be resumable");
    assert_eq!(uploader.upload_count(), 2, "both must reach the uploader");
    let queued: Vec<String> = persistence
        .upload_batch_repo()
        .pending_batches()
        .unwrap()
        .into_iter()
        .map(|batch| batch.batch_id)
        .collect();
    assert_eq!(
        queued,
        vec!["batch-scheduled".to_owned()],
        "a batch scheduled for later is queued, not resumable, and not gone"
    );
}

/// The finding, end to end: sign-up as anyone else on this Mac must not carry
/// the previous account's queue with it.
#[tokio::test]
async fn account_deletion_destroys_the_queue_the_next_account_would_upload() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    seed_queue(&persistence);
    assert_eq!(
        persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .len(),
        3,
        "the queue must exist before it can be destroyed"
    );
    assert_eq!(
        batch_event_count(&scratch.path),
        6,
        "and so must the events inside it"
    );

    let router = router_answering_deletion_with(&persistence, 202);
    let reply = router
        .route(ClientMessage::DeleteAccount(DeleteAccount {}))
        .await
        .unwrap();
    assert!(
        matches!(reply, Some(ServerMessage::AccountDeletionAccepted(_))),
        "the cloud accepted, so the local half runs"
    );

    assert!(
        persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .is_empty(),
        "no batch may remain that an upload could reach, including one whose \
         next attempt has not come due"
    );
    assert_eq!(
        batch_event_count(&scratch.path),
        0,
        "the events go with the batch: batch_event is ON DELETE CASCADE"
    );

    // The attack itself. A new account has signed up on this Mac and the retry
    // loop runs under its token.
    let uploader =
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted, UploadOutcome::Accepted]);
    let resumed = resume_queue(&persistence, uploader.clone()).await;

    assert_eq!(resumed, 0, "there is nothing left to resume");
    assert_eq!(
        uploader.upload_count(),
        0,
        "the deleted account's activity must not be POSTed under the next account's token"
    );
}

/// The queue is destroyed because the account that owned it is gone. A cloud
/// that refuses the deletion leaves the user signed in, so nothing is gone and
/// nothing may be destroyed.
#[tokio::test]
async fn a_queue_survives_a_refused_deletion() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    seed_queue(&persistence);

    let router = router_answering_deletion_with(&persistence, 500);
    let reply = router
        .route(ClientMessage::DeleteAccount(DeleteAccount {}))
        .await
        .unwrap();

    match reply {
        Some(ServerMessage::ErrorResponse(error)) => {
            assert_eq!(error.code, "account_deletion_failed");
        }
        other => panic!("a refused deletion must report itself: {other:?}"),
    }
    assert_eq!(
        persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .len(),
        3,
        "activity waiting to upload is the user's until the account is gone"
    );
    assert_eq!(batch_event_count(&scratch.path), 6);
}

/// A destroyed queue must stay destroyed across a restart.
///
/// Deleting a batch left its events upload-eligible and in no batch, and
/// `recover_unbatched` re-queues exactly those at the next start, before any
/// session exists, to be sent under whoever signs in. The same went for an
/// event acked but not yet batched when the deletion arrived.
#[tokio::test]
async fn a_destroyed_queue_does_not_come_back_at_the_next_start() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let raw_events = persistence.raw_event_repo();
    for event_id in ["queued-1", "queued-2", "unbatched-1"] {
        raw_events.insert(&eligible_raw_event(event_id)).unwrap();
    }
    persist(
        &persistence,
        &mint(
            &[payload_event("queued-1"), payload_event("queued-2")],
            "device-e5",
        ),
    );
    assert_eq!(raw_events.unbatched_events(10).unwrap().len(), 1);

    let router = router_answering_deletion_with(&persistence, 202);
    router
        .route(ClientMessage::DeleteAccount(DeleteAccount {}))
        .await
        .unwrap();

    assert!(raw_events.unbatched_events(10).unwrap().is_empty());
    let mut restarted = UploadBatcher::new(
        BatchAssembler::new("device-next", 1, StdDuration::from_secs(0)),
        UploadCoordinator::new(
            persistence.upload_batch_repo(),
            FakeBatchUploader::default(),
            FakePrivacyAlertSink::default(),
        ),
    );
    let recovered = restarted
        .recover_unbatched(raw_events.as_ref(), 100, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        recovered, 0,
        "the deleted account's events must not be re-queued for the next account"
    );
    assert!(batch_ids(&scratch.path).is_empty());
}

/// Batch ids are derived from the device id and the event ids, so the same
/// device and the same events reproduce one: a retry after a crash is the
/// same batch, which the cloud answers as a duplicate.
#[test]
fn a_batch_id_is_stable_for_the_device_and_events_that_minted_it() {
    let events = [payload_event("event-1"), payload_event("event-2")];

    let mine = mint(&events, "device-a").batch_id;
    let mine_again = mint(&events, "device-a").batch_id;
    let theirs = mint(&events, "device-b").batch_id;

    assert_eq!(mine, mine_again);
    assert_ne!(mine, theirs);
}

/// The ownership rule on the retry loop: signed in as B, B's batch and a batch
/// from before owners existed are sent, and A's is deleted, events and all,
/// rather than sent under B's token.
#[tokio::test]
async fn the_retry_loop_sends_only_the_signed_in_accounts_batches() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let raw_events = persistence.raw_event_repo();
    for event_id in ["their-1", "their-2"] {
        raw_events.insert(&eligible_raw_event(event_id)).unwrap();
    }
    let theirs = owned(&["their-1", "their-2"], "account-a");
    let mine = owned(&["my-1", "my-2"], "account-b");
    let legacy = mint(&[payload_event("old-1")], "device-old");
    persist(&persistence, &theirs);
    persist(&persistence, &mine);
    persist(&persistence, &legacy);

    let uploader =
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted, UploadOutcome::Accepted]);
    UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_ownership_policy(Arc::new(signed_in_as("account-b")))
    .resume_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap();

    assert_eq!(
        uploader.upload_count(),
        2,
        "B's batch and the ownerless one may be POSTed under B's token; A's may not"
    );
    assert!(
        !batch_ids(&scratch.path).contains(&theirs.batch_id),
        "A's batch is deleted, not skipped and left to retry"
    );
    assert!(
        raw_events.unbatched_events(10).unwrap().is_empty(),
        "and its events are local-only, so no restart re-queues them"
    );
}

/// The same rule on the other send path.
///
/// `resume_pending` is the retry loop. `flush_all_pending` is what the menu
/// bar's "Send all now" reaches through `FlushUploadQueue`, and it additionally
/// ignores `next_attempt_at` and the host backoff — so it is the fastest way to
/// push a queue out. A rule enforced on the loop but not the button is not a
/// rule.
#[tokio::test]
async fn the_user_facing_flush_obeys_the_same_ownership_rule() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let theirs = owned(&["their-1", "their-2"], "account-a");
    let mine = owned(&["my-1", "my-2"], "account-b");
    persist(&persistence, &theirs);
    persist(&persistence, &mine);

    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_ownership_policy(Arc::new(signed_in_as("account-b")))
    .flush_all_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap();

    assert_eq!(
        uploader.upload_count(),
        1,
        "\"Send all now\" must not send another account's queue"
    );
    assert_eq!(batch_ids(&scratch.path), vec![mine.batch_id.clone()]);
}

/// Signed out, nothing is sent and nothing is spent: every batch keeps its
/// owner, its attempts and its schedule for when its account signs in again.
#[tokio::test]
async fn nothing_is_sent_or_put_off_while_signed_out() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let mine = owned(&["my-1"], "account-a");
    let legacy = mint(&[payload_event("old-1")], "device-old");
    persist(&persistence, &mine);
    persist(&persistence, &legacy);

    let tokens = Arc::new(FakeTokenStore::default());
    tokens.store_user_id("account-a").unwrap();
    let (_auth, auth_state) = tokio::sync::watch::channel(AuthState::Unauthenticated);
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_ownership_policy(Arc::new(SessionOwnership::new(tokens, auth_state)));
    coordinator
        .resume_pending(SCHEMA_VERSION, CLIENT_VERSION)
        .await
        .unwrap();
    coordinator
        .flush_all_pending(SCHEMA_VERSION, CLIENT_VERSION)
        .await
        .unwrap();

    assert_eq!(uploader.upload_count(), 0);
    let queued = persistence.upload_batch_repo().pending_batches().unwrap();
    assert_eq!(queued.len(), 2, "held, not discarded");
    assert!(
        queued.iter().all(|batch| batch.attempt_count == 0),
        "a batch held for want of a session spends no attempt"
    );
    assert_eq!(
        persistence
            .upload_batch_repo()
            .queue_diagnostics()
            .unwrap()
            .last_error_code,
        None,
        "and is not marked authentication_required"
    );
}

/// Signed in under an account the service was not told, whose a batch is
/// cannot be decided: an owned batch is held, not sent and not deleted.
#[tokio::test]
async fn an_owned_batch_is_held_while_the_signed_in_account_is_unknown() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    persist(&persistence, &owned(&["my-1"], "account-a"));

    let (_auth, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: "device-1".into(),
    });
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_ownership_policy(Arc::new(SessionOwnership::new(
        Arc::new(FakeTokenStore::default()),
        auth_state,
    )))
    .resume_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap();

    assert_eq!(uploader.upload_count(), 0);
    assert_eq!(batch_ids(&scratch.path).len(), 1);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The service's ownership policy, signed in as `account`.
fn signed_in_as(account: &str) -> SessionOwnership {
    let tokens = Arc::new(FakeTokenStore::default());
    tokens.store_user_id(account).unwrap();
    let (_auth, auth_state) = tokio::sync::watch::channel(AuthState::Authenticated {
        device_id: format!("device-of-{account}"),
    });
    SessionOwnership::new(tokens, auth_state)
}

/// A batch of `event_ids` recorded under `account`.
fn owned(event_ids: &[&str], account: &str) -> BatchPayload {
    let events = event_ids
        .iter()
        .map(|event_id| payload_event(event_id))
        .collect::<Vec<_>>();
    mint(&events, "device-shared").owned_by(Some(account.into()))
}

/// An upload-eligible row in `raw_event_buffer`, as the router writes one for
/// an event acked while signed in.
fn eligible_raw_event(event_id: &str) -> RawEventEntry {
    RawEventEntry {
        event_id: event_id.into(),
        stable_id: "abs_e5".into(),
        label: "document:edit".into(),
        local_display_label: None,
        local_name_suggestion: None,
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "fallback".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "fallback".into(),
        occurred_at: Utc::now() - Duration::hours(2),
        duration_seconds: 30,
        upload_eligible: true,
        app_stable_id: None,
        app_scope_eligible: false,
        site_stable_id: None,
    }
}

/// Runs the real assembler for one batch, which is how every batch id in `src/`
/// is produced.
fn mint(events: &[BatchEventPayload], device_id: &str) -> BatchPayload {
    let mut assembler =
        BatchAssembler::new(device_id, events.len().max(1), StdDuration::from_secs(0));
    let now = Utc::now();
    let mut minted = None;
    for event in events {
        minted = assembler.push(event.clone(), now).or(minted);
    }
    minted.expect("the assembler yields a batch once the count threshold is reached")
}

fn payload_event(event_id: &str) -> BatchEventPayload {
    BatchEventPayload {
        event_id: event_id.into(),
        stable_id: "abs_e5".into(),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        classification_tier: "fallback".into(),
        occurred_at: Utc::now() - Duration::hours(2),
        duration_seconds: 30,
    }
}

/// Copies the payload to disk the way `persist_batch` does: event ids
/// verbatim and in order, under the batch's owner.
fn persist(persistence: &SqlitePersistence, batch: &BatchPayload) {
    let events = batch
        .events
        .iter()
        .map(|event| BatchEvent {
            event_id: event.event_id.clone(),
            stable_id: event.stable_id.clone(),
            label: event.label.clone(),
            category: event.category.clone(),
            taxonomy_version: event.taxonomy_version.clone(),
            classification_tier: event.classification_tier.clone(),
            occurred_at: event.occurred_at,
            duration_seconds: event.duration_seconds,
        })
        .collect::<Vec<_>>();
    persistence
        .upload_batch_repo()
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: batch.batch_id.clone(),
                owner_user_id: batch.owner_user_id.clone(),
            },
            &events,
        )
        .unwrap();
}

/// Three batches of two events each: one pending, one failed and due, one
/// failed with its next attempt six hours out. The development device's queue
/// on 2026-08-31 was 181 failed and 1 pending, every one of them scheduled
/// forward — this is that shape, scaled down, with a due batch added so the
/// control has something to send.
fn seed_queue(persistence: &SqlitePersistence) {
    let batches = persistence.upload_batch_repo();
    let retries = [
        ("batch-pending", None),
        ("batch-due", Some(Utc::now() - Duration::minutes(30))),
        ("batch-scheduled", Some(Utc::now() + Duration::hours(6))),
    ];
    for (batch_id, next_attempt_at) in retries {
        let events = (0..2)
            .map(|slot| BatchEvent {
                event_id: format!("{batch_id}-event-{slot}"),
                stable_id: "abs_e5".into(),
                label: "document:edit".into(),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                classification_tier: "fallback".into(),
                occurred_at: Utc::now() - Duration::hours(2),
                duration_seconds: 30,
            })
            .collect::<Vec<_>>();
        batches
            .insert_batch_with_events(
                &NewUploadBatch {
                    batch_id: batch_id.into(),
                    owner_user_id: None,
                },
                &events,
            )
            .unwrap();
        if let Some(next_attempt_at) = next_attempt_at {
            batches
                .mark_failed(batch_id, next_attempt_at, "host_backoff")
                .unwrap();
        }
    }
}

/// Drives the real retry path: `resume_pending` is what `run_retry_loop` calls,
/// and what would carry a stranded batch to a new account's token.
async fn resume_queue(persistence: &SqlitePersistence, uploader: FakeBatchUploader) -> usize {
    UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader,
        FakePrivacyAlertSink::default(),
    )
    .resume_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap()
}

/// The router wired the way `main.rs` wires it for the queue: the upload batch
/// repository is attached, so `ClientMessage::DeleteAccount` can reach it.
fn router_answering_deletion_with(persistence: &SqlitePersistence, status: u16) -> R7Router {
    let engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let ingestor: Arc<dyn EventIngestor> = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-e5", 8, StdDuration::from_secs(3_600)),
        UploadCoordinator::new(
            persistence.upload_batch_repo(),
            FakeBatchUploader::default(),
            FakePrivacyAlertSink::default(),
        ),
    )));
    let account = Arc::new(AccountAuthService::new(
        Arc::new(StaticHttp(status)),
        Arc::new(StaticHttp(status)),
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Authenticated {
            device_id: "device-e5".into(),
        })),
    ));

    R7Router::new(
        Arc::new(FakeCacheManager::new()),
        engine,
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
    .with_classification_corrections(
        persistence.abstraction_map_repo(),
        persistence.upload_batch_repo(),
        Arc::new(StaticHttp(status)),
    )
}

/// Every batch id on disk, whatever its status. `pending_batches` cannot answer
/// "was this deleted or merely retired", because it filters on status.
fn batch_ids(path: &Path) -> Vec<String> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT batch_id FROM upload_batch ORDER BY id")
        .unwrap();
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    ids
}

/// A second connection to the same file. `batch_event` has no repository read
/// that survives its parent, and the row count is the number the finding is
/// about.
fn batch_event_count(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM batch_event", [], |row| row.get(0))
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
            std::env::temp_dir().join(format!("velvt-account-deletion-{}", Uuid::new_v4()));
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

/// Answers every account request with one status and no body.
struct StaticHttp(u16);

impl HttpClient for StaticHttp {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(HttpResponse {
                status: self.0,
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
