//! History and insight fetch, cache, and delivery pipeline (R6 + R7).
//!
//! R6 public surface: `CacheManager`, `FetchService`, `FetchScheduler`.
//! R7 public surface: `PushAdapter`, `PushQueue`, `PushConfig`,
//! `PushAdapterAlertSink`, and the shaper types.

mod cache;
mod fetch;
mod parser;
pub mod push;
mod scheduler;
pub mod shaper;

pub use cache::{CacheError, CacheManager, FakeCacheManager};
pub use fetch::{FetchConfig, FetchError, FetchService, Fetchable};
#[cfg(any(test, feature = "test-helpers"))]
pub use push::FakePushAdapter;
pub use push::{PushAdapter, PushAdapterAlertSink, PushConfig, PushQueue};
pub use scheduler::FetchScheduler;
pub use shaper::{ValidatePayload, ValidatedPayload, ValidationError};
