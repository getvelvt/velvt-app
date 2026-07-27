use std::{collections::HashMap, sync::Mutex};

#[derive(Clone, PartialEq, Eq)]
pub struct PersonalOverride {
    pub category: String,
    pub local_activity_name: Option<String>,
}

impl std::fmt::Debug for PersonalOverride {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersonalOverride")
            .field("category", &self.category)
            .field(
                "local_activity_name",
                &self.local_activity_name.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

/// Stable-ID persistence boundary. R3 will provide the SQLite implementation.
pub struct MappingResolution<'a> {
    pub stable_key: &'a str,
    pub fresh_id: &'a str,
    pub label: &'a str,
    pub category: &'a str,
    pub taxonomy_version: &'a str,
    pub classification_tier: &'a str,
    pub classification_status: &'a str,
    pub classification_confidence: &'a str,
    pub classification_source: &'a str,
    pub local_display_label: Option<&'a str>,
}

pub trait AbstractionMappingStore: Send + Sync {
    /// Returns a user-selected category for the exact local app/title key.
    fn personal_override(&self, stable_key: &str) -> Result<Option<PersonalOverride>, StoreError>;

    /// Returns the existing ID for a key or atomically persists the fresh mapping.
    fn resolve_id(&self, mapping: MappingResolution<'_>) -> Result<String, StoreError>;

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
    overrides: Mutex<HashMap<String, PersonalOverride>>,
}

impl AbstractionMappingStore for InMemoryMappingStore {
    fn personal_override(&self, stable_key: &str) -> Result<Option<PersonalOverride>, StoreError> {
        Ok(self
            .overrides
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .get(stable_key)
            .cloned())
    }

    fn resolve_id(&self, mapping: MappingResolution<'_>) -> Result<String, StoreError> {
        let mut mappings = self.mappings.lock().map_err(|_| StoreError::Unavailable)?;
        Ok(mappings
            .entry(mapping.stable_key.to_owned())
            .or_insert_with(|| mapping.fresh_id.to_owned())
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
