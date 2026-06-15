//! Privacy-safe upload batching, transport, and retry policy.

mod assembly;
mod coordinator;
mod dto;
mod retry;
mod runtime;
mod transport;

pub use assembly::BatchAssembler;
pub use coordinator::{
    BatchRetentionPolicy, CoordinatorError, FakePrivacyAlertSink, IpcPrivacyAlertSink,
    KeepAllBatches, PrivacyAlertSink, UploadCoordinator,
};
pub use dto::{BatchEventPayload, BatchPayload};
pub use retry::HostBackoff;
pub use runtime::UploadBatcher;
pub use transport::{
    BatchUploadError, BatchUploader, FakeBatchUploader, HttpBatchUploader, UploadOutcome,
};
