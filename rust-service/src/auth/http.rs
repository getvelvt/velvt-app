use super::{AuthError, RedactedString, TokenPair};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Delete,
}

#[derive(Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub authorization: Option<RedactedString>,
    pub refresh_token: Option<RedactedString>,
    pub json_body: Option<Value>,
    pub timeout: Option<Duration>,
}

impl HttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            authorization: None,
            refresh_token: None,
            json_body: None,
            timeout: None,
        }
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Post,
            path: path.into(),
            authorization: None,
            refresh_token: None,
            json_body: None,
            timeout: None,
        }
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Delete,
            path: path.into(),
            authorization: None,
            refresh_token: None,
            json_body: None,
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub error_code: Option<String>,
    pub tokens: Option<TokenPair>,
    pub retry_after: Option<String>,
    pub message: Option<String>,
    /// Raw response body as a parsed JSON value, available for non-auth endpoints.
    pub raw_body: Option<serde_json::Value>,
    /// Present on `/v1/devices` and account responses that identify a user or device.
    pub user_id: Option<String>,
    /// Present on `/v1/devices` responses.
    pub device_id: Option<String>,
}

pub trait HttpClient: Send + Sync {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>>;
}

pub struct ReqwestHttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            // An unreachable or slow cloud host (e.g. during startup device
            // registration) must fail fast, not hang the service or any
            // test that exercises real startup.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

impl HttpClient for ReqwestHttpClient {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!("{}{}", self.base_url.trim_end_matches('/'), request.path);
            let mut builder = match request.method {
                HttpMethod::Get => self.client.get(url),
                HttpMethod::Post => self.client.post(url),
                HttpMethod::Delete => self.client.delete(url),
            };
            if let Some(timeout) = request.timeout {
                builder = builder.timeout(timeout);
            }
            if let Some(token) = request.authorization {
                builder = builder.bearer_auth(token.expose());
            }
            if let Some(token) = request.refresh_token {
                builder = builder.json(&serde_json::json!({ "refresh_token": token.expose() }));
            } else if let Some(body) = request.json_body {
                builder = builder.json(&body);
            }
            let response = builder.send().await.map_err(|_| AuthError::Transport)?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let bytes = response.bytes().await.unwrap_or_default();
            let raw_body: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
            let body: ApiResponse = serde_json::from_slice(&bytes).unwrap_or_default();
            let error_code = body.error_code();
            let message = body.message();
            let user_id = body.user_id();
            let device_id = body.device_id();
            Ok(HttpResponse {
                status,
                error_code,
                tokens: body.into_tokens(),
                retry_after,
                message,
                raw_body,
                user_id,
                device_id,
            })
        })
    }
}

#[derive(Default, Deserialize)]
struct ApiResponse {
    error: Option<ApiError>,
    user: Option<ApiUser>,
    device: Option<ApiDevice>,
    tokens: Option<ApiTokens>,
    #[serde(flatten)]
    flat_tokens: ApiTokens,
}

#[derive(Default, Deserialize)]
struct ApiError {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Default, Deserialize)]
struct ApiUser {
    id: Option<String>,
}

#[derive(Default, Deserialize)]
struct ApiDevice {
    id: Option<String>,
}

#[derive(Default, Deserialize)]
struct ApiTokens {
    access_token: Option<RedactedString>,
    refresh_token: Option<RedactedString>,
    expires_in: Option<i64>,
    device_id: Option<String>,
}

impl ApiResponse {
    fn error_code(&self) -> Option<String> {
        self.error.as_ref().and_then(|error| error.code.clone())
    }

    fn message(&self) -> Option<String> {
        self.error.as_ref().and_then(|error| error.message.clone())
    }

    fn user_id(&self) -> Option<String> {
        self.user.as_ref().and_then(|user| user.id.clone())
    }

    fn device_id(&self) -> Option<String> {
        self.device
            .as_ref()
            .and_then(|device| device.id.clone())
            .or_else(|| {
                self.tokens
                    .as_ref()
                    .and_then(|tokens| tokens.device_id.clone())
            })
            .or_else(|| self.flat_tokens.device_id.clone())
    }

    fn into_tokens(self) -> Option<TokenPair> {
        let tokens = self.tokens.unwrap_or(self.flat_tokens);
        Some(TokenPair::new(
            tokens.access_token?,
            tokens.refresh_token?,
            Utc::now().checked_add_signed(chrono::Duration::seconds(tokens.expires_in?))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signup_response_uses_nested_tokens_with_expires_in() {
        let response: ApiResponse = serde_json::from_value(serde_json::json!({
            "user": { "id": "user-1", "email": "person@example.test", "status": "active" },
            "tokens": {
                "access_token": "access",
                "refresh_token": "refresh",
                "token_type": "bearer",
                "expires_in": 3600,
                "refresh_expires_in": 86400,
                "device_id": "device-1"
            }
        }))
        .unwrap();

        assert_eq!(response.user_id().as_deref(), Some("user-1"));
        assert!(response.into_tokens().is_some());
    }

    #[test]
    fn error_response_uses_nested_error_envelope() {
        let response: ApiResponse = serde_json::from_value(serde_json::json!({
            "error": { "code": "invalid_credentials", "message": "Invalid credentials" }
        }))
        .unwrap();

        assert_eq!(
            response.error_code().as_deref(),
            Some("invalid_credentials")
        );
        assert_eq!(response.message().as_deref(), Some("Invalid credentials"));
    }

    #[test]
    fn device_reissue_response_uses_top_level_tokens() {
        let response: ApiResponse = serde_json::from_value(serde_json::json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "token_type": "bearer",
            "expires_in": 3600,
            "refresh_expires_in": 86400,
            "device_id": "device-1"
        }))
        .unwrap();

        assert_eq!(response.device_id().as_deref(), Some("device-1"));
        assert!(response.into_tokens().is_some());
    }
}
