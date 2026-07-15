//! History and insight fetch, cache, and delivery pipeline (R6 + R7).
//!
//! R6 public surface: `CacheManager`, `FetchService`, `FetchScheduler`.
//! R7 public surface: `PushAdapter`, `PushQueue`, `PushConfig`,
//! `PushAdapterAlertSink`, and the shaper types.

mod cache;
mod fetch;
mod parser;
pub mod poll;
pub mod push;
mod rehydrate;
mod scheduler;
pub mod shaper;

pub use cache::{CacheError, CacheManager, FakeCacheManager};
pub use fetch::{FetchConfig, FetchError, FetchService, Fetchable};
pub use parser::{parse_insight, parse_insight_with_rehydrator};
pub use poll::{PollClient, PollConfig, PollScheduler};
#[cfg(any(test, feature = "test-helpers"))]
pub use push::FakePushAdapter;
pub use push::{PushAdapter, PushAdapterAlertSink, PushConfig, PushQueue};
pub use rehydrate::{InsightLabelReference, LocalInsightRehydrator};
pub use scheduler::FetchScheduler;
pub use shaper::{ValidatePayload, ValidatedPayload, ValidationError};
