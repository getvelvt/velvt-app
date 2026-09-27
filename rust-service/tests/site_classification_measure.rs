//! Blind measurement of how the abstraction engine classifies what people do,
//! against the persona corpus in `tests/fixtures/site_classification` (see its
//! README for how the corpus was made and why it is never edited to fit).
//!
//! This measures; it does not gate. The only assertions are that the corpus is
//! well formed and that every visit was classified without a Tier 2 timeout,
//! so a run that finishes printed numbers that describe the engine. Run it
//! with
//!
//! ```text
//! cargo test --test site_classification_measure -- --ignored --nocapture
//! ```
//!
//! It prints a report and writes the same numbers, plus every visit's
//! prediction, as JSON to the path in `VELVT_MEASURE_OUT` (default:
//! `site_classification_measure.json` in Cargo's integration-test temporary
//! directory).
//!
//! A prediction is *confident* exactly when the drift gate would count it as
//! evidence (`is_confident` in `src/work_block/mod.rs`): classified, High or
//! Medium confidence, and not SYSTEM, UNCLASSIFIED or UNLOGGED. Every other
//! second is time the person would have to categorize themselves, which the
//! report calls needs-a-category time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;
use velvt_service::abstraction::{
    AbstractedEvent, AbstractionEngine, ClassificationConfidence, ClassificationStatus,
    EmbeddingSalt, EmbeddingSimilarityPlugin, InMemoryMappingStore, Taxonomy,
};
use velvt_shared_types::RawEvent;

const CORPUS_PATH: &str = "tests/fixtures/site_classification/corpus.jsonl";
const OUTPUT_ENV: &str = "VELVT_MEASURE_OUT";

/// Every truth value the corpus may carry: the taxonomy's categories a person
/// can mean (UNLOGGED is "not classified", never an answer) plus UNSURE.
const TRUTHS: [&str; 8] = [
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    UNSURE,
];
const UNSURE: &str = "UNSURE";
/// The confusion-matrix column for every visit the gate would not count.
const NEEDS_A_CATEGORY: &str = "NEEDS_A_CATEGORY";
const SPLITS: [&str; 3] = ["dev", "test", "all"];
const KINDS: [Kind; 2] = [Kind::Browser, Kind::Native];
const TOP_WRONG_CONFIDENT: usize = 25;
const TOP_NEEDS_A_CATEGORY_LISTED: usize = 25;
const TOP_NEEDS_A_CATEGORY_COVERAGE: usize = 8;
/// A pass that hit a Tier 2 timeout is discarded and rerun on a fresh engine;
/// this many discarded passes in a row fails the measurement instead.
const MAX_PASSES: usize = 5;
/// The metric `EmbeddingSimilarityPlugin::infer` logs when it gives up.
const TIER2_TIMEOUT_METRIC: &str = "tier2_timeout_count";

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn measure_site_classification() {
    let corpus_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS_PATH);
    let visits = load_corpus(&corpus_path);
    let taxonomy_version = Taxonomy::from_builtin()
        .expect("the shipped taxonomy loads")
        .version()
        .to_owned();

    let configs = Config::ALL
        .iter()
        .map(|&config| measure_config(config, &visits))
        .collect::<Vec<_>>();
    let report = Report {
        corpus: CorpusSummary::new(CORPUS_PATH, &visits),
        taxonomy_version,
        confident_rule: "status == classified && confidence in {high, medium} && \
                         category (case-insensitive) not in {SYSTEM, UNCLASSIFIED, UNLOGGED}",
        configs,
    };

    println!("{}", render(&report));
    let output = output_path();
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).expect("the report directory can be created");
    }
    let json = serde_json::to_string_pretty(&report).expect("the report serializes");
    std::fs::write(&output, json + "\n").expect("the report can be written");
    println!("JSON report: {}", output.display());
}

// ---------------------------------------------------------------------------
// Corpus

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Visit {
    persona: String,
    app_name: String,
    bundle_id: Option<String>,
    host: Option<String>,
    title: String,
    seconds: u64,
    truth: String,
    split: String,
}

impl Visit {
    fn kind(&self) -> Kind {
        if self.host.is_some() {
            Kind::Browser
        } else {
            Kind::Native
        }
    }

    /// What the visit is about: its normalized host in a browser, the
    /// application otherwise. The split rule hashes exactly this.
    fn subject(&self) -> String {
        match &self.host {
            Some(host) => normalized_host(host),
            None => self.app_name.clone(),
        }
    }

    /// The frame the Swift client sends for this visit, as the router hands it
    /// to `AbstractionEngine::process`: the browser's focused URL is reduced to
    /// the host by the engine, so the corpus only has to carry the host.
    fn raw_event(&self, index: usize) -> RawEvent {
        RawEvent {
            event_id: Uuid::from_u128(index as u128),
            occurred_at: Utc
                .with_ymd_and_hms(2026, 9, 21, 9, 0, 0)
                .single()
                .expect("a valid fixed timestamp"),
            duration_seconds: self.seconds,
            app_name: self.app_name.clone(),
            window_title: self.title.clone(),
            bundle_id: self.bundle_id.clone(),
            declared_app_category: None,
            document_type_ids: Vec::new(),
            focused_document_url: self.host.as_ref().map(|host| format!("https://{host}/")),
            in_progress: false,
        }
    }
}

/// Lowercase, with one leading `www.` removed.
fn normalized_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.strip_prefix("www.") {
        Some(rest) => rest.to_owned(),
        None => host,
    }
}

/// dev when the first byte of sha256(subject) is even, test otherwise.
fn split_for(subject: &str) -> &'static str {
    if Sha256::digest(subject.as_bytes())[0] % 2 == 0 {
        "dev"
    } else {
        "test"
    }
}

fn load_corpus(path: &Path) -> Vec<Visit> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    let visits = text
        .lines()
        .enumerate()
        .map(|(offset, line)| {
            let number = offset + 1;
            let visit: Visit = serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("corpus line {number} is not a visit: {err}"));
            assert!(
                TRUTHS.contains(&visit.truth.as_str()),
                "corpus line {number}: truth {:?} is not a category or UNSURE",
                visit.truth
            );
            assert!(visit.seconds > 0, "corpus line {number}: zero seconds");
            assert!(
                !visit.persona.is_empty() && !visit.app_name.is_empty(),
                "corpus line {number}: persona and app_name are required"
            );
            assert!(
                visit.host.as_deref().is_none_or(|host| !host.is_empty()),
                "corpus line {number}: an empty host must be null"
            );
            assert_eq!(
                visit.split,
                split_for(&visit.subject()),
                "corpus line {number}: split does not follow the split rule"
            );
            visit
        })
        .collect::<Vec<_>>();
    assert!(!visits.is_empty(), "the corpus is empty");
    visits
}

// ---------------------------------------------------------------------------
// Engines

#[derive(Debug, Clone, Copy)]
enum Config {
    Builtin,
    Production,
}

impl Config {
    const ALL: [Config; 2] = [Config::Builtin, Config::Production];

    fn name(self) -> &'static str {
        match self {
            Config::Builtin => "builtin",
            Config::Production => "production",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Config::Builtin => "AbstractionEngine::from_builtin_taxonomy: the builtin plugins without Tier 2",
            Config::Production => {
                "builtin plugins plus the shipped Tier 2 \
                 (EmbeddingSimilarityPlugin::builtin_salted, fixed salt, no learning store), as main.rs wires it"
            }
        }
    }

    /// A fresh engine on an empty store, so no visit sees a correction.
    fn engine(self) -> AbstractionEngine {
        let store = Arc::new(InMemoryMappingStore::default());
        match self {
            Config::Builtin => {
                AbstractionEngine::from_builtin_taxonomy(store).expect("the builtin engine builds")
            }
            Config::Production => {
                let taxonomy = Taxonomy::from_builtin().expect("the shipped taxonomy loads");
                let embedding = EmbeddingSimilarityPlugin::builtin_salted(
                    taxonomy.version(),
                    EmbeddingSalt::from_bytes([7u8; EmbeddingSalt::LENGTH]),
                )
                .expect("the shipped Tier 2 builds");
                AbstractionEngine::builder(store, taxonomy)
                    .register_builtin_plugins_with_embedding(Some(embedding))
                    .build()
                    .expect("the production engine builds")
            }
        }
    }
}

/// Counts Tier 2 timeouts on the measuring thread. A timed-out inference
/// makes Tier 2 abstain, so a pass that hit one measured the machine's load
/// rather than the classifier and is thrown away.
#[derive(Clone, Default)]
struct Tier2Timeouts(Arc<AtomicU64>);

impl Tier2Timeouts {
    fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Tier2Timeouts {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct MetricVisitor(bool);
        impl tracing::field::Visit for MetricVisitor {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "metric" && value == TIER2_TIMEOUT_METRIC {
                    self.0 = true;
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        let mut visitor = MetricVisitor(false);
        event.record(&mut visitor);
        if visitor.0 {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Prediction {
    /// 1-based line in the corpus.
    line: usize,
    split: String,
    kind: &'static str,
    subject: String,
    seconds: u64,
    truth: String,
    category: String,
    label: String,
    status: &'static str,
    confidence: &'static str,
    source: &'static str,
    tier: &'static str,
    confident: bool,
}

impl Prediction {
    fn new(line: usize, visit: &Visit, outcome: Result<AbstractedEvent, String>) -> Self {
        let base = |category: String, label: String| Prediction {
            line,
            split: visit.split.clone(),
            kind: visit.kind().name(),
            subject: visit.subject(),
            seconds: visit.seconds,
            truth: visit.truth.clone(),
            category,
            label,
            status: "error",
            confidence: "none",
            source: "none",
            tier: "none",
            confident: false,
        };
        match outcome {
            Ok(event) => Prediction {
                status: event.classification_status().as_str(),
                confidence: event.classification_confidence().as_str(),
                source: event.classification_source().as_str(),
                tier: event.classification_tier().as_str(),
                confident: is_confident(
                    event.category(),
                    event.classification_status(),
                    event.classification_confidence(),
                ),
                ..base(event.category().to_owned(), event.label().to_owned())
            },
            Err(error) => base("ABSTRACTION_ERROR".to_owned(), error),
        }
    }

    /// The category the gate would act on, or NEEDS_A_CATEGORY.
    fn gated_category(&self) -> &str {
        if self.confident {
            &self.category
        } else {
            NEEDS_A_CATEGORY
        }
    }

    fn judged(&self) -> bool {
        self.confident && self.truth != UNSURE
    }

    fn correct(&self) -> bool {
        self.judged() && self.category == self.truth
    }

    fn wrong(&self) -> bool {
        self.judged() && self.category != self.truth
    }
}

/// Mirrors `is_confident` in `src/work_block/mod.rs`, which is private.
fn is_confident(
    category: &str,
    status: ClassificationStatus,
    confidence: ClassificationConfidence,
) -> bool {
    status == ClassificationStatus::Classified
        && matches!(
            confidence,
            ClassificationConfidence::High | ClassificationConfidence::Medium
        )
        && !matches!(
            category.to_ascii_lowercase().as_str(),
            "system" | "unclassified" | "unlogged"
        )
}

fn classify_all(config: Config, visits: &[Visit]) -> (Vec<Prediction>, u64) {
    let timeouts = Tier2Timeouts::default();
    let subscriber = tracing_subscriber::registry().with(timeouts.clone());
    let predictions = tracing::subscriber::with_default(subscriber, || {
        let engine = config.engine();
        visits
            .iter()
            .enumerate()
            .map(|(offset, visit)| {
                let line = offset + 1;
                let outcome = engine
                    .process(visit.raw_event(line))
                    .map_err(|err| err.to_string());
                Prediction::new(line, visit, outcome)
            })
            .collect::<Vec<_>>()
    });
    (predictions, timeouts.count())
}

fn measure_config(config: Config, visits: &[Visit]) -> ConfigReport {
    let mut discarded_passes = 0;
    let predictions = loop {
        let (predictions, timeouts) = classify_all(config, visits);
        if timeouts == 0 {
            break predictions;
        }
        discarded_passes += 1;
        assert!(
            discarded_passes < MAX_PASSES,
            "{}: every one of {MAX_PASSES} passes hit a Tier 2 timeout; the machine is too loaded to measure",
            config.name()
        );
    };
    let sections = SPLITS
        .iter()
        .flat_map(|&split| KINDS.iter().map(move |&kind| (split, kind)))
        .map(|(split, kind)| {
            let selected = predictions
                .iter()
                .filter(|prediction| split == "all" || prediction.split == split)
                .filter(|prediction| prediction.kind == kind.name())
                .collect::<Vec<_>>();
            Section::new(split, kind, &selected)
        })
        .collect();
    ConfigReport {
        name: config.name(),
        description: config.description(),
        discarded_passes_for_tier2_timeouts: discarded_passes,
        abstraction_errors: predictions
            .iter()
            .filter(|prediction| prediction.status == "error")
            .count(),
        sections,
        predictions,
    }
}

// ---------------------------------------------------------------------------
// Metrics

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Browser,
    Native,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Browser => "browser",
            Kind::Native => "native",
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    corpus: CorpusSummary,
    taxonomy_version: String,
    confident_rule: &'static str,
    configs: Vec<ConfigReport>,
}

#[derive(Debug, Serialize)]
struct CorpusSummary {
    path: &'static str,
    visits: usize,
    seconds: u64,
    browser_visits: usize,
    native_visits: usize,
    distinct_hosts: usize,
    distinct_native_apps: usize,
    splits: BTreeMap<&'static str, SplitSummary>,
    truth: BTreeMap<String, Tally>,
}

#[derive(Debug, Default, Serialize)]
struct SplitSummary {
    visits: usize,
    browser_visits: usize,
    native_visits: usize,
    browser_seconds: u64,
    native_seconds: u64,
    distinct_hosts: usize,
    distinct_native_apps: usize,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct Tally {
    visits: usize,
    seconds: u64,
}

impl Tally {
    fn add(&mut self, seconds: u64) {
        self.visits += 1;
        self.seconds += seconds;
    }
}

impl CorpusSummary {
    fn new(path: &'static str, visits: &[Visit]) -> Self {
        let mut splits = BTreeMap::new();
        for split in SPLITS {
            let selected = visits
                .iter()
                .filter(|visit| split == "all" || visit.split == split)
                .collect::<Vec<_>>();
            let of_kind = |kind: Kind| {
                selected
                    .iter()
                    .filter(move |visit| visit.kind() == kind)
                    .copied()
            };
            let distinct = |kind: Kind| {
                of_kind(kind)
                    .map(Visit::subject)
                    .collect::<BTreeSet<_>>()
                    .len()
            };
            let seconds = |kind: Kind| of_kind(kind).map(|visit| visit.seconds).sum();
            splits.insert(
                split,
                SplitSummary {
                    visits: selected.len(),
                    browser_visits: of_kind(Kind::Browser).count(),
                    native_visits: of_kind(Kind::Native).count(),
                    browser_seconds: seconds(Kind::Browser),
                    native_seconds: seconds(Kind::Native),
                    distinct_hosts: distinct(Kind::Browser),
                    distinct_native_apps: distinct(Kind::Native),
                },
            );
        }
        let mut truth = BTreeMap::<String, Tally>::new();
        for visit in visits {
            truth
                .entry(visit.truth.clone())
                .or_default()
                .add(visit.seconds);
        }
        let all = &splits["all"];
        CorpusSummary {
            path,
            visits: visits.len(),
            seconds: visits.iter().map(|visit| visit.seconds).sum(),
            browser_visits: all.browser_visits,
            native_visits: all.native_visits,
            distinct_hosts: all.distinct_hosts,
            distinct_native_apps: all.distinct_native_apps,
            splits,
            truth,
        }
    }
}

#[derive(Debug, Serialize)]
struct ConfigReport {
    name: &'static str,
    description: &'static str,
    discarded_passes_for_tier2_timeouts: usize,
    abstraction_errors: usize,
    sections: Vec<Section>,
    /// Every visit's prediction, in corpus order, for diffing two runs.
    predictions: Vec<Prediction>,
}

/// Every metric for one split and one kind of visit.
#[derive(Debug, Serialize)]
struct Section {
    split: &'static str,
    kind: &'static str,
    visits: usize,
    seconds: u64,
    confident_seconds: u64,
    confident_share: Option<f64>,
    needs_a_category_seconds: u64,
    needs_a_category_share: Option<f64>,
    /// Confident visits whose truth is not UNSURE: the ones precision judges.
    judged_confident: Tally,
    correct_confident: Tally,
    precision_by_seconds: Option<f64>,
    precision_by_visits: Option<f64>,
    wrong_confident_seconds: u64,
    /// Of all seconds in this section: the time the gate would act on wrongly.
    wrong_confident_share: Option<f64>,
    unsure_seconds: u64,
    confident_on_unsure_seconds: u64,
    /// Of all seconds in this section.
    confident_on_unsure_share: Option<f64>,
    /// Of the UNSURE seconds: how often a genuinely mixed visit got a verdict.
    confident_on_unsure_share_of_unsure: Option<f64>,
    /// truth -> gated category (a confident category or NEEDS_A_CATEGORY) -> seconds.
    confusion_seconds: BTreeMap<String, BTreeMap<String, u64>>,
    /// How the engine answered, confident or not, largest first.
    decisions: Vec<DecisionRow>,
    wrong_confident_top: Vec<WrongConfidentRow>,
    needs_a_category_subjects: usize,
    needs_a_category_top: Vec<NeedsACategoryRow>,
    needs_a_category_top8_seconds: u64,
    /// Of the needs-a-category seconds.
    needs_a_category_top8_share: Option<f64>,
}

#[derive(Debug, Serialize)]
struct DecisionRow {
    source: &'static str,
    tier: &'static str,
    status: &'static str,
    confidence: &'static str,
    category: String,
    confident: bool,
    visits: usize,
    seconds: u64,
}

#[derive(Debug, Serialize)]
struct WrongConfidentRow {
    subject: String,
    truth: String,
    predicted: String,
    label: String,
    visits: usize,
    seconds: u64,
}

#[derive(Debug, Serialize)]
struct NeedsACategoryRow {
    subject: String,
    visits: usize,
    seconds: u64,
    /// The truth of those seconds, by category.
    truth_seconds: BTreeMap<String, u64>,
}

impl Section {
    fn new(split: &'static str, kind: Kind, predictions: &[&Prediction]) -> Self {
        let sum = |keep: &dyn Fn(&Prediction) -> bool| -> u64 {
            predictions
                .iter()
                .filter(|prediction| keep(prediction))
                .map(|prediction| prediction.seconds)
                .sum()
        };
        let count = |keep: &dyn Fn(&Prediction) -> bool| -> usize {
            predictions
                .iter()
                .filter(|prediction| keep(prediction))
                .count()
        };
        let seconds = sum(&|_| true);
        let confident_seconds = sum(&|prediction| prediction.confident);
        let needs_a_category_seconds = seconds - confident_seconds;
        let judged_confident = Tally {
            visits: count(&Prediction::judged),
            seconds: sum(&Prediction::judged),
        };
        let correct_confident = Tally {
            visits: count(&Prediction::correct),
            seconds: sum(&Prediction::correct),
        };
        let wrong_confident_seconds = sum(&Prediction::wrong);
        let unsure_seconds = sum(&|prediction| prediction.truth == UNSURE);
        let confident_on_unsure_seconds =
            sum(&|prediction| prediction.confident && prediction.truth == UNSURE);

        let mut confusion_seconds = BTreeMap::<String, BTreeMap<String, u64>>::new();
        for prediction in predictions {
            *confusion_seconds
                .entry(prediction.truth.clone())
                .or_default()
                .entry(prediction.gated_category().to_owned())
                .or_default() += prediction.seconds;
        }

        let mut decisions = BTreeMap::<_, Tally>::new();
        for prediction in predictions {
            decisions
                .entry((
                    prediction.source,
                    prediction.tier,
                    prediction.status,
                    prediction.confidence,
                    prediction.category.clone(),
                    prediction.confident,
                ))
                .or_default()
                .add(prediction.seconds);
        }
        let mut decisions = decisions
            .into_iter()
            .map(
                |((source, tier, status, confidence, category, confident), tally)| DecisionRow {
                    source,
                    tier,
                    status,
                    confidence,
                    category,
                    confident,
                    visits: tally.visits,
                    seconds: tally.seconds,
                },
            )
            .collect::<Vec<_>>();
        // Stable: ties keep the key order the BTreeMap produced.
        decisions.sort_by_key(|row| std::cmp::Reverse(row.seconds));

        let mut wrong = BTreeMap::<_, Tally>::new();
        for prediction in predictions.iter().filter(|prediction| prediction.wrong()) {
            wrong
                .entry((
                    prediction.subject.clone(),
                    prediction.truth.clone(),
                    prediction.category.clone(),
                    prediction.label.clone(),
                ))
                .or_default()
                .add(prediction.seconds);
        }
        let mut wrong_confident_top = wrong
            .into_iter()
            .map(
                |((subject, truth, predicted, label), tally)| WrongConfidentRow {
                    subject,
                    truth,
                    predicted,
                    label,
                    visits: tally.visits,
                    seconds: tally.seconds,
                },
            )
            .collect::<Vec<_>>();
        wrong_confident_top.sort_by_key(|row| std::cmp::Reverse(row.seconds));
        wrong_confident_top.truncate(TOP_WRONG_CONFIDENT);

        let mut needs = BTreeMap::<String, (Tally, BTreeMap<String, u64>)>::new();
        for prediction in predictions
            .iter()
            .filter(|prediction| !prediction.confident)
        {
            let (tally, truth_seconds) = needs.entry(prediction.subject.clone()).or_default();
            tally.add(prediction.seconds);
            *truth_seconds.entry(prediction.truth.clone()).or_default() += prediction.seconds;
        }
        let needs_a_category_subjects = needs.len();
        let mut needs_a_category_top = needs
            .into_iter()
            .map(|(subject, (tally, truth_seconds))| NeedsACategoryRow {
                subject,
                visits: tally.visits,
                seconds: tally.seconds,
                truth_seconds,
            })
            .collect::<Vec<_>>();
        needs_a_category_top.sort_by_key(|row| std::cmp::Reverse(row.seconds));
        let needs_a_category_top8_seconds = needs_a_category_top
            .iter()
            .take(TOP_NEEDS_A_CATEGORY_COVERAGE)
            .map(|row| row.seconds)
            .sum();
        needs_a_category_top.truncate(TOP_NEEDS_A_CATEGORY_LISTED);

        Section {
            split,
            kind: kind.name(),
            visits: predictions.len(),
            seconds,
            confident_seconds,
            confident_share: share(confident_seconds, seconds),
            needs_a_category_seconds,
            needs_a_category_share: share(needs_a_category_seconds, seconds),
            precision_by_seconds: share(correct_confident.seconds, judged_confident.seconds),
            precision_by_visits: share(
                correct_confident.visits as u64,
                judged_confident.visits as u64,
            ),
            judged_confident,
            correct_confident,
            wrong_confident_seconds,
            wrong_confident_share: share(wrong_confident_seconds, seconds),
            unsure_seconds,
            confident_on_unsure_seconds,
            confident_on_unsure_share: share(confident_on_unsure_seconds, seconds),
            confident_on_unsure_share_of_unsure: share(confident_on_unsure_seconds, unsure_seconds),
            confusion_seconds,
            decisions,
            wrong_confident_top,
            needs_a_category_subjects,
            needs_a_category_top,
            needs_a_category_top8_seconds,
            needs_a_category_top8_share: share(
                needs_a_category_top8_seconds,
                needs_a_category_seconds,
            ),
        }
    }
}

/// A ratio rounded to six places, so two runs' JSON diff cleanly; `None`
/// when there is nothing to divide by.
fn share(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| (part as f64 / whole as f64 * 1e6).round() / 1e6)
}

// ---------------------------------------------------------------------------
// Output

fn output_path() -> PathBuf {
    std::env::var_os(OUTPUT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("site_classification_measure.json")
        })
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{:.1}%", value * 100.0))
}

/// Short column names for the confusion matrix, so it fits a terminal.
fn short(category: &str) -> &str {
    match category {
        "FOCUS_WORK" => "FOCUS",
        "PASSIVE_CONSUMPTION" => "PASSIVE",
        "SOCIAL_FEED" => "SOCIAL",
        "COMMUNICATION" => "COMMS",
        "TASK_MANAGEMENT" => "TASK",
        "REFERENCE" => "REF",
        NEEDS_A_CATEGORY => "NEEDS",
        other => other,
    }
}

/// Headline table columns and widths; the first three are left-aligned.
const HEADLINE_COLUMNS: [(&str, usize); 14] = [
    ("config", 10),
    ("split", 5),
    ("kind", 7),
    ("visits", 6),
    ("seconds", 8),
    ("confident", 9),
    ("needs", 7),
    ("prec(s)", 8),
    ("prec(n)", 8),
    ("wrong-conf", 10),
    ("conf-UNSR", 10),
    ("of-UNSURE", 10),
    ("needs-hst", 9),
    ("top8", 6),
];

fn headline_row(cells: &[String]) -> String {
    cells
        .iter()
        .zip(HEADLINE_COLUMNS)
        .enumerate()
        .map(|(index, (cell, (_, width)))| {
            if index < 3 {
                format!("{cell:<width$}")
            } else {
                format!("{cell:>width$}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render(report: &Report) -> String {
    let mut out = String::new();
    let corpus = &report.corpus;
    let _ = writeln!(
        out,
        "\n== Site classification measurement (taxonomy {}) ==",
        report.taxonomy_version
    );
    let _ = writeln!(
        out,
        "corpus {}: {} visits, {} s ({} browser visits on {} hosts, {} native visits in {} apps)",
        corpus.path,
        corpus.visits,
        corpus.seconds,
        corpus.browser_visits,
        corpus.distinct_hosts,
        corpus.native_visits,
        corpus.distinct_native_apps
    );
    for (split, summary) in &corpus.splits {
        let _ = writeln!(
            out,
            "  {split:<4} {:>5} visits | browser {:>5} visits {:>7} s {:>4} hosts | native {:>4} visits {:>6} s {:>3} apps",
            summary.visits,
            summary.browser_visits,
            summary.browser_seconds,
            summary.distinct_hosts,
            summary.native_visits,
            summary.native_seconds,
            summary.distinct_native_apps
        );
    }
    let _ = writeln!(out, "  truth (all visits):");
    for (truth, tally) in &corpus.truth {
        let _ = writeln!(
            out,
            "    {truth:<20} {:>5} visits {:>7} s",
            tally.visits, tally.seconds
        );
    }
    let _ = writeln!(out, "confident = {}", report.confident_rule);

    let _ = writeln!(out, "\n-- Headline --");
    let header = HEADLINE_COLUMNS.map(|(name, _)| name.to_owned());
    let _ = writeln!(out, "{}", headline_row(&header));
    for config in &report.configs {
        for section in &config.sections {
            let cells = [
                config.name.to_owned(),
                section.split.to_owned(),
                section.kind.to_owned(),
                section.visits.to_string(),
                section.seconds.to_string(),
                percent(section.confident_share),
                percent(section.needs_a_category_share),
                percent(section.precision_by_seconds),
                percent(section.precision_by_visits),
                percent(section.wrong_confident_share),
                percent(section.confident_on_unsure_share),
                percent(section.confident_on_unsure_share_of_unsure),
                section.needs_a_category_subjects.to_string(),
                percent(section.needs_a_category_top8_share),
            ];
            let _ = writeln!(out, "{}", headline_row(&cells));
        }
    }
    let _ = writeln!(
        out,
        "columns: confident/needs/wrong-conf/conf-UNSR are shares of the row's seconds; \
         prec = correct / confident with truth != UNSURE, by seconds (s) and visits (n); \
         of-UNSURE = confident share of UNSURE seconds; needs-hst = distinct hosts or apps \
         with needs-a-category time; top8 = share of needs-a-category seconds on the top 8 of them"
    );

    for config in &report.configs {
        let _ = writeln!(
            out,
            "\n== {}: {} ==\n(discarded passes for Tier 2 timeouts: {}, abstraction errors: {})",
            config.name,
            config.description,
            config.discarded_passes_for_tier2_timeouts,
            config.abstraction_errors
        );
        for section in &config.sections {
            render_section(&mut out, config.name, section);
        }
    }
    out
}

fn render_section(out: &mut String, config: &str, section: &Section) {
    let noun = if section.kind == Kind::Browser.name() {
        "host"
    } else {
        "app"
    };
    let _ = writeln!(
        out,
        "\n--- {config} / {} / {} : {} visits, {} s ---",
        section.split, section.kind, section.visits, section.seconds
    );
    let _ = writeln!(
        out,
        "confident {} s ({}) | needs a category {} s ({})",
        section.confident_seconds,
        percent(section.confident_share),
        section.needs_a_category_seconds,
        percent(section.needs_a_category_share)
    );
    let _ = writeln!(
        out,
        "precision {} by seconds ({}/{} s), {} by visits ({}/{}) | wrong-confident {} s ({}) | \
         confident on UNSURE {} s ({} of all, {} of {} UNSURE s)",
        percent(section.precision_by_seconds),
        section.correct_confident.seconds,
        section.judged_confident.seconds,
        percent(section.precision_by_visits),
        section.correct_confident.visits,
        section.judged_confident.visits,
        section.wrong_confident_seconds,
        percent(section.wrong_confident_share),
        section.confident_on_unsure_seconds,
        percent(section.confident_on_unsure_share),
        percent(section.confident_on_unsure_share_of_unsure),
        section.unsure_seconds
    );

    // Confusion matrix: truth rows in TRUTHS order, predicted columns sorted
    // with NEEDS_A_CATEGORY last.
    let mut columns = section
        .confusion_seconds
        .values()
        .flat_map(|row| row.keys().map(String::as_str))
        .filter(|category| *category != NEEDS_A_CATEGORY)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    columns.push(NEEDS_A_CATEGORY);
    let _ = writeln!(out, "confusion (seconds; rows truth, columns predicted)");
    let _ = write!(out, "{}", " ".repeat(10));
    for heading in columns.iter().map(|column| short(column)).chain(["total"]) {
        let _ = write!(out, " {heading:>8}");
    }
    let _ = writeln!(out);
    for truth in TRUTHS {
        let Some(row) = section.confusion_seconds.get(truth) else {
            continue;
        };
        let _ = write!(out, "  {:<8}", short(truth));
        for column in &columns {
            let _ = write!(out, " {:>8}", row.get(*column).copied().unwrap_or(0));
        }
        let _ = writeln!(out, " {:>8}", row.values().sum::<u64>());
    }

    let _ = writeln!(
        out,
        "decisions (source / tier / status / confidence -> category):"
    );
    for row in &section.decisions {
        let _ = writeln!(
            out,
            "  {:>7} s {:>4} visits  {} / {} / {} / {} -> {}{}",
            row.seconds,
            row.visits,
            row.source,
            row.tier,
            row.status,
            row.confidence,
            row.category,
            if row.confident { "  [confident]" } else { "" }
        );
    }

    if section.wrong_confident_top.is_empty() {
        let _ = writeln!(out, "wrong-confident: none");
    } else {
        let _ = writeln!(
            out,
            "top {} wrong-confident {noun}s (truth -> predicted, label):",
            section.wrong_confident_top.len()
        );
        for row in &section.wrong_confident_top {
            let _ = writeln!(
                out,
                "  {:>7} s {:>3} visits  {:<36} {} -> {} ({})",
                row.seconds, row.visits, row.subject, row.truth, row.predicted, row.label
            );
        }
    }

    let _ = writeln!(
        out,
        "needs a category: {} distinct {noun}s; top {} cover {} s ({} of needs-a-category time):",
        section.needs_a_category_subjects,
        TOP_NEEDS_A_CATEGORY_COVERAGE,
        section.needs_a_category_top8_seconds,
        percent(section.needs_a_category_top8_share)
    );
    for row in section
        .needs_a_category_top
        .iter()
        .take(TOP_NEEDS_A_CATEGORY_COVERAGE)
    {
        let truths = row
            .truth_seconds
            .iter()
            .map(|(truth, seconds)| format!("{} {seconds}", short(truth)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "  {:>7} s {:>3} visits  {:<36} truth: {truths}",
            row.seconds, row.visits, row.subject
        );
    }
}
