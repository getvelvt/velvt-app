//! History and insight fetch, cache, and delivery pipeline (R6).
//!
//! The public interface R7 depends on is `CacheManager`.  Everything else —
//! `FetchService`, `FetchScheduler`, and the parser — is internal to this
//! module and wired together in `main.rs`.

mod cache;
mod fetch;
mod parser;
mod scheduler;

pub use cache::{CacheError, CacheManager, FakeCacheManager};
pub use fetch::{FetchConfig, FetchError, FetchService, Fetchable};
pub use scheduler::FetchScheduler;
