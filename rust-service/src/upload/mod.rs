//! Privacy-safe upload batching, transport, and retry policy.

mod assembly;
mod coordinator;
mod dto;
mod retry;
mod runtime;
mod transport;

pub use assembly::BatchAssembler;
pub use coordinator::{
    payload_for_queued_batch, BatchDisposition, BatchOwnershipPolicy, CoordinatorError,
    FakePrivacyAlertSink, IpcPrivacyAlertSink, PrivacyAlertSink, SendEveryBatch, SessionOwnership,
    UploadCoordinator,
};
pub use dto::{BatchEventPayload, BatchPayload};
pub use retry::HostBackoff;
pub use runtime::{destroy_upload_queue, EventIngestor, SharedUploadBatcher, UploadBatcher};
pub use transport::{
    BatchUploadError, BatchUploader, FakeBatchUploader, HttpBatchUploader, UploadOutcome,
};
