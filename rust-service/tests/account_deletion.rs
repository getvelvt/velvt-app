//! Finding E5: what deleting an account destroys on this Mac, and what it does
//! not.
//!
//! `upload_batch` records no owner. `resumable_batches` selects on status and
//! schedule and on nothing about who queued the row, and `BatchPayload` carries
//! no device or user identifier either, so the cloud
//! attributes a batch by whichever bearer token the retry loop holds when it
//! next runs. Deleting the account clears the stored device id, so the next
//! sign-up on the same Mac registers a *new* device — and the ingestion service
//! scopes duplicate detection by device id, so batches queued under the deleted
//! account arrive as new work and are stored against the account that replaces
//! it. On the development device on 2026-08-31 the queue held 181 failed and 1
//! pending batch carrying 2,771 `batch_event` rows, all of them from a single
//! outage between 2026-08-28 21:53 and 2026-08-29 06:32 UTC.
//!
//! The router now deletes that queue when the cloud accepts the deletion.
//! `a_queue_survives_a_refused_deletion` is the other half: a deletion the
//! cloud turned down leaves the user signed in, and the queue is theirs.
//!
//! The last two tests are about the other half of the finding — resuming a
//! queue that is not yours — and they are the two that most need reading
//! carefully. The service ships no ownership rule: it constructs its
//! coordinator with `KeepAllBatches` and calls `with_retention_policy` nowhere
//! in `src/`. What those tests show is that the input such a rule needs is
//! already on every device's disk, because `BatchAssembler` derives the batch id
//! as a hash over the device id and the event ids. They do not show that the
//! binary uses it.
//!
//! What this file does *not* claim: the rest of the database survives account
//! deletion. `raw_event_buffer`, `abstraction_map`, both override tables, the
//! prototype and embedding stores, and the terminal (`sent`, `rejected`,
//! `abandoned`) batch rows are all untouched by it. The deletion dialog says so
//! in those words, and `PRIVACY.md`'s removal procedure is the only complete
//! path.

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
    HttpRequest, HttpResponse,
};
use velvt_service::delivery::FakeCacheManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{BatchEvent, NewUploadBatch, SqlitePersistence, UploadBatch};
use velvt_service::upload::{
    BatchAssembler, BatchEventPayload, BatchPayload, BatchRetentionPolicy, EventIngestor,
    FakeBatchUploader, FakePrivacyAlertSink, SharedUploadBatcher, UploadBatcher, UploadCoordinator,
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

/// The queue is not as ownerless as it looks.
///
/// `upload_batch` has no `device_id` column and `BatchPayload` carries no
/// device identifier, which is what leaves the cloud attributing a batch purely
/// by bearer token. But `BatchAssembler` derives the batch id as SHA-256 over
/// the device id and the event ids in order, so the id already commits to the
/// device that minted it: the same device and the same events reproduce it, and
/// a different device holding those same events cannot.
///
/// That is the whole input an ownership rule needs, and every shipped device
/// already has it on disk — no column, no migration, and no backfill decision
/// about the rows that predate one. This pins the property so a later change to
/// the derivation cannot quietly remove it.
#[test]
fn a_batch_id_commits_to_the_device_that_minted_it() {
    let events = [payload_event("event-1"), payload_event("event-2")];

    let mine = mint(&events, "device-a").batch_id;
    let mine_again = mint(&events, "device-a").batch_id;
    let theirs = mint(&events, "device-b").batch_id;

    assert_eq!(
        mine, mine_again,
        "the device that minted a batch can recompute its id from the events"
    );
    assert_ne!(
        mine, theirs,
        "another device holding the same events cannot"
    );
}

/// The ownership rule the finding asks for, built out of that commitment: a
/// batch minted by device A is deleted rather than uploaded once this device is
/// B, and B's own batch is left alone.
///
/// Read what this does and does not show. The policy is defined in this file.
/// The shipped service constructs its coordinator with `KeepAllBatches` and
/// calls `with_retention_policy` nowhere in `src/`, so no such rule runs in the
/// binary today. This shows that the mechanism works and that the data it needs
/// is already persisted — not that the product uses it.
#[tokio::test]
async fn a_policy_that_recomputes_the_batch_id_separates_two_devices_queues() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let batches = persistence.upload_batch_repo();
    // Distinct event sets, because `batch_event.event_id` is UNIQUE and two
    // devices never hold the same events. Test 1 above is where "same events,
    // different device" is checked.
    let theirs = mint(
        &[payload_event("their-1"), payload_event("their-2")],
        "device-a",
    );
    let mine = mint(&[payload_event("my-1"), payload_event("my-2")], "device-b");
    persist(&persistence, &theirs);
    persist(&persistence, &mine);

    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let resumed = UploadCoordinator::new(
        batches.clone(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_retention_policy(Arc::new(MintedByThisDevice {
        device_id: "device-b".into(),
    }))
    .resume_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap();

    assert_eq!(resumed, 2, "both batches were read off the queue");
    assert_eq!(
        uploader.upload_count(),
        1,
        "only one of them may be POSTed under this device's token"
    );
    assert_eq!(
        batch_ids(&scratch.path),
        vec![mine.batch_id.clone()],
        "the other device's batch is deleted, not skipped and left to retry"
    );
    assert_eq!(
        batch_event_count(&scratch.path),
        2,
        "and its events go with it"
    );
}

/// The same rule on the other send path.
///
/// `resume_pending` is the retry loop. `flush_all_pending` is what the menu
/// bar's "Send all now" reaches through `FlushUploadQueue`, and it additionally
/// ignores `next_attempt_at` and the host backoff — so it is the fastest way to
/// push a queue out, and it consulted no retention policy at all until this
/// change. A rule enforced on the loop but not the button is not a rule.
#[tokio::test]
async fn the_user_facing_flush_obeys_the_same_ownership_rule() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let theirs = mint(
        &[payload_event("their-1"), payload_event("their-2")],
        "device-a",
    );
    let mine = mint(&[payload_event("my-1"), payload_event("my-2")], "device-b");
    persist(&persistence, &theirs);
    persist(&persistence, &mine);

    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader.clone(),
        FakePrivacyAlertSink::default(),
    )
    .with_retention_policy(Arc::new(MintedByThisDevice {
        device_id: "device-b".into(),
    }))
    .flush_all_pending(SCHEMA_VERSION, CLIENT_VERSION)
    .await
    .unwrap();

    assert_eq!(
        uploader.upload_count(),
        1,
        "\"Send all now\" must not send another device's queue"
    );
    assert_eq!(batch_ids(&scratch.path), vec![mine.batch_id.clone()]);
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Ownership recomputed rather than stored: re-derives the batch id from this
/// device's identity and the event ids the row already holds, and discards
/// anything that does not match. `BatchAssembler` is the only minting path in
/// `src/`, so re-running it is the same derivation rather than a second copy of
/// it.
///
/// One case this does not decide: `main.rs` mints under `"unregistered-device"`
/// when no device id is on disk yet, so a policy wired for real has to accept
/// that identity too or it deletes the first session's own queue.
struct MintedByThisDevice {
    device_id: String,
}

impl BatchRetentionPolicy for MintedByThisDevice {
    fn should_discard(&self, batch: &UploadBatch) -> bool {
        let events = batch
            .events
            .iter()
            .map(|event| BatchEventPayload {
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
        mint(&events, &self.device_id).batch_id != batch.batch_id
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

/// Copies the payload to disk the way `persist_batch` does — event ids
/// verbatim and in order. The ownership check below reads them back and rehashes
/// them, so a store that rewrote or reordered an event id would break it.
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
