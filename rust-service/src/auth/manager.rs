use super::{
    AuthState, AuthStateMachine, AuthTransitionError, HttpClient, HttpRequest, HttpResponse,
    TokenStore, TokenStoreError,
};
use chrono::{Duration, Utc};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AuthManager<S, H> {
    store: Arc<S>,
    http: Arc<H>,
    state: Arc<AuthStateMachine>,
    refresh_buffer: Duration,
    refresh_lock: Mutex<()>,
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
        }
    }

    pub async fn send_authenticated(
        &self,
        mut request: HttpRequest,
    ) -> Result<HttpResponse, AuthError> {
        if self.state.current() == AuthState::DeviceRevoked {
            return Err(AuthError::DeviceRevoked);
        }
        let tokens = self.tokens_for_request().await?;
        request.authorization = Some(tokens.access_token().clone());
        let response = self.http.send(request.clone()).await?;
        if matches!(
            (response.status, response.error_code.as_deref()),
            (
                401,
                Some("invalid_credentials" | "invalid_token" | "token_expired")
            )
        ) {
            let fresh = self.refresh_tokens(true).await?;
            request.authorization = Some(fresh.access_token().clone());
            let retried = self.http.send(request.clone()).await?;
            return self.handle_response(retried, request).await;
        }
        self.handle_response(response, request).await
    }

    async fn tokens_for_request(&self) -> Result<super::TokenPair, AuthError> {
        let tokens = self.store.load_tokens()?.ok_or(AuthError::NeedsReauth)?;
        if tokens.expires_at() > Utc::now() + self.refresh_buffer {
            return Ok(tokens);
        }
        self.refresh_tokens(false).await
    }

    async fn refresh_tokens(&self, force: bool) -> Result<super::TokenPair, AuthError> {
        let _guard = self.refresh_lock.lock().await;
        let tokens = self.store.load_tokens()?.ok_or(AuthError::NeedsReauth)?;
        if !force && tokens.expires_at() > Utc::now() + self.refresh_buffer {
            return Ok(tokens);
        }
        let device_id = match self.state.current() {
            AuthState::Authenticated { device_id } => device_id,
            AuthState::RefreshInFlight => return Err(AuthError::NeedsReauth),
            AuthState::DeviceRevoked => return Err(AuthError::DeviceRevoked),
            AuthState::Unauthenticated | AuthState::NeedsReauth => {
                return Err(AuthError::NeedsReauth)
            }
        };
        self.state.transition(AuthState::RefreshInFlight)?;
        let mut refresh_request = HttpRequest::post("/v1/auth/refresh");
        refresh_request.refresh_token = Some(tokens.refresh_token().clone());
        let response = match self.http.send(refresh_request).await {
            Ok(response) => response,
            Err(error) => {
                self.state
                    .transition(AuthState::Authenticated { device_id })?;
                return Err(error);
            }
        };
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
        if matches!(
            (response.status, response.error_code.as_deref()),
            (403, Some("device_revoked"))
        ) {
            self.state.transition(AuthState::DeviceRevoked)?;
            return Err(AuthError::DeviceRevoked);
        }
        if matches!(
            (response.status, response.error_code.as_deref()),
            (403, Some("device_token_revoked"))
        ) {
            self.state.transition(AuthState::NeedsReauth)?;
            return Err(AuthError::NeedsReauth);
        }
        self.state.transition(AuthState::NeedsReauth)?;
        match response.status {
            403 => Err(AuthError::Forbidden),
            429 => Err(AuthError::RateLimited),
            _ => Err(AuthError::NeedsReauth),
        }
    }

    async fn handle_response(
        &self,
        response: HttpResponse,
        mut request: HttpRequest,
    ) -> Result<HttpResponse, AuthError> {
        match (response.status, response.error_code.as_deref()) {
            (403, Some("device_revoked")) => {
                self.state.transition(AuthState::DeviceRevoked)?;
                Err(AuthError::DeviceRevoked)
            }
            (403, Some("device_token_revoked")) => {
                let fresh = self.reissue_device_tokens().await?;
                request.authorization = Some(fresh.access_token().clone());
                let retried = self.http.send(request).await?;
                match (retried.status, retried.error_code.as_deref()) {
                    (403, Some("device_revoked")) => {
                        self.state.transition(AuthState::DeviceRevoked)?;
                        Err(AuthError::DeviceRevoked)
                    }
                    (403, Some("device_token_revoked")) => {
                        self.state.transition(AuthState::NeedsReauth)?;
                        Err(AuthError::NeedsReauth)
                    }
                    (401, Some("invalid_credentials" | "invalid_token" | "token_expired")) => {
                        self.state.transition(AuthState::NeedsReauth)?;
                        Err(AuthError::NeedsReauth)
                    }
                    (403, _) => {
                        self.state.transition(AuthState::NeedsReauth)?;
                        Err(AuthError::NeedsReauth)
                    }
                    (429, _) => Err(AuthError::RateLimited),
                    _ => Ok(retried),
                }
            }
            (401, Some("invalid_credentials" | "invalid_token" | "token_expired")) => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            (403, _) => {
                self.state.transition(AuthState::NeedsReauth)?;
                Err(AuthError::NeedsReauth)
            }
            (429, _) => Err(AuthError::RateLimited),
            _ => Ok(response),
        }
    }

    async fn reissue_device_tokens(&self) -> Result<super::TokenPair, AuthError> {
        let _guard = self.refresh_lock.lock().await;
        let tokens = self.store.load_tokens()?.ok_or(AuthError::NeedsReauth)?;
        let device_id = match self.state.current() {
            AuthState::Authenticated { device_id } => device_id,
            AuthState::DeviceRevoked => return Err(AuthError::DeviceRevoked),
            AuthState::Unauthenticated | AuthState::NeedsReauth | AuthState::RefreshInFlight => {
                return Err(AuthError::NeedsReauth);
            }
        };
        self.state.transition(AuthState::RefreshInFlight)?;
        let mut request = HttpRequest::post("/v1/auth/devices/reissue");
        request.authorization = Some(tokens.access_token().clone());
        request.json_body = Some(serde_json::json!({ "device_id": device_id }));
        let response = match self.http.send(request).await {
            Ok(response) => response,
            Err(error) => {
                self.state
                    .transition(AuthState::Authenticated { device_id })?;
                return Err(error);
            }
        };
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
        if matches!(
            (response.status, response.error_code.as_deref()),
            (403, Some("device_revoked"))
        ) {
            self.state.transition(AuthState::DeviceRevoked)?;
            return Err(AuthError::DeviceRevoked);
        }
        self.state.transition(AuthState::NeedsReauth)?;
        match response.status {
            403 => Err(AuthError::Forbidden),
            429 => Err(AuthError::RateLimited),
            _ => Err(AuthError::NeedsReauth),
        }
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
    #[error("request forbidden")]
    Forbidden,
    #[error("request rate limited")]
    RateLimited,
    #[error("authentication transport unavailable")]
    Transport,
    #[error("authentication response invalid")]
    InvalidResponse,
    #[error(transparent)]
    TokenStore(#[from] TokenStoreError),
    #[error(transparent)]
    Transition(#[from] AuthTransitionError),
}
