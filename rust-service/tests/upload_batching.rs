use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::time::Duration;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use velvt_service::abstraction::{AbstractionEngine, InMemoryMappingStore};
use velvt_service::auth::{AuthError, HttpClient, HttpRequest, HttpResponse};
use velvt_service::persistence::{
    BatchEvent, NewUploadBatch, PersistenceError, SqlitePersistence, UploadBatch, UploadBatchRepo,
    UploadBatchStatus,
};
use velvt_service::upload::{
    BatchAssembler, BatchEventPayload, BatchPayload, BatchRetentionPolicy, BatchUploader,
    EventIngestor, FakeBatchUploader, FakePrivacyAlertSink, HostBackoff, HttpBatchUploader,
    IpcPrivacyAlertSink, SharedUploadBatcher, UploadBatcher, UploadCoordinator, UploadOutcome,
};
use velvt_shared_types::RawEvent;

fn event(id: &str, seconds: i64) -> BatchEventPayload {
    BatchEventPayload {
        event_id: id.into(),
        stable_id: format!("stable-{id}"),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-1".into(),
        occurred_at: Utc.timestamp_opt(seconds, 0).unwrap(),
        duration_seconds: 0,
    }
}

#[derive(Clone)]
struct FakeHttp {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    responses: Arc<Mutex<VecDeque<HttpResponse>>>,
}

#[derive(Clone, Default)]
struct FailingUploader {
    uploads: Arc<Mutex<usize>>,
}

#[derive(Clone)]
struct InFlightUploader {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    uploads: Arc<Mutex<usize>>,
}

struct FailFirstInsertRepo {
    inner: Arc<dyn UploadBatchRepo>,
    failed: std::sync::atomic::AtomicBool,
}

impl FailFirstInsertRepo {
    fn new(inner: Arc<dyn UploadBatchRepo>) -> Self {
        Self {
            inner,
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl UploadBatchRepo for FailFirstInsertRepo {
    fn insert_batch(&self, batch: &NewUploadBatch) -> Result<(), PersistenceError> {
        self.inner.insert_batch(batch)
    }

    fn insert_batch_with_events(
        &self,
        batch: &NewUploadBatch,
        events: &[BatchEvent],
    ) -> Result<(), PersistenceError> {
        if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err(PersistenceError::NotFound {
                entity: "upload_batch",
            });
        }
        self.inner.insert_batch_with_events(batch, events)
    }

    fn mark_sent(&self, batch_id: &str) -> Result<(), PersistenceError> {
        self.inner.mark_sent(batch_id)
    }

    fn pending_batches(&self) -> Result<Vec<UploadBatch>, PersistenceError> {
        self.inner.pending_batches()
    }

    fn resumable_batches(&self, now: DateTime<Utc>) -> Result<Vec<UploadBatch>, PersistenceError> {
        self.inner.resumable_batches(now)
    }

    fn mark_failed(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError> {
        self.inner
            .mark_failed(batch_id, next_attempt_at, error_code)
    }

    fn mark_pending_retry(
        &self,
        batch_id: &str,
        next_attempt_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<(), PersistenceError> {
        self.inner
            .mark_pending_retry(batch_id, next_attempt_at, error_code)
    }

    fn mark_rejected(&self, batch_id: &str, error_code: &str) -> Result<(), PersistenceError> {
        self.inner.mark_rejected(batch_id, error_code)
    }

    fn discard_batch(&self, batch_id: &str) -> Result<(), PersistenceError> {
        self.inner.discard_batch(batch_id)
    }

    fn batch_status(&self, batch_id: &str) -> Result<UploadBatchStatus, PersistenceError> {
        self.inner.batch_status(batch_id)
    }

    fn host_backoff_attempt(&self, host: &str) -> Result<u32, PersistenceError> {
        self.inner.host_backoff_attempt(host)
    }

    fn host_backoff_until(&self, host: &str) -> Result<Option<DateTime<Utc>>, PersistenceError> {
        self.inner.host_backoff_until(host)
    }

    fn set_host_backoff(
        &self,
        host: &str,
        attempt_count: u32,
        next_attempt_at: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        self.inner
            .set_host_backoff(host, attempt_count, next_attempt_at)
    }

    fn clear_host_backoff(&self, host: &str) -> Result<(), PersistenceError> {
        self.inner.clear_host_backoff(host)
    }

    fn add_event_to_batch(
        &self,
        batch_id: &str,
        event: &BatchEvent,
    ) -> Result<(), PersistenceError> {
        self.inner.add_event_to_batch(batch_id, event)
    }

    fn delete_sent_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        self.inner.delete_sent_batch(cutoff, limit)
    }

    fn delete_rejected_batch(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<u64, PersistenceError> {
        self.inner.delete_rejected_batch(cutoff, limit)
    }
}

impl BatchUploader for InFlightUploader {
    fn upload<'a>(
        &'a self,
        _batch: &'a BatchPayload,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<UploadOutcome, velvt_service::upload::BatchUploadError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            *self.uploads.lock().unwrap() += 1;
            self.started.notify_one();
            self.release.notified().await;
            Ok(UploadOutcome::Accepted)
        })
    }
}

struct DiscardAll;

impl BatchRetentionPolicy for DiscardAll {
    fn should_discard(&self, _batch: &velvt_service::persistence::UploadBatch) -> bool {
        true
    }
}

impl BatchUploader for FailingUploader {
    fn upload<'a>(
        &'a self,
        _batch: &'a BatchPayload,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<UploadOutcome, velvt_service::upload::BatchUploadError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            *self.uploads.lock().unwrap() += 1;
            Err(velvt_service::upload::BatchUploadError::Transport)
        })
    }
}

impl HttpClient for FakeHttp {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            Ok(self.responses.lock().unwrap().pop_front().unwrap())
        })
    }
}

#[test]
fn count_threshold_flushes_independently() {
    let mut assembler = BatchAssembler::new("device-1", 2, Duration::from_secs(180));

    assert!(assembler
        .push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap())
        .is_none());
    let batch = assembler
        .push(event("two", 11), Utc.timestamp_opt(11, 0).unwrap())
        .unwrap();

    assert_eq!(batch.events.len(), 2);
}

#[test]
fn time_threshold_flushes_independently() {
    let mut assembler = BatchAssembler::new("device-1", 100, Duration::from_secs(16));
    assembler.push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap());

    assert!(assembler
        .flush_due(Utc.timestamp_opt(25, 0).unwrap())
        .is_none());
    assert!(assembler
        .flush_due(Utc.timestamp_opt(26, 0).unwrap())
        .is_some());
}

#[test]
fn shutdown_flushes_immediately_and_batch_id_is_deterministic() {
    let mut first = BatchAssembler::new("device-1", 100, Duration::from_secs(180));
    let mut second = BatchAssembler::new("device-1", 100, Duration::from_secs(180));
    first.push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap());
    second.push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap());

    assert_eq!(
        first.flush_shutdown().unwrap().batch_id,
        second.flush_shutdown().unwrap().batch_id
    );
}

#[test]
fn sleep_flushes_immediately() {
    let mut assembler = BatchAssembler::new("device-1", 100, Duration::from_secs(180));
    assembler.push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap());

    assert_eq!(assembler.flush_sleep().unwrap().events.len(), 1);
}

#[test]
fn simultaneous_count_and_time_flush_only_assembles_one_batch() {
    let mut assembler = BatchAssembler::new("device-1", 2, Duration::from_secs(16));
    assembler.push(event("one", 10), Utc.timestamp_opt(10, 0).unwrap());

    let batch = assembler
        .push(event("two", 26), Utc.timestamp_opt(26, 0).unwrap())
        .unwrap();

    assert_eq!(batch.events.len(), 2);
    assert!(assembler
        .flush_due(Utc.timestamp_opt(26, 0).unwrap())
        .is_none());
}

#[test]
fn payload_serialization_contains_only_audited_safe_fields() {
    let payload = BatchPayload::new(
        "batch-1",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![event("one", 10)],
    );
    let value = serde_json::to_value(payload).unwrap();
    let event = &value["events"][0];

    // Test-level reflection: serde exposes the complete serialized field set,
    // so this exact-key assertion fails if any raw-capable field is added.
    assert_eq!(
        event
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            "category",
            "duration_seconds",
            "label",
            "occurred_at",
            "stable_id",
            "taxonomy_version"
        ]
    );
    let json = serde_json::to_string(&value).unwrap();
    for forbidden in [
        "event_id",
        "raw_app_name",
        "raw_window_title",
        "app_name",
        "window_title",
    ] {
        assert!(!json.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn fake_uploader_treats_duplicate_as_success() {
    let uploader =
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted, UploadOutcome::Duplicate]);
    let payload = BatchPayload::new(
        "batch-1",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![event("one", 10)],
    );

    assert!(uploader.upload(&payload).await.unwrap().is_success());
    assert!(uploader.upload(&payload).await.unwrap().is_success());
    assert_eq!(uploader.upload_count(), 2);
}

#[tokio::test]
async fn http_uploader_posts_exact_payload_and_maps_duplicate() {
    let http = Arc::new(FakeHttp {
        requests: Arc::default(),
        responses: Arc::new(Mutex::new(
            vec![
                HttpResponse {
                    status: 202,
                    error_code: None,
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: None,
                    user_id: None,
                    device_id: None,
                },
                HttpResponse {
                    status: 409,
                    error_code: Some("duplicate_batch".into()),
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: None,
                    user_id: None,
                    device_id: None,
                },
                HttpResponse {
                    status: 202,
                    error_code: None,
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: Some(serde_json::json!({ "status": "duplicate" })),
                    user_id: None,
                    device_id: None,
                },
            ]
            .into(),
        )),
    });
    let uploader = HttpBatchUploader::new(http.clone());
    let payload = BatchPayload::new(
        "batch-1",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![event("one", 10)],
    );

    assert_eq!(
        uploader.upload(&payload).await.unwrap(),
        UploadOutcome::Accepted
    );
    assert_eq!(
        uploader.upload(&payload).await.unwrap(),
        UploadOutcome::Duplicate
    );
    assert_eq!(
        uploader.upload(&payload).await.unwrap(),
        UploadOutcome::Duplicate
    );
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/events/batches");
    let body = requests[0].json_body.as_ref().unwrap();
    assert_eq!(body["schema_version"], "1");
    assert_eq!(body["client_version"], "0.1.0");
    assert_eq!(body["supported_abstraction_types"][0], "document:edit");
    assert_eq!(body["category_taxonomy_version"], "mvp-1");
}

#[test]
fn retry_after_and_exponential_fallback_are_host_scoped() {
    let mut backoff = HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0);

    assert_eq!(
        backoff.next_delay("dev-api.getvelvt.com", Some("120")),
        Duration::from_secs(120)
    );
    assert_eq!(
        backoff.next_delay("dev-api.getvelvt.com", Some("1800")),
        Duration::from_secs(1800)
    );
    assert_eq!(
        backoff.next_delay("other.velvt.test", None),
        Duration::from_secs(30)
    );
    assert_eq!(
        backoff.next_delay("other.velvt.test", None),
        Duration::from_secs(60)
    );
    backoff.reset("other.velvt.test");
    assert_eq!(
        backoff.next_delay("other.velvt.test", None),
        Duration::from_secs(30)
    );
}

#[test]
fn privacy_payload_is_not_an_untyped_json_value() {
    let payload = BatchPayload::new(
        "batch-1",
        "1",
        "0.1.0",
        vec!["document:edit".into()],
        "mvp-1",
        vec![event("one", 10)],
    );

    assert!(matches!(
        serde_json::to_value(payload).unwrap(),
        Value::Object(_)
    ));
}

#[tokio::test]
async fn raw_field_rejected_is_terminal_and_alerted() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch(&NewUploadBatch {
            batch_id: "batch-rejected".into(),
        })
        .unwrap();
    let http = Arc::new(FakeHttp {
        requests: Arc::default(),
        responses: Arc::new(Mutex::new(
            vec![HttpResponse {
                status: 422,
                error_code: Some("raw_field_rejected".into()),
                tokens: None,
                retry_after: None,
                message: Some("forbidden field".into()),
                raw_body: None,
                user_id: None,
                device_id: None,
            }]
            .into(),
        )),
    });
    let uploader = HttpBatchUploader::new(http.clone());
    let alerts = FakePrivacyAlertSink::default();
    let coordinator = UploadCoordinator::new(repository.clone(), uploader, alerts.clone());

    coordinator
        .upload_batch(BatchPayload::new(
            "batch-rejected",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    assert_eq!(alerts.alert_count(), 1);
    assert_eq!(http.requests.lock().unwrap().len(), 1);
    assert!(repository.resumable_batches(Utc::now()).unwrap().is_empty());
    assert_eq!(
        repository.batch_status("batch-rejected").unwrap(),
        UploadBatchStatus::Rejected
    );
}

#[tokio::test]
async fn pending_batch_is_resumed_after_restart() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "batch-pending".into(),
            },
            &[BatchEvent {
                event_id: "event-one".into(),
                stable_id: "stable-one".into(),
                label: "document:edit".into(),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                duration_seconds: 0,
            }],
        )
        .unwrap();
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    );

    assert_eq!(
        coordinator
            .resume_pending("1", "0.1.0", &["document:edit".into()])
            .await
            .unwrap(),
        1
    );
    assert_eq!(inspection.upload_count(), 1);
    assert_eq!(
        repository.batch_status("batch-pending").unwrap(),
        UploadBatchStatus::Sent
    );
}

#[tokio::test]
async fn submitted_batch_is_persisted_before_upload() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Retryable {
        code: "transport".into(),
    }]);
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    )
    .with_host_and_backoff(
        "dev-api.getvelvt.com",
        HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0),
    );

    coordinator
        .submit_batch(BatchPayload::new(
            "batch-stored",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    assert_eq!(
        repository.batch_status("batch-stored").unwrap(),
        UploadBatchStatus::Failed
    );
}

#[tokio::test]
async fn transport_failure_persists_retry_state() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        FailingUploader::default(),
        FakePrivacyAlertSink::default(),
    )
    .with_host_and_backoff(
        "dev-api.getvelvt.com",
        HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0),
    );

    coordinator
        .submit_batch(BatchPayload::new(
            "batch-transport-failure",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    assert_eq!(
        repository.batch_status("batch-transport-failure").unwrap(),
        UploadBatchStatus::Pending
    );
    assert_eq!(
        repository
            .host_backoff_attempt("dev-api.getvelvt.com")
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn network_failure_is_retried_on_next_cycle() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    let first = FailingUploader::default();
    let coordinator =
        UploadCoordinator::new(repository.clone(), first, FakePrivacyAlertSink::default())
            .with_host_and_backoff(
                "dev-api.getvelvt.com",
                HostBackoff::new(Duration::ZERO, Duration::from_secs(900), || 1.0),
            );
    coordinator
        .submit_batch(BatchPayload::new(
            "batch-network-retry",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    let retry = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = retry.clone();
    UploadCoordinator::new(repository.clone(), retry, FakePrivacyAlertSink::default())
        .resume_pending("1", "0.1.0", &["document:edit".into()])
        .await
        .unwrap();

    assert_eq!(inspection.upload_count(), 1);
    assert_eq!(
        repository.batch_status("batch-network-retry").unwrap(),
        UploadBatchStatus::Sent
    );
}

#[tokio::test]
async fn shutdown_during_in_flight_upload_is_persisted_and_restart_submits_once() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    let in_flight = InFlightUploader {
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        uploads: Arc::new(Mutex::new(0)),
    };
    let started = in_flight.started.clone();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        in_flight,
        FakePrivacyAlertSink::default(),
    );
    let task = tokio::spawn(async move {
        coordinator
            .submit_batch(BatchPayload::new(
                "batch-in-flight",
                "1",
                "0.1.0",
                vec!["document:edit".into()],
                "mvp-1",
                vec![event("one", 10)],
            ))
            .await
    });
    started.notified().await;
    task.abort();

    assert_eq!(
        repository.batch_status("batch-in-flight").unwrap(),
        UploadBatchStatus::Pending
    );
    let retry = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = retry.clone();
    UploadCoordinator::new(repository.clone(), retry, FakePrivacyAlertSink::default())
        .resume_pending("1", "0.1.0", &["document:edit".into()])
        .await
        .unwrap();

    assert_eq!(inspection.upload_count(), 1);
    assert_eq!(
        repository.batch_status("batch-in-flight").unwrap(),
        UploadBatchStatus::Sent
    );
}

#[tokio::test]
async fn retention_policy_discards_old_batch_before_upload() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "batch-expired".into(),
            },
            &[BatchEvent {
                event_id: "event-expired".into(),
                stable_id: "stable-expired".into(),
                label: "document:edit".into(),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                duration_seconds: 0,
            }],
        )
        .unwrap();
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = uploader.clone();

    UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    )
    .with_retention_policy(Arc::new(DiscardAll))
    .resume_pending("1", "0.1.0", &["document:edit".into()])
    .await
    .unwrap();

    assert_eq!(inspection.upload_count(), 0);
    assert!(repository.pending_batches().unwrap().is_empty());
}

#[tokio::test]
async fn raw_field_rejected_broadcasts_ipc_alert_and_rejects_batch() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch(&NewUploadBatch {
            batch_id: "batch-ipc-alert".into(),
        })
        .unwrap();
    let (alerts, mut receiver) = tokio::sync::broadcast::channel(1);
    let http = Arc::new(FakeHttp {
        requests: Arc::default(),
        responses: Arc::new(Mutex::new(
            vec![HttpResponse {
                status: 422,
                error_code: Some("raw_field_rejected".into()),
                tokens: None,
                retry_after: None,
                message: Some("safe rejection".into()),
                raw_body: None,
                user_id: None,
                device_id: None,
            }]
            .into(),
        )),
    });
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        HttpBatchUploader::new(http.clone()),
        IpcPrivacyAlertSink::new(alerts),
    );

    coordinator
        .upload_batch(BatchPayload::new(
            "batch-ipc-alert",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    assert_eq!(receiver.recv().await.unwrap().code, "raw_field_rejected");
    assert_eq!(http.requests.lock().unwrap().len(), 1);
    assert_eq!(
        repository.batch_status("batch-ipc-alert").unwrap(),
        UploadBatchStatus::Rejected
    );
    assert!(repository.resumable_batches(Utc::now()).unwrap().is_empty());
}

#[tokio::test]
async fn rate_limit_retry_after_is_persisted() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch(&NewUploadBatch {
            batch_id: "batch-rate-limited".into(),
        })
        .unwrap();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        FakeBatchUploader::with_outcomes(vec![UploadOutcome::RateLimited {
            retry_after: Some("600".into()),
        }]),
        FakePrivacyAlertSink::default(),
    )
    .with_host_and_backoff(
        "dev-api.getvelvt.com",
        HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0),
    );
    let before = Utc::now();

    coordinator
        .upload_batch(BatchPayload::new(
            "batch-rate-limited",
            "1",
            "0.1.0",
            vec!["document:edit".into()],
            "mvp-1",
            vec![event("one", 10)],
        ))
        .await
        .unwrap();

    assert!(repository
        .resumable_batches(before + chrono::Duration::seconds(500))
        .unwrap()
        .is_empty());
    assert_eq!(
        repository
            .resumable_batches(before + chrono::Duration::seconds(700))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn host_backoff_pauses_other_batches_for_same_host() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    for batch_id in ["batch-first", "batch-second"] {
        repository
            .insert_batch(&NewUploadBatch {
                batch_id: batch_id.into(),
            })
            .unwrap();
    }
    let uploader = FakeBatchUploader::with_outcomes(vec![
        UploadOutcome::RateLimited {
            retry_after: Some("600".into()),
        },
        UploadOutcome::Accepted,
    ]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    )
    .with_host_and_backoff(
        "dev-api.getvelvt.com",
        HostBackoff::new(Duration::from_secs(30), Duration::from_secs(900), || 1.0),
    );

    for batch_id in ["batch-first", "batch-second"] {
        coordinator
            .upload_batch(BatchPayload::new(
                batch_id,
                "1",
                "0.1.0",
                vec!["document:edit".into()],
                "mvp-1",
                vec![event(batch_id, 10)],
            ))
            .await
            .unwrap();
    }

    assert_eq!(inspection.upload_count(), 1);
    assert_eq!(
        repository.batch_status("batch-second").unwrap(),
        UploadBatchStatus::Failed
    );
}

#[tokio::test]
async fn upload_batcher_accumulates_abstracted_events_and_flushes_on_count() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    );
    let mut batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 1, Duration::from_secs(180)),
        coordinator,
    );
    let abstraction =
        AbstractionEngine::from_builtin_taxonomy(Arc::new(InMemoryMappingStore::default()))
            .unwrap()
            .process(RawEvent {
                event_id: uuid::Uuid::new_v4(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                app_name: "VS Code".into(),
                window_title: "private title".into(),
                bundle_id: None,
            })
            .unwrap();

    batcher
        .ingest_abstracted(
            "event-one",
            &abstraction,
            5,
            Utc.timestamp_opt(10, 0).unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(repository.pending_batches().unwrap().len(), 0);
}

#[tokio::test]
async fn upload_batcher_flush_now_drains_memory_and_resumes_ready_batches() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "batch-pending".into(),
            },
            &[BatchEvent {
                event_id: "event-pending".into(),
                stable_id: "stable-pending".into(),
                label: "document:edit".into(),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                occurred_at: Utc.timestamp_opt(9, 0).unwrap(),
                duration_seconds: 0,
            }],
        )
        .unwrap();
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted; 2]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(
        repository.clone(),
        uploader,
        FakePrivacyAlertSink::default(),
    );
    let mut batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 100, Duration::from_secs(180)),
        coordinator,
    );
    let abstraction =
        AbstractionEngine::from_builtin_taxonomy(Arc::new(InMemoryMappingStore::default()))
            .unwrap()
            .process(RawEvent {
                event_id: uuid::Uuid::new_v4(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                app_name: "VS Code".into(),
                window_title: "private title".into(),
                bundle_id: None,
            })
            .unwrap();

    batcher
        .ingest_abstracted(
            "event-buffered",
            &abstraction,
            5,
            Utc.timestamp_opt(10, 0).unwrap(),
        )
        .await
        .unwrap();

    assert!(batcher.flush_now().await.unwrap());
    assert_eq!(inspection.upload_count(), 2);
    assert_eq!(
        repository.batch_status("batch-pending").unwrap(),
        UploadBatchStatus::Sent
    );
    assert!(repository.pending_batches().unwrap().is_empty());
}

#[tokio::test]
async fn flush_now_requeues_events_after_a_persistence_failure() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository: Arc<dyn UploadBatchRepo> =
        Arc::new(FailFirstInsertRepo::new(database.upload_batch_repo()));
    let uploader = FakeBatchUploader::with_outcomes(vec![UploadOutcome::Accepted]);
    let inspection = uploader.clone();
    let coordinator = UploadCoordinator::new(repository, uploader, FakePrivacyAlertSink::default());
    let mut batcher = UploadBatcher::new(
        BatchAssembler::new("device-1", 100, Duration::from_secs(180)),
        coordinator,
    );
    let abstraction =
        AbstractionEngine::from_builtin_taxonomy(Arc::new(InMemoryMappingStore::default()))
            .unwrap()
            .process(RawEvent {
                event_id: uuid::Uuid::new_v4(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                app_name: "VS Code".into(),
                window_title: "private title".into(),
                bundle_id: None,
            })
            .unwrap();

    batcher
        .ingest_abstracted(
            "event-buffered",
            &abstraction,
            5,
            Utc.timestamp_opt(10, 0).unwrap(),
        )
        .await
        .unwrap();

    assert!(batcher.flush_now().await.is_err());
    assert!(batcher.flush_now().await.unwrap());
    assert_eq!(inspection.upload_count(), 1);
}

#[tokio::test]
async fn shared_flush_now_does_not_block_ingestion_during_persisted_replay() {
    let database = SqlitePersistence::open_in_memory().unwrap();
    let repository = database.upload_batch_repo();
    repository
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "batch-pending".into(),
            },
            &[BatchEvent {
                event_id: "event-pending".into(),
                stable_id: "stable-pending".into(),
                label: "document:edit".into(),
                category: "FOCUS_WORK".into(),
                taxonomy_version: "mvp-1".into(),
                occurred_at: Utc.timestamp_opt(9, 0).unwrap(),
                duration_seconds: 0,
            }],
        )
        .unwrap();
    let uploader = InFlightUploader {
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        uploads: Arc::new(Mutex::new(0)),
    };
    let started = Arc::clone(&uploader.started);
    let release = Arc::clone(&uploader.release);
    let shared = Arc::new(SharedUploadBatcher::new(UploadBatcher::new(
        BatchAssembler::new("device-1", 100, Duration::from_secs(180)),
        UploadCoordinator::new(repository, uploader, FakePrivacyAlertSink::default()),
    )));
    let abstraction =
        AbstractionEngine::from_builtin_taxonomy(Arc::new(InMemoryMappingStore::default()))
            .unwrap()
            .process(RawEvent {
                event_id: uuid::Uuid::new_v4(),
                occurred_at: Utc.timestamp_opt(10, 0).unwrap(),
                app_name: "VS Code".into(),
                window_title: "private title".into(),
                bundle_id: None,
            })
            .unwrap();
    let flush_task = tokio::spawn({
        let shared = Arc::clone(&shared);
        async move { shared.flush_now().await }
    });

    started.notified().await;
    let ingestion = tokio::time::timeout(
        Duration::from_millis(100),
        shared.ingest("event-buffered".into(), &abstraction, 5, Utc::now()),
    )
    .await;
    release.notify_one();
    flush_task.await.unwrap().unwrap();

    assert!(ingestion.is_ok());
    assert!(ingestion.unwrap().is_ok());
}
