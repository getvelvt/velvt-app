use serde::Deserialize;
use std::{collections::HashSet, path::Path};

const BUILTIN_TAXONOMY: &[u8] = include_bytes!("../../resources/abstraction-taxonomy-mvp-1.json");
pub const API_EXPECTED_TAXONOMY_VERSION: &str = "mvp-1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedApplication {
    app_name_pattern: String,
    label: String,
    category: String,
}

impl SeedApplication {
    pub fn app_name_pattern(&self) -> &str {
        &self.app_name_pattern
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn category(&self) -> &str {
        &self.category
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxonomyFile {
    category_taxonomy_version: String,
    default_category: String,
    categories: Vec<String>,
    seed_applications: Vec<SeedApplication>,
}

#[derive(Debug, Clone)]
pub struct Taxonomy {
    category_taxonomy_version: String,
    default_category: String,
    categories: HashSet<String>,
    seed_applications: Vec<SeedApplication>,
}

impl Taxonomy {
    pub fn from_builtin() -> Result<Self, TaxonomyError> {
        Self::from_json(BUILTIN_TAXONOMY)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TaxonomyError> {
        let bytes = std::fs::read(path).map_err(|_| TaxonomyError::Read)?;
        Self::from_json(&bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, TaxonomyError> {
        let file: TaxonomyFile =
            serde_json::from_slice(bytes).map_err(|_| TaxonomyError::InvalidJson)?;
        if file.category_taxonomy_version.trim().is_empty() {
            return Err(TaxonomyError::MissingVersion);
        }
        if file.seed_applications.is_empty() {
            return Err(TaxonomyError::NoSeedApplications);
        }
        let categories: HashSet<_> = file.categories.into_iter().collect();
        if categories.is_empty() || !categories.contains(&file.default_category) {
            return Err(TaxonomyError::NoCategories);
        }
        let mut patterns = HashSet::new();
        if file.seed_applications.iter().any(|entry| {
            entry.app_name_pattern.trim().is_empty()
                || !is_valid_label(&entry.label)
                || !categories.contains(&entry.category)
                || !patterns.insert(entry.app_name_pattern.to_ascii_lowercase())
        }) {
            return Err(TaxonomyError::InvalidSeedApplication);
        }
        Ok(Self {
            category_taxonomy_version: file.category_taxonomy_version,
            default_category: file.default_category,
            categories,
            seed_applications: file.seed_applications,
        })
    }

    pub fn version(&self) -> &str {
        &self.category_taxonomy_version
    }

    pub fn default_category(&self) -> &str {
        &self.default_category
    }

    pub fn contains_category(&self, category: &str) -> bool {
        self.categories.contains(category)
    }

    pub fn seed_applications(&self) -> Vec<SeedApplication> {
        self.seed_applications.clone()
    }
}

pub(crate) fn is_valid_label(label: &str) -> bool {
    if label == "unlogged" {
        return true;
    }
    let mut parts = label.split(':');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    };
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(prefix), Some(action), None) if valid_part(prefix) && valid_part(action)
    )
}

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
    #[error("taxonomy contains an invalid seed application")]
    InvalidSeedApplication,
}
