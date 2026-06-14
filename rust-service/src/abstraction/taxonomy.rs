use serde::Deserialize;
use std::{collections::HashMap, path::Path};

const BUILTIN_TAXONOMY: &[u8] = include_bytes!("../../resources/abstraction-taxonomy-mvp-1.json");

/// One built-in application-to-label seed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SeedApplication {
    pub(crate) app_name: String,
    pub(crate) label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxonomyFile {
    version: String,
    default_category: String,
    label_categories: HashMap<String, String>,
    seed_applications: Vec<SeedApplication>,
}

/// Versioned, data-driven category taxonomy and seed dictionary.
#[derive(Debug, Clone)]
pub struct Taxonomy {
    version: String,
    default_category: String,
    label_categories: HashMap<String, String>,
    seed_applications: Vec<SeedApplication>,
}

impl Taxonomy {
    /// Loads the built-in MVP taxonomy.
    pub fn from_builtin() -> Result<Self, TaxonomyError> {
        Self::from_json(BUILTIN_TAXONOMY)
    }

    /// Loads a taxonomy from a configurable JSON path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TaxonomyError> {
        let bytes = std::fs::read(path).map_err(|_| TaxonomyError::Read)?;
        Self::from_json(&bytes)
    }

    /// Parses and validates taxonomy JSON.
    pub fn from_json(bytes: &[u8]) -> Result<Self, TaxonomyError> {
        let file: TaxonomyFile =
            serde_json::from_slice(bytes).map_err(|_| TaxonomyError::InvalidJson)?;
        if file.version.trim().is_empty() {
            return Err(TaxonomyError::MissingVersion);
        }
        if file.seed_applications.is_empty() {
            return Err(TaxonomyError::NoSeedApplications);
        }
        if file.label_categories.is_empty() {
            return Err(TaxonomyError::NoCategories);
        }
        if file.default_category.trim().is_empty() {
            return Err(TaxonomyError::MissingDefaultCategory);
        }
        if file.seed_applications.iter().any(|entry| {
            entry.app_name.trim().is_empty()
                || entry.label.trim().is_empty()
                || !file
                    .label_categories
                    .keys()
                    .any(|prefix| entry.label.starts_with(prefix))
        }) {
            return Err(TaxonomyError::InvalidSeedApplication);
        }
        Ok(Self {
            version: file.version,
            default_category: file.default_category,
            label_categories: file.label_categories,
            seed_applications: file.seed_applications,
        })
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn category_for_label(&self, label: &str) -> &str {
        self.label_categories
            .iter()
            .filter(|(prefix, _)| label.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map_or(self.default_category.as_str(), |(_, category)| category)
    }

    pub(crate) fn seed_applications(&self) -> Vec<SeedApplication> {
        self.seed_applications.clone()
    }
}

/// Clear startup errors for invalid or unavailable taxonomy configuration.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaxonomyError {
    #[error("failed to read abstraction taxonomy")]
    Read,
    #[error("abstraction taxonomy is invalid JSON")]
    InvalidJson,
    #[error("taxonomy version is missing")]
    MissingVersion,
    #[error("taxonomy has no category entries")]
    NoCategories,
    #[error("taxonomy has no seed application entries")]
    NoSeedApplications,
    #[error("taxonomy default category is missing")]
    MissingDefaultCategory,
    #[error("taxonomy contains an invalid seed application")]
    InvalidSeedApplication,
}
