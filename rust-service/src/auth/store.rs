use super::{RedactedString, TokenPair};
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

pub trait TokenStore: Send + Sync {
    fn store_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError>;
    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError>;
    fn clear_tokens(&self) -> Result<(), TokenStoreError>;

    fn store_pair(&self, tokens: TokenPair) -> Result<(), TokenStoreError> {
        self.store_tokens(
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
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct KeychainTokenStore {
    service: String,
    account: String,
}

impl KeychainTokenStore {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }
}

impl Default for KeychainTokenStore {
    fn default() -> Self {
        Self::new("com.velvt.service.auth", "tokens")
    }
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize)]
struct KeychainWriteRecord<'a> {
    access: &'a str,
    refresh: &'a str,
    expiry: DateTime<Utc>,
}

#[cfg(target_os = "macos")]
#[derive(serde::Deserialize)]
struct KeychainReadRecord {
    access: RedactedString,
    refresh: RedactedString,
    expiry: DateTime<Utc>,
}

#[cfg(target_os = "macos")]
impl TokenStore for KeychainTokenStore {
    fn store_tokens(
        &self,
        access: RedactedString,
        refresh: RedactedString,
        expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        let record = KeychainWriteRecord {
            access: access.expose(),
            refresh: refresh.expose(),
            expiry,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| TokenStoreError::InvalidData)?;
        security_framework::passwords::set_generic_password(&self.service, &self.account, &bytes)
            .map_err(|_| TokenStoreError::Unavailable)
    }

    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        let bytes =
            match security_framework::passwords::get_generic_password(&self.service, &self.account)
            {
                Ok(bytes) => bytes,
                Err(error) if error.code() == -25300 => return Ok(None),
                Err(_) => return Err(TokenStoreError::Unavailable),
            };
        let record: KeychainReadRecord =
            serde_json::from_slice(&bytes).map_err(|_| TokenStoreError::InvalidData)?;
        Ok(Some(TokenPair::new(
            record.access,
            record.refresh,
            record.expiry,
        )))
    }

    fn clear_tokens(&self) -> Result<(), TokenStoreError> {
        match security_framework::passwords::delete_generic_password(&self.service, &self.account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(TokenStoreError::Unavailable),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl TokenStore for KeychainTokenStore {
    fn store_tokens(
        &self,
        _access: RedactedString,
        _refresh: RedactedString,
        _expiry: DateTime<Utc>,
    ) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }

    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }

    fn clear_tokens(&self) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }
}
