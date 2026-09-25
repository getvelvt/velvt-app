use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;
use velvt_shared_types::RawEvent;

use super::{
    app_bundle_key_for,
    browser::focused_site_context,
    plugin::{
        BrowserContextPlugin, BundleSeedPlugin, DeclaredCategoryPlugin, DeclaredMetadata,
        DocumentTypePlugin, GenericBrowserPriorPlugin, LocalPurposeHeuristicPlugin,
        SeedDictionaryPlugin, UnloggedFallbackPlugin,
    },
    taxonomy::is_valid_label,
    AbstractionMappingStore, ClassificationConfidence, ClassificationPlugin, ClassificationResult,
    ClassificationSource, ClassificationStatus, ClassificationTier, MappingResolution, RawKey,
    StableKeySalt, StoreError, Taxonomy, TaxonomyError, TitleAbstractor,
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
    /// Identity of the application this event was classified under, so a later
    /// correction can be generalized to the app. The raw application name is
    /// discarded after abstraction and is not recoverable at correction time.
    #[serde(skip)]
    app_stable_id: String,
    /// Whether generalizing a correction to the whole app is meaningful. False
    /// for a browser window carrying a site context: one tab being focus work
    /// says nothing about the next.
    #[serde(skip)]
    app_scope_eligible: bool,
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
    pub fn app_stable_id(&self) -> &str {
        &self.app_stable_id
    }
    pub fn app_scope_eligible(&self) -> bool {
        self.app_scope_eligible
    }
}

pub struct AbstractionEngine {
    store: Arc<dyn AbstractionMappingStore>,
    taxonomy: Taxonomy,
    title_abstractor: Arc<dyn TitleAbstractor>,
    plugins: Vec<Box<dyn ClassificationPlugin>>,
    semantic_observer: Option<Arc<super::EmbeddingSimilarityPlugin>>,
    /// Read from `store` once, at build, so every key this engine computes is
    /// one the store's own rows can match (migration 0037).
    stable_key_salt: StableKeySalt,
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

    /// The bundle-scoped key for `bundle_id`, under this engine's salt.
    ///
    /// For the one caller that persists a bundle key the engine did not return:
    /// the router records it on the event row before `process` consumes the raw
    /// frame. Computing it here rather than with a salt of the router's own is
    /// what keeps the stored key and the engine's bundle rung the same key.
    pub fn app_bundle_key(&self, bundle_id: &str) -> String {
        app_bundle_key_for(&self.stable_key_salt, bundle_id)
    }

    pub fn process(&self, raw_event: RawEvent) -> Result<AbstractedEvent, AbstractionError> {
        let RawEvent {
            occurred_at,
            app_name,
            window_title,
            bundle_id,
            declared_app_category,
            document_type_ids,
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
        let stable_key = raw_key.stable_key(&self.stable_key_salt);
        if let Some(observer) = &self.semantic_observer {
            observer.observe(&stable_key, raw_key.app_name(), &classifier_context);
        }
        // Correction precedence, most specific first:
        //   1. this exact window              (`personal_override`)
        //   2. this application, by bundle id (`personal_app_override`)
        //   3. this application, by name      (`personal_app_override`)
        //   4. classifier plugins
        //
        // The app rung is what makes a correction stick. Without it a
        // correction binds to one (app, title) hash, so the next file opened
        // in the same editor is unclassified again and no amount of correcting
        // converges. A window-scoped correction still wins, so "all of Cursor
        // is work, except this one window" remains expressible.
        //
        // The bundle rung sits above the name rung because it is the identity
        // that survives a rename, a locale change and a marketing name that
        // differs from the reported one. Both rungs go through the same store
        // method, which resolves either identity — the two key domains cannot
        // collide, so one lookup cannot answer with the other's row. The name
        // rung stays and is consulted second so corrections recorded before
        // bundle identifiers existed keep working untouched.
        let app_stable_key = raw_key.app_stable_key(&self.stable_key_salt);
        let app_bundle_key = bundle_id
            .as_deref()
            .map(|bundle_id| self.app_bundle_key(bundle_id));
        let mut personal_override = self.store.personal_override(&stable_key)?;
        if personal_override.is_none() {
            if let Some(app_bundle_key) = &app_bundle_key {
                personal_override = self.store.personal_app_override(app_bundle_key)?;
            }
        }
        if personal_override.is_none() {
            personal_override = self.store.personal_app_override(&app_stable_key)?;
        }
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
            None => {
                // What the application declared about itself, unjudged. The
                // plugin order below is the arbitration: a tier that keys on
                // this metadata runs where its evidence deserves to rank, and
                // absent metadata makes every such tier abstain, leaving the
                // pre-metadata answer.
                let declared = DeclaredMetadata {
                    bundle_id: bundle_id.as_deref(),
                    declared_app_category: declared_app_category.as_deref(),
                    document_type_ids: &document_type_ids,
                };
                self.plugins
                    .iter()
                    .find_map(|plugin| {
                        plugin.classify_declared(raw_key.app_name(), &classifier_context, declared)
                    })
                    .ok_or(AbstractionError::NoPluginMatch)?
            }
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
            app_stable_id: app_stable_key,
            // A site context means the window's identity came from the page,
            // not the app, so the app tells us nothing about the next window.
            app_scope_eligible: focused_site.is_none(),
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
        // Reached by the bundle seed for `com.apple.AddressBook`: Contacts has
        // no name seed because `Contacts` is too generic a name to match on.
        "reference:contacts" => "Contacts",
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
        let bundles = self.taxonomy.seed_bundles();
        // Registration order IS the arbitration order: the engine takes the
        // first plugin that answers. It runs from the most specific identifier
        // to the least:
        //   browser site context  — the tab, for a browser window
        //   bundle seed           — the identifier the developer chose
        //   name seed            — the localized name macOS reports
        //   name/title heuristic  — curated keyword families
        //   declared document types — what the application says it opens
        //   declared App Store category — a whitelist of unambiguous values
        //   embedding             — Tier 2, when enabled
        //   generic browser prior — an explicitly ambiguous REFERENCE
        //   unlogged fallback     — captured, not classified
        // The two declared-metadata tiers sit exactly where
        // `ClassificationResult::precedence` ranks their sources, so plugin
        // order and explicit arbitration cannot disagree.
        let builder = self.register_plugin(BrowserContextPlugin::new(version.clone()));
        let builder = builder.register_plugin(BundleSeedPlugin::new(bundles, version.clone()));
        let builder = builder.register_plugin(SeedDictionaryPlugin::new(entries, version.clone()));
        let builder = builder.register_plugin(LocalPurposeHeuristicPlugin::new(version.clone()));
        let builder = builder.register_plugin(DocumentTypePlugin::new(version.clone()));
        let builder = builder.register_plugin(DeclaredCategoryPlugin::new(version.clone()));
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
        let stable_key_salt = self.store.stable_key_salt()?;
        Ok(AbstractionEngine {
            store: self.store,
            taxonomy: self.taxonomy,
            title_abstractor: self.title_abstractor,
            plugins: self.plugins,
            semantic_observer: self.semantic_observer,
            stable_key_salt,
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use std::sync::Arc;
    use uuid::Uuid;
    use velvt_shared_types::RawEvent;

    use crate::abstraction::{
        app_bundle_key_for, app_stable_key_for, stable_key_for, AbstractionEngine,
        AbstractionMappingStore, ClassificationSource, ClassificationTier, InMemoryMappingStore,
        PersonalOverride,
    };

    fn raw_event(app_name: &str, window_title: &str) -> RawEvent {
        RawEvent {
            event_id: Uuid::new_v4(),
            occurred_at: Utc.with_ymd_and_hms(2026, 9, 23, 9, 0, 0).unwrap(),
            duration_seconds: 60,
            app_name: app_name.to_owned(),
            window_title: window_title.to_owned(),
            bundle_id: None,
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: None,
        }
    }

    fn engine(store: Arc<InMemoryMappingStore>) -> AbstractionEngine {
        AbstractionEngine::from_builtin_taxonomy(store).expect("the shipped taxonomy loads")
    }

    /// The motivating case, end to end through the engine: macOS reports the
    /// editor as `Code`, which matches no seed pattern and no heuristic keyword,
    /// and the bundle identifier is what makes it focus work rather than
    /// UNLOGGED — and UNLOGGED is what `is_confident_evidence` excludes.
    #[test]
    fn the_bundle_identifier_classifies_the_editor_macos_calls_code() {
        let engine = engine(Arc::new(InMemoryMappingStore::default()));
        let mut event = raw_event("Code", "private project");
        event.bundle_id = Some("com.microsoft.VSCode".to_owned());

        let abstracted = engine.process(event).expect("the event abstracts");

        assert_eq!(abstracted.category(), "FOCUS_WORK");
        assert_eq!(
            abstracted.classification_tier(),
            ClassificationTier::ExactMatch
        );
        assert_eq!(
            abstracted.classification_source(),
            ClassificationSource::Seed
        );
    }

    /// Invariant 4 at the engine boundary. The same event without the metadata
    /// must classify exactly as it did before protocol v30 — UNLOGGED for the
    /// unreadable name, and unchanged for a name the taxonomy already knew.
    #[test]
    fn an_event_with_no_declared_metadata_classifies_exactly_as_before() {
        let engine = engine(Arc::new(InMemoryMappingStore::default()));

        let unreadable = engine
            .process(raw_event("Code", "private project"))
            .expect("the event abstracts");
        let seeded = engine
            .process(raw_event("VS Code", "private project"))
            .expect("the event abstracts");

        assert_eq!(unreadable.category(), "UNLOGGED");
        assert_eq!(unreadable.label(), "unlogged");
        assert_eq!(
            unreadable.classification_tier(),
            ClassificationTier::Fallback
        );
        assert_eq!(seeded.category(), "FOCUS_WORK");
        assert_eq!(seeded.classification_tier(), ClassificationTier::ExactMatch);
    }

    /// Declared document types decide when no seed knows the application, and
    /// the verdict is explicitly weaker than a seed's: Medium confidence, and
    /// attributed to the declaration it came from.
    #[test]
    fn declared_document_types_classify_an_application_no_seed_knows() {
        let engine = engine(Arc::new(InMemoryMappingStore::default()));
        let mut event = raw_event("Quillard", "private project");
        event.document_type_ids = vec![
            "public.source-code".to_owned(),
            "public.swift-source".to_owned(),
        ];

        let abstracted = engine.process(event).expect("the event abstracts");

        assert_eq!(abstracted.category(), "FOCUS_WORK");
        assert_eq!(
            abstracted.classification_source(),
            ClassificationSource::DeclaredDocumentTypes
        );
    }

    /// The whitelisted declared category is the last word before the embedding
    /// tier, and it only speaks when nothing more specific did.
    #[test]
    fn a_whitelisted_declared_category_classifies_when_nothing_else_does() {
        let engine = engine(Arc::new(InMemoryMappingStore::default()));
        let mut event = raw_event("Quillard", "private project");
        event.declared_app_category = Some("public.app-category.developer-tools".to_owned());

        let abstracted = engine.process(event).expect("the event abstracts");

        assert_eq!(abstracted.category(), "FOCUS_WORK");
        assert_eq!(
            abstracted.classification_source(),
            ClassificationSource::DeclaredAppCategory
        );
    }

    /// An excluded declared category must leave the event exactly where it was.
    /// Terminal declares `utilities` and is focus work; mapping that value to
    /// SYSTEM would misclassify the most-used focus application on the machine.
    #[test]
    fn an_excluded_declared_category_changes_nothing() {
        let engine = engine(Arc::new(InMemoryMappingStore::default()));
        let mut event = raw_event("Quillard", "private project");
        event.declared_app_category = Some("public.app-category.utilities".to_owned());

        let abstracted = engine.process(event).expect("the event abstracts");

        assert_eq!(abstracted.category(), "UNLOGGED");
    }

    /// The correction rungs, in order. The bundle identity outranks the name
    /// identity because it is the one that survives a rename; the window
    /// correction outranks both because naming one window is a more specific
    /// statement than naming the application.
    #[test]
    fn the_override_rungs_run_window_then_bundle_then_name() {
        let store = Arc::new(InMemoryMappingStore::default());
        let salt = store.stable_key_salt().unwrap();
        store.set_app_override(
            &app_stable_key_for(&salt, "Code"),
            PersonalOverride {
                category: "REFERENCE".to_owned(),
                local_activity_name: None,
            },
        );
        let engine = engine(Arc::clone(&store));
        let mut event = raw_event("Code", "private project");
        event.bundle_id = Some("com.microsoft.VSCode".to_owned());

        let name_rung = engine.process(event.clone()).expect("the event abstracts");

        store.set_app_override(
            &app_bundle_key_for(&salt, "com.microsoft.VSCode"),
            PersonalOverride {
                category: "TASK_MANAGEMENT".to_owned(),
                local_activity_name: None,
            },
        );
        let bundle_rung = engine.process(event.clone()).expect("the event abstracts");

        store.set_override(
            &stable_key_for(&salt, "Code", "private project"),
            PersonalOverride {
                category: "SOCIAL_FEED".to_owned(),
                local_activity_name: None,
            },
        );
        let window_rung = engine.process(event).expect("the event abstracts");

        assert_eq!(name_rung.category(), "REFERENCE");
        assert_eq!(bundle_rung.category(), "TASK_MANAGEMENT");
        assert_eq!(window_rung.category(), "SOCIAL_FEED");
        assert_eq!(
            window_rung.classification_source(),
            ClassificationSource::UserRule
        );
    }

    /// The engine keys under the salt of the store it looks keys up in, and
    /// under nothing else: a correction written under a different install's salt
    /// is a correction for some other Mac, and must not apply here.
    #[test]
    fn a_correction_keyed_under_another_salt_does_not_apply() {
        let store = Arc::new(InMemoryMappingStore::default());
        let foreign = crate::abstraction::StableKeySalt::from_bytes([7; 32]);
        store.set_app_override(
            &app_stable_key_for(&foreign, "Quillard"),
            PersonalOverride {
                category: "REFERENCE".to_owned(),
                local_activity_name: None,
            },
        );
        let engine = engine(Arc::clone(&store));

        let abstracted = engine
            .process(raw_event("Quillard", "private project"))
            .expect("the event abstracts");

        assert_ne!(
            abstracted.classification_source(),
            ClassificationSource::UserRule
        );
        assert_eq!(
            engine.app_bundle_key("com.example.quillard"),
            app_bundle_key_for(&store.stable_key_salt().unwrap(), "com.example.quillard")
        );
    }

    /// A correction recorded before bundle identifiers existed is keyed on the
    /// name alone. It must keep working for an event that now carries a bundle
    /// identifier, or the upgrade silently discards what the user taught.
    #[test]
    fn a_name_keyed_correction_still_applies_to_an_event_carrying_a_bundle_id() {
        let store = Arc::new(InMemoryMappingStore::default());
        let salt = store.stable_key_salt().unwrap();
        store.set_app_override(
            &app_stable_key_for(&salt, "Code"),
            PersonalOverride {
                category: "REFERENCE".to_owned(),
                local_activity_name: None,
            },
        );
        let engine = engine(Arc::clone(&store));
        let mut event = raw_event("Code", "private project");
        event.bundle_id = Some("com.microsoft.VSCode".to_owned());

        let abstracted = engine.process(event).expect("the event abstracts");

        assert_eq!(abstracted.category(), "REFERENCE");
        assert_eq!(
            abstracted.classification_source(),
            ClassificationSource::UserRule
        );
    }
}
