use super::{
    BatchEventPayload, BatchPayload, BatchUploadError, BatchUploader, HostBackoff, UploadOutcome,
};
use crate::persistence::{BatchEvent, NewUploadBatch, PersistenceError, UploadBatchRepo};
use chrono::{Duration, Utc};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};

pub trait BatchRetentionPolicy: Send + Sync {
    fn should_discard(&self, batch: &crate::persistence::UploadBatch) -> bool;
}

#[derive(Debug, Default)]
pub struct KeepAllBatches;

impl BatchRetentionPolicy for KeepAllBatches {
    fn should_discard(&self, _batch: &crate::persistence::UploadBatch) -> bool {
        false
    }
}

pub trait PrivacyAlertSink: Send + Sync {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

#[derive(Clone)]
pub struct IpcPrivacyAlertSink {
    sender: tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>,
}

impl IpcPrivacyAlertSink {
    pub fn new(
        sender: tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>,
    ) -> Self {
        Self { sender }
    }
}

impl PrivacyAlertSink for IpcPrivacyAlertSink {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.sender.send(velvt_shared_types::PrivacyViolationAlert {
                code: "raw_field_rejected".into(),
                message: message.to_owned(),
            });
        })
    }
}

#[derive(Clone, Default)]
pub struct FakePrivacyAlertSink(Arc<Mutex<Vec<String>>>);

impl FakePrivacyAlertSink {
    pub fn alert_count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

impl PrivacyAlertSink for FakePrivacyAlertSink {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(message.to_owned());
        })
    }
}

pub struct UploadCoordinator<U, A> {
    repository: Arc<dyn UploadBatchRepo>,
    uploader: U,
    alerts: A,
    host: String,
    backoff: Mutex<HostBackoff>,
    retention: Arc<dyn BatchRetentionPolicy>,
}

impl<U, A> UploadCoordinator<U, A>
where
    U: BatchUploader,
    A: PrivacyAlertSink,
{
    pub fn new(repository: Arc<dyn UploadBatchRepo>, uploader: U, alerts: A) -> Self {
        Self {
            repository,
            uploader,
            alerts,
            host: "dev-api.getvelvt.com".into(),
            backoff: Mutex::new(HostBackoff::production(
                StdDuration::from_secs(30),
                StdDuration::from_secs(15 * 60),
            )),
            retention: Arc::new(KeepAllBatches),
        }
    }

    pub fn with_host_and_backoff(mut self, host: impl Into<String>, backoff: HostBackoff) -> Self {
        self.host = host.into();
        self.backoff = Mutex::new(backoff);
        self
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    pub fn with_retention_policy(mut self, retention: Arc<dyn BatchRetentionPolicy>) -> Self {
        self.retention = retention;
        self
    }

    pub async fn upload_batch(&self, batch: BatchPayload) -> Result<(), CoordinatorError> {
        if let Some(next_attempt_at) = self.repository.host_backoff_until(&self.host)? {
            if next_attempt_at > Utc::now() {
                self.repository
                    .mark_failed(&batch.batch_id, next_attempt_at, "host_backoff")?;
                return Ok(());
            }
        }
        let outcome = match self.uploader.upload(&batch).await {
            Ok(outcome) => outcome,
            Err(BatchUploadError::Transport) => {
                self.schedule_network_retry(&batch.batch_id)?;
                return Ok(());
            }
        };
        match outcome {
            UploadOutcome::Accepted | UploadOutcome::Duplicate => {
                self.repository.mark_sent(&batch.batch_id)?;
                self.backoff.lock().unwrap().reset(&self.host);
                self.repository.clear_host_backoff(&self.host)?;
            }
            UploadOutcome::RawFieldRejected { message } => {
                // SECURITY: raw_field_rejected is a hard privacy signal. It is
                // permanently terminal and must never enter retry scheduling.
                self.repository
                    .mark_rejected(&batch.batch_id, "raw_field_rejected")?;
                tracing::error!(
                    error_code = "raw_field_rejected",
                    batch_id = batch.batch_id,
                    rejection_message = message,
                    "cloud rejected a batch for a privacy violation"
                );
                self.alerts.alert(&message).await;
            }
            UploadOutcome::RateLimited { retry_after } => {
                let attempt = self.repository.host_backoff_attempt(&self.host)?;
                self.backoff
                    .lock()
                    .unwrap()
                    .set_attempt(&self.host, attempt);
                let delay = self
                    .backoff
                    .lock()
                    .unwrap()
                    .next_delay(&self.host, retry_after.as_deref());
                let next_attempt_at = Utc::now()
                    + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
                self.repository.set_host_backoff(
                    &self.host,
                    attempt.saturating_add(1),
                    next_attempt_at,
                )?;
                self.repository
                    .mark_failed(&batch.batch_id, next_attempt_at, "rate_limited")?;
            }
            UploadOutcome::Retryable { code } => {
                let attempt = self.repository.host_backoff_attempt(&self.host)?;
                self.backoff
                    .lock()
                    .unwrap()
                    .set_attempt(&self.host, attempt);
                let delay = self.backoff.lock().unwrap().next_delay(&self.host, None);
                let next_attempt_at = Utc::now()
                    + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
                self.repository.set_host_backoff(
                    &self.host,
                    attempt.saturating_add(1),
                    next_attempt_at,
                )?;
                self.repository
                    .mark_failed(&batch.batch_id, next_attempt_at, &code)?;
            }
        }
        Ok(())
    }

    pub async fn submit_batch(&self, batch: BatchPayload) -> Result<(), CoordinatorError> {
        self.persist_batch(&batch)?;
        self.upload_batch(batch).await
    }

    pub(crate) fn persist_batch(&self, batch: &BatchPayload) -> Result<(), CoordinatorError> {
        let events = batch
            .events
            .iter()
            .map(|event| BatchEvent {
                event_id: event.event_id.clone(),
                stable_id: event.stable_id.clone(),
                label: event.label.clone(),
                category: event.category.clone(),
                taxonomy_version: event.taxonomy_version.clone(),
                occurred_at: event.occurred_at,
                duration_seconds: event.duration_seconds,
            })
            .collect::<Vec<_>>();
        self.repository.insert_batch_with_events(
            &NewUploadBatch {
                batch_id: batch.batch_id.clone(),
            },
            &events,
        )?;
        Ok(())
    }

    pub async fn resume_pending(
        &self,
        schema_version: &str,
        client_version: &str,
        supported_abstraction_types: &[String],
    ) -> Result<usize, CoordinatorError> {
        let batches = self.repository.resumable_batches(Utc::now())?;
        let count = batches.len();
        for batch in batches {
            if self.retention.should_discard(&batch) {
                self.repository.discard_batch(&batch.batch_id)?;
                tracing::info!(
                    batch_id = batch.batch_id,
                    reason = "retention_boundary",
                    "discarded upload batch before upload"
                );
                continue;
            }
            let taxonomy = batch
                .events
                .first()
                .map(|event| event.taxonomy_version.clone())
                .unwrap_or_default();
            let events = batch
                .events
                .into_iter()
                .map(|event| BatchEventPayload {
                    event_id: event.event_id,
                    stable_id: event.stable_id,
                    label: event.label,
                    category: event.category,
                    taxonomy_version: event.taxonomy_version,
                    occurred_at: event.occurred_at,
                    duration_seconds: event.duration_seconds,
                })
                .collect();
            self.upload_batch(BatchPayload::new(
                batch.batch_id,
                schema_version,
                client_version,
                supported_abstraction_types.to_vec(),
                taxonomy,
                events,
            ))
            .await?;
        }
        Ok(count)
    }

    fn schedule_network_retry(&self, batch_id: &str) -> Result<(), CoordinatorError> {
        let attempt = self.repository.host_backoff_attempt(&self.host)?;
        self.backoff
            .lock()
            .unwrap()
            .set_attempt(&self.host, attempt);
        let delay = self.backoff.lock().unwrap().next_delay(&self.host, None);
        let next_attempt_at =
            Utc::now() + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
        self.repository
            .set_host_backoff(&self.host, attempt.saturating_add(1), next_attempt_at)?;
        self.repository
            .mark_pending_retry(batch_id, next_attempt_at, "transport")?;
        Ok(())
    }

    pub async fn run_retry_loop(
        &self,
        interval: StdDuration,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        schema_version: &str,
        client_version: &str,
        supported_abstraction_types: &[String],
    ) {
        loop {
            if *shutdown.borrow() {
                return;
            }
            if self
                .resume_pending(schema_version, client_version, supported_abstraction_types)
                .await
                .is_err()
            {
                tracing::error!(
                    error_code = "upload_recovery_failed",
                    "failed to resume persisted upload batches"
                );
            }
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    #[error(transparent)]
    Upload(#[from] BatchUploadError),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}
