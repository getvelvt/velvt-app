//! cross-device-03 / helper-backend-03: activity recorded under one account is
//! never uploaded into another.
//!
//! The simulation of 1.0.14 found two ways it was. On one Mac, events acked
//! while account A was signed in sat in the in-memory batch; after Log Out and
//! a sign-in to account B, they were sent with B's next batch, under B's
//! token. And after A was deleted on another Mac, the queue this Mac held for
//! A was sent under the next account to sign in here.
//!
//! These tests drive the real router, account service, batcher, coordinator
//! and SQLite queue, with a scripted cloud behind the account service and a
//! recording uploader in place of `POST /v1/events/batches`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthManager, AuthState, AuthStateMachine, FakeTokenStore,
    HttpClient, HttpRequest, HttpResponse, RedactedString, TokenPair, TokenStore,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{
    end_deleted_account_session, MenuStatusProvider, MessageRouter, R7Router, SessionWatch,
};
use velvt_service::persistence::{BatchEvent, NewUploadBatch, SqlitePersistence};
use velvt_service::upload::{
    BatchAssembler, BatchDisposition, BatchEventPayload, BatchOwnershipPolicy, BatchPayload,
    BatchUploadError, BatchUploader, EventIngestor, FakePrivacyAlertSink, HttpBatchUploader,
    SessionOwnership, SharedUploadBatcher, UploadBatcher, UploadCoordinator, UploadOutcome,
};
use velvt_shared_types::{ClientMessage, LogIn, LogOut, RawEvent, RawEventStatus, ServerMessage};

/// The finding, end to end. A records three events and logs out before they
/// are batched; B signs in on the same Mac, records two, and sends the queue.
/// Only B's two may be POSTed, and A's three may not come back at a restart.
#[tokio::test]
async fn activity_queued_under_one_account_never_uploads_into_the_next() {
    let mac = Mac::new();

    let a = mac.log_in("a@example.test").await;
    let a_events = mac.record(3).await;
    assert_eq!(
        mac.uploader.sent_event_ids(),
        Vec::<String>::new(),
        "A's events are still in the open batch, which is the case the finding is about"
    );
    mac.route(ClientMessage::LogOut(LogOut {})).await;

    let b = mac.log_in("b@example.test").await;
    assert_ne!(a.user_id, b.user_id);
    assert_ne!(
        a.device_id, b.device_id,
        "B cannot reissue A's device, so this Mac registers again"
    );
    let b_events = mac.record(2).await;
    mac.ingestor.flush_now().await.unwrap();

    let sent = mac.uploader.sent_event_ids();
    assert_eq!(
        sorted(sent),
        sorted(b_events),
        "under B's token only B's events are sent; A's are neither sent nor mixed in"
    );
    assert!(
        mac.uploader.batches().iter().all(|batch| batch
            .events
            .iter()
            .all(|event| !a_events.contains(&event.event_id))),
        "no batch carried any of A's events"
    );
    assert!(
        mac.persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .is_empty(),
        "A's batch was discarded once B was signed in, not left to retry"
    );

    let recovered = mac.restart_recovery().await;
    assert_eq!(
        recovered, 0,
        "A's events are local-only now, so a restart does not re-queue them"
    );
}

/// Log Out is not a deletion: A's activity waits for A, and goes when A signs
/// in again.
#[tokio::test]
async fn activity_queued_before_log_out_is_sent_when_the_same_account_signs_back_in() {
    let mac = Mac::new();

    let first = mac.log_in("a@example.test").await;
    let before = mac.record(2).await;
    mac.route(ClientMessage::LogOut(LogOut {})).await;
    assert_eq!(
        mac.persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .len(),
        1,
        "Log Out queued A's open batch rather than leaving it in memory"
    );

    let again = mac.log_in("a@example.test").await;
    assert_eq!(first.device_id, again.device_id, "A's device is reissued");
    let after = mac.record(1).await;
    mac.ingestor.flush_now().await.unwrap();

    let mut expected = before;
    expected.extend(after);
    assert_eq!(sorted(mac.uploader.sent_event_ids()), sorted(expected));
}

/// While signed out nothing is sent, and nothing is marked
/// `authentication_required`: the queue waits, untouched, for its account.
#[tokio::test]
async fn a_signed_out_mac_sends_nothing_and_spends_nothing() {
    let mac = Mac::new();
    mac.log_in("a@example.test").await;
    mac.record(2).await;
    mac.route(ClientMessage::LogOut(LogOut {})).await;

    mac.ingestor.flush_now().await.unwrap();
    mac.coordinator().resume_pending("1", "test").await.unwrap();

    assert!(mac.uploader.sent_event_ids().is_empty());
    let queued = mac
        .persistence
        .upload_batch_repo()
        .pending_batches()
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].attempt_count, 0);
    assert_eq!(
        mac.persistence
            .upload_batch_repo()
            .queue_diagnostics()
            .unwrap()
            .last_error_code,
        None
    );
}

/// A deleted on another Mac: this Mac learns it from `401 account_deleted`
/// and destroys what it queued for A, batched or not, and tells the app why.
/// The next account to sign in here sends none of it.
#[tokio::test]
async fn a_mac_whose_account_was_deleted_elsewhere_destroys_what_it_queued() {
    let mac = Mac::new();
    mac.log_in("e@example.test").await;
    let batched = mac.record(2).await;
    mac.ingestor.persist_open().await.unwrap();
    let buffered = mac.record(1).await;
    // A batch queued before migration 0042 names no account, so the ownership
    // policy would send it under whoever signs in next. Only this destroys it.
    let legacy = mac.queue_ownerless_batch();

    // What `AuthManager` does on `401 account_deleted`.
    mac.auth.transition(AuthState::AccountDeleted).unwrap();
    let queue = PushQueue::new(10);
    let push = PushAdapter::new(Arc::clone(&queue));
    end_deleted_account_session(
        &mac.account,
        mac.ingestor.as_ref(),
        mac.persistence.upload_batch_repo().as_ref(),
        mac.persistence.raw_event_repo().as_ref(),
        &push,
    )
    .await;

    match queue.try_pop().await {
        Some(ServerMessage::NeedsReauth(needs)) => assert_eq!(needs.reason, "account_deleted"),
        other => panic!("the app must be told the account was deleted: {other:?}"),
    }
    assert_eq!(mac.auth.current(), AuthState::Unauthenticated);
    assert_eq!(mac.tokens.load_user_id().unwrap(), None);
    assert_eq!(
        mac.tokens.load_device_id().unwrap(),
        None,
        "the device row went with the account"
    );
    // Destroyed now, not left for the ownership policy to discard once
    // another account signs in.
    assert!(
        mac.persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .is_empty(),
        "every queued batch, the ownerless one too, went with the account"
    );
    assert!(
        mac.persistence
            .raw_event_repo()
            .unbatched_events(100)
            .unwrap()
            .is_empty(),
        "no event is left eligible to be batched at the next start"
    );
    assert!(
        !mac.ingestor.flush_now().await.unwrap(),
        "nothing the account recorded is left in memory"
    );

    mac.log_in("f@example.test").await;
    let f_events = mac.record(1).await;
    mac.ingestor.flush_now().await.unwrap();
    let sent = mac.uploader.sent_event_ids();
    assert_eq!(sent, f_events);
    for event_id in batched.iter().chain(&buffered).chain([&legacy]) {
        assert!(
            !sent.contains(event_id),
            "a deleted account's event was sent"
        );
    }
    assert_eq!(mac.restart_recovery().await, 0);
}

/// The watcher `main.rs` runs for the service's lifetime ends a deleted
/// account's session when the auth state says so, and asks readiness again
/// once the next session is usable. Tests above call
/// `end_deleted_account_session` directly; this holds the wiring to it.
#[tokio::test]
async fn the_session_watch_ends_a_deleted_accounts_session_and_reprobes_on_sign_in() {
    let mac = Mac::new();
    mac.log_in("e@example.test").await;
    mac.record(2).await;
    mac.ingestor.persist_open().await.unwrap();
    let readiness = Arc::new(DeviceCloud::default());
    let queue = PushQueue::new(10);
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let watch = tokio::spawn(
        SessionWatch {
            account: Arc::clone(&mac.account),
            ingestor: Arc::clone(&mac.ingestor) as Arc<dyn EventIngestor>,
            batches: mac.persistence.upload_batch_repo(),
            raw_events: mac.persistence.raw_event_repo(),
            push: PushAdapter::new(Arc::clone(&queue)),
            menu_status: Arc::new(MenuStatusProvider::new(
                Arc::clone(&readiness) as Arc<dyn HttpClient>,
                Arc::clone(&mac.tokens) as Arc<dyn TokenStore>,
                mac.persistence.upload_batch_repo(),
                mac.persistence.raw_event_repo(),
                mac.persistence.abstraction_map_repo(),
            )),
        }
        .run(mac.auth.subscribe(), shutdown),
    );

    mac.auth.transition(AuthState::AccountDeleted).unwrap();
    let pushed = eventually(|| async { queue.try_pop().await }).await;
    match pushed {
        ServerMessage::NeedsReauth(needs) => assert_eq!(needs.reason, "account_deleted"),
        other => panic!("the app must be told the account was deleted: {other:?}"),
    }
    assert_eq!(mac.auth.current(), AuthState::Unauthenticated);
    assert!(mac
        .persistence
        .upload_batch_repo()
        .pending_batches()
        .unwrap()
        .is_empty());
    assert!(readiness.bearers_sent_to("/v1/ready").is_empty());

    mac.log_in("f@example.test").await;
    eventually(|| async { (!readiness.bearers_sent_to("/v1/ready").is_empty()).then_some(()) })
        .await;

    stop.send(true).unwrap();
    watch.await.unwrap();
}

/// Polls `probe` until it answers, for at most a second.
async fn eventually<T, F, Fut>(mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    tokio::time::timeout(StdDuration::from_secs(1), async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(StdDuration::from_millis(5)).await;
        }
    })
    .await
    .expect("the session watch did not act within a second")
}

/// Events acked while A was signed in and not yet batched when the helper
/// was killed are re-queued at the next start under A, the account each was
/// recorded under (`upload_owners`), not ownerless. Ownerless, they went out
/// under whoever signed in next.
#[tokio::test]
async fn activity_recovered_at_a_restart_is_never_sent_under_the_next_account() {
    let mac = Mac::new();
    mac.log_in("a@example.test").await;
    let a_events = mac.record(3).await;

    // The helper is killed: A's events are only in its memory and in
    // `raw_event_buffer`. The next process starts with no session until the
    // app hands one over.
    mac.forget_session();
    let uploader = RecordingUploader::default();
    let mut restarted = UploadBatcher::new(
        BatchAssembler::new("unregistered-device", 100, StdDuration::from_secs(3_600)),
        mac.coordinator_with(uploader.clone()),
    );
    let recovered = restarted
        .recover_unbatched(mac.persistence.raw_event_repo().as_ref(), 100, Utc::now())
        .await
        .unwrap();
    assert_eq!(recovered, a_events.len());

    mac.log_in("b@example.test").await;
    restarted.flush_now().await.unwrap();

    assert!(
        uploader.sent_event_ids().is_empty(),
        "A's recovered events went out under B's session"
    );
    assert!(
        mac.persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .is_empty(),
        "the recovered batch was A's, and was discarded once B was signed in"
    );
    assert_eq!(mac.restart_recovery().await, 0);
}

/// The same recovery, with A the next to sign in: A's events are A's, and go.
#[tokio::test]
async fn activity_recovered_at_a_restart_is_sent_when_its_account_signs_in() {
    let mac = Mac::new();
    mac.log_in("a@example.test").await;
    let a_events = mac.record(2).await;

    mac.forget_session();
    let uploader = RecordingUploader::default();
    let mut restarted = UploadBatcher::new(
        BatchAssembler::new("unregistered-device", 100, StdDuration::from_secs(3_600)),
        mac.coordinator_with(uploader.clone()),
    );
    restarted
        .recover_unbatched(mac.persistence.raw_event_repo().as_ref(), 100, Utc::now())
        .await
        .unwrap();
    restarted.flush_now().await.unwrap();
    assert!(
        uploader.sent_event_ids().is_empty(),
        "nothing is sent before a session is handed over"
    );

    mac.log_in("a@example.test").await;
    restarted.flush_now().await.unwrap();
    assert_eq!(sorted(uploader.sent_event_ids()), sorted(a_events));
}

/// A sign-up whose device registration fails answers `auth_failure`, keeps no
/// session, and queues nothing: it used to answer `auth_success` with an empty
/// device id, which showed the person signed in while nothing uploaded.
#[tokio::test]
async fn a_sign_in_whose_registration_fails_records_nothing_for_upload() {
    let mac = Mac::new();
    mac.cloud.fail_registration();

    let reply = mac
        .route(ClientMessage::LogIn(LogIn {
            email: "a@example.test".into(),
            password: "correct horse".into(),
        }))
        .await;

    match reply {
        Some(ServerMessage::AuthFailure(failure)) => {
            assert_eq!(
                failure.message,
                "This Mac could not be registered. Please try again."
            );
        }
        other => panic!("a sign-in with no device must not succeed: {other:?}"),
    }
    assert_eq!(mac.auth.current(), AuthState::Unauthenticated);
    assert!(mac.tokens.load_tokens().unwrap().is_none());
    mac.record(1).await;
    assert!(
        mac.persistence
            .raw_event_repo()
            .unbatched_events(10)
            .unwrap()
            .is_empty(),
        "an event recorded with no session is local-only"
    );
}

// ---------------------------------------------------------------------------
// A request that waits while the session changes
// ---------------------------------------------------------------------------

/// The ownership policy judges a batch before it is sent, and the bearer
/// token is read as it is sent. A batch judged A's, waiting on the refresh
/// lock behind a slow refresh while A logged out and B signed in, was POSTed
/// under B's token. It now fails for want of A's session, and the policy's
/// next pass discards it. Any other request is bound the same way, to the
/// account signed in when it started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_waiting_on_a_refresh_is_not_sent_under_the_next_account() {
    let mac = Mac::new();
    let a = mac.log_in("a@example.test").await;
    let cloud = Arc::new(DeviceCloud::default());
    // Every token is inside a two-hour refresh buffer, so every request
    // refreshes first, one at a time behind the refresh lock.
    let manager = Arc::new(AuthManager::new(
        Arc::clone(&mac.tokens),
        Arc::clone(&cloud),
        Arc::clone(&mac.auth),
        Duration::hours(2),
    ));
    let ownership = mac.ownership();

    let poll = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move { manager.send(HttpRequest::get("/v1/insights/poll")).await })
    };
    tokio::time::sleep(StdDuration::from_millis(30)).await;

    let batch = batch_owned_by(&a.user_id);
    assert_eq!(
        ownership.disposition(batch.owner_user_id.as_deref()),
        BatchDisposition::Send
    );
    let upload = {
        let uploader = HttpBatchUploader::new(Arc::clone(&manager));
        tokio::spawn(async move { uploader.upload(&batch).await })
    };
    let outcome = {
        let manager = Arc::clone(&manager);
        tokio::spawn(async move { manager.send(HttpRequest::post("/v1/outcomes")).await })
    };
    tokio::time::sleep(StdDuration::from_millis(30)).await;

    mac.route(ClientMessage::LogOut(LogOut {})).await;
    let b = mac.log_in("b@example.test").await;
    assert_ne!(a.user_id, b.user_id);

    let _ = poll.await.unwrap();
    assert!(matches!(
        upload.await.unwrap(),
        Err(BatchUploadError::AuthenticationRequired)
    ));
    assert!(matches!(
        outcome.await.unwrap(),
        Err(AuthError::NeedsReauth)
    ));
    assert_eq!(
        cloud.bearers_sent_to("/v1/events/batches"),
        Vec::<Option<RedactedString>>::new(),
        "A's batch was POSTed under B's session"
    );
    assert_eq!(
        cloud.bearers_sent_to("/v1/outcomes"),
        Vec::<Option<RedactedString>>::new(),
        "a request A made was sent under B's session"
    );
    assert_eq!(
        ownership.disposition(Some(&a.user_id)),
        BatchDisposition::Discard
    );
    assert_eq!(
        mac.tokens.load_user_id().unwrap().as_deref(),
        Some(b.user_id.as_str()),
        "B's session is untouched"
    );
}

/// A batch in flight under A's token when A logs out and B signs in comes
/// back refused: A's tokens were revoked. The refused request used to refresh
/// (`401 invalid_token`) or reissue (`403 device_token_revoked`) whatever
/// session was current, B's, and send A's batch again under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_refused_after_the_account_changed_is_not_retried_under_the_next() {
    for (refusal, code) in [(401, "invalid_token"), (403, "device_token_revoked")] {
        let mac = Mac::new();
        let a = mac.log_in("a@example.test").await;
        let a_bearer = mac
            .tokens
            .load_tokens()
            .unwrap()
            .map(|tokens| tokens.access_token().clone());
        let cloud = Arc::new(DeviceCloud::refusing_the_first_batch(refusal, code));
        let manager = Arc::new(AuthManager::new(
            Arc::clone(&mac.tokens),
            Arc::clone(&cloud),
            Arc::clone(&mac.auth),
            Duration::zero(),
        ));

        let batch = batch_owned_by(&a.user_id);
        let upload = {
            let uploader = HttpBatchUploader::new(Arc::clone(&manager));
            tokio::spawn(async move { uploader.upload(&batch).await })
        };
        tokio::time::sleep(StdDuration::from_millis(30)).await;
        mac.route(ClientMessage::LogOut(LogOut {})).await;
        mac.log_in("b@example.test").await;

        assert!(
            matches!(
                upload.await.unwrap(),
                Err(BatchUploadError::AuthenticationRequired)
            ),
            "{code}"
        );
        assert_eq!(
            cloud.requests(),
            vec![("/v1/events/batches".to_owned(), a_bearer)],
            "{code}: B's session was refreshed or reissued, or A's batch sent again"
        );
        assert!(
            matches!(mac.auth.current(), AuthState::Authenticated { .. }),
            "{code}: B stays signed in"
        );
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct Signed {
    user_id: String,
    device_id: String,
}

/// One Mac: the service's router, account service and upload path over one
/// SQLite database, wired as `main.rs` wires them.
struct Mac {
    persistence: SqlitePersistence,
    tokens: Arc<FakeTokenStore>,
    auth: Arc<AuthStateMachine>,
    account: Arc<AccountAuthService>,
    cloud: Arc<ScriptedCloud>,
    uploader: RecordingUploader,
    ingestor: Arc<SharedUploadBatcher<RecordingUploader, FakePrivacyAlertSink>>,
    router: R7Router,
    clock: AtomicUsize,
}

impl Mac {
    fn new() -> Self {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let tokens = Arc::new(FakeTokenStore::default());
        let auth = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
        let cloud = Arc::new(ScriptedCloud::default());
        let account = Arc::new(AccountAuthService::new(
            Arc::clone(&cloud) as Arc<dyn HttpClient>,
            Arc::clone(&cloud) as Arc<dyn HttpClient>,
            Arc::clone(&tokens) as Arc<dyn TokenStore>,
            Arc::clone(&auth),
        ));
        let uploader = RecordingUploader::default();
        let ownership = Arc::new(SessionOwnership::new(
            Arc::clone(&tokens) as Arc<dyn TokenStore>,
            auth.subscribe(),
        ));
        let ingestor = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
            BatchAssembler::new("unregistered-device", 100, StdDuration::from_secs(3_600)),
            UploadCoordinator::new(
                persistence.upload_batch_repo(),
                uploader.clone(),
                FakePrivacyAlertSink::default(),
            )
            .with_ownership_policy(ownership),
        )));
        let engine = Arc::new(
            AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap(),
        );
        let router = R7Router::new(
            Arc::new(FakeCacheManager::new()),
            engine,
            persistence.raw_event_repo(),
            Arc::clone(&ingestor) as Arc<dyn EventIngestor>,
            Arc::clone(&account),
        )
        .with_auth_state(auth.subscribe())
        .with_classification_corrections(
            persistence.abstraction_map_repo(),
            persistence.upload_batch_repo(),
            Arc::clone(&cloud) as Arc<dyn HttpClient>,
        );
        Self {
            persistence,
            tokens,
            auth,
            account,
            cloud,
            uploader,
            ingestor,
            router,
            clock: AtomicUsize::new(0),
        }
    }

    async fn route(&self, message: ClientMessage) -> Option<ServerMessage> {
        self.router.route(message).await.unwrap()
    }

    async fn log_in(&self, email: &str) -> Signed {
        match self
            .route(ClientMessage::LogIn(LogIn {
                email: email.into(),
                password: "correct horse".into(),
            }))
            .await
        {
            Some(ServerMessage::AuthSuccess(success)) => {
                assert!(!success.device_id.is_empty());
                Signed {
                    user_id: success.user_id,
                    device_id: success.device_id,
                }
            }
            other => panic!("log in as {email} failed: {other:?}"),
        }
    }

    /// Records `count` dwells in an editor and returns their event ids.
    async fn record(&self, count: usize) -> Vec<String> {
        let mut event_ids = Vec::new();
        for _ in 0..count {
            let tick = self.clock.fetch_add(1, Ordering::SeqCst) as i64;
            let event = RawEvent {
                event_id: uuid::Uuid::new_v4(),
                occurred_at: Utc::now() - Duration::minutes(30) + Duration::seconds(tick),
                app_name: "Xcode".into(),
                window_title: "AccountSwitch.swift".into(),
                bundle_id: None,
                declared_app_category: None,
                document_type_ids: Vec::new(),
                focused_document_url: None,
                in_progress: false,
                duration_seconds: 60,
            };
            let event_id = event.event_id.to_string();
            match self.route(ClientMessage::RawEvent(event)).await {
                Some(ServerMessage::RawEventAck(ack)) => {
                    assert_eq!(ack.status, RawEventStatus::Accepted);
                }
                other => panic!("the event was not accepted: {other:?}"),
            }
            event_ids.push(event_id);
        }
        event_ids
    }

    fn coordinator(&self) -> UploadCoordinator<RecordingUploader, FakePrivacyAlertSink> {
        self.coordinator_with(self.uploader.clone())
    }

    /// A coordinator over this Mac's queue and session, sending through
    /// `uploader`, as `main.rs` builds one.
    fn coordinator_with(
        &self,
        uploader: RecordingUploader,
    ) -> UploadCoordinator<RecordingUploader, FakePrivacyAlertSink> {
        UploadCoordinator::new(
            self.persistence.upload_batch_repo(),
            uploader,
            FakePrivacyAlertSink::default(),
        )
        .with_ownership_policy(Arc::new(SessionOwnership::new(
            Arc::clone(&self.tokens) as Arc<dyn TokenStore>,
            self.auth.subscribe(),
        )))
    }

    fn ownership(&self) -> SessionOwnership {
        SessionOwnership::new(
            Arc::clone(&self.tokens) as Arc<dyn TokenStore>,
            self.auth.subscribe(),
        )
    }

    /// What a new helper process starts with: the session lives in memory
    /// only (`VolatileTokenStore`), so none, until the app hands one over.
    fn forget_session(&self) {
        self.tokens.clear_tokens().unwrap();
        self.tokens.clear_user_tokens().unwrap();
        self.tokens.clear_user_id().unwrap();
        self.tokens.clear_device_id().unwrap();
        self.auth.transition(AuthState::Unauthenticated).unwrap();
    }

    /// Queues a batch as a helper before migration 0042 did, with no owner,
    /// and returns its one event's id.
    fn queue_ownerless_batch(&self) -> String {
        let event_id = uuid::Uuid::new_v4().to_string();
        self.persistence
            .upload_batch_repo()
            .insert_batch_with_events(
                &NewUploadBatch {
                    batch_id: "queued-before-0042".into(),
                    owner_user_id: None,
                },
                &[BatchEvent {
                    event_id: event_id.clone(),
                    stable_id: "abs_legacy".into(),
                    label: "document:code".into(),
                    category: "FOCUS_WORK".into(),
                    taxonomy_version: "mvp-2".into(),
                    classification_tier: "exact_match".into(),
                    occurred_at: Utc::now() - Duration::hours(2),
                    duration_seconds: 60,
                }],
            )
            .unwrap();
        event_id
    }

    /// What the next start would re-queue: every upload-eligible event that
    /// is in no batch.
    async fn restart_recovery(&self) -> usize {
        let mut restarted = UploadBatcher::new(
            BatchAssembler::new("unregistered-device", 100, StdDuration::from_secs(3_600)),
            UploadCoordinator::new(
                self.persistence.upload_batch_repo(),
                RecordingUploader::default(),
                FakePrivacyAlertSink::default(),
            ),
        );
        restarted
            .recover_unbatched(self.persistence.raw_event_repo().as_ref(), 100, Utc::now())
            .await
            .unwrap()
    }
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// velvt-core's account endpoints, as far as a sign-in needs them: login,
/// device registration and reissue scoped to the account that owns the device,
/// and logout.
#[derive(Default)]
struct ScriptedCloud {
    /// Each user access token issued, and its account.
    user_tokens: Mutex<Vec<(RedactedString, String)>>,
    /// Device id to the account that registered it.
    devices: Mutex<HashMap<String, String>>,
    next_device: AtomicUsize,
    registration_fails: std::sync::atomic::AtomicBool,
}

impl ScriptedCloud {
    fn fail_registration(&self) {
        self.registration_fails.store(true, Ordering::SeqCst);
    }

    fn answer(&self, request: &HttpRequest) -> HttpResponse {
        let account_of_bearer = request.authorization.as_ref().and_then(|bearer| {
            self.user_tokens
                .lock()
                .unwrap()
                .iter()
                .find(|(token, _)| token == bearer)
                .map(|(_, account)| account.clone())
        });
        match request.path.as_str() {
            "/v1/auth/login" => {
                let email = request.json_body.as_ref().unwrap()["email"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let account = format!("account-{email}");
                let issued = tokens(&format!("user-access:{account}"));
                self.user_tokens
                    .lock()
                    .unwrap()
                    .push((issued.access_token().clone(), account.clone()));
                HttpResponse {
                    user_id: Some(account),
                    tokens: Some(issued),
                    ..status(200)
                }
            }
            "/v1/devices" => {
                if self.registration_fails.load(Ordering::SeqCst) {
                    return status(503);
                }
                let account = account_of_bearer.expect("registration carries the user token");
                let device = format!("device-{}", self.next_device.fetch_add(1, Ordering::SeqCst));
                self.devices.lock().unwrap().insert(device.clone(), account);
                HttpResponse {
                    device_id: Some(device.clone()),
                    tokens: Some(tokens(&format!("device-access:{device}"))),
                    ..status(201)
                }
            }
            "/v1/auth/devices/reissue" => {
                let account = account_of_bearer.expect("reissue carries the user token");
                let device = request.json_body.as_ref().unwrap()["device_id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                if self.devices.lock().unwrap().get(&device) != Some(&account) {
                    return HttpResponse {
                        error_code: Some("device_not_found".into()),
                        ..status(404)
                    };
                }
                HttpResponse {
                    device_id: Some(device.clone()),
                    tokens: Some(tokens(&format!("device-access:{device}"))),
                    ..status(200)
                }
            }
            "/v1/auth/logout" => status(204),
            other => panic!("the account service sent an unexpected request to {other}"),
        }
    }
}

impl HttpClient for ScriptedCloud {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        let response = self.answer(&request);
        Box::pin(async move { Ok(response) })
    }
}

fn status(status: u16) -> HttpResponse {
    HttpResponse {
        status,
        error_code: None,
        tokens: None,
        retry_after: None,
        message: None,
        raw_body: None,
        user_id: None,
        device_id: None,
    }
}

fn tokens(access: &str) -> TokenPair {
    TokenPair::new(
        RedactedString::new(access),
        RedactedString::new(format!("{access}:refresh")),
        Utc::now() + Duration::hours(1),
    )
}

/// A one-event batch recorded under `owner`.
fn batch_owned_by(owner: &str) -> BatchPayload {
    BatchPayload::new(
        uuid::Uuid::new_v4().to_string(),
        "1",
        "test",
        Vec::new(),
        "mvp-2",
        vec![BatchEventPayload {
            event_id: uuid::Uuid::new_v4().to_string(),
            stable_id: "abs_1".into(),
            label: "document:code".into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-2".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc::now() - Duration::minutes(10),
            duration_seconds: 60,
        }],
    )
    .owned_by(Some(owner.to_owned()))
}

/// The device-authenticated cloud an `AuthManager` sends through. A refresh
/// takes `REFRESH_TAKES` and renews the session it was sent for; the first
/// batch POST can be held and refused; everything else is accepted.
#[derive(Default)]
struct DeviceCloud {
    /// Every request, as its path and bearer token.
    sent: Mutex<Vec<(String, Option<RedactedString>)>>,
    first_batch: Mutex<Option<HttpResponse>>,
    renewals: AtomicUsize,
}

impl DeviceCloud {
    const REFRESH_TAKES: StdDuration = StdDuration::from_millis(300);
    const REFUSAL_TAKES: StdDuration = StdDuration::from_millis(300);

    /// Holds the first batch POST for `REFUSAL_TAKES`, then refuses it.
    fn refusing_the_first_batch(status_code: u16, error_code: &str) -> Self {
        Self {
            first_batch: Mutex::new(Some(HttpResponse {
                error_code: Some(error_code.to_owned()),
                ..status(status_code)
            })),
            ..Self::default()
        }
    }

    fn requests(&self) -> Vec<(String, Option<RedactedString>)> {
        self.sent.lock().unwrap().clone()
    }

    fn bearers_sent_to(&self, path: &str) -> Vec<Option<RedactedString>> {
        self.requests()
            .into_iter()
            .filter(|(sent_to, _)| sent_to == path)
            .map(|(_, bearer)| bearer)
            .collect()
    }
}

impl HttpClient for DeviceCloud {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let bearer = request.authorization.clone();
            self.sent
                .lock()
                .unwrap()
                .push((request.path.clone(), bearer));
            match request.path.as_str() {
                "/v1/auth/refresh" => {
                    tokio::time::sleep(Self::REFRESH_TAKES).await;
                    let renewal = self.renewals.fetch_add(1, Ordering::SeqCst);
                    Ok(HttpResponse {
                        tokens: Some(tokens(&format!("renewed-{renewal}"))),
                        ..status(200)
                    })
                }
                "/v1/events/batches" => {
                    let refusal = self.first_batch.lock().unwrap().take();
                    match refusal {
                        Some(refusal) => {
                            tokio::time::sleep(Self::REFUSAL_TAKES).await;
                            Ok(refusal)
                        }
                        None => Ok(status(202)),
                    }
                }
                _ => Ok(status(200)),
            }
        })
    }
}

/// `POST /v1/events/batches`, accepting everything and keeping what it was
/// sent.
#[derive(Clone, Default)]
struct RecordingUploader {
    sent: Arc<Mutex<Vec<BatchPayload>>>,
}

impl RecordingUploader {
    fn batches(&self) -> Vec<BatchPayload> {
        self.sent.lock().unwrap().clone()
    }

    fn sent_event_ids(&self) -> Vec<String> {
        self.batches()
            .into_iter()
            .flat_map(|batch| batch.events.into_iter().map(|event| event.event_id))
            .collect()
    }
}

impl BatchUploader for RecordingUploader {
    fn upload<'a>(
        &'a self,
        batch: &'a BatchPayload,
    ) -> Pin<Box<dyn Future<Output = Result<UploadOutcome, BatchUploadError>> + Send + 'a>> {
        self.sent.lock().unwrap().push(batch.clone());
        Box::pin(async { Ok(UploadOutcome::Accepted) })
    }
}
