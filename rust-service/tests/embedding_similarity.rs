use std::{
    collections::HashMap,
    sync::{Arc, Barrier, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
use velvt_service::abstraction::{
    ClassificationConfidence, ClassificationPlugin, ClassificationStatus, ClassificationTier,
    EmbeddingError, EmbeddingMetrics, EmbeddingModel, EmbeddingSalt, EmbeddingSimilarityPlugin,
    HashedEmbeddingModel, PersonalSemanticPrototype, SemanticLearningStore, StoreError,
};

/// A fixed salt, so every assertion about the hashed model below is about the
/// model rather than about which salt the run happened to get. On a real
/// install this is `randomblob(32)` from migration 0031 and differs per device.
const TEST_SALT: EmbeddingSalt = EmbeddingSalt::from_bytes([7; EmbeddingSalt::LENGTH]);

fn hashed_model() -> HashedEmbeddingModel {
    HashedEmbeddingModel::new(TEST_SALT)
}

struct PersonalStore(Vec<PersonalSemanticPrototype>);

impl SemanticLearningStore for PersonalStore {
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

struct FakeEmbeddingModel {
    embedding: Vec<f32>,
    delay: Duration,
}

impl EmbeddingModel for FakeEmbeddingModel {
    fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
        thread::sleep(self.delay);
        Ok(self.embedding.clone())
    }
}

fn centroids() -> HashMap<String, Vec<f32>> {
    HashMap::from([
        ("FOCUS_WORK".to_owned(), vec![1.0, 0.0]),
        ("PASSIVE_CONSUMPTION".to_owned(), vec![0.0, 1.0]),
    ])
}

#[test]
fn high_similarity_unknown_app_is_classified() {
    let metrics = Arc::new(EmbeddingMetrics::default());
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![1.0, 0.0],
            delay: Duration::ZERO,
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        metrics,
    )
    .unwrap();

    let result = plugin.classify("Unknown IDE", "private title").unwrap();
    assert_eq!(result.category(), "FOCUS_WORK");
    assert_eq!(result.label(), "document:inferred");
    assert_eq!(result.tier(), ClassificationTier::EmbeddingSimilarity);
}

#[test]
fn similarity_equal_to_threshold_is_included() {
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![0.72, 0.693_974],
            delay: Duration::ZERO,
        }),
        HashMap::from([("FOCUS_WORK".to_owned(), vec![1.0, 0.0])]),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    assert!(plugin.classify("Unknown IDE", "private title").is_some());
}

#[test]
fn below_threshold_similarity_returns_none() {
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![0.0, 1.0],
            delay: Duration::ZERO,
        }),
        HashMap::from([("FOCUS_WORK".to_owned(), vec![1.0, 0.0])]),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    assert!(plugin.classify("Unknown IDE", "private title").is_none());
}

struct RecordingModel {
    inputs: Arc<Mutex<Vec<String>>>,
}

impl EmbeddingModel for RecordingModel {
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.inputs.lock().unwrap().push(input.to_owned());
        Ok(vec![1.0, 0.0])
    }
}

#[test]
fn empty_window_title_runs_inference_on_app_name_alone() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(RecordingModel {
            inputs: Arc::clone(&inputs),
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    assert!(plugin.classify("Unknown IDE", "").is_some());
    assert_eq!(inputs.lock().unwrap().as_slice(), ["unknown ide"]);
}

#[test]
fn inferred_label_preserves_the_selected_category() {
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![0.0, 1.0],
            delay: Duration::ZERO,
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    let result = plugin.classify("Unknown", "Video Player").unwrap();

    assert_eq!(result.category(), "PASSIVE_CONSUMPTION");
    assert_eq!(result.label(), "video:inferred");
}

#[test]
fn multiple_prototypes_preserve_distinct_category_modes() {
    let plugin = EmbeddingSimilarityPlugin::new_with_prototypes(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![0.0, 1.0],
            delay: Duration::ZERO,
        }),
        HashMap::from([
            (
                "FOCUS_WORK".to_owned(),
                vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            ),
            ("REFERENCE".to_owned(), vec![vec![0.8, 0.6]]),
        ]),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    let result = plugin.classify("Unknown", "Private title").unwrap();
    assert_eq!(result.category(), "FOCUS_WORK");
}

#[test]
fn one_correction_cannot_drift_beyond_the_personal_similarity_radius() {
    let input = vec![0.89, 0.455_96];
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: input.clone(),
            delay: Duration::ZERO,
        }),
        HashMap::from([("REFERENCE".to_owned(), input)]),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap()
    .with_learning_store(Arc::new(PersonalStore(vec![PersonalSemanticPrototype {
        category: "COMMUNICATION".into(),
        embedding: vec![1.0, 0.0],
        weight: 1.0,
    }])));

    let result = plugin.classify("Unknown", "unseen context").unwrap();
    assert_eq!(result.category(), "REFERENCE");
    assert_ne!(result.source().as_str(), "user_rule");
}

#[test]
fn conflicting_personal_prototypes_abstain_instead_of_drifting() {
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![1.0, 0.0],
            delay: Duration::ZERO,
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap()
    .with_learning_store(Arc::new(PersonalStore(vec![
        PersonalSemanticPrototype {
            category: "COMMUNICATION".into(),
            embedding: vec![1.0, 0.0],
            weight: 1.0,
        },
        PersonalSemanticPrototype {
            category: "REFERENCE".into(),
            embedding: vec![1.0, 0.0],
            weight: 1.0,
        },
    ])));

    let result = plugin.classify("Unknown", "ambiguous context").unwrap();
    assert_eq!(result.status(), ClassificationStatus::Ambiguous);
    assert_eq!(result.category(), "UNLOGGED");
}

#[test]
fn builtin_classifier_handles_unknown_tools_with_semantic_context() {
    let plugin = EmbeddingSimilarityPlugin::builtin("mvp-1").unwrap();
    let focus = plugin
        .classify("Nova", "programming workspace code editor")
        .unwrap();
    assert_eq!(focus.category(), "FOCUS_WORK");
    let communication = plugin
        .classify("Relay", "team chat messaging conversation")
        .unwrap();
    assert_eq!(communication.category(), "COMMUNICATION");
}

#[test]
fn builtin_embedding_bounds_the_work_it_does_on_any_input() {
    // What keeps this fast is that the work is bounded, not that the machine is
    // idle: HashedEmbeddingModel truncates to 4096 characters and then hashes
    // at most 128 tokens. Both bounds are observable in the output, so they can
    // be asserted deterministically. The elapsed-time ceiling this replaces
    // measured the load on the runner and failed under a concurrent build.
    //
    // This is the input half of that ceiling and the whole of what a clock-free
    // assertion can carry. It is not the whole of the per-call cost, and saying
    // so would be wrong: `classify` normalizes and SHA-256s the untruncated
    // app-and-title into a cache key before `embed` ever truncates, so Tier 2
    // still costs more on a longer title. The timing half is
    // `builtin_tier2_median_is_under_ten_milliseconds` below, which times the
    // model that ships.
    let embedding = hashed_model()
        .embed(&"programming ".repeat(10_000))
        .unwrap();
    assert_eq!(embedding.len(), 256);
    assert!(embedding.iter().all(|value| value.is_finite()));

    // The token bound: everything past the 128th token is not hashed at all.
    let bounded_tokens = hashed_model()
        .embed(&format!(
            "{}{}",
            "programming ".repeat(128),
            "streaming ".repeat(10_000)
        ))
        .unwrap();
    assert_eq!(
        bounded_tokens,
        hashed_model().embed(&"programming ".repeat(128)).unwrap()
    );
    // And the same input one token short does see the 128th token, so the
    // equality above is a bound rather than an insensitivity to the tail.
    assert_ne!(
        bounded_tokens,
        hashed_model()
            .embed(&format!("{}streaming ", "programming ".repeat(127)))
            .unwrap()
    );

    // The character bound, on an input with no token boundary to stop at.
    assert_eq!(
        hashed_model().embed(&"x".repeat(10_000)).unwrap(),
        hashed_model().embed(&"x".repeat(4_096)).unwrap()
    );
    assert_ne!(
        hashed_model().embed(&"x".repeat(10_000)).unwrap(),
        hashed_model().embed(&"x".repeat(4_095)).unwrap()
    );
}

#[test]
fn equal_similarity_abstains_deterministically() {
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![1.0, 0.0],
            delay: Duration::ZERO,
        }),
        HashMap::from([
            ("REFERENCE".to_owned(), vec![1.0, 0.0]),
            ("COMMUNICATION".to_owned(), vec![1.0, 0.0]),
        ]),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();

    for _ in 0..20 {
        let result = plugin.classify("Unknown", "Unknown").unwrap();
        assert_eq!(result.category(), "UNLOGGED");
        assert_eq!(result.label(), "unlogged");
        assert_eq!(result.status(), ClassificationStatus::Ambiguous);
        assert_eq!(result.confidence(), ClassificationConfidence::Low);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_inference_has_no_state_corruption() {
    let plugin = Arc::new(
        EmbeddingSimilarityPlugin::new(
            Arc::new(FakeEmbeddingModel {
                embedding: vec![1.0, 0.0],
                delay: Duration::ZERO,
            }),
            centroids(),
            "mvp-1",
            0.72,
            Duration::from_millis(100),
            Arc::new(EmbeddingMetrics::default()),
        )
        .unwrap(),
    );
    let barrier = Arc::new(Barrier::new(8));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let plugin = Arc::clone(&plugin);
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            plugin.classify("Unknown IDE", "private title")
        }));
    }
    let mut matches = 0;
    for task in tasks {
        if let Some(result) = task.await.unwrap() {
            assert_eq!(result.category(), "FOCUS_WORK");
            matches += 1;
        }
    }
    assert!(matches > 0);
}

#[test]
fn slow_inference_times_out_and_increments_metric() {
    struct BlockingEmbeddingModel {
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl EmbeddingModel for BlockingEmbeddingModel {
        fn embed(&self, _input: &str) -> Result<Vec<f32>, EmbeddingError> {
            let (lock, signal) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = signal.wait(released).unwrap();
            }
            Ok(vec![1.0, 0.0])
        }
    }

    let metrics = Arc::new(EmbeddingMetrics::default());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(BlockingEmbeddingModel {
            release: Arc::clone(&release),
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::clone(&metrics),
    )
    .unwrap();

    assert!(plugin.classify("Unknown IDE", "private title").is_none());
    assert_eq!(metrics.tier2_timeout_count(), 1);

    let (lock, signal) = &*release;
    *lock.lock().unwrap() = true;
    signal.notify_all();
}

// Two Tier 2 latency bounds, in two places, for two different reasons.
//
// The median runs in the correctness suite, on every pull request.
// PERFORMANCE_REPORT.md publishes a Tier 2 p50 gate of 10 ms against a
// measured 21.6 µs and nothing enforced it; the gate is several hundred times
// the observed cost, which is the point. Runner load cannot reach it, because
// moving a median needs half of 500 samples to be slow rather than a few. What
// a bound that loose still catches is anything that puts ten milliseconds into
// a call that costs tens of microseconds: a model loaded per call, a taxonomy
// parsed per call, an artifact read from disk per call.
//
// The p95 is #[ignore]d and runs in `make bench-rust`, which CI runs in the
// `bench` job on pushes to develop and main and on demand. A 25 ms wall-clock
// tail bound on a shared runner fails under a busy neighbour about as readily
// as under a regression, and a required check that goes red for reasons
// unrelated to the change teaches everyone to re-run it. The cost of putting it
// there, stated so nobody has to rediscover it: a Tier 2 tail regression is
// caught on develop within one merge rather than before it lands.
fn fake_model_plugin() -> EmbeddingSimilarityPlugin {
    EmbeddingSimilarityPlugin::new(
        Arc::new(FakeEmbeddingModel {
            embedding: vec![1.0, 0.0],
            delay: Duration::ZERO,
        }),
        centroids(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap()
}

/// p50 and p95 over 500 timed calls, with the whole distribution printed under
/// `label` so a failing run shows where the samples sat and not only which
/// percentile tripped. The suite bound and the bench bound both read from this,
/// so they cannot drift apart in sample count or method.
fn tier2_percentiles(label: &str, mut classify_once: impl FnMut()) -> (Duration, Duration) {
    let mut samples = Vec::with_capacity(500);
    for _ in 0..500 {
        let started = Instant::now();
        classify_once();
        samples.push(started.elapsed());
    }
    samples.sort();
    let (p50, p95, p99) = (samples[249], samples[474], samples[494]);
    eprintln!("Tier 2 {label} p50={p50:?} p95={p95:?} p99={p99:?}");
    (p50, p95)
}

#[test]
fn tier2_median_is_under_ten_milliseconds_with_available_model() {
    let plugin = fake_model_plugin();
    let (p50, _) = tier2_percentiles("fake-model", || {
        assert!(plugin.classify("Unknown IDE", "private title").is_some());
    });
    assert!(p50 < Duration::from_millis(10), "Tier 2 p50 was {p50:?}");
}

#[test]
#[ignore = "wall-clock tail; runs in make bench-rust, which CI runs in the bench job"]
fn tier2_p95_is_under_twenty_five_milliseconds_with_available_model() {
    let plugin = fake_model_plugin();
    let (_, p95) = tier2_percentiles("fake-model", || {
        assert!(plugin.classify("Unknown IDE", "private title").is_some());
    });
    assert!(p95 < Duration::from_millis(25), "Tier 2 p95 was {p95:?}");
}

/// The same median bound, against the model that actually ships.
///
/// The two tests above drive a zero-delay stand-in, so they bound the plugin's
/// own overhead and nothing about an embedding. `main.rs` reads the device salt
/// and builds the Tier 2 plugin through `builtin_salted`, so
/// `HashedEmbeddingModel` is the Tier 2 model on every install without ONNX
/// artifacts -- all of them today. Without this test, nothing anywhere times
/// the code that runs.
///
/// An ordinary window title on purpose. Per-call cost is not independent of
/// title length: `classify` hashes the untruncated app-and-title into a cache
/// key before `embed` truncates to 4096 characters, and the 120,000-character
/// input the deleted `builtin_embedding_bounds_input_and_latency` assertion
/// used costs about fifty times this at the median in a debug build. That input
/// is covered by `builtin_embedding_bounds_the_work_it_does_on_any_input`,
/// which asserts both ceilings by value and needs no clock to do it.
#[test]
fn builtin_tier2_median_is_under_ten_milliseconds() {
    let plugin = EmbeddingSimilarityPlugin::builtin_salted("mvp-1", TEST_SALT).unwrap();
    let (p50, _) = tier2_percentiles("builtin-model", || {
        let _ = plugin.classify("Unknown IDE", "programming project notes");
    });
    assert!(
        p50 < Duration::from_millis(10),
        "Tier 2 builtin p50 was {p50:?}"
    );
}

// Returns None, having said why, when the model artifacts are not configured.
// They are not in the repository, so this is the ordinary case everywhere but a
// machine that has downloaded them.
#[cfg(feature = "onnx")]
fn real_model_plugin() -> Option<EmbeddingSimilarityPlugin> {
    use std::path::PathBuf;
    use velvt_service::abstraction::{CategoryCentroids, OrtEmbeddingModel};

    let (Ok(model_path), Ok(centroid_path)) = (
        std::env::var("VELVT_ABSTRACTION_MODEL_PATH"),
        std::env::var("VELVT_ABSTRACTION_CENTROIDS_PATH"),
    ) else {
        eprintln!("real-model test skipped: model artifacts are not configured");
        return None;
    };
    let centroids = CategoryCentroids::from_path(PathBuf::from(centroid_path)).unwrap();
    Some(
        EmbeddingSimilarityPlugin::new(
            Arc::new(OrtEmbeddingModel::load(&PathBuf::from(model_path)).unwrap()),
            centroids.into_vectors(),
            "mvp-1",
            0.72,
            Duration::from_millis(20),
            Arc::new(EmbeddingMetrics::default()),
        )
        .unwrap(),
    )
}

// Determinism is a correctness property and carries no clock, so it stays in
// the suite; the p95 measurement it used to share a test with is below, under
// the same #[ignore] as the fake-model tail.
#[cfg(feature = "onnx")]
#[test]
fn real_model_is_deterministic_when_available() {
    let Some(plugin) = real_model_plugin() else {
        return;
    };
    let first = plugin.classify("Unknown IDE", "private title");
    let second = plugin.classify("Unknown IDE", "private title");
    assert_eq!(first, second);
}

// This one has no in-suite median counterpart, because there is nothing for it
// to run on: `make test-rust` does not enable the `onnx` feature, and the model
// artifacts are not in the repository, so it returns early on every CI machine
// either way. That is the whole reason ARCHITECTURE.md's Tier 2 row records the
// measurement as fake-model with real-model latency not independently verified.
#[cfg(feature = "onnx")]
#[test]
#[ignore = "wall-clock tail; runs in make bench-rust, which CI runs in the bench job"]
fn real_model_p95_is_under_twenty_five_milliseconds_when_available() {
    let Some(plugin) = real_model_plugin() else {
        return;
    };
    let (_, p95) = tier2_percentiles("real-model", || {
        let _ = plugin.classify("Unknown IDE", "private title");
    });
    assert!(p95 < Duration::from_millis(25), "Tier 2 p95 was {p95:?}");
}

/// The salt has to move the vectors without moving the classifier.
///
/// Same salt, same input, same vector: a sketch cached before a restart has to
/// stay comparable with one computed after it, or the cache is worse than no
/// cache. Different salt, different vector: otherwise the salt does nothing,
/// and the dictionary attack that recovered 1,190 real words from a live
/// `semantic_embedding_cache` still runs from this file alone.
#[test]
fn the_salt_moves_the_vectors_without_moving_the_classifier() {
    let input = "Chrome [SEP] divorce attorney consultation booking";
    let mine = hashed_model().embed(input).unwrap();

    assert_eq!(mine, hashed_model().embed(input).unwrap());
    assert_eq!(
        mine,
        HashedEmbeddingModel::new(TEST_SALT).embed(input).unwrap()
    );

    let another_install =
        HashedEmbeddingModel::new(EmbeddingSalt::from_bytes([9; EmbeddingSalt::LENGTH]))
            .embed(input)
            .unwrap();
    assert_eq!(mine.len(), another_install.len());
    assert_ne!(mine, another_install);

    // The unsalted space is a third space, which is why migration 0031 empties
    // the rows written under it rather than carrying them forward.
    assert_ne!(
        mine,
        HashedEmbeddingModel::new(EmbeddingSalt::UNSALTED)
            .embed(input)
            .unwrap()
    );

    // The classifier is unchanged by any of it. The seed prototypes are
    // embedded under the same salt as the observation they are compared with,
    // so the geometry that decides a category is the geometry it always was.
    let salted = EmbeddingSimilarityPlugin::builtin_salted("mvp-1", TEST_SALT).unwrap();
    assert_eq!(
        salted
            .classify("Nova", "programming workspace code editor")
            .unwrap()
            .category(),
        "FOCUS_WORK"
    );
    assert_eq!(
        salted
            .classify("Relay", "team chat messaging conversation")
            .unwrap()
            .category(),
        "COMMUNICATION"
    );
}

/// Migration 0031 has to leave exactly two things behind: a salt this install
/// alone holds, and no vector computed before that salt existed.
#[test]
fn migration_0031_mints_a_per_install_salt_and_empties_both_vector_stores() {
    fn rows(connection: &rusqlite::Connection, table: &str) -> i64 {
        connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    // The two stores as 0013 left them, holding vectors from the unsalted space.
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../migrations/0013_personal_semantic_learning.sql"
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO semantic_embedding_cache(key_hash, embedding, dimensions)
             VALUES (?1, X'0102', 256)",
            [&"a".repeat(64)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO personal_semantic_prototype(
                key_hash, category, embedding, dimensions, correction_count
             ) VALUES (?1, 'FOCUS_WORK', X'0102', 256, 4)",
            [&"b".repeat(64)],
        )
        .unwrap();

    connection
        .execute_batch(include_str!("../migrations/0031_embedding_salt.sql"))
        .unwrap();

    let salt: Vec<u8> = connection
        .query_row("SELECT salt FROM embedding_salt", [], |row| row.get(0))
        .unwrap();
    assert_eq!(salt.len(), EmbeddingSalt::LENGTH);
    assert!(
        salt.iter().any(|byte| *byte != 0),
        "a zero salt is the one every reader of the source already knows"
    );
    assert_eq!(rows(&connection, "semantic_embedding_cache"), 0);
    assert_eq!(rows(&connection, "personal_semantic_prototype"), 0);

    // Per install, not per build: a second database gets a different salt.
    let second = rusqlite::Connection::open_in_memory().unwrap();
    second
        .execute_batch(include_str!(
            "../migrations/0013_personal_semantic_learning.sql"
        ))
        .unwrap();
    second
        .execute_batch(include_str!("../migrations/0031_embedding_salt.sql"))
        .unwrap();
    let other: Vec<u8> = second
        .query_row("SELECT salt FROM embedding_salt", [], |row| row.get(0))
        .unwrap();
    assert_ne!(salt, other);

    // And it is in the shipped migration set, not only in this test.
    assert!(
        velvt_service::persistence::SqlitePersistence::open_in_memory()
            .unwrap()
            .schema_snapshot()
            .unwrap()
            .iter()
            .any(|name| name == "embedding_salt")
    );
}
