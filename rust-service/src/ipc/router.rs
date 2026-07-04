use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use velvt_shared_types::{
    CacheEmpty, ClientMessage, MenuStatus, QueuedEventSummary, RawEventAck, RawEventStatus,
    ServerMessage,
};

use crate::abstraction::AbstractionEngine;
use crate::auth::{
    AccountAuthService, AuthError, HttpClient, HttpRequest, SessionValidator, TokenStore,
};
use crate::delivery::{shaper, CacheManager};
use crate::persistence::{RawEventEntry, RawEventRepo, UploadBatchRepo};
use crate::upload::EventIngestor;

use super::IpcError;

/// Routes validated post-handshake messages independently of their transport.
///
/// The returned future must be `Send`: the transport layer spawns each
/// connection's message handling onto a `JoinSet`, which requires `Send`
/// futures. Declaring the bound here (rather than relying on the default
/// `async fn` desugaring) keeps that requirement visible at the trait
/// definition instead of surfacing as a confusing error deep in `transport.rs`.
#[allow(async_fn_in_trait)]
pub trait MessageRouter {
    fn route(
        &self,
        message: ClientMessage,
    ) -> impl std::future::Future<Output = Result<Option<ServerMessage>, IpcError>> + Send;
}

pub trait MenuStatusProviding: Send + Sync {
    fn snapshot<'a>(&'a self) -> Pin<Box<dyn Future<Output = MenuStatus> + Send + 'a>>;
}

pub struct MenuStatusProvider {
    http: Arc<dyn HttpClient>,
    token_store: Arc<dyn TokenStore>,
    batches: Arc<dyn UploadBatchRepo>,
    raw_events: Arc<dyn RawEventRepo>,
    readiness: Mutex<Option<(Instant, bool)>>,
}

impl MenuStatusProvider {
    pub fn new(
        http: Arc<dyn HttpClient>,
        token_store: Arc<dyn TokenStore>,
        batches: Arc<dyn UploadBatchRepo>,
        raw_events: Arc<dyn RawEventRepo>,
    ) -> Self {
        Self {
            http,
            token_store,
            batches,
            raw_events,
            readiness: Mutex::new(None),
        }
    }
}

impl MenuStatusProviding for MenuStatusProvider {
    fn snapshot<'a>(&'a self) -> Pin<Box<dyn Future<Output = MenuStatus> + Send + 'a>> {
        Box::pin(async move {
            let cached_ready = self.readiness.lock().ok().and_then(|cache| {
                cache.as_ref().and_then(|(checked_at, ready)| {
                    (checked_at.elapsed() < Duration::from_secs(60)).then_some(*ready)
                })
            });
            let cloud_ready = match cached_ready {
                Some(ready) => ready,
                None => {
                    let ready = matches!(self.http.send(HttpRequest::get("/v1/ready")).await, Ok(response) if response.status / 100 == 2 && response.raw_body.as_ref().and_then(|body| body.get("status")).and_then(|value| value.as_str()) == Some("ready"));
                    if let Ok(mut cache) = self.readiness.lock() {
                        *cache = Some((Instant::now(), ready));
                    }
                    ready
                }
            };
            let mut events: Vec<_> = self
                .batches
                .pending_batches()
                .unwrap_or_default()
                .into_iter()
                .flat_map(|batch| batch.events)
                .collect();
            events.sort_by_key(|event| std::cmp::Reverse(event.occurred_at));
            let queued_event_count = events.len() as u64;
            let event_ids = events
                .iter()
                .map(|event| event.event_id.clone())
                .collect::<Vec<_>>();
            let local_labels = self
                .raw_events
                .local_display_labels(&event_ids)
                .unwrap_or_default();
            let queued_events = events
                .into_iter()
                .take(10)
                .map(|event| QueuedEventSummary {
                    label: event.label,
                    local_label: local_labels.get(&event.event_id).cloned(),
                    category: event.category,
                    occurred_at: event.occurred_at,
                })
                .collect::<Vec<_>>();
            let unbatched = self.raw_events.unbatched_events(10).unwrap_or_default();
            let queued_event_count = queued_event_count + unbatched.len() as u64;
            let queued_events = queued_events
                .into_iter()
                .chain(unbatched.into_iter().map(|event| QueuedEventSummary {
                    label: event.label,
                    local_label: event.local_display_label,
                    category: event.category,
                    occurred_at: event.occurred_at,
                }))
                .take(10)
                .collect();
            MenuStatus {
                device_id: self.token_store.load_device_id().unwrap_or_default(),
                cloud_ready,
                queued_event_count,
                queued_events,
            }
        })
    }
}

struct EmptyMenuStatusProvider;
impl MenuStatusProviding for EmptyMenuStatusProvider {
    fn snapshot<'a>(&'a self) -> Pin<Box<dyn Future<Output = MenuStatus> + Send + 'a>> {
        Box::pin(async {
            MenuStatus {
                device_id: None,
                cloud_ready: false,
                queued_event_count: 0,
                queued_events: vec![],
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{FakeTokenStore, HttpResponse, TokenStore};
    use crate::persistence::SqlitePersistence;
    use std::future::Future;

    struct ReadyHttp;

    impl HttpClient for ReadyHttp {
        fn send<'a>(
            &'a self,
            _request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
            Box::pin(async {
                Ok(HttpResponse {
                    status: 200,
                    error_code: None,
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: Some(serde_json::json!({ "status": "ready" })),
                    user_id: None,
                    device_id: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn menu_status_reads_device_id_stored_after_provider_construction() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let token_store = Arc::new(FakeTokenStore::default());
        let provider = MenuStatusProvider::new(
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            persistence.upload_batch_repo(),
            persistence.raw_event_repo(),
        );

        token_store.store_device_id("device-1").unwrap();

        let status = provider.snapshot().await;

        assert_eq!(status.device_id.as_deref(), Some("device-1"));
    }
}

/// Minimal R1 router used until business handlers are introduced.
#[derive(Debug, Clone, Copy)]
pub struct DefaultRouter;

impl MessageRouter for DefaultRouter {
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError> {
        match message {
            ClientMessage::ClientHello(_) => Err(IpcError::MalformedMessage),
            _ => Ok(None),
        }
    }
}

/// R7 router: handles on-demand insight and history requests from Swift, raw
/// event ingestion, and the v6 account-auth relay.
///
/// On a cache miss or validation failure the router returns `CacheEmpty` so
/// Swift can display a loading state rather than crashing.  Cache errors are
/// logged but never surfaced to the transport layer.
#[derive(Clone)]
pub struct R7Router {
    cache: Arc<dyn CacheManager>,
    abstraction_engine: Arc<AbstractionEngine>,
    raw_event_repo: Arc<dyn RawEventRepo>,
    ingestor: Arc<dyn EventIngestor>,
    account: Arc<AccountAuthService>,
    menu_status: Arc<dyn MenuStatusProviding>,
    session_validator: Option<Arc<dyn SessionValidator>>,
}

impl R7Router {
    pub fn new(
        cache: Arc<dyn CacheManager>,
        abstraction_engine: Arc<AbstractionEngine>,
        raw_event_repo: Arc<dyn RawEventRepo>,
        ingestor: Arc<dyn EventIngestor>,
        account: Arc<AccountAuthService>,
    ) -> Self {
        Self {
            cache,
            abstraction_engine,
            raw_event_repo,
            ingestor,
            account,
            menu_status: Arc::new(EmptyMenuStatusProvider),
            session_validator: None,
        }
    }

    pub fn with_menu_status(mut self, menu_status: Arc<dyn MenuStatusProviding>) -> Self {
        self.menu_status = menu_status;
        self
    }

    pub fn with_session_validator(mut self, session_validator: Arc<dyn SessionValidator>) -> Self {
        self.session_validator = Some(session_validator);
        self
    }
}

impl MessageRouter for R7Router {
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError> {
        match message {
            ClientMessage::ClientHello(_) => Err(IpcError::MalformedMessage),

            ClientMessage::RawEvent(event) => Ok(Some(self.handle_raw_event(event).await)),

            ClientMessage::SignUp(req) => {
                tracing::info!(
                    message_type = "sign_up",
                    "received account credential request"
                );
                Ok(Some(self.account.sign_up(req.email, req.password).await))
            }

            ClientMessage::LogIn(req) => {
                tracing::info!(
                    message_type = "log_in",
                    "received account credential request"
                );
                Ok(Some(self.account.log_in(req.email, req.password).await))
            }

            ClientMessage::AuthSession(session) => {
                self.account.apply_session(session);
                if let Some(session_validator) = &self.session_validator {
                    match session_validator.validate_restored_session().await {
                        Ok(()) => {
                            tracing::info!(
                                message_type = "auth_session",
                                "restored auth session validated"
                            );
                        }
                        Err(AuthError::Transport | AuthError::RateLimited) => {
                            tracing::warn!(
                                message_type = "auth_session",
                                "restored auth session validation was deferred"
                            );
                        }
                        Err(error) => {
                            tracing::warn!(
                                message_type = "auth_session",
                                error = %error,
                                "restored auth session validation failed"
                            );
                        }
                    }
                }
                Ok(None)
            }

            ClientMessage::LogOut(_) => {
                self.account.log_out().await;
                Ok(None)
            }

            ClientMessage::DeleteAccount(_) => Ok(Some(self.account.delete_account().await)),

            ClientMessage::RequestMenuStatus(_) => Ok(Some(ServerMessage::MenuStatus(
                self.menu_status.snapshot().await,
            ))),

            ClientMessage::FlushUploadQueue(_) => {
                if self.ingestor.flush_now().await.is_err() {
                    tracing::error!(
                        error_code = "upload_flush_now_failed",
                        "failed to flush the upload queue"
                    );
                    return Ok(Some(ServerMessage::ErrorResponse(
                        velvt_shared_types::ErrorResponse {
                            code: "upload_flush_failed".into(),
                            message: "Unable to send queued events. Try again later.".into(),
                            related_event_id: None,
                        },
                    )));
                }
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status.snapshot().await,
                )))
            }

            ClientMessage::RequestLatestInsight(req) => {
                let result = self.cache.daily_insight(req.date).await;
                let response = match result {
                    Ok(Some(insight)) => match shaper::shape_insight(insight) {
                        Ok(validated) => ServerMessage::InsightPayload(validated.into_inner()),
                        Err(err) => {
                            tracing::warn!(
                                message_type = "insight_payload",
                                error_code = "outbound_validation_failed",
                                error = %err,
                                "shaped insight failed validation; sending cache_empty"
                            );
                            cache_empty("insight_payload")
                        }
                    },
                    Ok(None) => cache_empty("insight_payload"),
                    Err(err) => {
                        tracing::warn!(
                            date = %req.date,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read insight from cache"
                        );
                        cache_empty("insight_payload")
                    }
                };
                Ok(Some(response))
            }

            ClientMessage::RequestLatestHistory(req) => {
                let result = self.cache.daily_history(req.days).await;
                let response = match result {
                    Ok(history) => match shaper::shape_history(history) {
                        Ok(validated) => ServerMessage::HistoryPayload(validated.into_inner()),
                        Err(err) => {
                            tracing::warn!(
                                message_type = "history_payload",
                                error_code = "outbound_validation_failed",
                                error = %err,
                                "shaped history failed validation; sending cache_empty"
                            );
                            cache_empty("history_payload")
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            days = req.days,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read history from cache"
                        );
                        cache_empty("history_payload")
                    }
                };
                Ok(Some(response))
            }

            _ => Ok(None),
        }
    }
}

impl R7Router {
    /// Runs the privacy-enforcement boundary: classify, persist a privacy-safe
    /// audit row, feed the upload batcher, and acknowledge. Raw `app_name`/
    /// `window_title` are consumed only by `abstraction_engine.process` and
    /// never appear in `RawEventEntry`, `BatchEventPayload`, or this ack.
    async fn handle_raw_event(&self, event: velvt_shared_types::RawEvent) -> ServerMessage {
        let event_id = event.event_id;
        let occurred_at = event.occurred_at;
        let local_display_label = raw_display_label(&event.app_name, &event.window_title);
        match self.abstraction_engine.process(event) {
            Ok(abstracted) => {
                let entry = RawEventEntry {
                    event_id: event_id.to_string(),
                    stable_id: abstracted.stable_id().to_owned(),
                    label: abstracted.label().to_owned(),
                    local_display_label,
                    category: abstracted.category().to_owned(),
                    taxonomy_version: abstracted.taxonomy_version().to_owned(),
                    occurred_at,
                    duration_seconds: 0,
                };
                if let Err(err) = self.raw_event_repo.insert(&entry) {
                    tracing::error!(
                        error_code = "raw_event_persist_failed",
                        error = %err,
                        "failed to persist abstracted event audit row"
                    );
                    return ServerMessage::RawEventAck(RawEventAck {
                        event_id,
                        status: RawEventStatus::Dropped,
                        drop_reason: Some("persistence_failed".into()),
                    });
                }
                if let Err(err) = self
                    .ingestor
                    .ingest(event_id.to_string(), &abstracted, 0, Utc::now())
                    .await
                {
                    tracing::error!(
                        error_code = "raw_event_ingest_failed",
                        error = %err,
                        "failed to enqueue abstracted event for upload"
                    );
                }
                ServerMessage::RawEventAck(RawEventAck {
                    event_id,
                    status: RawEventStatus::Accepted,
                    drop_reason: None,
                })
            }
            Err(err) => {
                tracing::warn!(
                    error_code = "abstraction_failed",
                    error = %err,
                    "dropped raw event that failed classification"
                );
                ServerMessage::RawEventAck(RawEventAck {
                    event_id,
                    status: RawEventStatus::Dropped,
                    drop_reason: Some("abstraction_failed".into()),
                })
            }
        }
    }
}

fn raw_display_label(app_name: &str, window_title: &str) -> Option<String> {
    let title = window_title.trim();
    if !title.is_empty() {
        Some(title.to_owned())
    } else {
        let app = app_name.trim();
        (!app.is_empty()).then(|| app.to_owned())
    }
}

fn cache_empty(payload_type: &'static str) -> ServerMessage {
    ServerMessage::CacheEmpty(CacheEmpty {
        payload_type: payload_type.to_owned(),
    })
}
