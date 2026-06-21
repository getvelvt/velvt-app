//! User-account relay for the protocol-v6 auth messages (`sign_up`, `log_in`,
//! `log_out`, `delete_account`).
//!
//! This is deliberately a thin, stateless HTTP relay: Swift owns its own
//! session tokens in Keychain (see `swift-client/Sources/VelvtMac/Auth`).
//! Rust never persists the resulting `access_token` / `refresh_token` — it
//! only forwards credentials to the cloud and relays the typed result back
//! over IPC.
//!
//! The one exception is device registration: `/v1/devices` requires a
//! user's access token and has no anonymous mode, so the first successful
//! sign-up or login also registers this device (if one isn't already
//! persisted), using that access token only for the lifetime of the single
//! `/v1/devices` call. See [`AccountAuthService::ensure_device_registered`].

use super::{
    AuthState, AuthStateMachine, HttpClient, HttpRequest, RedactedString, TokenPair, TokenStore,
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
        }
    }

    pub async fn sign_up(&self, email: String, password: String) -> ServerMessage {
        let message = self
            .credential_flow("/v1/auth/signup", 201, email, password)
            .await;
        self.ensure_device_registered(&message).await;
        message
    }

    pub async fn log_in(&self, email: String, password: String) -> ServerMessage {
        let message = self
            .credential_flow("/v1/auth/login", 200, email, password)
            .await;
        self.ensure_device_registered(&message).await;
        message
    }

    /// Registers this device with the cloud the first time a user
    /// authenticates. `/v1/devices` requires the user's access token (it has
    /// no anonymous/device-only mode), so this can only run after a
    /// successful sign-up or login — never speculatively at service startup.
    /// A device_id already on disk means a prior login already registered
    /// it, so this is a no-op on every subsequent app launch.
    async fn ensure_device_registered(&self, outcome: &ServerMessage) {
        let ServerMessage::AuthSuccess(success) = outcome else {
            return;
        };
        match self.token_store.load_device_id() {
            Ok(Some(device_id)) => {
                let tokens = TokenPair::new(
                    RedactedString::new(success.access_token.clone()),
                    RedactedString::new(success.refresh_token.clone()),
                    success.expires_at,
                );
                if self.token_store.store_pair(tokens).is_ok() {
                    let _ = self
                        .auth_state
                        .transition(AuthState::Authenticated { device_id });
                }
                return;
            }
            Err(error) => {
                tracing::error!(
                    error_code = "device_id_load_failed",
                    error = %error,
                    "failed to read stored device identifier; skipping device registration"
                );
                return;
            }
            Ok(None) => {}
        }

        let mut request = HttpRequest::post("/v1/devices");
        request.authorization = Some(RedactedString::new(success.access_token.clone()));
        request.json_body =
            Some(serde_json::json!({ "client_version": env!("CARGO_PKG_VERSION") }));
        let response = match self.raw_http.send(request).await {
            Ok(response) if (200..300).contains(&response.status) => response,
            Ok(response) => {
                tracing::error!(
                    error_code = "device_registration_rejected",
                    status = response.status,
                    "device registration was rejected by the server"
                );
                return;
            }
            Err(error) => {
                tracing::error!(
                    error_code = "device_registration_failed",
                    error = %error,
                    "device registration request failed"
                );
                return;
            }
        };
        let (Some(device_id), Some(tokens)) = (response.device_id, response.tokens) else {
            tracing::error!(
                error_code = "device_registration_invalid_response",
                "device registration response was missing tokens or a device id"
            );
            return;
        };
        if let Err(error) = self.token_store.store_pair(tokens) {
            tracing::error!(
                error_code = "device_registration_persist_failed",
                error = %error,
                "failed to persist device tokens after registration"
            );
            return;
        }
        if let Err(error) = self.token_store.store_device_id(&device_id) {
            tracing::error!(
                error_code = "device_registration_persist_failed",
                error = %error,
                "failed to persist device id after registration"
            );
            return;
        }
        if let Err(error) = self
            .auth_state
            .transition(AuthState::Authenticated { device_id })
        {
            tracing::error!(
                error_code = "device_registration_state_transition_failed",
                error = %error,
                "device registered but auth state transition was rejected"
            );
        }
    }

    /// Fire-and-forget per the IPC contract: best-effort server-side
    /// revocation. Failures are not surfaced — the client has already
    /// cleared its local session by the time this is called.
    pub async fn log_out(&self) {
        let _ = self
            .authenticated_http
            .send(HttpRequest::post("/v1/auth/logout"))
            .await;
    }

    pub async fn delete_account(&self) -> ServerMessage {
        match self
            .authenticated_http
            .send(HttpRequest::delete("/v1/account"))
            .await
        {
            Ok(response) if response.status == 202 => {
                ServerMessage::AccountDeletionAccepted(AccountDeletionAccepted {})
            }
            _ => ServerMessage::ErrorResponse(ErrorResponse {
                code: "account_deletion_failed".into(),
                message: "Unable to delete account".into(),
                related_event_id: None,
            }),
        }
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
                        access_token: tokens.access_token().expose().to_owned(),
                        refresh_token: tokens.refresh_token().expose().to_owned(),
                        expires_at: tokens.expires_at(),
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

/// Prefers the cloud API's own `error.message` (e.g. "Email is already
/// registered.", "Request validation failed.") over a generic fallback —
/// the server already writes a message safe to show verbatim, and the
/// fallback before this used to mask everything but bad credentials as
/// "Something went wrong", which made a 409 email-conflict indistinguishable
/// from any other failure.
fn map_auth_failure(response: &super::HttpResponse) -> AuthFailure {
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
            token_store.load_device_id().unwrap(),
            Some("device-1".into())
        );
    }

    #[tokio::test]
    async fn login_skips_device_registration_when_device_id_already_stored() {
        let raw_http = Arc::new(FakeHttpClient::with_responses(vec![HttpResponse {
            status: 200,
            ..signup_response()
        }]));
        let (service, token_store) = service(Arc::clone(&raw_http));
        token_store.store_device_id("existing-device").unwrap();

        let outcome = service
            .log_in("a@example.test".into(), "password".into())
            .await;

        assert!(matches!(outcome, ServerMessage::AuthSuccess(_)));
        assert_eq!(raw_http.requests().len(), 1, "no /v1/devices call expected");
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
}
