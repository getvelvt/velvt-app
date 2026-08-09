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

    /// Re-ingests upload-eligible rows that never reached a batch.
    ///
    /// Between an ack and the next flush, an event lives only in the in-memory
    /// assembler; `resume_pending` reads `upload_batch`, so nothing re-batched
    /// these after a hard kill and raw-event retention eventually deleted them.
    /// Called once at startup, before live ingestion begins.
    ///
    /// The assembler is the only writer of batch identity, so re-pushing an
    /// event that a concurrent flush had already persisted would duplicate it —
    /// which is why this runs before the router and flush task are wired up.
    ///
    /// Recovers at most `limit` events per start, newest first, so a pathological
    /// backlog cannot stall startup. Anything beyond that stays queued and is
    /// picked up by the next start rather than being dropped: raw-event
    /// retention spares unbatched eligible rows for the same reason.
    pub async fn recover_unbatched(
        &mut self,
        raw_events: &dyn crate::persistence::RawEventRepo,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<usize, CoordinatorError> {
        let pending = raw_events.unbatched_events(limit)?;
        if pending.is_empty() {
            return Ok(0);
        }
        let recovered = pending.len();
        // Oldest first, so a partial recovery still uploads in event order.
        for entry in pending.into_iter().rev() {
            let event = BatchEventPayload {
                event_id: entry.event_id,
                stable_id: entry.stable_id,
                label: entry.label,
                category: entry.category,
                taxonomy_version: entry.taxonomy_version,
                classification_tier: entry.classification_tier,
                occurred_at: entry.occurred_at,
                duration_seconds: entry.duration_seconds,
            };
            if let Some(batch) = self.assembler.push(event, now) {
                self.submit(batch).await?;
            }
        }
        tracing::info!(
            recovered_events = recovered,
            "re-ingested upload-eligible events that were never batched"
        );
        Ok(recovered)
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
        if let Some(batch) = self.assembler.flush_shutdown() {
            if let Err(error) = self.coordinator.persist_batch(&batch) {
                Self::log_submit_failure(&error);
                self.assembler.requeue(batch);
                return Err(error);
            }
            if let Err(error) = self
                .coordinator
                .flush_all_pending("1", env!("CARGO_PKG_VERSION"))
                .await
            {
                Self::log_submit_failure(&error);
                return Err(error);
            }
            return Ok(true);
        }
        self.coordinator
            .flush_all_pending("1", env!("CARGO_PKG_VERSION"))
            .await?;
        Ok(false)
    }

    async fn submit(&mut self, batch: super::BatchPayload) -> Result<(), CoordinatorError> {
        // Persist before upload, and requeue on persist failure: the batch
        // was already taken out of the assembler, so dropping it here would
        // lose events the client has been acked for. Once persisted the
        // batch is durable — an upload failure lands in the pending-retry
        // path and is resumed later, so no requeue is needed there.
        if let Err(error) = self.coordinator.persist_batch(&batch) {
            Self::log_submit_failure(&error);
            self.assembler.requeue(batch);
            return Err(error);
        }
        if let Err(error) = self.coordinator.upload_batch(batch).await {
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
            if let Some(batch) = batch {
                if let Err(error) = coordinator.persist_batch(&batch) {
                    UploadBatcher::<U, A>::log_submit_failure(&error);
                    self.inner.lock().await.assembler.requeue(batch);
                    return Err(error);
                }
                if let Err(error) = coordinator
                    .flush_all_pending("1", env!("CARGO_PKG_VERSION"))
                    .await
                {
                    UploadBatcher::<U, A>::log_submit_failure(&error);
                    return Err(error);
                }
                return Ok(true);
            }
            coordinator
                .flush_all_pending("1", env!("CARGO_PKG_VERSION"))
                .await?;
            Ok(false)
        })
    }
}
