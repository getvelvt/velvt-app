use std::sync::Mutex;
use velvt_service::config::ServiceConfig;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn socket_path_and_log_level_are_configurable() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("VELVT_IPC_SOCKET_PATH", "/tmp/velvt-test.sock");
    std::env::set_var("VELVT_IPC_MAX_ERRORS", "7");
    std::env::set_var("VELVT_LOG_LEVEL", "debug");
    std::env::set_var(
        "VELVT_ABSTRACTION_TAXONOMY_PATH",
        "/tmp/custom-taxonomy.json",
    );
    std::env::set_var("VELVT_ABSTRACTION_MODEL_PATH", "/tmp/model.onnx");
    std::env::set_var("VELVT_ABSTRACTION_CENTROIDS_PATH", "/tmp/centroids.bin");
    std::env::set_var("VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS", "19");
    std::env::set_var("VELVT_ABSTRACTION_SIMILARITY_THRESHOLD", "0.81");

    let config = ServiceConfig::load().unwrap();

    assert_eq!(config.socket_path.to_string_lossy(), "/tmp/velvt-test.sock");
    assert_eq!(config.ipc_max_errors, 7);
    assert_eq!(config.log_level, "debug");
    assert_eq!(
        config.abstraction_taxonomy_path.to_string_lossy(),
        "/tmp/custom-taxonomy.json"
    );
    assert_eq!(
        config
            .abstraction_model_path
            .as_ref()
            .unwrap()
            .to_string_lossy(),
        "/tmp/model.onnx"
    );
    assert_eq!(config.abstraction_inference_timeout.as_millis(), 19);
    assert_eq!(config.abstraction_similarity_threshold, 0.81);

    std::env::remove_var("VELVT_IPC_SOCKET_PATH");
    std::env::remove_var("VELVT_IPC_MAX_ERRORS");
    std::env::remove_var("VELVT_LOG_LEVEL");
    std::env::remove_var("VELVT_ABSTRACTION_TAXONOMY_PATH");
    std::env::remove_var("VELVT_ABSTRACTION_MODEL_PATH");
    std::env::remove_var("VELVT_ABSTRACTION_CENTROIDS_PATH");
    std::env::remove_var("VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS");
    std::env::remove_var("VELVT_ABSTRACTION_SIMILARITY_THRESHOLD");
}

#[test]
fn zero_ipc_error_threshold_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("VELVT_IPC_MAX_ERRORS", "0");

    assert!(ServiceConfig::load().is_err());

    std::env::remove_var("VELVT_IPC_MAX_ERRORS");
}

#[test]
fn socket_path_defaults_to_the_canonical_proto_value() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("VELVT_IPC_SOCKET_PATH");

    let config = ServiceConfig::load().unwrap();

    assert!(config
        .socket_path
        .ends_with(std::path::Path::new(".velvt/velvt-service.sock")));
}
