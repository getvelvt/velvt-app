use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Utc;
use uuid::Uuid;
use velvt_shared_types::{
    CacheEmpty, ClassificationConfidence, ClassificationCorrectionSummary, ClassificationSource,
    ClassificationStatus, ClientMessage, CorrectionHistoryPage, MenuStatus, QueuedEventSummary,
    RawEventAck, RawEventStatus, RequestLocalDashboard, ServerMessage,
};

use crate::abstraction::AbstractionEngine;
use crate::auth::{
    AccountAuthService, AuthError, AuthState, HttpClient, HttpRequest, SessionValidator, TokenStore,
};
use crate::delivery::{shaper, CacheManager, PushAdapter};
use crate::persistence::{
    AbstractionMapRepo, RawEventEntry, RawEventRepo, UploadBatchRepo, UploadQueueDiagnostics,
};
use crate::upload::EventIngestor;
use crate::work_block::{WorkBlockError, WorkBlockManager};

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
    abstraction_map: Arc<dyn AbstractionMapRepo>,
    readiness: Mutex<Option<(Instant, bool)>>,
}

impl MenuStatusProvider {
    pub fn new(
        http: Arc<dyn HttpClient>,
        token_store: Arc<dyn TokenStore>,
        batches: Arc<dyn UploadBatchRepo>,
        raw_events: Arc<dyn RawEventRepo>,
        abstraction_map: Arc<dyn AbstractionMapRepo>,
    ) -> Self {
        Self {
            http,
            token_store,
            batches,
            raw_events,
            abstraction_map,
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
            let local_metadata = self
                .raw_events
                .local_event_metadata(&event_ids)
                .unwrap_or_default();
            let queued_events = events
                .into_iter()
                .take(10)
                .filter_map(|event| {
                    let event_id = Uuid::parse_str(&event.event_id).ok()?;
                    let metadata = local_metadata.get(&event.event_id);
                    Some(QueuedEventSummary {
                        event_id,
                        stable_id: event.stable_id,
                        label: event.label,
                        local_label: metadata.and_then(|value| value.local_display_label.clone()),
                        category: event.category,
                        classification_tier: event.classification_tier,
                        classification_status: parse_classification_status(
                            metadata.map(|value| value.classification_status.as_str()),
                        ),
                        classification_confidence: parse_classification_confidence(
                            metadata.map(|value| value.classification_confidence.as_str()),
                        ),
                        classification_source: parse_classification_source(
                            metadata.map(|value| value.classification_source.as_str()),
                        ),
                        occurred_at: event.occurred_at,
                    })
                })
                .collect::<Vec<_>>();
            let unbatched = self.raw_events.unbatched_events(10).unwrap_or_default();
            let queued_event_count = queued_event_count + unbatched.len() as u64;
            let queued_events = queued_events
                .into_iter()
                .chain(unbatched.into_iter().filter_map(|event| {
                    let event_id = Uuid::parse_str(&event.event_id).ok()?;
                    Some(QueuedEventSummary {
                        event_id,
                        stable_id: event.stable_id,
                        label: event.label,
                        local_label: event.local_display_label,
                        category: event.category,
                        classification_tier: event.classification_tier,
                        classification_status: parse_classification_status(Some(
                            &event.classification_status,
                        )),
                        classification_confidence: parse_classification_confidence(Some(
                            &event.classification_confidence,
                        )),
                        classification_source: parse_classification_source(Some(
                            &event.classification_source,
                        )),
                        occurred_at: event.occurred_at,
                    })
                }))
                .take(10)
                .collect();
            let diagnostics = self.batches.queue_diagnostics().unwrap_or_else(|error| {
                tracing::warn!(
                    error_code = "upload_queue_diagnostics_failed",
                    error = %error,
                    "failed to read upload queue diagnostics"
                );
                UploadQueueDiagnostics {
                    pending_batch_count: 0,
                    failed_batch_count: 0,
                    rejected_batch_count: 0,
                    next_attempt_at: None,
                    last_error_code: None,
                    last_successful_sync_at: None,
                }
            });
            let upload_status = upload_status_for(cloud_ready, &diagnostics).to_owned();
            let correction_history = self
                .abstraction_map
                .personal_overrides(25)
                .unwrap_or_default()
                .into_iter()
                .map(|correction| ClassificationCorrectionSummary {
                    stable_id: correction.stable_id,
                    label: correction.label,
                    local_label: correction.local_activity_name,
                    category: correction.category,
                    updated_at: correction.updated_at,
                })
                .collect();
            MenuStatus {
                device_id: self.token_store.load_device_id().unwrap_or_default(),
                cloud_ready,
                upload_status,
                last_upload_error_code: diagnostics.last_error_code,
                next_upload_attempt_at: diagnostics.next_attempt_at,
                last_successful_sync_at: diagnostics.last_successful_sync_at,
                pending_upload_batch_count: diagnostics.pending_batch_count,
                failed_upload_batch_count: diagnostics.failed_batch_count,
                rejected_upload_batch_count: diagnostics.rejected_batch_count,
                queued_event_count,
                queued_events,
                correction_history,
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
                upload_status: "network_unavailable".into(),
                last_upload_error_code: None,
                next_upload_attempt_at: None,
                last_successful_sync_at: None,
                pending_upload_batch_count: 0,
                failed_upload_batch_count: 0,
                rejected_upload_batch_count: 0,
                queued_event_count: 0,
                queued_events: vec![],
                correction_history: vec![],
            }
        })
    }
}

fn upload_status_for(cloud_ready: bool, diagnostics: &UploadQueueDiagnostics) -> &'static str {
    match diagnostics.last_error_code.as_deref() {
        Some("authentication_required") => "auth_required",
        Some("raw_field_rejected") => "privacy_rejected",
        Some("rate_limited") => "rate_limited",
        _ if !cloud_ready => "network_unavailable",
        _ if diagnostics.failed_batch_count > 0 => "retrying",
        _ if diagnostics.pending_batch_count > 0 => "pending",
        _ if diagnostics.rejected_batch_count > 0 => "privacy_rejected",
        _ => "ready",
    }
}

fn normalized_local_activity_name(value: Option<&str>) -> Result<Option<String>, ()> {
    let Some(value) = value else { return Ok(None) };
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 48 || trimmed.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(trimmed.to_owned()))
}

fn normalized_correction_query(value: Option<&str>) -> Result<Option<String>, ()> {
    let Some(value) = value else { return Ok(None) };
    let trimmed = value.trim();
    if trimmed.chars().count() > 64 || trimmed.chars().any(char::is_control) {
        return Err(());
    }
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

fn correction_summary(
    correction: crate::persistence::PersonalOverrideRecord,
) -> ClassificationCorrectionSummary {
    ClassificationCorrectionSummary {
        stable_id: correction.stable_id,
        label: correction.label,
        local_label: correction.local_activity_name,
        category: correction.category,
        updated_at: correction.updated_at,
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
            persistence.abstraction_map_repo(),
        );

        token_store.store_device_id("device-1").unwrap();

        let status = provider.snapshot().await;

        assert_eq!(status.device_id.as_deref(), Some("device-1"));
    }

    #[test]
    fn local_activity_names_are_trimmed_and_privacy_bounded() {
        assert_eq!(
            normalized_local_activity_name(Some("  Research reading  ")),
            Ok(Some("Research reading".into()))
        );
        assert_eq!(normalized_local_activity_name(None), Ok(None));
        assert!(normalized_local_activity_name(Some("")).is_err());
        assert!(normalized_local_activity_name(Some("private\nwindow")).is_err());
        assert!(normalized_local_activity_name(Some(&"x".repeat(49))).is_err());
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
    abstraction_map: Option<Arc<dyn AbstractionMapRepo>>,
    correction_http: Option<Arc<dyn HttpClient>>,
    upload_batches: Option<Arc<dyn UploadBatchRepo>>,
    work_blocks: Option<Arc<WorkBlockManager>>,
    work_block_push: Option<Arc<PushAdapter>>,
    auth_state: Option<tokio::sync::watch::Receiver<AuthState>>,
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
            abstraction_map: None,
            correction_http: None,
            upload_batches: None,
            work_blocks: None,
            work_block_push: None,
            auth_state: None,
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

    pub fn with_classification_corrections(
        mut self,
        abstraction_map: Arc<dyn AbstractionMapRepo>,
        upload_batches: Arc<dyn UploadBatchRepo>,
        correction_http: Arc<dyn HttpClient>,
    ) -> Self {
        self.abstraction_map = Some(abstraction_map);
        self.upload_batches = Some(upload_batches);
        self.correction_http = Some(correction_http);
        self
    }

    pub fn with_work_blocks(
        mut self,
        work_blocks: Arc<WorkBlockManager>,
        push: Arc<PushAdapter>,
    ) -> Self {
        self.work_blocks = Some(work_blocks);
        self.work_block_push = Some(push);
        self
    }

    pub fn with_auth_state(mut self, auth_state: tokio::sync::watch::Receiver<AuthState>) -> Self {
        self.auth_state = Some(auth_state);
        self
    }

    fn upload_eligible(&self) -> bool {
        self.auth_state
            .as_ref()
            .is_some_and(|state| matches!(*state.borrow(), AuthState::Authenticated { .. }))
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

            ClientMessage::RequestCorrectionHistory(request) => {
                let Some(abstraction_map) = &self.abstraction_map else {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_unavailable",
                    )));
                };
                let query = match normalized_correction_query(request.query.as_deref()) {
                    Ok(value) => value,
                    Err(()) => {
                        return Ok(Some(classification_correction_error(
                            "invalid_correction_history_query",
                        )));
                    }
                };
                let page_size = request.page_size.clamp(1, 20);
                let (items, total_count) = match abstraction_map.search_personal_overrides(
                    query.as_deref(),
                    request.offset as usize,
                    page_size as usize,
                ) {
                    Ok(result) => result,
                    Err(_) => {
                        return Ok(Some(classification_correction_error(
                            "classification_correction_history_failed",
                        )));
                    }
                };
                let returned = items.len() as u64;
                Ok(Some(ServerMessage::CorrectionHistoryPage(
                    CorrectionHistoryPage {
                        items: items.into_iter().map(correction_summary).collect(),
                        offset: request.offset,
                        page_size,
                        total_count,
                        has_more: u64::from(request.offset) + returned < total_count,
                    },
                )))
            }

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

            ClientMessage::CorrectEventClassification(correction) => {
                let Some(label) =
                    crate::abstraction::override_label_for_category(&correction.category)
                else {
                    return Ok(Some(classification_correction_error(
                        "invalid_classification_category",
                    )));
                };
                let (Some(abstraction_map), Some(upload_batches), Some(correction_http)) = (
                    &self.abstraction_map,
                    &self.upload_batches,
                    &self.correction_http,
                ) else {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_unavailable",
                    )));
                };
                let local_activity_name =
                    match normalized_local_activity_name(correction.local_activity_name.as_deref())
                    {
                        Ok(value) => value,
                        Err(()) => {
                            return Ok(Some(classification_correction_error(
                                "invalid_local_activity_name",
                            )));
                        }
                    };
                if abstraction_map
                    .save_personal_override(
                        &correction.stable_id,
                        &correction.category,
                        local_activity_name.as_deref(),
                    )
                    .and_then(|_| {
                        self.raw_event_repo.update_classification(
                            &correction.event_id.to_string(),
                            label,
                            &correction.category,
                            local_activity_name.as_deref(),
                        )
                    })
                    .and_then(|_| {
                        upload_batches.update_event_classification(
                            &correction.event_id.to_string(),
                            label,
                            &correction.category,
                        )
                    })
                    .is_err()
                {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_persistence_failed",
                    )));
                }

                if self.upload_eligible() {
                    match correction_http
                        .send(HttpRequest::patch(
                            format!("/v1/events/{}/classification", correction.event_id),
                            serde_json::json!({ "category": correction.category }),
                        ))
                        .await
                    {
                        Ok(response) if response.status / 100 == 2 || response.status == 404 => {}
                        Ok(response) => tracing::warn!(
                            status = response.status,
                            error_code = "classification_correction_sync_failed",
                            "local classification correction saved but cloud sync failed"
                        ),
                        Err(error) => tracing::warn!(
                            error = %error,
                            error_code = "classification_correction_sync_failed",
                            "local classification correction saved but cloud sync was deferred"
                        ),
                    }
                }
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status.snapshot().await,
                )))
            }

            ClientMessage::UpdateClassificationOverride(correction) => {
                if crate::abstraction::override_label_for_category(&correction.category).is_none() {
                    return Ok(Some(classification_correction_error(
                        "invalid_classification_category",
                    )));
                }
                let Some(abstraction_map) = &self.abstraction_map else {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_unavailable",
                    )));
                };
                let local_activity_name =
                    match normalized_local_activity_name(correction.local_activity_name.as_deref())
                    {
                        Ok(value) => value,
                        Err(()) => {
                            return Ok(Some(classification_correction_error(
                                "invalid_local_activity_name",
                            )));
                        }
                    };
                if abstraction_map
                    .save_personal_override(
                        &correction.stable_id,
                        &correction.category,
                        local_activity_name.as_deref(),
                    )
                    .is_err()
                {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_persistence_failed",
                    )));
                }
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status.snapshot().await,
                )))
            }

            ClientMessage::RemoveClassificationOverride(request) => {
                let Some(abstraction_map) = &self.abstraction_map else {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_unavailable",
                    )));
                };
                if abstraction_map
                    .remove_personal_override(&request.stable_id)
                    .is_err()
                {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_persistence_failed",
                    )));
                }
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status.snapshot().await,
                )))
            }

            ClientMessage::ResetClassificationOverrides(_) => {
                let Some(abstraction_map) = &self.abstraction_map else {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_unavailable",
                    )));
                };
                if abstraction_map.reset_personal_overrides().is_err() {
                    return Ok(Some(classification_correction_error(
                        "classification_correction_persistence_failed",
                    )));
                }
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status.snapshot().await,
                )))
            }

            ClientMessage::StartWorkBlock(request) => {
                self.work_block_response(|manager| manager.start(request, Utc::now()))
            }

            ClientMessage::PauseWorkBlock(request) => {
                self.work_block_response(|manager| manager.pause(request.block_id, Utc::now()))
            }

            ClientMessage::ResumeWorkBlock(request) => {
                self.work_block_response(|manager| manager.resume(request.block_id, Utc::now()))
            }

            ClientMessage::EndWorkBlock(request) => {
                self.work_block_response(|manager| manager.end(request.block_id, Utc::now()))
            }

            ClientMessage::RequestWorkBlockState(_) => {
                self.work_block_response(|manager| manager.request_state(Utc::now()))
            }

            ClientMessage::RequestLocalDashboard(request) => self.local_dashboard_response(request),

            ClientMessage::AcceptWorkBlockRecovery(request) => {
                self.work_block_response(|manager| {
                    manager.accept_recovery(request.block_id, &request.action_id, Utc::now())
                })
            }

            ClientMessage::ReportInterventionOutcome(request) => {
                self.work_block_response(|manager| {
                    manager.report_intervention_outcome(
                        request.block_id,
                        request.response,
                        Utc::now(),
                    )
                })
            }

            ClientMessage::WorkBlockLifecycle(request) => {
                self.work_block_response(|manager| manager.lifecycle(request.event, Utc::now()))
            }

            ClientMessage::ClearWorkBlockData(_) => {
                self.work_block_response(WorkBlockManager::clear_data)
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
                            cache_empty("insight_payload", "invalid_cached_payload")
                        }
                    },
                    Ok(None) => cache_empty("insight_payload", "insufficient_evidence"),
                    Err(err) => {
                        tracing::warn!(
                            date = %req.date,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read insight from cache"
                        );
                        cache_empty("insight_payload", "backend_unavailable")
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
                            cache_empty("history_payload", "invalid_cached_payload")
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            days = req.days,
                            error_code = "cache_read_failed",
                            error = %err,
                            "failed to read history from cache"
                        );
                        cache_empty("history_payload", "backend_unavailable")
                    }
                };
                Ok(Some(response))
            }

            _ => Ok(None),
        }
    }
}

fn classification_correction_error(code: &str) -> ServerMessage {
    ServerMessage::ErrorResponse(velvt_shared_types::ErrorResponse {
        code: code.to_owned(),
        message: "Unable to save this classification. Try again later.".into(),
        related_event_id: None,
    })
}

fn parse_classification_status(value: Option<&str>) -> ClassificationStatus {
    match value {
        Some("classified") => ClassificationStatus::Classified,
        Some("ambiguous") => ClassificationStatus::Ambiguous,
        _ => ClassificationStatus::Unclassified,
    }
}

fn parse_classification_confidence(value: Option<&str>) -> ClassificationConfidence {
    match value {
        Some("high") => ClassificationConfidence::High,
        Some("medium") => ClassificationConfidence::Medium,
        Some("low") => ClassificationConfidence::Low,
        _ => ClassificationConfidence::None,
    }
}

fn parse_classification_source(value: Option<&str>) -> ClassificationSource {
    match value {
        Some("seed") => ClassificationSource::Seed,
        Some("heuristic") => ClassificationSource::Heuristic,
        Some("embedding") => ClassificationSource::Embedding,
        Some("user_rule") => ClassificationSource::UserRule,
        _ => ClassificationSource::Fallback,
    }
}

impl R7Router {
    fn local_dashboard_response(
        &self,
        request: RequestLocalDashboard,
    ) -> Result<Option<ServerMessage>, IpcError> {
        let now = Utc::now();
        let work_block = self
            .work_blocks
            .as_ref()
            .and_then(|manager| manager.request_state(now).ok());
        let snapshot = match crate::dashboard::snapshot(
            &*self.raw_event_repo,
            work_block.as_ref(),
            now,
            request.window_seconds,
            request.utc_offset_seconds,
        ) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return Ok(Some(ServerMessage::ErrorResponse(
                    velvt_shared_types::ErrorResponse {
                        code: "local_dashboard_unavailable".into(),
                        message: "Local dashboard data is temporarily unavailable.".into(),
                        related_event_id: None,
                    },
                )))
            }
        };
        match shaper::shape_local_dashboard(snapshot) {
            Ok(validated) => Ok(Some(ServerMessage::LocalDashboard(validated.into_inner()))),
            Err(err) => {
                tracing::warn!(
                    message_type = "local_dashboard",
                    error_code = "outbound_validation_failed",
                    error = %err,
                    "local dashboard payload failed validation"
                );
                Ok(Some(ServerMessage::ErrorResponse(
                    velvt_shared_types::ErrorResponse {
                        code: "local_dashboard_unavailable".into(),
                        message: "Local dashboard data is temporarily unavailable.".into(),
                        related_event_id: None,
                    },
                )))
            }
        }
    }

    fn work_block_response(
        &self,
        operation: impl FnOnce(
            &WorkBlockManager,
        ) -> Result<velvt_shared_types::WorkBlockSnapshot, WorkBlockError>,
    ) -> Result<Option<ServerMessage>, IpcError> {
        let Some(manager) = &self.work_blocks else {
            return Ok(Some(work_block_error("work_block_unavailable")));
        };
        Ok(Some(match operation(manager) {
            Ok(snapshot) => ServerMessage::WorkBlockState(snapshot),
            Err(WorkBlockError::InvalidRequest) => work_block_error("invalid_work_block_request"),
            Err(WorkBlockError::InvalidTransition) => {
                work_block_error("invalid_work_block_transition")
            }
            Err(WorkBlockError::Persistence(_)) => {
                tracing::error!(
                    error_code = "work_block_persistence_failed",
                    "local work-block operation failed"
                );
                work_block_error("work_block_persistence_failed")
            }
        }))
    }

    /// Runs the privacy-enforcement boundary: classify, persist a privacy-safe
    /// audit row, feed the upload batcher, and acknowledge. Raw `app_name`/
    /// `window_title` are consumed only by `abstraction_engine.process` and
    /// never appear in `RawEventEntry`, `BatchEventPayload`, or this ack.
    async fn handle_raw_event(&self, event: velvt_shared_types::RawEvent) -> ServerMessage {
        let event_id = event.event_id;
        let occurred_at = event.occurred_at;
        let duration_seconds = event.duration_seconds.min(30 * 60);
        let upload_eligible = self.upload_eligible();
        match self.abstraction_engine.process(event) {
            Ok(abstracted) => {
                let entry = RawEventEntry {
                    event_id: event_id.to_string(),
                    stable_id: abstracted.stable_id().to_owned(),
                    label: abstracted.label().to_owned(),
                    local_display_label: abstracted.local_display_label().map(str::to_owned),
                    local_name_suggestion: abstracted.local_name_suggestion().map(str::to_owned),
                    category: abstracted.category().to_owned(),
                    taxonomy_version: abstracted.taxonomy_version().to_owned(),
                    classification_tier: abstracted.classification_tier().as_str().to_owned(),
                    classification_status: abstracted.classification_status().as_str().to_owned(),
                    classification_confidence: abstracted
                        .classification_confidence()
                        .as_str()
                        .to_owned(),
                    classification_source: abstracted.classification_source().as_str().to_owned(),
                    occurred_at,
                    duration_seconds,
                    upload_eligible,
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
                if upload_eligible {
                    if let Err(err) = self
                        .ingestor
                        .ingest(
                            event_id.to_string(),
                            &abstracted,
                            duration_seconds,
                            Utc::now(),
                        )
                        .await
                    {
                        tracing::error!(
                            error_code = "raw_event_ingest_failed",
                            error = %err,
                            "failed to enqueue abstracted event for upload"
                        );
                    }
                }
                if let Some(work_blocks) = &self.work_blocks {
                    match work_blocks.observe_safe_category(
                        abstracted.category(),
                        abstracted.classification_status(),
                        abstracted.classification_confidence(),
                        occurred_at,
                    ) {
                        Ok(Some(outcome)) => {
                            if let Some(push) = &self.work_block_push {
                                push.push_work_block_state(outcome.snapshot).await;
                                // Delivered on the same local path as the daily
                                // insight, but authored entirely on-device: an
                                // in-session offer never waits on the cloud or
                                // on a mature baseline.
                                if let Some(intervention) = outcome.intervention {
                                    push.push_notification(
                                        Uuid::new_v4(),
                                        &intervention.title,
                                        &intervention.body,
                                        occurred_at.date_naive(),
                                    )
                                    .await;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => tracing::warn!(
                            error_code = "work_block_observation_failed",
                            "safe work-block observation was not recorded"
                        ),
                    }
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

fn work_block_error(code: &str) -> ServerMessage {
    ServerMessage::ErrorResponse(velvt_shared_types::ErrorResponse {
        code: code.to_owned(),
        message: "Unable to update this local work block. Try again.".into(),
        related_event_id: None,
    })
}

fn cache_empty(payload_type: &'static str, reason: &'static str) -> ServerMessage {
    ServerMessage::CacheEmpty(CacheEmpty {
        payload_type: payload_type.to_owned(),
        reason: Some(reason.to_owned()),
    })
}
