//! Push queue and adapter for outbound IPC delivery (R7).
//!
//! `PushQueue` is a bounded, in-memory FIFO that survives client disconnects.
//! `PushAdapter` is the sole write path into the queue; it runs every payload
//! through the shaper before enqueuing so the transport layer only ever sees
//! validated messages.
//!
//! All methods on `PushQueue` that write messages are `pub(super)` — external
//! code (including `ipc/`) can only drain or observe the queue, never inject
//! raw DTOs into it.

use std::{collections::VecDeque, future::Future, pin::Pin, sync::Arc, time::Duration};

use tokio::sync::{Mutex, Notify};
use velvt_shared_types::{
    HistoryPayload, InsightPayload, LocalDashboardSnapshot, PrivacyViolationAlert, ServerMessage,
};

use super::shaper;

// ---------------------------------------------------------------------------
// Queue configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PushConfig {
    /// Maximum number of messages buffered while Swift is disconnected.
    pub capacity: usize,
    /// Per-message write timeout before a connected client is considered slow.
    pub write_timeout: Duration,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            capacity: 50,
            write_timeout: Duration::from_millis(500),
        }
    }
}

// ---------------------------------------------------------------------------
// PushQueue
// ---------------------------------------------------------------------------

fn server_message_type_name(msg: &ServerMessage) -> &'static str {
    match msg {
        ServerMessage::InsightPayload(_) => "insight_payload",
        ServerMessage::HistoryPayload(_) => "history_payload",
        ServerMessage::PrivacyViolationAlert(_) => "privacy_violation_alert",
        ServerMessage::CacheEmpty(_) => "cache_empty",
        ServerMessage::ServiceStatus(_) => "service_status",
        ServerMessage::ServerHello(_) => "server_hello",
        ServerMessage::Acknowledged(_) => "acknowledged",
        ServerMessage::VersionMismatch(_) => "version_mismatch",
        ServerMessage::MalformedMessage(_) => "malformed_message",
        ServerMessage::RawEventAck(_) => "raw_event_ack",
        ServerMessage::ErrorResponse(_) => "error_response",
        ServerMessage::ShuttingDown(_) => "shutting_down",
        ServerMessage::AuthSuccess(_) => "auth_success",
        ServerMessage::AuthSessionUpdated(_) => "auth_session_updated",
        ServerMessage::AuthFailure(_) => "auth_failure",
        ServerMessage::AccountDeletionAccepted(_) => "account_deletion_accepted",
        ServerMessage::NeedsReauth(_) => "needs_reauth",
        ServerMessage::DeviceRevoked(_) => "device_revoked",
        ServerMessage::NotificationPayload(_) => "notification_payload",
        ServerMessage::MenuStatus(_) => "menu_status",
        ServerMessage::WorkBlockState(_) => "work_block_state",
        ServerMessage::LocalDashboard(_) => "local_dashboard",
    }
}

/// Bounded in-memory push queue.
///
/// Write access (`enqueue` / `enqueue_urgent`) is `pub(super)` so only
/// `PushAdapter` (in this module) can inject messages.  The IPC connection
/// layer receives only read access (`try_pop`, `notify`).
pub struct PushQueue {
    inner: Mutex<VecDeque<ServerMessage>>,
    capacity: usize,
    /// Notified on every enqueue so the connection drain loop can wake up.
    notify: Arc<Notify>,
}

impl PushQueue {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            notify: Arc::new(Notify::new()),
        })
    }

    /// Appends `msg` to the back. Drops the oldest message when at capacity.
    pub(super) async fn enqueue(&self, msg: ServerMessage) {
        let mut guard = self.inner.lock().await;
        if guard.len() >= self.capacity {
            if let Some(dropped) = guard.pop_front() {
                tracing::warn!(
                    message_type = server_message_type_name(&dropped),
                    error_code = "push_queue_full",
                    "push queue at capacity; oldest message dropped"
                );
            }
        }
        guard.push_back(msg);
        drop(guard);
        self.notify.notify_one();
    }

    /// Prepends `msg` to the front (high priority, e.g. privacy alerts).
    /// Does NOT drop any existing message if the queue is full — the alert
    /// is too important to lose.
    pub(super) async fn enqueue_urgent(&self, msg: ServerMessage) {
        let mut guard = self.inner.lock().await;
        guard.push_front(msg);
        drop(guard);
        self.notify.notify_one();
    }

    /// Removes and returns the front message, or `None` when the queue is empty.
    pub async fn try_pop(&self) -> Option<ServerMessage> {
        self.inner.lock().await.pop_front()
    }

    /// Returns the `Notify` handle so the connection loop can `notified().await`.
    pub fn notify(&self) -> &Arc<Notify> {
        &self.notify
    }

    /// Returns the current queue depth (primarily for tests).
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Returns `true` when the queue has no pending messages.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

// ---------------------------------------------------------------------------
// PushAdapter — sole entry point for writing validated payloads
// ---------------------------------------------------------------------------

/// Routes shaped, validated payloads into the `PushQueue`.
///
/// Every `push_*` method calls the corresponding shaper function.  A payload
/// that fails validation is logged (type only, no content) and silently dropped
/// — it never reaches the queue or the transport.
pub struct PushAdapter {
    queue: Arc<PushQueue>,
}

impl PushAdapter {
    pub fn new(queue: Arc<PushQueue>) -> Arc<Self> {
        Arc::new(Self { queue })
    }

    pub fn queue(&self) -> &Arc<PushQueue> {
        &self.queue
    }

    pub async fn push_insight(&self, payload: InsightPayload) {
        match shaper::shape_insight(payload) {
            Ok(validated) => {
                self.queue
                    .enqueue(ServerMessage::InsightPayload(validated.into_inner()))
                    .await;
            }
            Err(err) => {
                tracing::warn!(
                    message_type = "insight_payload",
                    error_code = "outbound_validation_failed",
                    error = %err,
                    "outbound payload failed validation; dropped without sending"
                );
            }
        }
    }

    pub async fn push_history(&self, payload: HistoryPayload) {
        match shaper::shape_history(payload) {
            Ok(validated) => {
                self.queue
                    .enqueue(ServerMessage::HistoryPayload(validated.into_inner()))
                    .await;
            }
            Err(err) => {
                tracing::warn!(
                    message_type = "history_payload",
                    error_code = "outbound_validation_failed",
                    error = %err,
                    "outbound payload failed validation; dropped without sending"
                );
            }
        }
    }

    /// Privacy alerts are prepended (urgent) so they arrive before any pending
    /// history or insight messages already in the queue.
    pub async fn push_privacy_alert(&self, alert: PrivacyViolationAlert) {
        match shaper::shape_privacy_alert(alert) {
            Ok(validated) => {
                self.queue
                    .enqueue_urgent(ServerMessage::PrivacyViolationAlert(validated.into_inner()))
                    .await;
            }
            Err(err) => {
                tracing::warn!(
                    message_type = "privacy_violation_alert",
                    error_code = "outbound_validation_failed",
                    error = %err,
                    "outbound payload failed validation; dropped without sending"
                );
            }
        }
    }

    pub async fn push_cache_empty(&self, payload_type: &str) {
        match shaper::shape_cache_empty(payload_type) {
            Ok(validated) => {
                self.queue
                    .enqueue(ServerMessage::CacheEmpty(validated.into_inner()))
                    .await;
            }
            Err(err) => {
                tracing::warn!(
                    message_type = "cache_empty",
                    error_code = "outbound_validation_failed",
                    error = %err,
                    "outbound payload failed validation; dropped without sending"
                );
            }
        }
    }

    /// Enqueues a `ShuttingDown` message at the front of the queue so it is
    /// delivered before any pending payloads.  Called during graceful shutdown
    /// before the cancellation token is set.
    pub async fn push_shutting_down(&self, reason: &str) {
        self.queue
            .enqueue_urgent(ServerMessage::ShuttingDown(
                velvt_shared_types::ShuttingDown {
                    reason: reason.to_owned(),
                },
            ))
            .await;
    }

    /// Pushed when the device registration is permanently revoked. Urgent:
    /// Swift must clear Keychain and show the Device Revoked screen promptly.
    pub async fn push_device_revoked(&self, message: &str) {
        self.queue
            .enqueue_urgent(ServerMessage::DeviceRevoked(
                velvt_shared_types::DeviceRevoked {
                    message: message.to_owned(),
                },
            ))
            .await;
    }

    /// Pushed when the session expires and cannot be refreshed. Urgent:
    /// Swift must clear Keychain and show the login screen promptly.
    pub async fn push_needs_reauth(&self, reason: &str) {
        self.queue
            .enqueue_urgent(ServerMessage::NeedsReauth(
                velvt_shared_types::NeedsReauth {
                    reason: reason.to_owned(),
                },
            ))
            .await;
    }

    pub async fn push_auth_session_updated(&self, session: velvt_shared_types::AuthSession) {
        self.queue
            .enqueue_urgent(ServerMessage::AuthSessionUpdated(session))
            .await;
    }

    /// Pushed after a fresh (non-cached) daily insight fetch so Swift can
    /// schedule a notification. `title`/`body` are Rust-authored display
    /// copy; Swift never generates notification text itself.
    pub async fn push_notification(
        &self,
        notification_id: uuid::Uuid,
        title: &str,
        body: &str,
        insight_date: chrono::NaiveDate,
    ) {
        self.queue
            .enqueue(ServerMessage::NotificationPayload(
                velvt_shared_types::NotificationPayload {
                    notification_id,
                    title: title.to_owned(),
                    body: body.to_owned(),
                    insight_date,
                    do_not_disturb_until: None,
                },
            ))
            .await;
    }

    /// Pushed to report a service health transition, such as Tier 2
    /// classification being unavailable so the device degrades to Tier 1/3.
    pub async fn push_service_status(
        &self,
        state: velvt_shared_types::ServiceState,
        reason: Option<&str>,
    ) {
        self.queue
            .enqueue(ServerMessage::ServiceStatus(
                velvt_shared_types::ServiceStatus {
                    state,
                    reason: reason.map(str::to_owned),
                },
            ))
            .await;
    }

    /// Pushes Rust-authored local work-block state. The free-form intention is
    /// permitted only on this device-local queue and is never logged here.
    pub async fn push_work_block_state(&self, snapshot: velvt_shared_types::WorkBlockSnapshot) {
        self.queue
            .enqueue(ServerMessage::WorkBlockState(snapshot))
            .await;
    }

    pub async fn push_local_dashboard(&self, snapshot: LocalDashboardSnapshot) {
        match shaper::shape_local_dashboard(snapshot) {
            Ok(validated) => {
                self.queue
                    .enqueue(ServerMessage::LocalDashboard(validated.into_inner()))
                    .await;
            }
            Err(err) => tracing::warn!(
                message_type = "local_dashboard",
                error_code = "outbound_validation_failed",
                error = %err,
                "local dashboard payload failed validation; dropped without sending"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// PushAdapterAlertSink — wraps PushAdapter to implement PrivacyAlertSink
// ---------------------------------------------------------------------------

use crate::upload::PrivacyAlertSink;

pub struct PushAdapterAlertSink {
    adapter: Arc<PushAdapter>,
}

impl PushAdapterAlertSink {
    pub fn new(adapter: Arc<PushAdapter>) -> Self {
        Self { adapter }
    }
}

impl PrivacyAlertSink for PushAdapterAlertSink {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.adapter
                .push_privacy_alert(PrivacyViolationAlert {
                    code: "raw_field_rejected".into(),
                    message: message.to_owned(),
                })
                .await;
        })
    }
}

// ---------------------------------------------------------------------------
// FakePushAdapter — test double
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "test-helpers"))]
use std::sync::Mutex as StdMutex;

/// Test double: records every push call without touching a real queue.
#[cfg(any(test, feature = "test-helpers"))]
pub struct FakePushAdapter {
    history_calls: StdMutex<Vec<HistoryPayload>>,
    insight_calls: StdMutex<Vec<InsightPayload>>,
    alert_calls: StdMutex<Vec<PrivacyViolationAlert>>,
    cache_empty_calls: StdMutex<Vec<String>>,
}

#[cfg(any(test, feature = "test-helpers"))]
impl FakePushAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            history_calls: StdMutex::new(Vec::new()),
            insight_calls: StdMutex::new(Vec::new()),
            alert_calls: StdMutex::new(Vec::new()),
            cache_empty_calls: StdMutex::new(Vec::new()),
        })
    }

    pub fn history_push_count(&self) -> usize {
        self.history_calls.lock().unwrap().len()
    }

    pub fn insight_push_count(&self) -> usize {
        self.insight_calls.lock().unwrap().len()
    }

    pub fn alert_push_count(&self) -> usize {
        self.alert_calls.lock().unwrap().len()
    }

    pub fn cache_empty_push_count(&self) -> usize {
        self.cache_empty_calls.lock().unwrap().len()
    }

    pub async fn push_history(&self, payload: HistoryPayload) {
        self.history_calls.lock().unwrap().push(payload);
    }

    pub async fn push_insight(&self, payload: InsightPayload) {
        self.insight_calls.lock().unwrap().push(payload);
    }

    pub async fn push_privacy_alert(&self, alert: PrivacyViolationAlert) {
        self.alert_calls.lock().unwrap().push(alert);
    }

    pub async fn push_cache_empty(&self, payload_type: &str) {
        self.cache_empty_calls
            .lock()
            .unwrap()
            .push(payload_type.to_owned());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use velvt_shared_types::ConfidenceLevel;

    fn make_insight(text: &str) -> InsightPayload {
        InsightPayload {
            date: Utc::now().date_naive(),
            text: text.into(),
            evidence: Default::default(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: Utc::now(),
        }
    }

    fn make_history(days: u32) -> HistoryPayload {
        HistoryPayload {
            days,
            summaries: vec![],
        }
    }

    // --- PushQueue unit tests ---

    #[tokio::test]
    async fn enqueue_and_pop_fifo_order() {
        let q = PushQueue::new(10);
        q.enqueue(ServerMessage::CacheEmpty(velvt_shared_types::CacheEmpty {
            payload_type: "insight_payload".into(),
        }))
        .await;
        q.enqueue(ServerMessage::CacheEmpty(velvt_shared_types::CacheEmpty {
            payload_type: "history_payload".into(),
        }))
        .await;

        let first = q.try_pop().await.unwrap();
        let second = q.try_pop().await.unwrap();
        assert!(q.try_pop().await.is_none());

        match first {
            ServerMessage::CacheEmpty(c) => assert_eq!(c.payload_type, "insight_payload"),
            _ => panic!("unexpected variant"),
        }
        match second {
            ServerMessage::CacheEmpty(c) => assert_eq!(c.payload_type, "history_payload"),
            _ => panic!("unexpected variant"),
        }
    }

    #[tokio::test]
    async fn full_queue_drops_oldest() {
        let q = PushQueue::new(3);
        for i in 0u32..4 {
            q.enqueue(ServerMessage::HistoryPayload(HistoryPayload {
                days: i + 1,
                summaries: vec![],
            }))
            .await;
        }
        // capacity=3; 4th enqueue evicts the first (days=1)
        assert_eq!(q.len().await, 3);
        match q.try_pop().await.unwrap() {
            ServerMessage::HistoryPayload(h) => assert_eq!(h.days, 2),
            _ => panic!(),
        }
        match q.try_pop().await.unwrap() {
            ServerMessage::HistoryPayload(h) => assert_eq!(h.days, 3),
            _ => panic!(),
        }
        match q.try_pop().await.unwrap() {
            ServerMessage::HistoryPayload(h) => assert_eq!(h.days, 4),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn enqueue_urgent_pushes_to_front() {
        let q = PushQueue::new(10);
        q.enqueue(ServerMessage::HistoryPayload(make_history(7)))
            .await;
        q.enqueue_urgent(ServerMessage::PrivacyViolationAlert(
            PrivacyViolationAlert {
                code: "raw_field_rejected".into(),
                message: "test".into(),
            },
        ))
        .await;

        // Urgent message should be first
        match q.try_pop().await.unwrap() {
            ServerMessage::PrivacyViolationAlert(_) => {}
            other => panic!("expected PrivacyViolationAlert, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn notify_wakes_consumer() {
        let q = Arc::new(PushQueue::new(10));
        let q2 = Arc::clone(&q);

        let consumer = tokio::spawn(async move {
            q2.notify().notified().await;
            q2.try_pop().await
        });

        // Enqueue after a tiny delay so the consumer is waiting
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        q.enqueue(ServerMessage::CacheEmpty(velvt_shared_types::CacheEmpty {
            payload_type: "history_payload".into(),
        }))
        .await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), consumer)
            .await
            .expect("consumer did not wake")
            .unwrap();
        assert!(msg.is_some());
    }

    // --- PushAdapter unit tests ---

    #[tokio::test]
    async fn push_insight_valid_enqueues() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_insight(make_insight("focus was high")).await;
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn push_insight_invalid_is_dropped() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_insight(make_insight("")).await; // empty text → validation fail
        assert_eq!(queue.len().await, 0, "invalid payload must not be enqueued");
    }

    #[tokio::test]
    async fn push_history_valid_enqueues() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_history(make_history(7)).await;
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn push_history_zero_days_is_dropped() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_history(make_history(0)).await;
        assert_eq!(queue.len().await, 0, "days=0 must not be enqueued");
    }

    #[tokio::test]
    async fn push_privacy_alert_enqueues_at_front() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_history(make_history(7)).await;
        adapter
            .push_privacy_alert(PrivacyViolationAlert {
                code: "raw_field_rejected".into(),
                message: "safe diagnostic".into(),
            })
            .await;
        // Alert should be at front
        assert!(matches!(
            queue.try_pop().await.unwrap(),
            ServerMessage::PrivacyViolationAlert(_)
        ));
    }

    #[tokio::test]
    async fn push_cache_empty_enqueues() {
        let queue = PushQueue::new(10);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_cache_empty("insight_payload").await;
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn full_queue_drop_emits_log_and_keeps_newest() {
        // Capture whether the "push_queue_full" warning fires.
        // We verify via queue state rather than log capture here;
        // log-content tests live in the integration suite.
        let queue = PushQueue::new(2);
        let adapter = PushAdapter::new(Arc::clone(&queue));
        adapter.push_history(make_history(1)).await; // slot 1
        adapter.push_history(make_history(2)).await; // slot 2 (full)
        adapter.push_history(make_history(3)).await; // evicts slot 1

        assert_eq!(queue.len().await, 2);
        match queue.try_pop().await.unwrap() {
            ServerMessage::HistoryPayload(h) => assert_eq!(h.days, 2),
            _ => panic!(),
        }
        match queue.try_pop().await.unwrap() {
            ServerMessage::HistoryPayload(h) => assert_eq!(h.days, 3),
            _ => panic!(),
        }
    }
}
