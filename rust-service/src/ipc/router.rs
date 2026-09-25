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
    QueuedEventSummary, RawEventAck, RawEventMetadataError, RawEventStatus, RequestLocalDashboard,
    ServerMessage, SetApplicationCategory, UnclassifiedTriage, UnclassifiedTriageEntry,
};

use crate::abstraction::AbstractionEngine;
use crate::auth::{
    AccountAuthService, AuthError, AuthState, HttpClient, HttpRequest, SessionValidator, TokenStore,
};
use crate::delivery::{shaper, CacheManager, PushAdapter};
use crate::focus::FocusManager;
use crate::initiation::InitiationManager;
use crate::persistence::{
    AbstractionMapRepo, DeclaredAppMetadata, PersistenceError, RawEventEntry, RawEventRepo,
    UnclassifiedAppEntry, UploadBatchRepo, UploadQueueDiagnostics, MAX_REPORTED_DWELL_SECONDS,
    TRIAGE_MAX_ENTRIES, TRIAGE_MAX_LOOKBACK_DAYS, TRIAGE_MIN_SECONDS,
};
use crate::receipts::ReceiptsManager;
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
                    scope: correction.scope,
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

/// Accepts an application key only in the shape Velvt itself issues.
///
/// Every app key Velvt hands out is an HMAC-SHA-256 digest rendered as 64
/// lowercase hex characters (`key.rs`), and the client's only source for one is
/// the triage list it is answering. Anything else is a defect or a forgery, and accepting
/// it would write a rule under a key no event can ever match — invisible in the
/// history's app rules, unreachable by removal, and impossible to explain.
fn normalized_app_stable_id(value: &str) -> Option<&str> {
    (value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(value)
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
        // The scope travels with the rule: `stable_id` means an abstraction
        // stable id for a window rule and an application key hash for an app
        // rule, so the client cannot act on the id without it.
        scope: correction.scope,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abstraction::{app_bundle_key_for, app_stable_key_for, StableKeySalt};
    use crate::auth::{FakeTokenStore, HttpResponse, TokenStore};
    use crate::persistence::SqlitePersistence;
    use std::future::Future;

    /// The app key for `app_name` as this database computes it: under its own
    /// salt (migration 0037), which is the only salt its rows can match.
    fn app_key(persistence: &SqlitePersistence, app_name: &str) -> String {
        let salt = persistence
            .abstraction_map_repo()
            .stable_key_salt()
            .unwrap();
        app_stable_key_for(&salt, app_name)
    }

    /// The bundle key for `bundle_id`, under the same salt.
    fn bundle_key(persistence: &SqlitePersistence, bundle_id: &str) -> String {
        let salt = persistence
            .abstraction_map_repo()
            .stable_key_salt()
            .unwrap();
        app_bundle_key_for(&salt, bundle_id)
    }

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

    /// An application key is only ever something Velvt issued: 64 lowercase hex
    /// characters. A rule written under anything else could never be matched,
    /// seen, or removed.
    #[test]
    fn an_application_key_is_only_accepted_in_the_shape_velvt_issues() {
        let key = app_stable_key_for(&StableKeySalt::from_bytes([1; 32]), "Qwybex");

        assert_eq!(normalized_app_stable_id(&key), Some(key.as_str()));
        assert_eq!(normalized_app_stable_id(""), None);
        assert_eq!(normalized_app_stable_id("not-a-key"), None);
        // Uppercase hex is not what `encode_hex` emits, so it is not a key
        // Velvt issued and it would not match one either.
        assert_eq!(normalized_app_stable_id(&key.to_ascii_uppercase()), None);
        assert_eq!(normalized_app_stable_id(&format!("{key}0")), None);
    }

    /// Undo, reset and per-app teaching all answer in the same plain voice as a
    /// correction: what is true now, and nothing else.
    #[test]
    fn the_new_acknowledgments_state_a_fact_and_nothing_more() {
        let sentences = [
            removal_acknowledgment(true),
            removal_acknowledgment(false),
            reset_acknowledgment(),
            application_acknowledgment(Some("Qwybex"), "FOCUS_WORK"),
        ];

        assert_eq!(
            application_acknowledgment(Some("Qwybex"), "FOCUS_WORK"),
            "Got it — Qwybex counts as focus work from now on."
        );
        assert_eq!(
            application_acknowledgment(None, "REFERENCE"),
            "Got it — This app counts as reference from now on."
        );
        for sentence in sentences {
            for forbidden in [
                "streak",
                "great",
                "nice",
                "well done",
                "keep it up",
                "wasted",
                "again today",
            ] {
                assert!(
                    !sentence.to_ascii_lowercase().contains(forbidden),
                    "acknowledgment must not praise or scold: {sentence}"
                );
            }
        }
    }

    /// An ingestor for the correction and triage paths, which never upload:
    /// without an auth state the router is not upload-eligible, so nothing here
    /// is called and a real batcher would only add moving parts.
    struct NoopIngestor;

    impl crate::upload::EventIngestor for NoopIngestor {
        fn ingest<'a>(
            &'a self,
            _event_id: String,
            _event: &'a crate::abstraction::AbstractedEvent,
            _duration_seconds: u64,
            _now: chrono::DateTime<Utc>,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::upload::CoordinatorError>> + Send + 'a>>
        {
            Box::pin(async { Ok(()) })
        }

        fn flush_due<'a>(
            &'a self,
            _now: chrono::DateTime<Utc>,
        ) -> Pin<Box<dyn Future<Output = Result<bool, crate::upload::CoordinatorError>> + Send + 'a>>
        {
            Box::pin(async { Ok(false) })
        }

        fn flush_shutdown<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<bool, crate::upload::CoordinatorError>> + Send + 'a>>
        {
            Box::pin(async { Ok(false) })
        }

        fn flush_now<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<bool, crate::upload::CoordinatorError>> + Send + 'a>>
        {
            Box::pin(async { Ok(false) })
        }
    }

    /// The router wired for corrections and triage: the real engine over the
    /// real store, so an event routed here lands the same rows production
    /// would write and the rules under test are read back out of SQLite.
    fn correction_router(persistence: &SqlitePersistence) -> R7Router {
        let engine = crate::abstraction::AbstractionEngine::from_builtin_taxonomy(
            persistence.abstraction_mapping_store(),
        )
        .unwrap();
        let account = Arc::new(AccountAuthService::new(
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
            Arc::new(FakeTokenStore::default()) as Arc<dyn TokenStore>,
            Arc::new(crate::auth::AuthStateMachine::new(
                crate::auth::AuthState::Unauthenticated,
            )),
        ));
        R7Router::new(
            Arc::new(crate::delivery::FakeCacheManager::new()),
            Arc::new(engine),
            persistence.raw_event_repo(),
            Arc::new(NoopIngestor),
            account,
        )
        .with_menu_status(Arc::new(MenuStatusProvider::new(
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
            Arc::new(FakeTokenStore::default()) as Arc<dyn TokenStore>,
            persistence.upload_batch_repo(),
            persistence.raw_event_repo(),
            persistence.abstraction_map_repo(),
        )))
        .with_classification_corrections(
            persistence.abstraction_map_repo(),
            persistence.upload_batch_repo(),
            Arc::new(ReadyHttp) as Arc<dyn HttpClient>,
        )
    }

    fn raw_event(
        app_name: &str,
        window_title: &str,
        bundle_id: Option<&str>,
        duration_seconds: u64,
    ) -> ClientMessage {
        ClientMessage::RawEvent(velvt_shared_types::RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            duration_seconds,
            app_name: app_name.to_owned(),
            window_title: window_title.to_owned(),
            bundle_id: bundle_id.map(str::to_owned),
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: None,
        })
    }

    async fn menu_status(router: &R7Router, message: ClientMessage) -> MenuStatus {
        match router.route(message).await.unwrap() {
            Some(ServerMessage::MenuStatus(status)) => status,
            other => panic!("expected a menu status, got {other:?}"),
        }
    }

    async fn triage(
        router: &R7Router,
        lookback_days: u32,
    ) -> velvt_shared_types::UnclassifiedTriage {
        let message = ClientMessage::RequestUnclassifiedTriage(
            velvt_shared_types::RequestUnclassifiedTriage { lookback_days },
        );
        match router.route(message).await.unwrap() {
            Some(ServerMessage::UnclassifiedTriage(triage)) => triage,
            other => panic!("expected a triage list, got {other:?}"),
        }
    }

    /// The mapping id the client holds for a rule, read back out of the event
    /// the router just wrote. It is not the key hash: `abstraction_map` issues
    /// an opaque id per key, and that id is what every correction message
    /// carries.
    fn only_stable_id(persistence: &SqlitePersistence) -> String {
        let now = Utc::now();
        let mut events = persistence
            .raw_event_repo()
            .events_between(
                now - chrono::Duration::hours(1),
                now + chrono::Duration::hours(1),
                10,
            )
            .unwrap();
        assert_eq!(events.len(), 1, "expected exactly one recorded event");
        events.remove(0).stable_id
    }

    fn update(stable_id: &str, category: &str) -> ClientMessage {
        ClientMessage::UpdateClassificationOverride(
            velvt_shared_types::UpdateClassificationOverride {
                stable_id: stable_id.to_owned(),
                category: category.to_owned(),
                local_activity_name: None,
            },
        )
    }

    /// The bug the app rung was invisible for: editing a saved rule wrote only
    /// the window rung, so every other window of the same application kept
    /// answering with the category the user had just replaced.
    #[tokio::test]
    async fn editing_a_saved_rule_moves_the_app_rule_with_it() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let rules = persistence.abstraction_map_repo();
        router
            .route(raw_event(
                "Qwybex",
                "Zarniwoop",
                Some("com.example.qwybex"),
                600,
            ))
            .await
            .unwrap();
        let stable_id = only_stable_id(&persistence);
        let app_key = app_key(&persistence, "Qwybex");

        menu_status(&router, update(&stable_id, "FOCUS_WORK")).await;
        let first = rules.app_scope_override(&app_key).unwrap().unwrap();
        menu_status(&router, update(&stable_id, "REFERENCE")).await;
        let second = rules.app_scope_override(&app_key).unwrap().unwrap();

        assert_eq!(first.category, "FOCUS_WORK");
        assert_eq!(second.category, "REFERENCE");
        // Written from the event row, so the rule also answers under the
        // identity that survives a rename.
        assert_eq!(
            second.bundle_key_hash.as_deref(),
            Some(bundle_key(&persistence, "com.example.qwybex").as_str())
        );
    }

    /// The history now lists app rules, so the client can send an edit for one.
    /// `save_personal_override` answers `NotFound` for an application key, so
    /// without the scope check that edit failed outright.
    #[tokio::test]
    async fn an_app_rule_can_be_edited_from_the_history() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let rules = persistence.abstraction_map_repo();
        let app_key = app_key(&persistence, "Qwybex");
        rules
            .save_app_scope_override(
                &app_key,
                Some(&bundle_key(&persistence, "com.example.qwybex")),
                "REFERENCE",
                Some("Qwybex"),
            )
            .unwrap();

        let status = menu_status(&router, update(&app_key, "FOCUS_WORK")).await;

        let rule = rules.app_scope_override(&app_key).unwrap().unwrap();
        assert_eq!(rule.category, "FOCUS_WORK");
        // The edit keeps the identity the rule was taught with.
        assert_eq!(
            rule.bundle_key_hash.as_deref(),
            Some(bundle_key(&persistence, "com.example.qwybex").as_str())
        );
        assert!(status.correction_acknowledgment.is_some());
    }

    /// A rule the user cannot see is a rule they cannot undo.
    #[tokio::test]
    async fn the_correction_history_shows_app_rules_with_their_scope() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let app_key = app_key(&persistence, "Qwybex");
        persistence
            .abstraction_map_repo()
            .save_app_scope_override(&app_key, None, "FOCUS_WORK", Some("Qwybex"))
            .unwrap();

        let page = match router
            .route(ClientMessage::RequestCorrectionHistory(
                velvt_shared_types::RequestCorrectionHistory {
                    query: None,
                    offset: 0,
                    page_size: 20,
                },
            ))
            .await
            .unwrap()
        {
            Some(ServerMessage::CorrectionHistoryPage(page)) => page,
            other => panic!("expected a correction history page, got {other:?}"),
        };

        let rule = page
            .items
            .iter()
            .find(|item| item.stable_id == app_key)
            .expect("the app rule must be listed");
        assert_eq!(rule.scope, velvt_shared_types::CorrectionScope::App);
        assert_eq!(rule.category, "FOCUS_WORK");
    }

    /// Removing an app rule, which nothing could reach before: the history had
    /// no app rules in it, and removing the window rule left the engine falling
    /// through into the surviving app rule and answering exactly as before.
    #[tokio::test]
    async fn an_app_rule_can_be_removed_and_the_removal_is_acknowledged() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let rules = persistence.abstraction_map_repo();
        let app_key = app_key(&persistence, "Qwybex");
        rules
            .save_app_scope_override(&app_key, None, "FOCUS_WORK", Some("Qwybex"))
            .unwrap();
        let remove = || {
            ClientMessage::RemoveClassificationOverride(
                velvt_shared_types::RemoveClassificationOverride {
                    stable_id: app_key.clone(),
                },
            )
        };

        let removed = menu_status(&router, remove()).await;
        let again = menu_status(&router, remove()).await;

        assert!(rules.app_scope_override(&app_key).unwrap().is_none());
        assert_eq!(
            removed.correction_acknowledgment.as_deref(),
            Some("Removed — Velvt classifies this on its own again.")
        );
        // A rule that was already gone is not reported as removed a second
        // time: the confirmation says what is true, not what was asked for.
        assert_eq!(
            again.correction_acknowledgment.as_deref(),
            Some("Nothing to remove — Velvt is already classifying this on its own.")
        );
    }

    /// Reset answered with a bare status, so the only evidence it had run was a
    /// list emptying somewhere the user might not have been looking.
    #[tokio::test]
    async fn resetting_every_rule_is_acknowledged() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let rules = persistence.abstraction_map_repo();
        let app_key = app_key(&persistence, "Qwybex");
        rules
            .save_app_scope_override(&app_key, None, "FOCUS_WORK", Some("Qwybex"))
            .unwrap();

        let status = menu_status(
            &router,
            ClientMessage::ResetClassificationOverrides(
                velvt_shared_types::ResetClassificationOverrides {},
            ),
        )
        .await;

        assert_eq!(
            status.correction_acknowledgment.as_deref(),
            Some("Removed every saved rule — Velvt classifies everything on its own again.")
        );
        assert!(rules.app_scope_override(&app_key).unwrap().is_none());
    }

    /// The whole point of the triage surface: teaching happens once per app,
    /// and the app leaves the list the moment it is taught.
    #[tokio::test]
    async fn teaching_an_app_from_the_triage_list_takes_it_off_the_list() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        router
            .route(raw_event(
                "Qwybex",
                "Zarniwoop",
                Some("com.example.qwybex"),
                600,
            ))
            .await
            .unwrap();

        let listed = triage(&router, 30).await;
        let entry = listed.entries.first().expect("one unreadable app").clone();
        let status = menu_status(
            &router,
            ClientMessage::SetApplicationCategory(velvt_shared_types::SetApplicationCategory {
                app_stable_id: entry.app_stable_id.clone(),
                category: "FOCUS_WORK".into(),
                activity_name: Some("Qwybex".into()),
            }),
        )
        .await;
        let after = triage(&router, 30).await;

        // A request past the retention window reports the window actually used.
        assert_eq!(listed.window_days, 14);
        assert_eq!(listed.entries.len(), 1);
        assert_eq!(entry.app_stable_id, app_key(&persistence, "Qwybex"));
        assert_eq!(entry.display_name, "Qwybex");
        assert_eq!(entry.seconds_observed, 600);
        assert_eq!(entry.event_count, 1);
        // No bundle identity on the wire in either form: the raw identifier is
        // consumed at the abstraction boundary, and the key it becomes is read
        // back out of the stored row by Rust, below.
        assert_eq!(
            status.correction_acknowledgment.as_deref(),
            Some("Got it — Qwybex counts as focus work from now on.")
        );
        // Resolved from the list rather than taken from the client, so the rule
        // answers under the bundle identity too.
        assert!(persistence
            .abstraction_map_repo()
            .bundle_app_override(&bundle_key(&persistence, "com.example.qwybex"))
            .unwrap()
            .is_some());
        assert!(after.entries.is_empty());
    }

    /// Saying the same thing twice is saying it once, and the repeat must not
    /// cost the rule the bundle identity it was written with — by then the app
    /// has left the triage list, so there is nothing to resolve it from.
    #[tokio::test]
    async fn teaching_the_same_app_twice_is_idempotent() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let rules = persistence.abstraction_map_repo();
        router
            .route(raw_event(
                "Qwybex",
                "Zarniwoop",
                Some("com.example.qwybex"),
                600,
            ))
            .await
            .unwrap();
        let teach = |category: &str| {
            ClientMessage::SetApplicationCategory(velvt_shared_types::SetApplicationCategory {
                app_stable_id: app_key(&persistence, "Qwybex"),
                category: category.to_owned(),
                activity_name: Some("Qwybex".into()),
            })
        };

        menu_status(&router, teach("FOCUS_WORK")).await;
        menu_status(&router, teach("REFERENCE")).await;

        let rule = rules
            .app_scope_override(&app_key(&persistence, "Qwybex"))
            .unwrap()
            .unwrap();
        assert_eq!(rule.category, "REFERENCE");
        assert_eq!(
            rule.bundle_key_hash.as_deref(),
            Some(bundle_key(&persistence, "com.example.qwybex").as_str())
        );
        // One row, answered twice: `correction_count` starts at 1 and the
        // repeat advances it, because how often someone had to say the same
        // thing is the signal that something upstream is wrong.
        assert_eq!(rule.correction_count, 2);
    }

    /// A key Velvt never issued would write a rule no event can match: not in
    /// the history's app rules, not reachable by removal, not explainable.
    #[tokio::test]
    async fn teaching_refuses_a_key_or_category_velvt_does_not_recognise() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        let teach = |app_stable_id: &str, category: &str| {
            ClientMessage::SetApplicationCategory(velvt_shared_types::SetApplicationCategory {
                app_stable_id: app_stable_id.to_owned(),
                category: category.to_owned(),
                activity_name: None,
            })
        };

        let bad_key = router
            .route(teach("Qwybex", "FOCUS_WORK"))
            .await
            .unwrap()
            .unwrap();
        let bad_category = router
            .route(teach(&app_key(&persistence, "Qwybex"), "PROCRASTINATION"))
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            bad_key,
            ServerMessage::ErrorResponse(ref error) if error.code == "invalid_app_stable_id"
        ));
        assert!(matches!(
            bad_category,
            ServerMessage::ErrorResponse(ref error) if error.code == "invalid_classification_category"
        ));
        assert!(persistence
            .abstraction_map_repo()
            .app_scope_override(&app_key(&persistence, "Qwybex"))
            .unwrap()
            .is_none());
    }

    /// A list of one-second curiosities is not a task anyone will do.
    #[tokio::test]
    async fn an_app_seen_only_briefly_is_not_a_triage_task() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        router
            .route(raw_event("Qwybex", "Zarniwoop", None, 60))
            .await
            .unwrap();

        assert!(triage(&router, 14).await.entries.is_empty());
    }

    /// Absent declared metadata must behave exactly as it did before the
    /// columns existed: the event is stored, and the triage row reads the same.
    #[tokio::test]
    async fn an_event_without_declared_metadata_is_recorded_as_before() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);

        let ack = router
            .route(raw_event("Qwybex", "Zarniwoop", None, 600))
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            ack,
            ServerMessage::RawEventAck(ref ack) if ack.status == RawEventStatus::Accepted
        ));
        let entry = triage(&router, 14).await.entries.remove(0);
        assert_eq!(entry.display_name, "Qwybex");
        assert_eq!(entry.seconds_observed, 600);
    }

    /// A frame that breaks a bound the client already applies is a defect or a
    /// forgery — in the *declaration*, not in the clock. Velvt abstains from
    /// the metadata and keeps the event: ten minutes of an application it could
    /// not read is exactly the first row the triage list exists to show, and
    /// throwing the observation away would lose real observed time to fix a
    /// hint it never needed. Cleared rather than truncated: a shortened list is
    /// a different set of declared types, and classifying on a set the
    /// application never declared is worse than classifying on nothing.
    #[tokio::test]
    async fn declared_metadata_beyond_its_bounds_is_cleared_and_the_event_stored() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let router = correction_router(&persistence);
        // Every identifier in the over-long list would have carried a verdict
        // had it been honoured, so an event that lands UNLOGGED is proof the
        // declaration was cleared rather than trimmed to a prefix.
        let mut event = velvt_shared_types::RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            duration_seconds: 600,
            app_name: "Qwybex".into(),
            window_title: "Zarniwoop".into(),
            bundle_id: Some("com.example.qwybex".into()),
            declared_app_category: None,
            document_type_ids: (0..=velvt_shared_types::MAX_DOCUMENT_TYPE_IDS)
                .map(|_| "public.source-code".to_owned())
                .collect(),
            focused_document_url: None,
        };

        let too_many = router
            .route(ClientMessage::RawEvent(event.clone()))
            .await
            .unwrap()
            .unwrap();
        event.event_id = Uuid::new_v4();
        event.document_type_ids =
            vec!["a".repeat(velvt_shared_types::MAX_DOCUMENT_TYPE_ID_LENGTH + 1)];
        let too_long = router
            .route(ClientMessage::RawEvent(event.clone()))
            .await
            .unwrap()
            .unwrap();

        for ack in [too_many, too_long] {
            assert!(
                matches!(
                    ack,
                    ServerMessage::RawEventAck(ref ack)
                        if ack.status == RawEventStatus::Accepted && ack.drop_reason.is_none()
                ),
                "a bound violation must cost the metadata, not the event: {ack:?}"
            );
        }

        // Both events are stored, with their full observed duration, and
        // neither carries a verdict read off the declaration that was refused.
        let now = Utc::now();
        let stored = persistence
            .raw_event_repo()
            .events_between(
                now - chrono::Duration::hours(1),
                now + chrono::Duration::hours(1),
                10,
            )
            .unwrap();
        assert_eq!(stored.len(), 2);
        for entry in &stored {
            assert_eq!(entry.duration_seconds, 600);
            assert_ne!(
                entry.classification_source,
                ClassificationSource::DeclaredDocumentTypes.as_str()
            );
            assert_eq!(entry.category, "UNLOGGED");
        }

        // And the observed time reaches the surface that exists to collect it.
        let triaged = triage(&router, 14).await.entries.remove(0);
        assert_eq!(triaged.display_name, "Qwybex");
        assert_eq!(triaged.seconds_observed, 1200);
        assert_eq!(triaged.event_count, 2);
    }

    /// The two tiers that read declared metadata have to survive the round trip
    /// through the audit row, or the menu reports them as `fallback`.
    #[test]
    fn the_declared_metadata_sources_parse_back_out_of_storage() {
        assert_eq!(
            parse_classification_source(Some("declared_document_types")),
            ClassificationSource::DeclaredDocumentTypes
        );
        assert_eq!(
            parse_classification_source(Some("declared_app_category")),
            ClassificationSource::DeclaredAppCategory
        );
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
    receipts: Option<Arc<ReceiptsManager>>,
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
            receipts: None,
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

    /// Attaches the weekly receipts and probe-bucket owner. Without it,
    /// digest messages are acknowledged and dropped and no digest exists.
    pub fn with_receipts(mut self, receipts: Arc<ReceiptsManager>) -> Self {
        self.receipts = Some(receipts);
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
        self.menu_status_saying(correction_acknowledgment(activity, category, during_block))
            .await
    }

    /// The same one-shot confirmation for a command that names no category:
    /// an undo, a reset, or a rule taught about a whole application.
    ///
    /// Undo and reset answered with a bare status until protocol 30, so the
    /// only evidence either had run was a row leaving a list the user may not
    /// have been looking at — and a command with no visible effect reads as a
    /// broken one. Same voice as `correction_acknowledgment`: what happened,
    /// and nothing else.
    async fn menu_status_saying(&self, acknowledgment: String) -> MenuStatus {
        MenuStatus {
            correction_acknowledgment: Some(acknowledgment),
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
                let outcome = self.account.delete_account().await;
                // After acceptance, not before: a deletion the cloud refused
                // leaves the user signed in and the queue theirs.
                if matches!(outcome, ServerMessage::AccountDeletionAccepted(_)) {
                    self.destroy_resumable_upload_queue();
                }
                Ok(Some(outcome))
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
                // The history lists app rules and window rules together and
                // the client edits whichever one the user tapped, so the same
                // field arrives meaning two different identities: `stable_id`
                // is an abstraction stable id for a window rule and the
                // application's own key hash for an app rule. The two hash
                // domains cannot collide (`key.rs:32`), so the stored rule
                // decides which write this is. Getting it wrong is not
                // cosmetic: `save_personal_override` answers `NotFound` for an
                // app key, which is how editing an app rule failed outright.
                let editing_app_rule = match abstraction_map
                    .app_scope_override(&correction.stable_id)
                {
                    Ok(found) => found.is_some(),
                    Err(err) => {
                        // Unreadable app rung: treat the edit as the window
                        // edit it has always been rather than refuse it.
                        tracing::warn!(
                            error_code = "app_scope_rule_read_failed",
                            error = %err,
                            "could not tell whether this rule is app-scoped; editing it as a window rule"
                        );
                        false
                    }
                };
                if editing_app_rule {
                    // `None` for the bundle key on purpose: the write
                    // coalesces, so an edit keeps the bundle identity the rule
                    // was taught with instead of dropping it on every edit.
                    if abstraction_map
                        .save_app_scope_override(
                            &correction.stable_id,
                            None,
                            &correction.category,
                            local_activity_name.as_deref(),
                        )
                        .is_err()
                    {
                        return Ok(Some(classification_correction_error(
                            "classification_correction_persistence_failed",
                        )));
                    }
                    return Ok(Some(ServerMessage::MenuStatus(
                        self.menu_status_acknowledging(
                            local_activity_name.as_deref(),
                            &correction.category,
                        )
                        .await,
                    )));
                }
                // An edit moves the app rung with it, or it is a lie: the
                // window the user was looking at changes and every other
                // window of the same application keeps falling through to the
                // app rung, which still holds the category the user just
                // replaced. Reached by stable id because an edit arrives long
                // after its source event left the queue, so the event id is
                // gone. Best-effort and deliberately outside the failure chain
                // below, exactly as in `CorrectEventClassification`: an edit
                // that took effect for the rule the user was looking at must
                // not report failure because it could not also be generalized.
                // Returns false when no event under this rule recorded an app
                // identity, or when the window was a browser tab, where one
                // site says nothing about the next.
                match abstraction_map.save_personal_app_override_by_stable_id(
                    &correction.stable_id,
                    &correction.category,
                    local_activity_name.as_deref(),
                ) {
                    Ok(generalized) => {
                        tracing::debug!(generalized, "classification edit app-scope outcome")
                    }
                    Err(err) => tracing::warn!(
                        error_code = "app_scoped_correction_failed",
                        error = %err,
                        "edit applied to the window rule but not generalized to the app"
                    ),
                }
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
                // Window rung first, then the app rung, with no scope from the
                // client: the two key domains cannot collide, so an id belongs
                // to exactly one of them and trying both in order is
                // unambiguous. `remove_personal_override` already removes the
                // app rule a window correction generalized to, so `Ok(false)`
                // here means this id was never a window rule — which is
                // precisely the app rule the history can now show, and which
                // until protocol 30 nothing could delete.
                let removed = match abstraction_map.remove_personal_override(&request.stable_id) {
                    Ok(true) => true,
                    Ok(false) => {
                        match abstraction_map.remove_app_scope_override(&request.stable_id) {
                            Ok(removed) => removed,
                            Err(_) => {
                                return Ok(Some(classification_correction_error(
                                    "classification_correction_persistence_failed",
                                )))
                            }
                        }
                    }
                    Err(_) => {
                        return Ok(Some(classification_correction_error(
                            "classification_correction_persistence_failed",
                        )))
                    }
                };
                Ok(Some(ServerMessage::MenuStatus(
                    self.menu_status_saying(removal_acknowledgment(removed))
                        .await,
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
                    self.menu_status_saying(reset_acknowledgment()).await,
                )))
            }

            ClientMessage::RequestUnclassifiedTriage(request) => {
                // Facts only, and bounded: the applications Velvt could not
                // read, longest observed first. All three bounds are re-clamped
                // inside the query, so this reports the window that was
                // actually used rather than the one that was asked for.
                let entries = match self.raw_event_repo.unclassified_triage(
                    request.lookback_days,
                    TRIAGE_MIN_SECONDS,
                    TRIAGE_MAX_ENTRIES,
                ) {
                    Ok(entries) => entries,
                    Err(err) => {
                        tracing::warn!(
                            error_code = "unclassified_triage_failed",
                            error = %err,
                            "could not read the list of applications Velvt cannot read"
                        );
                        // An empty list is the good state and the UI says so,
                        // so a failure must not borrow that sentence.
                        return Ok(Some(triage_error()));
                    }
                };
                Ok(Some(ServerMessage::UnclassifiedTriage(
                    UnclassifiedTriage {
                        entries: entries.into_iter().map(triage_entry).collect(),
                        window_days: request.lookback_days.clamp(1, TRIAGE_MAX_LOOKBACK_DAYS),
                    },
                )))
            }

            ClientMessage::SetApplicationCategory(request) => {
                Ok(Some(self.set_application_category(request).await))
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

            ClientMessage::InterventionCardSeen(request) => self.work_block_response(|manager| {
                manager.record_intervention_card_seen(request.block_id, Utc::now())
            }),

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
                // Digests and probe buckets are derived from the record
                // being cleared; the demotion singleton is cleared inside
                // `WorkBlockManager::clear_data` with the record itself.
                if let Some(receipts) = &self.receipts {
                    if receipts.clear_data().is_err() {
                        tracing::warn!(
                            error_code = "receipts_data_clear_failed",
                            "weekly digest record was not cleared"
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

            ClientMessage::RequestDemotionState(_) => {
                let Some(work_blocks) = &self.work_blocks else {
                    return Ok(None);
                };
                match work_blocks.demotion_state_payload(Utc::now()) {
                    Ok(state) => Ok(Some(ServerMessage::DemotionState(state))),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "demotion_state_read_failed",
                            "demotion state could not be evaluated"
                        );
                        Ok(None)
                    }
                }
            }

            ClientMessage::ResetInterventionDemotion(_) => {
                let Some(work_blocks) = &self.work_blocks else {
                    return Ok(None);
                };
                match work_blocks.reset_demotion(Utc::now()) {
                    Ok(state) => Ok(Some(ServerMessage::DemotionState(state))),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "demotion_reset_failed",
                            "demotion reset was not recorded"
                        );
                        Ok(None)
                    }
                }
            }

            ClientMessage::RequestWeeklyDigest(request) => {
                let Some(receipts) = &self.receipts else {
                    return Ok(None);
                };
                match receipts.pending_digest(Utc::now(), request.utc_offset_seconds) {
                    Ok(Some(digest)) => Ok(Some(ServerMessage::WeeklyDigest(digest))),
                    Ok(None) => Ok(None),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "weekly_digest_check_failed",
                            "pending weekly digest could not be evaluated"
                        );
                        Ok(None)
                    }
                }
            }

            ClientMessage::AcknowledgeWeeklyDigest(request) => {
                let Some(receipts) = &self.receipts else {
                    return Ok(None);
                };
                if receipts
                    .acknowledge(&request.week_start_local_date, Utc::now())
                    .is_err()
                {
                    tracing::warn!(
                        error_code = "weekly_digest_acknowledge_failed",
                        "weekly digest acknowledgment was not recorded"
                    );
                }
                Ok(None)
            }

            ClientMessage::RequestInterventionExplanation(request) => {
                let Some(work_blocks) = &self.work_blocks else {
                    return Ok(None);
                };
                match work_blocks.explain_intervention(request.block_id) {
                    Ok(Some(sentence)) => {
                        // The probe metric (D7; roadmap Metrics 5): one
                        // coarse local weekly counter, incremented only
                        // when an explanation was actually shown. Nothing
                        // about which nudge or when within the week.
                        if let Some(receipts) = &self.receipts {
                            if receipts
                                .record_explain_tap(Utc::now(), request.utc_offset_seconds)
                                .is_err()
                            {
                                tracing::warn!(
                                    error_code = "explain_tap_record_failed",
                                    "explain-tap bucket was not incremented"
                                );
                            }
                        }
                        Ok(Some(ServerMessage::InterventionExplanation(
                            velvt_shared_types::InterventionExplanation {
                                block_id: request.block_id,
                                sentence,
                            },
                        )))
                    }
                    Ok(None) => Ok(None),
                    Err(_) => {
                        tracing::warn!(
                            error_code = "intervention_explanation_failed",
                            "intervention explanation could not be built"
                        );
                        Ok(None)
                    }
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

fn triage_error() -> ServerMessage {
    ServerMessage::ErrorResponse(velvt_shared_types::ErrorResponse {
        code: "unclassified_triage_failed".to_owned(),
        message: "Unable to list the apps Velvt could not read. Try again later.".into(),
        related_event_id: None,
    })
}

/// One stored triage row as the client sees it.
///
/// Facts only, and only the ones the client needs: a key to send back, a name
/// to show, and the time observed. The bundle identity Velvt holds for the
/// application stays here — the rule the user saves is keyed on it in Rust,
/// which already reads it out of the stored row, so sending it to Swift would
/// hand the client an identifier it has no use for and cannot display.
fn triage_entry(entry: UnclassifiedAppEntry) -> UnclassifiedTriageEntry {
    UnclassifiedTriageEntry {
        app_stable_id: entry.app_stable_id,
        display_name: entry.display_name,
        seconds_observed: entry.seconds_observed,
        event_count: entry.event_count,
    }
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
    let category = spoken_category(category);
    if during_active_block {
        format!("Got it — {subject} counts as {category} for the rest of this block.")
    } else {
        format!("Got it — {subject} counts as {category} from now on.")
    }
}

/// Confirms what a whole application was taught.
///
/// Carries no block qualifier, unlike `correction_acknowledgment`: an app rule
/// outlives the block it was written in, and "for the rest of this block" would
/// be a false promise about a rule that never expires.
fn application_acknowledgment(activity: Option<&str>, category: &str) -> String {
    let subject = activity.unwrap_or("This app");
    let category = spoken_category(category);
    format!("Got it — {subject} counts as {category} from now on.")
}

/// Confirms an undo, including the case where there was nothing left to undo.
///
/// Says what is true now rather than congratulating anybody, and never claims a
/// removal that did not happen: a rule the user had already deleted on another
/// surface must not be reported as deleted again.
fn removal_acknowledgment(removed: bool) -> String {
    if removed {
        "Removed — Velvt classifies this on its own again.".to_owned()
    } else {
        "Nothing to remove — Velvt is already classifying this on its own.".to_owned()
    }
}

/// Confirms a reset of every saved rule.
///
/// Deliberately quotes no total. `reset_personal_overrides` returns the number
/// of *window* rules it deleted, and it also deletes every app rule, so any
/// number printed here would undercount what was destroyed — and a count
/// presented back as a total invites reading it as a score.
fn reset_acknowledgment() -> String {
    "Removed every saved rule — Velvt classifies everything on its own again.".to_owned()
}

/// The category as the copy says it out loud: `FOCUS_WORK` becomes `focus work`.
fn spoken_category(category: &str) -> String {
    category.replace('_', " ").to_ascii_lowercase()
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
        Some("declared_document_types") => ClassificationSource::DeclaredDocumentTypes,
        Some("declared_app_category") => ClassificationSource::DeclaredAppCategory,
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

    /// Teaches Velvt one application, with no source event.
    ///
    /// The triage surface is per-app and once, so there is nothing to correct:
    /// the user is naming an application, not disputing a moment of it, and the
    /// key being answered is the one Velvt's own list handed out. Idempotent —
    /// saying the same thing twice is saying it once — and it acknowledges,
    /// because a user who cannot see the answer land has no reason to give
    /// another one.
    async fn set_application_category(&self, request: SetApplicationCategory) -> ServerMessage {
        if crate::abstraction::override_label_for_category(&request.category).is_none() {
            return classification_correction_error("invalid_classification_category");
        }
        let Some(abstraction_map) = &self.abstraction_map else {
            return classification_correction_error("classification_correction_unavailable");
        };
        let Some(app_stable_id) = normalized_app_stable_id(&request.app_stable_id) else {
            return classification_correction_error("invalid_app_stable_id");
        };
        let activity_name = match normalized_local_activity_name(request.activity_name.as_deref()) {
            Ok(value) => value,
            Err(()) => return classification_correction_error("invalid_local_activity_name"),
        };
        let bundle_key_hash = self.bundle_key_for_app(app_stable_id);
        if abstraction_map
            .save_app_scope_override(
                app_stable_id,
                bundle_key_hash.as_deref(),
                &request.category,
                activity_name.as_deref(),
            )
            .is_err()
        {
            return classification_correction_error("classification_correction_persistence_failed");
        }
        ServerMessage::MenuStatus(
            self.menu_status_saying(application_acknowledgment(
                activity_name.as_deref(),
                &request.category,
            ))
            .await,
        )
    }

    /// The bundle identity Velvt recorded for an application, if it has one.
    ///
    /// Read back out of the same list the client is answering rather than taken
    /// from the message, for two reasons. Swift reports facts and this is a
    /// conclusion about which stored rows are one application; and the raw
    /// bundle identifier never survives the abstraction boundary, so the only
    /// bundle key that exists anywhere is the hash the event rows already hold.
    ///
    /// Best-effort by design. When the application is no longer on the list —
    /// it fell below the cap, or a rule for it already exists, which is exactly
    /// the repeat case — the rule is keyed on the name alone, which is how
    /// every rule worked before protocol 30. A repeat never loses the stored
    /// bundle key either: `save_app_scope_override` coalesces, so `None` here
    /// leaves whatever was written the first time.
    fn bundle_key_for_app(&self, app_stable_id: &str) -> Option<String> {
        self.raw_event_repo
            .unclassified_triage(
                TRIAGE_MAX_LOOKBACK_DAYS,
                TRIAGE_MIN_SECONDS,
                TRIAGE_MAX_ENTRIES,
            )
            .ok()?
            .into_iter()
            .find(|entry| entry.app_stable_id == app_stable_id)?
            .app_bundle_stable_id
    }

    /// Deletes every upload batch that can still be sent, and the events
    /// inside them, once the cloud has accepted the account deletion.
    ///
    /// The queue has no owner. `resumable_batches` selects on status and
    /// schedule and on nothing about who queued the row, and neither
    /// `upload_batch` nor `BatchPayload` carries a device or user identifier, so
    /// the cloud attributes a batch purely by whichever bearer
    /// token the retry loop is holding when it next runs. Account deletion
    /// clears the device id along with the tokens
    /// (`AccountAuthService::clear_local_session`), so the next sign-up on this
    /// Mac registers a new device — and the backend scopes duplicate detection
    /// by device id, so batches minted under the deleted account are not
    /// refused as duplicates there. They are stored against the new account.
    /// The likeliest person on the other end of that is the same one, deleting
    /// and re-registering, which restores the history they asked to destroy.
    ///
    /// `SqliteUploadBatchRepo::pending_batches` is `resumable_batches` at an
    /// unbounded horizon, which is the horizon this needs: on the development
    /// device every one of the 182 queued batches carried the same
    /// `next_attempt_at`, and all of them were scheduled past the moment the
    /// service last ran. A purge on the retry loop's own horizon would have
    /// left every one of them behind.
    ///
    /// It reaches `pending` and `failed` and nothing else, because those are
    /// the two statuses `resumable_batches` returns: `sent`, `rejected` and
    /// `abandoned` are terminal and no code path uploads them. Those rows stay
    /// on disk with the rest of the local database, and the deletion dialog
    /// says so rather than implying a purge that does not happen.
    ///
    /// A process that dies between the cloud's acceptance and this call leaves
    /// the queue behind, and this is the only place ownership is enforced. The
    /// standing check that would also cover it belongs in `resume_pending` —
    /// `BatchAssembler` derives a batch id by hashing the device id with the
    /// event ids, so a resumed batch can be tested against the device holding
    /// it. `tests/account_deletion.rs` pins that property and shows the rule
    /// working; nothing in `src/` installs it.
    fn destroy_resumable_upload_queue(&self) {
        let Some(batches) = &self.upload_batches else {
            tracing::error!(
                error_code = "account_deletion_queue_unreachable",
                "no upload queue was attached, so queued activity was not destroyed"
            );
            return;
        };
        let queued = match batches.pending_batches() {
            Ok(queued) => queued,
            Err(error) => {
                tracing::error!(
                    error_code = "account_deletion_queue_read_failed",
                    error = %error,
                    "queued activity was not destroyed with the account"
                );
                return;
            }
        };
        let mut destroyed = 0usize;
        let mut surviving = 0usize;
        for batch in &queued {
            match batches.discard_batch(&batch.batch_id) {
                // A row already gone is the outcome this wants.
                Ok(()) | Err(PersistenceError::NotFound { .. }) => destroyed += 1,
                Err(error) => {
                    surviving += 1;
                    tracing::error!(
                        error_code = "account_deletion_batch_delete_failed",
                        error = %error,
                        "a queued batch survived account deletion"
                    );
                }
            }
        }
        if surviving == 0 {
            tracing::info!(
                message_type = "delete_account",
                destroyed_batches = destroyed,
                "queued activity was destroyed with the account"
            );
        } else {
            tracing::error!(
                error_code = "account_deletion_queue_purge_incomplete",
                destroyed_batches = destroyed,
                surviving_batches = surviving,
                "queued activity survived account deletion"
            );
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
    async fn handle_raw_event(&self, mut event: velvt_shared_types::RawEvent) -> ServerMessage {
        let event_id = event.event_id;
        let occurred_at = event.occurred_at;
        let duration_seconds = event
            .duration_seconds
            .min(u64::from(MAX_REPORTED_DWELL_SECONDS));
        let upload_eligible = self.upload_eligible();
        // Before abstraction, because a frame that breaks a published bound is
        // not evidence. What is refused is the *declaration*, never the event:
        // the duration is real observed time and the product's primary input,
        // while declared metadata is an optional hint. So the offending
        // declaration is cleared and the event is classified and stored exactly
        // as one that never carried any metadata — which is also the behaviour
        // the absent-metadata invariant already guarantees.
        //
        // Dropping the event here would be the most expensive possible reading
        // of a client bug: the applications that break a client-side cap are
        // the richest declarers, so their time would vanish from the audit row,
        // the work blocks and the triage list at once, silently. The contract
        // authorises abstaining from the metadata, not discarding observations.
        //
        // Cleared rather than truncated: a shortened list is a different set of
        // declared types, and classifying on a set the application did not
        // declare is worse than classifying on nothing.
        if let Err(err) = event.validate_declared_metadata() {
            // Matched exhaustively on purpose: every bound published today is a
            // bound on the declared document-type list, and a bound added later
            // on some other declared field must fail to compile here rather
            // than leave that field feeding the classifier unchecked.
            match err {
                RawEventMetadataError::TooManyDocumentTypes
                | RawEventMetadataError::DocumentTypeTooLong
                | RawEventMetadataError::EmptyDocumentType => event.document_type_ids.clear(),
            }
            // Debug, not warn: with the observation kept this is a degraded
            // hint, not an incident. The reason code is a fixed string and
            // carries no fragment of the values that were refused.
            tracing::debug!(
                error_code = "raw_event_metadata_cleared",
                reason = err.code(),
                "cleared declared metadata that broke its published bounds and kept the event"
            );
        }
        // Captured before `process` consumes the event. The bundle identifier
        // is hashed here and the raw value is dropped with the rest of the raw
        // frame: what is persisted, and what the triage list later hands back,
        // is only ever the key. An all-absent value writes the row exactly as
        // it was written before these columns existed.
        //
        // A blank identifier is treated as no identifier rather than hashed.
        // The digest of an empty string is a perfectly good hash and that is
        // the problem: every application whose bundle id came through blank
        // would share one bundle key, and one rule taught about any of them
        // would answer for all of them. The value is otherwise hashed exactly
        // as received, so this key matches the one the engine computes.
        let declared_metadata = DeclaredAppMetadata {
            app_bundle_stable_id: event
                .bundle_id
                .as_deref()
                .filter(|bundle_id| !bundle_id.trim().is_empty())
                .map(|bundle_id| self.abstraction_engine.app_bundle_key(bundle_id)),
            declared_app_category: event.declared_app_category.clone(),
            document_type_ids: event.document_type_ids.clone(),
        };
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
                if let Err(err) = self
                    .raw_event_repo
                    .insert_with_declared_metadata(&entry, &declared_metadata)
                {
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
                                let mut snapshot = outcome.snapshot;
                                // An in-session offer is authored entirely
                                // on-device: it never waits on the cloud or on
                                // a mature baseline.
                                //
                                // Salience is the whole delivery instruction,
                                // and the snapshot is the only thing that
                                // carries it to the client. `Normal` rings and
                                // renders; `Quiet` renders only. Reduced
                                // salience is already set by backoff: after a
                                // negative reply in this block, the offer never
                                // regains the OS notification.
                                //
                                // Velvt's own quiet hours reduce delivery the
                                // same way — inside the accepted window the
                                // offer keeps its in-app card and sends no OS
                                // notification — so the window is applied here,
                                // on the field the client reads, rather than on
                                // a parallel push the client's notification
                                // path never consulted. (Active system DND
                                // never reaches this point: the manager holds
                                // the whole decision and no offer is surfaced
                                // at all.)
                                if snapshot.active_intervention.is_some()
                                    && self.focus.as_ref().is_some_and(|focus| {
                                        focus.in_velvt_quiet_hours(occurred_at)
                                    })
                                {
                                    if let Some(active) = snapshot.active_intervention.as_mut() {
                                        active.salience = InterventionSalience::Quiet;
                                    }
                                }
                                push.push_work_block_state(snapshot).await;
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
