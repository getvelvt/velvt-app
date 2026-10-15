//! User-account relay for the protocol-v6 auth messages (`sign_up`, `log_in`,
//! `log_out`, `delete_account`).
//!
//! This is deliberately a thin, stateless HTTP relay: Swift owns its own
//! session tokens in platform storage (Keychain on macOS; see
//! `swift-client/Sources/VelvtMac/Auth`). Rust never persists the resulting
//! `access_token` / `refresh_token` beyond process memory.
//!
//! The one exception is device registration: `/v1/devices` requires a
//! user's access token and has no anonymous mode, so the first successful
//! sign-up or login also registers this device (if one isn't already
//! persisted), using that access token only for the lifetime of the single
//! `/v1/devices` call. See [`AccountAuthService::ensure_device_registered`].

use super::{
    AuthState, AuthStateMachine, HttpClient, HttpRequest, RedactedString, TokenPair, TokenStore,
    TokenStoreError,
};
use std::sync::Arc;
use velvt_shared_types::{
    AccountDeletionAccepted, AuthFailure, AuthFailureCode, AuthSuccess, ErrorResponse,
    ServerMessage,
};

pub struct AccountAuthService {
    /// Unauthenticated transport for credential-based signup/login and
    /// device registration (which authenticates via the just-issued user
    /// access token, not a device-bound one).
    raw_http: Arc<dyn HttpClient>,
    /// Device-authenticated transport for logout/account deletion.
    authenticated_http: Arc<dyn HttpClient>,
    token_store: Arc<dyn TokenStore>,
    auth_state: Arc<AuthStateMachine>,
    /// The revocation `log_out` sent in the background, until a sign-up or
    /// login has waited for it.
    pending_logout: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl AccountAuthService {
    pub fn new(
        raw_http: Arc<dyn HttpClient>,
        authenticated_http: Arc<dyn HttpClient>,
        token_store: Arc<dyn TokenStore>,
        auth_state: Arc<AuthStateMachine>,
    ) -> Self {
        Self {
            raw_http,
            authenticated_http,
            token_store,
            auth_state,
            pending_logout: std::sync::Mutex::new(None),
        }
    }

    pub async fn sign_up(&self, email: String, password: String) -> ServerMessage {
        self.finish_pending_logout().await;
        self.leave_ended_session();
        let message = self
            .credential_flow("/v1/auth/signup", 201, email, password)
            .await;
        self.ensure_device_registered(message, CredentialFlow::SignUp)
            .await
    }

    pub async fn log_in(&self, email: String, password: String) -> ServerMessage {
        self.finish_pending_logout().await;
        self.leave_ended_session();
        let message = self
            .credential_flow("/v1/auth/login", 200, email, password)
            .await;
        self.ensure_device_registered(message, CredentialFlow::LogIn)
            .await
    }

    /// Clears a session the cloud ended for good before a new sign-in starts.
    ///
    /// `DeviceRevoked` and `AccountDeleted` are terminal: no transition leads
    /// from either to `Authenticated`, so a sign-in that registered a new
    /// device from one was refused by the state machine and the service kept
    /// answering `device_revoked` until it was relaunched. The revoked device
    /// id goes too: reissuing it can only be refused again.
    fn leave_ended_session(&self) {
        if matches!(
            self.auth_state.current(),
            AuthState::DeviceRevoked | AuthState::AccountDeleted
        ) {
            self.clear_local_session(true);
        }
    }

    /// The account an event recorded now belongs to, when the service knows
    /// it: the one the current session was issued for.
    pub fn current_owner(&self) -> Option<String> {
        self.token_store.load_user_id().ok().flatten()
    }

    pub fn apply_session(&self, session: velvt_shared_types::AuthSession) {
        let user_tokens_stored = match (
            session.user_access_token,
            session.user_refresh_token,
            session.user_expires_at,
        ) {
            (Some(access), Some(refresh), Some(expires_at)) => self
                .token_store
                .store_user_tokens(
                    RedactedString::new(access),
                    RedactedString::new(refresh),
                    expires_at,
                )
                .is_ok(),
            (None, None, None) => true,
            _ => false,
        };
        // A client that predates protocol 34 names no account. The owner is
        // then unknown rather than left over from an earlier session, and
        // `validate_session` fills it in from the cloud's answer.
        let session_stored = self
            .store_session(
                TokenPair::new(
                    RedactedString::new(session.access_token),
                    RedactedString::new(session.refresh_token),
                    session.expires_at,
                ),
                session.user_id.as_deref(),
            )
            .is_ok();
        if session_stored
            && self.token_store.store_device_id(&session.device_id).is_ok()
            && user_tokens_stored
        {
            let _ = self.auth_state.transition(AuthState::Authenticated {
                device_id: session.device_id,
            });
        }
    }

    /// Stores a session's device tokens and the account they were issued
    /// for, in the order `AuthManager` relies on to bind each request to its
    /// account: the old account id is cleared before the tokens change, and
    /// the new one is stored after them, so no reader sees one session's
    /// tokens beside another session's account. With no account (or an
    /// empty one), the owner is left unknown.
    ///
    /// The same account's session handed over again (the app does so on
    /// every connection) keeps its id throughout: clearing it would fail a
    /// request of that account's that read it in between.
    fn store_session(
        &self,
        tokens: TokenPair,
        user_id: Option<&str>,
    ) -> Result<(), TokenStoreError> {
        let user_id = user_id.filter(|user_id| !user_id.is_empty());
        let same_account =
            user_id.is_some() && self.token_store.load_user_id()?.as_deref() == user_id;
        if !same_account {
            self.token_store.clear_user_id()?;
        }
        self.token_store.store_pair(tokens)?;
        match user_id {
            Some(user_id) => self.token_store.store_user_id(user_id),
            None => Ok(()),
        }
    }

    /// Registers this device with the cloud the first time a user
    /// authenticates. `/v1/devices` requires the user's access token (it has
    /// no anonymous/device-only mode), so this can only run after a
    /// successful sign-up or login — never speculatively at service startup.
    /// A device_id already on disk means a prior login already registered
    /// it, so this is a no-op on every subsequent app launch.
    async fn ensure_device_registered(
        &self,
        outcome: ServerMessage,
        flow: CredentialFlow,
    ) -> ServerMessage {
        let ServerMessage::AuthSuccess(success) = outcome else {
            return outcome;
        };
        match self.token_store.load_device_id() {
            Ok(Some(device_id)) => {
                // Stored only once the reissue (or the registration it falls
                // through to) succeeds: a sign-in that fails keeps none of
                // the new account's session, which left beside another
                // session's device tokens would be handed to Swift as theirs.
                let user_tokens = TokenPair::new(
                    RedactedString::new(success.access_token.clone()),
                    RedactedString::new(success.refresh_token.clone()),
                    success.expires_at,
                );
                return self
                    .reissue_device_tokens(success.user_id, user_tokens, device_id, flow)
                    .await;
            }
            Err(error) => {
                tracing::error!(
                    error_code = "device_id_load_failed",
                    error = %error,
                    "failed to read stored device identifier; this Mac was not registered"
                );
                return registration_failure(flow, AuthFailureCode::ServerError);
            }
            Ok(None) => {}
        }

        self.register_device(success, flow).await
    }

    /// Registers this Mac and, only once a device exists to upload as,
    /// answers `auth_success`.
    ///
    /// A registration that fails answers `auth_failure`. It used to answer
    /// `auth_success` with an empty device id and no device tokens: the app
    /// showed the person signed in, nothing they did was ever queued for
    /// upload, and the next restart handed that empty session back and
    /// signed them out.
    async fn register_device(&self, success: AuthSuccess, flow: CredentialFlow) -> ServerMessage {
        let mut request = HttpRequest::post("/v1/devices");
        request.authorization = Some(RedactedString::new(success.access_token.clone()));
        request.json_body =
            Some(serde_json::json!({ "client_version": crate::build_info::SERVICE_VERSION }));
        let response = match self.raw_http.send(request).await {
            Ok(response) if (200..300).contains(&response.status) => response,
            Ok(response) => {
                tracing::error!(
                    error_code = "device_registration_rejected",
                    status = response.status,
                    "device registration was rejected by the server"
                );
                return registration_failure(flow, AuthFailureCode::ServerError);
            }
            Err(error) => {
                tracing::error!(
                    error_code = "device_registration_failed",
                    error = %error,
                    "device registration request failed"
                );
                return registration_failure(flow, AuthFailureCode::NetworkError);
            }
        };
        let (Some(device_id), Some(tokens)) = (response.device_id, response.tokens) else {
            tracing::error!(
                error_code = "device_registration_invalid_response",
                "device registration response was missing tokens or a device id"
            );
            return registration_failure(flow, AuthFailureCode::ServerError);
        };
        if device_id.is_empty() {
            tracing::error!(
                error_code = "device_registration_invalid_response",
                "device registration response carried an empty device id"
            );
            return registration_failure(flow, AuthFailureCode::ServerError);
        }
        if let Err(error) = self
            .store_session(tokens.clone(), Some(&success.user_id))
            .and_then(|()| self.token_store.store_device_id(&device_id))
        {
            tracing::error!(
                error_code = "device_registration_persist_failed",
                error = %error,
                "failed to keep the session after registration"
            );
            self.clear_local_session(false);
            return registration_failure(flow, AuthFailureCode::ServerError);
        }
        if let Err(error) = self.auth_state.transition(AuthState::Authenticated {
            device_id: device_id.clone(),
        }) {
            tracing::error!(
                error_code = "device_registration_state_transition_failed",
                error = %error,
                "device registered but auth state transition was rejected"
            );
        }
        let user_tokens = TokenPair::new(
            RedactedString::new(success.access_token),
            RedactedString::new(success.refresh_token),
            success.expires_at,
        );
        let _ = self.token_store.store_user_pair(user_tokens.clone());
        auth_success_from_device_tokens(success.user_id, device_id, tokens, Some(user_tokens))
    }

    async fn reissue_device_tokens(
        &self,
        user_id: String,
        user_tokens: TokenPair,
        device_id: String,
        flow: CredentialFlow,
    ) -> ServerMessage {
        let mut request = HttpRequest::post("/v1/auth/devices/reissue");
        request.authorization = Some(user_tokens.access_token().clone());
        request.json_body = Some(serde_json::json!({ "device_id": device_id }));
        let response = match self.raw_http.send(request).await {
            Ok(response) if response.status == 200 => response,
            // The stored device is not this account's (404), or it was
            // revoked (403 `device_inactive` from reissue, `device_revoked`
            // elsewhere): either way it can never be reissued, and this Mac
            // registers as a new device. Only 404 used to, so a Mac removed
            // from the account could not sign in again until relaunched.
            Ok(response)
                if response.status == 404
                    || (response.status == 403
                        && matches!(
                            response.error_code.as_deref(),
                            Some("device_inactive" | "device_revoked")
                        )) =>
            {
                tracing::warn!(
                    error_code = "stored_device_id_unusable",
                    status = response.status,
                    "stored device id cannot be reissued; registering a new device"
                );
                if let Err(error) = self.token_store.clear_device_id() {
                    tracing::error!(
                        error_code = "device_id_clear_failed",
                        error = %error,
                        "failed to clear stale device id before registration retry"
                    );
                }
                return self
                    .register_device(
                        AuthSuccess {
                            user_id,
                            device_id: String::new(),
                            access_token: user_tokens.access_token().expose().to_owned(),
                            refresh_token: user_tokens.refresh_token().expose().to_owned(),
                            expires_at: user_tokens.expires_at(),
                            user_access_token: None,
                            user_refresh_token: None,
                            user_expires_at: None,
                        },
                        flow,
                    )
                    .await;
            }
            Ok(response) => {
                tracing::error!(
                    error_code = "device_token_reissue_rejected",
                    status = response.status,
                    "device token reissue was rejected by the server"
                );
                return ServerMessage::AuthFailure(AuthFailure {
                    code: AuthFailureCode::ServerError,
                    message: "Unable to register this device.".into(),
                });
            }
            Err(error) => {
                tracing::error!(
                    error_code = "device_token_reissue_failed",
                    error = %error,
                    "device token reissue request failed"
                );
                return ServerMessage::AuthFailure(AuthFailure {
                    code: AuthFailureCode::NetworkError,
                    message: "Could not reach the server. Check your connection and try again."
                        .into(),
                });
            }
        };
        let Some(tokens) = response.tokens else {
            tracing::error!(
                error_code = "device_token_reissue_invalid_response",
                "device token reissue response was missing tokens"
            );
            return ServerMessage::AuthFailure(AuthFailure {
                code: AuthFailureCode::ServerError,
                message: "The server response was invalid.".into(),
            });
        };
        if let Err(error) = self.store_session(tokens.clone(), Some(&user_id)) {
            tracing::error!(
                error_code = "device_token_reissue_persist_failed",
                error = %error,
                "failed to persist reissued device tokens"
            );
            self.clear_local_session(false);
            return ServerMessage::AuthFailure(AuthFailure {
                code: AuthFailureCode::ServerError,
                message: "Unable to save this device session.".into(),
            });
        }
        let _ = self.token_store.store_user_pair(user_tokens.clone());
        if let Err(error) = self.auth_state.transition(AuthState::Authenticated {
            device_id: device_id.clone(),
        }) {
            tracing::error!(
                error_code = "device_token_reissue_state_transition_failed",
                error = %error,
                "device tokens were reissued but auth state transition was rejected"
            );
        }
        auth_success_from_device_tokens(user_id, device_id, tokens, Some(user_tokens))
    }

    /// Fire-and-forget per the IPC contract: best-effort server-side
    /// revocation. Failures are not surfaced — the client has already
    /// cleared its local session by the time this is called.
    ///
    /// The local session is cleared before this returns, and the revocation
    /// is sent in a task of its own: the router calls this on the connection
    /// that carries every raw event, which must not wait on the cloud.
    pub fn log_out(&self) {
        // Snapshot the credential first, then fail closed immediately for
        // local collection. The authenticated transport deliberately rejects
        // requests once state is unauthenticated, so issue this one explicit
        // best-effort revocation through the raw transport instead.
        let device_access_token = self
            .token_store
            .load_tokens()
            .ok()
            .flatten()
            .map(|tokens| tokens.access_token().clone());
        let _ = self.auth_state.transition(AuthState::Unauthenticated);
        self.clear_local_session(false);
        if let Some(access_token) = device_access_token {
            let mut request = HttpRequest::post("/v1/auth/logout");
            request.authorization = Some(access_token);
            let raw_http = Arc::clone(&self.raw_http);
            let revocation = tokio::spawn(async move {
                let _ = raw_http.send(request).await;
            });
            if let Ok(mut pending) = self.pending_logout.lock() {
                *pending = Some(revocation);
            }
        }
    }

    /// Waits for a revocation `log_out` sent in the background.
    ///
    /// The cloud revokes every unrevoked refresh token of the device it is
    /// sent for, and this Mac keeps its device id across a log out. A
    /// revocation that landed after the next login reissued that device's
    /// tokens would revoke the new session, so a sign-up or login waits for
    /// it first.
    async fn finish_pending_logout(&self) {
        let pending = self
            .pending_logout
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        if let Some(revocation) = pending {
            let _ = revocation.await;
        }
    }

    /// Relays the deletion to the cloud and, on acceptance, clears this
    /// device's session.
    ///
    /// Cloud-side only. `clear_local_session(true)` clears the device and user
    /// tokens and the stored device id; nothing here touches
    /// `~/.velvt/velvt-service.sqlite3`, and this service holds no persistence
    /// handle through which it could. The local half belongs to the router's
    /// `ClientMessage::DeleteAccount` arm, which destroys the upload queue once
    /// this returns `AccountDeletionAccepted`.
    ///
    /// Clearing the device id is what makes that half necessary.
    /// `ensure_device_registered` reuses a device id it finds on disk, so
    /// clearing it means the next sign-up on this Mac registers a new device —
    /// and the cloud scopes duplicate detection by device id, so a batch queued
    /// under the deleted account arrives as new work and is stored against the
    /// account that replaces it.
    pub async fn delete_account(&self) -> ServerMessage {
        match self
            .authenticated_http
            .send(HttpRequest::delete("/v1/account"))
            .await
        {
            Ok(response) if response.status == 202 => {
                self.clear_local_session(true);
                ServerMessage::AccountDeletionAccepted(AccountDeletionAccepted {})
            }
            _ => ServerMessage::ErrorResponse(ErrorResponse {
                code: "account_deletion_failed".into(),
                message: "Unable to delete account".into(),
                related_event_id: None,
            }),
        }
    }

    /// Clears the session of an account the cloud reported deleted (`401
    /// account_deleted`), with its device id: the device row went with the
    /// account. Leaves the service signed out.
    pub fn forget_deleted_account(&self) {
        self.clear_local_session(true);
    }

    fn clear_local_session(&self, clear_device_id: bool) {
        if let Err(error) = self.token_store.clear_tokens() {
            tracing::error!(
                error_code = "device_tokens_clear_failed",
                error = %error,
                "failed to clear local device tokens"
            );
        }
        if let Err(error) = self.token_store.clear_user_tokens() {
            tracing::error!(
                error_code = "user_tokens_clear_failed",
                error = %error,
                "failed to clear local user tokens"
            );
        }
        if let Err(error) = self.token_store.clear_user_id() {
            tracing::error!(
                error_code = "user_id_clear_failed",
                error = %error,
                "failed to clear the signed-in account id"
            );
        }
        if clear_device_id {
            if let Err(error) = self.token_store.clear_device_id() {
                tracing::error!(
                    error_code = "device_id_clear_failed",
                    error = %error,
                    "failed to clear deleted account device identifier"
                );
            }
        }
        let _ = self.auth_state.transition(AuthState::Unauthenticated);
    }

    async fn credential_flow(
        &self,
        path: &str,
        success_status: u16,
        email: String,
        password: String,
    ) -> ServerMessage {
        let mut request = HttpRequest::post(path);
        request.json_body = Some(serde_json::json!({ "email": email, "password": password }));
        match self.raw_http.send(request).await {
            Ok(response) if response.status == success_status => {
                match (response.user_id, response.tokens) {
                    (Some(user_id), Some(tokens)) => ServerMessage::AuthSuccess(AuthSuccess {
                        user_id,
                        device_id: String::new(),
                        access_token: tokens.access_token().expose().to_owned(),
                        refresh_token: tokens.refresh_token().expose().to_owned(),
                        expires_at: tokens.expires_at(),
                        user_access_token: None,
                        user_refresh_token: None,
                        user_expires_at: None,
                    }),
                    _ => ServerMessage::AuthFailure(AuthFailure {
                        code: AuthFailureCode::ServerError,
                        message: "The server response was invalid.".into(),
                    }),
                }
            }
            Ok(response) => ServerMessage::AuthFailure(map_auth_failure(&response)),
            Err(_) => ServerMessage::AuthFailure(AuthFailure {
                code: AuthFailureCode::NetworkError,
                message: "Could not reach the server. Check your connection and try again.".into(),
            }),
        }
    }
}

/// Which credential request a device registration finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialFlow {
    SignUp,
    LogIn,
}

/// The answer to a sign-up or login whose credentials the cloud accepted but
/// whose device registration failed. Nothing is kept, so the person stays
/// signed out and nothing is queued under a session with no device.
///
/// A sign-up has created the account by then, and signing up again would be
/// refused as an email already registered, so it says to log in.
fn registration_failure(flow: CredentialFlow, code: AuthFailureCode) -> ServerMessage {
    let message = match (flow, code) {
        (CredentialFlow::SignUp, AuthFailureCode::NetworkError) => {
            "Your account was created, but this Mac could not reach the server to register. \
             Check your connection, then log in."
        }
        (CredentialFlow::SignUp, _) => {
            "Your account was created, but this Mac could not be registered. Log in to try again."
        }
        (CredentialFlow::LogIn, AuthFailureCode::NetworkError) => {
            "Could not reach the server. Check your connection and try again."
        }
        (CredentialFlow::LogIn, _) => "This Mac could not be registered. Please try again.",
    };
    ServerMessage::AuthFailure(AuthFailure {
        code,
        message: message.into(),
    })
}

/// Prefers the cloud API's own `error.message` (e.g. "Email is already
/// registered.", "Request validation failed.") over a generic fallback —
/// the server already writes a message safe to show verbatim, and the
/// fallback before this used to mask everything but bad credentials as
/// "Something went wrong", which made a 409 email-conflict indistinguishable
/// from any other failure.
fn map_auth_failure(response: &super::HttpResponse) -> AuthFailure {
    if response.status == 429 {
        return AuthFailure {
            code: AuthFailureCode::RateLimited,
            message: rate_limited_message(response.retry_after.as_deref()),
        };
    }
    if let Some(message) = credential_validation_message(response) {
        return AuthFailure {
            code: AuthFailureCode::InvalidCredentials,
            message: message.into(),
        };
    }
    match (response.status, response.error_code.as_deref()) {
        (401 | 403, _) | (_, Some("invalid_credentials")) => AuthFailure {
            code: AuthFailureCode::InvalidCredentials,
            message: response
                .message
                .clone()
                .unwrap_or_else(|| "Incorrect email or password.".into()),
        },
        _ => AuthFailure {
            code: AuthFailureCode::ServerError,
            message: response
                .message
                .clone()
                .unwrap_or_else(|| "Something went wrong. Please try again.".into()),
        },
    }
}

/// The words for a sign-up or login the cloud refused for too many attempts
/// (backend-e2e-07, helper-backend-12), with the wait taken from
/// `Retry-After`.
///
/// velvt-core sends `Retry-After` in whole seconds on every rate-limit 429
/// from #41 on, and words of its own that say the same wait; a server from
/// before sends neither, only "Too many requests.". The wait is read from the
/// header, so the app says the same thing to either server, and rounded up to
/// the minute, so the person never comes back early.
fn rate_limited_message(retry_after: Option<&str>) -> String {
    let Some(wait) = retry_after.and_then(super::retry_after_delay) else {
        return "Too many attempts. Try again later.".into();
    };
    let minutes = wait.as_secs().div_ceil(60).max(1);
    let wait = match minutes {
        1 => "a minute".to_owned(),
        2..=119 => format!("{minutes} minutes"),
        _ => format!("{} hours", minutes.div_ceil(60)),
    };
    format!("Too many attempts. Try again in {wait}.")
}

/// Shown when velvt-core refuses the password's length. Its sign-up rule is
/// 12 to 256 characters (`SignupRequest` in `app/schemas/auth.py`); the forms
/// say the same before anything is sent.
const PASSWORD_TOO_SHORT_MESSAGE: &str = "Use at least 12 characters for your password.";
const PASSWORD_TOO_LONG_MESSAGE: &str = "Use at most 256 characters for your password.";
const PASSWORD_INVALID_MESSAGE: &str = "Use a password of 12 to 256 characters.";
const EMAIL_INVALID_MESSAGE: &str = "Enter a valid email address.";

/// Readable text for a 422 `validation_error` on the email or the password.
///
/// velvt-core answers every request it cannot validate with "Request
/// validation failed.", the same words for an 11-character password as for a
/// malformed address, and the app showed them as they came (1.0.14
/// simulation, helper-backend-08). The field is in `details[].loc` and the
/// rule in `details[].type`. Nothing else in a detail is read: a detail can
/// also echo the submitted value in `input`. The email is named first, since
/// it is the first field on both forms. `None` for any other validation
/// error, which keeps the server's own message.
fn credential_validation_message(response: &super::HttpResponse) -> Option<&'static str> {
    if response.status != 422 || response.error_code.as_deref() != Some("validation_error") {
        return None;
    }
    let details = response
        .raw_body
        .as_ref()?
        .pointer("/error/details")?
        .as_array()?;
    let mut password = None;
    for detail in details {
        let field = detail
            .get("loc")
            .and_then(serde_json::Value::as_array)
            .and_then(|loc| loc.last())
            .and_then(serde_json::Value::as_str);
        let kind = detail.get("type").and_then(serde_json::Value::as_str);
        match field {
            Some("email") => return Some(EMAIL_INVALID_MESSAGE),
            Some("password") if password.is_none() => {
                password = Some(match kind {
                    Some("string_too_short") => PASSWORD_TOO_SHORT_MESSAGE,
                    Some("string_too_long") => PASSWORD_TOO_LONG_MESSAGE,
                    _ => PASSWORD_INVALID_MESSAGE,
                });
            }
            _ => {}
        }
    }
    password
}

fn auth_success_from_device_tokens(
    user_id: String,
    device_id: String,
    tokens: TokenPair,
    user_tokens: Option<TokenPair>,
) -> ServerMessage {
    ServerMessage::AuthSuccess(AuthSuccess {
        user_id,
        device_id,
        access_token: tokens.access_token().expose().to_owned(),
        refresh_token: tokens.refresh_token().expose().to_owned(),
        expires_at: tokens.expires_at(),
        user_access_token: user_tokens
            .as_ref()
            .map(|tokens| tokens.access_token().expose().to_owned()),
        user_refresh_token: user_tokens
            .as_ref()
            .map(|tokens| tokens.refresh_token().expose().to_owned()),
        user_expires_at: user_tokens.as_ref().map(TokenPair::expires_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{FakeTokenStore, HttpResponse, TokenPair};
    use chrono::{Duration, Utc};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttpClient {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<VecDeque<HttpResponse>>,
    }

    impl FakeHttpClient {
        fn with_responses(responses: Vec<HttpResponse>) -> Self {
            Self {
                requests: Mutex::default(),
                responses: Mutex::new(responses.into()),
            }
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HttpClient for FakeHttpClient {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, crate::auth::AuthError>> + Send + 'a>>
        {
            self.requests.lock().unwrap().push(request);
            let response = self.responses.lock().unwrap().pop_front();
            Box::pin(async move { response.ok_or(crate::auth::AuthError::Transport) })
        }
    }

    fn signup_response() -> HttpResponse {
        HttpResponse {
            status: 201,
            error_code: None,
            tokens: Some(TokenPair::new(
                RedactedString::new("user-access"),
                RedactedString::new("user-refresh"),
                Utc::now() + Duration::hours(1),
            )),
            retry_after: None,
            message: None,
            raw_body: None,
            user_id: Some("user-1".into()),
            device_id: None,
        }
    }

    fn device_response() -> HttpResponse {
        HttpResponse {
            status: 201,
            error_code: None,
            tokens: Some(TokenPair::new(
                RedactedString::new("device-access"),
                RedactedString::new("device-refresh"),
                Utc::now() + Duration::hours(1),
            )),
            retry_after: None,
            message: None,
            raw_body: None,
            user_id: None,
            device_id: Some("device-1".into()),
        }
    }

    fn reissued_device_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            tokens: Some(TokenPair::new(
                RedactedString::new("reissued-device-access"),
                RedactedString::new("reissued-device-refresh"),
                Utc::now() + Duration::hours(1),
            )),
            device_id: Some("existing-device".into()),
            ..device_response()
        }
    }

    fn service(raw_http: Arc<FakeHttpClient>) -> (AccountAuthService, Arc<FakeTokenStore>) {
        let token_store = Arc::new(FakeTokenStore::default());
        let auth_state = Arc::new(AuthStateMachine::new(AuthState::Unauthenticated));
        let service = AccountAuthService::new(
            raw_http,
            Arc::new(FakeHttpClient::default()),
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            auth_state,
        );
        (service, token_store)
    }

    #[tokio::test]
    async fn successful_sign_up_registers_device_using_the_new_access_token() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            signup_response(),
            device_response(),
        ]));
        let (service, token_store) = service(Arc::clone(&raw_http));

        let outcome = service
            .sign_up("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthSuccess(_)));
        let requests = raw_http.requests();
        assert_eq!(requests[1].path, "/v1/devices");
        assert_eq!(
            requests[1]
                .authorization
                .as_ref()
                .map(|t| t.expose().to_owned()),
            Some("user-access".to_owned())
        );
        assert_eq!(
            requests[1].json_body,
            Some(serde_json::json!({
                "client_version": crate::build_info::SERVICE_VERSION
            })),
            "the device registers under the release version, not Cargo's"
        );
        assert_eq!(
            token_store.load_device_id().unwrap(),
            Some("device-1".into())
        );
    }

    #[tokio::test]
    async fn login_reissues_device_tokens_when_device_id_already_stored() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            HttpResponse {
                status: 200,
                ..signup_response()
            },
            reissued_device_response(),
        ]));
        let (service, token_store) = service(Arc::clone(&raw_http));
        token_store.store_device_id("existing-device").unwrap();
        token_store
            .store_pair(TokenPair::new(
                RedactedString::new("device-access"),
                RedactedString::new("device-refresh"),
                Utc::now() + Duration::hours(1),
            ))
            .unwrap();

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthSuccess(_)));
        let requests = raw_http.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
        assert_eq!(
            requests[1]
                .authorization
                .as_ref()
                .map(|token| token.expose()),
            Some("user-access")
        );
        let stored = token_store.load_tokens().unwrap().unwrap();
        assert_eq!(stored.access_token().expose(), "reissued-device-access");
        assert_eq!(stored.refresh_token().expose(), "reissued-device-refresh");
    }

    #[tokio::test]
    async fn login_registers_new_device_when_stored_device_id_is_stale() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            HttpResponse {
                status: 200,
                ..signup_response()
            },
            HttpResponse {
                status: 404,
                tokens: None,
                device_id: None,
                ..device_response()
            },
            device_response(),
        ]));
        let (service, token_store) = service(Arc::clone(&raw_http));
        token_store.store_device_id("missing-device").unwrap();

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthSuccess(_)));
        let requests = raw_http.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].path, "/v1/auth/devices/reissue");
        assert_eq!(requests[2].path, "/v1/devices");
        assert_eq!(
            token_store.load_device_id().unwrap(),
            Some("device-1".into())
        );
        let stored = token_store.load_tokens().unwrap().unwrap();
        assert_eq!(stored.access_token().expose(), "device-access");
    }

    /// cross-device-13: reissue answers a revoked device with `403
    /// device_inactive` (`device_revoked` elsewhere). Only 404 fell through to
    /// registering, so a Mac removed from the account answered every sign-in
    /// with "Unable to register this device." until it was relaunched.
    #[tokio::test]
    async fn a_device_refused_at_reissue_as_revoked_registers_again() {
        for code in ["device_inactive", "device_revoked"] {
            let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
                HttpResponse {
                    status: 200,
                    ..signup_response()
                },
                HttpResponse {
                    status: 403,
                    error_code: Some(code.into()),
                    tokens: None,
                    device_id: None,
                    ..device_response()
                },
                device_response(),
            ]));
            let (service, token_store) = service(Arc::clone(&raw_http));
            token_store.store_device_id("removed-device").unwrap();

            let outcome = service
                .log_in("a@example.test".into(), "password".into())
                .await;

            assert!(
                matches!(outcome, ServerMessage::AuthSuccess(ref success) if success.device_id == "device-1"),
                "{code}: a removed Mac must be able to sign in again: {outcome:?}"
            );
            assert_eq!(raw_http.requests()[1].path, "/v1/auth/devices/reissue");
            assert_eq!(raw_http.requests()[2].path, "/v1/devices");
            assert_eq!(
                token_store.load_device_id().unwrap(),
                Some("device-1".into())
            );
        }
    }

    /// After the service has seen the revocation, `DeviceRevoked` is
    /// terminal: no transition leads from it to `Authenticated`, so the new
    /// device's session was refused by the state machine even when the
    /// registration succeeded. A sign-in leaves it first, and drops the
    /// revoked device id rather than asking to reissue it.
    #[tokio::test]
    async fn a_sign_in_after_revocation_registers_and_is_authenticated() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            HttpResponse {
                status: 200,
                ..signup_response()
            },
            device_response(),
        ]));
        let token_store = Arc::new(FakeTokenStore::default());
        token_store.store_device_id("revoked-device").unwrap();
        let auth_state = Arc::new(AuthStateMachine::new(AuthState::DeviceRevoked));
        let service = AccountAuthService::new(
            Arc::clone(&raw_http) as Arc<dyn HttpClient>,
            Arc::new(FakeHttpClient::default()),
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            Arc::clone(&auth_state),
        );

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(
            matches!(outcome, ServerMessage::AuthSuccess(_)),
            "{outcome:?}"
        );
        let paths = raw_http
            .requests()
            .into_iter()
            .map(|request| request.path)
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/v1/auth/login", "/v1/devices"]);
        assert_eq!(
            auth_state.current(),
            AuthState::Authenticated {
                device_id: "device-1".into()
            }
        );
    }

    /// helper-backend-06: a registration that fails answers `auth_failure`
    /// and keeps nothing. It answered `auth_success` with an empty device id.
    #[tokio::test]
    async fn a_failed_registration_answers_auth_failure_and_keeps_no_session() {
        for (flow, registration, expected_code, expected_message) in [
            (
                "sign_up",
                Some(HttpResponse {
                    status: 503,
                    tokens: None,
                    device_id: None,
                    ..device_response()
                }),
                AuthFailureCode::ServerError,
                "Your account was created, but this Mac could not be registered. Log in to try again.",
            ),
            (
                "sign_up",
                None,
                AuthFailureCode::NetworkError,
                "Your account was created, but this Mac could not reach the server to register. Check your connection, then log in.",
            ),
            (
                "log_in",
                Some(HttpResponse {
                    status: 201,
                    device_id: None,
                    ..device_response()
                }),
                AuthFailureCode::ServerError,
                "This Mac could not be registered. Please try again.",
            ),
        ] {
            let mut responses = vec![HttpResponse {
                status: if flow == "sign_up" { 201 } else { 200 },
                ..signup_response()
            }];
            responses.extend(registration);
            let raw_http = Arc::new(FakeHttpClient::with_responses(responses));
            let (service, token_store) = service(Arc::clone(&raw_http));

            let outcome = if flow == "sign_up" {
                service.sign_up("a@example.test".into(), "password".into()).await
            } else {
                service.log_in("a@example.test".into(), "password".into()).await
            };

            let ServerMessage::AuthFailure(failure) = outcome else {
                panic!("{flow}: a sign-in with no device must not succeed: {outcome:?}");
            };
            assert_eq!(failure.code, expected_code, "{flow}");
            assert_eq!(failure.message, expected_message, "{flow}");
            assert!(token_store.load_tokens().unwrap().is_none(), "{flow}");
            assert!(token_store.load_user_tokens().unwrap().is_none(), "{flow}");
            assert_eq!(token_store.load_user_id().unwrap(), None, "{flow}");
            assert_eq!(token_store.load_device_id().unwrap(), None, "{flow}");
        }
    }

    /// The same on a Mac that already has a device id from an earlier
    /// session, whose sign-in reissues it: a reissue refused or unreachable,
    /// or one that falls through to a registration that fails, keeps none of
    /// the new account's session. The user tokens used to be stored before
    /// the reissue and left behind, where a later sign-in could hand them to
    /// the app beside another session's device tokens.
    #[tokio::test]
    async fn a_failed_sign_in_on_a_mac_with_a_stored_device_keeps_no_session() {
        let not_this_accounts = HttpResponse {
            status: 404,
            error_code: Some("device_not_found".into()),
            tokens: None,
            device_id: None,
            ..device_response()
        };
        let unavailable = HttpResponse {
            status: 503,
            tokens: None,
            device_id: None,
            ..device_response()
        };
        for (case, after_login) in [
            (
                "registration after a reissue for another account's device",
                vec![not_this_accounts, unavailable.clone()],
            ),
            ("reissue refused", vec![unavailable]),
            ("reissue unreachable", vec![]),
        ] {
            let mut responses = vec![HttpResponse {
                status: 200,
                ..signup_response()
            }];
            responses.extend(after_login);
            let (service, token_store) =
                service(Arc::new(FakeHttpClient::with_responses(responses)));
            token_store.store_device_id("earlier-device").unwrap();

            let outcome = service
                .log_in("a@example.test".into(), "password".into())
                .await;

            assert!(
                matches!(outcome, ServerMessage::AuthFailure(_)),
                "{case}: {outcome:?}"
            );
            assert!(token_store.load_tokens().unwrap().is_none(), "{case}");
            assert!(token_store.load_user_tokens().unwrap().is_none(), "{case}");
            assert_eq!(token_store.load_user_id().unwrap(), None, "{case}");
        }
    }

    /// A sign-in that reissues the stored device keeps the account's user
    /// tokens with the session, as a registration does.
    #[tokio::test]
    async fn a_reissued_sign_in_keeps_the_user_session() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            HttpResponse {
                status: 200,
                ..signup_response()
            },
            reissued_device_response(),
        ]));
        let (service, token_store) = service(raw_http);
        token_store.store_device_id("existing-device").unwrap();

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthSuccess(_)));
        let user_tokens = token_store.load_user_tokens().unwrap().unwrap();
        assert_eq!(user_tokens.access_token().expose(), "user-access");
        assert_eq!(
            token_store.load_user_id().unwrap().as_deref(),
            Some("user-1")
        );
    }

    /// Records the order of the writes `AuthManager::ensure_bound_session`
    /// depends on.
    #[derive(Default)]
    struct OrderedTokenStore {
        inner: FakeTokenStore,
        writes: Mutex<Vec<&'static str>>,
    }

    impl OrderedTokenStore {
        fn record(&self, write: &'static str) {
            self.writes.lock().unwrap().push(write);
        }

        fn writes(&self) -> Vec<&'static str> {
            self.writes.lock().unwrap().clone()
        }
    }

    impl TokenStore for OrderedTokenStore {
        fn store_tokens(
            &self,
            access: RedactedString,
            refresh: RedactedString,
            expiry: chrono::DateTime<Utc>,
        ) -> Result<(), TokenStoreError> {
            self.record("store_tokens");
            self.inner.store_tokens(access, refresh, expiry)
        }
        fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
            self.inner.load_tokens()
        }
        fn clear_tokens(&self) -> Result<(), TokenStoreError> {
            self.inner.clear_tokens()
        }
        fn store_device_id(&self, device_id: &str) -> Result<(), TokenStoreError> {
            self.inner.store_device_id(device_id)
        }
        fn load_device_id(&self) -> Result<Option<String>, TokenStoreError> {
            self.inner.load_device_id()
        }
        fn clear_device_id(&self) -> Result<(), TokenStoreError> {
            self.inner.clear_device_id()
        }
        fn store_user_id(&self, user_id: &str) -> Result<(), TokenStoreError> {
            self.record("store_user_id");
            self.inner.store_user_id(user_id)
        }
        fn load_user_id(&self) -> Result<Option<String>, TokenStoreError> {
            self.inner.load_user_id()
        }
        fn clear_user_id(&self) -> Result<(), TokenStoreError> {
            self.record("clear_user_id");
            self.inner.clear_user_id()
        }
        fn store_user_tokens(
            &self,
            access: RedactedString,
            refresh: RedactedString,
            expiry: chrono::DateTime<Utc>,
        ) -> Result<(), TokenStoreError> {
            self.inner.store_user_tokens(access, refresh, expiry)
        }
        fn load_user_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
            self.inner.load_user_tokens()
        }
        fn clear_user_tokens(&self) -> Result<(), TokenStoreError> {
            self.inner.clear_user_tokens()
        }
    }

    /// Every path that puts a session in place clears the old account id
    /// before the tokens change and stores the new one after them, so a
    /// request bound to one account never reads another session's tokens
    /// beside its account id and is sent with them.
    #[tokio::test]
    async fn a_session_is_stored_between_clearing_and_naming_its_account() {
        let expected = vec!["clear_user_id", "store_tokens", "store_user_id"];
        for (case, responses, stored_device) in [
            (
                "registration",
                vec![signup_response(), device_response()],
                None,
            ),
            (
                "reissue",
                vec![
                    HttpResponse {
                        status: 200,
                        ..signup_response()
                    },
                    reissued_device_response(),
                ],
                Some("existing-device"),
            ),
        ] {
            let token_store = Arc::new(OrderedTokenStore::default());
            if let Some(device_id) = stored_device {
                token_store.store_device_id(device_id).unwrap();
            }
            let service = AccountAuthService::new(
                Arc::new(FakeHttpClient::with_responses(responses)),
                Arc::new(FakeHttpClient::default()),
                Arc::clone(&token_store) as Arc<dyn TokenStore>,
                Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
            );
            let outcome = if stored_device.is_some() {
                service
                    .log_in("a@example.test".into(), "password".into())
                    .await
            } else {
                service
                    .sign_up("a@example.test".into(), "password".into())
                    .await
            };
            assert!(matches!(outcome, ServerMessage::AuthSuccess(_)), "{case}");
            assert_eq!(token_store.writes(), expected, "{case}");
        }

        let token_store = Arc::new(OrderedTokenStore::default());
        let service = AccountAuthService::new(
            Arc::new(FakeHttpClient::default()),
            Arc::new(FakeHttpClient::default()),
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            Arc::new(AuthStateMachine::new(AuthState::Unauthenticated)),
        );
        let handover = |user_id: &str| velvt_shared_types::AuthSession {
            device_id: "device-1".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: Utc::now() + Duration::hours(1),
            user_access_token: None,
            user_refresh_token: None,
            user_expires_at: None,
            user_id: Some(user_id.into()),
        };
        service.apply_session(handover("user-7"));
        assert_eq!(token_store.writes(), expected, "handover");

        // The app hands the same session over on every connection.
        service.apply_session(handover("user-7"));
        assert_eq!(
            token_store.writes()[expected.len()..],
            ["store_tokens", "store_user_id"],
            "the same account's id is never cleared in between"
        );
        service.apply_session(handover("user-8"));
        assert_eq!(
            token_store.writes()[expected.len() + 2..],
            expected,
            "another account's session"
        );
    }

    /// The account a session belongs to is what the upload queue is stamped
    /// with: kept on a successful sign-in, cleared on log out.
    #[tokio::test]
    async fn a_sign_in_keeps_its_account_and_a_log_out_clears_it() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![
            signup_response(),
            device_response(),
        ]));
        let (service, token_store) = service(Arc::clone(&raw_http));

        service
            .sign_up("a@example.test".into(), "password".into())
            .await;
        assert_eq!(service.current_owner().as_deref(), Some("user-1"));

        service.log_out();
        assert_eq!(service.current_owner(), None);
        assert_eq!(token_store.load_user_id().unwrap(), None);
    }

    /// `auth_session` names its account (protocol 34); one from a client that
    /// predates the field leaves the owner unknown, not left over.
    #[tokio::test]
    async fn a_handed_over_session_names_the_owner_or_clears_it() {
        let (service, token_store) = service(Arc::new(FakeHttpClient::default()));
        let session = |user_id: Option<&str>| velvt_shared_types::AuthSession {
            device_id: "device-1".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: Utc::now() + Duration::hours(1),
            user_access_token: None,
            user_refresh_token: None,
            user_expires_at: None,
            user_id: user_id.map(str::to_owned),
        };

        service.apply_session(session(Some("user-7")));
        assert_eq!(
            token_store.load_user_id().unwrap().as_deref(),
            Some("user-7")
        );

        service.apply_session(session(None));
        assert_eq!(token_store.load_user_id().unwrap(), None);
    }

    #[tokio::test]
    async fn failed_login_never_attempts_device_registration() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 401,
            error_code: Some("invalid_credentials".into()),
            tokens: None,
            retry_after: None,
            message: None,
            raw_body: None,
            user_id: None,
            device_id: None,
        }]));
        let (service, token_store) = service(Arc::clone(&raw_http));

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthFailure(_)));
        assert_eq!(raw_http.requests().len(), 1);
        assert_eq!(token_store.load_device_id().unwrap(), None);
    }

    #[tokio::test]
    async fn sign_up_conflict_surfaces_the_servers_own_message_not_a_generic_one() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 409,
            error_code: Some("email_in_use".into()),
            tokens: None,
            retry_after: None,
            message: Some("Email is already registered.".into()),
            raw_body: None,
            user_id: None,
            device_id: None,
        }]));
        let (service, _) = service(Arc::clone(&raw_http));

        let outcome = service
            .sign_up("a@example.test".into(), "password".into())
            .await;

        let ServerMessage::AuthFailure(failure) = outcome else {
            panic!("expected AuthFailure, got {outcome:?}");
        };
        assert_eq!(failure.code, AuthFailureCode::ServerError);
        assert_eq!(failure.message, "Email is already registered.");
    }

    /// velvt-core's 422 for an 11-character sign-up password, as the 1.0.14
    /// simulation captured it, `input` and all. The app showed "Request
    /// validation failed." for it, coded `server_error`.
    fn validation_error(details: serde_json::Value) -> HttpResponse {
        HttpResponse {
            status: 422,
            error_code: Some("validation_error".into()),
            tokens: None,
            retry_after: None,
            message: Some("Request validation failed.".into()),
            raw_body: Some(serde_json::json!({
                "error": {
                    "code": "validation_error",
                    "message": "Request validation failed.",
                    "details": details,
                }
            })),
            user_id: None,
            device_id: None,
        }
    }

    #[tokio::test]
    async fn a_refused_email_or_password_is_named_in_words_a_person_can_act_on() {
        for (details, expected) in [
            (
                serde_json::json!([{
                    "type": "string_too_short",
                    "loc": ["body", "password"],
                    "msg": "String should have at least 12 characters",
                    "input": "short-pw-11",
                }]),
                "Use at least 12 characters for your password.",
            ),
            (
                serde_json::json!([{
                    "type": "string_too_long",
                    "loc": ["body", "password"],
                    "msg": "String should have at most 256 characters",
                }]),
                "Use at most 256 characters for your password.",
            ),
            (
                serde_json::json!([{"type": "missing", "loc": ["body", "password"]}]),
                "Use a password of 12 to 256 characters.",
            ),
            (
                serde_json::json!([{
                    "type": "string_pattern_mismatch",
                    "loc": ["body", "email"],
                    "msg": "String should match pattern",
                    "input": "not-an-address",
                }]),
                "Enter a valid email address.",
            ),
            (
                serde_json::json!([
                    {"type": "string_too_short", "loc": ["body", "password"]},
                    {"type": "string_pattern_mismatch", "loc": ["body", "email"]},
                ]),
                "Enter a valid email address.",
            ),
        ] {
            let raw_http = Arc::new(FakeHttpClient::with_responses(vec![validation_error(
                details.clone(),
            )]));
            let (service, token_store) = service(Arc::clone(&raw_http));

            let outcome = service
                .sign_up("a@example.test".into(), "short-pw-11".into())
                .await;

            let ServerMessage::AuthFailure(failure) = outcome else {
                panic!("expected AuthFailure, got {outcome:?}");
            };
            assert_eq!(failure.message, expected, "{details}");
            assert_eq!(failure.code, AuthFailureCode::InvalidCredentials);
            assert!(
                !failure.message.contains("short-pw")
                    && !failure.message.contains("not-an-address"),
                "the submitted value must never be echoed: {}",
                failure.message
            );
            assert_eq!(token_store.load_device_id().unwrap(), None);
        }
    }

    /// A validation error on any other field keeps the server's own words.
    #[tokio::test]
    async fn a_validation_error_on_another_field_keeps_the_servers_message() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![validation_error(
            serde_json::json!([{"type": "missing", "loc": ["body", "device_name"]}]),
        )]));
        let (service, _) = service(Arc::clone(&raw_http));

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        let ServerMessage::AuthFailure(failure) = outcome else {
            panic!("expected AuthFailure, got {outcome:?}");
        };
        assert_eq!(failure.code, AuthFailureCode::ServerError);
        assert_eq!(failure.message, "Request validation failed.");
    }

    fn rate_limited(retry_after: Option<&str>, message: &str) -> HttpResponse {
        HttpResponse {
            status: 429,
            error_code: Some("rate_limit_exceeded".into()),
            tokens: None,
            retry_after: retry_after.map(str::to_owned),
            message: Some(message.into()),
            raw_body: None,
            user_id: None,
            device_id: None,
        }
    }

    /// backend-e2e-07 / helper-backend-12: a refused sign-in for too many
    /// attempts was `server_error` with whatever words the server sent. It is
    /// its own failure now, and its wait comes from `Retry-After`, the same
    /// way from velvt-core with #41 (seconds and words) and from the server
    /// deployed before it (neither).
    #[tokio::test]
    async fn a_rate_limited_sign_in_is_its_own_failure_with_the_wait_from_retry_after() {
        for (case, response, expected) in [
            (
                "velvt-core with #41",
                rate_limited(Some("900"), "Too many attempts. Try again in 15 minutes."),
                "Too many attempts. Try again in 15 minutes.",
            ),
            (
                "velvt-core with #41, under a minute left",
                rate_limited(Some("12"), "Too many attempts. Try again in a minute."),
                "Too many attempts. Try again in a minute.",
            ),
            (
                "the server deployed before #41",
                rate_limited(None, "Too many requests."),
                "Too many attempts. Try again later.",
            ),
        ] {
            for flow in ["sign_up", "log_in"] {
                let raw_http = Arc::new(FakeHttpClient::with_responses(vec![response.clone()]));
                let (service, token_store) = service(Arc::clone(&raw_http));

                let outcome = if flow == "sign_up" {
                    service
                        .sign_up("a@example.test".into(), "password".into())
                        .await
                } else {
                    service
                        .log_in("a@example.test".into(), "password".into())
                        .await
                };

                assert_eq!(
                    outcome,
                    ServerMessage::AuthFailure(AuthFailure {
                        code: AuthFailureCode::RateLimited,
                        message: expected.into(),
                    }),
                    "{case}, {flow}"
                );
                assert_eq!(
                    raw_http.requests().len(),
                    1,
                    "{case}, {flow}: nothing after it"
                );
                assert_eq!(
                    token_store.load_device_id().unwrap(),
                    None,
                    "{case}, {flow}"
                );
            }
        }
    }

    #[test]
    fn the_wait_is_rounded_up_to_the_minute_and_never_shorter_than_retry_after() {
        for (retry_after, expected) in [
            (Some("0"), "Too many attempts. Try again in a minute."),
            (Some("60"), "Too many attempts. Try again in a minute."),
            (Some("61"), "Too many attempts. Try again in 2 minutes."),
            (Some(" 300 "), "Too many attempts. Try again in 5 minutes."),
            (Some("7140"), "Too many attempts. Try again in 119 minutes."),
            (Some("7200"), "Too many attempts. Try again in 2 hours."),
            (Some("not a wait"), "Too many attempts. Try again later."),
            (None, "Too many attempts. Try again later."),
        ] {
            assert_eq!(
                rate_limited_message(retry_after),
                expected,
                "{retry_after:?}"
            );
        }
        let in_ten_minutes = (Utc::now() + Duration::seconds(600)).to_rfc2822();
        assert_eq!(
            rate_limited_message(Some(&in_ten_minutes)),
            "Too many attempts. Try again in 10 minutes.",
            "an HTTP date is a wait too"
        );
    }

    #[tokio::test]
    async fn auth_failure_falls_back_to_a_generic_message_when_the_server_sends_none() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 500,
            error_code: None,
            tokens: None,
            retry_after: None,
            message: None,
            raw_body: None,
            user_id: None,
            device_id: None,
        }]));
        let (service, _) = service(Arc::clone(&raw_http));

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        let ServerMessage::AuthFailure(failure) = outcome else {
            panic!("expected AuthFailure, got {outcome:?}");
        };
        assert_eq!(failure.message, "Something went wrong. Please try again.");
    }

    #[tokio::test]
    async fn logout_revokes_with_snapshotted_token_then_clears_local_session() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 204,
            tokens: None,
            device_id: None,
            ..device_response()
        }]));
        let token_store = Arc::new(FakeTokenStore::default());
        let auth_state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
            device_id: "device-1".into(),
        }));
        token_store
            .store_pair(TokenPair::new(
                RedactedString::new("device-access"),
                RedactedString::new("device-refresh"),
                Utc::now() + Duration::hours(1),
            ))
            .unwrap();
        token_store
            .store_user_pair(TokenPair::new(
                RedactedString::new("user-access"),
                RedactedString::new("user-refresh"),
                Utc::now() + Duration::hours(1),
            ))
            .unwrap();
        token_store.store_device_id("device-1").unwrap();
        let service = AccountAuthService::new(
            Arc::clone(&raw_http) as Arc<dyn HttpClient>,
            Arc::new(FakeHttpClient::default()),
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            Arc::clone(&auth_state),
        );

        service.log_out();
        assert!(token_store.load_tokens().unwrap().is_none());
        service.finish_pending_logout().await;

        let requests = raw_http.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/auth/logout");
        assert_eq!(
            requests[0]
                .authorization
                .as_ref()
                .map(|token| token.expose()),
            Some("device-access")
        );
        assert!(token_store.load_tokens().unwrap().is_none());
        assert!(token_store.load_user_tokens().unwrap().is_none());
        assert_eq!(
            token_store.load_device_id().unwrap().as_deref(),
            Some("device-1")
        );
        assert_eq!(auth_state.current(), AuthState::Unauthenticated);
    }

    #[tokio::test]
    async fn accepted_account_deletion_clears_all_local_credentials() {
        let authenticated_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 202,
            tokens: None,
            device_id: None,
            ..device_response()
        }]));
        let token_store = Arc::new(FakeTokenStore::default());
        let auth_state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
            device_id: "device-1".into(),
        }));
        token_store
            .store_pair(TokenPair::new(
                RedactedString::new("device-access"),
                RedactedString::new("device-refresh"),
                Utc::now() + Duration::hours(1),
            ))
            .unwrap();
        token_store
            .store_user_pair(TokenPair::new(
                RedactedString::new("user-access"),
                RedactedString::new("user-refresh"),
                Utc::now() + Duration::hours(1),
            ))
            .unwrap();
        token_store.store_device_id("device-1").unwrap();
        let service = AccountAuthService::new(
            Arc::new(FakeHttpClient::default()),
            authenticated_http,
            Arc::clone(&token_store) as Arc<dyn TokenStore>,
            Arc::clone(&auth_state),
        );

        let outcome = service.delete_account().await;

        assert!(matches!(outcome, ServerMessage::AccountDeletionAccepted(_)));
        assert!(token_store.load_tokens().unwrap().is_none());
        assert!(token_store.load_user_tokens().unwrap().is_none());
        assert!(token_store.load_device_id().unwrap().is_none());
        assert_eq!(auth_state.current(), AuthState::Unauthenticated);
    }
}
