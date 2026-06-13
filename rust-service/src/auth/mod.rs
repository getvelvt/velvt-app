//! Authentication and platform credential-store interfaces.
//!
//! This module owns token retrieval, storage, and refresh coordination. It does
//! not store tokens in SQLite or own upload batching.

#![allow(async_fn_in_trait)]

use chrono::{DateTime, Utc};

/// Stores authentication tokens in the platform credential store.
pub trait TokenStore {
    /// Loads the current token pair.
    async fn load(&self) -> Result<Option<TokenPair>, AuthError>;

    /// Saves the current token pair.
    async fn save(&self, tokens: &TokenPair) -> Result<(), AuthError>;

    /// Removes all stored authentication tokens.
    async fn clear(&self) -> Result<(), AuthError>;
}

/// Coordinates authentication and token refresh.
pub trait AuthClient {
    /// Refreshes an expired access token.
    async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, AuthError>;
}

/// Access and refresh token pair held only in the credential store.
#[derive(Debug, Clone)]
pub struct TokenPair {
    /// Short-lived cloud API access token.
    pub access_token: String,
    /// Long-lived token used to refresh access.
    pub refresh_token: String,
    /// UTC access-token expiry time.
    pub expires_at: DateTime<Utc>,
}

/// Errors produced by authentication or credential storage.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Platform credential-store operation failed.
    #[error("credential store operation failed")]
    CredentialStore,
    /// Token refresh failed.
    #[error("token refresh failed")]
    Refresh,
    /// Authentication is required.
    #[error("authentication required")]
    Required,
}
