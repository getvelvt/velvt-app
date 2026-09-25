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
    // Reached through the module rather than the crate-level re-export, which
    // this module does not need widened to name one type.
    taxonomy::SeedBundle,
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
    ///
    /// The two declared-metadata sources sit between the name heuristic and the
    /// embedding tier, and document types outrank the declared App Store
    /// category because they are far more specific: an application that says it
    /// opens `public.source-code` has told you what it is, while
    /// `productivity` covers four Velvt categories at once. The ranks below the
    /// insertion are unchanged, so no existing arbitration moves.
    pub fn precedence(&self) -> u8 {
        match (self.source, self.status) {
            (ClassificationSource::UserRule, _) => 8,
            (ClassificationSource::Seed, _) => 7,
            (ClassificationSource::Heuristic, _) => 6,
            (ClassificationSource::DeclaredDocumentTypes, _) => 5,
            (ClassificationSource::DeclaredAppCategory, _) => 4,
            (ClassificationSource::Embedding, _) => 3,
            (ClassificationSource::Fallback, ClassificationStatus::Ambiguous) => 2,
            (ClassificationSource::Fallback, _) => 1,
        }
    }
}

/// What the application says about itself, read out of its own `Info.plist` by
/// the client and passed through unjudged.
///
/// Facts, never conclusions: Swift reports the bundle identifier, the declared
/// App Store category and the declared document types exactly as the developer
/// wrote them, and every decision about what they mean is made here. All three
/// are device-local — they key corrections and drive the tiers below, and no
/// upload DTO has a field they could occupy.
///
/// Every field is optional or empty-able because every one of them can be
/// legitimately missing: an older client sends none of it, a sandbox denial
/// costs one of them, and a plist can simply omit the key. Absence must classify
/// exactly as it did before this type existed, which is why
/// [`ClassificationPlugin::classify_declared`] defaults to the metadata-free
/// path and the tiers that key on it return `None` rather than guessing.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeclaredMetadata<'a> {
    /// `CFBundleIdentifier`, e.g. `com.microsoft.VSCode`.
    pub bundle_id: Option<&'a str>,
    /// `LSApplicationCategoryType`, e.g. `public.app-category.developer-tools`.
    pub declared_app_category: Option<&'a str>,
    /// The `LSItemContentTypes` declared across `CFBundleDocumentTypes`,
    /// flattened, deduplicated and sorted by the client.
    pub document_type_ids: &'a [String],
}

/// One independently registrable classification strategy.
pub trait ClassificationPlugin: Send + Sync {
    /// Classify from the raw name and context alone.
    ///
    /// This is the whole of what a tier could see before declared metadata
    /// existed, and it stays the required method so that every existing tier is
    /// untouched by the addition.
    fn classify(&self, app_name: &str, window_title: &str) -> Option<ClassificationResult>;

    /// Classify with whatever the application declared about itself.
    ///
    /// The engine calls this for every registered plugin. The default ignores
    /// the metadata and answers exactly as [`Self::classify`] does, so a tier
    /// only overrides it if the metadata is the thing it keys on — and a client
    /// that reports no metadata gets the pre-v30 answer from every tier by
    /// construction rather than by each tier remembering to check.
    fn classify_declared(
        &self,
        app_name: &str,
        window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        let _ = declared;
        self.classify(app_name, window_title)
    }
}

/// Tier 1 keyed on the bundle identifier, ahead of the name seed.
///
/// A bundle identifier is a strictly stronger identifier than a displayed name:
/// the developer chose it once and changing it makes a different application,
/// while `NSRunningApplication.localizedName` is translated per locale, changes
/// between releases, and is often not the marketing name at all — for Visual
/// Studio Code it is `Code`, which matches no seed pattern and left the user's
/// editor `UNLOGGED`. Running before [`SeedDictionaryPlugin`] means the stronger
/// identifier decides when both have an answer.
pub(crate) struct BundleSeedPlugin {
    /// Keyed by lowercased bundle identifier. Launch Services compares bundle
    /// identifiers case-insensitively, so the lookup does too; the taxonomy
    /// loader has already rejected two entries that differ only in case.
    entries: HashMap<String, SeedBundle>,
    taxonomy_version: String,
}

impl BundleSeedPlugin {
    pub(crate) fn new(entries: Vec<SeedBundle>, taxonomy_version: String) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|entry| (entry.bundle_identifier().trim().to_ascii_lowercase(), entry))
                .collect(),
            taxonomy_version,
        }
    }
}

impl ClassificationPlugin for BundleSeedPlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        // No bundle identifier, nothing to key on. This is the path taken by
        // every event from a client that does not report one, and it is what
        // makes this tier invisible to those events.
        None
    }

    fn classify_declared(
        &self,
        app_name: &str,
        _window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        // Same refusal `SeedDictionaryPlugin` makes, for the same reason: a
        // browser window's identity is the tab, not the application, so
        // classifying the application here would answer the wrong question with
        // high confidence and pre-empt the browser tiers that answer the right
        // one. The shipped taxonomy seeds no browser; this keeps that true even
        // if a later editor adds one.
        if is_browser_app(app_name) {
            return None;
        }
        let bundle_id = declared.bundle_id?;
        let entry = self.entries.get(&bundle_id.trim().to_ascii_lowercase())?;
        Some(ClassificationResult::new(
            entry.label(),
            entry.category(),
            &self.taxonomy_version,
            ClassificationTier::ExactMatch,
        ))
    }
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

/// Tier 1.5: what the application says it opens (`CFBundleDocumentTypes`).
///
/// About half of installed applications declare the file types they handle, and
/// the declaration is far more specific than an App Store category: an
/// application that opens `public.source-code` has said what it is, while
/// `productivity` covers four Velvt categories at once. It costs nothing to
/// read and needs no TCC permission.
///
/// Runs after the name and bundle seeds — a seed is a curated statement about a
/// specific application and outranks an inference — and before
/// [`DeclaredCategoryPlugin`], which is the weaker of the two declarations.
pub(crate) struct DocumentTypePlugin {
    taxonomy_version: String,
}

impl DocumentTypePlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for DocumentTypePlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        None
    }

    fn classify_declared(
        &self,
        app_name: &str,
        _window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        // **The contract asks for no browser rule here. This tier adds one, and
        // this is the reason.**
        //
        // A browser declares the types it can render, not what this window is
        // showing. Chrome declares 17 document types covering images, video,
        // audio and text; they describe Chrome, and the same list is attached to
        // a spreadsheet tab and a film. Reading it would make every unrecognised
        // tab a Medium-confidence verdict where today it is an explicitly
        // ambiguous `reference:browser` prior — and because this tier runs
        // before the browser-context tier, it would pre-empt the one classifier
        // that looks at the tab, which is the only thing that can be right.
        //
        // This is the same judgement the name and bundle seeds already make
        // (`SeedDictionaryPlugin` refuses any browser pattern) and the same one
        // [`DeclaredCategoryPlugin`] makes below, so declared metadata never
        // decides a browser window at any tier. Removing the rule is a decision
        // about which evidence wins for a browser, not a cleanup: a verdict here
        // silences the tab. `document_types_are_ignored_for_browsers` locks it.
        if is_browser_app(app_name) {
            return None;
        }
        let category = unambiguous_document_type_category(declared.document_type_ids)?;
        Some(ClassificationResult::with_quality(
            inferred_label_for_category(category)?,
            category,
            &self.taxonomy_version,
            ClassificationTier::LocalPurposeHeuristic,
            ClassificationStatus::Classified,
            ClassificationConfidence::Medium,
            ClassificationSource::DeclaredDocumentTypes,
        ))
    }
}

/// The share of mapped document types one category must hold before the set is
/// allowed to mean anything, as a numerator over ten.
///
/// Applications declare mixed sets on purpose — an editor that also previews
/// images, a note-taker that imports PDFs — so a bare plurality says very
/// little. Below this share the honest answer is no answer: the next tier, or
/// ultimately `UNLOGGED`, is a better outcome than a category the declaration
/// does not actually support.
const UNAMBIGUOUS_DOCUMENT_TYPE_MAJORITY: u32 = 7;

/// The single category a declared type set implies, or `None`.
///
/// Types that map to nothing are not counted on either side: they are silence,
/// not disagreement, and putting them in the denominator would mean Xcode's 152
/// mostly-`com.apple.*` entries could veto the handful that are informative.
fn unambiguous_document_type_category(document_type_ids: &[String]) -> Option<&'static str> {
    let mut tally: Vec<(&'static str, u32)> = Vec::new();
    let mut mapped = 0_u32;
    for identifier in document_type_ids {
        let Some(category) = category_for_document_type(&identifier.trim().to_ascii_lowercase())
        else {
            continue;
        };
        mapped += 1;
        match tally.iter_mut().find(|(known, _)| *known == category) {
            Some((_, count)) => *count += 1,
            None => tally.push((category, 1)),
        }
    }
    if mapped == 0 {
        return None;
    }
    // Sorted by count, then by name, so a tie resolves the same way on every
    // run. At or above the majority share the leader is unique anyway; the
    // ordering matters only for the case just below it, which abstains.
    tally.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let (category, count) = *tally.first()?;
    (count * 10 >= mapped * UNAMBIGUOUS_DOCUMENT_TYPE_MAJORITY).then_some(category)
}

/// The Velvt category a single declared UTI implies, or `None` when it implies
/// nothing on its own.
///
/// The identifier arrives lowercased. Conformance is approximated by the
/// declared identifier itself rather than resolved through Uniform Type
/// Identifiers: resolving conformance means asking Launch Services about a type
/// that may not be registered on this machine, and the declared string is the
/// fact the application actually shipped.
fn category_for_document_type(identifier: &str) -> Option<&'static str> {
    match identifier {
        "public.source-code"
        | "public.shell-script"
        | "public.swift-source"
        | "public.python-script" => Some("FOCUS_WORK"),
        "public.plain-text"
        | "public.rtf"
        | "net.daringfireball.markdown"
        | "com.adobe.pdf"
        | "public.composite-content" => Some("REFERENCE"),
        "public.movie" | "public.audio" | "public.audiovisual-content" => {
            Some("PASSIVE_CONSUMPTION")
        }
        // `public.image` is deliberately unmapped. A design tool, a screenshot
        // utility and a photo viewer all claim it, and they are three different
        // categories; counting it would move the majority without adding
        // information.
        //
        // The whole `public.c-*` and `public.objective-c-*` families are source
        // code: header, source and the C++ spellings alike.
        _ if identifier.starts_with("public.c-")
            || identifier.starts_with("public.objective-c-") =>
        {
            Some("FOCUS_WORK")
        }
        _ => None,
    }
}

/// The last classifier before the embedding tier: the App Store category the
/// developer declared (`LSApplicationCategoryType`).
///
/// High recall — most foreground applications declare one — and poor precision,
/// so this is a whitelist and nothing else. See
/// [`DELIBERATELY_UNMAPPED_APP_CATEGORIES`] for what is left out and why.
pub(crate) struct DeclaredCategoryPlugin {
    taxonomy_version: String,
}

impl DeclaredCategoryPlugin {
    pub(crate) fn new(taxonomy_version: String) -> Self {
        Self { taxonomy_version }
    }
}

impl ClassificationPlugin for DeclaredCategoryPlugin {
    fn classify(&self, _app_name: &str, _window_title: &str) -> Option<ClassificationResult> {
        None
    }

    fn classify_declared(
        &self,
        app_name: &str,
        _window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        // The contract asks for no browser rule here either; see the long note
        // in [`DocumentTypePlugin::classify_declared`] for the reasoning, which
        // applies unchanged. In this tier the rule is also cheap: Safari
        // declares `productivity`, which the whitelist already refuses, and
        // Chrome declares nothing at all, so on the measured machine it changes
        // no verdict. It is here so that a later whitelist entry — `reference`,
        // say, which some browser could plausibly declare — cannot start
        // answering for a browser window without anyone deciding that it should.
        if is_browser_app(app_name) {
            return None;
        }
        let category = category_for_declared_app_category(declared.declared_app_category?)?;
        Some(ClassificationResult::with_quality(
            inferred_label_for_category(category)?,
            category,
            &self.taxonomy_version,
            ClassificationTier::LocalPurposeHeuristic,
            ClassificationStatus::Classified,
            ClassificationConfidence::Medium,
            ClassificationSource::DeclaredAppCategory,
        ))
    }
}

/// The whole of the declared-category mapping. Nothing outside this list has a
/// verdict.
///
/// Each value here maps to exactly one Velvt category for every application
/// measured that declares it. Adding a value is only safe after checking that
/// the applications declaring it agree — see
/// [`DELIBERATELY_UNMAPPED_APP_CATEGORIES`], which is the record of the ones
/// that do not.
fn category_for_declared_app_category(declared: &str) -> Option<&'static str> {
    match normalized_app_category(declared).as_str() {
        "developer-tools" => Some("FOCUS_WORK"),
        "video" | "music" | "entertainment" => Some("PASSIVE_CONSUMPTION"),
        "news" | "books" | "reference" | "education" => Some("REFERENCE"),
        _ => None,
    }
}

/// Values that are deliberately **not** mapped, each with the reason, so that a
/// later reader does not "complete" the mapping and destroy its precision. Ten
/// more mapped values would raise coverage and lower accuracy, which is the
/// wrong trade for a signal that feeds the drift gate.
///
/// The test below asserts every one of them still returns no verdict; the list
/// is therefore executable documentation rather than a comment that can rot.
///
/// Unused outside that test on purpose: the mapping above is the whole decision,
/// and a lookup in this list at classify time would only be a slower way of
/// returning `None`.
#[allow(dead_code)]
const DELIBERATELY_UNMAPPED_APP_CATEGORIES: &[(&str, &str)] = &[
    (
        "utilities",
        "mostly SYSTEM, but Terminal declares it and Terminal is focus work",
    ),
    (
        "productivity",
        "covers four Velvt categories at once: Pages is focus work, Mail is \
         communication, Reminders is task management, Safari is reference",
    ),
    (
        "social-networking",
        "polysemous exactly where it would do damage: Messages and WhatsApp both \
         declare it and both are COMMUNICATION, not a feed. A feed is drift; a \
         message often is not. Both are seeded by bundle identifier instead",
    ),
    (
        "business",
        "Slack declares it and Slack is communication; so do invoicing tools, \
         which are task management",
    ),
    (
        "graphics-design",
        "a design tool in use is focus work, an image viewer is not, and both \
         declare it",
    ),
    (
        "photography",
        "Photos is passive consumption and Lightroom is focus work",
    ),
    (
        "action-games",
        "every *-games value is left out: playing is not one Velvt category, and \
         no shipped category means 'leisure'",
    ),
    ("games", "as above"),
    (
        "finance",
        "reading a balance and doing payroll are different categories",
    ),
    (
        "medical",
        "no measured application, and guessing on an unmeasured value is how a \
         whitelist stops being one",
    ),
];

/// Accepts both `public.app-category.developer-tools` and a bare
/// `developer-tools`. Apple writes the full identifier; third-party plists are
/// not reliably that careful, and treating the two spellings alike keeps the
/// whitelist the only place a value is accepted or refused.
fn normalized_app_category(declared: &str) -> String {
    let declared = declared.trim().to_ascii_lowercase();
    declared
        .strip_prefix("public.app-category.")
        .unwrap_or(&declared)
        .to_owned()
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
    // The three Google Workspace editors are all served from `docs.google.com`
    // -- `sheets.google.com` and `slides.google.com` are redirect entry points,
    // not where a document lives -- and `focused_site_context` keeps the host
    // and drops the path, so in production every one of them reaches this list
    // as `docs google com` plus the tab title. What names the product is
    // therefore the title Google writes ("Q3 budget - Google Sheets"), which is
    // why the two product rules are title-led and sit *ahead* of the
    // `docs.google.com` rule that every Workspace tab also matches: first match
    // wins, so with the host rule first `document:sheets` and
    // `document:slides` were unreachable for a real spreadsheet or deck. All
    // three are FOCUS_WORK, so a tab with no readable title still lands in the
    // right category under `document:docs` rather than going unclassified, and
    // a Doc whose title happens to mention Sheets is a label away from right,
    // never UNLOGGED.
    //
    // The bare `sheets`/`slides` keywords are gone for the reason bare `docs`
    // is, below: a general word in a host-specific rule matches windows that
    // have nothing to do with the host.
    PurposeRule {
        keywords: &[
            "google sheets",
            "docs google com spreadsheets",
            "sheets google com",
        ],
        label: "document:sheets",
        category: "FOCUS_WORK",
    },
    PurposeRule {
        keywords: &[
            "google slides",
            "docs google com presentation",
            "slides google com",
        ],
        label: "document:slides",
        category: "FOCUS_WORK",
    },
    // A bare `docs` keyword is deliberately absent. `classify_matching_rules`
    // takes the first matching rule and returns UNLOGGED when any later rule
    // matches with a different category, so `docs` here made every
    // documentation subdomain ambiguous: `docs.github.com` matched this rule
    // and `github com` below and resolved to UNLOGGED, as did `docs.gitlab.com`,
    // `docs.rs`, and any github.com tab whose title contained the word (a pull
    // request called "docs: fix typo"). Real reference reading recorded as
    // unclassified, and the `docs rs` entry in `reference:read` could never
    // win. What that keyword was reaching for is covered by the host itself and
    // by "google docs" in the title.
    PurposeRule {
        keywords: &["docs google com", "google docs"],
        label: "document:docs",
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
    /// Which embedded input each persisted row was written from, by store key.
    ///
    /// See [`Self::observe`]: the persisted cache is addressed by a key the
    /// caller owns, and that key does not identify the string that was
    /// embedded. This is how a row is matched to the input that produced it
    /// before it is read back as that input's sketch.
    recorded_inputs: Mutex<HashMap<String, String>>,
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
            recorded_inputs: Mutex::new(HashMap::new()),
            artifact_version: "unversioned".into(),
        })
    }

    pub fn with_artifact_version(mut self, version: impl Into<String>) -> Self {
        self.artifact_version = version.into();
        self
    }

    /// The same classifier on the zero salt, for callers with no database.
    ///
    /// The service startup path does not call this. `main.rs` reads the device
    /// salt out of `embedding_salt` (migration 0031) through
    /// `AbstractionMapRepo::embedding_salt` and calls [`Self::builtin_salted`],
    /// and it disables Tier 2 rather than falling back here if that read fails.
    /// What is left for this entry point is tests and tooling that have no
    /// database to read a salt from, and any sketch it caches is recoverable
    /// offline exactly as described on [`EmbeddingSalt`] — which is what the
    /// warning below says, at the one remaining place it is true.
    pub fn builtin(taxonomy_version: impl Into<String>) -> Result<Self, EmbeddingError> {
        tracing::warn!(
            error_code = "embedding_salt_not_installed",
            "hashed embedding features are unsalted; cached sketches are recoverable from the source"
        );
        Self::builtin_salted(taxonomy_version, EmbeddingSalt::UNSALTED)
    }

    /// The same classifier, keyed to one device's salt.
    ///
    /// The seed prototypes are embedded with the same salted model as the
    /// observations they are compared against, so similarity, the threshold,
    /// and the ambiguity margin all behave exactly as they did unsalted. What
    /// changes is only which coordinates a word lands on.
    pub fn builtin_salted(
        taxonomy_version: impl Into<String>,
        salt: EmbeddingSalt,
    ) -> Result<Self, EmbeddingError> {
        const ARTIFACT: &str = "builtin-hash-v1";
        let model = Arc::new(HashedEmbeddingModel::new(salt));
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

    /// Records the sketch for one observed window, for Tier 2 and for the
    /// correction path to reuse.
    ///
    /// `key_hash` is the caller's stable key, which for a browser window is
    /// (application, site) -- deliberately one key for every page on that site,
    /// because `focused_site_context` drops the path. The string that gets
    /// embedded is not that: it carries the page title too. So the persisted
    /// row cannot be read back by key alone and treated as *this* window's
    /// sketch -- doing that classified a tab with whichever page of the site was
    /// observed last, which is a different page's embedding.
    ///
    /// The read is therefore keyed by what actually went into the embedding: a
    /// row is only reused when this process recorded it from this exact input.
    /// The write stays at the stable key because another reader addresses these
    /// rows by it -- `personal_semantic_prototype` is filled by joining
    /// `semantic_embedding_cache` against `abstraction_map.key_hash`, so a
    /// correction can only find a sketch stored under the stable key, and
    /// re-keying the write would silently end personal learning.
    ///
    /// Dropping the title from the embedded string was the other way to close
    /// this, and it is the worse one: the site is the only other thing the
    /// engine passes, so every tab on a host would embed identically and Tier 2
    /// would stop telling a spreadsheet from a video on the same domain at all.
    pub fn observe(&self, key_hash: &str, app_name: &str, window_title: &str) {
        let input = embedding_input(app_name, window_title);
        let input_hash = input_hash(&input);
        let row_holds_this_input = self
            .recorded_inputs
            .lock()
            .is_ok_and(|recorded| recorded.get(key_hash) == Some(&input_hash));
        let cached = row_holds_this_input
            .then_some(self.learning_store.as_ref())
            .flatten()
            .and_then(|store| store.embedding(key_hash).ok().flatten());
        let Some(embedding) = cached.or_else(|| self.infer(&input)) else {
            return;
        };
        if let Ok(mut observed) = self.observed.lock() {
            if observed.len() >= 64 {
                observed.clear();
            }
            observed.insert(input_hash.clone(), embedding.clone());
        }
        if let Some(store) = &self.learning_store {
            if store.record_embedding(key_hash, &embedding).is_ok() {
                if let Ok(mut recorded) = self.recorded_inputs.lock() {
                    // Bounded exactly as `observed` is, and for the same
                    // reason: a long-running service observes unboundedly many
                    // windows, and a forgotten row costs one inference, not a
                    // wrong answer.
                    if recorded.len() >= 64 {
                        recorded.clear();
                    }
                    recorded.insert(key_hash.to_owned(), input_hash);
                }
            }
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

    /// Forwarded rather than left to the default, which would otherwise route an
    /// `Arc`-wrapped plugin's declared-metadata call to its metadata-free
    /// method and silently discard the metadata for that one plugin.
    fn classify_declared(
        &self,
        app_name: &str,
        window_title: &str,
        declared: DeclaredMetadata<'_>,
    ) -> Option<ClassificationResult> {
        (**self).classify_declared(app_name, window_title, declared)
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
        // The nearest *disagreeing* candidate, which is not the same thing as
        // the second-ranked one. A category can hold several corrected
        // prototypes, so `ranked[1]` is often another prototype of the winning
        // category -- and when it is, inspecting only that slot skipped the
        // guard entirely and let a genuine near-tie further down the list
        // through as a High-confidence UserRule verdict. `ranked` is sorted by
        // score descending, so the first entry of a different category is the
        // best case that disagreement has.
        let contender = ranked
            .iter()
            .find(|(other_category, _)| other_category != category)
            .map(|(_, other_score)| *other_score);
        if contender.is_some_and(|other_score| *score - other_score < 0.08) {
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

/// The per-install key mixed into every hashed feature index.
///
/// Without it the sketches in `semantic_embedding_cache` are an oracle anyone
/// can compute offline. The hash family is described completely by `embed`
/// below, so a reader of this file can embed a dictionary word, look for its
/// coordinates in a stored row, and read back which words the window title
/// contained -- a verifier did exactly that against a live database and
/// recovered 1,190 distinct real words from 448 of 512 rows. Mixing in a value
/// that only this device holds means the enumeration has to be redone per
/// device, with that device's salt in hand.
///
/// It is not a secret from someone who already has the database file. The salt
/// lives beside the cache and has to, because the vectors must survive a
/// restart. What it removes is the source-only attack, which is the one the
/// sketch was open to. It never leaves the device: nothing in `upload::dto` has
/// a field it could occupy, and `Debug` below refuses to print it so a log line
/// cannot carry it out either.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingSalt([u8; Self::LENGTH]);

impl EmbeddingSalt {
    /// Matches `randomblob(32)` in migration 0031, which is what generates it.
    pub const LENGTH: usize = 32;

    /// The zero salt. Every reader of this file knows it, so it protects
    /// nothing -- it exists so that [`EmbeddingSimilarityPlugin::builtin`] has
    /// something to pass for callers with no database, and so that a caller
    /// cannot reach it by defaulting. The startup path does not use it.
    pub const UNSALTED: Self = Self([0; Self::LENGTH]);

    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl std::fmt::Debug for EmbeddingSalt {
    /// Redacted rather than derived. `HashedEmbeddingModel` derives `Debug`, so
    /// a derived salt would be one `?` format away from a tracing field, and a
    /// salt in a log is a salt in a crash report.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EmbeddingSalt(redacted)")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HashedEmbeddingModel {
    salt: EmbeddingSalt,
}

impl HashedEmbeddingModel {
    /// Deterministic for a fixed salt: the same input always produces the same
    /// vector, so cached sketches stay comparable with freshly computed ones
    /// across restarts. A different salt is a different vector space, which is
    /// why migration 0031 empties both stores that hold vectors.
    pub fn new(salt: EmbeddingSalt) -> Self {
        Self { salt }
    }
}

impl EmbeddingModel for HashedEmbeddingModel {
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError> {
        use sha2::{Digest, Sha256};
        const DIMENSIONS: usize = 256;
        let bounded = input.chars().take(4096).collect::<String>();
        let normalized = normalize_classifier_text(&bounded);
        let mut vector = vec![0.0_f32; DIMENSIONS];
        for token in normalized.split_whitespace().take(128) {
            add_hashed_feature(&mut vector, &self.salt, token.as_bytes(), 1.0);
            let padded = format!("^{token}$");
            for trigram in padded.as_bytes().windows(3).take(32) {
                add_hashed_feature(&mut vector, &self.salt, trigram, 0.25);
            }
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(EmbeddingError::Unavailable);
        }
        for value in &mut vector {
            *value /= norm;
        }
        fn add_hashed_feature(
            vector: &mut [f32],
            salt: &EmbeddingSalt,
            feature: &[u8],
            weight: f32,
        ) {
            // The salt goes in first so that changing it changes the index and
            // the sign of every feature, not a tail of the digest that the
            // index and sign are not read from.
            let digest = Sha256::new()
                .chain_update(salt.as_bytes())
                .chain_update(feature)
                .finalize();
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
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };

    use crate::abstraction::{
        plugin::ClassificationPlugin, PersonalSemanticPrototype, SemanticLearningStore, StoreError,
        Taxonomy,
    };
    use velvt_shared_types::{
        ClassificationConfidence, ClassificationSource, ClassificationStatus,
    };

    use super::{pattern_matches, DeclaredMetadata};

    fn document_types(identifiers: &[&str]) -> Vec<String> {
        identifiers
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    }

    fn bundle_seed_plugin() -> super::BundleSeedPlugin {
        let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");
        super::BundleSeedPlugin::new(taxonomy.seed_bundles(), taxonomy.version().to_owned())
    }

    /// The case the whole of Classification v2 exists for. `localizedName` for
    /// Visual Studio Code is `Code`, which matches no seed pattern and no
    /// heuristic keyword, so before the bundle tier the user's editor was
    /// UNLOGGED — and UNLOGGED is excluded from `is_confident_evidence`, so it
    /// was invisible to the drift gate as well.
    #[test]
    fn the_bundle_seed_classifies_the_editor_macos_calls_code() {
        let plugin = bundle_seed_plugin();

        let result = plugin
            .classify_declared(
                "Code",
                "main.rs",
                DeclaredMetadata {
                    bundle_id: Some("com.microsoft.VSCode"),
                    ..DeclaredMetadata::default()
                },
            )
            .expect("the editor classifies by its bundle identifier");

        assert_eq!(result.category(), "FOCUS_WORK");
        assert_eq!(result.tier(), super::ClassificationTier::ExactMatch);
        assert_eq!(result.source(), ClassificationSource::Seed);
        assert_eq!(result.confidence(), ClassificationConfidence::High);
    }

    /// A name Velvt cannot read for any reason — a translation, an accented
    /// spelling `normalize_classifier_text` destroys rather than folds, a
    /// rename — is no obstacle once the identifier is the key.
    #[test]
    fn the_bundle_seed_ignores_whatever_name_the_system_reports() {
        let plugin = bundle_seed_plugin();

        for reported_name in ["Code", "Código", "Visual Studio Code — Insiders", ""] {
            let result = plugin
                .classify_declared(
                    reported_name,
                    "",
                    DeclaredMetadata {
                        bundle_id: Some("com.microsoft.VSCode"),
                        ..DeclaredMetadata::default()
                    },
                )
                .unwrap_or_else(|| panic!("`{reported_name}` should classify by bundle id"));

            assert_eq!(result.category(), "FOCUS_WORK");
        }
    }

    /// Launch Services compares bundle identifiers case-insensitively, and a
    /// plist is written by hand.
    #[test]
    fn the_bundle_seed_matches_case_insensitively() {
        let plugin = bundle_seed_plugin();

        let result = plugin.classify_declared(
            "Code",
            "",
            DeclaredMetadata {
                bundle_id: Some("COM.MICROSOFT.VSCODE"),
                ..DeclaredMetadata::default()
            },
        );

        assert_eq!(
            result.map(|found| found.category().to_owned()),
            Some("FOCUS_WORK".to_owned())
        );
    }

    /// An unknown identifier is not a near miss to be guessed at.
    #[test]
    fn the_bundle_seed_abstains_on_an_unknown_identifier() {
        let plugin = bundle_seed_plugin();

        assert!(plugin
            .classify_declared(
                "Some App",
                "",
                DeclaredMetadata {
                    bundle_id: Some("com.example.unknown"),
                    ..DeclaredMetadata::default()
                },
            )
            .is_none());
    }

    /// The identity of a browser window is its tab. Classifying the browser
    /// itself here would pre-empt the tiers that read the tab.
    #[test]
    fn the_bundle_seed_refuses_browsers() {
        let plugin = super::BundleSeedPlugin::new(
            vec![super::SeedBundle::new_for_test(
                "com.apple.Safari",
                "reference:browser",
                "REFERENCE",
            )],
            "mvp-2".to_owned(),
        );

        assert!(plugin
            .classify_declared(
                "Safari",
                "",
                DeclaredMetadata {
                    bundle_id: Some("com.apple.Safari"),
                    ..DeclaredMetadata::default()
                },
            )
            .is_none());
    }

    #[test]
    fn document_types_classify_an_unseeded_editor() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());

        let result = plugin
            .classify_declared(
                "Unknown Editor",
                "private title",
                DeclaredMetadata {
                    document_type_ids: &document_types(&[
                        "public.c-source",
                        "public.objective-c-source",
                        "public.shell-script",
                        "public.source-code",
                        "public.swift-source",
                    ]),
                    ..DeclaredMetadata::default()
                },
            )
            .expect("an application that opens source code is doing focus work");

        assert_eq!(result.category(), "FOCUS_WORK");
        assert_eq!(result.label(), "document:inferred");
        assert_eq!(
            result.tier(),
            super::ClassificationTier::LocalPurposeHeuristic
        );
        assert_eq!(result.status(), ClassificationStatus::Classified);
        assert_eq!(result.confidence(), ClassificationConfidence::Medium);
        assert_eq!(result.source(), ClassificationSource::DeclaredDocumentTypes);
    }

    /// Declared types that disagree are not evidence. Half source code and half
    /// video is an application Velvt cannot read from its plist, and the next
    /// tier deserves the chance the guess would have taken.
    #[test]
    fn document_types_that_disagree_produce_no_verdict() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());

        assert!(plugin
            .classify_declared(
                "Unknown",
                "",
                DeclaredMetadata {
                    document_type_ids: &document_types(&[
                        "public.audio",
                        "public.movie",
                        "public.source-code",
                        "public.swift-source",
                    ]),
                    ..DeclaredMetadata::default()
                },
            )
            .is_none());
    }

    /// The majority rule is a share of the *mapped* types, and the boundary is
    /// inclusive: seven in ten decides, six in ten abstains.
    #[test]
    fn the_document_type_majority_boundary_is_seventy_percent_inclusive() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());
        let seven_of_ten = document_types(&[
            "public.c-header",
            "public.c-source",
            "public.objective-c-source",
            "public.python-script",
            "public.shell-script",
            "public.source-code",
            "public.swift-source",
            "com.adobe.pdf",
            "public.plain-text",
            "public.rtf",
        ]);
        let six_of_ten = document_types(&[
            "public.c-header",
            "public.c-source",
            "public.objective-c-source",
            "public.python-script",
            "public.shell-script",
            "public.source-code",
            "com.adobe.pdf",
            "public.composite-content",
            "public.plain-text",
            "public.rtf",
        ]);

        let decided = plugin.classify_declared(
            "Unknown",
            "",
            DeclaredMetadata {
                document_type_ids: &seven_of_ten,
                ..DeclaredMetadata::default()
            },
        );
        let abstained = plugin.classify_declared(
            "Unknown",
            "",
            DeclaredMetadata {
                document_type_ids: &six_of_ten,
                ..DeclaredMetadata::default()
            },
        );

        assert_eq!(
            decided.map(|result| result.category().to_owned()),
            Some("FOCUS_WORK".to_owned())
        );
        assert!(abstained.is_none());
    }

    /// Unmapped types are silence, not disagreement. Xcode declares 152 types,
    /// almost all of them `com.apple.*` project files that mean nothing here;
    /// counting them in the denominator would let them veto the ones that do.
    #[test]
    fn unmapped_document_types_neither_decide_nor_veto() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());
        let mut declared = document_types(&["public.source-code", "public.swift-source"]);
        declared.extend((0..150).map(|index| format!("com.apple.xcode.project-{index}")));

        let result = plugin.classify_declared(
            "Unknown",
            "",
            DeclaredMetadata {
                document_type_ids: &declared,
                ..DeclaredMetadata::default()
            },
        );

        assert_eq!(
            result.map(|found| found.category().to_owned()),
            Some("FOCUS_WORK".to_owned())
        );
    }

    /// `public.image` is claimed by a design tool, a screenshot utility and a
    /// photo viewer alike, so on its own it decides nothing.
    #[test]
    fn images_alone_produce_no_verdict() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());

        assert!(plugin
            .classify_declared(
                "Unknown",
                "",
                DeclaredMetadata {
                    document_type_ids: &document_types(&[
                        "public.image",
                        "public.jpeg",
                        "public.png"
                    ]),
                    ..DeclaredMetadata::default()
                },
            )
            .is_none());
    }

    /// A browser declares what it can render, not what this window is showing.
    #[test]
    fn document_types_are_ignored_for_browsers() {
        let plugin = super::DocumentTypePlugin::new("mvp-2".to_owned());

        assert!(plugin
            .classify_declared(
                "Google Chrome",
                "",
                DeclaredMetadata {
                    document_type_ids: &document_types(&["public.movie", "public.audio"]),
                    ..DeclaredMetadata::default()
                },
            )
            .is_none());
    }

    #[test]
    fn the_declared_category_whitelist_classifies_its_unambiguous_values() {
        let plugin = super::DeclaredCategoryPlugin::new("mvp-2".to_owned());
        let cases = [
            ("public.app-category.developer-tools", "FOCUS_WORK"),
            ("public.app-category.video", "PASSIVE_CONSUMPTION"),
            ("public.app-category.music", "PASSIVE_CONSUMPTION"),
            ("public.app-category.entertainment", "PASSIVE_CONSUMPTION"),
            ("public.app-category.news", "REFERENCE"),
            ("public.app-category.books", "REFERENCE"),
            ("public.app-category.reference", "REFERENCE"),
            ("public.app-category.education", "REFERENCE"),
        ];

        for (declared, expected) in cases {
            let result = plugin
                .classify_declared(
                    "Unknown",
                    "",
                    DeclaredMetadata {
                        declared_app_category: Some(declared),
                        ..DeclaredMetadata::default()
                    },
                )
                .unwrap_or_else(|| panic!("{declared} is on the whitelist"));

            assert_eq!(result.category(), expected, "{declared}");
            assert_eq!(result.confidence(), ClassificationConfidence::Medium);
            assert_eq!(result.source(), ClassificationSource::DeclaredAppCategory);
        }
    }

    /// Declared metadata never decides a browser window, at either new tier: a
    /// verdict about the application would silence the tab, which is the only
    /// evidence that says what the window is. Asserted with a whitelisted value
    /// that *would* classify any other application, so the test fails if the
    /// rule is removed rather than passing because nothing matched.
    #[test]
    fn the_declared_category_is_ignored_for_browsers() {
        let plugin = super::DeclaredCategoryPlugin::new("mvp-2".to_owned());

        for browser in ["Safari", "Google Chrome", "Arc", "Firefox"] {
            assert!(
                plugin
                    .classify_declared(
                        browser,
                        "",
                        DeclaredMetadata {
                            declared_app_category: Some("public.app-category.reference"),
                            ..DeclaredMetadata::default()
                        },
                    )
                    .is_none(),
                "{browser} must fall through to the tiers that read the tab"
            );
        }

        assert!(plugin
            .classify_declared(
                "Dictionary",
                "",
                DeclaredMetadata {
                    declared_app_category: Some("public.app-category.reference"),
                    ..DeclaredMetadata::default()
                },
            )
            .is_some());
    }

    /// The exclusions are the precision of this tier. `utilities` and
    /// `productivity` are the two that most invite a mapping and the two that
    /// most punish one: Terminal declares `utilities` and is focus work, and
    /// `productivity` covers four Velvt categories at once.
    #[test]
    fn every_deliberately_unmapped_declared_category_abstains() {
        let plugin = super::DeclaredCategoryPlugin::new("mvp-2".to_owned());

        for (value, reason) in super::DELIBERATELY_UNMAPPED_APP_CATEGORIES {
            assert!(!reason.is_empty(), "{value} is excluded without a reason");
            for spelling in [(*value).to_owned(), format!("public.app-category.{value}")] {
                assert!(
                    plugin
                        .classify_declared(
                            "Unknown",
                            "",
                            DeclaredMetadata {
                                declared_app_category: Some(spelling.as_str()),
                                ..DeclaredMetadata::default()
                            },
                        )
                        .is_none(),
                    "{spelling} must not classify: {reason}"
                );
            }
        }
    }

    /// An unmeasured value is not a value to guess at; the whitelist is the
    /// whole of the mapping.
    #[test]
    fn an_unrecognised_declared_category_abstains() {
        let plugin = super::DeclaredCategoryPlugin::new("mvp-2".to_owned());

        for declared in [
            "public.app-category.weather",
            "public.app-category.magazines-newspapers",
            "com.example.invented",
            "",
        ] {
            assert!(plugin
                .classify_declared(
                    "Unknown",
                    "",
                    DeclaredMetadata {
                        declared_app_category: Some(declared),
                        ..DeclaredMetadata::default()
                    },
                )
                .is_none());
        }
    }

    /// Invariant 4, stated at the tier boundary: with no declared metadata every
    /// plugin — the three new ones included — answers exactly what it answered
    /// before declared metadata existed.
    #[test]
    fn absent_metadata_leaves_every_tier_answering_exactly_as_before() {
        let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");
        let version = taxonomy.version().to_owned();
        let plugins: Vec<Box<dyn ClassificationPlugin>> = vec![
            Box::new(super::BrowserContextPlugin::new(version.clone())),
            Box::new(super::BundleSeedPlugin::new(
                taxonomy.seed_bundles(),
                version.clone(),
            )),
            Box::new(super::SeedDictionaryPlugin::new(
                taxonomy.seed_applications(),
                version.clone(),
            )),
            Box::new(super::LocalPurposeHeuristicPlugin::new(version.clone())),
            Box::new(super::DocumentTypePlugin::new(version.clone())),
            Box::new(super::DeclaredCategoryPlugin::new(version.clone())),
            Box::new(super::GenericBrowserPriorPlugin::new(version.clone())),
            Box::new(super::UnloggedFallbackPlugin::new(
                version,
                taxonomy.default_category().to_owned(),
            )),
        ];
        let cases = [
            ("Code", "private project"),
            ("Slack", "general"),
            ("Google Chrome", "youtube.com/watch"),
            ("Autodesk Fusion", "Untitled"),
            ("Unknown App", ""),
        ];

        for plugin in &plugins {
            for (app_name, window_title) in cases {
                assert_eq!(
                    plugin.classify_declared(app_name, window_title, DeclaredMetadata::default()),
                    plugin.classify(app_name, window_title),
                    "{app_name} / {window_title}"
                );
            }
        }
    }

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

    /// The shapes this tier actually sees, which is what the cases below are.
    ///
    /// `AbstractionEngine::process` composes the browser context as the site
    /// from `focused_site_context` followed by the abstracted title, and
    /// `focused_site_context` keeps the host and drops the path. So a Google
    /// Sheet arrives as `docs.google.com` plus `... - Google Sheets`, never as a
    /// `/spreadsheets/` path and never from a `sheets.google.com` host. The
    /// cases that used to stand here asserted `sheets.google.com` and
    /// `slides.google.com`, which no Google document is served from, so they
    /// passed while `document:sheets` and `document:slides` were unreachable for
    /// every real spreadsheet and deck.
    #[test]
    fn browser_context_classifies_domain_like_tab_hints() {
        let plugin = super::BrowserContextPlugin::new("mvp-1".to_owned());
        let cases = [
            (
                "Google Chrome",
                "docs.google.com Quarterly plan - Google Docs",
                "document:docs",
                "FOCUS_WORK",
            ),
            (
                "Safari",
                "docs.google.com Q3 budget - Google Sheets",
                "document:sheets",
                "FOCUS_WORK",
            ),
            (
                "Arc",
                "docs.google.com Kickoff - Google Slides",
                "document:slides",
                "FOCUS_WORK",
            ),
            (
                "Brave Browser",
                "mail.google.com Inbox (12)",
                "communication:gmail",
                "COMMUNICATION",
            ),
            (
                "Firefox",
                "youtube.com/watch?v=private",
                "video:youtube",
                "PASSIVE_CONSUMPTION",
            ),
            // A tab whose title says nothing still names the right category
            // from the host alone; only the label is the general one.
            ("Chromium", "docs.google.com", "document:docs", "FOCUS_WORK"),
            // The redirect entry points, which a browser can report for the
            // instant before the redirect lands.
            (
                "Google Chrome",
                "sheets.google.com/spreadsheets/d/abc",
                "document:sheets",
                "FOCUS_WORK",
            ),
            (
                "Google Chrome",
                "slides.google.com/presentation/d/abc",
                "document:slides",
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

    /// Documentation subdomains are reference reading, and they were arriving as
    /// unclassified. A bare `docs` keyword sat in the `docs.google.com` rule, so
    /// `docs.github.com` matched it *and* `github com`; `classify_matching_rules`
    /// takes the first match and returns UNLOGGED when a later match disagrees,
    /// so FOCUS_WORK plus REFERENCE resolved to neither. It also meant the
    /// `docs rs` entry in `reference:read` could never win.
    #[test]
    fn documentation_hosts_read_as_reference_rather_than_unlogged() {
        let plugin = super::BrowserContextPlugin::new("mvp-1".to_owned());
        let cases = [
            (
                "docs.github.com en/actions/writing-workflows",
                "reference:github",
            ),
            (
                "github.com docs: fix a typo in the README",
                "reference:github",
            ),
            ("docs.gitlab.com ee/user/project", "reference:gitlab"),
            ("docs.rs serde::de::Deserialize", "reference:read"),
        ];

        for (context, expected_label) in cases {
            let result = plugin
                .classify("Google Chrome", context)
                .unwrap_or_else(|| panic!("{context} should classify locally"));

            assert_eq!(result.label(), expected_label, "{context}");
            assert_eq!(result.category(), "REFERENCE", "{context}");
            assert_eq!(
                result.status(),
                ClassificationStatus::Classified,
                "{context}"
            );
        }
    }

    /// The Google rules answer for Google windows. A general word is not a
    /// Google window: with bare `docs`, `sheets` and `slides` in host-specific
    /// rules, a shopping tab and an encyclopedia article were FOCUS_WORK.
    /// Abstaining is the right answer here rather than a missing one — the
    /// generic browser prior picks these up as the ambiguous REFERENCE they are.
    #[test]
    fn general_words_alone_do_not_reach_the_google_workspace_rules() {
        let plugin = super::BrowserContextPlugin::new("mvp-1".to_owned());

        for context in [
            "amazon.com linen bed sheets",
            "en.wikipedia.org Water slides",
            "news.ycombinator.com Show HN: docs for everything",
        ] {
            assert!(plugin.classify("Safari", context).is_none(), "{context}");
        }
    }

    /// Counts its calls, so a test can tell an inference from a cache hit.
    struct CountingModel {
        embedding: Vec<f32>,
        calls: Arc<AtomicU64>,
    }

    impl super::EmbeddingModel for CountingModel {
        fn embed(&self, _input: &str) -> Result<Vec<f32>, super::EmbeddingError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.embedding.clone())
        }
    }

    /// The persisted sketch cache, as a map from the caller's key to a row.
    struct RowStore {
        rows: Mutex<HashMap<String, Vec<f32>>>,
    }

    impl RowStore {
        fn holding(key_hash: &str, embedding: Vec<f32>) -> Self {
            Self {
                rows: Mutex::new(HashMap::from([(key_hash.to_owned(), embedding)])),
            }
        }
    }

    impl SemanticLearningStore for RowStore {
        fn record_embedding(&self, key_hash: &str, embedding: &[f32]) -> Result<(), StoreError> {
            self.rows
                .lock()
                .expect("the row lock is not poisoned")
                .insert(key_hash.to_owned(), embedding.to_vec());
            Ok(())
        }

        fn embedding(&self, key_hash: &str) -> Result<Option<Vec<f32>>, StoreError> {
            Ok(self
                .rows
                .lock()
                .expect("the row lock is not poisoned")
                .get(key_hash)
                .cloned())
        }

        fn personal_prototypes(&self) -> Result<Vec<PersonalSemanticPrototype>, StoreError> {
            Ok(Vec::new())
        }

        fn record_classifier_use(&self, _artifact_version: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    fn two_category_centroids() -> HashMap<String, Vec<f32>> {
        HashMap::from([
            ("FOCUS_WORK".to_owned(), vec![1.0, 0.0]),
            ("PASSIVE_CONSUMPTION".to_owned(), vec![0.0, 1.0]),
        ])
    }

    /// A browser window must not be classified with another page's sketch.
    ///
    /// The persisted row is addressed by the stable key, which for a browser is
    /// (application, site) — one key for every page on the host, because the
    /// path is dropped before it ever reaches this crate — while the embedded
    /// string carries the title as well. Reading the row back by key alone
    /// therefore handed this window whichever page of the site was observed
    /// last: here, a row holding a PASSIVE_CONSUMPTION sketch answering for a
    /// spreadsheet.
    #[test]
    fn a_persisted_row_is_not_reused_for_a_different_page_of_the_same_site() {
        let store = Arc::new(RowStore::holding("site-key", vec![0.0, 1.0]));
        // Cloned into a trait object so the test keeps a typed handle on the
        // rows the plugin writes.
        let learning_store: Arc<dyn SemanticLearningStore> = store.clone();
        let calls = Arc::new(AtomicU64::new(0));
        let plugin = super::EmbeddingSimilarityPlugin::new(
            Arc::new(CountingModel {
                embedding: vec![1.0, 0.0],
                calls: Arc::clone(&calls),
            }),
            two_category_centroids(),
            "mvp-1",
            0.72,
            Duration::from_millis(500),
            Arc::new(super::EmbeddingMetrics::default()),
        )
        .expect("the plugin builds")
        .with_learning_store(learning_store);

        plugin.observe("site-key", "Safari", "docs.google.com Q3 budget");
        let result = plugin
            .classify("Safari", "docs.google.com Q3 budget")
            .expect("an observed window classifies");

        assert_eq!(result.category(), "FOCUS_WORK");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "the window has to be embedded rather than read out of another page's row"
        );

        // Still a cache, which is the point of keying the read by the input
        // rather than dropping the read: the row now holds this input's sketch,
        // so observing the same window again reuses it instead of inferring.
        plugin.observe("site-key", "Safari", "docs.google.com Q3 budget");
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        // And the row is still written under the stable key, which is what
        // `personal_semantic_prototype` joins `abstraction_map` against.
        assert_eq!(
            store
                .rows
                .lock()
                .expect("the row lock is not poisoned")
                .get("site-key")
                .cloned(),
            Some(vec![1.0, 0.0])
        );
    }

    /// Personal prototypes only, so the corrected-evidence path is the one under
    /// test and the seed prototypes cannot answer first.
    struct PersonalPrototypeStore(Vec<PersonalSemanticPrototype>);

    impl SemanticLearningStore for PersonalPrototypeStore {
        fn record_embedding(&self, _key_hash: &str, _embedding: &[f32]) -> Result<(), StoreError> {
            Ok(())
        }

        fn embedding(&self, _key_hash: &str) -> Result<Option<Vec<f32>>, StoreError> {
            Ok(None)
        }

        fn personal_prototypes(&self) -> Result<Vec<PersonalSemanticPrototype>, StoreError> {
            Ok(self.0.clone())
        }

        fn record_classifier_use(&self, _artifact_version: &str) -> Result<(), StoreError> {
            Ok(())
        }
    }

    /// A prototype pointing exactly at the observed embedding, so its weight is
    /// its score: cosine 1.0 times the weight.
    fn personal_prototype(category: &str, weight: f32) -> PersonalSemanticPrototype {
        PersonalSemanticPrototype {
            category: category.to_owned(),
            embedding: vec![1.0, 0.0],
            weight,
        }
    }

    fn plugin_with_personal_prototypes(
        prototypes: Vec<PersonalSemanticPrototype>,
    ) -> super::EmbeddingSimilarityPlugin {
        super::EmbeddingSimilarityPlugin::new(
            Arc::new(CountingModel {
                embedding: vec![1.0, 0.0],
                calls: Arc::new(AtomicU64::new(0)),
            }),
            two_category_centroids(),
            "mvp-1",
            0.72,
            Duration::from_millis(500),
            Arc::new(super::EmbeddingMetrics::default()),
        )
        .expect("the plugin builds")
        .with_learning_store(Arc::new(PersonalPrototypeStore(prototypes)))
    }

    /// One category can hold several corrected prototypes, so the second-ranked
    /// candidate is frequently another prototype of the winning category. A
    /// guard that only inspects that slot never sees the disagreement below it,
    /// and 0.95 COMMUNICATION against 0.92 REFERENCE is exactly the near-tie the
    /// 0.08 margin exists to refuse.
    #[test]
    fn a_same_category_runner_up_does_not_hide_a_disagreeing_near_tie() {
        let plugin = plugin_with_personal_prototypes(vec![
            personal_prototype("COMMUNICATION", 0.95),
            personal_prototype("COMMUNICATION", 0.94),
            personal_prototype("REFERENCE", 0.92),
        ]);

        let result = plugin
            .classify("Unknown", "ambiguous context")
            .expect("the personal tier answers");

        assert_eq!(result.category(), "UNLOGGED");
        assert_eq!(result.label(), "unlogged");
        assert_eq!(result.status(), ClassificationStatus::Ambiguous);
        assert_eq!(result.confidence(), ClassificationConfidence::Low);
        assert_eq!(result.source(), ClassificationSource::UserRule);
    }

    /// The other half of the same guard: a crowd of agreeing prototypes is not
    /// ambiguity, so a winner whose nearest *disagreeing* candidate is far away
    /// still classifies with the confidence a correction earns.
    #[test]
    fn a_personal_winner_nothing_disagrees_with_still_classifies() {
        let plugin = plugin_with_personal_prototypes(vec![
            personal_prototype("COMMUNICATION", 0.95),
            personal_prototype("COMMUNICATION", 0.94),
            personal_prototype("REFERENCE", 0.80),
        ]);

        let result = plugin
            .classify("Unknown", "corrected context")
            .expect("the personal tier answers");

        assert_eq!(result.category(), "COMMUNICATION");
        assert_eq!(result.label(), "communication:inferred");
        assert_eq!(result.status(), ClassificationStatus::Classified);
        assert_eq!(result.confidence(), ClassificationConfidence::High);
        assert_eq!(result.source(), ClassificationSource::UserRule);
    }
}
