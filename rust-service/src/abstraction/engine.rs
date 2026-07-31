use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;
use velvt_shared_types::RawEvent;

use super::{
    browser::focused_site_context,
    plugin::{
        BrowserContextPlugin, GenericBrowserPriorPlugin, LocalPurposeHeuristicPlugin,
        SeedDictionaryPlugin, UnloggedFallbackPlugin,
    },
    taxonomy::is_valid_label,
    AbstractionMappingStore, ClassificationConfidence, ClassificationPlugin, ClassificationResult,
    ClassificationSource, ClassificationStatus, ClassificationTier, MappingResolution, RawKey,
    StoreError, Taxonomy, TaxonomyError, TitleAbstractor,
};

/// Privacy-safe result. Raw fields cannot be constructed into or read from this type.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct AbstractedEvent {
    stable_id: String,
    label: String,
    category: String,
    taxonomy_version: String,
    occurred_at: DateTime<Utc>,
    #[serde(skip)]
    classification_tier: ClassificationTier,
    classification_status: ClassificationStatus,
    classification_confidence: ClassificationConfidence,
    classification_source: ClassificationSource,
    #[serde(skip)]
    local_display_label: Option<String>,
    #[serde(skip)]
    local_name_suggestion: Option<String>,
}

impl std::fmt::Debug for AbstractedEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AbstractedEvent")
            .field("stable_id", &self.stable_id)
            .field("label", &self.label)
            .field("category", &self.category)
            .field("taxonomy_version", &self.taxonomy_version)
            .field("occurred_at", &self.occurred_at)
            .field("classification_tier", &self.classification_tier)
            .field("classification_status", &self.classification_status)
            .field("classification_confidence", &self.classification_confidence)
            .field("classification_source", &self.classification_source)
            .field(
                "local_display_label",
                &self.local_display_label.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "local_name_suggestion",
                &self.local_name_suggestion.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
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
    pub fn classification_status(&self) -> ClassificationStatus {
        self.classification_status
    }
    pub fn classification_confidence(&self) -> ClassificationConfidence {
        self.classification_confidence
    }
    pub fn classification_source(&self) -> ClassificationSource {
        self.classification_source
    }
    pub fn local_display_label(&self) -> Option<&str> {
        self.local_display_label.as_deref()
    }
    pub fn local_name_suggestion(&self) -> Option<&str> {
        self.local_name_suggestion.as_deref()
    }
}

pub struct AbstractionEngine {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    title_abstractor: Arc<dyn TitleAbstractor>,
    plugins: Vec<Box<dyn ClassificationPlugin>>,
    semantic_observer: Option<Arc<super::EmbeddingSimilarityPlugin>>,
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
            semantic_observer: None,
        }
    }

    pub fn process(&self, raw_event: RawEvent) -> Result<AbstractedEvent, AbstractionError> {
        let RawEvent {
            occurred_at,
            app_name,
            window_title,
            focused_document_url,
            ..
        } = raw_event;
        let focused_site = focused_site_context(focused_document_url.as_deref());
        let stable_context = focused_site.as_deref().unwrap_or(&window_title).to_owned();
        let raw_key = RawKey::new(app_name, stable_context);
        let abstracted_title = self.title_abstractor.abstract_title(&window_title);
        let classifier_context = match (focused_site.as_deref(), abstracted_title.is_empty()) {
            (Some(site), false) => format!("{site} {abstracted_title}"),
            (Some(site), true) => site.to_owned(),
            (None, _) => abstracted_title.into_owned(),
        };
        let stable_key = raw_key.stable_key();
        if let Some(observer) = &self.semantic_observer {
            observer.observe(&stable_key, raw_key.app_name(), &classifier_context);
        }
        let personal_override = self.store.personal_override(&stable_key)?;
        let classification = match &personal_override {
            Some(personal_override) => ClassificationResult::with_quality(
                override_label_for_category(&personal_override.category)
                    .ok_or(AbstractionError::InvalidPluginResult)?,
                personal_override.category.clone(),
                self.taxonomy.version(),
                ClassificationTier::ExactMatch,
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                ClassificationSource::UserRule,
            ),
            None => self
                .plugins
                .iter()
                .find_map(|plugin| plugin.classify(raw_key.app_name(), &classifier_context))
                .ok_or(AbstractionError::NoPluginMatch)?,
        };
        if !is_valid_label(classification.label())
            || !self.taxonomy.contains_category(classification.category())
            || classification.taxonomy_version() != self.taxonomy.version()
            || matches_raw_input(classification.label(), &raw_key, &window_title)
        {
            return Err(AbstractionError::InvalidPluginResult);
        }
        let fresh_id = format!("abs_{}", Uuid::new_v4().simple());
        let local_display_label = personal_override
            .as_ref()
            .and_then(|personal_override| personal_override.local_activity_name.clone())
            .or_else(|| {
                curated_display_label(raw_key.app_name(), &window_title, classification.label())
            });
        let local_name_suggestion = personal_override
            .is_none()
            .then(|| responsible_local_name_suggestion(raw_key.app_name(), classification.source()))
            .flatten();
        let stable_id = self.store.resolve_id(MappingResolution {
            stable_key: &stable_key,
            fresh_id: &fresh_id,
            label: classification.label(),
            category: classification.category(),
            taxonomy_version: classification.taxonomy_version(),
            classification_tier: classification.tier().as_str(),
            classification_status: classification.status().as_str(),
            classification_confidence: classification.confidence().as_str(),
            classification_source: classification.source().as_str(),
            local_display_label: local_display_label.as_deref(),
        })?;
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
            classification_status: classification.status(),
            classification_confidence: classification.confidence(),
            classification_source: classification.source(),
            local_display_label,
            local_name_suggestion,
        })
    }
}

fn responsible_local_name_suggestion(
    app_name: &str,
    source: ClassificationSource,
) -> Option<String> {
    if source == ClassificationSource::Seed || source == ClassificationSource::UserRule {
        return None;
    }
    let trimmed = app_name.trim();
    let generic = [
        "unknown",
        "unknown app",
        "application",
        "app",
        "browser",
        "unclassifiable",
    ];
    if trimmed.is_empty()
        || trimmed.chars().count() > 48
        || trimmed.chars().any(char::is_control)
        || generic
            .iter()
            .any(|value| trimmed.eq_ignore_ascii_case(value))
    {
        return None;
    }
    Some(trimmed.to_owned())
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

fn matches_raw_input(label: &str, raw_key: &RawKey, original_window_title: &str) -> bool {
    label.eq_ignore_ascii_case(raw_key.app_name())
        || label.eq_ignore_ascii_case(raw_key.window_title())
        || label.eq_ignore_ascii_case(original_window_title)
}

fn curated_display_label(app_name: &str, window_title: &str, label: &str) -> Option<String> {
    let app = app_name.to_ascii_lowercase();
    let title = window_title.to_ascii_lowercase();
    let curated = match label {
        "communication:slack" => "Slack",
        "communication:gmail" => "Gmail",
        "communication:outlook" => "Outlook",
        "communication:calendar" => "Calendar",
        "communication:email" => "Email",
        "communication:chat" => "Chat",
        "meeting:meet" => "Google Meet",
        "meeting:zoom" => "Zoom",
        "meeting:teams" => "Microsoft Teams",
        "meeting:video" => "Video meeting",
        "reference:github" => "GitHub",
        "reference:gitlab" => "GitLab",
        "reference:stack_overflow" => "Stack Overflow",
        "reference:wikipedia" => "Wikipedia",
        "reference:mdn" => "MDN",
        "reference:read" => "Reading",
        "reference:ai_assistant" => "AI Assistant",
        "reference:browser" => "Browser",
        "document:docs" => "Docs",
        "document:sheets" | "document:spreadsheet" => "Spreadsheet",
        "document:slides" | "document:presentation" => "Presentation",
        "document:drive" => "Drive",
        "document:notion" => "Notion",
        "document:obsidian" => "Obsidian",
        "document:overleaf" => "Overleaf",
        "document:word" => "Word",
        "document:excel" => "Excel",
        "document:powerpoint" => "PowerPoint",
        "document:pages" => "Pages",
        "document:numbers" => "Numbers",
        "document:keynote" => "Keynote",
        "document:write" if title.contains("docs") => "Docs",
        "document:write" => "Writing",
        "document:edit" | "document:code"
            if app.contains("vs code") || app.contains("visual studio code") =>
        {
            "VS Code"
        }
        "document:edit" => "Document editing",
        "document:code" => "Coding",
        "video:youtube" => "YouTube",
        "video:netflix" => "Netflix",
        "video:tiktok" => "TikTok",
        "video:twitch" => "Twitch",
        "video:streaming" => "Streaming video",
        "audio:spotify" => "Spotify",
        "audio:music" | "audio:listen" => "Audio",
        "social:reddit" => "Reddit",
        "social:twitter" | "social:x" => "X",
        "social:instagram" => "Instagram",
        "social:facebook" => "Facebook",
        "social:threads" => "Threads",
        "social:linkedin" => "LinkedIn",
        "social:feed" => "Social feed",
        "task:manage" => "Task management",
        "task:finance" => "Finance task",
        "design:cad" => "CAD",
        "design:3d" => "3D design",
        "design:visual" => "Visual design",
        "creative:edit" => "Creative editing",
        "system:manage" => "System management",
        _ => return None,
    };
    Some(curated.to_owned())
}

pub struct AbstractionEngineBuilder {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    title_abstractor: Arc<dyn TitleAbstractor>,
    plugins: Vec<Box<dyn ClassificationPlugin>>,
    semantic_observer: Option<Arc<super::EmbeddingSimilarityPlugin>>,
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
            Some(plugin) => {
                let plugin = Arc::new(plugin);
                let mut builder = builder.register_plugin(Arc::clone(&plugin));
                builder.semantic_observer = Some(plugin);
                builder
            }
            None => builder,
        };
        builder
            .register_plugin(GenericBrowserPriorPlugin::new(version.clone()))
            .register_plugin(UnloggedFallbackPlugin::new(version, default_category))
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
            semantic_observer: self.semantic_observer,
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
