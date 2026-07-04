use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::watch;
use velvt_shared_types::{ConfidenceLevel, InsightPayload};

use crate::auth::{AuthError, AuthState, HttpClient, HttpRequest};

use super::{parser, PushAdapter};

#[derive(Clone, Debug)]
pub struct PollConfig {
    pub path: String,
    pub poll_timeout: Duration,
    pub idle_interval: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PolledInsight {
    pub id: String,
    pub payload: InsightPayload,
}

#[derive(Debug, PartialEq)]
pub enum PollOutcome {
    Insight(PolledInsight),
    NoContent,
}

#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("long-poll request failed: {0}")]
    Http(#[from] AuthError),
    #[error("long-poll API returned unexpected status {status}")]
    ApiStatus { status: u16 },
    #[error("long-poll API rejected concurrent polling")]
    RateLimited { retry_after: Option<Duration> },
    #[error("long-poll response could not be parsed: {0}")]
    InvalidResponse(#[from] parser::ParseError),
    #[error("long-poll request timed out")]
    Timeout,
}

#[derive(Debug)]
pub struct PollClient<H> {
    http: Arc<H>,
    config: PollConfig,
}

impl<H: HttpClient> PollClient<H> {
    pub fn new(http: Arc<H>, config: PollConfig) -> Self {
        Self { http, config }
    }

    pub async fn poll_once(&self) -> Result<PollOutcome, PollError> {
        let request = HttpRequest::get(self.config.path.clone());
        let response =
            match tokio::time::timeout(self.config.poll_timeout, self.http.send(request)).await {
                Ok(result) => result?,
                Err(_) => return Err(PollError::Timeout),
            };

        match response.status {
            200 => {
                let body = response
                    .raw_body
                    .ok_or(parser::ParseError::MissingField { field: "body" })?;
                Ok(PollOutcome::Insight(parse_polled_insight(body)?))
            }
            204 => Ok(PollOutcome::NoContent),
            429 => Err(PollError::RateLimited {
                retry_after: parse_retry_after(response.retry_after.as_deref()),
            }),
            status => Err(PollError::ApiStatus { status }),
        }
    }
}

pub struct PollScheduler<H> {
    client: PollClient<H>,
    push_adapter: Arc<PushAdapter>,
    auth_state: watch::Receiver<AuthState>,
    shutdown: watch::Receiver<bool>,
    dedupe: InsightDedupeGuard,
    backoff: BackoffPolicy,
}

impl<H: HttpClient> PollScheduler<H> {
    pub fn new(
        client: PollClient<H>,
        push_adapter: Arc<PushAdapter>,
        auth_state: watch::Receiver<AuthState>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let backoff = BackoffPolicy::new(client.config.initial_backoff, client.config.max_backoff);
        Self {
            client,
            push_adapter,
            auth_state,
            shutdown,
            dedupe: InsightDedupeGuard::default(),
            backoff,
        }
    }

    pub async fn run(mut self) {
        loop {
            if *self.shutdown.borrow() {
                return;
            }
            if !matches!(*self.auth_state.borrow(), AuthState::Authenticated { .. }) {
                if self.wait_for_auth_or_shutdown().await {
                    return;
                }
                continue;
            }

            match self.client.poll_once().await {
                Ok(PollOutcome::Insight(insight)) => {
                    self.backoff.reset();
                    deliver_polled_insight(&self.push_adapter, &mut self.dedupe, insight).await;
                }
                Ok(PollOutcome::NoContent) => {
                    self.backoff.reset();
                    if self
                        .sleep_or_shutdown(self.client.config.idle_interval)
                        .await
                    {
                        return;
                    }
                }
                Err(PollError::ApiStatus { status }) => {
                    let delay = self.backoff.next_delay();
                    tracing::warn!(
                        status,
                        delay_ms = delay.as_millis() as u64,
                        error_code = "insight_poll_api_status",
                        "long-poll endpoint returned a non-success status"
                    );
                    if self.sleep_or_shutdown(delay).await {
                        return;
                    }
                }
                Err(PollError::RateLimited { retry_after }) => {
                    let delay = retry_after.unwrap_or(
                        self.client.config.poll_timeout + self.client.config.idle_interval,
                    );
                    tracing::warn!(
                        delay_ms = delay.as_millis() as u64,
                        error_code = "insight_poll_rate_limited",
                        "long-poll endpoint rejected a concurrent poll"
                    );
                    if self.sleep_or_shutdown(delay).await {
                        return;
                    }
                }
                Err(error) => {
                    let delay = self.backoff.next_delay();
                    tracing::warn!(
                        error = %error,
                        delay_ms = delay.as_millis() as u64,
                        error_code = "insight_poll_failed",
                        "long-poll request failed"
                    );
                    if self.sleep_or_shutdown(delay).await {
                        return;
                    }
                }
            }
        }
    }

    async fn wait_for_auth_or_shutdown(&mut self) -> bool {
        loop {
            tokio::select! {
                changed = self.shutdown.changed() => {
                    return changed.is_err() || *self.shutdown.borrow();
                }
                changed = self.auth_state.changed() => {
                    if changed.is_err() {
                        return true;
                    }
                    if matches!(*self.auth_state.borrow(), AuthState::Authenticated { .. }) {
                        return false;
                    }
                }
            }
        }
    }

    async fn sleep_or_shutdown(&mut self, duration: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(duration) => false,
            changed = self.shutdown.changed() => changed.is_err() || *self.shutdown.borrow(),
            changed = self.auth_state.changed() => {
                changed.is_err()
            }
        }
    }
}

fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let seconds = value?.trim().parse::<u64>().ok()?;
    (seconds > 0).then_some(Duration::from_secs(seconds))
}

#[derive(Deserialize)]
struct RawPolledInsight {
    id: Option<String>,
    date: Option<chrono::NaiveDate>,
    text: Option<String>,
    generated_at: Option<DateTime<Utc>>,
    meta: Option<RawPolledInsightMeta>,
}

#[derive(Deserialize)]
struct RawPolledInsightMeta {
    confidence_level: Option<ConfidenceLevel>,
    low_confidence: Option<bool>,
}

fn parse_polled_insight(value: serde_json::Value) -> Result<PolledInsight, parser::ParseError> {
    let raw: RawPolledInsight = serde_json::from_value(value)?;
    let id = raw
        .id
        .ok_or(parser::ParseError::MissingField { field: "id" })?;
    let meta = raw
        .meta
        .ok_or(parser::ParseError::MissingField { field: "meta" })?;
    let payload = InsightPayload {
        date: raw
            .date
            .ok_or(parser::ParseError::MissingField { field: "date" })?,
        text: raw
            .text
            .ok_or(parser::ParseError::MissingField { field: "text" })?,
        confidence_level: meta
            .confidence_level
            .ok_or(parser::ParseError::MissingField {
                field: "meta.confidence_level",
            })?,
        low_confidence: meta
            .low_confidence
            .ok_or(parser::ParseError::MissingField {
                field: "meta.low_confidence",
            })?,
        generated_at: raw.generated_at.unwrap_or_else(Utc::now),
    };
    Ok(PolledInsight { id, payload })
}

#[derive(Debug)]
pub struct BackoffPolicy {
    initial: Duration,
    max: Duration,
    next: Duration,
}

impl BackoffPolicy {
    pub fn new(initial: Duration, max: Duration) -> Self {
        let initial = initial.min(max);
        Self {
            initial,
            max,
            next: initial,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.max);
        delay
    }

    pub fn reset(&mut self) {
        self.next = self.initial;
    }
}

#[derive(Default, Debug)]
pub struct InsightDedupeGuard {
    last_delivered_id: Option<String>,
}

impl InsightDedupeGuard {
    pub fn should_deliver(&mut self, insight_id: &str) -> bool {
        if self.last_delivered_id.as_deref() == Some(insight_id) {
            return false;
        }
        self.last_delivered_id = Some(insight_id.to_owned());
        true
    }
}

pub async fn deliver_polled_insight(
    push_adapter: &PushAdapter,
    dedupe: &mut InsightDedupeGuard,
    insight: PolledInsight,
) {
    if !dedupe.should_deliver(&insight.id) {
        tracing::debug!("duplicate long-poll insight suppressed");
        return;
    }
    let notification_id = uuid::Uuid::new_v4();
    let title = "Your Velvt insight is ready";
    let body = insight.payload.text.clone();
    let date = insight.payload.date;
    push_adapter.push_insight(insight.payload).await;
    push_adapter
        .push_notification(notification_id, title, &body, date)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, HttpClient, HttpRequest, HttpResponse};
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use velvt_shared_types::{ConfidenceLevel, ServerMessage};

    struct FakeHttpClient {
        responses: Mutex<Vec<Result<HttpResponse, AuthError>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeHttpClient {
        fn new(responses: Vec<Result<HttpResponse, AuthError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl HttpClient for FakeHttpClient {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
            self.calls.lock().unwrap().push(request.path.clone());
            Box::pin(async move {
                self.responses
                    .lock()
                    .unwrap()
                    .pop()
                    .unwrap_or(Err(AuthError::Transport))
            })
        }
    }

    fn response(status: u16, raw_body: Option<serde_json::Value>) -> HttpResponse {
        HttpResponse {
            status,
            error_code: None,
            tokens: None,
            retry_after: None,
            message: None,
            raw_body,
            user_id: None,
            device_id: None,
        }
    }

    fn insight_body(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "date": "2026-06-14",
            "text": "You switched away from your document 23 times in 40 minutes.",
            "generated_at": "2026-06-14T10:00:00Z",
            "meta": {
                "confidence_level": "high",
                "low_confidence": false
            }
        })
    }

    fn daily_insight_body() -> serde_json::Value {
        json!({
            "date": "2026-06-14",
            "text": "You switched away from your document 23 times in 40 minutes.",
            "confidence_level": "high",
            "low_confidence": false,
            "generated_at": "2026-06-14T10:00:00Z"
        })
    }

    fn config() -> PollConfig {
        PollConfig {
            path: "/v1/insights/poll".into(),
            poll_timeout: Duration::from_secs(25),
            idle_interval: Duration::from_millis(10),
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn poll_once_maps_200_json_to_insight() {
        let http = FakeHttpClient::new(vec![Ok(response(200, Some(insight_body("insight-1"))))]);
        let client = PollClient::new(Arc::new(http), config());

        let result = client.poll_once().await.unwrap();

        match result {
            PollOutcome::Insight(insight) => {
                assert_eq!(insight.id, "insight-1");
                assert_eq!(insight.payload.confidence_level, ConfidenceLevel::High);
                assert_eq!(
                    insight.payload.generated_at,
                    Utc.with_ymd_and_hms(2026, 6, 14, 10, 0, 0).unwrap()
                );
            }
            other => panic!("expected insight, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_once_accepts_core_poll_body_without_generated_at() {
        let body = json!({
            "id": "insight-1",
            "date": "2026-06-14",
            "text": "You switched away from your document 23 times in 40 minutes.",
            "meta": {
                "confidence_level": "high",
                "low_confidence": false,
                "quality_metadata": {"novelty": 0.8}
            }
        });
        let http = FakeHttpClient::new(vec![Ok(response(200, Some(body)))]);
        let client = PollClient::new(Arc::new(http), config());

        let result = client.poll_once().await.unwrap();

        match result {
            PollOutcome::Insight(insight) => {
                assert_eq!(insight.id, "insight-1");
                assert_eq!(insight.payload.confidence_level, ConfidenceLevel::High);
                assert!(!insight.payload.low_confidence);
            }
            other => panic!("expected insight, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_once_maps_204_to_empty() {
        let http = FakeHttpClient::new(vec![Ok(response(204, None))]);
        let client = PollClient::new(Arc::new(http), config());

        let result = client.poll_once().await.unwrap();

        assert!(matches!(result, PollOutcome::NoContent));
    }

    #[tokio::test]
    async fn poll_once_maps_non_success_to_status_error() {
        let http = FakeHttpClient::new(vec![Ok(response(503, None))]);
        let client = PollClient::new(Arc::new(http), config());

        let result = client.poll_once().await;

        assert!(matches!(result, Err(PollError::ApiStatus { status: 503 })));
    }

    #[tokio::test]
    async fn poll_once_maps_429_to_rate_limited() {
        let mut response = response(429, None);
        response.retry_after = Some("45".into());
        let http = FakeHttpClient::new(vec![Ok(response)]);
        let client = PollClient::new(Arc::new(http), config());

        let result = client.poll_once().await;

        assert!(matches!(result, Err(PollError::RateLimited { retry_after })
            if retry_after == Some(Duration::from_secs(45))));
    }

    #[test]
    fn backoff_policy_doubles_until_cap_and_resets() {
        let mut backoff = BackoffPolicy::new(Duration::from_secs(1), Duration::from_secs(4));

        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        assert_eq!(backoff.next_delay(), Duration::from_secs(4));
        assert_eq!(backoff.next_delay(), Duration::from_secs(4));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn dedupe_guard_allows_only_new_ids() {
        let mut guard = InsightDedupeGuard::default();

        assert!(guard.should_deliver("insight-1"));
        assert!(!guard.should_deliver("insight-1"));
        assert!(guard.should_deliver("insight-2"));
        assert!(!guard.should_deliver("insight-2"));
    }

    #[tokio::test]
    async fn deliver_once_for_duplicate_polled_insight() {
        let queue = crate::delivery::PushQueue::new(10);
        let push = crate::delivery::PushAdapter::new(Arc::clone(&queue));
        let mut dedupe = InsightDedupeGuard::default();
        let insight = PolledInsight {
            id: "insight-1".into(),
            payload: crate::delivery::parser::parse_insight(daily_insight_body()).unwrap(),
        };

        deliver_polled_insight(&push, &mut dedupe, insight.clone()).await;
        deliver_polled_insight(&push, &mut dedupe, insight).await;

        let mut count = 0;
        while let Some(message) = queue.try_pop().await {
            if matches!(
                message,
                ServerMessage::InsightPayload(_) | ServerMessage::NotificationPayload(_)
            ) {
                count += 1;
            }
        }
        assert_eq!(count, 2, "one insight push and one notification push");
    }
}
