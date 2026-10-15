use super::{
    AuthState, AuthStateMachine, AuthTransitionError, HttpClient, HttpRequest, HttpResponse,
    TokenStore, TokenStoreError,
};
use chrono::{Duration, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// How long a refresh that failed in transport is remembered before the next
/// request tries again.
const REFRESH_TRANSPORT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

pub trait SessionValidator: Send + Sync {
    fn validate_restored_session<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>>;
}

pub struct AuthManager<S, H> {
    store: Arc<S>,
    http: Arc<H>,
    state: Arc<AuthStateMachine>,
    refresh_buffer: Duration,
    refresh_lock: Mutex<()>,
    /// When a refresh or a reissue last failed in transport or got no
    /// verdict on the session, until one gets one.
    refresh_transport_failed_at: std::sync::Mutex<Option<Instant>>,
    refresh_transport_retry_after: std::time::Duration,
}

impl<S, H> AuthManager<S, H>
where
    S: TokenStore,
    H: HttpClient,
{
    pub fn new(
        store: Arc<S>,
        http: Arc<H>,
        state: Arc<AuthStateMachine>,
        refresh_buffer: Duration,
    ) -> Self {
        Self {
            store,
            http,
            state,
            refresh_buffer,
            refresh_lock: Mutex::new(()),
            refresh_transport_failed_at: std::sync::Mutex::new(None),
            refresh_transport_retry_after: REFRESH_TRANSPORT_RETRY_AFTER,
        }
    }

    /// Replaces `REFRESH_TRANSPORT_RETRY_AFTER`, for a test that cannot wait
    /// a minute.
    pub fn with_refresh_transport_retry_after(mut self, retry_after: std::time::Duration) -> Self {
        self.refresh_transport_retry_after = retry_after;
        self
    }

    pub async fn send_authenticated(
        &self,
        mut request: HttpRequest,
    ) -> Result<HttpResponse, AuthError> {
        match self.state.current() {
            AuthState::DeviceRevoked => return Err(AuthError::DeviceRevoked),
            AuthState::AccountDeleted => return Err(AuthError::AccountDeleted),
            _ => {}
        }
        let account = match request.account.clone() {
            Some(account) => Some(account),
            None => self.store.load_user_id()?,
        };
        let account = account.as_deref();
        let tokens = self.tokens_for_request().await?;
        self.ensure_bound_session(account)?;
        request.authorization = Some(tokens.access_token().clone());
        let response = self.http.send(request.clone()).await?;
        if refusal(&response) == Refusal::SessionRejected {
            self.ensure_bound_session(account)?;
            let fresh = self.refresh_tokens(true).await?;
            self.ensure_bound_session(account)?;
            request.authorization = Some(fresh.access_token().clone());
            let retried = self.http.send(request.clone()).await?;
            return self.handle_response(retried, request, account).await;
        }
        self.handle_response(response, request, account).await
    }

    /// Refuses to send a request under any session but its account's: the
    /// one it names (`HttpRequest::account`; an upload names its batch's
    /// owner), else the one signed in when it started.
    ///
    /// The tokens a request is sent with are read when it is sent, not when
    /// it was decided on. One waiting on the refresh lock behind a slow
    /// refresh, or on its own refresh and retry, while Log Out and a sign-in
    /// replace the session, is handed the new account's tokens: a batch the
    /// ownership policy had judged account A's was POSTed under B's token
    /// that way. It fails as `NeedsReauth` instead, and moves no state: the
    /// upload path keeps the batch as `authentication_required`, and the
    /// ownership policy judges it again on its next pass.
    ///
    /// Called after the tokens are read. Every path that puts another
    /// account's session in place clears the account id before it stores
    /// the new tokens, and stores the new account id last
    /// (`AccountAuthService::store_session`), so tokens read from that
    /// session are never seen beside the old account id. A request with no
    /// account to bind to (a session handed over without one, before the
    /// cloud named it) is sent as before.
    fn ensure_bound_session(&self, account: Option<&str>) -> Result<(), AuthError> {
        let Some(account) = account else {
            return Ok(());
        };
        match self.store.load_user_id() {
            Ok(Some(current)) if current == account => Ok(()),
            _ => {
                tracing::info!(
                    error_code = "request_session_replaced",
                    "the session a request was bound to ended before it was sent; not sent"
                );
                Err(AuthError::NeedsReauth)
            }
        }
    }

    /// Asks the cloud whether the restored session still stands, and keeps
    /// the account id it answers with when the client did not hand one over.
    pub async fn validate_session(&self) -> Result<(), AuthError> {
        let response = self
            .send_authenticated(HttpRequest::get("/v1/auth/session"))
            .await?;
        match response.status {
            200..=299 => {
                if let Some(user_id) = response.user_id.as_deref() {
                    if self.store.load_user_id()?.is_none() {
                        self.store.store_user_id(user_id)?;
                    }
                }
                Ok(())
            }
            429 => Err(AuthError::RateLimited),
            _ => Err(AuthError::InvalidResponse),
        }
    }

    /// The device tokens to send a request with, refreshed first when they
    /// are within `refresh_buffer` of expiring.
    ///
    /// A refresh that fails in transport does not stop a token that has not
    /// expired from being used: the cloud still accepts it. The failure is
    /// remembered for `REFRESH_TRANSPORT_RETRY_AFTER`, so requests in that
    /// time neither try the refresh again nor wait on it; one that needs a
    /// refresh because its token has expired fails at once with `Transport`.
    async fn tokens_for_request(&self) -> Result<super::TokenPair, AuthError> {
        let tokens = self.store.load_tokens()?.ok_or(AuthError::NeedsReauth)?;
        if tokens.expires_at() > Utc::now() + self.refresh_buffer {
            return Ok(tokens);
        }
        let still_valid = |tokens: &super::TokenPair| tokens.expires_at() > Utc::now();
        if self.refresh_recently_failed_in_transport() {
            return if still_valid(&tokens) {
                Ok(tokens)
            } else {
                Err(AuthError::Transport)
            };
        }
        match self.refresh_tokens(false).await {
            Err(AuthError::Transport | AuthError::RateLimited) if still_valid(&tokens) => {
                Ok(tokens)
            }
            result => result,
        }
    }

    fn refresh_recently_failed_in_transport(&self) -> bool {
        self.refresh_transport_failed_at
            .lock()
            .ok()
            .and_then(|failed_at| *failed_at)
            .is_some_and(|failed_at| failed_at.elapsed() < self.refresh_transport_retry_after)
    }

    fn record_refresh_transport(&self, failed: bool) {
        if let Ok(mut failed_at) = self.refresh_transport_failed_at.lock() {
            *failed_at = failed.then(Instant::now);
        }
    }

    /// Exchanges the refresh token for new device tokens.
    ///
    /// `RefreshInFlight` is published only when the current access token can
    /// no longer be used — it has expired, or the cloud refused it (`force`).
    /// A refresh ahead of expiry leaves the device `Authenticated` throughout,
    /// so one that fails in transport changes no state at all; publishing the
    /// roundtrip made every failed attempt flip the state Swift shows twice.
    async fn refresh_tokens(&self, force: bool) -> Result<super::TokenPair, AuthError> {
        let _guard = self.refresh_lock.lock().await;
        let tokens = self.store.load_tokens()?.ok_or(AuthError::NeedsReauth)?;
        if !force && tokens.expires_at() > Utc::now() + self.refresh_buffer {
            return Ok(tokens);
        }
        if force && self.refresh_recently_failed_in_transport() {
            return Err(AuthError::Transport);
        }
        let device_id = match self.state.current() {
            AuthState::Authenticated { device_id } => device_id,
            AuthState::RefreshInFlight => return Err(AuthError::NeedsReauth),
            AuthState::DeviceRevoked => return Err(AuthError::DeviceRevoked),
            AuthState::AccountDeleted => return Err(AuthError::AccountDeleted),
            AuthState::Unauthenticated | AuthState::NeedsReauth => {
                return Err(AuthError::NeedsReauth)
            }
        };
        let token_unusable = force || tokens.expires_at() <= Utc::now();
        if token_unusable {
            self.state.transition(AuthState::RefreshInFlight)?;
        }
        let mut refresh_request = HttpRequest::post("/v1/auth/refresh");
        refresh_request.refresh_token = Some(tokens.refresh_token().clone());
        let response = match self.http.send(refresh_request).await {
            Ok(response) => response,
            Err(error) => {
                if matches!(error, AuthError::Transport) {
                    self.record_refresh_transport(true);
                }
                if self.session_replaced_since(Some(tokens.refresh_token())) {
                    return Err(AuthError::NeedsReauth);
                }
                if token_unusable {
                    self.state
                        .transition(AuthState::Authenticated { device_id })?;
                }
                return Err(error);
            }
        };
        self.record_refresh_transport(false);
        if self.session_replaced_since(Some(tokens.refresh_token())) {
            return Err(AuthError::NeedsReauth);
        }
        if response.status == 200 {
            let Some(fresh) = response.tokens else {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(AuthError::InvalidResponse);
            };
            if let Err(error) = self.store.store_pair(fresh.clone()) {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(error.into());
            }
            self.state
                .transition(AuthState::Authenticated { device_id })?;
            return Ok(fresh);
        }
        match refusal(&response) {
            Refusal::DeviceRevoked => {
                self.state.transition(AuthState::DeviceRevoked)?;
                Err(AuthError::DeviceRevoked)
            }
            Refusal::AccountDeleted => {
                self.state.transition(AuthState::AccountDeleted)?;
                Err(AuthError::AccountDeleted)
            }
            Refusal::DeviceTokenRevoked => {
                self.state
                    .transition(AuthState::Authenticated { device_id })?;
                drop(_guard);
                self.reissue_device_tokens().await
            }
            Refusal::SessionRejected => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            // An edge, an outage or a rate limit answered instead of the
            // cloud's auth service. The session stands: an unexpired token
            // goes on being used, and the refresh is tried again after
            // `refresh_transport_retry_after`, as after a transport failure.
            Refusal::Unattributed => {
                self.record_refresh_transport(true);
                if token_unusable {
                    self.state
                        .transition(AuthState::Authenticated { device_id })?;
                }
                tracing::warn!(
                    error_code = "token_refresh_unattributed_refusal",
                    status = response.status,
                    "token refresh was refused without a session error code; kept the session"
                );
                Err(transient_error(response.status))
            }
        }
    }

    /// Whether the session a refresh or reissue started from was ended or
    /// replaced while its request was in flight.
    ///
    /// `log_out` clears the tokens and moves to `Unauthenticated` without
    /// waiting on the refresh lock, and a login stores its own tokens the
    /// same way. A response that arrives afterwards belongs to the old
    /// session: storing its tokens would sign a signed-out device back in, or
    /// overwrite the new login's tokens, so it is dropped and nothing is
    /// stored or transitioned.
    fn session_replaced_since(&self, started_from: Option<&super::RedactedString>) -> bool {
        if self.state.current() == AuthState::Unauthenticated {
            return true;
        }
        // A store that cannot be read says nothing about a sign-out; the
        // store write that follows reports its own failure.
        let Ok(current) = self.store.load_tokens() else {
            return false;
        };
        current.as_ref().map(super::TokenPair::refresh_token) != started_from
    }

    async fn handle_response(
        &self,
        response: HttpResponse,
        mut request: HttpRequest,
        account: Option<&str>,
    ) -> Result<HttpResponse, AuthError> {
        match refusal(&response) {
            Refusal::DeviceTokenRevoked => {
                self.ensure_bound_session(account)?;
                let fresh = self.reissue_device_tokens().await?;
                self.ensure_bound_session(account)?;
                request.authorization = Some(fresh.access_token().clone());
                let retried = self.http.send(request).await?;
                let verdict = refusal(&retried);
                self.settle(retried, verdict)
            }
            verdict => self.settle(response, verdict),
        }
    }

    /// Ends the session only on an error code the cloud attributes to it.
    ///
    /// Everything else goes back to the caller as the response it is: a 403
    /// with no code is a Cloudflare WAF rule, Bot Fight Mode or a challenge,
    /// and a 5xx is an outage. Each caller already backs off on a status it
    /// cannot use. Ending the session there signed the person out and wiped
    /// the Keychain on one edge response, with nothing to recover it.
    fn settle(&self, response: HttpResponse, verdict: Refusal) -> Result<HttpResponse, AuthError> {
        match verdict {
            Refusal::DeviceRevoked => {
                self.state.transition(AuthState::DeviceRevoked)?;
                Err(AuthError::DeviceRevoked)
            }
            Refusal::AccountDeleted => {
                self.state.transition(AuthState::AccountDeleted)?;
                Err(AuthError::AccountDeleted)
            }
            // A token rejected again straight after a refresh or a reissue
            // is not one more refresh away from working.
            Refusal::SessionRejected | Refusal::DeviceTokenRevoked => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            Refusal::Unattributed if response.status == 429 => Err(AuthError::RateLimited),
            Refusal::Unattributed => {
                if response.status == 401 || response.status == 403 {
                    tracing::warn!(
                        error_code = "unattributed_auth_refusal",
                        status = response.status,
                        "a request was refused without a session error code; kept the session"
                    );
                }
                Ok(response)
            }
        }
    }

    /// Exchanges the user session for new device tokens, after the cloud
    /// answered `device_token_revoked`.
    ///
    /// Backs off as a refresh does: a reissue (or the user-token refresh it
    /// needs) that fails in transport, or that an edge, an outage or a rate
    /// limit answers, is not tried again for `refresh_transport_retry_after`.
    /// Without that, every request answered `device_token_revoked` while the
    /// edge kept refusing POSTed one more reissue.
    async fn reissue_device_tokens(&self) -> Result<super::TokenPair, AuthError> {
        let _guard = self.refresh_lock.lock().await;
        if self.refresh_recently_failed_in_transport() {
            return Err(AuthError::Transport);
        }
        let user_tokens = match self.user_tokens_for_reissue().await {
            Ok(tokens) => tokens,
            // No verdict on the session: the user-token refresh could not be
            // made, or was answered for the cloud by something else.
            Err(error @ (AuthError::Transport | AuthError::RateLimited)) => return Err(error),
            // `refresh_user_tokens` has already moved the state.
            Err(error @ AuthError::AccountDeleted) => return Err(error),
            Err(error) => {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(error);
            }
        };
        let device_id = match self.state.current() {
            AuthState::Authenticated { device_id } => device_id,
            AuthState::DeviceRevoked => return Err(AuthError::DeviceRevoked),
            AuthState::AccountDeleted => return Err(AuthError::AccountDeleted),
            AuthState::Unauthenticated | AuthState::NeedsReauth | AuthState::RefreshInFlight => {
                return Err(AuthError::NeedsReauth);
            }
        };
        let started_from = self
            .store
            .load_tokens()?
            .map(|tokens| tokens.refresh_token().clone());
        self.state.transition(AuthState::RefreshInFlight)?;
        let mut request = HttpRequest::post("/v1/auth/devices/reissue");
        request.authorization = Some(user_tokens.access_token().clone());
        request.json_body = Some(serde_json::json!({ "device_id": device_id }));
        let response = match self.http.send(request).await {
            Ok(response) => response,
            Err(error) => {
                if matches!(error, AuthError::Transport) {
                    self.record_refresh_transport(true);
                }
                if self.session_replaced_since(started_from.as_ref()) {
                    return Err(AuthError::NeedsReauth);
                }
                self.state
                    .transition(AuthState::Authenticated { device_id })?;
                return Err(error);
            }
        };
        self.record_refresh_transport(false);
        if self.session_replaced_since(started_from.as_ref()) {
            return Err(AuthError::NeedsReauth);
        }
        if response.status == 200 {
            let Some(fresh) = response.tokens else {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(AuthError::InvalidResponse);
            };
            if let Err(error) = self.store.store_pair(fresh.clone()) {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(error.into());
            }
            self.state
                .transition(AuthState::Authenticated { device_id })?;
            return Ok(fresh);
        }
        match refusal(&response) {
            Refusal::DeviceRevoked => {
                self.state.transition(AuthState::DeviceRevoked)?;
                Err(AuthError::DeviceRevoked)
            }
            Refusal::AccountDeleted => {
                self.state.transition(AuthState::AccountDeleted)?;
                Err(AuthError::AccountDeleted)
            }
            Refusal::SessionRejected | Refusal::DeviceTokenRevoked => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            // The device row is not this account's any more: a sign-in
            // registers a new one (`AccountAuthService::reissue_device_tokens`).
            Refusal::Unattributed
                if response.status == 404
                    && response.error_code.as_deref() == Some("device_not_found") =>
            {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            Refusal::Unattributed => {
                self.record_refresh_transport(true);
                self.state
                    .transition(AuthState::Authenticated { device_id })?;
                tracing::warn!(
                    error_code = "device_token_reissue_unattributed_refusal",
                    status = response.status,
                    "device token reissue was refused without a session error code; kept the session"
                );
                Err(transient_error(response.status))
            }
        }
    }

    async fn user_tokens_for_reissue(&self) -> Result<super::TokenPair, AuthError> {
        let tokens = self
            .store
            .load_user_tokens()?
            .ok_or(AuthError::NeedsReauth)?;
        if tokens.expires_at() > Utc::now() + self.refresh_buffer {
            return Ok(tokens);
        }
        self.refresh_user_tokens().await
    }

    async fn refresh_user_tokens(&self) -> Result<super::TokenPair, AuthError> {
        let tokens = self
            .store
            .load_user_tokens()?
            .ok_or(AuthError::NeedsReauth)?;
        let mut refresh_request = HttpRequest::post("/v1/auth/refresh");
        refresh_request.refresh_token = Some(tokens.refresh_token().clone());
        let response = self.http.send(refresh_request).await.inspect_err(|error| {
            if matches!(error, AuthError::Transport) {
                self.record_refresh_transport(true);
            }
        })?;
        if response.status == 200 {
            let Some(fresh) = response.tokens else {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(AuthError::InvalidResponse);
            };
            if let Err(error) = self.store.store_user_pair(fresh.clone()) {
                self.state.transition(AuthState::NeedsReauth)?;
                return Err(error.into());
            }
            return Ok(fresh);
        }
        match refusal(&response) {
            Refusal::AccountDeleted => {
                self.state.transition(AuthState::AccountDeleted)?;
                Err(AuthError::AccountDeleted)
            }
            Refusal::SessionRejected | Refusal::DeviceTokenRevoked | Refusal::DeviceRevoked => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            Refusal::Unattributed => {
                self.record_refresh_transport(true);
                Err(transient_error(response.status))
            }
        }
    }
}

impl<S, H> SessionValidator for AuthManager<S, H>
where
    S: TokenStore,
    H: HttpClient,
{
    fn validate_restored_session<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(self.validate_session())
    }
}

impl<S, H> HttpClient for AuthManager<S, H>
where
    S: TokenStore,
    H: HttpClient,
{
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>,
    > {
        Box::pin(self.send_authenticated(request))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("authentication required")]
    NeedsReauth,
    #[error("device revoked")]
    DeviceRevoked,
    #[error("request rate limited")]
    RateLimited,
    #[error("authentication transport unavailable")]
    Transport,
    #[error("authentication response invalid")]
    InvalidResponse,
    #[error("account deleted")]
    AccountDeleted,
    #[error(transparent)]
    TokenStore(#[from] TokenStoreError),
    #[error(transparent)]
    Transition(#[from] AuthTransitionError),
}

/// What a refused request says about the session, read only from the error
/// code velvt-core puts on it.
///
/// A status alone says nothing about the session. A 403 with no code is what
/// Cloudflare's WAF, Bot Fight Mode or a challenge answers with, and a 5xx or a
/// 429 is an outage or a limit; none of them is the cloud refusing this
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// `403 device_revoked`, or `403 device_inactive` from a reissue: this
    /// device's row was revoked.
    DeviceRevoked,
    /// `401 account_deleted`: the account is gone.
    AccountDeleted,
    /// `401|403 device_token_revoked`: a reissue under the user session
    /// can recover.
    DeviceTokenRevoked,
    /// The cloud rejected the token itself.
    SessionRejected,
    /// No session error code: an edge, an outage or a rate limit, or a
    /// response that is not a refusal at all.
    Unattributed,
}

fn refusal(response: &HttpResponse) -> Refusal {
    match (response.status, response.error_code.as_deref()) {
        (403, Some("device_revoked" | "device_inactive")) => Refusal::DeviceRevoked,
        (401, Some("account_deleted")) => Refusal::AccountDeleted,
        (401 | 403, Some("device_token_revoked")) => Refusal::DeviceTokenRevoked,
        (
            401,
            Some("invalid_credentials" | "invalid_token" | "token_expired" | "missing_token"),
        ) => Refusal::SessionRejected,
        _ => Refusal::Unattributed,
    }
}

/// The error a request that got no verdict on the session fails with.
fn transient_error(status: u16) -> AuthError {
    if status == 429 {
        AuthError::RateLimited
    } else {
        AuthError::Transport
    }
}
