//! The IPC connection against a cloud that accepts every request and never
//! answers, which is how the backend looked while it answered 522 after about
//! twenty seconds and the helper gave up after ten.
//!
//! The connection reads one message at a time and awaits each answer before it
//! reads the next, so a cloud request awaited on it holds back every raw event
//! behind it. Nothing in the shipped wiring may do that: a readiness probe, an
//! insight or history read, "Send all now", a restored session's validation,
//! a correction's sync, a log out's revocation and a filled batch's upload
//! are all started here and left outstanding, and a raw event sent after each
//! is still acknowledged within 100 ms.

use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use tokio::io::{
    duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};
use velvt_service::abstraction::AbstractionEngine;
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthManager, AuthState, AuthStateMachine, FakeTokenStore,
    HttpClient, HttpRequest, HttpResponse, SessionValidator, TokenStore,
};
use velvt_service::delivery::{FetchConfig, FetchService, PushAdapter, PushQueue};
use velvt_service::ipc::{serve_connection_with_push_queue, MenuStatusProvider, R7Router};
use velvt_service::persistence::SqlitePersistence;
use velvt_service::upload::{
    BatchAssembler, EventIngestor, FakePrivacyAlertSink, HttpBatchUploader, SharedUploadBatcher,
    UploadBatcher, UploadCoordinator,
};
use velvt_shared_types::{
    ClientHello, ClientMessage, CorrectEventClassification, FlushUploadQueue, LogOut, RawEvent,
    RequestLatestHistory, RequestLatestInsight, RequestMenuStatus, ServerMessage, PROTOCOL_VERSION,
};

const ANSWER_WITHIN: Duration = Duration::from_millis(100);

/// Accepts every request and never answers it, counting what was sent.
#[derive(Default)]
struct BlackHole {
    sent: AtomicUsize,
}

impl HttpClient for BlackHole {
    fn send<'a>(
        &'a self,
        _request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl Client {
    async fn send(&mut self, message: &ClientMessage) {
        let mut bytes = serde_json::to_vec(message).unwrap();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.unwrap();
    }

    /// The next frame, which must arrive within `ANSWER_WITHIN`.
    async fn receive(&mut self, waiting_for: &str) -> ServerMessage {
        let mut line = String::new();
        let read = tokio::time::timeout(ANSWER_WITHIN, self.reader.read_line(&mut line))
            .await
            .unwrap_or_else(|_| panic!("no frame within {ANSWER_WITHIN:?} ({waiting_for})"))
            .unwrap();
        assert!(read > 0, "the connection closed ({waiting_for})");
        serde_json::from_str(line.trim_end()).unwrap()
    }

    async fn receive_until<T>(
        &mut self,
        waiting_for: &str,
        mut pick: impl FnMut(ServerMessage) -> Option<T>,
    ) -> T {
        loop {
            if let Some(found) = pick(self.receive(waiting_for).await) {
                return found;
            }
        }
    }

    /// Sends a raw event and returns how long its ack took.
    async fn raw_event_ack_after(&mut self, after: &str) -> Duration {
        let event = RawEvent {
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
        };
        let event_id = event.event_id;
        let started = Instant::now();
        self.send(&ClientMessage::RawEvent(event)).await;
        let waiting_for = format!("the raw event ack after {after}");
        self.receive_until(&waiting_for, |message| match message {
            ServerMessage::RawEventAck(ack) if ack.event_id == event_id => Some(()),
            _ => None,
        })
        .await;
        started.elapsed()
    }
}

#[tokio::test]
async fn no_cloud_request_holds_back_a_raw_event() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let cloud = Arc::new(BlackHole::default());
    let sent = || cloud.sent.load(Ordering::SeqCst);

    let token_store = Arc::new(FakeTokenStore::default());
    let state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
    // The service's own wiring, over the black hole: one authenticated
    // transport for every cloud read and upload, the raw one for the
    // readiness probe and the log out's revocation.
    let authenticated = Arc::new(AuthManager::new(
        Arc::clone(&token_store),
        Arc::clone(&cloud),
        Arc::clone(&state),
        ChronoDuration::minutes(5),
    ));
    let queue = PushQueue::new(64);
    let push = PushAdapter::new(Arc::clone(&queue));
    let cache = Arc::new(
        FetchService::new(
            Arc::clone(&authenticated),
            persistence.history_cache_repo(),
            persistence.insight_cache_repo(),
            FetchConfig {
                history_ttl: Duration::from_secs(600),
                insight_ttl: Duration::from_secs(1_800),
                insight_negative_ttl: Duration::from_secs(600),
                read_timeout: Duration::from_millis(50),
            },
        )
        .with_push_adapter(Arc::clone(&push)),
    );
    // One event per batch: every raw event fills a batch and starts an upload.
    let ingestor: Arc<dyn EventIngestor> = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-black-hole", 1, Duration::from_secs(3_600)),
        UploadCoordinator::new(
            persistence.upload_batch_repo(),
            HttpBatchUploader::new(Arc::clone(&authenticated)),
            FakePrivacyAlertSink::default(),
        ),
    )));
    let account = Arc::new(AccountAuthService::new(
        Arc::clone(&cloud) as Arc<dyn HttpClient>,
        Arc::clone(&authenticated) as Arc<dyn HttpClient>,
        Arc::clone(&token_store) as Arc<dyn TokenStore>,
        Arc::clone(&state),
    ));
    let router = R7Router::new(
        cache,
        Arc::new(
            AbstractionEngine::from_builtin_taxonomy(persistence.abstraction_mapping_store())
                .unwrap(),
        ),
        persistence.raw_event_repo(),
        ingestor,
        account,
    )
    .with_session_validator(Arc::clone(&authenticated) as Arc<dyn SessionValidator>)
    .with_classification_corrections(
        persistence.abstraction_map_repo(),
        persistence.upload_batch_repo(),
        Arc::clone(&authenticated) as Arc<dyn HttpClient>,
    )
    .with_auth_state(state.subscribe())
    .with_delivery_push(push)
    .with_menu_status(Arc::new(MenuStatusProvider::new(
        Arc::clone(&cloud) as Arc<dyn HttpClient>,
        Arc::clone(&token_store) as Arc<dyn TokenStore>,
        persistence.upload_batch_repo(),
        persistence.raw_event_repo(),
        persistence.abstraction_map_repo(),
    )));

    let (client, server) = duplex(64 * 1024);
    let connection = tokio::spawn(serve_connection_with_push_queue(
        server,
        router,
        8,
        Some(state.subscribe()),
        queue,
        Duration::from_secs(1),
    ));
    let (reader, writer) = tokio::io::split(client);
    let mut client = Client {
        reader: BufReader::new(reader),
        writer,
    };
    client
        .receive_until("server_hello", |message| {
            matches!(message, ServerMessage::ServerHello(_)).then_some(())
        })
        .await;
    client
        .send(&ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "black-hole-test".into(),
        }))
        .await;
    client
        .receive_until("acknowledged", |message| {
            matches!(message, ServerMessage::Acknowledged(_)).then_some(())
        })
        .await;

    // A restored session two minutes from expiry: validating it starts a
    // refresh, which the black hole never answers.
    client
        .send(&ClientMessage::AuthSession(
            velvt_shared_types::AuthSession {
                device_id: "device-black-hole".into(),
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: Utc::now() + ChronoDuration::minutes(2),
                user_access_token: None,
                user_refresh_token: None,
                user_expires_at: None,
            },
        ))
        .await;
    let ack = client.raw_event_ack_after("a session validation").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");

    client
        .send(&ClientMessage::RequestMenuStatus(RequestMenuStatus {}))
        .await;
    let status = client
        .receive_until("menu_status", |message| match message {
            ServerMessage::MenuStatus(status) => Some(status),
            _ => None,
        })
        .await;
    assert!(!status.cloud_ready, "nothing has said the cloud is ready");
    let ack = client.raw_event_ack_after("a readiness probe").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");

    client
        .send(&ClientMessage::RequestLatestInsight(RequestLatestInsight {
            date: Utc::now().date_naive(),
        }))
        .await;
    let ack = client.raw_event_ack_after("an insight read").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");

    client
        .send(&ClientMessage::RequestLatestHistory(RequestLatestHistory {
            days: 14,
            utc_offset_seconds: 0,
        }))
        .await;
    let history = client
        .receive_until("history_payload", |message| match message {
            ServerMessage::HistoryPayload(history) => Some(history),
            _ => None,
        })
        .await;
    assert_eq!(history.source, velvt_shared_types::HistorySource::ThisMac);
    let ack = client.raw_event_ack_after("a history read").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");

    for press in 0..3 {
        client
            .send(&ClientMessage::FlushUploadQueue(FlushUploadQueue {}))
            .await;
        client
            .receive_until("the menu status a flush answers with", |message| {
                matches!(message, ServerMessage::MenuStatus(_)).then_some(())
            })
            .await;
        let ack = client
            .raw_event_ack_after(&format!("\"Send all now\" pressed {} times", press + 1))
            .await;
        assert!(ack < ANSWER_WITHIN, "{ack:?}");
    }

    let corrected = persistence
        .raw_event_repo()
        .events_before(Utc::now())
        .unwrap()
        .remove(0);
    client
        .send(&ClientMessage::CorrectEventClassification(
            CorrectEventClassification {
                event_id: uuid::Uuid::parse_str(&corrected.event_id).unwrap(),
                stable_id: corrected.stable_id,
                category: "REFERENCE".into(),
                local_activity_name: None,
            },
        ))
        .await;
    client
        .receive_until("the menu status a correction answers with", |message| {
            matches!(message, ServerMessage::MenuStatus(_)).then_some(())
        })
        .await;
    let ack = client.raw_event_ack_after("a correction's sync").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");

    // Every one of those reads is outstanding, and the black hole has
    // answered none of them.
    assert!(sent() >= 2, "the black hole was never asked: {}", sent());

    client.send(&ClientMessage::LogOut(LogOut {})).await;
    let ack = client.raw_event_ack_after("a log out").await;
    assert!(ack < ANSWER_WITHIN, "{ack:?}");
    assert!(
        token_store.load_tokens().unwrap().is_none(),
        "the log out did not clear the session before its revocation was answered"
    );

    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(1), connection).await;
}
