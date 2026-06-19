//! User-account relay for the protocol-v6 auth messages (`sign_up`, `log_in`,
//! `log_out`, `delete_account`).
//!
//! This is deliberately a thin, stateless HTTP relay: Swift owns its own
//! session tokens in Keychain (see `swift-client/Sources/VelvtMac/Auth`).
//! Rust never persists the resulting `access_token` / `refresh_token` — it
//! only forwards credentials to the cloud and relays the typed result back
//! over IPC. This is a separate concern from device-bound auth
//! ([`super::AuthManager`]), which gates the upload/fetch pipeline.

use super::{HttpClient, HttpRequest};
use std::sync::Arc;
use velvt_shared_types::{
    AccountDeletionAccepted, AuthFailure, AuthFailureCode, AuthSuccess, ErrorResponse,
    ServerMessage,
};

pub struct AccountAuthService {
    /// Unauthenticated transport for credential-based signup/login.
    raw_http: Arc<dyn HttpClient>,
    /// Device-authenticated transport for logout/account deletion.
    authenticated_http: Arc<dyn HttpClient>,
}

impl AccountAuthService {
    pub fn new(raw_http: Arc<dyn HttpClient>, authenticated_http: Arc<dyn HttpClient>) -> Self {
        Self {
            raw_http,
            authenticated_http,
        }
    }

    pub async fn sign_up(&self, email: String, password: String) -> ServerMessage {
        self.credential_flow("/v1/auth/signup", email, password)
            .await
    }

    pub async fn log_in(&self, email: String, password: String) -> ServerMessage {
        self.credential_flow("/v1/auth/login", email, password)
            .await
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
            .send(HttpRequest::post("/v1/auth/account/delete"))
            .await
        {
            Ok(response) if (200..300).contains(&response.status) => {
                ServerMessage::AccountDeletionAccepted(AccountDeletionAccepted {})
            }
            _ => ServerMessage::ErrorResponse(ErrorResponse {
                code: "account_deletion_failed".into(),
                message: "Unable to delete account".into(),
                related_event_id: None,
            }),
        }
    }

    async fn credential_flow(&self, path: &str, email: String, password: String) -> ServerMessage {
        tracing::debug!(path, email = %email, "auth.credential_flow: sending request");
        let mut request = HttpRequest::post(path);
        request.json_body = Some(serde_json::json!({ "email": email, "password": password }));
        match self.raw_http.send(request).await {
            Ok(response) if (200..300).contains(&response.status) => {
                match (response.user_id, response.tokens) {
                    (Some(user_id), Some(tokens)) => {
                        tracing::debug!(user_id = %user_id, "auth.credential_flow: success, got tokens");
                        ServerMessage::AuthSuccess(AuthSuccess {
                            user_id,
                            access_token: tokens.access_token().expose().to_owned(),
                            refresh_token: tokens.refresh_token().expose().to_owned(),
                            expires_at: tokens.expires_at(),
                        })
                    }
                    _ => {
                        tracing::warn!(
                            "auth.credential_flow: 2xx response but missing user_id or tokens"
                        );
                        ServerMessage::AuthFailure(AuthFailure {
                            code: AuthFailureCode::ServerError,
                            message: "The server response was invalid.".into(),
                        })
                    }
                }
            }
            Ok(response) => {
                tracing::warn!(
                    status = response.status,
                    error_code = ?response.error_code,
                    "auth.credential_flow: non-2xx response"
                );
                ServerMessage::AuthFailure(map_auth_failure(&response))
            }
            Err(err) => {
                tracing::error!(error = %err, "auth.credential_flow: HTTP send failed");
                ServerMessage::AuthFailure(AuthFailure {
                    code: AuthFailureCode::NetworkError,
                    message: "Could not reach the server. Check your connection and try again."
                        .into(),
                })
            }
        }
    }
}

fn map_auth_failure(response: &super::HttpResponse) -> AuthFailure {
    match (response.status, response.error_code.as_deref()) {
        (401 | 403, _) | (_, Some("invalid_credentials")) => AuthFailure {
            code: AuthFailureCode::InvalidCredentials,
            message: "Incorrect email or password.".into(),
        },
        _ => AuthFailure {
            code: AuthFailureCode::ServerError,
            message: "Something went wrong. Please try again.".into(),
        },
    }
}
