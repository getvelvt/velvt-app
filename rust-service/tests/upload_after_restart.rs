//! helper-backend-02: after a restart, the retry loop scans the queue before
//! the app has handed its session over. Every due batch failed then as
//! `authentication_required` and was put off fifteen minutes, while new
//! uploads went through and the menu said "Sign in required".
//!
//! These drive the real retry loop, `AuthManager` and `HttpBatchUploader` over
//! a SQLite queue, with the cloud answering every batch with 202.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use velvt_service::auth::{
    AccountAuthService, AuthError, AuthManager, AuthState, AuthStateMachine, FakeTokenStore,
    HttpClient, HttpRequest, HttpResponse, TokenStore,
};
use velvt_service::persistence::{
    BatchEvent, NewUploadBatch, SqlitePersistence, UploadBatchRepo, UploadBatchStatus,
};
use velvt_service::upload::{
    FakePrivacyAlertSink, HttpBatchUploader, SessionOwnership, UploadCoordinator,
};
use velvt_shared_types::AuthSession;

#[tokio::test]
async fn a_batch_due_at_start_waits_for_the_session_instead_of_failing() {
    let restart = Restart::new();
    restart.queue("batch-due");

    restart.start_retry_loop();
    // The loop's first scan runs with no session, as at every start.
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    let held = restart.batch("batch-due");
    assert_eq!(held.0, UploadBatchStatus::Pending);
    assert_eq!(held.1, 0, "no attempt was spent on a batch with no session");
    assert_eq!(
        restart.repo.queue_diagnostics().unwrap().last_error_code,
        None,
        "and it was not marked authentication_required"
    );
    assert!(restart.cloud.batch_posts() == 0);

    restart.hand_over_session();

    restart
        .eventually_sent("batch-due", "the batch goes once the session arrives")
        .await;
}

/// A batch already put off as `authentication_required` (by a build before
/// this one, or by a session the cloud really did refuse) is made due the
/// moment a session is usable, rather than waiting out fifteen minutes.
#[tokio::test]
async fn a_batch_put_off_for_authentication_goes_once_a_session_is_usable() {
    let restart = Restart::new();
    restart.queue("batch-put-off");
    restart
        .repo
        .mark_failed(
            "batch-put-off",
            Utc::now() + Duration::minutes(15),
            "authentication_required",
        )
        .unwrap();

    restart.start_retry_loop();
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    assert_eq!(restart.cloud.batch_posts(), 0);

    restart.hand_over_session();

    restart
        .eventually_sent("batch-put-off", "the put-off batch is due again")
        .await;
}

struct Restart {
    persistence: SqlitePersistence,
    repo: Arc<dyn UploadBatchRepo>,
    tokens: Arc<FakeTokenStore>,
    auth: Arc<AuthStateMachine>,
    cloud: Arc<AcceptingCloud>,
    account: AccountAuthService,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Restart {
    fn new() -> Self {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let repo = persistence.upload_batch_repo();
        let tokens = Arc::new(FakeTokenStore::default());
        let auth = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
        let cloud = Arc::new(AcceptingCloud::default());
        let account = AccountAuthService::new(
            Arc::clone(&cloud) as Arc<dyn HttpClient>,
            Arc::clone(&cloud) as Arc<dyn HttpClient>,
            Arc::clone(&tokens) as Arc<dyn TokenStore>,
            Arc::clone(&auth),
        );
        let (shutdown, _) = tokio::sync::watch::channel(false);
        Self {
            persistence,
            repo,
            tokens,
            auth,
            cloud,
            account,
            shutdown,
        }
    }

    fn queue(&self, batch_id: &str) {
        self.repo
            .insert_batch_with_events(
                &NewUploadBatch {
                    batch_id: batch_id.into(),
                    owner_user_id: Some("account-a".into()),
                },
                &[BatchEvent {
                    event_id: format!("{batch_id}-event"),
                    stable_id: "abs_restart".into(),
                    label: "document:edit".into(),
                    category: "FOCUS_WORK".into(),
                    taxonomy_version: "mvp-1".into(),
                    classification_tier: "fallback".into(),
                    occurred_at: Utc::now() - Duration::minutes(5),
                    duration_seconds: 60,
                }],
            )
            .unwrap();
    }

    /// The service's retry loop as `main.rs` starts it, with a scan interval
    /// long enough that only a session change can wake it in a test.
    fn start_retry_loop(&self) {
        let manager = Arc::new(AuthManager::new(
            Arc::clone(&self.tokens),
            Arc::clone(&self.cloud),
            Arc::clone(&self.auth),
            chrono::Duration::minutes(5),
        ));
        let coordinator = UploadCoordinator::new(
            Arc::clone(&self.repo),
            HttpBatchUploader::new(manager),
            FakePrivacyAlertSink::default(),
        )
        .with_ownership_policy(Arc::new(SessionOwnership::new(
            Arc::clone(&self.tokens) as Arc<dyn TokenStore>,
            self.auth.subscribe(),
        )));
        let shutdown = self.shutdown.subscribe();
        let session = self.auth.subscribe();
        tokio::spawn(async move {
            coordinator
                .run_retry_loop(
                    StdDuration::from_secs(3_600),
                    shutdown,
                    Some(session),
                    "1",
                    "test",
                )
                .await;
        });
    }

    /// What the app does half a second after the service starts.
    fn hand_over_session(&self) {
        self.account.apply_session(AuthSession {
            device_id: "device-a".into(),
            access_token: "device-access".into(),
            refresh_token: "device-refresh".into(),
            expires_at: Utc::now() + Duration::hours(1),
            user_access_token: None,
            user_refresh_token: None,
            user_expires_at: None,
            user_id: Some("account-a".into()),
        });
        assert!(matches!(
            self.auth.current(),
            AuthState::Authenticated { .. }
        ));
    }

    fn batch(&self, batch_id: &str) -> (UploadBatchStatus, u32) {
        let status = self.repo.batch_status(batch_id).unwrap();
        let attempts = self
            .repo
            .pending_batches()
            .unwrap()
            .into_iter()
            .find(|batch| batch.batch_id == batch_id)
            .map_or(0, |batch| batch.attempt_count);
        (status, attempts)
    }

    async fn eventually_sent(&self, batch_id: &str, what: &str) {
        let deadline = Instant::now() + StdDuration::from_secs(5);
        while self.repo.batch_status(batch_id).unwrap() != UploadBatchStatus::Sent {
            assert!(Instant::now() < deadline, "never happened: {what}");
            tokio::time::sleep(StdDuration::from_millis(5)).await;
        }
        assert_eq!(self.cloud.batch_posts(), 1);
        let _ = &self.persistence;
    }
}

impl Drop for Restart {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Answers every batch with 202, and counts them.
#[derive(Default)]
struct AcceptingCloud {
    posts: Mutex<usize>,
}

impl AcceptingCloud {
    fn batch_posts(&self) -> usize {
        *self.posts.lock().unwrap()
    }
}

impl HttpClient for AcceptingCloud {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        assert_eq!(request.path, "/v1/events/batches", "only batches are sent");
        assert!(request.authorization.is_some(), "a batch carries the token");
        *self.posts.lock().unwrap() += 1;
        Box::pin(async {
            Ok(HttpResponse {
                status: 202,
                error_code: None,
                tokens: None,
                retry_after: None,
                message: None,
                raw_body: None,
                user_id: None,
                device_id: None,
            })
        })
    }
}
