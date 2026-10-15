//! The helper against a fake velvt-core over a real socket, in the two shapes
//! it has to work with: the server deployed before velvt-core #41-#43 ("old":
//! rate-limit 429s with "Too many requests." and no `Retry-After`, and a
//! device update that velvt-core develop records but an older deployment, or
//! a proxy in front of one, answers 404 or 405), and one with them ("new":
//! `Retry-After` in whole seconds on every rate-limit 429, and a device object
//! that also carries `notify_here`).
//!
//! Every request goes through `ReqwestHttpClient`, so headers are parsed as
//! the shipped service parses them, and the signed-in ones through
//! `AuthManager`, as in `main.rs`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthManager, AuthState, AuthStateMachine, FakeTokenStore, HttpClient,
    ReqwestHttpClient, SessionValidator, TokenStore,
};
use velvt_service::delivery::FakeCacheManager;
use velvt_service::ipc::{MessageRouter, R7Router};
use velvt_service::persistence::{NewUploadBatch, SqlitePersistence};
use velvt_service::upload::{
    BatchAssembler, BatchPayload, EventIngestor, FakePrivacyAlertSink, HostBackoff,
    HttpBatchUploader, SharedUploadBatcher, UploadBatcher, UploadCoordinator,
};
use velvt_shared_types::{
    AuthFailure, AuthFailureCode, AuthSession, ClientMessage, RawEvent, ServerMessage,
};

// ---------------------------------------------------------------------------
// A fake velvt-core
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Server {
    /// Before #41-#43: answers `PATCH /v1/devices/{id}` with this status (200
    /// as velvt-core develop does, 404 for a deployment without the route, 405
    /// for one that takes no PATCH), and rate-limit 429s with "Too many
    /// requests." and no `Retry-After`.
    Old { device_update: u16 },
    /// With #41-#43.
    New,
}

#[derive(Debug, Clone)]
struct Arrival {
    method: String,
    path: String,
    headers: String,
    body: String,
}

struct Reply {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

impl Reply {
    fn json(status: &'static str, body: serde_json::Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    fn with_header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    fn bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
            self.status,
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(self.body.as_bytes());
        bytes
    }
}

struct FakeCore {
    base_url: String,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
}

impl FakeCore {
    /// `hold_device_update` leaves every `PATCH /v1/devices/…` unanswered.
    async fn start(server: Server, hold_device_update: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&arrivals);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let sink = Arc::clone(&sink);
                tokio::spawn(async move {
                    let Some(arrival) = read_request(&mut socket).await else {
                        return;
                    };
                    sink.lock().unwrap().push(arrival.clone());
                    if hold_device_update
                        && arrival.method == "PATCH"
                        && arrival.path.starts_with("/v1/devices/")
                    {
                        std::future::pending::<()>().await;
                    }
                    let reply = answer(server, &arrival);
                    let _ = socket.write_all(&reply.bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { base_url, arrivals }
    }

    fn arrivals(&self) -> Vec<Arrival> {
        self.arrivals.lock().unwrap().clone()
    }

    fn count(&self, method: &str, path_prefix: &str) -> usize {
        self.arrivals()
            .iter()
            .filter(|arrival| arrival.method == method && arrival.path.starts_with(path_prefix))
            .count()
    }

    /// Waits until `count` requests matching `method` and `path_prefix` have
    /// arrived.
    async fn wait_for(&self, method: &str, path_prefix: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.count(method, path_prefix) < count {
            assert!(
                Instant::now() < deadline,
                "{method} {path_prefix} arrived {} times, expected {count}: {:?}",
                self.count(method, path_prefix),
                self.arrivals()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Arrival> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let content_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let (request_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    let mut parts = request_line.split(' ');
    Some(Arrival {
        method: parts.next()?.to_owned(),
        path: parts.next()?.to_owned(),
        headers: headers.to_ascii_lowercase(),
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn answer(server: Server, arrival: &Arrival) -> Reply {
    let rate_limited = |new_message: &str| match server {
        Server::New => Reply::json(
            "429 Too Many Requests",
            serde_json::json!({"error": {"code": "rate_limit_exceeded", "message": new_message}}),
        ),
        Server::Old { .. } => Reply::json(
            "429 Too Many Requests",
            serde_json::json!({"error": {"code": "rate_limit_exceeded", "message": "Too many requests."}}),
        ),
    };
    match (arrival.method.as_str(), arrival.path.as_str()) {
        ("GET", "/v1/auth/session") => Reply::json(
            "200 OK",
            serde_json::json!({"user": {"id": "user-1", "status": "active"}}),
        ),
        ("PATCH", path) if path.starts_with("/v1/devices/") => match server {
            Server::New => Reply::json(
                "200 OK",
                serde_json::json!({
                    "id": "device-1",
                    "device_public_id": "dev_public",
                    "client_version": "1.0.15",
                    "supported_abstraction_types": [],
                    "capabilities": {},
                    "apns_environment": null,
                    "notify_here": true,
                    "status": "active",
                    "disabled_reason": null
                }),
            ),
            Server::Old { device_update: 200 } => Reply::json(
                "200 OK",
                serde_json::json!({
                    "id": "device-1",
                    "device_public_id": "dev_public",
                    "client_version": "1.0.15",
                    "supported_abstraction_types": [],
                    "capabilities": {},
                    "apns_environment": null,
                    "status": "active",
                    "disabled_reason": null
                }),
            ),
            Server::Old { device_update: 404 } => {
                Reply::json("404 Not Found", serde_json::json!({"detail": "Not Found"}))
            }
            Server::Old { .. } => Reply::json(
                "405 Method Not Allowed",
                serde_json::json!({"detail": "Method Not Allowed"}),
            ),
        },
        ("POST", "/v1/auth/login") => {
            let reply = rate_limited("Too many attempts. Try again in 15 minutes.");
            match server {
                Server::New => reply.with_header("retry-after", "900"),
                Server::Old { .. } => reply,
            }
        }
        ("POST", "/v1/events/batches") => {
            let reply = rate_limited("Too many requests.");
            match server {
                Server::New => reply.with_header("retry-after", "600"),
                Server::Old { .. } => reply,
            }
        }
        _ => Reply::json("404 Not Found", serde_json::json!({"detail": "Not Found"})),
    }
}

// ---------------------------------------------------------------------------
// The helper, wired as main.rs wires it
// ---------------------------------------------------------------------------

struct Helper {
    router: R7Router,
    state: Arc<AuthStateMachine>,
    _persistence: SqlitePersistence,
}

fn helper(core: &FakeCore) -> Helper {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let raw_http = Arc::new(ReqwestHttpClient::new(
        core.base_url.clone(),
        persistence.egress_ledger_repo(),
    ));
    let token_store = Arc::new(FakeTokenStore::default());
    let state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
    let authenticated = Arc::new(AuthManager::new(
        Arc::clone(&token_store),
        Arc::clone(&raw_http),
        Arc::clone(&state),
        ChronoDuration::minutes(5),
    ));
    let ingestor: Arc<dyn EventIngestor> = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-1", 50, Duration::from_secs(3_600)),
        UploadCoordinator::new(
            persistence.upload_batch_repo(),
            HttpBatchUploader::new(Arc::clone(&authenticated)),
            FakePrivacyAlertSink::default(),
        ),
    )));
    let account = Arc::new(AccountAuthService::new(
        Arc::clone(&raw_http) as Arc<dyn HttpClient>,
        Arc::clone(&authenticated) as Arc<dyn HttpClient>,
        Arc::clone(&token_store) as Arc<dyn TokenStore>,
        Arc::clone(&state),
    ));
    let router = R7Router::new(
        Arc::new(FakeCacheManager::default()),
        Arc::new(
            AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap(),
        ),
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
    .with_session_validator(Arc::clone(&authenticated) as Arc<dyn SessionValidator>)
    .with_auth_state(state.subscribe());
    Helper {
        router,
        state,
        _persistence: persistence,
    }
}

/// The session the app hands over on every connection.
fn restored_session() -> ClientMessage {
    ClientMessage::AuthSession(AuthSession {
        device_id: "device-1".into(),
        access_token: "device-access".into(),
        refresh_token: "device-refresh".into(),
        expires_at: Utc::now() + ChronoDuration::hours(1),
        user_access_token: None,
        user_refresh_token: None,
        user_expires_at: None,
        user_id: Some("user-1".into()),
    })
}

/// Sends a raw event and returns how long its acknowledgement took.
async fn raw_event_ack_time(router: &R7Router) -> Duration {
    let started = Instant::now();
    let answer = router
        .route(ClientMessage::RawEvent(RawEvent {
            event_id: uuid::Uuid::new_v4(),
            occurred_at: Utc::now() - ChronoDuration::seconds(30),
            app_name: "Visual Studio Code".into(),
            window_title: "main.rs".into(),
            bundle_id: None,
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: None,
            in_progress: false,
            duration_seconds: 30,
        }))
        .await
        .unwrap();
    assert!(
        matches!(answer, Some(ServerMessage::RawEventAck(_))),
        "{answer:?}"
    );
    started.elapsed()
}

// ---------------------------------------------------------------------------
// The running version, once per launch (install-release-09)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_restored_session_tells_the_new_server_the_running_version_once() {
    let core = FakeCore::start(Server::New, false).await;
    let helper = helper(&core);

    assert!(helper
        .router
        .route(restored_session())
        .await
        .unwrap()
        .is_none());
    core.wait_for("PATCH", "/v1/devices/", 1).await;

    let patch = core
        .arrivals()
        .into_iter()
        .find(|arrival| arrival.method == "PATCH")
        .unwrap();
    assert_eq!(patch.path, "/v1/devices/device-1");
    assert_eq!(
        patch.body,
        format!(
            r#"{{"client_version":"{}"}}"#,
            velvt_service::build_info::SERVICE_VERSION
        ),
        "the app version, and nothing else"
    );
    assert!(patch
        .headers
        .contains("authorization: bearer device-access"));
    let order = core
        .arrivals()
        .iter()
        .map(|arrival| format!("{} {}", arrival.method, arrival.path))
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec!["GET /v1/auth/session", "PATCH /v1/devices/device-1"],
        "sent only once the server has confirmed the session"
    );

    // The app hands the session over again on every connection.
    helper.router.route(restored_session()).await.unwrap();
    core.wait_for("GET", "/v1/auth/session", 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(core.count("PATCH", "/v1/devices/"), 1, "once per launch");
    assert_eq!(
        helper.state.current(),
        AuthState::Authenticated {
            device_id: "device-1".into()
        }
    );
}

/// velvt-core develop records the version (200); a deployment without the
/// route answers 404 or 405. Either is its answer, not an error: the session
/// stands, nothing is sent again this launch, and the helper goes on as
/// before.
#[tokio::test]
async fn an_old_server_is_answered_once_and_left_alone_whatever_it_says() {
    for status in [200, 404, 405] {
        let core = FakeCore::start(
            Server::Old {
                device_update: status,
            },
            false,
        )
        .await;
        let helper = helper(&core);

        helper.router.route(restored_session()).await.unwrap();
        core.wait_for("PATCH", "/v1/devices/", 1).await;
        helper.router.route(restored_session()).await.unwrap();
        core.wait_for("GET", "/v1/auth/session", 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(core.count("PATCH", "/v1/devices/"), 1, "{status}");
        assert_eq!(
            helper.state.current(),
            AuthState::Authenticated {
                device_id: "device-1".into()
            },
            "{status}: the session stands"
        );
        let ack = raw_event_ack_time(&helper.router).await;
        assert!(ack < Duration::from_millis(100), "{status}: {ack:?}");
    }
}

/// The report runs in the background: a server that never answers it holds
/// back no raw event.
#[tokio::test]
async fn a_report_the_server_never_answers_holds_back_no_raw_event() {
    let core = FakeCore::start(Server::New, true).await;
    let helper = helper(&core);

    helper.router.route(restored_session()).await.unwrap();
    core.wait_for("PATCH", "/v1/devices/", 1).await;

    let ack = raw_event_ack_time(&helper.router).await;
    assert!(ack < Duration::from_millis(100), "{ack:?}");
}

/// Nor the next session's check. The report ran inside the validation's
/// single-flight task, so while it waited (up to the request timeout) a
/// session handed over on a new connection was applied but never checked
/// with the server. Only one report is out at a time.
#[tokio::test]
async fn a_report_the_server_never_answers_holds_back_no_session_check() {
    let core = FakeCore::start(Server::New, true).await;
    let helper = helper(&core);

    helper.router.route(restored_session()).await.unwrap();
    core.wait_for("PATCH", "/v1/devices/", 1).await;
    helper.router.route(restored_session()).await.unwrap();

    core.wait_for("GET", "/v1/auth/session", 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(core.count("PATCH", "/v1/devices/"), 1);
}

// ---------------------------------------------------------------------------
// A rate-limited sign-in (backend-e2e-07, helper-backend-12)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rate_limited_log_in_says_how_long_to_wait_from_either_server() {
    for (server, expected) in [
        (Server::New, "Too many attempts. Try again in 15 minutes."),
        (
            Server::Old { device_update: 404 },
            "Too many attempts. Try again later.",
        ),
    ] {
        let core = FakeCore::start(server, false).await;
        let helper = helper(&core);

        let answer = helper
            .router
            .route(ClientMessage::LogIn(velvt_shared_types::LogIn {
                email: "person@example.test".into(),
                password: "correct horse battery".into(),
            }))
            .await
            .unwrap();

        assert_eq!(
            answer,
            Some(ServerMessage::AuthFailure(AuthFailure {
                code: AuthFailureCode::RateLimited,
                message: expected.into(),
            })),
            "{server:?}"
        );
        assert_eq!(
            core.arrivals().len(),
            1,
            "{server:?}: no device request after a refused sign-in"
        );
        assert_eq!(helper.state.current(), AuthState::Unauthenticated);
    }
}

// ---------------------------------------------------------------------------
// An ingestion 429
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_ingestion_429_waits_as_long_as_the_server_says() {
    // The new server asks for 600 seconds. The old one says nothing, and the
    // queue waits its own backoff, 30 seconds for a first failure.
    for (server, not_before, by) in [
        (Server::New, 500, 700),
        (Server::Old { device_update: 404 }, 0, 31),
    ] {
        let core = FakeCore::start(server, false).await;
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let token_store = Arc::new(FakeTokenStore::default());
        token_store
            .store_pair(velvt_service::auth::TokenPair::new(
                velvt_service::auth::RedactedString::new("device-access"),
                velvt_service::auth::RedactedString::new("device-refresh"),
                Utc::now() + ChronoDuration::hours(1),
            ))
            .unwrap();
        let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
            device_id: "device-1".into(),
        }));
        let authenticated = Arc::new(AuthManager::new(
            token_store,
            Arc::new(ReqwestHttpClient::new(
                core.base_url.clone(),
                persistence.egress_ledger_repo(),
            )),
            Arc::clone(&state),
            ChronoDuration::minutes(5),
        ));
        let repository = persistence.upload_batch_repo();
        repository
            .insert_batch(&NewUploadBatch {
                batch_id: "batch-1".into(),
                owner_user_id: None,
            })
            .unwrap();
        let coordinator = UploadCoordinator::new(
            Arc::clone(&repository),
            HttpBatchUploader::new(authenticated),
            FakePrivacyAlertSink::default(),
        )
        .with_host_and_backoff(
            "127.0.0.1",
            HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0),
        );
        let before = Utc::now();

        coordinator
            .upload_batch(BatchPayload::new(
                "batch-1",
                "1",
                "1.0.15",
                vec!["document:edit".into()],
                "mvp-2",
                Vec::new(),
            ))
            .await
            .unwrap();

        assert_eq!(core.count("POST", "/v1/events/batches"), 1, "{server:?}");
        if not_before > 0 {
            assert!(
                repository
                    .resumable_batches(before + ChronoDuration::seconds(not_before))
                    .unwrap()
                    .is_empty(),
                "{server:?}: sent again before the server's Retry-After"
            );
        }
        assert_eq!(
            repository
                .resumable_batches(before + ChronoDuration::seconds(by))
                .unwrap()
                .len(),
            1,
            "{server:?}"
        );
        assert_eq!(
            repository
                .queue_diagnostics()
                .unwrap()
                .last_error_code
                .as_deref(),
            Some("rate_limited"),
            "{server:?}"
        );
        assert!(matches!(state.current(), AuthState::Authenticated { .. }));
    }
}
