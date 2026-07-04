use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    time::Duration,
};

use super::SeedApplication;

/// Internal telemetry describing which classification tier produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationTier {
    ExactMatch,
    LocalPurposeHeuristic,
    EmbeddingSimilarity,
    Fallback,
}

/// Privacy-safe classification returned by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationResult {
    label: String,
    category: String,
    taxonomy_version: String,
    tier: ClassificationTier,
}

impl ClassificationResult {
    pub fn new(
        label: impl Into<String>,
        category: impl Into<String>,
        taxonomy_version: impl Into<String>,
        tier: ClassificationTier,
    ) -> Self {
        Self {
            label: label.into(),
            category: category.into(),
            taxonomy_version: taxonomy_version.into(),
            tier,
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
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        self.entries
            .iter()
            .find(|entry| {
                pattern_matches(entry.app_name_pattern(), app_name)
                    || title_pattern_matches(entry.app_name_pattern(), window_title)
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
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
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

fn title_pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern.contains('*') {
        return pattern_matches(pattern, value);
    }

    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    pattern.len() > 2 && value.contains(&pattern)
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
        PURPOSE_RULES
            .iter()
            .find(|rule| rule.matches(&haystack))
            .map(|rule| {
                ClassificationResult::new(
                    rule.label,
                    rule.category,
                    &self.taxonomy_version,
                    ClassificationTier::LocalPurposeHeuristic,
                )
            })
    }
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
            .any(|keyword| haystack.contains(keyword))
    }
}

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
        ],
        label: "design:visual",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "vscode",
            "visual studio",
            "xcode",
            "cursor",
            "zed",
            "sublime",
            "intellij",
            "pycharm",
            "webstorm",
            "clion",
            "android studio",
            "terminal",
            "iterm",
        ],
        label: "document:code",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &["mail", "gmail", "outlook", "superhuman", "spark"],
        label: "communication:email",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &["zoom", "meet", "teams", "webex", "around"],
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
        ],
        label: "communication:chat",
        category: "COMMUNICATION",
    },
    PurposeRule {
        keywords: &[
            "todo", "task", "asana", "linear", "jira", "trello", "clickup",
        ],
        label: "task:manage",
        category: "TASK_MANAGEMENT",
    },
    PurposeRule {
        keywords: &["youtube", "netflix", "tiktok", "twitch", "hulu"],
        label: "video:streaming",
        category: "PASSIVE_CONSUMPTION",
    },
    PurposeRule {
        keywords: &["spotify", "music", "podcast"],
        label: "audio:listen",
        category: "PASSIVE_CONSUMPTION",
    },
    PurposeRule {
        keywords: &["chatgpt", "claude", "perplexity", "copilot", "gemini"],
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
        ],
        label: "reference:read",
        category: "REFERENCE",
    },
];

fn normalized_purpose_input(app_name: &str, window_title: &str) -> String {
    let mut input = String::with_capacity(app_name.len() + window_title.len() + 1);
    input.push_str(app_name);
    input.push(' ');
    input.push_str(window_title);
    input
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) struct UnloggedFallbackPlugin {
    taxonomy_version: String,
}

impl UnloggedFallbackPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for UnloggedFallbackPlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        Some(ClassificationResult::new(
            "unlogged",
            "UNLOGGED",
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
    centroids: HashMap<String, Vec<f32>>,
    taxonomy_version: String,
    threshold: f32,
    timeout: Duration,
    metrics: Arc<EmbeddingMetrics>,
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
        if centroids.is_empty() || !(0.0..=1.0).contains(&threshold) {
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
            centroids,
            taxonomy_version: taxonomy_version.into(),
            threshold,
            timeout,
            metrics,
        })
    }
}

impl ClassificationPlugin for EmbeddingSimilarityPlugin {
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult> {
        let input = if window_title.is_empty() {
            app_name.to_owned()
        } else {
            format!("{app_name} [SEP] {window_title}")
        };
        let (response, receiver) = mpsc::sync_channel(1);
        if self
            .requests
            .try_send(InferenceRequest { input, response })
            .is_err()
        {
            return None;
        }
        let embedding = match receiver.recv_timeout(self.timeout) {
            Ok(InferenceOutcome::Embedding(embedding)) => embedding,
            Ok(InferenceOutcome::Failed) => return None,
            Ok(InferenceOutcome::TimedOut) | Err(_) => {
                self.metrics
                    .tier2_timeout_count
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(metric = "tier2_timeout_count", increment = 1_u64);
                return None;
            }
        };
        let (category, similarity) = self
            .centroids
            .iter()
            .filter_map(|(category, centroid)| {
                cosine_similarity(&embedding, centroid).map(|score| (category, score))
            })
            .max_by(|(_, left), (_, right)| left.total_cmp(right))?;
        // The threshold is inclusive so an offline-tuned boundary remains stable
        // after serialization. Tune it using labeled validation data, balancing
        // false-positive privacy risk against Tier 3 fallback frequency.
        (similarity >= self.threshold).then(|| {
            ClassificationResult::new(
                "document:inferred",
                category,
                &self.taxonomy_version,
                ClassificationTier::EmbeddingSimilarity,
            )
        })
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
    fn seed_dictionary_matches_window_title_for_browser_contexts() {
        let plugin = super::SeedDictionaryPlugin::new(
            vec![super::SeedApplication::new_for_test(
                "YouTube",
                "video:passive",
                "PASSIVE_CONSUMPTION",
            )],
            "mvp-1".to_owned(),
        );

        let result = plugin.classify("Google Chrome", "YouTube - Creator Studio");

        assert_eq!(
            result.map(|classification| classification.category().to_owned()),
            Some("PASSIVE_CONSUMPTION".to_owned())
        );
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
}
