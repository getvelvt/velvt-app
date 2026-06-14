use std::{collections::HashMap, sync::Mutex};

/// Stable-ID persistence boundary. R3 will provide the SQLite implementation.
pub trait AbstractionMappingStore: Send + Sync {
    /// Returns the existing ID for a key or atomically persists the fresh ID.
    fn resolve_id(&self, stable_key: &str, fresh_id: &str) -> Result<String, StoreError>;
}

/// In-memory store used until R3 supplies durable persistence.
#[derive(Debug, Default)]
pub struct InMemoryMappingStore {
    mappings: Mutex<HashMap<String, String>>,
}

impl AbstractionMappingStore for InMemoryMappingStore {
    fn resolve_id(&self, stable_key: &str, fresh_id: &str) -> Result<String, StoreError> {
        let mut mappings = self.mappings.lock().map_err(|_| StoreError::Unavailable)?;
        Ok(mappings
            .entry(stable_key.to_owned())
            .or_insert_with(|| fresh_id.to_owned())
            .clone())
    }
}

/// Privacy-safe mapping-store failures.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Mapping persistence is unavailable.
    #[error("abstraction mapping store unavailable")]
    Unavailable,
}
