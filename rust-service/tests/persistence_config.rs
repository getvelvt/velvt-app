use std::sync::Mutex;
use velvt_service::config::ServiceConfig;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn database_path_is_configurable_and_supports_memory_mode() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("VELVT_DATABASE_PATH", ":memory:");

    let config = ServiceConfig::load().unwrap();

    assert_eq!(config.database_path.to_string_lossy(), ":memory:");
    std::env::remove_var("VELVT_DATABASE_PATH");
}
