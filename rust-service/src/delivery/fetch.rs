//! Cache-first fetch service for daily history and insights.
//!
//! `FetchService` is the single implementation of `CacheManager`.  On every
//! request it checks the SQLite cache first (with a configurable read timeout
//! so it can never block the IPC path), then falls through to the cloud API on
//! a miss.  A 404 from the insight endpoint is stored as a negative cache
//! entry so the API is not hammered on every poll cycle.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

use chrono::{Duration as ChronoDuration, NaiveDate, Utc};

use crate::{
    auth::{AuthError, HttpClient, HttpRequest},
    persistence::{
        HistoryCacheEntry, HistoryCacheRepo, InsightCacheEntry, InsightCacheRepo, PersistenceError,
    },
};
use velvt_shared_types::{DailySummary, HistoryPayload, InsightPayload};

use super::parser::{self, ParseError};
use super::push::PushAdapter;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("API request failed: {0}")]
    Http(#[from] AuthError),
    #[error("API response could not be parsed: {0}")]
    InvalidResponse(#[from] ParseError),
    #[error("API returned unexpected status {status}")]
    ApiError { status: u16 },
    #[error("cache persistence error: {0}")]
    Persistence(#[from] PersistenceError),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct FetchConfig {
    pub history_ttl: Duration,
    pub insight_ttl: Duration,
    pub insight_negative_ttl: Duration,
    /// Maximum time a blocking cache read may take before being treated as a miss.
    pub read_timeout: Duration,
}

// ---------------------------------------------------------------------------
// Fetchable — internal trait used by the scheduler
// ---------------------------------------------------------------------------

pub trait Fetchable: Send + Sync {
    fn fetch_all<'a>(&'a self, days: u8) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

// ---------------------------------------------------------------------------
// FetchService
// ---------------------------------------------------------------------------

pub struct FetchService<H> {
    http: Arc<H>,
    history_cache: Arc<dyn HistoryCacheRepo>,
    insight_cache: Arc<dyn InsightCacheRepo>,
    config: FetchConfig,
    /// Per-`days` lock: deduplicates concurrent history fetches for the same window.
    inflight_history: std::sync::Mutex<HashMap<u8, Weak<AsyncMutex<()>>>>,
    /// Per-date lock: deduplicates concurrent insight fetches for the same date.
    inflight_insight: std::sync::Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    push_adapter: Option<Arc<PushAdapter>>,
}

impl<H: HttpClient> FetchService<H> {
    pub fn new(
        http: Arc<H>,
        history_cache: Arc<dyn HistoryCacheRepo>,
        insight_cache: Arc<dyn InsightCacheRepo>,
        config: FetchConfig,
    ) -> Self {
        Self {
            http,
            history_cache,
            insight_cache,
            config,
            inflight_history: std::sync::Mutex::new(HashMap::new()),
            inflight_insight: std::sync::Mutex::new(HashMap::new()),
            push_adapter: None,
        }
    }

    /// Attaches a push adapter; called after construction so existing tests
    /// using `FetchService::new()` need no changes.
    pub fn with_push_adapter(mut self, adapter: Arc<PushAdapter>) -> Self {
        self.push_adapter = Some(adapter);
        self
    }

    // -----------------------------------------------------------------------
    // In-flight deduplication helpers
    // -----------------------------------------------------------------------

    /// Returns a shared `Arc<AsyncMutex<()>>` for `key`, reusing a still-live
    /// lock or creating a new one.  Callers must hold the returned `Arc` for
    /// the entire duration of the fetch so concurrent callers block on it.
    fn inflight_lock<K: Eq + std::hash::Hash + Clone>(
        map: &std::sync::Mutex<HashMap<K, Weak<AsyncMutex<()>>>>,
        key: &K,
    ) -> Arc<AsyncMutex<()>> {
        let mut guard = map.lock().unwrap();
        if let Some(weak) = guard.get(key) {
            if let Some(arc) = weak.upgrade() {
                return arc;
            }
        }
        let arc = Arc::new(AsyncMutex::new(()));
        guard.insert(key.clone(), Arc::downgrade(&arc));
        arc
    }

    /// Returns all summaries from cache for `dates`, or `None` on any miss or
    /// unparseable payload so the caller can fall through to the API.
    async fn all_history_cached(&self, dates: &[NaiveDate]) -> Option<Vec<DailySummary>> {
        let mut cached = Vec::with_capacity(dates.len());
        for date in dates {
            let date_str = date.format("%Y-%m-%d").to_string();
            match self.read_history_cached(&date_str).await {
                Some(entry) => match serde_json::from_str::<DailySummary>(&entry.payload) {
                    Ok(summary) => cached.push(summary),
                    Err(_) => return None,
                },
                None => return None,
            }
        }
        Some(cached)
    }

    // -----------------------------------------------------------------------
    // Public API methods (also used by CacheManager impl in cache.rs)
    // -----------------------------------------------------------------------

    pub async fn daily_history(&self, days: u8) -> Result<HistoryPayload, FetchError> {
        let today = Utc::now().date_naive();
        let dates: Vec<NaiveDate> = (0..days as i64)
            .filter_map(|i| today.checked_sub_signed(ChronoDuration::days(i)))
            .collect();

        // Fast path: serve entirely from cache.
        if let Some(mut summaries) = self.all_history_cached(&dates).await {
            summaries.sort_by_key(|s| s.date);
            return Ok(HistoryPayload {
                days: days as u32,
                summaries,
            });
        }

        // Slow path: acquire per-window lock to deduplicate concurrent fetches.
        let lock = Self::inflight_lock(&self.inflight_history, &days);
        let _guard = lock.lock().await;

        // Recheck after acquiring the lock: another task may have populated it.
        if let Some(mut summaries) = self.all_history_cached(&dates).await {
            summaries.sort_by_key(|s| s.date);
            return Ok(HistoryPayload {
                days: days as u32,
                summaries,
            });
        }

        // Fetch from the API.
        let summaries = self.fetch_history_from_api(days).await?;
        let expires_at = Utc::now() + to_chrono(&self.config.history_ttl);
        for summary in &summaries {
            let payload = serde_json::to_string(summary).unwrap_or_default();
            self.history_cache.upsert(&HistoryCacheEntry {
                date: summary.date.format("%Y-%m-%d").to_string(),
                payload,
                expires_at,
            })?;
        }
        let mut sorted = summaries;
        sorted.sort_by_key(|s| s.date);
        let result = HistoryPayload {
            days: days as u32,
            summaries: sorted,
        };
        if let Some(adapter) = &self.push_adapter {
            adapter.push_history(result.clone()).await;
        }
        Ok(result)
    }

    pub async fn daily_insight(
        &self,
        date: NaiveDate,
    ) -> Result<Option<InsightPayload>, FetchError> {
        let date_str = date.format("%Y-%m-%d").to_string();

        // Fast path: check cache without acquiring the in-flight lock.
        if let Some(entry) = self.read_insight_cached(&date_str).await {
            if entry.is_negative {
                return Ok(None);
            }
            if let Ok(insight) = serde_json::from_str::<InsightPayload>(&entry.payload) {
                return Ok(Some(insight));
            }
        }

        // Slow path: acquire per-date lock to deduplicate concurrent fetches.
        let lock = Self::inflight_lock(&self.inflight_insight, &date_str);
        let _guard = lock.lock().await;

        // Recheck after acquiring the lock: another task may have populated it.
        if let Some(entry) = self.read_insight_cached(&date_str).await {
            if entry.is_negative {
                return Ok(None);
            }
            if let Ok(insight) = serde_json::from_str::<InsightPayload>(&entry.payload) {
                return Ok(Some(insight));
            }
        }

        // Cache miss — fetch from the API.
        let now = Utc::now();
        match self.fetch_insight_from_api(date).await? {
            Some(insight) => {
                let expires_at = now + to_chrono(&self.config.insight_ttl);
                let payload = serde_json::to_string(&insight).unwrap_or_default();
                self.insight_cache.upsert(&InsightCacheEntry {
                    date: date_str,
                    payload,
                    expires_at,
                    is_negative: false,
                })?;
                tracing::debug!(
                    date = %date,
                    confidence_level = ?insight.confidence_level,
                    "cached daily insight"
                );
                if let Some(adapter) = &self.push_adapter {
                    adapter.push_insight(insight.clone()).await;
                }
                Ok(Some(insight))
            }
            None => {
                // 404 — store a negative entry to suppress API hammering.
                let expires_at = now + to_chrono(&self.config.insight_negative_ttl);
                self.insight_cache.upsert_negative(&date_str, expires_at)?;
                tracing::debug!(date = %date, "cached negative insight (404)");
                Ok(None)
            }
        }
    }

    /// Proactive refresh used by the scheduler: fetches history and all N days
    /// of insights, logging any individual failures without propagating them.
    pub async fn refresh_all(&self, days: u8) -> Result<(), FetchError> {
        if let Err(error) = self.daily_history(days).await {
            tracing::warn!(
                error_code = "history_refresh_failed",
                error = %error,
                "proactive history refresh failed"
            );
        }
        let today = Utc::now().date_naive();
        for i in 0..days as i64 {
            if let Some(date) = today.checked_sub_signed(ChronoDuration::days(i)) {
                if let Err(error) = self.daily_insight(date).await {
                    tracing::warn!(
                        error_code = "insight_refresh_failed",
                        date = %date,
                        error = %error,
                        "proactive insight refresh failed"
                    );
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Invalidation — called by the CacheManager implementation
    // -----------------------------------------------------------------------

    pub fn invalidate_history_date(&self, date: NaiveDate) -> Result<(), PersistenceError> {
        let date_str = date.format("%Y-%m-%d").to_string();
        self.history_cache.invalidate(&date_str)?;
        Ok(())
    }

    pub fn invalidate_history_all(&self) -> Result<(), PersistenceError> {
        self.history_cache.invalidate_all()?;
        Ok(())
    }

    pub fn invalidate_insight_date(&self, date: NaiveDate) -> Result<(), PersistenceError> {
        let date_str = date.format("%Y-%m-%d").to_string();
        self.insight_cache.invalidate(&date_str)?;
        Ok(())
    }

    pub fn invalidate_insight_all(&self) -> Result<(), PersistenceError> {
        self.insight_cache.invalidate_all()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Reads a history cache entry with a timeout; returns None on timeout or
    /// any error so that the caller falls through to an API fetch.
    async fn read_history_cached(&self, date: &str) -> Option<HistoryCacheEntry> {
        let repo = Arc::clone(&self.history_cache);
        let date = date.to_owned();
        let task = tokio::task::spawn_blocking(move || repo.get(&date));
        match tokio::time::timeout(self.config.read_timeout, task).await {
            Ok(Ok(Ok(entry))) => entry,
            Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => None,
        }
    }

    async fn read_insight_cached(&self, date: &str) -> Option<InsightCacheEntry> {
        let repo = Arc::clone(&self.insight_cache);
        let date = date.to_owned();
        let task = tokio::task::spawn_blocking(move || repo.get(&date));
        match tokio::time::timeout(self.config.read_timeout, task).await {
            Ok(Ok(Ok(entry))) => entry,
            Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => None,
        }
    }

    async fn fetch_history_from_api(&self, days: u8) -> Result<Vec<DailySummary>, FetchError> {
        let path = format!("/v1/history/daily?days={days}");
        let request = HttpRequest::get(path);
        let response = self.http.send(request).await?;
        if response.status != 200 {
            return Err(FetchError::ApiError {
                status: response.status,
            });
        }
        let body = response
            .raw_body
            .ok_or(ParseError::MissingField { field: "body" })?;
        let parsed = parser::parse_history(body)?;
        Ok(parsed.summaries)
    }

    async fn fetch_insight_from_api(
        &self,
        date: NaiveDate,
    ) -> Result<Option<InsightPayload>, FetchError> {
        let path = format!("/v1/insights/daily?date={}", date.format("%Y-%m-%d"));
        let request = HttpRequest::get(path);
        let response = self.http.send(request).await?;
        match response.status {
            200 => {
                let body = response
                    .raw_body
                    .ok_or(ParseError::MissingField { field: "body" })?;
                let insight = parser::parse_insight(body)?;
                Ok(Some(insight))
            }
            404 => Ok(None),
            status => Err(FetchError::ApiError { status }),
        }
    }
}

impl<H: HttpClient + 'static> Fetchable for FetchService<H> {
    fn fetch_all<'a>(&'a self, days: u8) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Err(error) = self.refresh_all(days).await {
                tracing::warn!(
                    error_code = "fetch_all_failed",
                    error = %error,
                    "scheduled fetch_all encountered an error"
                );
            }
        })
    }
}

fn to_chrono(duration: &Duration) -> ChronoDuration {
    ChronoDuration::seconds(duration.as_secs() as i64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SqlitePersistence;
    use serde_json::json;
    use std::sync::Mutex;
    use velvt_shared_types::{ConfidenceLevel, HistoryStatus};

    // -----------------------------------------------------------------------
    // FakeHttpClient
    // -----------------------------------------------------------------------

    struct FakeHttpClient {
        calls: Arc<Mutex<Vec<String>>>,
        /// path prefix → (status, body)
        routes: Vec<(String, u16, serde_json::Value)>,
    }

    impl FakeHttpClient {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                routes: Vec::new(),
            }
        }

        fn with_route(
            mut self,
            path_prefix: impl Into<String>,
            status: u16,
            body: serde_json::Value,
        ) -> Self {
            self.routes.push((path_prefix.into(), status, body));
            self
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn called_paths(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl HttpClient for FakeHttpClient {
        fn send<'a>(
            &'a self,
            request: crate::auth::HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<crate::auth::HttpResponse, AuthError>> + Send + 'a>>
        {
            let path = request.path.clone();
            self.calls.lock().unwrap().push(path.clone());
            let route = self
                .routes
                .iter()
                .find(|(prefix, _, _)| path.starts_with(prefix.as_str()))
                .map(|(_, status, body)| (*status, body.clone()));

            Box::pin(async move {
                let (status, raw_body) = route.unwrap_or((404, json!({})));
                Ok(crate::auth::HttpResponse {
                    status,
                    error_code: None,
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: Some(raw_body),
                })
            })
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_config() -> FetchConfig {
        FetchConfig {
            history_ttl: Duration::from_secs(600),
            insight_ttl: Duration::from_secs(1800),
            insight_negative_ttl: Duration::from_secs(300),
            read_timeout: Duration::from_millis(500),
        }
    }

    fn make_summary(date: NaiveDate) -> DailySummary {
        DailySummary {
            date,
            status: HistoryStatus::Ready,
            event_count: 10,
            active_seconds: 3600,
            confidence_level: ConfidenceLevel::High,
            focus_score: Some(0.8),
            fragmentation_score: None,
        }
    }

    fn make_insight(date: NaiveDate) -> InsightPayload {
        InsightPayload {
            date,
            text: "Test insight text".into(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: Utc::now(),
        }
    }

    fn history_api_body(date: NaiveDate) -> serde_json::Value {
        json!({
            "summaries": [{
                "date": date.format("%Y-%m-%d").to_string(),
                "status": "ready",
                "event_count": 10,
                "active_seconds": 3600,
                "confidence_level": "high",
                "focus_score": 0.8
            }]
        })
    }

    fn insight_api_body(date: NaiveDate) -> serde_json::Value {
        json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "text": "Test insight text",
            "confidence_level": "high",
            "low_confidence": false,
            "generated_at": "2026-06-14T10:00:00Z"
        })
    }

    // -----------------------------------------------------------------------
    // History tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn history_cache_hit_skips_http() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let summary = make_summary(today);
        let payload = serde_json::to_string(&summary).unwrap();
        let expires_at = Utc::now() + ChronoDuration::hours(1);
        db.history_cache_repo()
            .upsert(&HistoryCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new());
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_history(1).await.unwrap();
        assert_eq!(http.call_count(), 0, "should not call HTTP on cache hit");
        assert_eq!(result.summaries.len(), 1);
        assert_eq!(result.summaries[0].event_count, 10);
    }

    #[tokio::test]
    async fn history_cache_miss_calls_http_and_populates_cache() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/history/daily",
            200,
            history_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_history(1).await.unwrap();
        assert_eq!(http.call_count(), 1);
        assert_eq!(result.summaries.len(), 1);

        // Second call should hit cache.
        let result2 = service.daily_history(1).await.unwrap();
        assert_eq!(http.call_count(), 1, "second call should use cache");
        assert_eq!(result2.summaries.len(), 1);
    }

    #[tokio::test]
    async fn history_stale_cache_calls_api() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let summary = make_summary(today);
        let payload = serde_json::to_string(&summary).unwrap();
        // Store with expired TTL (in the past).
        let expires_at = Utc::now() - ChronoDuration::hours(1);
        db.history_cache_repo()
            .upsert(&HistoryCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/history/daily",
            200,
            history_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        service.daily_history(1).await.unwrap();
        assert_eq!(http.call_count(), 1, "stale cache should trigger API call");
    }

    #[tokio::test]
    async fn history_api_error_is_propagated() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let http = Arc::new(FakeHttpClient::new().with_route("/v1/history/daily", 500, json!({})));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let err = service.daily_history(1).await.unwrap_err();
        assert!(
            matches!(err, FetchError::ApiError { status: 500 }),
            "expected ApiError(500), got {err:?}"
        );
    }

    #[tokio::test]
    async fn history_invalid_response_is_rejected() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        // Missing required fields in the body.
        let bad_body = json!({ "summaries": [{ "date": "2026-06-14" }] });
        let http = Arc::new(FakeHttpClient::new().with_route("/v1/history/daily", 200, bad_body));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let err = service.daily_history(1).await.unwrap_err();
        assert!(
            matches!(err, FetchError::InvalidResponse(_)),
            "expected InvalidResponse, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Insight tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn insight_cache_hit_skips_http() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let insight = make_insight(today);
        let payload = serde_json::to_string(&insight).unwrap();
        let expires_at = Utc::now() + ChronoDuration::hours(1);
        db.insight_cache_repo()
            .upsert(&InsightCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at,
                is_negative: false,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new());
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_insight(today).await.unwrap();
        assert_eq!(http.call_count(), 0, "should not call HTTP on cache hit");
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn insight_cache_miss_calls_http_and_populates_cache() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/insights/daily",
            200,
            insight_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_insight(today).await.unwrap();
        assert_eq!(http.call_count(), 1);
        assert!(result.is_some());

        let result2 = service.daily_insight(today).await.unwrap();
        assert_eq!(http.call_count(), 1, "second call must use cache");
        assert!(result2.is_some());
    }

    #[tokio::test]
    async fn insight_stale_cache_calls_api() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let insight = make_insight(today);
        let payload = serde_json::to_string(&insight).unwrap();
        let expires_at = Utc::now() - ChronoDuration::hours(1);
        db.insight_cache_repo()
            .upsert(&InsightCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at,
                is_negative: false,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/insights/daily",
            200,
            insight_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        service.daily_insight(today).await.unwrap();
        assert_eq!(http.call_count(), 1, "stale entry should trigger API call");
    }

    #[tokio::test]
    async fn insight_404_stores_negative_entry() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let http = Arc::new(FakeHttpClient::new().with_route("/v1/insights/daily", 404, json!({})));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_insight(today).await.unwrap();
        assert!(result.is_none(), "404 should return None");
        assert_eq!(http.call_count(), 1);

        // Second call within negative TTL must not hit the API.
        let result2 = service.daily_insight(today).await.unwrap();
        assert!(result2.is_none());
        assert_eq!(
            http.call_count(),
            1,
            "negative cache should suppress API call"
        );
    }

    #[tokio::test]
    async fn insight_negative_cache_hit_returns_none_without_http() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let expires_at = Utc::now() + ChronoDuration::hours(1);
        db.insight_cache_repo()
            .upsert_negative(&today.format("%Y-%m-%d").to_string(), expires_at)
            .unwrap();

        let http = Arc::new(FakeHttpClient::new());
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_insight(today).await.unwrap();
        assert!(result.is_none());
        assert_eq!(
            http.call_count(),
            0,
            "negative cache hit must not call HTTP"
        );
    }

    // -----------------------------------------------------------------------
    // Cache invalidation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn invalidate_all_history_then_fetch_hits_api() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let summary = make_summary(today);
        let payload = serde_json::to_string(&summary).unwrap();
        db.history_cache_repo()
            .upsert(&HistoryCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at: Utc::now() + ChronoDuration::hours(1),
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/history/daily",
            200,
            history_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        service.invalidate_history_all().unwrap();
        service.daily_history(1).await.unwrap();
        assert_eq!(
            http.call_count(),
            1,
            "after invalidation, API must be called"
        );
    }

    #[tokio::test]
    async fn invalidate_all_insights_then_fetch_hits_api() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let insight = make_insight(today);
        let payload = serde_json::to_string(&insight).unwrap();
        db.insight_cache_repo()
            .upsert(&InsightCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at: Utc::now() + ChronoDuration::hours(1),
                is_negative: false,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/insights/daily",
            200,
            insight_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        service.invalidate_insight_all().unwrap();
        service.daily_insight(today).await.unwrap();
        assert_eq!(
            http.call_count(),
            1,
            "after invalidation, API must be called"
        );
    }

    #[tokio::test]
    async fn insight_paths_contain_date_parameter() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let http = Arc::new(FakeHttpClient::new().with_route("/v1/insights/daily", 404, json!({})));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        service.daily_insight(date).await.unwrap();
        let paths = http.called_paths();
        assert!(
            paths[0].contains("2026-01-15"),
            "path must contain the date, got: {:?}",
            paths
        );
    }

    // -----------------------------------------------------------------------
    // Edge-case hardening tests
    // -----------------------------------------------------------------------

    /// HTTP client that sleeps briefly before responding, ensuring concurrent
    /// callers are both in-flight before either receives a response.
    struct SlowHttpClient {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        status: u16,
        body: serde_json::Value,
        delay_ms: u64,
    }

    impl SlowHttpClient {
        fn new(
            status: u16,
            body: serde_json::Value,
            calls: Arc<std::sync::atomic::AtomicUsize>,
            delay_ms: u64,
        ) -> Arc<Self> {
            Arc::new(Self {
                calls,
                status,
                body,
                delay_ms,
            })
        }
    }

    impl HttpClient for SlowHttpClient {
        fn send<'a>(
            &'a self,
            _request: crate::auth::HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<crate::auth::HttpResponse, AuthError>> + Send + 'a>>
        {
            use std::sync::atomic::Ordering;
            self.calls.fetch_add(1, Ordering::SeqCst);
            let body = self.body.clone();
            let status = self.status;
            let delay_ms = self.delay_ms;
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(crate::auth::HttpResponse {
                    status,
                    error_code: None,
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: Some(body),
                })
            })
        }
    }

    #[tokio::test]
    async fn history_500_preserves_stale_cache_entry() {
        // When the API returns 500, an expired (stale) cache row must NOT be
        // deleted.  The stale row survives so a later successful fetch can
        // overwrite it rather than finding an empty table.
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let date_str = today.format("%Y-%m-%d").to_string();
        let payload = serde_json::to_string(&make_summary(today)).unwrap();

        db.history_cache_repo()
            .upsert(&HistoryCacheEntry {
                date: date_str.clone(),
                payload,
                expires_at: Utc::now() - ChronoDuration::hours(1), // already stale
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route("/v1/history/daily", 500, json!({})));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let err = service.daily_history(1).await.unwrap_err();
        assert!(
            matches!(err, FetchError::ApiError { status: 500 }),
            "expected ApiError(500), got {err:?}"
        );

        // invalidate() returns the number of rows deleted; 1 means the stale
        // row is still in the DB and was not wiped by the error path.
        let deleted = db.history_cache_repo().invalidate(&date_str).unwrap();
        assert_eq!(deleted, 1, "stale cache row must survive an API 500 error");
    }

    #[tokio::test]
    async fn insight_500_preserves_stale_cache_entry() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let date_str = today.format("%Y-%m-%d").to_string();
        let payload = serde_json::to_string(&make_insight(today)).unwrap();

        db.insight_cache_repo()
            .upsert(&InsightCacheEntry {
                date: date_str.clone(),
                payload,
                expires_at: Utc::now() - ChronoDuration::hours(1),
                is_negative: false,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route("/v1/insights/daily", 500, json!({})));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let err = service.daily_insight(today).await.unwrap_err();
        assert!(
            matches!(err, FetchError::ApiError { status: 500 }),
            "expected ApiError(500), got {err:?}"
        );

        let deleted = db.insight_cache_repo().invalidate(&date_str).unwrap();
        assert_eq!(
            deleted, 1,
            "stale insight row must survive an API 500 error"
        );
    }

    #[tokio::test]
    async fn history_no_data_status_is_cached_and_served() {
        // status: no_data is a valid API value meaning the server has no events
        // for that day.  It must be stored as a regular cache entry (not an
        // error) and re-served on the next call without hitting the API.
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let no_data_body = json!({
            "summaries": [{
                "date": today.format("%Y-%m-%d").to_string(),
                "status": "no_data",
                "event_count": 0,
                "active_seconds": 0,
                "confidence_level": "low"
            }]
        });
        let http =
            Arc::new(FakeHttpClient::new().with_route("/v1/history/daily", 200, no_data_body));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_history(1).await.unwrap();
        assert_eq!(result.summaries.len(), 1);
        assert_eq!(
            result.summaries[0].status,
            HistoryStatus::NoData,
            "no_data status must be preserved as a distinct entry, not mapped to an error"
        );
        assert_eq!(http.call_count(), 1);

        // Second call must be served from cache with the same no_data status.
        let result2 = service.daily_history(1).await.unwrap();
        assert_eq!(result2.summaries[0].status, HistoryStatus::NoData);
        assert_eq!(
            http.call_count(),
            1,
            "no_data result must be cached for subsequent calls"
        );
    }

    #[tokio::test]
    async fn clock_skew_expired_entry_is_cache_miss_not_panic() {
        // Simulate forward clock skew: the stored expires_at is in the past,
        // as would happen if the wall clock jumped forward after the entry was
        // stored.  The service must treat the row as a cache miss and fall
        // through to the API — no panic, no error exposed to the caller.
        let db = SqlitePersistence::open_in_memory().unwrap();
        let today = Utc::now().date_naive();
        let payload = serde_json::to_string(&make_insight(today)).unwrap();

        db.insight_cache_repo()
            .upsert(&InsightCacheEntry {
                date: today.format("%Y-%m-%d").to_string(),
                payload,
                expires_at: Utc::now() - ChronoDuration::seconds(1), // already expired
                is_negative: false,
            })
            .unwrap();

        let http = Arc::new(FakeHttpClient::new().with_route(
            "/v1/insights/daily",
            200,
            insight_api_body(today),
        ));
        let service = FetchService::new(
            Arc::clone(&http),
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        );

        let result = service.daily_insight(today).await;
        assert!(
            result.is_ok(),
            "clock-skew expired entry must not cause a panic or error"
        );
        assert_eq!(
            http.call_count(),
            1,
            "expired entry must fall through to the API"
        );
    }

    #[test]
    fn zero_ttl_does_not_panic() {
        // A zero-second TTL causes immediate expiry but must not panic or
        // overflow when passed through to_chrono or used in chrono arithmetic.
        let zero = to_chrono(&Duration::ZERO);
        assert_eq!(zero, ChronoDuration::seconds(0));

        // Large but realistic TTL (30 days) must not overflow i64.
        let thirty_days = to_chrono(&Duration::from_secs(30 * 24 * 3600));
        assert!(thirty_days > ChronoDuration::seconds(0));
    }

    #[tokio::test]
    async fn concurrent_insight_fetches_deduplicated() {
        // Two concurrent daily_insight calls for the same date must result in
        // exactly one HTTP request.  The second caller blocks on the in-flight
        // lock, then re-checks the cache and finds the result already stored.
        use std::sync::atomic::{AtomicUsize, Ordering};

        let db = Arc::new(SqlitePersistence::open_in_memory().unwrap());
        let today = Utc::now().date_naive();
        let call_count = Arc::new(AtomicUsize::new(0));
        // 20 ms delay ensures both futures are both past their initial cache
        // check before the first HTTP response arrives.
        let http = SlowHttpClient::new(200, insight_api_body(today), Arc::clone(&call_count), 20);
        let service = Arc::new(FetchService::new(
            http,
            db.history_cache_repo(),
            db.insight_cache_repo(),
            test_config(),
        ));

        let svc1 = Arc::clone(&service);
        let svc2 = Arc::clone(&service);
        let (r1, r2) = tokio::join!(svc1.daily_insight(today), svc2.daily_insight(today),);

        assert!(r1.is_ok(), "first concurrent fetch must succeed: {r1:?}");
        assert!(r2.is_ok(), "second concurrent fetch must succeed: {r2:?}");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "concurrent fetches for the same date must be deduplicated to exactly 1 HTTP call"
        );
    }
}
