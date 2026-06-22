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

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct KeychainTokenStore {
    service: String,
    account: String,
    cache: Mutex<KeychainCache>,
}

#[derive(Default)]
struct KeychainCache {
    tokens: Option<Option<TokenPair>>,
    device_id: Option<Option<String>>,
}

impl KeychainTokenStore {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
            cache: Mutex::new(KeychainCache::default()),
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
            .map_err(|_| TokenStoreError::Unavailable)?;
        self.cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .tokens = Some(Some(TokenPair::new(access, refresh, expiry)));
        Ok(())
    }

    fn load_tokens(&self) -> Result<Option<TokenPair>, TokenStoreError> {
        if let Some(tokens) = self
            .cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .tokens
            .clone()
        {
            return Ok(tokens);
        }
        let bytes =
            match security_framework::passwords::get_generic_password(&self.service, &self.account)
            {
                Ok(bytes) => bytes,
                Err(error) if error.code() == -25300 => {
                    self.cache
                        .lock()
                        .map_err(|_| TokenStoreError::Unavailable)?
                        .tokens = Some(None);
                    return Ok(None);
                }
                Err(_) => return Err(TokenStoreError::Unavailable),
            };
        let record: KeychainReadRecord =
            serde_json::from_slice(&bytes).map_err(|_| TokenStoreError::InvalidData)?;
        let tokens = Some(TokenPair::new(record.access, record.refresh, record.expiry));
        self.cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .tokens = Some(tokens.clone());
        Ok(tokens)
    }

    fn clear_tokens(&self) -> Result<(), TokenStoreError> {
        match security_framework::passwords::delete_generic_password(&self.service, &self.account) {
            Ok(()) => {
                self.cache
                    .lock()
                    .map_err(|_| TokenStoreError::Unavailable)?
                    .tokens = Some(None);
                Ok(())
            }
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(TokenStoreError::Unavailable),
        }
    }

    fn store_device_id(&self, device_id: &str) -> Result<(), TokenStoreError> {
        security_framework::passwords::set_generic_password(
            &self.service,
            &self.device_id_account(),
            device_id.as_bytes(),
        )
        .map_err(|_| TokenStoreError::Unavailable)?;
        self.cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .device_id = Some(Some(device_id.to_owned()));
        Ok(())
    }

    fn load_device_id(&self) -> Result<Option<String>, TokenStoreError> {
        if let Some(device_id) = self
            .cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .device_id
            .clone()
        {
            return Ok(device_id);
        }
        let device_id = match security_framework::passwords::get_generic_password(
            &self.service,
            &self.device_id_account(),
        ) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| TokenStoreError::InvalidData),
            Err(error) if error.code() == -25300 => {
                self.cache
                    .lock()
                    .map_err(|_| TokenStoreError::Unavailable)?
                    .device_id = Some(None);
                return Ok(None);
            }
            Err(_) => Err(TokenStoreError::Unavailable),
        }?;
        self.cache
            .lock()
            .map_err(|_| TokenStoreError::Unavailable)?
            .device_id = Some(device_id.clone());
        Ok(device_id)
    }

    fn clear_device_id(&self) -> Result<(), TokenStoreError> {
        match security_framework::passwords::delete_generic_password(
            &self.service,
            &self.device_id_account(),
        ) {
            Ok(()) => {
                self.cache
                    .lock()
                    .map_err(|_| TokenStoreError::Unavailable)?
                    .device_id = Some(None);
                Ok(())
            }
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(TokenStoreError::Unavailable),
        }
    }
}

#[cfg(target_os = "macos")]
impl KeychainTokenStore {
    fn device_id_account(&self) -> String {
        format!("{}.device_id", self.account)
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

    fn store_device_id(&self, _device_id: &str) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }

    fn load_device_id(&self) -> Result<Option<String>, TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }

    fn clear_device_id(&self) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Unavailable)
    }
}
