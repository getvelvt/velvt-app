//! Privacy-safe upload batching and cloud HTTP interfaces.
//!
//! This module owns batch assembly, retry outcomes, and cloud event upload. It
//! accepts only abstracted events and does not accept or serialize raw events.

#![allow(async_fn_in_trait)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::abstraction::AbstractedEvent;

/// Coordinates threshold, interval, and shutdown-triggered uploads.
pub trait UploadBatcher {
    /// Flushes pending privacy-safe events.
    async fn flush(&self) -> Result<UploadOutcome, UploadError>;
}

/// Assembles privacy-safe events into an idempotent upload batch.
pub trait BatchAssembler {
    /// Builds one upload batch from privacy-safe abstracted events.
    fn assemble(&self, events: Vec<AbstractedEvent>) -> Result<UploadBatch, UploadError>;
}

/// Sends privacy-safe batches to the cloud API.
pub trait UploadClient {
    /// Uploads one idempotent batch.
    async fn upload(&self, batch: &UploadBatch) -> Result<UploadOutcome, UploadError>;
}

/// Privacy-safe idempotent cloud upload batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadBatch {
    /// Stable retry-safe batch identifier.
    pub batch_id: Uuid,
    /// UTC batch creation time.
    pub created_at: DateTime<Utc>,
    /// Privacy-safe abstracted events.
    pub events: Vec<AbstractedEvent>,
}

/// Result of an upload attempt.
#[derive(Debug, Clone)]
pub enum UploadOutcome {
    /// Batch was accepted, including duplicate-id success.
    Accepted,
    /// Authentication refresh is required before retrying.
    AuthenticationRequired,
    /// Device access is revoked and upload must pause.
    DeviceRevoked,
}

/// Errors produced while assembling or uploading batches.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// Batch assembly failed.
    #[error("batch assembly failed")]
    Assembly,
    /// Cloud transport failed.
    #[error("upload transport failed")]
    Transport,
    /// Cloud rejected a forbidden raw field.
    #[error("cloud rejected a raw field")]
    RawFieldRejected,
}
