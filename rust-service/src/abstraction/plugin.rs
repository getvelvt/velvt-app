use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::Duration,
};
use velvt_shared_types::{ClassificationConfidence, ClassificationSource, ClassificationStatus};

use super::{
    normalize::{contains_token_phrase, normalize_classifier_input, normalize_classifier_text},
    SeedApplication,
};

/// Internal telemetry describing which classification tier produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationTier {
    ExactMatch,
    LocalPurposeHeuristic,
    EmbeddingSimilarity,
    Fallback,
}

impl ClassificationTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactMatch => "exact_match",
            Self::LocalPurposeHeuristic => "local_purpose_heuristic",
            Self::EmbeddingSimilarity => "embedding_similarity",
            Self::Fallback => "fallback",
        }
    }
}

/// Privacy-safe classification returned by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationResult {
    label: String,
    category: String,
    taxonomy_version: String,
    tier: ClassificationTier,
    status: ClassificationStatus,
    confidence: ClassificationConfidence,
    source: ClassificationSource,
}

impl ClassificationResult {
    pub fn new(
        label: impl Into<String>,
        category: impl Into<String>,
        taxonomy_version: impl Into<String>,
        tier: ClassificationTier,
    ) -> Self {
        let (status, confidence, source) = match tier {
            ClassificationTier::ExactMatch => (
                ClassificationStatus::Classified,
                ClassificationConfidence::High,
                ClassificationSource::Seed,
            ),
            ClassificationTier::LocalPurposeHeuristic => (
                ClassificationStatus::Classified,
                ClassificationConfidence::Medium,
                ClassificationSource::Heuristic,
            ),
            ClassificationTier::EmbeddingSimilarity => (
                ClassificationStatus::Classified,
                ClassificationConfidence::Medium,
                ClassificationSource::Embedding,
            ),
            ClassificationTier::Fallback => (
                ClassificationStatus::Unclassified,
                ClassificationConfidence::None,
                ClassificationSource::Fallback,
            ),
        };
        Self::with_quality(
            label,
            category,
            taxonomy_version,
            tier,
            status,
            confidence,
            source,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_quality(
        label: impl Into<String>,
        category: impl Into<String>,
        taxonomy_version: impl Into<String>,
        tier: ClassificationTier,
        status: ClassificationStatus,
        confidence: ClassificationConfidence,
        source: ClassificationSource,
    ) -> Self {
        Self {
            label: label.into(),
            category: category.into(),
            taxonomy_version: taxonomy_version.into(),
            tier,
            status,
            confidence,
            source,
        }
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

    pub fn tier(&self) -> ClassificationTier {
        self.tier
    }

    pub fn status(&self) -> ClassificationStatus {
        self.status
    }

    pub fn confidence(&self) -> ClassificationConfidence {
        self.confidence
    }

    pub fn source(&self) -> ClassificationSource {
        self.source
    }

    /// Explicit arbitration rank. Higher-specificity evidence always wins.
    pub fn precedence(&self) -> u8 {
        match (self.source, self.status) {
            (ClassificationSource::UserRule, _) => 6,
            (ClassificationSource::Seed, _) => 5,
            (ClassificationSource::Heuristic, _) => 4,
            (ClassificationSource::Embedding, _) => 3,
            (ClassificationSource::Fallback, ClassificationStatus::Ambiguous) => 2,
            (ClassificationSource::Fallback, _) => 1,
        }
    }
}

/// One independently registrable classification strategy.
pub trait ClassificationPlugin: Send + Sync {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult>;
}

pub(crate) struct SeedDictionaryPlugin {
    entries: Vec<SeedApplication>,
    taxonomy_version: String,
}

impl SeedDictionaryPlugin {
    pub(crate) fn new(entries: Vec<SeedApplication>, taxonomy_version: String) -> Self {
        Self {
            entries,
            taxonomy_version,
        }
    }
}

impl ClassificationPlugin for SeedDictionaryPlugin {
    fn classify(&self, app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        self.entries
            .iter()
            .find(|entry| {
                !is_browser_app(entry.app_name_pattern())
                    && pattern_matches(entry.app_name_pattern(), app_name)
            })
            .map(|entry| {
                ClassificationResult::new(
                    entry.label(),
                    entry.category(),
                    &self.taxonomy_version,
                    ClassificationTier::ExactMatch,
                )
            })
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern
        .split('*')
        .map(normalize_classifier_text)
        .collect::<Vec<_>>()
        .join("*");
    let value = normalize_classifier_text(value);
    let parts: Vec<_> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut remainder = value.as_str();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(position) = remainder.find(part) else {
            return false;
        };
        if index == 0 && !pattern.starts_with('*') && position != 0 {
            return false;
        }
        remainder = &remainder[position + part.len()..];
    }
    pattern.ends_with('*') || remainder.is_empty()
}

pub(crate) struct LocalPurposeHeuristicPlugin {
    taxonomy_version: String,
}

impl LocalPurposeHeuristicPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for LocalPurposeHeuristicPlugin {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        let haystack = normalized_purpose_input(app_name, window_title);
        classify_matching_rules(&haystack, PURPOSE_RULES, &self.taxonomy_version)
    }
}

pub(crate) struct BrowserContextPlugin {
    taxonomy_version: String,
}

impl BrowserContextPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for BrowserContextPlugin {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        if !is_browser_app(app_name) {
            return None;
        }
        let haystack = normalized_purpose_input(app_name, window_title);
        classify_matching_rules(&haystack, BROWSER_CONTEXT_RULES, &self.taxonomy_version)
    }
}

pub(crate) struct GenericBrowserPriorPlugin {
    taxonomy_version: String,
}

impl GenericBrowserPriorPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for GenericBrowserPriorPlugin {
    fn classify(&self, app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        is_browser_app(app_name).then(|| {
            ClassificationResult::with_quality(
                "reference:browser",
                "REFERENCE",
                &self.taxonomy_version,
                ClassificationTier::Fallback,
                ClassificationStatus::Ambiguous,
                ClassificationConfidence::Low,
                ClassificationSource::Fallback,
            )
        })
    }
}

fn is_browser_app(app_name: &str) -> bool {
    let app_name = normalize_classifier_text(app_name);
    [
        "safari",
        "google chrome",
        "chrome",
        "chromium",
        "arc",
        "firefox",
        "brave browser",
        "brave",
        "microsoft edge",
        "edge",
        "opera",
        "vivaldi",
        "orion",
        "dia",
    ]
    .iter()
    .any(|browser| app_name == *browser || contains_token_phrase(&app_name, browser))
}

struct PurposeRule {
    keywords: &'static [&'static str],
    label: &'static str,
    category: &'static str,
}

impl PurposeRule {
    fn matches(&self, haystack: &str) -> bool {
        self.keywords
            .iter()
            .any(|keyword| contains_token_phrase(haystack, keyword))
    }
}

fn classify_matching_rules(
    haystack: &str,
    rules: &[PurposeRule],
    taxonomy_version: &str,
) -> Option<ClassificationResult> {
    let mut matches = rules.iter().filter(|rule| rule.matches(haystack));
    let first = matches.next()?;
    if matches.any(|rule| rule.category != first.category) {
        return Some(ClassificationResult::with_quality(
            "unlogged",
            "UNLOGGED",
            taxonomy_version,
            ClassificationTier::LocalPurposeHeuristic,
            ClassificationStatus::Ambiguous,
            ClassificationConfidence::Low,
            ClassificationSource::Heuristic,
        ));
    }
    Some(ClassificationResult::new(
        first.label,
        first.category,
        taxonomy_version,
        ClassificationTier::LocalPurposeHeuristic,
    ))
}

const BROWSER_CONTEXT_RULES: &[PurposeRule] = &[
    PurposeRule {
        keywords: &["docs google com", "google docs", "docs"],
        label: "document:docs",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &["sheets google com", "google sheets", "sheets"],
        label: "document:sheets",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &["slides google com", "google slides", "slides"],
        label: "document:slides",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &["drive google com", "google drive"],
        label: "document:drive",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &["mail google com", "gmail", "inbox"],
        label: "communication:gmail",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &["calendar google com", "google calendar"],
        label: "communication:calendar",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &["meet google com", "google meet"],
        label: "meeting:meet",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &["youtube com", "youtu be", "youtube"],
        label: "video:youtube",
        category: "PASSIVE_CONSUMPTION",
    },
    PurposeRule {
        keywords: &["github com", "github"],
        label: "reference:github",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &["gitlab com", "gitlab"],
        label: "reference:gitlab",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &["stackoverflow com", "stack overflow"],
        label: "reference:stack_overflow",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &[
            "developer apple com",
            "docs rs",
            "rust docs",
            "python docs",
            "developer mozilla org",
            "react docs",
            "nextjs docs",
            "tailwind docs",
            "api reference",
        ],
        label: "reference:read",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &["linear app", "linear issue", "linear"],
        label: "task:manage",
        category: "TASK_MANAGEMENT",
    },
    PurposeRule {
        keywords: &["atlassian net", "jira"],
        label: "task:manage",
        category: "TASK_MANAGEMENT",
    },
    PurposeRule {
        keywords: &["notion so", "notion"],
        label: "document:write",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "chatgpt com",
            "chat openai com",
            "claude ai",
            "perplexity ai",
        ],
        label: "reference:ai_assistant",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &["reddit com", "reddit"],
        label: "social:reddit",
        category: "SOCIAL_FEED",
    },
    PurposeRule {
        keywords: &["x com", "twitter com", "twitter"],
        label: "social:x",
        category: "SOCIAL_FEED",
    },
    PurposeRule {
        keywords: &["instagram com", "instagram"],
        label: "social:instagram",
        category: "SOCIAL_FEED",
    },
    PurposeRule {
        keywords: &["linkedin com feed", "linkedin feed"],
        label: "social:linkedin",
        category: "SOCIAL_FEED",
    },
];

const PURPOSE_RULES: &[PurposeRule] = &[
    PurposeRule {
        keywords: &[
            "autodesk",
            "fusion",
            "fusion360",
            "solidworks",
            "onshape",
            "rhino",
            "rhinoceros",
            "sketchup",
            "freecad",
            "openscad",
            "cad",
            "revit",
            "archicad",
            "vectorworks",
            "creo",
            "catia",
            "inventor",
            "shapr3d",
            "plasticity",
        ],
        label: "design:cad",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "blender",
            "maya",
            "cinema4d",
            "houdini",
            "zbrush",
            "substance",
            "unity",
            "unreal",
            "prusa",
            "prusaslicer",
            "bambu",
            "orca slicer",
            "cura",
            "meshmixer",
        ],
        label: "design:3d",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "figma",
            "sketch",
            "adobe xd",
            "illustrator",
            "photoshop",
            "indesign",
            "canva",
            "affinity",
            "framer",
            "principle",
            "procreate",
        ],
        label: "design:visual",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "premiere",
            "after effects",
            "davinci",
            "resolve",
            "final cut",
            "capcut",
            "screenflow",
            "audition",
            "logic pro",
            "garageband",
            "ableton",
            "fl studio",
            "reaper",
            "descript",
        ],
        label: "creative:edit",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "vscode",
            "visual studio code",
            "code oss",
            "vscodium",
            "visual studio",
            "xcode",
            "cursor",
            "windsurf",
            "trae",
            "zed",
            "nova",
            "sublime",
            "intellij",
            "idea",
            "pycharm",
            "webstorm",
            "clion",
            "android studio",
            "terminal",
            "iterm",
            "warp",
            "ghostty",
            "alacritty",
            "wezterm",
            "kitty",
            "github desktop",
            "gitkraken",
            "fork",
            "docker",
            "postman",
            "insomnia",
            "tableplus",
            "datagrip",
            "sequel ace",
            "localhost",
            "pull request",
            "merge request",
            "stack trace",
            "api docs",
            "swagger",
        ],
        label: "document:code",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "word",
            "pages",
            "ulysses",
            "bear",
            "typora",
            "ia writer",
            "scrivener",
            "latex",
            "overleaf",
            "google docs",
            "docs google com",
        ],
        label: "document:write",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "excel",
            "numbers",
            "google sheets",
            "sheets google com",
            "spreadsheet",
            "airtable",
        ],
        label: "document:spreadsheet",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "powerpoint",
            "keynote",
            "google slides",
            "slides google com",
            "presentation",
            "pitch deck",
        ],
        label: "document:presentation",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "mail",
            "gmail",
            "outlook",
            "superhuman",
            "spark",
            "hey",
            "proton mail",
            "fastmail",
        ],
        label: "communication:email",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &[
            "zoom",
            "meet",
            "teams",
            "webex",
            "around",
            "facetime",
            "whereby",
            "tuple",
            "screen share",
            "video call",
        ],
        label: "meeting:video",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &[
            "slack",
            "discord",
            "telegram",
            "whatsapp",
            "messages",
            "messenger",
            "signal",
            "mattermost",
            "zulip",
            "wechat",
        ],
        label: "communication:chat",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &[
            "todo",
            "task",
            "asana",
            "linear",
            "jira",
            "trello",
            "clickup",
            "monday",
            "height",
            "notion task",
            "things",
            "omnifocus",
            "todoist",
        ],
        label: "task:manage",
        category: "TASK_MANAGEMENT",
    },
    PurposeRule {
        keywords: &[
            "quickbooks",
            "xero",
            "stripe",
            "bank",
            "invoice",
            "payroll",
            "expense",
            "budget",
        ],
        label: "task:finance",
        category: "TASK_MANAGEMENT",
    },
    PurposeRule {
        keywords: &[
            "youtube",
            "youtu be",
            "netflix",
            "tiktok",
            "twitch",
            "hulu",
            "disney",
            "prime video",
            "max",
            "peacock",
            "paramount",
        ],
        label: "video:streaming",
        category: "PASSIVE_CONSUMPTION",
    },
    PurposeRule {
        keywords: &[
            "spotify",
            "music",
            "podcast",
            "apple music",
            "soundcloud",
            "overcast",
            "pocket casts",
        ],
        label: "audio:listen",
        category: "PASSIVE_CONSUMPTION",
    },
    PurposeRule {
        keywords: &[
            "reddit",
            "twitter",
            "x com",
            "instagram",
            "facebook",
            "threads",
            "linkedin feed",
            "bsky",
            "bluesky",
            "mastodon",
        ],
        label: "social:feed",
        category: "SOCIAL_FEED",
    },
    PurposeRule {
        keywords: &[
            "chatgpt",
            "claude",
            "perplexity",
            "copilot",
            "gemini",
            "cursor chat",
            "poe",
            "mistral",
        ],
        label: "reference:ai_assistant",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &[
            "wikipedia",
            "docs",
            "developer",
            "stackoverflow",
            "stack overflow",
            "mdn",
            "readme",
            "manual",
            "reference",
            "documentation",
            "pdf",
            "preview",
            "acrobat",
            "coursera",
            "udemy",
            "edx",
            "khan academy",
            "blackboard",
            "canvas lms",
        ],
        label: "reference:read",
        category: "REFERENCE",
    },
    PurposeRule {
        keywords: &[
            "settings",
            "preferences",
            "activity monitor",
            "disk utility",
            "keychain",
            "finder",
            "installer",
            "software update",
        ],
        label: "system:manage",
        category: "SYSTEM",
    },
];

fn normalized_purpose_input(app_name: &str, window_title: &str) -> String {
    normalize_classifier_input(app_name, window_title)
}

pub(crate) struct UnloggedFallbackPlugin {
    taxonomy_version: String,
    default_category: String,
}

impl UnloggedFallbackPlugin {
    pub(crate) fn new(taxonomy_version: String, default_category: String) -> Self {
        Self {
            taxonomy_version,
            default_category,
        }
    }
}

impl ClassificationPlugin for UnloggedFallbackPlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        Some(ClassificationResult::new(
            "unlogged",
            &self.default_category,
            &self.taxonomy_version,
            ClassificationTier::Fallback,
        ))
    }
}

/// Local embedding implementation. The ONNX adapter is the production implementation.
pub trait EmbeddingModel: Send + Sync {
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError>;
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding inference unavailable")]
    Unavailable,
}

#[derive(Debug, Default)]
pub struct EmbeddingMetrics {
    tier2_timeout_count: AtomicU64,
}

impl EmbeddingMetrics {
    pub fn tier2_timeout_count(&self) -> u64 {
        self.tier2_timeout_count.load(Ordering::Relaxed)
    }
}

struct InferenceRequest {
    input: String,
    response: mpsc::SyncSender<InferenceOutcome>,
}

enum InferenceOutcome {
    Embedding(Vec<f32>),
    Failed,
    TimedOut,
}

/// Tier 2 classifier backed by a dedicated Tokio blocking worker.
pub struct EmbeddingSimilarityPlugin {
    requests: SyncSender<InferenceRequest>,
    prototypes: HashMap<String, Vec<Vec<f32>>>,
    taxonomy_version: String,
    threshold: f32,
    timeout: Duration,
    metrics: Arc<EmbeddingMetrics>,
    learning_store: Option<Arc<dyn super::SemanticLearningStore>>,
    observed: Mutex<HashMap<String, Vec<f32>>>,
    artifact_version: String,
}

impl EmbeddingSimilarityPlugin {
    pub fn new(
        model: Arc<dyn EmbeddingModel>,
        centroids: HashMap<String, Vec<f32>>,
        taxonomy_version: impl Into<String>,
        threshold: f32,
        timeout: Duration,
        metrics: Arc<EmbeddingMetrics>,
    ) -> Result<Self, EmbeddingError> {
        Self::new_with_prototypes(
            model,
            centroids
                .into_iter()
                .map(|(category, centroid)| (category, vec![centroid]))
                .collect(),
            taxonomy_version,
            threshold,
            timeout,
            metrics,
        )
    }

    pub fn new_with_prototypes(
        model: Arc<dyn EmbeddingModel>,
        prototypes: HashMap<String, Vec<Vec<f32>>>,
        taxonomy_version: impl Into<String>,
        threshold: f32,
        timeout: Duration,
        metrics: Arc<EmbeddingMetrics>,
    ) -> Result<Self, EmbeddingError> {
        if prototypes.is_empty()
            || prototypes.values().any(Vec::is_empty)
            || !(0.0..=1.0).contains(&threshold)
        {
            return Err(EmbeddingError::Unavailable);
        }
        let (requests, receiver) = mpsc::sync_channel::<InferenceRequest>(1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|_| EmbeddingError::Unavailable)?;
        std::thread::Builder::new()
            .name("velvt-tier2-inference".to_owned())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let model = Arc::clone(&model);
                    let input = request.input;
                    runtime.block_on(async {
                        let mut inference = tokio::task::spawn_blocking(move || {
                            // PRIVACY BOUNDARY: this is the only call site that passes the
                            // raw app name/title-derived inference string to external model code.
                            model.embed(&input)
                        });
                        let outcome = match tokio::time::timeout(timeout, &mut inference).await {
                            Ok(Ok(Ok(embedding))) => InferenceOutcome::Embedding(embedding),
                            Ok(_) => InferenceOutcome::Failed,
                            Err(_) => {
                                let _ = request.response.send(InferenceOutcome::TimedOut);
                                // Native ONNX calls cannot be cancelled safely. Waiting here
                                // keeps timed-out tasks bounded to one per inference worker.
                                let _ = inference.await;
                                return;
                            }
                        };
                        let _ = request.response.send(outcome);
                    });
                }
            })
            .map_err(|_| EmbeddingError::Unavailable)?;
        Ok(Self {
            requests,
            prototypes,
            taxonomy_version: taxonomy_version.into(),
            threshold,
            timeout,
            metrics,
            learning_store: None,
            observed: Mutex::new(HashMap::new()),
            artifact_version: "unversioned".into(),
        })
    }

    pub fn with_artifact_version(mut self, version: impl Into<String>) -> Self {
        self.artifact_version = version.into();
        self
    }

    pub fn builtin(taxonomy_version: impl Into<String>) -> Result<Self, EmbeddingError> {
        const ARTIFACT: &str = "builtin-hash-v1";
        let model = Arc::new(HashedEmbeddingModel);
        let phrases: [(&str, &[&str]); 7] = [
            (
                "FOCUS_WORK",
                &[
                    "programming code editor",
                    "developer terminal",
                    "writing document",
                    "design cad modeling",
                    "spreadsheet analysis",
                ],
            ),
            (
                "PASSIVE_CONSUMPTION",
                &[
                    "video streaming entertainment",
                    "music media player",
                    "television movies",
                ],
            ),
            (
                "SOCIAL_FEED",
                &["social feed community", "forum posts network"],
            ),
            (
                "COMMUNICATION",
                &[
                    "chat messaging conversation",
                    "email inbox mail",
                    "meeting video call",
                ],
            ),
            (
                "TASK_MANAGEMENT",
                &[
                    "task project planning",
                    "issue ticket tracker",
                    "calendar schedule",
                ],
            ),
            (
                "REFERENCE",
                &[
                    "documentation reference guide",
                    "search research encyclopedia",
                    "browser web article",
                    "ai assistant question",
                ],
            ),
            (
                "SYSTEM",
                &[
                    "system settings preferences",
                    "installer software update",
                    "file manager monitor",
                ],
            ),
        ];
        let mut prototypes = HashMap::new();
        for (category, examples) in phrases {
            let vectors = examples
                .iter()
                .map(|phrase| model.embed(phrase))
                .collect::<Result<Vec<_>, _>>()?;
            prototypes.insert(category.to_owned(), vectors);
        }
        Self::new_with_prototypes(
            model,
            prototypes,
            taxonomy_version,
            0.42,
            Duration::from_millis(20),
            Arc::new(EmbeddingMetrics::default()),
        )
        .map(|plugin| plugin.with_artifact_version(ARTIFACT))
    }

    pub fn with_learning_store(mut self, store: Arc<dyn super::SemanticLearningStore>) -> Self {
        self.learning_store = Some(store);
        self
    }

    pub fn observe(&self, key_hash: &str, app_name: &str, window_title: &str) {
        let input = embedding_input(app_name, window_title);
        let cached = self
            .learning_store
            .as_ref()
            .and_then(|store| store.embedding(key_hash).ok().flatten());
        let Some(embedding) = cached.or_else(|| self.infer(&input)) else {
            return;
        };
        if let Ok(mut observed) = self.observed.lock() {
            if observed.len() >= 64 {
                observed.clear();
            }
            observed.insert(input_hash(&input), embedding.clone());
        }
        if let Some(store) = &self.learning_store {
            let _ = store.record_embedding(key_hash, &embedding);
        }
    }

    fn infer(&self, input: &str) -> Option<Vec<f32>> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.requests
            .try_send(InferenceRequest {
                input: input.to_owned(),
                response,
            })
            .ok()?;
        match receiver.recv_timeout(self.timeout) {
            Ok(InferenceOutcome::Embedding(embedding)) => Some(embedding),
            Ok(InferenceOutcome::Failed) => None,
            Ok(InferenceOutcome::TimedOut) | Err(_) => {
                self.metrics
                    .tier2_timeout_count
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(metric = "tier2_timeout_count", increment = 1_u64);
                None
            }
        }
    }
}

impl<T: ClassificationPlugin + ?Sized> ClassificationPlugin for Arc<T> {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        (**self).classify(app_name, window_title)
    }
}

impl ClassificationPlugin for EmbeddingSimilarityPlugin {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        let input = embedding_input(app_name, window_title);
        let embedding = self
            .observed
            .lock()
            .ok()
            .and_then(|mut values| values.remove(&input_hash(&input)))
            .or_else(|| self.infer(&input))?;
        if let Some(store) = &self.learning_store {
            let _ = store.record_classifier_use(&self.artifact_version);
        }
        if let Some(result) = self.classify_personal(&embedding) {
            return Some(result);
        }
        let mut ranked = self
            .prototypes
            .iter()
            .filter_map(|(category, prototypes)| {
                prototypes
                    .iter()
                    .filter_map(|prototype| cosine_similarity(&embedding, prototype))
                    .max_by(f32::total_cmp)
                    .map(|score| (category, score))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_category, left), (right_category, right)| {
            right
                .total_cmp(left)
                .then_with(|| left_category.cmp(right_category))
        });
        let (category, similarity) = *ranked.first()?;
        // The threshold is inclusive so an offline-tuned boundary remains stable
        // after serialization. Tune it using labeled validation data, balancing
        // false-positive privacy risk against Tier 3 fallback frequency.
        if similarity < self.threshold {
            return None;
        }
        if ranked
            .get(1)
            .is_some_and(|(_, runner_up)| similarity - runner_up < 0.05)
        {
            return Some(ClassificationResult::with_quality(
                "unlogged",
                "UNLOGGED",
                &self.taxonomy_version,
                ClassificationTier::EmbeddingSimilarity,
                ClassificationStatus::Ambiguous,
                ClassificationConfidence::Low,
                ClassificationSource::Embedding,
            ));
        }
        Some(ClassificationResult::with_quality(
            inferred_label_for_category(category)?,
            category,
            &self.taxonomy_version,
            ClassificationTier::EmbeddingSimilarity,
            ClassificationStatus::Classified,
            if similarity >= 0.9 {
                ClassificationConfidence::High
            } else {
                ClassificationConfidence::Medium
            },
            ClassificationSource::Embedding,
        ))
    }
}

impl EmbeddingSimilarityPlugin {
    fn classify_personal(&self, embedding: &[f32]) -> Option<ClassificationResult> {
        let store = self.learning_store.as_ref()?;
        let mut ranked = store
            .personal_prototypes()
            .ok()?
            .into_iter()
            .filter_map(|prototype| {
                cosine_similarity(embedding, &prototype.embedding)
                    .map(|score| (prototype.category, score * prototype.weight))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let (category, score) = ranked.first()?;
        if *score < 0.90 {
            return None;
        }
        if ranked
            .get(1)
            .is_some_and(|runner| runner.0 != *category && *score - runner.1 < 0.08)
        {
            return Some(ClassificationResult::with_quality(
                "unlogged",
                "UNLOGGED",
                &self.taxonomy_version,
                ClassificationTier::EmbeddingSimilarity,
                ClassificationStatus::Ambiguous,
                ClassificationConfidence::Low,
                ClassificationSource::UserRule,
            ));
        }
        Some(ClassificationResult::with_quality(
            inferred_label_for_category(category)?,
            category,
            &self.taxonomy_version,
            ClassificationTier::EmbeddingSimilarity,
            ClassificationStatus::Classified,
            ClassificationConfidence::High,
            ClassificationSource::UserRule,
        ))
    }
}

fn embedding_input(app_name: &str, window_title: &str) -> String {
    let app_name = normalize_classifier_text(app_name);
    let window_title = normalize_classifier_text(window_title);
    if window_title.is_empty() {
        app_name
    } else {
        format!("{app_name} [SEP] {window_title}")
    }
}

fn input_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

#[derive(Debug, Clone, Copy)]
pub struct HashedEmbeddingModel;

impl EmbeddingModel for HashedEmbeddingModel {
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError> {
        use sha2::{Digest, Sha256};
        const DIMENSIONS: usize = 256;
        let bounded = input.chars().take(4096).collect::<String>();
        let normalized = normalize_classifier_text(&bounded);
        let mut vector = vec![0.0_f32; DIMENSIONS];
        for token in normalized.split_whitespace().take(128) {
            add_hashed_feature(&mut vector, token.as_bytes(), 1.0);
            let padded = format!("^{token}$");
            for trigram in padded.as_bytes().windows(3).take(32) {
                add_hashed_feature(&mut vector, trigram, 0.25);
            }
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(EmbeddingError::Unavailable);
        }
        for value in &mut vector {
            *value /= norm;
        }
        fn add_hashed_feature(vector: &mut [f32], feature: &[u8], weight: f32) {
            let digest = Sha256::digest(feature);
            let index = u16::from_le_bytes([digest[0], digest[1]]) as usize % vector.len();
            let sign = if digest[2] & 1 == 0 { 1.0 } else { -1.0 };
            vector[index] += sign * weight;
        }
        Ok(vector)
    }
}

fn inferred_label_for_category(category: &str) -> Option<&'static str> {
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

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    (left_norm > 0.0 && right_norm > 0.0).then(|| dot / (left_norm * right_norm))
}

#[cfg(test)]
mod tests {
    use crate::abstraction::plugin::ClassificationPlugin;

    use super::pattern_matches;

    #[test]
    fn patterns_support_exact_and_glob_matching() {
        assert!(pattern_matches("Twitter*", "Twitter/X"));
        assert!(pattern_matches("*Docs", "Google Docs"));
        assert!(!pattern_matches("Docs", "Google Docs"));
    }

    #[test]
    fn seed_dictionary_does_not_promote_raw_title_matches() {
        let plugin = super::SeedDictionaryPlugin::new(
            vec![super::SeedApplication::new_for_test(
                "YouTube",
                "video:passive",
                "PASSIVE_CONSUMPTION",
            )],
            "mvp-1".to_owned(),
        );

        let result = plugin.classify("Google Chrome", "YouTube - Creator Studio");

        assert!(result.is_none());
    }

    #[test]
    fn local_purpose_heuristic_classifies_cad_apps_without_seed_entry() {
        let plugin = super::LocalPurposeHeuristicPlugin::new("mvp-1".to_owned());

        let result = plugin
            .classify("Autodesk Fusion", "Untitled")
            .expect("cad app should classify locally");

        assert_eq!(result.label(), "design:cad");
        assert_eq!(result.category(), "FOCUS_WORK");
        assert_eq!(
            result.tier(),
            super::ClassificationTier::LocalPurposeHeuristic
        );
    }

    #[test]
    fn local_purpose_heuristic_classifies_unknown_app_families() {
        let plugin = super::LocalPurposeHeuristicPlugin::new("mvp-1".to_owned());
        let cases = [
            ("PrusaSlicer", "plate setup", "design:3d", "FOCUS_WORK"),
            (
                "Unknown",
                "Pitch deck - Google Slides",
                "document:presentation",
                "FOCUS_WORK",
            ),
            (
                "Unknown",
                "Invoice export - Stripe",
                "task:finance",
                "TASK_MANAGEMENT",
            ),
            (
                "Unknown",
                "Pull request review",
                "document:code",
                "FOCUS_WORK",
            ),
            (
                "Unknown",
                "Reddit - front page",
                "social:feed",
                "SOCIAL_FEED",
            ),
        ];

        for (app_name, window_title, expected_label, expected_category) in cases {
            let result = plugin
                .classify(app_name, window_title)
                .unwrap_or_else(|| panic!("{app_name} / {window_title} should classify locally"));
            assert_eq!(result.label(), expected_label);
            assert_eq!(result.category(), expected_category);
        }
    }

    #[test]
    fn collision_prone_keywords_require_complete_tokens() {
        let plugin = super::LocalPurposeHeuristicPlugin::new("mvp-1".to_owned());
        let unrelated = [
            "email parser",
            "meeting notes",
            "doctors portal",
            "password manager",
            "multitasking guide",
            "riverbank trail",
            "musical theater",
            "maximum effort",
            "xylophone lesson",
            "forklift operator",
            "ideal outcome",
            "pdfkit source",
            "settingsmanager",
            "preference pane",
        ];

        for title in unrelated {
            assert!(
                plugin.classify("Unknown App", title).is_none(),
                "collision-prone title misclassified: {title}"
            );
        }
    }

    #[test]
    fn seed_title_matching_requires_complete_tokens() {
        let plugin = super::SeedDictionaryPlugin::new(
            [
                ("Docs", "document:docs"),
                ("Word", "document:word"),
                ("Max", "video:max"),
                ("X", "social:x"),
                ("IDEA", "document:code"),
            ]
            .into_iter()
            .map(|(pattern, label)| {
                super::SeedApplication::new_for_test(pattern, label, "FOCUS_WORK")
            })
            .collect(),
            "mvp-1".to_owned(),
        );

        for title in [
            "Doctors portal",
            "Password reset",
            "Maximum effort",
            "Xylophone lesson",
            "Ideal outcome",
        ] {
            assert!(plugin.classify("Unknown App", title).is_none(), "{title}");
        }
    }

    #[test]
    fn fallback_uses_the_taxonomy_default_category() {
        let plugin = super::UnloggedFallbackPlugin::new("custom-1".to_owned(), "SYSTEM".to_owned());

        let result = plugin.classify("Unknown", "Unknown").unwrap();

        assert_eq!(result.category(), "SYSTEM");
        assert_eq!(result.label(), "unlogged");
    }

    #[test]
    fn browser_context_classifies_domain_like_tab_hints() {
        let plugin = super::BrowserContextPlugin::new("mvp-1".to_owned());
        let cases = [
            (
                "Google Chrome",
                "docs.google.com/document/d/abc",
                "document:docs",
                "FOCUS_WORK",
            ),
            (
                "Safari",
                "sheets.google.com/spreadsheets/d/abc",
                "document:sheets",
                "FOCUS_WORK",
            ),
            (
                "Arc",
                "slides.google.com/presentation/d/abc",
                "document:slides",
                "FOCUS_WORK",
            ),
            (
                "Brave Browser",
                "mail.google.com/mail/u/0/#inbox",
                "communication:gmail",
                "COMMUNICATION",
            ),
            (
                "Firefox",
                "youtube.com/watch?v=private",
                "video:youtube",
                "PASSIVE_CONSUMPTION",
            ),
            (
                "Chromium",
                "docs.google.com/document/d/abc",
                "document:docs",
                "FOCUS_WORK",
            ),
        ];

        for (app_name, window_title, expected_label, expected_category) in cases {
            let result = plugin
                .classify(app_name, window_title)
                .unwrap_or_else(|| panic!("{app_name} / {window_title} should classify locally"));
            assert_eq!(result.label(), expected_label);
            assert_eq!(result.category(), expected_category);
        }
    }

    #[test]
    fn browser_context_does_not_classify_non_browser_apps() {
        let plugin = super::BrowserContextPlugin::new("mvp-1".to_owned());

        assert!(plugin
            .classify("Slack", "docs.google.com/document/d/abc")
            .is_none());
    }
}
