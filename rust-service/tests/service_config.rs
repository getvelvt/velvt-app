use std::sync::Mutex;
use velvt_service::config::ServiceConfig;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn socket_path_and_log_level_are_configurable() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("VELVT_IPC_SOCKET_PATH", "/tmp/velvt-test.sock");
    std::env::set_var("VELVT_IPC_MAX_ERRORS", "7");
    std::env::set_var("VELVT_LOG_LEVEL", "debug");

    let config = ServiceConfig::load().unwrap();

    assert_eq!(config.socket_path.to_string_lossy(), "/tmp/velvt-test.sock");
    assert_eq!(config.ipc_max_errors, 7);
    assert_eq!(config.log_level, "debug");

    std::env::remove_var("VELVT_IPC_SOCKET_PATH");
    std::env::remove_var("VELVT_IPC_MAX_ERRORS");
    std::env::remove_var("VELVT_LOG_LEVEL");
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
