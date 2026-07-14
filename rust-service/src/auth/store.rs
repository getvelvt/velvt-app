use super::{RedactedString, TokenPair};
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use velvt_shared_types::AuthSession;

pub trait TokenStore: Send + Sync {
    fn store_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError>;
    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError>;
    fn clear_tokens(&self) -> Result<(), TokenStoreError>;

    /// Persists the locally-assigned device identifier returned by device
    /// registration (`POST /v1/devices`). Device-bound, not user-bound:
    /// stored alongside tokens in the same platform credential store, never
    /// in SQLite.
    fn store_device_id(&self, device_id: &str) -> Result<(), TokenStoreError>;
    fn load_device_id(&self) -> Result<Option<String>, TokenStoreError>;
    fn clear_device_id(&self) -> Result<(), TokenStoreError>;

    fn store_pair(&self, tokens: TokenPair) -> Result<(), TokenStoreError> {
        self.store_tokens(
            tokens.access_token().clone(),
            tokens.refresh_token().clone(),
            tokens.expires_at(),
        )
    }

    fn store_user_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError>;
    fn load_user_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError>;
    fn clear_user_tokens(&self) -> Result<(), TokenStoreError>;

    fn store_user_pair(&self, tokens: TokenPair) -> Result<(), TokenStoreError> {
        self.store_user_tokens(
            tokens.access_token().clone(),
            tokens.refresh_token().clone(),
            tokens.expires_at(),
        )
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenStoreError {
    #[error("credential store unavailable")]
    Unavailable,
    #[error("credential store data invalid")]
    InvalidData,
}

#[derive(Clone, Default)]
pub struct FakeTokenStore {
    tokens: Arc<Mutex<Option<TokenPair>>>,
    user_tokens: Arc<Mutex<Option<TokenPair>>>,
    device_id: Arc<Mutex<Option<String>>>,
}

impl TokenStore for FakeTokenStore {
    fn store_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        *self
            .tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? =
            Some(TokenPair::new(access, refresh, expiry));
        Ok(())
    }

    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        self.tokens
            .lock()
            .map(|tokens| tokens.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_tokens(&self) -> Result<(), TokenStoreError> {
        *self
            .tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }

    fn store_user_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        *self
            .user_tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? =
            Some(TokenPair::new(access, refresh, expiry));
        Ok(())
    }

    fn load_user_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        self.user_tokens
            .lock()
            .map(|tokens| tokens.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_user_tokens(&self) -> Result<(), TokenStoreError> {
        *self
            .user_tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }

    fn store_device_id(&self, device_id: &str) -> Result<(), TokenStoreError> {
        *self
            .device_id
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = Some(device_id.to_owned());
        Ok(())
    }

    fn load_device_id(&self) -> Result<Option<String>, TokenStoreError> {
        self.device_id
            .lock()
            .map(|device_id| device_id.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_device_id(&self) -> Result<(), TokenStoreError> {
        *self
            .device_id
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }
}

#[derive(Default)]
pub struct VolatileTokenStore {
    tokens: Mutex<Option<TokenPair>>,
    user_tokens: Mutex<Option<TokenPair>>,
    device_id: Mutex<Option<String>>,
    updates: Mutex<Option<mpsc::UnboundedSender<AuthSession>>>,
}

impl VolatileTokenStore {
    pub fn with_update_sender(sender: mpsc::UnboundedSender<AuthSession>) -> Self {
        Self {
            updates: Mutex::new(Some(sender)),
            ..Self::default()
        }
    }

    fn emit_update(&self) -> Result<(), TokenStoreError> {
        let tokens = self.load_tokens()?;
        let user_tokens = self.load_user_tokens()?;
        let device_id = self.load_device_id()?;
        let (Some(tokens), Some(device_id)) = (tokens, device_id) else {
            return Ok(());
        };
        if let Some(sender) = self
            .updates
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .as_ref()
        {
            let _ = sender.send(AuthSession {
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
            });
        }
        Ok(())
    }
}

impl TokenStore for VolatileTokenStore {
    fn store_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        *self
            .tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? =
            Some(TokenPair::new(access, refresh, expiry));
        self.emit_update()
    }

    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        self.tokens
            .lock()
            .map(|tokens| tokens.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_tokens(&self) -> Result<(), TokenStoreError> {
        *self
            .tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }

    fn store_user_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        *self
            .user_tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? =
            Some(TokenPair::new(access, refresh, expiry));
        self.emit_update()
    }

    fn load_user_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        self.user_tokens
            .lock()
            .map(|tokens| tokens.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_user_tokens(&self) -> Result<(), TokenStoreError> {
        *self
            .user_tokens
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }

    fn store_device_id(&self, device_id: &str) -> Result<(), TokenStoreError> {
        *self
            .device_id
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = Some(device_id.to_owned());
        self.emit_update()
    }

    fn load_device_id(&self) -> Result<Option<String>, TokenStoreError> {
        self.device_id
            .lock()
            .map(|device_id| device_id.clone())
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn clear_device_id(&self) -> Result<(), TokenStoreError> {
        *self
            .device_id
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)? = None;
        Ok(())
    }
}
