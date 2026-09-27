//! Validation of the two shadow segmentation models against synthetic traces
//! with known ground truth.
//!
//! `03-BEHAVIORAL-ENGINE-VALIDATION.md` states the job in one line: when the
//! engine says it found a pattern, did it find a pattern? There are two ways to
//! fail. Missing a real pattern is disappointing. **Inventing one is fatal**,
//! because the product's entire claim is that it tells the truth about what it
//! knows.
//!
//! # What this suite is evidence about, and what it is not
//!
//! The fixtures are `SYNTHETIC-suite-c-segmentation.jsonl`, which is run-level
//! and **not replayed through the ingestion path**. Suites A and B in
//! `trace_replay.rs` carry the ingestion evidence; this one carries none of it.
//! Every number below is therefore a claim about a **model**, measured on data
//! whose truth was known in advance. It is not evidence about people, it is not
//! evidence that the product behaves, and no real user has ever touched either
//! model.
//!
//! Saying exactly that is stronger than the alternative.
//!
//! # Why the module include, and what should replace it
//!
//! `behavior` is declared in `src/main.rs`, not `src/lib.rs`, so an integration
//! test cannot reach it through `velvt_service`. Until someone who owns
//! `lib.rs` moves it — one line, `pub mod behavior;` — the two model files are
//! included here by path. The cost is that their unit tests compile and run in
//! two crates.

#[path = "../src/behavior/bocpd.rs"]
mod bocpd;
#[path = "../src/behavior/hmm.rs"]
mod hmm;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use bocpd::{Bocpd, BocpdConfig, RunObservation};
use hmm::{HmmAbstention, HmmConfig, STATE_COUNT};

const FIXTURE: &str = "SYNTHETIC-suite-c-segmentation.jsonl";

/// The thresholds the sweep reports. Nothing here is a shipped threshold —
/// there is no shipped threshold, because the detector has no caller.
const THRESHOLDS: [f64; 7] = [0.20, 0.30, 0.50, 0.70, 0.90, 0.95, 0.99];

/// The threshold the report quotes when it needs one number. Chosen ABOVE the
/// hazard floor of 1/6 and above the shoulder the null sweep shows, and
/// recorded here so that a figure quoted elsewhere can be traced to it.
///
/// It is **not** a shipped threshold. Nothing ships with a threshold, because
/// nothing calls the detector.
const REPORTED_THRESHOLD: f64 = 0.90;

/// Runs per user-week in the NULL family, used to restate a per-run
/// false-alarm rate as the quantity anyone actually cares about. Measured from
/// the fixture rather than assumed; see `null_traces_false_alarm_rate`.
fn runs_per_user_week(traces: &[&Trace]) -> f64 {
    let runs: usize = traces.iter().map(|trace| trace.runs.len()).sum();
    let weeks: usize = traces.iter().map(|trace| trace.weeks).sum();
    runs as f64 / weeks as f64
}

/// Runs after a planted change point inside which a crossing counts as a
/// detection. Wider than this is not a detection, it is the next thing that
/// happened.
const DETECTION_WINDOW_RUNS: usize = 25;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Header {
    kind: String,
    suite: String,
    schema: String,
    synthetic: bool,
    label: String,
    acceptance: String,
    injection_method: String,
    families: Vec<String>,
    total_runs: usize,
    traces: usize,
}

#[derive(Debug, Deserialize)]
struct BlockTruth {
    index: usize,
    start: i64,
    duration: i64,
    runs: usize,
    routine: String,
}

#[derive(Debug, Deserialize)]
struct GroundTruth {
    planted: bool,
    #[serde(default)]
    blocks: Vec<BlockTruth>,
    #[serde(default)]
    antecedent_days: Vec<i64>,
    #[serde(default)]
    change_at_seconds: Option<i64>,
    #[serde(default)]
    change_run_index: Option<usize>,
    #[serde(default)]
    drift_run_index: Option<usize>,
    #[serde(default)]
    alternating_given_antecedent: Option<f64>,
    #[serde(default)]
    alternating_given_none: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Trace {
    trace_id: String,
    family: String,
    seed: u64,
    weeks: usize,
    /// `[t_seconds, category_index, dwell_seconds, block_index]`.
    runs: Vec<[i64; 4]>,
    ground_truth: GroundTruth,
}

impl Trace {
    fn observations(&self) -> Vec<RunObservation> {
        self.runs
            .iter()
            .map(|run| RunObservation::new(run[1] as usize, run[2] as f64))
            .collect()
    }

    fn categories(&self) -> Vec<usize> {
        self.runs.iter().map(|run| run[1] as usize).collect()
    }

    fn log_dwell(&self) -> Vec<f64> {
        self.runs
            .iter()
            .map(|run| bocpd::log_dwell(run[2] as f64))
            .collect()
    }

    fn local_hours(&self) -> Vec<usize> {
        self.runs
            .iter()
            .map(|run| ((run[0].rem_euclid(86_400)) / 3600) as usize)
            .collect()
    }
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("traces")
        .join(FIXTURE)
}

fn load() -> (Header, Vec<Trace>) {
    let path = fixture_path();
    let body = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}\n\nRegenerate with ./scripts/generate_traces.py",
            path.display()
        )
    });
    let mut lines = body.lines();
    let header: Header = serde_json::from_str(lines.next().expect("a header line"))
        .expect("the first line is the header record");
    let traces: Vec<Trace> = lines
        .map(|line| serde_json::from_str(line).expect("a trace record"))
        .collect();
    (header, traces)
}

fn family<'a>(traces: &'a [Trace], name: &str) -> Vec<&'a Trace> {
    traces.iter().filter(|trace| trace.family == name).collect()
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

/// Runs the detector over a whole run history without resetting it.
fn continuous(trace: &Trace, config: BocpdConfig) -> Vec<f64> {
    let mut detector = Bocpd::new(config);
    trace
        .observations()
        .into_iter()
        .map(|run| {
            detector
                .observe(run)
                .expect("a fixture run is well formed")
                .p_recent_change
        })
        .collect()
}

/// Runs the detector the way `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.1 specifies:
/// per work block, reset at every block boundary.
///
/// Returns `(p_recent_change, warm)` per run. Out-of-block runs get their own
/// detector, which is the honest treatment — they are not part of any block.
fn per_block(trace: &Trace, config: BocpdConfig) -> Vec<(f64, bool)> {
    let mut out = Vec::with_capacity(trace.runs.len());
    let mut detector = Bocpd::new(config);
    let mut current = i64::MIN;
    for run in &trace.runs {
        if run[3] != current {
            current = run[3];
            detector = Bocpd::new(config);
        }
        let update = detector
            .observe(RunObservation::new(run[1] as usize, run[2] as f64))
            .expect("a fixture run is well formed");
        out.push((update.p_recent_change, update.warm));
    }
    out
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let position = (q * (sorted.len() - 1) as f64).round() as usize;
    sorted[position.min(sorted.len() - 1)]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Mutual information in bits between two discrete labellings.
fn mutual_information(left: &[usize], right: &[usize]) -> f64 {
    assert_eq!(left.len(), right.len());
    let total = left.len() as f64;
    let mut joint: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut left_marginal: BTreeMap<usize, f64> = BTreeMap::new();
    let mut right_marginal: BTreeMap<usize, f64> = BTreeMap::new();
    for (a, b) in left.iter().zip(right) {
        *joint.entry((*a, *b)).or_default() += 1.0;
        *left_marginal.entry(*a).or_default() += 1.0;
        *right_marginal.entry(*b).or_default() += 1.0;
    }
    let mut information = 0.0;
    for ((a, b), count) in &joint {
        let p = count / total;
        let pa = left_marginal[a] / total;
        let pb = right_marginal[b] / total;
        information += p * (p / (pa * pb)).log2();
    }
    information
}

fn entropy(values: &[usize]) -> f64 {
    let total = values.len() as f64;
    let mut counts: BTreeMap<usize, f64> = BTreeMap::new();
    for value in values {
        *counts.entry(*value).or_default() += 1.0;
    }
    -counts
        .values()
        .map(|count| {
            let p = count / total;
            p * p.log2()
        })
        .sum::<f64>()
}

/// Accuracy of the best "predict the label from the bin" rule: per bin, guess
/// the modal label. The interpretable form of the same question mutual
/// information asks.
fn majority_rule_accuracy(bins: &[usize], labels: &[usize]) -> f64 {
    let mut table: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
    for (bin, label) in bins.iter().zip(labels) {
        *table.entry(*bin).or_default().entry(*label).or_default() += 1;
    }
    let hits: usize = table
        .values()
        .map(|counts| counts.values().copied().max().unwrap_or(0))
        .sum();
    hits as f64 / labels.len() as f64
}

fn marginal_majority_accuracy(labels: &[usize]) -> f64 {
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    for label in labels {
        *counts.entry(*label).or_default() += 1;
    }
    counts.values().copied().max().unwrap_or(0) as f64 / labels.len() as f64
}

/// Circular-shift null for a mutual-information statistic.
///
/// `03-BEHAVIORAL-ENGINE-SPEC.md` § 3.4 requires circular shifts rather than
/// i.i.d. resampling wherever a null is calibrated, because behavioural series
/// are autocorrelated and i.i.d. permutation understates the null. The same
/// reasoning applies here: a Viterbi path is extremely autocorrelated, and
/// shuffling it would make almost any association look significant.
fn circular_shift_null(labels: &[usize], bins: &[usize], shifts: usize) -> Vec<f64> {
    let n = labels.len();
    (1..=shifts)
        .map(|index| {
            let offset = index * n / (shifts + 1);
            let rotated: Vec<usize> = (0..n)
                .map(|position| labels[(position + offset) % n])
                .collect();
            mutual_information(&rotated, bins)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The fixtures are what they claim to be
// ---------------------------------------------------------------------------

#[test]
fn the_segmentation_fixture_is_labelled_and_well_formed() {
    let (header, traces) = load();
    assert_eq!(header.kind, "header");
    assert!(header.synthetic);
    assert!(header.label.starts_with("SYNTHETIC"));
    assert!(header.label.contains("No real user"));
    assert!(header.suite.starts_with("C "), "{}", header.suite);
    assert_eq!(header.schema, "velvt-runs/1");
    assert!(
        header.injection_method.starts_with("NONE"),
        "the suite must state that it does not exercise the ingestion path, so \
         that no result from it is read as evidence about the product: {}",
        header.injection_method
    );
    assert!(header.acceptance.contains("false-alarm"));
    assert_eq!(header.traces, traces.len());
    assert_eq!(
        header.total_runs,
        traces.iter().map(|trace| trace.runs.len()).sum::<usize>()
    );

    for name in ["PLANTED", "NULL", "REGIME", "DRIFTING", "SPARSE"] {
        assert!(
            header.families.iter().any(|entry| entry == name),
            "the fixture is missing the {name} family"
        );
        assert!(!family(&traces, name).is_empty());
    }

    for trace in &traces {
        assert!(!trace.runs.is_empty(), "{}", trace.trace_id);
        let mut previous = i64::MIN;
        for run in &trace.runs {
            assert!(
                run[0] >= previous,
                "{}: run offsets are not monotone",
                trace.trace_id
            );
            previous = run[0];
            assert!(
                (0..8).contains(&run[1]),
                "{}: category index {} is outside the shipped taxonomy",
                trace.trace_id,
                run[1]
            );
            assert!(
                run[2] > 0,
                "{}: a run has non-positive dwell",
                trace.trace_id
            );
        }
        assert!(trace.weeks > 0);
        assert!(trace.seed > 0);

        // Ground truth has to be coherent per family, or every result scored
        // against it is scored against a fixture bug.
        let truth = &trace.ground_truth;
        match trace.family.as_str() {
            "NULL" | "SPARSE" => {
                assert!(!truth.planted, "{}", trace.trace_id);
                assert!(truth.antecedent_days.is_empty(), "{}", trace.trace_id);
                assert!(truth.change_run_index.is_none(), "{}", trace.trace_id);
            }
            "REGIME" => {
                assert!(truth.planted);
                let index = truth.change_run_index.expect("a change run index");
                let at = truth.change_at_seconds.expect("a change instant");
                assert!(index > 0 && index < trace.runs.len(), "{}", trace.trace_id);
                assert!(trace.runs[index][0] >= at, "{}", trace.trace_id);
                assert!(trace.runs[index - 1][0] < at, "{}", trace.trace_id);
            }
            "PLANTED" => {
                assert!(truth.planted);
                let raised = truth.alternating_given_antecedent.expect("an effect size");
                let baseline = truth.alternating_given_none.expect("a baseline");
                assert!((0.0..=1.0).contains(&raised) && raised >= baseline);
            }
            "DRIFTING" => {
                assert!(truth.planted);
                let index = truth.drift_run_index.expect("a drift run index");
                assert!(index > 0 && index < trace.runs.len(), "{}", trace.trace_id);
                assert!(
                    !truth.antecedent_days.is_empty(),
                    "{}: a drifting trace with no antecedent days plants nothing",
                    trace.trace_id
                );
            }
            other => panic!("unregistered family {other}"),
        }

        // Block ground truth must line up with the runs it describes.
        for block in &truth.blocks {
            assert!(block.duration >= 300 && block.duration <= 10_800);
            let counted = trace
                .runs
                .iter()
                .filter(|run| run[3] == block.index as i64)
                .count();
            assert_eq!(counted, block.runs, "{}", trace.trace_id);
            let first = trace
                .runs
                .iter()
                .find(|run| run[3] == block.index as i64)
                .expect("a block with runs");
            assert!(first[0] >= block.start, "{}", trace.trace_id);
            assert!(
                first[0] < block.start + block.duration,
                "{}",
                trace.trace_id
            );
            assert!(
                block.routine == "alternating"
                    || block.routine == "sustained"
                    || block.routine == "unstructured"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Everything here ships in shadow
// ---------------------------------------------------------------------------

/// The shadow guarantee, enforced structurally rather than promised in a
/// comment. If either model acquires a caller anywhere outside its own module
/// and this suite, it can reach a user, and the whole basis for shipping an
/// unvalidated model disappears.
#[test]
fn neither_model_has_a_caller_anywhere_in_the_shipped_path() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![source.clone()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("src is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                continue;
            }
            // The models' own module, and the module that declares them.
            if path.starts_with(source.join("behavior")) {
                continue;
            }
            let body = fs::read_to_string(&path).expect("a source file");
            for (number, line) in body.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                    continue;
                }
                if line.contains("bocpd") || line.contains("hmm::") || line.contains("StickyHmm") {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the shadow models acquired callers in the shipped path:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The null test — the one that matters
// ---------------------------------------------------------------------------

/// The false-alarm rate of the change-point detector on traces containing no
/// structure, at every threshold, reported honestly.
///
/// A change-point detector that fires constantly on noise is useless. The rate
/// is measured and printed rather than asserted to be zero, because the number
/// is the finding.
#[test]
fn null_traces_false_alarm_rate() {
    let (_, traces) = load();
    let null = family(&traces, "NULL");
    let config = BocpdConfig::default();

    let mut scored = 0usize;
    let mut alarms = [0usize; THRESHOLDS.len()];
    let mut block_scored = 0usize;
    let mut block_alarms = [0usize; THRESHOLDS.len()];
    let mut warm_blocks = 0usize;
    let mut total_blocks = 0usize;
    let mut runs_per_block: Vec<f64> = Vec::new();
    let mut floor_hits = 0usize;

    for trace in &null {
        for (index, probability) in continuous(trace, config).iter().enumerate() {
            if index + 1 < bocpd::WARMUP_RUNS {
                continue;
            }
            scored += 1;
            if *probability > config.recent_change_floor() - 1e-9 {
                floor_hits += 1;
            }
            for (slot, threshold) in THRESHOLDS.iter().enumerate() {
                if *probability > *threshold {
                    alarms[slot] += 1;
                }
            }
        }
        for (probability, warm) in per_block(trace, config) {
            if !warm {
                continue;
            }
            block_scored += 1;
            for (slot, threshold) in THRESHOLDS.iter().enumerate() {
                if probability > *threshold {
                    block_alarms[slot] += 1;
                }
            }
        }
        let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
        for run in &trace.runs {
            *counts.entry(run[3]).or_default() += 1;
        }
        for (block, count) in counts {
            if block < 0 {
                continue;
            }
            total_blocks += 1;
            runs_per_block.push(count as f64);
            if count >= bocpd::WARMUP_RUNS {
                warm_blocks += 1;
            }
        }
    }

    runs_per_block.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let weekly = runs_per_user_week(&null);

    println!("\n=== SYNTHETIC — BOCPD on NULL traces (no structure) ===");
    println!(
        "traces {}  runs {}  scored after the {}-run warmup {}  runs/user-week {:.1}",
        null.len(),
        null.iter().map(|t| t.runs.len()).sum::<usize>(),
        bocpd::WARMUP_RUNS,
        scored,
        weekly
    );
    println!(
        "hazard floor on P(run_length < 3) = {:.4}; a threshold at or below it \
         fires on {:.1}% of runs",
        config.recent_change_floor(),
        100.0 * floor_hits as f64 / scored as f64
    );
    println!("threshold   continuous FA/run    false alarms per user-week   per-block FA/run");
    for (slot, threshold) in THRESHOLDS.iter().enumerate() {
        let rate = alarms[slot] as f64 / scored as f64;
        let block_rate = block_alarms[slot] as f64 / block_scored.max(1) as f64;
        println!(
            "  {threshold:>5.2}      {rate:>9.5} ({:>5})           {:>8.2}          {block_rate:>9.5} ({:>4})",
            alarms[slot],
            rate * weekly,
            block_alarms[slot]
        );
    }
    println!(
        "blocks {total_blocks}  median runs/block {:.1}  p90 {:.1}  reaching the {}-run \
         warmup {}/{} ({:.1}%)  per-block scored runs {block_scored}",
        quantile(&runs_per_block, 0.5),
        quantile(&runs_per_block, 0.9),
        bocpd::WARMUP_RUNS,
        warm_blocks,
        total_blocks,
        100.0 * warm_blocks as f64 / total_blocks as f64
    );

    assert!(scored > 5_000, "too few scored runs to report a rate");

    // The floor is not a threshold. Anything at or below it fires on every run
    // of every stream, forever.
    assert_eq!(
        floor_hits, scored,
        "a threshold at the hazard floor fired on {floor_hits} of {scored} runs \
         rather than all of them"
    );

    // The inversion control. A low false-alarm rate at a high threshold means
    // nothing unless the same detector on the same data fires freely at a low
    // one — otherwise a detector that never fires at all would pass.
    let low = alarms[0] as f64 / scored as f64;
    assert!(
        low > 0.5,
        "at threshold {:.2} the detector fired on only {low} of runs, so the \
         rate at high thresholds is not evidence of anything",
        THRESHOLDS[0]
    );

    // And the measured rates, bounded with margin so that a regression in the
    // detector is caught but ordinary numerical drift is not.
    let at = |threshold: f64| {
        let slot = THRESHOLDS
            .iter()
            .position(|value| (value - threshold).abs() < 1e-9)
            .expect("a swept threshold");
        alarms[slot] as f64 / scored as f64
    };
    assert!(
        at(0.90) < 0.02,
        "false-alarm rate at 0.90 rose to {}",
        at(0.90)
    );
    assert!(
        at(0.99) < 0.002,
        "false-alarm rate at 0.99 rose to {}",
        at(0.99)
    );
    assert!(
        at(0.50) > 0.02,
        "the mid-range of the sweep went silent ({}), which would mean the \
         sweep is not measuring a trade-off",
        at(0.50)
    );
}

// ---------------------------------------------------------------------------
// Recovery on REGIME traces
// ---------------------------------------------------------------------------

/// The planted change point, recovered — and scored against the rate at which
/// the same threshold fires by chance on the same traces before the change.
///
/// A detection rate quoted without that comparison is not a result. At a
/// threshold whose pre-change false-alarm rate is 13% per run, "detected
/// within 25 runs" happens 97% of the time on a trace where nothing changed.
#[test]
fn regime_traces_change_point_recovery() {
    let (_, traces) = load();
    let regime = family(&traces, "REGIME");
    let config = BocpdConfig::default();

    println!("\n=== SYNTHETIC — BOCPD on REGIME traces (known change point) ===");
    println!(
        "traces {}  detection window {} runs  change point falls at a DAY boundary, \
         between blocks",
        regime.len(),
        DETECTION_WINDOW_RUNS
    );
    println!(
        "thresh  detected  chance  median lag  p90 lag  median lag from the first  \
         median lag from midnight  pre-change"
    );
    println!(
        "                            (runs)   (runs)   post-change run (min)      \
         on the change day (min)   FA/run"
    );

    let mut summary: BTreeMap<String, (f64, f64, f64)> = BTreeMap::new();

    for threshold in THRESHOLDS {
        let mut lags_runs: Vec<f64> = Vec::new();
        let mut lags_from_run: Vec<f64> = Vec::new();
        let mut lags_from_midnight: Vec<f64> = Vec::new();
        let mut detected = 0usize;
        let mut pre_alarms = 0usize;
        let mut pre_scored = 0usize;

        for trace in &regime {
            let probabilities = continuous(trace, config);
            let change_index = trace.ground_truth.change_run_index.expect("REGIME truth");
            let change_at = trace.ground_truth.change_at_seconds.expect("REGIME truth");

            for (index, probability) in probabilities.iter().enumerate() {
                if index + 1 < bocpd::WARMUP_RUNS || index >= change_index {
                    continue;
                }
                pre_scored += 1;
                if *probability > threshold {
                    pre_alarms += 1;
                }
            }

            if let Some(index) = probabilities
                .iter()
                .enumerate()
                .skip(change_index)
                .take(DETECTION_WINDOW_RUNS)
                .find(|(_, probability)| **probability > threshold)
                .map(|(index, _)| index)
            {
                detected += 1;
                lags_runs.push((index - change_index) as f64);
                lags_from_run
                    .push((trace.runs[index][0] - trace.runs[change_index][0]) as f64 / 60.0);
                lags_from_midnight.push((trace.runs[index][0] - change_at) as f64 / 60.0);
            }
        }

        lags_runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lags_from_run.sort_by(|a, b| a.partial_cmp(b).unwrap());
        lags_from_midnight.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let detection = detected as f64 / regime.len() as f64;
        let pre_rate = pre_alarms as f64 / pre_scored.max(1) as f64;
        // What "detected within the window" would be worth on a trace where
        // nothing changed, at this threshold.
        let chance = 1.0 - (1.0 - pre_rate).powi(DETECTION_WINDOW_RUNS as i32);
        println!(
            " {threshold:>5.2}  {detected:>4}/{:<3}  {chance:>6.3}  {:>9.1} {:>8.1}  \
{:>21.1}  {:>24.1}  {pre_rate:>9.5}",
            regime.len(),
            quantile(&lags_runs, 0.5),
            quantile(&lags_runs, 0.9),
            quantile(&lags_from_run, 0.5),
            quantile(&lags_from_midnight, 0.5),
        );
        summary.insert(
            format!("{threshold:.2}"),
            (detection, chance, quantile(&lags_runs, 0.5)),
        );
    }

    // Per-block mode, which is what `03-BEHAVIORAL-ENGINE-SPEC.md` § 2.1
    // actually specifies. A change that falls at a day boundary is never inside
    // a block, so a detector reset at every block boundary has nothing to see.
    let mut before = (0usize, 0usize);
    let mut after = (0usize, 0usize);
    let mut before_block_runs: Vec<f64> = Vec::new();
    let mut after_block_runs: Vec<f64> = Vec::new();
    for trace in &regime {
        let change_index = trace.ground_truth.change_run_index.expect("REGIME truth");
        let mut counts: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
        for (index, run) in trace.runs.iter().enumerate() {
            if run[3] < 0 {
                continue;
            }
            let entry = counts.entry(run[3]).or_insert((0, index));
            entry.0 += 1;
        }
        for (count, first) in counts.into_values() {
            if first < change_index {
                before_block_runs.push(count as f64);
            } else {
                after_block_runs.push(count as f64);
            }
        }
        for (index, (probability, warm)) in per_block(trace, config).iter().enumerate() {
            if !*warm {
                continue;
            }
            let bucket = if index < change_index {
                &mut before
            } else {
                &mut after
            };
            bucket.1 += 1;
            if *probability > REPORTED_THRESHOLD {
                bucket.0 += 1;
            }
        }
    }
    before_block_runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    after_block_runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "per-block mode at {REPORTED_THRESHOLD}: before the change {}/{} warm runs \
         ({:.4}), after {}/{} ({:.4}). The change is between blocks, so a \
         per-block detector has nothing to detect — and the denominators are \
         wildly asymmetric because the pre-change routine produces a median of \
         {:.0} runs per block against {:.0} after, so most pre-change blocks \
         never reach the {}-run warmup at all.",
        before.0,
        before.1,
        before.0 as f64 / before.1.max(1) as f64,
        after.0,
        after.1,
        after.0 as f64 / after.1.max(1) as f64,
        quantile(&before_block_runs, 0.5),
        quantile(&after_block_runs, 0.5),
        bocpd::WARMUP_RUNS,
    );

    let (detection_50, chance_50, lag_50) = summary["0.50"];
    let (detection_90, chance_90, lag_90) = summary["0.90"];
    println!(
        "at 0.50: detected {detection_50:.3} against chance {chance_50:.3}, median lag \
         {lag_50} runs\nat 0.90: detected {detection_90:.3} against chance \
         {chance_90:.3}, median lag {lag_90} runs"
    );

    assert_eq!(regime.len(), 24);
    // Detection at a permissive threshold must be total, or the detector is not
    // responding to the change at all.
    assert!(
        detection_50 > 0.99,
        "only {detection_50} of REGIME traces crossed 0.50 within {} runs of the \
         planted change",
        DETECTION_WINDOW_RUNS
    );
    // And when it does fire, it fires promptly — the transient is three runs
    // wide, so a lag beyond that is a different event, not a late detection.
    for (threshold, (detection, _, lag)) in &summary {
        if *detection >= 0.5 {
            assert!(
                *lag <= 3.0,
                "median detection lag at threshold {threshold} was {lag} runs"
            );
        }
    }
    // The honest half: at a threshold strict enough to keep the null rate low,
    // detection is not much better than chance. Asserted so that a change which
    // silently improves or destroys it shows up as a test failure to be
    // re-reported, not as a number nobody re-read.
    assert!(
        detection_90 < 0.75,
        "detection at 0.90 rose to {detection_90}; the reported trade-off is \
         stale and the report must be re-derived"
    );
    assert!(chance_90 > 0.10, "chance detection at 0.90 was {chance_90}");
}

// ---------------------------------------------------------------------------
// HMM state stability — spec § 8, failure mode 1
// ---------------------------------------------------------------------------

/// `03-BEHAVIORAL-ENGINE-SPEC.md` § 8 failure mode 1, tested rather than
/// assumed against: *the HMM states are not interpretable — they may split on
/// time-of-day rather than behaviour. Detect: cluster state occupancy by hour;
/// if state identity is predictable from hour alone, the model fitted the
/// clock.*
///
/// The PLANTED generator makes this falsifiable on purpose. It applies a
/// time-of-day confound to dwell — `1 + 0.30 cos(2 pi (h - 10) / 24)`,
/// independent of routine — so there is a clock signal available for the model
/// to latch onto if it is going to. A generator with no time structure at all
/// would make the diagnostic unfalsifiable and the pass meaningless.
///
/// Two statistics, because neither alone is enough:
///
/// - `MI(state; hour)` against a **circular-shift null**. A Viterbi path is
///   heavily autocorrelated and the hour bin is periodic, so the two share
///   mutual information for reasons that have nothing to do with the model.
///   Rotating the path preserves that autocorrelation and destroys the
///   alignment, which is the right null. An i.i.d. shuffle would understate it
///   and make almost anything look significant.
/// - The **majority-rule accuracy**: predict the state from the hour bin alone,
///   against the accuracy of always guessing the most common state. This is the
///   spec's sentence read literally.
#[test]
fn hmm_states_are_not_merely_the_clock() {
    let (_, traces) = load();
    let planted: Vec<&Trace> = family(&traces, "PLANTED")
        .into_iter()
        .filter(|trace| {
            trace
                .ground_truth
                .alternating_given_antecedent
                .is_some_and(|value| value > 0.65)
        })
        .collect();
    assert!(!planted.is_empty(), "no large-effect PLANTED traces");

    println!("\n=== SYNTHETIC — sticky HMM on PLANTED traces, large planted effect ===");
    println!(
        "S = {STATE_COUNT}, kappa = {}, category alpha = {}, {} restarts, seed = the trace's own",
        hmm::DEFAULT_STICKINESS_KAPPA,
        hmm::DEFAULT_CATEGORY_ALPHA,
        HmmConfig::default().restarts
    );
    println!(
        "trace             runs iters conv  occupancy         ln-dwell means    H(state)  \
MI(s;routine)  MI(s;hour)  shift-null p90  hour-rule  base"
    );

    let mut routine_information = Vec::new();
    let mut hour_information = Vec::new();
    let mut hour_excess = Vec::new();
    let mut hour_rule_lift = Vec::new();
    let mut converged = 0usize;

    for trace in &planted {
        let categories = trace.categories();
        let dwell = trace.log_dwell();
        let hours = trace.local_hours();

        let fitted = hmm::fit(
            &categories,
            &dwell,
            &HmmConfig {
                seed: trace.seed,
                ..HmmConfig::default()
            },
        )
        .expect("a PLANTED trace carries enough history to fit");
        if fitted.converged {
            converged += 1;
        }

        // Ground-truth routine per run, taken from the block the run sits in.
        // Out-of-block runs — the planted antecedent itself — carry no routine
        // and are excluded rather than assigned one.
        let mut routine_of_block: BTreeMap<i64, usize> = BTreeMap::new();
        for block in &trace.ground_truth.blocks {
            routine_of_block.insert(
                block.index as i64,
                usize::from(block.routine == "alternating"),
            );
        }
        let mut states = Vec::new();
        let mut routines = Vec::new();
        let mut in_block_hours = Vec::new();
        for (position, run) in trace.runs.iter().enumerate() {
            let Some(routine) = routine_of_block.get(&run[3]) else {
                continue;
            };
            states.push(fitted.states[position]);
            routines.push(*routine);
            in_block_hours.push(hours[position]);
        }

        let routine_mi = mutual_information(&states, &routines);
        let hour_mi = mutual_information(&states, &in_block_hours);
        let mut null = circular_shift_null(&states, &in_block_hours, 200);
        null.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let null_p90 = quantile(&null, 0.9);
        let hour_rule = majority_rule_accuracy(&in_block_hours, &states);
        let base = marginal_majority_accuracy(&states);
        let state_entropy = entropy(&states);

        println!(
            "{:<17} {:>4} {:>5} {:<5} [{:.2} {:.2} {:.2}]  [{:.2} {:.2} {:.2}]  \
{state_entropy:>7.3}  {routine_mi:>12.4}  {hour_mi:>10.4}  {null_p90:>14.4}  \
{hour_rule:>9.3}  {base:>5.3}",
            trace.trace_id,
            fitted.runs,
            fitted.iterations,
            fitted.converged,
            fitted.occupancy[0],
            fitted.occupancy[1],
            fitted.occupancy[2],
            fitted.model.log_dwell_mean[0],
            fitted.model.log_dwell_mean[1],
            fitted.model.log_dwell_mean[2],
        );

        routine_information.push(routine_mi);
        hour_information.push(hour_mi);
        hour_excess.push(hour_mi - null_p90);
        hour_rule_lift.push(hour_rule - base);
    }

    let mean_routine = mean(&routine_information);
    let mean_hour = mean(&hour_information);
    let mean_excess = mean(&hour_excess);
    let mean_lift = mean(&hour_rule_lift);
    println!(
        "\nfits {} of which converged {}\nmean MI(state;routine) {mean_routine:.4} bits   \
mean MI(state;hour) {mean_hour:.4} bits   mean excess over the circular-shift null p90 \
{mean_excess:+.4}   mean hour-rule accuracy lift {mean_lift:+.4}\nMI is in bits against a \
state-entropy ceiling of log2({STATE_COUNT}) = {:.3}",
        planted.len(),
        converged,
        (STATE_COUNT as f64).log2()
    );

    // The states must track the planted routine much more strongly than the
    // clock. This is the whole diagnostic.
    assert!(
        mean_routine > 2.5 * mean_hour,
        "the states carry {mean_routine:.4} bits about the planted routine and \
         {mean_hour:.4} about the hour — not a wide enough margin to say the \
         model is tracking behaviour rather than the clock"
    );
    // And the hour association must not exceed what a rotation of the same path
    // produces, which is the honest null for an autocorrelated label sequence.
    assert!(
        mean_excess < 0.05,
        "MI(state;hour) exceeded its circular-shift null by {mean_excess:.4} bits, \
         which is the signature of a model that fitted the clock"
    );
    assert!(
        mean_lift < 0.20,
        "predicting the state from the hour alone beat the marginal baseline by \
         {mean_lift:.3}"
    );
    assert!(
        converged * 4 >= planted.len() * 3,
        "only {converged} of {} fits converged",
        planted.len()
    );
}

// ---------------------------------------------------------------------------
// Abstention
// ---------------------------------------------------------------------------

#[test]
fn sparse_traces_abstain_with_a_stated_reason() {
    let (_, traces) = load();
    let sparse = family(&traces, "SPARSE");
    assert!(!sparse.is_empty());

    println!("\n=== SYNTHETIC — abstention on SPARSE traces ===");
    let mut runs: Vec<f64> = Vec::new();
    for trace in &sparse {
        let outcome = hmm::fit(
            &trace.categories(),
            &trace.log_dwell(),
            &HmmConfig::default(),
        );
        runs.push(trace.runs.len() as f64);
        match outcome {
            Err(HmmAbstention::InsufficientRuns { runs, required }) => {
                assert_eq!(runs, trace.runs.len());
                assert_eq!(required, hmm::MIN_RUNS_TO_FIT);
            }
            other => panic!("{}: expected abstention, got {other:?}", trace.trace_id),
        }
    }
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "traces {}  runs per trace: min {} median {} max {}  (MIN_RUNS_TO_FIT = {})",
        sparse.len(),
        runs[0],
        quantile(&runs, 0.5),
        runs[runs.len() - 1],
        hmm::MIN_RUNS_TO_FIT
    );

    // And the reason is distinguishable from a failure, which is `T11` at the
    // model layer.
    let insufficient = HmmAbstention::InsufficientRuns {
        runs: 7,
        required: hmm::MIN_RUNS_TO_FIT,
    };
    assert_ne!(insufficient, HmmAbstention::DegenerateLikelihood);
    assert!(insufficient.to_string().contains("insufficient history"));
}
