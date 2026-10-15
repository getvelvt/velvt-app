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
    AccountAuthService, AuthError, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    HttpRequest, HttpResponse, RedactedString, TokenPair, TokenStore,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{end_deleted_account_session, MessageRouter, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::upload::{
    BatchAssembler, BatchPayload, BatchUploadError, BatchUploader, EventIngestor,
    FakePrivacyAlertSink, SessionOwnership, SharedUploadBatcher, UploadBatcher, UploadCoordinator,
    UploadOutcome,
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

    mac.log_in("f@example.test").await;
    let f_events = mac.record(1).await;
    mac.ingestor.flush_now().await.unwrap();
    let sent = mac.uploader.sent_event_ids();
    assert_eq!(sent, f_events);
    for event_id in batched.iter().chain(&buffered) {
        assert!(
            !sent.contains(event_id),
            "a deleted account's event was sent"
        );
    }
    assert_eq!(mac.restart_recovery().await, 0);
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
        UploadCoordinator::new(
            self.persistence.upload_batch_repo(),
            self.uploader.clone(),
            FakePrivacyAlertSink::default(),
        )
        .with_ownership_policy(Arc::new(SessionOwnership::new(
            Arc::clone(&self.tokens) as Arc<dyn TokenStore>,
            self.auth.subscribe(),
        )))
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
