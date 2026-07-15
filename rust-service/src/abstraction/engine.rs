use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;
use velvt_shared_types::RawEvent;

use super::{
    plugin::{
        BrowserContextPlugin, LocalPurposeHeuristicPlugin, SeedDictionaryPlugin,
        UnloggedFallbackPlugin,
    },
    taxonomy::is_valid_label,
    AbstractionMappingStore, ClassificationPlugin, ClassificationResult, ClassificationTier,
    RawKey, StoreError, Taxonomy, TaxonomyError, TitleAbstractor,
};

/// Privacy-safe result. Raw fields cannot be constructed into or read from this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AbstractedEvent {
    stable_id: String,
    label: String,
    category: String,
    taxonomy_version: String,
    occurred_at: DateTime<Utc>,
    #[serde(skip)]
    classification_tier: ClassificationTier,
}

impl AbstractedEvent {
    pub fn stable_id(&self) -> &str {
        &self.stable_id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn category(&self) -> &str {
        &self.category
    }
    pub fn taxonomy_version(&self) -> &str {
        &self.taxonomy_version
    }
    pub fn occurred_at(&self) -> DateTime<Utc> {
        self.occurred_at
    }
    pub fn classification_tier(&self) -> ClassificationTier {
        self.classification_tier
    }
}

pub struct AbstractionEngine {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    title_abstractor: Arc<dyn TitleAbstractor>,
    plugins: Vec<Box<dyn ClassificationPlugin>>,
}

impl AbstractionEngine {
    pub fn from_builtin_taxonomy(
        store: Arc<dyn AbstractionMappingStore>,
    ) -> Result<Self, AbstractionError> {
        let taxonomy = Taxonomy::from_builtin()?;
        Self::builder(store, taxonomy)
            .register_builtin_plugins()
            .build()
    }

    pub fn builder(
        store: Arc<dyn AbstractionMappingStore>,
        taxonomy: Taxonomy,
    ) -> AbstractionEngineBuilder {
        AbstractionEngineBuilder {
            store,
            taxonomy,
            title_abstractor: Arc::new(super::DefaultTitleAbstractor),
            plugins: Vec::new(),
        }
    }

    pub fn process(&self, raw_event: RawEvent) -> Result<AbstractedEvent, AbstractionError> {
        let RawEvent {
            occurred_at,
            app_name,
            window_title,
            ..
        } = raw_event;
        let raw_key = RawKey::new(app_name, window_title);
        let abstracted_title = self.title_abstractor.abstract_title(raw_key.window_title());
        let stable_key = raw_key.stable_key();
        let classification = match self.store.personal_override(&stable_key)? {
            Some(category) => ClassificationResult::new(
                override_label_for_category(&category)
                    .ok_or(AbstractionError::InvalidPluginResult)?,
                category,
                self.taxonomy.version(),
                ClassificationTier::ExactMatch,
            ),
            None => self
                .plugins
                .iter()
                .find_map(|plugin| plugin.classify(raw_key.app_name(), abstracted_title.as_ref()))
                .ok_or(AbstractionError::NoPluginMatch)?,
        };
        if !is_valid_label(classification.label())
            || !self.taxonomy.contains_category(classification.category())
            || classification.taxonomy_version() != self.taxonomy.version()
            || matches_raw_input(classification.label(), &raw_key)
        {
            return Err(AbstractionError::InvalidPluginResult);
        }
        let fresh_id = format!("abs_{}", Uuid::new_v4().simple());
        let stable_id = self.store.resolve_id(
            &stable_key,
            &fresh_id,
            classification.label(),
            classification.category(),
            classification.taxonomy_version(),
            classification.tier().as_str(),
            raw_key.app_name(),
        )?;
        self.store.increment_classification_count(
            classification.taxonomy_version(),
            classification.tier().as_str(),
        )?;
        Ok(AbstractedEvent {
            stable_id,
            label: classification.label().to_owned(),
            category: classification.category().to_owned(),
            taxonomy_version: classification.taxonomy_version().to_owned(),
            occurred_at,
            classification_tier: classification.tier(),
        })
    }
}

pub(crate) fn override_label_for_category(category: &str) -> Option<&'static str> {
    match category {
        "FOCUS_WORK" => Some("document:inferred"),
        "PASSIVE_CONSUMPTION" => Some("video:inferred"),
        "SOCIAL_FEED" => Some("social:inferred"),
        "COMMUNICATION" => Some("communication:inferred"),
        "TASK_MANAGEMENT" => Some("task:inferred"),
        "REFERENCE" => Some("reference:inferred"),
        "SYSTEM" => Some("system:inferred"),
        "UNLOGGED" => Some("unlogged"),
        _ => None,
    }
}

fn matches_raw_input(label: &str, raw_key: &RawKey) -> bool {
    label.eq_ignore_ascii_case(raw_key.app_name())
        || label.eq_ignore_ascii_case(raw_key.window_title())
}

pub struct AbstractionEngineBuilder {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    title_abstractor: Arc<dyn TitleAbstractor>,
    plugins: Vec<Box<dyn ClassificationPlugin>>,
}

impl AbstractionEngineBuilder {
    pub fn register_plugin(mut self, plugin: impl ClassificationPlugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    pub fn title_abstractor(mut self, abstractor: impl TitleAbstractor + 'static) -> Self {
        self.title_abstractor = Arc::new(abstractor);
        self
    }

    pub fn register_builtin_plugins(self) -> Self {
        self.register_builtin_plugins_with_embedding(None)
    }

    pub fn register_builtin_plugins_with_embedding(
        self,
        embedding: Option<super::EmbeddingSimilarityPlugin>,
    ) -> Self {
        let version = self.taxonomy.version().to_owned();
        let default_category = self.taxonomy.default_category().to_owned();
        let entries = self.taxonomy.seed_applications();
        let builder = self.register_plugin(BrowserContextPlugin::new(version.clone()));
        let builder = builder.register_plugin(SeedDictionaryPlugin::new(entries, version.clone()));
        let builder = builder.register_plugin(LocalPurposeHeuristicPlugin::new(version.clone()));
        let builder = match embedding {
            Some(plugin) => builder.register_plugin(plugin),
            None => builder,
        };
        builder.register_plugin(UnloggedFallbackPlugin::new(version, default_category))
    }

    pub fn build(self) -> Result<AbstractionEngine, AbstractionError> {
        if self.plugins.is_empty() {
            return Err(AbstractionError::NoPlugins);
        }
        Ok(AbstractionEngine {
            store: self.store,
            taxonomy: self.taxonomy,
            title_abstractor: self.title_abstractor,
            plugins: self.plugins,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AbstractionError {
    #[error(transparent)]
    Taxonomy(#[from] TaxonomyError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no classification plugins are registered")]
    NoPlugins,
    #[error("no classification plugin matched the event")]
    NoPluginMatch,
    #[error("classification plugin returned an invalid privacy-safe result")]
    InvalidPluginResult,
}
