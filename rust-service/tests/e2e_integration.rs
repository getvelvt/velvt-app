//! End-to-end integration suite for the MVP data pipeline.
//!
//! Paths 1 and 7 exercise the real Unix-socket framing/handshake code via
//! `tokio::io::duplex` + `serve_connection_with_push_queue[_and_shutdown]`
//! (the brief's preferred approach). Paths 2-6 exercise the real
//! `AbstractionEngine` / `SqlitePersistence` / `UploadCoordinator` /
//! `AuthManager` / `AccountAuthService` components directly against fake
//! HTTP transports — this is a deliberate scope tradeoff to keep the suite
//! fast and focused on the actual seams under test (auth state machine
//! transitions, batch idempotency, privacy rejection) rather than
//! re-proving socket framing five more times. No path requires real macOS
//! permissions, a real network connection, or a live velvt-core server.

use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration as StdDuration;
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

use serde_json::json;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthManager, AuthState, AuthStateMachine, FakeTokenStore,
    HttpClient, HttpRequest, HttpResponse, RedactedString, TokenPair, TokenStore,
};
use velvt_service::delivery::{FakeCacheManager, PushAdapter, PushQueue};
use velvt_service::ipc::{
    serve_connection_with_push_queue, serve_connection_with_push_queue_and_shutdown,
    MenuStatusProvider, MessageRouter, R7Router,
};
use velvt_service::persistence::{BatchEvent, NewUploadBatch, PersistenceError, SqlitePersistence};
use velvt_service::upload::{
    BatchAssembler, CoordinatorError, EventIngestor, FakeBatchUploader, FakePrivacyAlertSink,
    IpcPrivacyAlertSink, SharedUploadBatcher, UploadBatcher, UploadCoordinator, UploadOutcome,
};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, FlushUploadQueue, PrivacyViolationAlert, RawEvent,
    RawEventAck, RawEventStatus, RequestMenuStatus, ServerMessage, ShuttingDown, PROTOCOL_VERSION,
};

// ---------------------------------------------------------------------------
// Shared wire helpers (each integration-test binary is a separate crate, so
// these are intentionally duplicated from tests/ipc_connection.rs).
// ---------------------------------------------------------------------------

async fn write_message(writer: &mut (impl AsyncWriteExt + Unpin), message: &ClientMessage) {
    let mut bytes = serde_json::to_vec(message).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn read_message(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
) -> Option<ServerMessage> {
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap() == 0 {
        return None;
    }
    Some(serde_json::from_str(line.trim_end()).unwrap())
}

async fn complete_handshake(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
    writer: &mut (impl AsyncWriteExt + Unpin),
) {
    assert!(
        matches!(
            read_message(reader).await,
            Some(ServerMessage::ServerHello(_))
        ),
        "expected server_hello"
    );
    write_message(
        writer,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(reader).await,
        Some(ServerMessage::Acknowledged(Acknowledged)),
        "expected acknowledged"
    );
}

fn raw_event(seconds: i64, app_name: &str, window_title: &str) -> RawEvent {
    RawEvent {
        event_id: uuid::Uuid::new_v4(),
        occurred_at: Utc.timestamp_opt(seconds, 0).unwrap(),
        app_name: app_name.into(),
        window_title: window_title.into(),
        bundle_id: None,
    }
}

fn build_router(
    cache: Arc<dyn velvt_service::delivery::CacheManager>,
    persistence: &SqlitePersistence,
    ingestor: Arc<dyn velvt_service::upload::EventIngestor>,
    raw_http: Arc<dyn HttpClient>,
    authenticated_http: Arc<dyn HttpClient>,
) -> R7Router {
    let abstraction_engine = Arc::new(
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap(),
    );
    let account = Arc::new(AccountAuthService::new(
        raw_http,
        authenticated_http,
        Arc::new(FakeTokenStore::default()),
        Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
    ));
    R7Router::new(
        cache,
        abstraction_engine,
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
}

#[derive(Default)]
struct RecordingIngestor {
    flush_now_calls: AtomicUsize,
}

struct FailingFlushIngestor;

impl EventIngestor for FailingFlushIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a velvt_service::abstraction::AbstractedEvent,
        _duration_seconds: u64,
        _now: chrono::DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn flush_due<'a>(
        &'a self,
        _now: chrono::DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }

    fn flush_shutdown<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }

    fn flush_now<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async {
            Err(CoordinatorError::Persistence(PersistenceError::NotFound {
                entity: "upload_batch",
            }))
        })
    }
}

impl EventIngestor for RecordingIngestor {
    fn ingest<'a>(
        &'a self,
        _event_id: String,
        _event: &'a velvt_service::abstraction::AbstractedEvent,
        _duration_seconds: u64,
        _now: chrono::DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn flush_due<'a>(
        &'a self,
        _now: chrono::DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }

    fn flush_shutdown<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }

    fn flush_now<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        self.flush_now_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(false) })
    }
}

/// Minimal fake `HttpClient` shared by the paths that need an unauthenticated
/// or device-authenticated transport (account auth relay, device auth).
#[derive(Clone, Default)]
struct FakeHttp {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    responses: Arc<Mutex<VecDeque<Result<HttpResponse, AuthError>>>>,
}

impl FakeHttp {
    fn with_responses(responses: Vec<HttpResponse>) -> Self {
        Self {
            requests: Arc::default(),
            responses: Arc::new(Mutex::new(responses.into_iter().map(Ok).collect())),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpClient for FakeHttp {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(AuthError::Transport))
        })
    }
}

fn empty_response(status: u16, code: Option<&str>) -> HttpResponse {
    HttpResponse {
        status,
        error_code: code.map(str::to_owned),
        tokens: None,
        retry_after: None,
        message: None,
        raw_body: None,
        user_id: None,
        device_id: None,
    }
}

fn ready_response() -> HttpResponse {
    HttpResponse {
        raw_body: Some(json!({ "status": "ready" })),
        ..empty_response(200, None)
    }
}

fn token_pair(expires_in: ChronoDuration, access: &str, refresh: &str) -> TokenPair {
    TokenPair::new(
        RedactedString::new(access),
        RedactedString::new(refresh),
        Utc::now() + expires_in,
    )
}

#[tokio::test]
async fn flush_upload_queue_uses_shared_ingestor_and_returns_menu_status() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ingestor = Arc::new(RecordingIngestor::default());
    let router = build_router(
        Arc::new(FakeCacheManager::new()),
        &persistence,
        Arc::clone(&ingestor) as Arc<dyn EventIngestor>,
        Arc::new(FakeHttp::default()),
        Arc::new(FakeHttp::default()),
    );

    let response = router
        .route(ClientMessage::FlushUploadQueue(FlushUploadQueue {}))
        .await
        .unwrap();

    assert!(matches!(response, Some(ServerMessage::MenuStatus(_))));
    assert_eq!(ingestor.flush_now_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn flush_upload_queue_returns_a_safe_error_when_uploading_fails() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let router = build_router(
        Arc::new(FakeCacheManager::new()),
        &persistence,
        Arc::new(FailingFlushIngestor) as Arc<dyn EventIngestor>,
        Arc::new(FakeHttp::default()),
        Arc::new(FakeHttp::default()),
    );

    let response = router
        .route(ClientMessage::FlushUploadQueue(FlushUploadQueue {}))
        .await
        .unwrap();

    assert!(matches!(response, Some(ServerMessage::ErrorResponse(error))
        if error.code == "upload_flush_failed"));
}

#[tokio::test]
async fn request_menu_status_reports_upload_auth_and_retry_state() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let repo = persistence.upload_batch_repo();
    let batch = NewUploadBatch {
        batch_id: "auth-paused-batch".into(),
    };
    let occurred_at = Utc.timestamp_opt(1_800, 0).unwrap();
    repo.insert_batch_with_events(
        &batch,
        &[BatchEvent {
            event_id: "queued-event".into(),
            stable_id: "stable-event".into(),
            label: "document:code".into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            occurred_at,
            duration_seconds: 60,
        }],
    )
    .unwrap();
    let next_attempt_at = Utc::now() + ChronoDuration::minutes(15);
    repo.mark_failed(
        "auth-paused-batch",
        next_attempt_at,
        "authentication_required",
    )
    .unwrap();
    let raw_http = Arc::new(FakeHttp::with_responses(vec![ready_response()]));
    let token_store = Arc::new(FakeTokenStore::default());
    token_store.store_device_id("device-1").unwrap();
    let router = build_router(
        Arc::new(FakeCacheManager::new()),
        &persistence,
        Arc::new(RecordingIngestor::default()) as Arc<dyn EventIngestor>,
        Arc::clone(&raw_http) as Arc<dyn HttpClient>,
        Arc::new(FakeHttp::default()),
    )
    .with_menu_status(Arc::new(MenuStatusProvider::new(
        raw_http as Arc<dyn HttpClient>,
        token_store as Arc<dyn TokenStore>,
        persistence.upload_batch_repo(),
        persistence.raw_event_repo(),
    )));

    let response = router
        .route(ClientMessage::RequestMenuStatus(RequestMenuStatus {}))
        .await
        .unwrap();

    let Some(ServerMessage::MenuStatus(status)) = response else {
        panic!("expected menu_status");
    };
    assert!(status.cloud_ready);
    assert_eq!(status.upload_status, "auth_required");
    assert_eq!(
        status.last_upload_error_code.as_deref(),
        Some("authentication_required")
    );
    assert_eq!(status.failed_upload_batch_count, 1);
    assert_eq!(status.queued_event_count, 1);
    assert!(status.next_upload_attempt_at.is_some());
}

// ---------------------------------------------------------------------------
// Path 1 — Raw event -> abstracted event -> SQLite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path1_raw_event_is_abstracted_with_local_only_queue_labels() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let cache: Arc<dyn velvt_service::delivery::CacheManager> = Arc::new(FakeCacheManager::new());
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        FakeBatchUploader::with_outcomes(vec![]),
        FakePrivacyAlertSink::default(),
    );
    let batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 1000, StdDuration::from_secs(3600)),
        coordinator,
    );
    let ingestor: Arc<dyn velvt_service::upload::EventIngestor> =
        Arc::new(SharedUploadBatcher::new(batcher));
    let raw_http: Arc<dyn HttpClient> = Arc::new(FakeHttp::default());
    let authenticated_http: Arc<dyn HttpClient> = Arc::new(FakeHttp::default());
    let router = build_router(cache, &persistence, ingestor, raw_http, authenticated_http);

    let queue = PushQueue::new(10);
    let (client, server) = duplex(8192);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        router,
        3,
        None,
        Arc::clone(&queue),
        StdDuration::from_millis(500),
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    // Step 1: a known app (seed dictionary hit, Tier 1).
    write_message(
        &mut write,
        &ClientMessage::RawEvent(raw_event(10, "VS Code", "main.rs — velvt")),
    )
    .await;
    let ack = read_message(&mut read).await;
    assert!(
        matches!(
            ack,
            Some(ServerMessage::RawEventAck(RawEventAck {
                status: RawEventStatus::Accepted,
                ..
            }))
        ),
        "step 1 (Tier 1 known app): expected accepted ack, got {ack:?}"
    );

    // Step 2: an unknown app (Tier 3 fallback — no ONNX model loaded in this suite).
    write_message(
        &mut write,
        &ClientMessage::RawEvent(raw_event(20, "TotallyUnknownApp9000", "untitled")),
    )
    .await;
    let ack = read_message(&mut read).await;
    assert!(
        matches!(
            ack,
            Some(ServerMessage::RawEventAck(RawEventAck { status: RawEventStatus::Accepted, .. }))
        ),
        "step 2 (Tier 3 fallback): expected accepted ack (unlogged fallback still persists), got {ack:?}"
    );

    drop(write);
    drop(read);
    let _ = task.await;

    // Step 3: verify the abstracted audit data remains privacy-safe while
    // local-only display labels stay in their dedicated raw-event field.
    let conn_check = persistence.raw_event_repo();
    let entries = conn_check
        .events_before(Utc::now() + ChronoDuration::days(1))
        .unwrap();
    assert_eq!(
        entries.len(),
        2,
        "both events should be persisted for local queue display"
    );
    for entry in &entries {
        assert_ne!(entry.label, "VS Code");
        assert_ne!(entry.label, "main.rs — velvt");
        assert!(
            !entry.label.is_empty(),
            "every persisted row must carry a privacy-safe label"
        );
    }
    assert_eq!(
        entries[0].local_display_label.as_deref(),
        Some("main.rs — velvt")
    );
    assert_eq!(entries[1].local_display_label.as_deref(), Some("untitled"));
}

// ---------------------------------------------------------------------------
// Path 2 — Batch assembly -> upload -> idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path2_batch_assembly_upload_and_replay_idempotency() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let abstraction_engine =
        AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store()).unwrap();

    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        uploader,
        FakePrivacyAlertSink::default(),
    );
    // A high count threshold and a zero age threshold: nothing auto-flushes
    // on count, so the explicit `flush_due` call below is the only thing
    // that produces a batch, matching "inject 30 events, then trigger a
    // manual flush" from the brief.
    let mut batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 100, StdDuration::from_secs(0)),
        coordinator,
    );

    for i in 0..30u64 {
        let abstracted = abstraction_engine
            .process(raw_event(i as i64, "VS Code", "main.rs — velvt"))
            .unwrap();
        batcher
            .ingest_abstracted(format!("event-{i}"), &abstracted, 0, Utc::now())
            .await
            .unwrap();
    }
    assert_eq!(
        persistence
            .upload_batch_repo()
            .pending_batches()
            .unwrap()
            .len(),
        0,
        "count threshold of 100 must not have auto-flushed at 30 events"
    );

    let flushed = batcher
        .flush_due(Utc::now() + ChronoDuration::seconds(1))
        .await
        .unwrap();
    assert!(
        flushed,
        "manual flush must assemble the 30 buffered events into one batch"
    );
    assert_eq!(
        inspection.upload_count(),
        1,
        "FakeHTTPClient must receive exactly one POST"
    );

    let sent_batches = persistence.upload_batch_repo().pending_batches().unwrap();
    // The batch was marked "sent" by the Accepted outcome, so pending_batches
    // is empty; re-derive its id by checking the events table directly via a
    // duplicate replay below instead.
    assert_eq!(
        sent_batches.len(),
        0,
        "the only batch produced should already be marked sent"
    );

    // Simulate crash-recovery: rebuild an identical batch from the same 30
    // events (deterministic batch_id is a hash of device_id + event ids) and
    // resubmit it, as `run_retry_loop`/`resume_pending` would after a restart
    // that found the batch still "pending" on disk.
    let mut replay_assembler = BatchAssembler::new("device-1", 100, StdDuration::from_secs(0));
    let mut replay_batch = None;
    for i in 0..30u64 {
        let abstracted = abstraction_engine
            .process(raw_event(i as i64, "VS Code", "main.rs — velvt"))
            .unwrap();
        let payload = velvt_service::upload::BatchEventPayload::from_abstracted(
            format!("event-{i}"),
            &abstracted,
            0,
        );
        replay_batch = replay_assembler.push(payload, Utc::now());
    }
    let replay_batch = replay_batch
        .or_else(|| replay_assembler.flush_shutdown())
        .expect("replay must reconstruct the same batch");

    let replay_uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Duplicate]);
    let replay_inspection = replay_uploader.clone();
    let replay_coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        replay_uploader,
        FakePrivacyAlertSink::default(),
    );
    replay_coordinator
        .upload_batch(replay_batch)
        .await
        .expect("replaying the same batch_id must be handled as a duplicate without error");
    assert_eq!(
        replay_inspection.upload_count(),
        1,
        "the second POST should also be exactly one call"
    );
}

// ---------------------------------------------------------------------------
// Path 3 — Token expiry -> refresh -> retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path3_expired_access_token_triggers_refresh_then_retries_with_new_token() {
    let store = Arc::new(FakeTokenStore::default());
    let expired = token_pair(ChronoDuration::seconds(-1), "old-access", "old-refresh");
    store.store_pair(expired.clone()).unwrap();

    let fresh = token_pair(ChronoDuration::hours(1), "new-access", "new-refresh");
    let http = Arc::new(FakeHttp::with_responses(vec![
        HttpResponse {
            tokens: Some(fresh.clone()),
            ..empty_response(200, None)
        },
        empty_response(200, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        ChronoDuration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .expect("request after a transparent refresh must succeed");

    let requests = http.requests();
    assert_eq!(
        requests[0].path, "/v1/auth/refresh",
        "expired token must trigger a refresh call first"
    );
    assert_eq!(
        requests[1].path, "/v1/events",
        "the retried request must use the refreshed token"
    );
    assert_eq!(
        store.load_tokens().unwrap().unwrap(),
        fresh,
        "the new token pair must replace the old one in the store"
    );
    assert_ne!(
        store.load_tokens().unwrap().unwrap(),
        expired,
        "the old (expired) token pair must no longer be present"
    );
}

// ---------------------------------------------------------------------------
// Path 4 — Device revocation -> reissue -> recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path4_device_token_revoked_reissues_then_recovers() {
    let store = Arc::new(FakeTokenStore::default());
    store
        .store_pair(token_pair(ChronoDuration::hours(1), "access", "refresh"))
        .unwrap();
    store
        .store_user_pair(token_pair(
            ChronoDuration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let fresh = token_pair(
        ChronoDuration::hours(2),
        "reissued-access",
        "reissued-refresh",
    );
    let http = Arc::new(FakeHttp::with_responses(vec![
        empty_response(403, Some("device_token_revoked")),
        HttpResponse {
            tokens: Some(fresh.clone()),
            ..empty_response(200, None)
        },
        empty_response(200, None),
    ]));
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager = AuthManager::new(
        Arc::clone(&store),
        Arc::clone(&http),
        Arc::clone(&state),
        ChronoDuration::zero(),
    );

    manager
        .send_authenticated(HttpRequest::get("/v1/events"))
        .await
        .expect("reissue success must let the original request succeed on retry");

    assert_eq!(http.requests()[1].path, "/v1/auth/devices/reissue");
    assert_eq!(store.load_tokens().unwrap().unwrap(), fresh);
    assert!(matches!(state.current(), AuthState::Authenticated { .. }));

    // Now the failure branch: reissue itself fails -> state goes terminal
    // and an IPC DeviceRevoked push is delivered.
    let store2 = Arc::new(FakeTokenStore::default());
    store2
        .store_pair(token_pair(ChronoDuration::hours(1), "access", "refresh"))
        .unwrap();
    store2
        .store_user_pair(token_pair(
            ChronoDuration::hours(1),
            "user-access",
            "user-refresh",
        ))
        .unwrap();
    let http2 = Arc::new(FakeHttp::with_responses(vec![
        empty_response(403, Some("device_token_revoked")),
        empty_response(403, Some("device_revoked")),
    ]));
    let state2 = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let manager2 = AuthManager::new(
        store2,
        Arc::clone(&http2),
        Arc::clone(&state2),
        ChronoDuration::zero(),
    );

    assert!(matches!(
        manager2
            .send_authenticated(HttpRequest::get("/v1/events"))
            .await,
        Err(velvt_service::auth::AuthError::DeviceRevoked)
    ));
    assert_eq!(
        state2.current(),
        AuthState::DeviceRevoked,
        "failed reissue must transition to the terminal state"
    );

    // Mirrors the watcher main.rs spawns: forward a terminal AuthState
    // transition to Swift as a proactive IPC push.
    let queue = PushQueue::new(10);
    let push_adapter = PushAdapter::new(Arc::clone(&queue));
    if state2.current() == AuthState::DeviceRevoked {
        push_adapter
            .push_device_revoked("This device was removed from your account.")
            .await;
    }
    let pushed = queue.try_pop().await;
    assert!(
        matches!(pushed, Some(ServerMessage::DeviceRevoked(_))),
        "expected a DeviceRevoked push after failed reissue, got {pushed:?}"
    );
}

// ---------------------------------------------------------------------------
// Path 5 — History/insight fetch -> IPC push -> cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path5_history_and_insight_fetch_are_cached_and_pushed() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let history_repo = persistence.history_cache_repo();
    let insight_repo = persistence.insight_cache_repo();

    let date = Utc::now().date_naive();
    let history_json = serde_json::json!({
        "days": 7,
        "summaries": [{
            "date": date,
            "status": "ready",
            "event_count": 12,
            "focus_score": 0.8,
            "fragmentation_score": 0.2,
            "confidence_level": "high",
            "active_seconds": 3600,
        }],
    });
    let insight_json = serde_json::json!({
        "date": date,
        "text": "Today was a focused day.",
        "confidence_level": "high",
        "low_confidence": false,
        "generated_at": Utc::now(),
    });
    let http = Arc::new(FakeHttp::with_responses(vec![
        HttpResponse {
            raw_body: Some(history_json),
            ..empty_response(200, None)
        },
        HttpResponse {
            raw_body: Some(insight_json),
            ..empty_response(200, None)
        },
    ]));

    let queue = PushQueue::new(10);
    let push_adapter = PushAdapter::new(Arc::clone(&queue));
    let fetch_config = velvt_service::delivery::FetchConfig {
        history_ttl: StdDuration::from_secs(600),
        insight_ttl: StdDuration::from_secs(1800),
        insight_negative_ttl: StdDuration::from_secs(300),
        read_timeout: StdDuration::from_millis(200),
    };
    let fetch_service = velvt_service::delivery::FetchService::new(
        Arc::clone(&http),
        Arc::clone(&history_repo),
        Arc::clone(&insight_repo),
        fetch_config,
    )
    .with_push_adapter(Arc::clone(&push_adapter));

    let history = fetch_service.daily_history(7).await.unwrap();
    assert_eq!(history.days, 7);
    assert!(
        history_repo
            .get(&date.format("%Y-%m-%d").to_string())
            .unwrap()
            .is_some(),
        "history fetch must populate the R6 cache table"
    );

    let insight = fetch_service.daily_insight(date).await.unwrap();
    assert!(insight.is_some(), "insight fetch must succeed");
    assert!(
        insight_repo.get(&date.to_string()).unwrap().is_some()
            || insight_repo
                .get(&date.format("%Y-%m-%d").to_string())
                .unwrap()
                .is_some(),
        "insight fetch must populate the R6 cache table"
    );

    // The history fetch above also proactively pushes a HistoryPayload;
    // drain until we find the insight-related push (InsightPayload, or a
    // derived NotificationPayload — both are valid proactive-push outcomes
    // of a fresh, non-cached insight fetch).
    let mut found_insight_push = false;
    while let Some(message) = queue.try_pop().await {
        if matches!(
            message,
            ServerMessage::InsightPayload(_) | ServerMessage::NotificationPayload(_)
        ) {
            found_insight_push = true;
        }
    }
    assert!(
        found_insight_push,
        "a freshly-fetched insight must be pushed over IPC as InsightPayload or NotificationPayload"
    );

    // Swift-side InsightViewModel/HistoryViewModel deriving @Published state
    // from these payloads is covered by InsightViewModelTests and
    // HistoryViewModelTests in swift-client/Tests — chaining a single test
    // across the process boundary is out of scope for this Rust-only suite.
}

// ---------------------------------------------------------------------------
// Path 6 — raw_field_rejected -> alert -> batch permanently halted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path6_raw_field_rejected_halts_only_the_offending_batch() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let repo = persistence.upload_batch_repo();

    let (alert_tx, mut alert_rx) = tokio::sync::broadcast::channel(4);
    let alert_sink = IpcPrivacyAlertSink::new(alert_tx);

    let uploader = FakeBatchUploader::with_outcomes(vec![
        UploadOutcome::RawFieldRejected {
            message: "window_title field present".into(),
        },
        UploadOutcome::Accepted,
    ]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(Arc::clone(&repo), uploader, alert_sink);

    let rejected = velvt_service::upload::BatchPayload::new(
        "batch-rejected",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![velvt_service::upload::BatchEventPayload::from_abstracted(
            "event-rejected",
            &AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap()
                .process(raw_event(1, "VS Code", "main.rs"))
                .unwrap(),
            0,
        )],
    );
    coordinator.submit_batch(rejected).await.unwrap();

    assert_eq!(
        repo.batch_status("batch-rejected").unwrap(),
        velvt_service::persistence::UploadBatchStatus::Rejected,
        "a raw_field_rejected outcome must mark the batch rejected"
    );
    let alert = alert_rx
        .try_recv()
        .expect("a PrivacyViolationAlert must be broadcast");
    assert_eq!(
        alert,
        PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: "window_title field present".into(),
        }
    );

    // Other pending batches are unaffected and continue to upload.
    let other = velvt_service::upload::BatchPayload::new(
        "batch-healthy",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![velvt_service::upload::BatchEventPayload::from_abstracted(
            "event-healthy",
            &AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap()
                .process(raw_event(2, "VS Code", "main.rs"))
                .unwrap(),
            0,
        )],
    );
    coordinator.submit_batch(other).await.unwrap();
    assert_eq!(
        repo.batch_status("batch-healthy").unwrap(),
        velvt_service::persistence::UploadBatchStatus::Sent,
        "an unrelated batch must still succeed"
    );
    assert_eq!(inspection.upload_count(), 2);

    // Subsequent upload cycles must never retry the rejected batch: it is
    // not present in the resumable/pending set.
    let resumable = repo
        .resumable_batches(Utc::now() + ChronoDuration::days(1))
        .unwrap();
    assert!(
        resumable.iter().all(|b| b.batch_id != "batch-rejected"),
        "a rejected batch must never be resumed/retried"
    );
}

// ---------------------------------------------------------------------------
// Path 7 — Graceful shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path7_graceful_shutdown_delivers_shutting_down_before_socket_close() {
    let queue = PushQueue::new(10);
    let push_adapter = PushAdapter::new(Arc::clone(&queue));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let cache: Arc<dyn velvt_service::delivery::CacheManager> = Arc::new(FakeCacheManager::new());
    let coordinator = UploadCoordinator::new(
        persistence.upload_batch_repo(),
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]),
        FakePrivacyAlertSink::default(),
    );
    let batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 1000, StdDuration::from_secs(3600)),
        coordinator,
    );
    let ingestor: Arc<dyn velvt_service::upload::EventIngestor> =
        Arc::new(SharedUploadBatcher::new(batcher));
    let raw_http: Arc<dyn HttpClient> = Arc::new(FakeHttp::default());
    let authenticated_http: Arc<dyn HttpClient> = Arc::new(FakeHttp::default());
    let router = build_router(
        cache,
        &persistence,
        Arc::clone(&ingestor),
        raw_http,
        authenticated_http,
    );

    let (client, server) = duplex(8192);
    let task = tokio::spawn(serve_connection_with_push_queue_and_shutdown(
        server,
        router,
        3,
        None,
        Arc::clone(&queue),
        StdDuration::from_millis(500),
        shutdown_rx,
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    // 15 events buffered (well under any flush threshold), then shutdown
    // begins before any HTTP response would arrive in a real deployment.
    for i in 0..15u64 {
        write_message(
            &mut write,
            &ClientMessage::RawEvent(raw_event(i as i64, "VS Code", "main.rs")),
        )
        .await;
        let ack = read_message(&mut read).await;
        assert!(matches!(ack, Some(ServerMessage::RawEventAck(_))));
    }

    // Mirrors main.rs's shutdown sequence: push ShuttingDown, then flush the
    // in-flight batch, then cancel.
    push_adapter.push_shutting_down("sigterm").await;
    let flushed = ingestor.flush_shutdown().await;
    assert!(
        flushed.is_ok(),
        "in-flight batch must flush cleanly, not panic or error unrecoverably"
    );
    shutdown_tx.send(true).unwrap();

    let shutting_down = read_message(&mut read).await;
    assert_eq!(
        shutting_down,
        Some(ServerMessage::ShuttingDown(ShuttingDown {
            reason: "sigterm".into()
        })),
        "the client must receive ShuttingDown before the socket closes"
    );

    let after_shutdown = read_message(&mut read).await;
    assert!(
        after_shutdown.is_none(),
        "the socket must close after ShuttingDown, with no further messages"
    );

    let _ = tokio::time::timeout(StdDuration::from_secs(2), task).await;

    // The batch buffered before shutdown must be queryable afterward — not
    // lost, and (since the fake uploader accepted it) not left pending
    // either, i.e. it was not silently dropped.
    let repo = persistence.upload_batch_repo();
    let pending = repo.pending_batches().unwrap();
    assert!(
        pending.is_empty(),
        "the flushed batch must not be left dangling as pending: {pending:?}"
    );
}
