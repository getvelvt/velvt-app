use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;
use velvt_shared_types::RawEvent;

use super::{
    plugin::{AppTitlePlugin, UnloggedPlugin},
    AbstractionMappingStore, AbstractionPlugin, RawKey, StoreError, Taxonomy, TaxonomyError,
};

/// Privacy-safe abstraction result consumable by IPC handlers and upload code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AbstractedEvent {
    pub stable_id: String,
    pub label: String,
    pub category: String,
    pub taxonomy_version: String,
    pub occurred_at: DateTime<Utc>,
}

/// On-device abstraction engine.
pub struct AbstractionEngine {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    plugins: Vec<Box<dyn AbstractionPlugin>>,
}

impl AbstractionEngine {
    /// Creates an engine using the built-in taxonomy and plugins.
    pub fn from_builtin_taxonomy(
        store: Arc<dyn AbstractionMappingStore>,
    ) -> Result<Self, AbstractionError> {
        let taxonomy = Taxonomy::from_builtin()?;
        Self::builder(store, taxonomy)
            .register_builtin_plugins()
            .build()
    }

    /// Starts constructing an engine with an explicitly loaded taxonomy.
    pub fn builder(
        store: Arc<dyn AbstractionMappingStore>,
        taxonomy: Taxonomy,
    ) -> AbstractionEngineBuilder {
        AbstractionEngineBuilder {
            store,
            taxonomy,
            plugins: Vec::new(),
        }
    }

    /// Converts one local-only raw event into a privacy-safe abstracted event.
    pub fn process(&self, raw_event: RawEvent) -> Result<AbstractedEvent, AbstractionError> {
        let RawEvent {
            occurred_at,
            app_name,
            window_title,
            ..
        } = raw_event;
        let raw_key = RawKey::new(app_name, window_title);
        let (label, _) = self
            .plugins
            .iter()
            .find_map(|plugin| plugin.classify(&raw_key))
            .ok_or(AbstractionError::NoPluginMatch)?;
        if !is_valid_label(&label) {
            return Err(AbstractionError::InvalidPluginLabel);
        }
        let stable_key = raw_key.stable_key();
        let fresh_id = format!("abs_{}", Uuid::new_v4().simple());
        let stable_id = self.store.resolve_id(&stable_key, &fresh_id)?;
        let category = self.taxonomy.category_for_label(&label).to_owned();

        Ok(AbstractedEvent {
            stable_id,
            label,
            category,
            taxonomy_version: self.taxonomy.version().to_owned(),
            occurred_at,
        })
    }
}

fn is_valid_label(label: &str) -> bool {
    if label.len() > 64 {
        return false;
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

/// Builder supporting plugin extension without changes to engine core.
pub struct AbstractionEngineBuilder {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    plugins: Vec<Box<dyn AbstractionPlugin>>,
}

impl AbstractionEngineBuilder {
    /// Registers one plugin. Plugins are executed by ascending priority.
    pub fn register_plugin(mut self, plugin: impl AbstractionPlugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Registers the built-in seed dictionary and fallback plugins.
    pub fn register_builtin_plugins(self) -> Self {
        let entries = self.taxonomy.seed_applications();
        self.register_plugin(AppTitlePlugin::new(entries))
            .register_plugin(UnloggedPlugin)
    }

    /// Validates and constructs the engine.
    pub fn build(mut self) -> Result<AbstractionEngine, AbstractionError> {
        if self.plugins.is_empty() {
            return Err(AbstractionError::NoPlugins);
        }
        self.plugins.sort_by_key(|plugin| plugin.priority());
        Ok(AbstractionEngine {
            store: self.store,
            taxonomy: self.taxonomy,
            plugins: self.plugins,
        })
    }
}

/// Privacy-safe abstraction failures.
#[derive(Debug, thiserror::Error)]
pub enum AbstractionError {
    #[error(transparent)]
    Taxonomy(#[from] TaxonomyError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no abstraction plugins are registered")]
    NoPlugins,
    #[error("no abstraction plugin matched the event")]
    NoPluginMatch,
    #[error("abstraction plugin returned an invalid label")]
    InvalidPluginLabel,
}
