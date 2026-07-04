//! CacheManager trait — the only interface R7 uses to read history and insight
//! data.  R7 must not know whether data came from a live API call or the cache.

use std::{future::Future, pin::Pin};

use chrono::NaiveDate;
use velvt_shared_types::{HistoryPayload, InsightPayload};

use crate::{auth::HttpClient, persistence::PersistenceError};

use super::fetch::{FetchError, FetchService};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("fetch error: {0}")]
    Fetch(#[from] FetchError),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// The sole interface R7 uses to read history and insight data.
///
/// Methods use explicit `Pin<Box<dyn Future>>` so the trait is object-safe and
/// can be stored as `Arc<dyn CacheManager>`.
pub trait CacheManager: Send + Sync {
    /// Returns the history summary for the last `days` days.
    /// Fetches from the API on a cache miss; the caller cannot observe whether
    /// data was served from cache or the network.
    fn daily_history<'a>(
        &'a self,
        days: u8,
    ) -> Pin<Box<dyn Future<Output = Result<HistoryPayload, CacheError>> + Send + 'a>>;

    /// Returns the insight for `date`, or `None` when none exists on the server.
    fn daily_insight<'a>(
        &'a self,
        date: NaiveDate,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InsightPayload>, CacheError>> + Send + 'a>>;

    /// Drops the cached history for a specific date, or all dates when `None`.
    fn invalidate_history<'a>(
        &'a self,
        date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>>;

    /// Drops the cached insight for a specific date, or all dates when `None`.
    fn invalidate_insights<'a>(
        &'a self,
        date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>>;

    /// Drops all history and insight cache entries.  Triggers full API refresh
    /// on the next read.  Call after a successful batch upload that may have
    /// caused the server to generate new insights.
    fn invalidate_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// FetchService<H> implements CacheManager
// ---------------------------------------------------------------------------

impl<H: HttpClient + 'static> CacheManager for FetchService<H> {
    fn daily_history<'a>(
        &'a self,
        days: u8,
    ) -> Pin<Box<dyn Future<Output = Result<HistoryPayload, CacheError>> + Send + 'a>> {
        Box::pin(async move { self.daily_history(days).await.map_err(CacheError::Fetch) })
    }

    fn daily_insight<'a>(
        &'a self,
        date: NaiveDate,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InsightPayload>, CacheError>> + Send + 'a>> {
        Box::pin(async move { self.daily_insight(date).await.map_err(CacheError::Fetch) })
    }

    fn invalidate_history<'a>(
        &'a self,
        date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move {
            match date {
                Some(d) => self
                    .invalidate_history_date(d)
                    .map_err(CacheError::Persistence),
                None => self
                    .invalidate_history_all()
                    .map_err(CacheError::Persistence),
            }
        })
    }

    fn invalidate_insights<'a>(
        &'a self,
        date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move {
            match date {
                Some(d) => self
                    .invalidate_insight_date(d)
                    .map_err(CacheError::Persistence),
                None => self
                    .invalidate_insight_all()
                    .map_err(CacheError::Persistence),
            }
        })
    }

    fn invalidate_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move {
            self.invalidate_history_all()
                .map_err(CacheError::Persistence)?;
            self.invalidate_insight_all()
                .map_err(CacheError::Persistence)
        })
    }
}

// ---------------------------------------------------------------------------
// FakeCacheManager (for R7 tests)
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::Mutex;

/// Test double for `CacheManager`.  Pre-load history and insight values; all
/// invalidation calls are no-ops.  Counts calls for assertion.
pub struct FakeCacheManager {
    history: Mutex<HashMap<u8, HistoryPayload>>,
    insights: Mutex<HashMap<String, Option<InsightPayload>>>,
    call_count: Mutex<usize>,
}

impl FakeCacheManager {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(HashMap::new()),
            insights: Mutex::new(HashMap::new()),
            call_count: Mutex::new(0),
        }
    }

    pub fn with_history(self, days: u8, payload: HistoryPayload) -> Self {
        self.history.lock().unwrap().insert(days, payload);
        self
    }

    pub fn with_insight(self, date: NaiveDate, payload: Option<InsightPayload>) -> Self {
        self.insights
            .lock()
            .unwrap()
            .insert(date.format("%Y-%m-%d").to_string(), payload);
        self
    }

    pub fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

impl Default for FakeCacheManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheManager for FakeCacheManager {
    fn daily_history<'a>(
        &'a self,
        days: u8,
    ) -> Pin<Box<dyn Future<Output = Result<HistoryPayload, CacheError>> + Send + 'a>> {
        *self.call_count.lock().unwrap() += 1;
        let result = self
            .history
            .lock()
            .unwrap()
            .get(&days)
            .cloned()
            .unwrap_or_else(|| HistoryPayload {
                days: days as u32,
                summaries: vec![],
            });
        Box::pin(async move { Ok(result) })
    }

    fn daily_insight<'a>(
        &'a self,
        date: NaiveDate,
    ) -> Pin<Box<dyn Future<Output = Result<Option<InsightPayload>, CacheError>> + Send + 'a>> {
        *self.call_count.lock().unwrap() += 1;
        let key = date.format("%Y-%m-%d").to_string();
        let result = self.insights.lock().unwrap().get(&key).cloned().flatten();
        Box::pin(async move { Ok(result) })
    }

    fn invalidate_history<'a>(
        &'a self,
        _date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    fn invalidate_insights<'a>(
        &'a self,
        _date: Option<NaiveDate>,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    fn invalidate_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), CacheError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use velvt_shared_types::{ConfidenceLevel, DailySummary, HistoryStatus};

    fn make_history(days: u8) -> HistoryPayload {
        let today = chrono::Utc::now().date_naive();
        HistoryPayload {
            days: days as u32,
            summaries: vec![DailySummary {
                date: today,
                status: HistoryStatus::Ready,
                event_count: 5,
                active_seconds: 1800,
                confidence_level: ConfidenceLevel::Medium,
                focus_score: None,
                fragmentation_score: None,
                baseline_status: "early_stage".into(),
                baseline_comparison: serde_json::json!({ "status": "early_stage" }),
                type_proportions: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn fake_cache_manager_returns_preconfigured_history() {
        let cache = Arc::new(FakeCacheManager::new().with_history(7, make_history(7)));
        let result = cache.daily_history(7).await.unwrap();
        assert_eq!(result.days, 7);
        assert_eq!(result.summaries.len(), 1);
        assert_eq!(cache.call_count(), 1);
    }

    #[tokio::test]
    async fn fake_cache_manager_returns_preconfigured_insight() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 6, 14).unwrap();
        let insight = InsightPayload {
            date,
            text: "Some insight".into(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: chrono::Utc::now(),
        };
        let cache = Arc::new(FakeCacheManager::new().with_insight(date, Some(insight.clone())));
        let result = cache.daily_insight(date).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().text, insight.text);
    }

    #[tokio::test]
    async fn fake_cache_manager_invalidate_all_is_noop() {
        let cache = FakeCacheManager::new();
        cache.invalidate_all().await.unwrap();
    }

    #[tokio::test]
    async fn fake_cache_manager_is_dyn_compatible() {
        let cache: Arc<dyn CacheManager> = Arc::new(FakeCacheManager::new());
        let _ = cache.daily_history(7).await;
    }
}
