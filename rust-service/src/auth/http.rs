use super::{AuthError, RedactedString, TokenPair};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub authorization: Option<RedactedString>,
    pub refresh_token: Option<RedactedString>,
}

impl HttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            authorization: None,
            refresh_token: None,
        }
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Post,
            path: path.into(),
            authorization: None,
            refresh_token: None,
        }
    }
}

#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub error_code: Option<String>,
    pub tokens: Option<TokenPair>,
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
            client: reqwest::Client::new(),
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
            };
            if let Some(token) = request.authorization {
                builder = builder.bearer_auth(token.expose());
            }
            if let Some(token) = request.refresh_token {
                builder = builder.json(&serde_json::json!({ "refresh_token": token.expose() }));
            }
            let response = builder.send().await.map_err(|_| AuthError::Transport)?;
            let status = response.status().as_u16();
            let body = response.json::<ApiResponse>().await.unwrap_or_default();
            let error_code = body.code.clone();
            Ok(HttpResponse {
                status,
                error_code,
                tokens: body.into_tokens(),
            })
        })
    }
}

#[derive(Default, Deserialize)]
struct ApiResponse {
    code: Option<String>,
    access_token: Option<RedactedString>,
    refresh_token: Option<RedactedString>,
    expires_at: Option<DateTime<Utc>>,
}

impl ApiResponse {
    fn into_tokens(self) -> Option<TokenPair> {
        Some(TokenPair::new(
            self.access_token?,
            self.refresh_token?,
            self.expires_at?,
        ))
    }
}
