use super::{
    BatchAssembler, BatchEventPayload, BatchUploader, CoordinatorError, PrivacyAlertSink,
    UploadCoordinator,
};
use crate::abstraction::AbstractedEvent;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

pub struct UploadBatcher<U, A> {
    assembler: BatchAssembler,
    coordinator: Arc<UploadCoordinator<U, A>>,
}

impl<U, A> UploadBatcher<U, A>
where
    U: BatchUploader,
    A: PrivacyAlertSink,
{
    pub fn new(assembler: BatchAssembler, coordinator: UploadCoordinator<U, A>) -> Self {
        Self {
            assembler,
            coordinator: Arc::new(coordinator),
        }
    }

    pub async fn ingest_abstracted(
        &mut self,
        event_id: impl Into<String>,
        event: &AbstractedEvent,
        duration_seconds: u64,
        now: DateTime<Utc>,
    ) -> Result<(), CoordinatorError> {
        let event = BatchEventPayload::from_abstracted(event_id, event, duration_seconds);
        if let Some(batch) = self.assembler.push(event, now) {
            self.submit(batch).await?;
        }
        Ok(())
    }

    pub async fn flush_due(&mut self, now: DateTime<Utc>) -> Result<bool, CoordinatorError> {
        let Some(batch) = self.assembler.flush_due(now) else {
            return Ok(false);
        };
        self.submit(batch).await?;
        Ok(true)
    }

    pub async fn flush_sleep(&mut self) -> Result<bool, CoordinatorError> {
        let Some(batch) = self.assembler.flush_sleep() else {
            return Ok(false);
        };
        self.submit(batch).await?;
        Ok(true)
    }

    pub async fn flush_shutdown(&mut self) -> Result<bool, CoordinatorError> {
        let Some(batch) = self.assembler.flush_shutdown() else {
            return Ok(false);
        };
        self.submit(batch).await?;
        Ok(true)
    }

    pub async fn flush_now(&mut self) -> Result<bool, CoordinatorError> {
        let flushed = if let Some(batch) = self.assembler.flush_shutdown() {
            if let Err(error) = self.coordinator.persist_batch(&batch) {
                Self::log_submit_failure(&error);
                self.assembler.requeue(batch);
                return Err(error);
            }
            if let Err(error) = self.coordinator.upload_batch(batch).await {
                Self::log_submit_failure(&error);
                return Err(error);
            }
            true
        } else {
            false
        };
        self.coordinator
            .resume_pending("1", env!("CARGO_PKG_VERSION"), &["document:edit".into()])
            .await?;
        Ok(flushed)
    }

    async fn submit(&self, batch: super::BatchPayload) -> Result<(), CoordinatorError> {
        Self::submit_with(&self.coordinator, batch).await
    }

    async fn submit_with(
        coordinator: &UploadCoordinator<U, A>,
        batch: super::BatchPayload,
    ) -> Result<(), CoordinatorError> {
        if let Err(error) = coordinator.submit_batch(batch).await {
            Self::log_submit_failure(&error);
            return Err(error);
        }
        Ok(())
    }

    fn log_submit_failure(error: &CoordinatorError) {
        tracing::error!(
            error_code = "upload_batch_submit_failed",
            error = %error,
            "failed to persist or submit upload batch"
        );
    }
}

/// Object-safe facade over [`UploadBatcher`] so the IPC router can hold one
/// behind `Arc<dyn EventIngestor>` without threading the uploader/alert-sink
/// generics through `R7Router`.
pub trait EventIngestor: Send + Sync {
    fn ingest<'a>(
        &'a self,
        event_id: String,
        event: &'a AbstractedEvent,
        duration_seconds: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorError>> + Send + 'a>>;

    fn flush_due<'a>(
        &'a self,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>>;

    fn flush_shutdown<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>>;

    fn flush_now<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>>;
}

/// Shares one [`UploadBatcher`] between the IPC router (live ingestion) and
/// a periodic flush task behind a single async mutex.
pub struct SharedUploadBatcher<U, A> {
    inner: AsyncMutex<UploadBatcher<U, A>>,
    flush_gate: AsyncMutex<()>,
}

impl<U, A> SharedUploadBatcher<U, A> {
    pub fn new(batcher: UploadBatcher<U, A>) -> Self {
        Self {
            inner: AsyncMutex::new(batcher),
            flush_gate: AsyncMutex::new(()),
        }
    }
}

impl<U, A> EventIngestor for SharedUploadBatcher<U, A>
where
    U: BatchUploader,
    A: PrivacyAlertSink,
{
    fn ingest<'a>(
        &'a self,
        event_id: String,
        event: &'a AbstractedEvent,
        duration_seconds: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorError>> + Send + 'a>> {
        Box::pin(async move {
            self.inner
                .lock()
                .await
                .ingest_abstracted(event_id, event, duration_seconds, now)
                .await
        })
    }

    fn flush_due<'a>(
        &'a self,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async move { self.inner.lock().await.flush_due(now).await })
    }

    fn flush_shutdown<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async move { self.inner.lock().await.flush_shutdown().await })
    }

    fn flush_now<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<bool, CoordinatorError>> + Send + 'a>> {
        Box::pin(async move {
            let _flush_gate = self.flush_gate.lock().await;
            let (batch, coordinator) = {
                let mut batcher = self.inner.lock().await;
                (
                    batcher.assembler.flush_shutdown(),
                    Arc::clone(&batcher.coordinator),
                )
            };
            let flushed = if let Some(batch) = batch {
                if let Err(error) = coordinator.persist_batch(&batch) {
                    UploadBatcher::<U, A>::log_submit_failure(&error);
                    self.inner.lock().await.assembler.requeue(batch);
                    return Err(error);
                }
                if let Err(error) = coordinator.upload_batch(batch).await {
                    UploadBatcher::<U, A>::log_submit_failure(&error);
                    return Err(error);
                }
                true
            } else {
                false
            };
            coordinator
                .resume_pending("1", env!("CARGO_PKG_VERSION"), &["document:edit".into()])
                .await?;
            Ok(flushed)
        })
    }
}
