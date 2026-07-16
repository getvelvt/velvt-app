use std::{
    collections::HashMap,
    sync::{Arc, Barrier, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};
use velvt_service::abstraction::{
    ClassificationConfidence, ClassificationPlugin, ClassificationStatus, ClassificationTier,
    EmbeddingError, EmbeddingMetrics, EmbeddingModel, EmbeddingSimilarityPlugin,
};

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

#[test]
fn tier2_p95_is_under_twenty_five_milliseconds_with_available_model() {
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
    .unwrap();
    let mut samples = Vec::with_capacity(500);
    for _ in 0..500 {
        let started = Instant::now();
        assert!(plugin.classify("Unknown IDE", "private title").is_some());
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[249];
    let p95 = samples[474];
    let p99 = samples[494];
    eprintln!("Tier 2 fake-model p50={p50:?} p95={p95:?} p99={p99:?}");
    assert!(p95 < Duration::from_millis(25), "Tier 2 p95 was {p95:?}");
}

#[cfg(feature = "onnx")]
#[test]
fn real_model_is_deterministic_and_p95_is_under_twenty_five_milliseconds_when_available() {
    use std::path::PathBuf;
    use velvt_service::abstraction::{CategoryCentroids, OrtEmbeddingModel};

    let (Ok(model_path), Ok(centroid_path)) = (
        std::env::var("VELVT_ABSTRACTION_MODEL_PATH"),
        std::env::var("VELVT_ABSTRACTION_CENTROIDS_PATH"),
    ) else {
        eprintln!("real-model benchmark skipped: model artifacts are not configured");
        return;
    };
    let centroids = CategoryCentroids::from_path(PathBuf::from(centroid_path)).unwrap();
    let plugin = EmbeddingSimilarityPlugin::new(
        Arc::new(OrtEmbeddingModel::load(&PathBuf::from(model_path)).unwrap()),
        centroids.into_vectors(),
        "mvp-1",
        0.72,
        Duration::from_millis(20),
        Arc::new(EmbeddingMetrics::default()),
    )
    .unwrap();
    let first = plugin.classify("Unknown IDE", "private title");
    let second = plugin.classify("Unknown IDE", "private title");
    assert_eq!(first, second);

    let mut samples = Vec::with_capacity(500);
    for _ in 0..500 {
        let started = Instant::now();
        let _ = plugin.classify("Unknown IDE", "private title");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[249];
    let p95 = samples[474];
    let p99 = samples[494];
    eprintln!("Tier 2 real-model p50={p50:?} p95={p95:?} p99={p99:?}");
    assert!(p95 < Duration::from_millis(25), "Tier 2 p95 was {p95:?}");
}
