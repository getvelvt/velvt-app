use serde::Deserialize;
use std::{collections::HashSet, path::Path};

use super::normalize::normalize_classifier_text;

const BUILTIN_TAXONOMY: &[u8] = include_bytes!("../../resources/abstraction-taxonomy-mvp-1.json");
pub const API_EXPECTED_TAXONOMY_VERSION: &str = "mvp-2";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedApplication {
    app_name_pattern: String,
    label: String,
    category: String,
    /// The application's bundle identifier, when it has a known one.
    ///
    /// Optional because most seeds are patterns over a family of names
    /// (`Adobe Photoshop*`), which no single bundle identifier covers. Where a
    /// seed does name one concrete application, the bundle identifier is the
    /// better key: `app_name_pattern` matches the localized name macOS reports,
    /// and that name is translated, renamed between releases, and sometimes not
    /// the name anyone uses — for Visual Studio Code it is literally `Code`.
    #[serde(default)]
    bundle_identifier: Option<String>,
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

    pub fn bundle_identifier(&self) -> Option<&str> {
        self.bundle_identifier.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        app_name_pattern: impl Into<String>,
        label: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            app_name_pattern: app_name_pattern.into(),
            label: label.into(),
            category: category.into(),
            bundle_identifier: None,
        }
    }
}

/// A seed keyed on a bundle identifier rather than a displayed name.
///
/// Two things produce one of these: a `seed_applications` entry that carries a
/// `bundle_identifier`, and a `seed_bundles` entry, which is for an application
/// Velvt can identify but whose displayed name is too generic to match on
/// (`Messages`, `Notes`, `Contacts`). They are the same fact once loaded, so
/// [`Taxonomy::seed_bundles`] returns both as this type.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedBundle {
    bundle_identifier: String,
    label: String,
    category: String,
}

impl SeedBundle {
    pub fn bundle_identifier(&self) -> &str {
        &self.bundle_identifier
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        bundle_identifier: impl Into<String>,
        label: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        Self {
            bundle_identifier: bundle_identifier.into(),
            label: label.into(),
            category: category.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaxonomyFile {
    category_taxonomy_version: String,
    default_category: String,
    categories: Vec<String>,
    seed_applications: Vec<SeedApplication>,
    /// Bundle-only seeds: applications with no name worth matching on.
    #[serde(default)]
    seed_bundles: Vec<SeedBundle>,
    /// Editorial notes for whoever opens the JSON next.
    ///
    /// JSON has no comment syntax and `deny_unknown_fields` means a stray key
    /// fails the load, so a note about why an entry is *absent* has nowhere
    /// else to live — and an absence is exactly the thing a later editor will
    /// otherwise "fix". Read by the loader only to accept it; never used to
    /// classify anything.
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Taxonomy {
    category_taxonomy_version: String,
    default_category: String,
    categories: HashSet<String>,
    seed_applications: Vec<SeedApplication>,
    seed_bundles: Vec<SeedBundle>,
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
                || !patterns.insert(
                    entry
                        .app_name_pattern
                        .split('*')
                        .map(normalize_classifier_text)
                        .collect::<Vec<_>>()
                        .join("*"),
                )
        }) {
            return Err(TaxonomyError::InvalidSeedApplication);
        }
        let seed_bundles = merged_seed_bundles(&file.seed_applications, file.seed_bundles);
        let mut identifiers = HashSet::new();
        if seed_bundles.iter().any(|entry| {
            entry.bundle_identifier.trim().is_empty()
                || !is_valid_label(&entry.label)
                || !categories.contains(&entry.category)
                // Case-insensitively unique: Launch Services treats bundle
                // identifiers case-insensitively, so two entries differing only
                // in case are one application claiming two categories, which is
                // a mistake in the file rather than a tie to break at runtime.
                || !identifiers.insert(entry.bundle_identifier.trim().to_ascii_lowercase())
        }) {
            return Err(TaxonomyError::InvalidSeedBundle);
        }
        // `notes` is documentation for a human reader of the JSON. Binding it
        // here rather than dropping it in the pattern keeps the reason for the
        // field visible next to the field.
        let _editorial_notes = file.notes;
        Ok(Self {
            category_taxonomy_version: file.category_taxonomy_version,
            default_category: file.default_category,
            categories,
            seed_applications: file.seed_applications,
            seed_bundles,
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

    /// Every bundle-keyed seed, from both of the places the file can declare
    /// one. The bundle tier consumes this and nothing else.
    pub fn seed_bundles(&self) -> Vec<SeedBundle> {
        self.seed_bundles.clone()
    }
}

/// Flattens the two declaration sites into one list.
///
/// A `seed_applications` entry with a bundle identifier keeps its label and
/// category: the two tiers must agree about an application, or a client that
/// reports a bundle identifier would classify it differently from one that does
/// not.
fn merged_seed_bundles(
    applications: &[SeedApplication],
    declared: Vec<SeedBundle>,
) -> Vec<SeedBundle> {
    applications
        .iter()
        .filter_map(|entry| {
            entry
                .bundle_identifier()
                .map(|bundle_identifier| SeedBundle {
                    bundle_identifier: bundle_identifier.to_owned(),
                    label: entry.label.clone(),
                    category: entry.category.clone(),
                })
        })
        .chain(declared)
        .collect()
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
    #[error("taxonomy contains an invalid bundle seed")]
    InvalidSeedBundle,
}

#[cfg(test)]
mod tests {
    use super::Taxonomy;

    /// The case that motivated keying on the bundle identifier at all. If this
    /// entry is ever dropped from the shipped file, the user's editor becomes
    /// invisible again and nothing else fails.
    #[test]
    fn the_shipped_taxonomy_declares_the_editors_bundle_identifier() {
        let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");

        let editor = taxonomy
            .seed_bundles()
            .into_iter()
            .find(|entry| entry.bundle_identifier() == "com.microsoft.VSCode")
            .expect("Visual Studio Code is seeded by bundle identifier");

        assert_eq!(editor.category(), "FOCUS_WORK");
    }

    /// The browser seeds were unreachable and are gone (see the `notes` array in
    /// the shipped file). Re-adding one by bundle identifier would route browser
    /// windows through the bundle tier, which classifies the *application* —
    /// exactly the judgement a browser window must not receive, because its
    /// identity comes from the tab.
    #[test]
    fn the_shipped_taxonomy_seeds_no_browser() {
        let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");

        for browser in [
            "com.apple.Safari",
            "com.google.Chrome",
            "org.mozilla.firefox",
            "company.thebrowser.Browser",
            "com.brave.Browser",
            "com.microsoft.edgemac",
        ] {
            assert!(
                !taxonomy
                    .seed_bundles()
                    .iter()
                    .any(|entry| entry.bundle_identifier() == browser),
                "{browser} is seeded by bundle identifier"
            );
        }
        assert!(!taxonomy
            .seed_applications()
            .iter()
            .any(|entry| entry.label() == "reference:browser"));
    }

    #[test]
    fn a_bundle_seed_with_an_unknown_category_is_rejected() {
        let file = br#"{
            "category_taxonomy_version": "test-1",
            "default_category": "UNLOGGED",
            "categories": ["FOCUS_WORK", "UNLOGGED"],
            "seed_applications": [
                {"app_name_pattern": "Editor", "label": "document:code", "category": "FOCUS_WORK"}
            ],
            "seed_bundles": [
                {"bundle_identifier": "com.example.editor", "label": "document:code", "category": "INVENTED"}
            ]
        }"#;

        assert_eq!(
            Taxonomy::from_json(file).err(),
            Some(super::TaxonomyError::InvalidSeedBundle)
        );
    }

    /// One application, two categories, is a mistake in the file. Catching it at
    /// load time beats resolving it by iteration order at classify time.
    #[test]
    fn a_bundle_declared_twice_is_rejected_even_across_the_two_declaration_sites() {
        let file = br#"{
            "category_taxonomy_version": "test-1",
            "default_category": "UNLOGGED",
            "categories": ["FOCUS_WORK", "REFERENCE", "UNLOGGED"],
            "seed_applications": [
                {"app_name_pattern": "Editor", "label": "document:code", "category": "FOCUS_WORK",
                 "bundle_identifier": "com.example.Editor"}
            ],
            "seed_bundles": [
                {"bundle_identifier": "com.example.editor", "label": "reference:read", "category": "REFERENCE"}
            ]
        }"#;

        assert_eq!(
            Taxonomy::from_json(file).err(),
            Some(super::TaxonomyError::InvalidSeedBundle)
        );
    }

    /// A file written before bundle identifiers existed must still load, and
    /// must produce no bundle seeds rather than an error.
    #[test]
    fn a_taxonomy_without_any_bundle_declarations_still_loads() {
        let file = br#"{
            "category_taxonomy_version": "test-1",
            "default_category": "UNLOGGED",
            "categories": ["FOCUS_WORK", "UNLOGGED"],
            "seed_applications": [
                {"app_name_pattern": "Editor", "label": "document:code", "category": "FOCUS_WORK"}
            ]
        }"#;

        let taxonomy = Taxonomy::from_json(file).expect("a pre-bundle taxonomy still loads");

        assert!(taxonomy.seed_bundles().is_empty());
    }
}
