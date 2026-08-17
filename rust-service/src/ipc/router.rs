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
    ClassificationStatus, ClientMessage, CorrectionHistoryPage, InterventionSalience, MenuStatus,
    QueuedEventSummary, RawEventAck, RawEventStatus, RequestLocalDashboard, ServerMessage,
};

use crate::abstraction::AbstractionEngine;
use crate::auth::{
    AccountAuthService, AuthError, AuthState, HttpClient, HttpRequest, SessionValidator, TokenStore,
};
use crate::delivery::{shaper, CacheManager, PushAdapter};
use crate::focus::FocusManager;
use crate::initiation::InitiationManager;
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
                correction_acknowledgment: None,
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
                correction_acknowledgment: None,
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

    /// A polled status must not repeat a confirmation the user already read —
    /// a "Got it" that reappears every minute reads as a bug, not a reply.
    #[tokio::test]
    async fn a_polled_status_carries_no_correction_acknowledgment() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let provider = MenuStatusProvider::new(
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
            Arc::new(FakeTokenStore::default()) as Arc<dyn TokenStore>,
            persistence.upload_batch_repo(),
            persistence.raw_event_repo(),
            persistence.abstraction_map_repo(),
        );

        let status = provider.snapshot().await;

        assert_eq!(status.correction_acknowledgment, None);
    }

    #[test]
    fn a_correction_during_a_block_says_how_long_it_holds() {
        assert_eq!(
            correction_acknowledgment(Some("Research reading"), "REFERENCE", true),
            "Got it — Research reading counts as reference for the rest of this block."
        );
        assert_eq!(
            correction_acknowledgment(Some("Research reading"), "REFERENCE", false),
            "Got it — Research reading counts as reference from now on."
        );
    }

    /// The confirmation never argues with the correction and never mentions
    /// what Velvt thought before: the user is right by definition here.
    #[test]
    fn the_confirmation_is_plain_even_without_a_local_name() {
        let copy = correction_acknowledgment(None, "FOCUS_WORK", true);

        assert_eq!(
            copy,
            "Got it — This activity counts as focus work for the rest of this block."
        );
        for forbidden in ["still", "actually", "instead", "wrong", "but"] {
            assert!(
                !copy.to_ascii_lowercase().contains(forbidden),
                "acknowledgment must not push back: {copy}"
            );
        }
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
    focus: Option<Arc<FocusManager>>,
    initiation: Option<Arc<InitiationManager>>,
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
            focus: None,
            initiation: None,
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

    /// Attaches the Focus/DND evidence owner. Without it, Focus messages are
    /// acknowledged and dropped and no quiet-hours behavior exists.
    pub fn with_focus(mut self, focus: Arc<FocusManager>) -> Self {
        self.focus = Some(focus);
        self
    }

    /// Attaches the deterministic initiation-invitation policy owner.
    /// Without it, invitation messages are acknowledged and dropped and no
    /// invitation behavior exists.
    pub fn with_initiation(mut self, initiation: Arc<InitiationManager>) -> Self {
        self.initiation = Some(initiation);
        self
    }

    /// Whether an event ingested right now may ever be uploaded.
    ///
    /// `RefreshInFlight` is a logged-in state: the device holds a valid refresh
    /// token and is mid-roundtrip. The flag is stamped once at ingest and never
    /// reconsidered, so treating the refresh window as ineligible permanently
    /// excluded every event collected during it — acked to Swift as `Accepted`,
    /// never batched, and invisible in the queued count.
    /// The menu status a correction command returns, carrying a one-shot
    /// confirmation that the correction was taken.
    ///
    /// Invariant 3: corrections are believed instantly *and visibly*. A user who
    /// cannot see their correction land has no reason to make another one, and
    /// the local classifier stops learning.
    async fn menu_status_acknowledging(
        &self,
        activity: Option<&str>,
        category: &str,
    ) -> MenuStatus {
        let during_block = self
            .work_blocks
            .as_ref()
            .and_then(|manager| manager.has_active_block().ok())
            .unwrap_or(false);
        MenuStatus {
            correction_acknowledgment: Some(correction_acknowledgment(
                activity,
                category,
                during_block,
            )),
            ..self.menu_status.snapshot().await
        }
    }

    fn upload_eligible(&self) -> bool {
        self.auth_state.as_ref().is_some_and(|state| {
            matches!(
                *state.borrow(),
                AuthState::Authenticated { .. } | AuthState::RefreshInFlight
            )
        })
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
                // An account switch expires any invitation left over from
                // the previous session.
                self.expire_open_invitation();
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
                // An invitation extended under the departing session must
                // not outlive it (requirement: logout/account switch
                // expires invitation state).
                self.expire_open_invitation();
                self.account.log_out().await;
                Ok(None)
            }

            ClientMessage::DeleteAccount(_) => {
                self.expire_open_invitation();
                Ok(Some(self.account.delete_account().await))
            }

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
                // Generalize the same correction to every window of the app.
                // Without this the correction binds to one (app, title) hash
                // and the next file opened in the same editor is unclassified
                // again, so correcting never converges. Best-effort and
                // deliberately not part of the failure chain below: a
                // correction that took effect for the window the user was
                // looking at must not be reported as failed because it could
                // not also be generalized. Returns false for events that
                // predate app-scoped corrections or are browser windows.
                match abstraction_map.save_personal_app_override(
                    &correction.event_id.to_string(),
                    &correction.category,
                    local_activity_name.as_deref(),
                ) {
                    Ok(generalized) => {
                        tracing::debug!(generalized, "classification correction app-scope outcome")
                    }
                    Err(err) => tracing::warn!(
                        error_code = "app_scoped_correction_failed",
                        error = %err,
                        "correction applied to the window but not generalized to the app"
                    ),
                }
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
                    self.menu_status_acknowledging(
                        local_activity_name.as_deref(),
                        &correction.category,
                    )
                    .await,
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
                    self.menu_status_acknowledging(
                        local_activity_name.as_deref(),
                        &correction.category,
                    )
                    .await,
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
                // The one declaration path. A start command may carry an
                // invitation id; the initiation manager validates the claim
                // and the block records only a content-free origin marker.
                // A manual start expires any live invitation, because an
                // active block suppresses invitations (invariant 1).
                let now = Utc::now();
                let claimed = match (&self.initiation, request.invitation_id) {
                    (Some(initiation), Some(invitation_id)) => initiation
                        .claimable(invitation_id, now)
                        .unwrap_or(false)
                        .then_some(invitation_id),
                    _ => None,
                };
                let origin = if claimed.is_some() {
                    crate::persistence::WorkBlockOrigin::Invitation
                } else {
                    crate::persistence::WorkBlockOrigin::Manual
                };
                let response = self.work_block_response(|manager| {
                    manager.start_with_origin(request, origin, now)
                })?;
                if matches!(response, Some(ServerMessage::WorkBlockState(_))) {
                    if let Some(initiation) = &self.initiation {
                        if initiation.record_block_started(claimed, now).is_err() {
                            tracing::warn!(
                                error_code = "initiation_accept_record_failed",
                                "invitation outcome was not recorded"
                            );
                        }
                    }
                }
                Ok(response)
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
                // The popover requesting state is the calm daytime moment the
                // pattern rule's next-morning offer waits for.
                self.push_pending_quiet_hours_offer().await;
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
                // Focus evidence and offer memory are part of the local
                // behavioral record and clear with it.
                if let Some(focus) = &self.focus {
                    if focus.clear_evidence().is_err() {
                        tracing::warn!(
                            error_code = "focus_evidence_clear_failed",
                            "focus evidence was not cleared"
                        );
                    }
                }
                // The invitation record clears too; the opt-out setting is
                // an explicit user choice and survives.
                if let Some(initiation) = &self.initiation {
                    if initiation.clear_data().is_err() {
                        tracing::warn!(
                            error_code = "initiation_data_clear_failed",
                            "invitation record was not cleared"
                        );
                    }
                }
                self.work_block_response(WorkBlockManager::clear_data)
            }

            ClientMessage::FocusStateChanged(transition) => {
                let Some(focus) = &self.focus else {
                    return Ok(None);
                };
                if focus
                    .record_transition(
                        transition.active,
                        transition.occurred_at,
                        transition.utc_offset_seconds,
                        Utc::now(),
                    )
                    .is_err()
                {
                    tracing::warn!(
                        error_code = "focus_transition_record_failed",
                        "coarse focus transition was not recorded"
                    );
                    return Ok(None);
                }
                self.push_pending_quiet_hours_offer().await;
                Ok(None)
            }

            ClientMessage::RespondQuietHoursOffer(reply) => {
                let Some(focus) = &self.focus else {
                    return Ok(None);
                };
                if focus.respond_to_offer(reply.accepted, Utc::now()).is_err() {
                    tracing::warn!(
                        error_code = "quiet_hours_response_record_failed",
                        "quiet-hours offer response was not recorded"
                    );
                }
                Ok(None)
            }

            ClientMessage::RequestInitiationInvitation(request) => {
                let Some(initiation) = &self.initiation else {
                    return Ok(None);
                };
                match initiation.pending_invitation(Utc::now(), request.utc_offset_seconds) {
                    Ok(Some(invitation)) => {
                        Ok(Some(ServerMessage::InitiationInvitation(invitation)))
                    }
                    Ok(None) => Ok(None),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "initiation_invitation_check_failed",
                            "pending invitation could not be evaluated"
                        );
                        Ok(None)
                    }
                }
            }

            ClientMessage::DismissInitiationInvitation(request) => {
                let Some(initiation) = &self.initiation else {
                    return Ok(None);
                };
                if initiation
                    .dismiss(request.invitation_id, Utc::now())
                    .is_err()
                {
                    tracing::warn!(
                        error_code = "initiation_dismiss_record_failed",
                        "invitation dismissal was not recorded"
                    );
                }
                Ok(None)
            }

            ClientMessage::SetInitiationSettings(request) => {
                let Some(initiation) = &self.initiation else {
                    return Ok(None);
                };
                match initiation.set_enabled(request.invitations_enabled, Utc::now()) {
                    Ok(enabled) => Ok(Some(ServerMessage::InitiationSettings(
                        velvt_shared_types::InitiationSettings {
                            invitations_enabled: enabled,
                        },
                    ))),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "initiation_settings_write_failed",
                            "invitation setting was not persisted"
                        );
                        Ok(None)
                    }
                }
            }

            ClientMessage::RequestInitiationSettings(_) => {
                let Some(initiation) = &self.initiation else {
                    return Ok(None);
                };
                match initiation.enabled() {
                    Ok(enabled) => Ok(Some(ServerMessage::InitiationSettings(
                        velvt_shared_types::InitiationSettings {
                            invitations_enabled: enabled,
                        },
                    ))),
                    Err(_) => Ok(None),
                }
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

/// Confirms a correction in the user's own terms.
///
/// Says what changed and how long it holds, and never argues: the correction is
/// already saved by the time this is written. The activity name is local-only
/// display text that never leaves the device, and the sentence is authored here
/// rather than in Swift so copy stays beside the change it describes.
fn correction_acknowledgment(
    activity: Option<&str>,
    category: &str,
    during_active_block: bool,
) -> String {
    let subject = activity.unwrap_or("This activity");
    let category = category.replace('_', " ").to_ascii_lowercase();
    if during_active_block {
        format!("Got it — {subject} counts as {category} for the rest of this block.")
    } else {
        format!("Got it — {subject} counts as {category} from now on.")
    }
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

    /// Expires the live invitation, if any, at an account boundary. Never
    /// fails the surrounding auth flow.
    fn expire_open_invitation(&self) {
        if let Some(initiation) = &self.initiation {
            if initiation.expire_open(Utc::now()).is_err() {
                tracing::warn!(
                    error_code = "initiation_expire_failed",
                    "live invitation was not expired"
                );
            }
        }
    }

    /// Pushes the deterministic quiet-hours offer when the pattern rule has
    /// one waiting for a local morning. Push-only and repeat-safe: the
    /// manager owns every gate (trigger, morning window, decline memory).
    async fn push_pending_quiet_hours_offer(&self) {
        let (Some(focus), Some(push)) = (&self.focus, &self.work_block_push) else {
            return;
        };
        match focus.pending_morning_offer(Utc::now()) {
            Ok(Some(offer)) => push.push_quiet_hours_offer(offer).await,
            Ok(None) => {}
            Err(_) => tracing::warn!(
                error_code = "quiet_hours_offer_check_failed",
                "pending quiet-hours offer could not be evaluated"
            ),
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
                    app_stable_id: Some(abstracted.app_stable_id().to_owned()),
                    app_scope_eligible: abstracted.app_scope_eligible(),
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
                                // Reduced salience means the in-app card only:
                                // after a negative reply in this block, the
                                // offer never regains the OS notification.
                                // Velvt's own quiet hours only ever reduce
                                // delivery: inside the accepted window the
                                // offer keeps its in-app card but sends no
                                // OS notification, exactly like reduced
                                // salience. (Active system DND never reaches
                                // this point — the manager holds the whole
                                // decision.)
                                if let Some(intervention) = outcome.intervention {
                                    let in_quiet_hours = self.focus.as_ref().is_some_and(|focus| {
                                        focus.in_velvt_quiet_hours(occurred_at)
                                    });
                                    if intervention.salience == InterventionSalience::Normal
                                        && !in_quiet_hours
                                    {
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
