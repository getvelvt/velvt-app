use super::{
    BatchEventPayload, BatchPayload, BatchUploadError, BatchUploader, HostBackoff, UploadOutcome,
};
use crate::auth::{AuthState, TokenStore};
use crate::persistence::{BatchEvent, NewUploadBatch, PersistenceError, UploadBatchRepo};
use chrono::{Duration, Utc};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};
use tokio::sync::watch;

/// What may be done with a queued batch now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchDisposition {
    /// Send it under the current session.
    Send,
    /// Leave it queued and untouched. No session can send it now, but the
    /// account it belongs to may sign in again.
    Hold,
    /// Delete it and make its events local-only: another account is signed
    /// in, and no session on this Mac will ever be its owner's.
    Discard,
}

/// Decides whose token may send a queued batch, from the account the batch
/// was recorded under.
///
/// Every send path asks: `upload_batch` (a batch just closed), `resume_pending`
/// (the retry loop) and `flush_all_pending` ("Send all now"). A rule enforced on
/// the retry loop alone would be one button press away from being ignored.
pub trait BatchOwnershipPolicy: Send + Sync {
    fn disposition(&self, owner_user_id: Option<&str>) -> BatchDisposition;
}

/// Sends every batch. What a coordinator is constructed with, for the tests
/// that model the upload path alone; the service installs
/// [`SessionOwnership`].
#[derive(Debug, Default)]
pub struct SendEveryBatch;

impl BatchOwnershipPolicy for SendEveryBatch {
    fn disposition(&self, _owner_user_id: Option<&str>) -> BatchDisposition {
        BatchDisposition::Send
    }
}

/// The service's rule: a batch is sent only under its own account's session.
///
/// - No usable session (signed out, the session needs reauthentication, the
///   device was revoked or the account deleted): hold everything. Sending
///   then failed every due batch as `authentication_required` and put it off
///   fifteen minutes, which after every restart meant the batches due before
///   the client handed its session over (helper-backend-02).
/// - A batch with no owner was queued before migration 0042, or under a
///   client that named no account: sent, as it was before owners existed.
/// - The signed-in account's own batch: sent.
/// - Another account's batch: discarded. A batch queued under account A was
///   sent under B's token after Log Out and a sign-in to B, and a deleted
///   account's queue was sent under the next account to sign in.
/// - Signed in, but the service does not know the account: held, because
///   whose it is cannot be told.
pub struct SessionOwnership {
    tokens: Arc<dyn TokenStore>,
    auth_state: watch::Receiver<AuthState>,
}

impl SessionOwnership {
    pub fn new(tokens: Arc<dyn TokenStore>, auth_state: watch::Receiver<AuthState>) -> Self {
        Self { tokens, auth_state }
    }
}

impl BatchOwnershipPolicy for SessionOwnership {
    fn disposition(&self, owner_user_id: Option<&str>) -> BatchDisposition {
        if !session_usable(&self.auth_state.borrow()) {
            return BatchDisposition::Hold;
        }
        let Some(owner_user_id) = owner_user_id else {
            return BatchDisposition::Send;
        };
        match self.tokens.load_user_id() {
            Ok(Some(current)) if current == owner_user_id => BatchDisposition::Send,
            Ok(Some(_)) => BatchDisposition::Discard,
            Ok(None) | Err(_) => BatchDisposition::Hold,
        }
    }
}

/// Whether the device holds a session that can send: `RefreshInFlight` is
/// mid-roundtrip with a valid refresh token.
fn session_usable(state: &AuthState) -> bool {
    matches!(
        state,
        AuthState::Authenticated { .. } | AuthState::RefreshInFlight
    )
}

pub trait PrivacyAlertSink: Send + Sync {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

#[derive(Clone)]
pub struct IpcPrivacyAlertSink {
    sender: tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>,
}

impl IpcPrivacyAlertSink {
    pub fn new(
        sender: tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>,
    ) -> Self {
        Self { sender }
    }
}

impl PrivacyAlertSink for IpcPrivacyAlertSink {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.sender.send(velvt_shared_types::PrivacyViolationAlert {
                code: "raw_field_rejected".into(),
                message: message.to_owned(),
            });
        })
    }
}

#[derive(Clone, Default)]
pub struct FakePrivacyAlertSink(Arc<Mutex<Vec<String>>>);

impl FakePrivacyAlertSink {
    pub fn alert_count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

impl PrivacyAlertSink for FakePrivacyAlertSink {
    fn alert<'a>(&'a self, message: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(message.to_owned());
        })
    }
}

pub struct UploadCoordinator<U, A> {
    repository: Arc<dyn UploadBatchRepo>,
    uploader: U,
    alerts: A,
    host: String,
    backoff: Mutex<HostBackoff>,
    ownership: Arc<dyn BatchOwnershipPolicy>,
}

impl<U, A> UploadCoordinator<U, A>
where
    U: BatchUploader,
    A: PrivacyAlertSink,
{
    pub fn new(repository: Arc<dyn UploadBatchRepo>, uploader: U, alerts: A) -> Self {
        Self {
            repository,
            uploader,
            alerts,
            host: "dev-api.getvelvt.com".into(),
            backoff: Mutex::new(HostBackoff::production(
                StdDuration::from_secs(30),
                StdDuration::from_secs(15 * 60),
            )),
            ownership: Arc::new(SendEveryBatch),
        }
    }

    pub fn with_host_and_backoff(mut self, host: impl Into<String>, backoff: HostBackoff) -> Self {
        self.host = host.into();
        self.backoff = Mutex::new(backoff);
        self
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    pub fn with_ownership_policy(mut self, ownership: Arc<dyn BatchOwnershipPolicy>) -> Self {
        self.ownership = ownership;
        self
    }

    /// Sends a batch that was just persisted, if its account's session is
    /// the one signed in. A held batch stays queued for the retry loop.
    pub async fn upload_batch(&self, batch: BatchPayload) -> Result<(), CoordinatorError> {
        match self.ownership.disposition(batch.owner_user_id.as_deref()) {
            BatchDisposition::Send => {
                self.upload_batch_with_backoff(batch, true).await?;
            }
            BatchDisposition::Hold => {}
            BatchDisposition::Discard => self.discard_disowned(&batch.batch_id)?,
        }
        Ok(())
    }

    /// Sends one batch, or defers it while the host is in backoff, and says
    /// whether the host can be sent another batch now.
    async fn upload_batch_with_backoff(
        &self,
        batch: BatchPayload,
        respect_host_backoff: bool,
    ) -> Result<HostAvailability, CoordinatorError> {
        if respect_host_backoff {
            if let Some(next_attempt_at) = self.repository.host_backoff_until(&self.host)? {
                if next_attempt_at > Utc::now() {
                    // Not an attempt: nothing was sent, so nothing is counted
                    // against the batch's ceiling. Counting deferrals spent
                    // every queued batch's attempts at the rate the retry scan
                    // ran, so an outage of about three days abandoned the
                    // whole queue although only the first batch of each scan
                    // had ever been tried.
                    self.repository.defer_batch(
                        &batch.batch_id,
                        next_attempt_at,
                        "host_backoff",
                    )?;
                    return Ok(HostAvailability::InBackoff);
                }
            }
        }
        let outcome = match self.uploader.upload(&batch).await {
            Ok(outcome) => outcome,
            Err(BatchUploadError::Transport) => {
                self.schedule_network_retry(&batch.batch_id)?;
                return Ok(HostAvailability::InBackoff);
            }
            Err(BatchUploadError::AuthenticationRequired) => {
                let next_attempt_at = Utc::now() + Duration::minutes(15);
                self.repository.mark_failed(
                    &batch.batch_id,
                    next_attempt_at,
                    "authentication_required",
                )?;
                tracing::warn!(
                    error_code = "upload_authentication_required",
                    "upload paused pending device reauthentication"
                );
                return Ok(HostAvailability::Available);
            }
        };
        match outcome {
            UploadOutcome::Accepted | UploadOutcome::Duplicate => {
                self.repository.mark_sent(&batch.batch_id)?;
                self.backoff.lock().unwrap().reset(&self.host);
                self.repository.clear_host_backoff(&self.host)?;
            }
            UploadOutcome::RawFieldRejected { message: _ } => {
                // SECURITY: raw_field_rejected is a hard privacy signal. It is
                // permanently terminal and must never enter retry scheduling.
                self.repository
                    .mark_rejected(&batch.batch_id, "raw_field_rejected")?;
                tracing::error!(
                    error_code = "raw_field_rejected",
                    batch_id = batch.batch_id,
                    "cloud rejected a batch for a privacy violation"
                );
                self.alerts
                    .alert("Sensitive activity details were blocked before upload.")
                    .await;
            }
            UploadOutcome::RateLimited { retry_after } => {
                let attempt = self.repository.host_backoff_attempt(&self.host)?;
                self.backoff
                    .lock()
                    .unwrap()
                    .set_attempt(&self.host, attempt);
                let delay = self
                    .backoff
                    .lock()
                    .unwrap()
                    .next_delay(&self.host, retry_after.as_deref());
                let next_attempt_at = Utc::now()
                    + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
                self.repository.set_host_backoff(
                    &self.host,
                    attempt.saturating_add(1),
                    next_attempt_at,
                )?;
                self.repository
                    .mark_failed(&batch.batch_id, next_attempt_at, "rate_limited")?;
                return Ok(HostAvailability::InBackoff);
            }
            UploadOutcome::Retryable { code } => {
                let attempt = self.repository.host_backoff_attempt(&self.host)?;
                self.backoff
                    .lock()
                    .unwrap()
                    .set_attempt(&self.host, attempt);
                let delay = self.backoff.lock().unwrap().next_delay(&self.host, None);
                let next_attempt_at = Utc::now()
                    + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
                self.repository.set_host_backoff(
                    &self.host,
                    attempt.saturating_add(1),
                    next_attempt_at,
                )?;
                self.repository
                    .mark_failed(&batch.batch_id, next_attempt_at, &code)?;
                return Ok(HostAvailability::InBackoff);
            }
        }
        Ok(HostAvailability::Available)
    }

    /// Deletes a batch recorded under an account other than the one signed
    /// in. Its events become local-only in the same transaction, so nothing
    /// re-queues them.
    fn discard_disowned(&self, batch_id: &str) -> Result<(), CoordinatorError> {
        match self.repository.discard_batch(batch_id) {
            Ok(()) | Err(PersistenceError::NotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        tracing::info!(
            batch_id,
            reason = "another_account",
            "discarded upload batch recorded under another account"
        );
        Ok(())
    }

    /// "Send all now": sends every queued batch, the first one even while the
    /// host is in backoff, because a person asked.
    ///
    /// Stops at the first batch that leaves the host in backoff (a transport
    /// failure, a rate limit, a retryable status) and leaves the rest where
    /// they are, under the backoff that failure set. Sending on regardless
    /// cost each remaining batch a full request timeout against a host that
    /// had just failed to answer, one after another.
    pub async fn flush_all_pending(
        &self,
        schema_version: &str,
        client_version: &str,
    ) -> Result<usize, CoordinatorError> {
        let batches = self.repository.pending_batches()?;
        let count = batches.len();
        for batch in batches {
            match self.ownership.disposition(batch.owner_user_id.as_deref()) {
                BatchDisposition::Send => {}
                BatchDisposition::Hold => continue,
                BatchDisposition::Discard => {
                    self.discard_disowned(&batch.batch_id)?;
                    continue;
                }
            }
            let availability = self
                .upload_batch_with_backoff(
                    payload_for_queued_batch(batch, schema_version, client_version),
                    false,
                )
                .await?;
            if availability == HostAvailability::InBackoff {
                break;
            }
        }
        Ok(count)
    }

    pub async fn submit_batch(&self, batch: BatchPayload) -> Result<(), CoordinatorError> {
        self.persist_batch(&batch)?;
        self.upload_batch(batch).await
    }

    pub(crate) fn persist_batch(&self, batch: &BatchPayload) -> Result<(), CoordinatorError> {
        let events = batch
            .events
            .iter()
            .map(|event| BatchEvent {
                event_id: event.event_id.clone(),
                stable_id: event.stable_id.clone(),
                label: event.label.clone(),
                category: event.category.clone(),
                taxonomy_version: event.taxonomy_version.clone(),
                classification_tier: event.classification_tier.clone(),
                occurred_at: event.occurred_at,
                duration_seconds: event.duration_seconds,
            })
            .collect::<Vec<_>>();
        self.repository.insert_batch_with_events(
            &NewUploadBatch {
                batch_id: batch.batch_id.clone(),
                owner_user_id: batch.owner_user_id.clone(),
            },
            &events,
        )?;
        Ok(())
    }

    pub async fn resume_pending(
        &self,
        schema_version: &str,
        client_version: &str,
    ) -> Result<usize, CoordinatorError> {
        let batches = self.repository.resumable_batches(Utc::now())?;
        let count = batches.len();
        for batch in batches {
            self.upload_batch(payload_for_queued_batch(
                batch,
                schema_version,
                client_version,
            ))
            .await?;
        }
        Ok(count)
    }

    fn schedule_network_retry(&self, batch_id: &str) -> Result<(), CoordinatorError> {
        let attempt = self.repository.host_backoff_attempt(&self.host)?;
        self.backoff
            .lock()
            .unwrap()
            .set_attempt(&self.host, attempt);
        let delay = self.backoff.lock().unwrap().next_delay(&self.host, None);
        let next_attempt_at =
            Utc::now() + Duration::from_std(delay).unwrap_or_else(|_| Duration::minutes(15));
        self.repository
            .set_host_backoff(&self.host, attempt.saturating_add(1), next_attempt_at)?;
        self.repository
            .mark_pending_retry(batch_id, next_attempt_at, "transport")?;
        Ok(())
    }

    /// Resumes due batches every `interval`, and at once whenever a session
    /// becomes usable.
    ///
    /// `session` is the auth state: while it holds no usable session the loop
    /// sends nothing (the ownership policy holds every batch), and the moment
    /// one arrives, batches put off as `authentication_required` are made due
    /// and the queue is resumed. After a restart that is the client handing
    /// its session over, half a second after the loop's first scan; the
    /// batches that scan reached used to wait fifteen minutes.
    pub async fn run_retry_loop(
        &self,
        interval: StdDuration,
        mut shutdown: watch::Receiver<bool>,
        mut session: Option<watch::Receiver<AuthState>>,
        schema_version: &str,
        client_version: &str,
    ) {
        let mut was_usable = session
            .as_ref()
            .is_some_and(|session| session_usable(&session.borrow()));
        loop {
            if *shutdown.borrow() {
                return;
            }
            if self
                .resume_pending(schema_version, client_version)
                .await
                .is_err()
            {
                tracing::error!(
                    error_code = "upload_recovery_failed",
                    "failed to resume persisted upload batches"
                );
            }
            let next_scan = tokio::time::Instant::now() + interval;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(next_scan) => break,
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                    }
                    changed = session_changed(session.as_mut()) => {
                        if changed.is_err() {
                            session = None;
                            continue;
                        }
                        let usable = session
                            .as_ref()
                            .is_some_and(|session| session_usable(&session.borrow()));
                        let became_usable = usable && !was_usable;
                        was_usable = usable;
                        if became_usable {
                            self.reschedule_authentication_required();
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Makes due every batch that failed for want of a session.
    pub fn reschedule_authentication_required(&self) {
        match self
            .repository
            .reschedule_authentication_required(Utc::now())
        {
            Ok(0) => {}
            Ok(rescheduled) => tracing::info!(
                rescheduled_batches = rescheduled,
                "session usable; batches put off for authentication are due now"
            ),
            Err(error) => tracing::error!(
                error_code = "upload_reschedule_failed",
                error = %error,
                "failed to reschedule batches put off for authentication"
            ),
        }
    }
}

/// Resolves when `session` changes, and never when there is none to watch.
async fn session_changed(
    session: Option<&mut watch::Receiver<AuthState>>,
) -> Result<(), watch::error::RecvError> {
    match session {
        Some(session) => session.changed().await,
        None => std::future::pending().await,
    }
}

/// The payload a batch already on disk is sent as.
///
/// `resume_pending` and `flush_all_pending` send exactly this, and
/// `velvt-service --dry-run-egress` prints it, so the three cannot disagree
/// about what a queued batch looks like on the wire.
pub fn payload_for_queued_batch(
    batch: crate::persistence::UploadBatch,
    schema_version: &str,
    client_version: &str,
) -> BatchPayload {
    let taxonomy = batch
        .events
        .first()
        .map(|event| event.taxonomy_version.clone())
        .unwrap_or_default();
    let events: Vec<_> = batch
        .events
        .into_iter()
        .map(|event| BatchEventPayload {
            event_id: event.event_id,
            stable_id: event.stable_id,
            label: event.label,
            category: event.category,
            taxonomy_version: event.taxonomy_version,
            classification_tier: event.classification_tier,
            occurred_at: event.occurred_at,
            duration_seconds: event.duration_seconds,
        })
        .collect();
    let supported_abstraction_types = unique_abstraction_types(&events);
    BatchPayload::new(
        batch.batch_id,
        schema_version,
        client_version,
        supported_abstraction_types,
        taxonomy,
        events,
    )
    .owned_by(batch.owner_user_id)
}

fn unique_abstraction_types(events: &[BatchEventPayload]) -> Vec<String> {
    let mut types = Vec::new();
    for event in events {
        if !types.contains(&event.label) {
            types.push(event.label.clone());
        }
    }
    types
}

/// What a send left the upload host in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostAvailability {
    Available,
    /// The host is in backoff: this send set it, or found it set.
    InBackoff,
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    #[error(transparent)]
    Upload(#[from] BatchUploadError),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}
