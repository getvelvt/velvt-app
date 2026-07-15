use std::{collections::HashMap, sync::Mutex};

/// Stable-ID persistence boundary. R3 will provide the SQLite implementation.
pub trait AbstractionMappingStore: Send + Sync {
    /// Returns a user-selected category for the exact local app/title key.
    fn personal_override(&self, stable_key: &str) -> Result<Option<String>, StoreError>;

    /// Returns the existing ID for a key or atomically persists the fresh mapping.
    fn resolve_id(
        &self,
        stable_key: &str,
        fresh_id: &str,
        label: &str,
        category: &str,
        taxonomy_version: &str,
        classification_tier: &str,
        display_name: &str,
    ) -> Result<String, StoreError>;

    /// Increments a privacy-safe aggregate; no raw classifier input is stored.
    fn increment_classification_count(
        &self,
        taxonomy_version: &str,
        classification_tier: &str,
    ) -> Result<(), StoreError>;
}

/// In-memory store used until R3 supplies durable persistence.
#[derive(Debug, Default)]
pub struct InMemoryMappingStore {
    mappings: Mutex<HashMap<String, String>>,
    overrides: Mutex<HashMap<String, String>>,
}

impl AbstractionMappingStore for InMemoryMappingStore {
    fn personal_override(&self, stable_key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .overrides
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(stable_key)
            .cloned())
    }

    fn resolve_id(
        &self,
        stable_key: &str,
        fresh_id: &str,
        _label: &str,
        _category: &str,
        _taxonomy_version: &str,
        _classification_tier: &str,
        _display_name: &str,
    ) -> Result<String, StoreError> {
        let mut mappings = self.mappings.lock().map_err(|_| StoreError::Unavailable)?;
        Ok(mappings
            .entry(stable_key.to_owned())
            .or_insert_with(|| fresh_id.to_owned())
            .clone())
    }

    fn increment_classification_count(
        &self,
        _taxonomy_version: &str,
        _classification_tier: &str,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Privacy-safe mapping-store failures.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Mapping persistence is unavailable.
    #[error("abstraction mapping store unavailable")]
    Unavailable,
}

impl From<crate::persistence::PersistenceError> for StoreError {
    fn from(_: crate::persistence::PersistenceError) -> Self {
        Self::Unavailable
    }
}
