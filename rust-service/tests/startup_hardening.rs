use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;
use velvt_service::abstraction::Taxonomy;

/// Spawns the real service binary and captures its startup output.
///
/// The service is a long-running process: when startup fails (a missing
/// taxonomy, a bad database path, ...) it exits early on its own and this
/// returns quickly. When startup succeeds it blocks forever waiting for
/// SIGTERM/SIGINT by design (see `main.rs`), so these tests only care about
/// the logged startup output, not a natural exit — poll briefly for a fast
/// exit, then send SIGTERM and collect whatever was logged before that.
fn service_output(env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_velvt-service"));
    command.env("VELVT_LOG_LEVEL", "info");
    command.env("VELVT_DATABASE_PATH", temp_path("startup.sqlite3"));
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();

    let deadline = Instant::now() + Duration::from_millis(500);
    let exited_on_its_own = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !exited_on_its_own {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
    }
    child.wait_with_output().unwrap()
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("velvt-{name}-{}", Uuid::new_v4()))
}

fn logs(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write_taxonomy(version: &str) -> PathBuf {
    let path = temp_path("taxonomy.json");
    fs::write(
        &path,
        format!(
            r#"{{
                "category_taxonomy_version":"{version}",
                "default_category":"UNLOGGED",
                "categories":["FOCUS_WORK","UNLOGGED"],
                "seed_applications":[
                    {{"app_name_pattern":"VS Code","label":"document:edit","category":"FOCUS_WORK"}}
                ]
            }}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn missing_taxonomy_halts_with_structured_error() {
    let missing = temp_path("missing-taxonomy.json");
    let output = service_output(&[("VELVT_ABSTRACTION_TAXONOMY_PATH", &missing)]);
    let logs = logs(&output);

    assert!(logs.contains("abstraction_taxonomy_load_failed"), "{logs}");
    assert!(logs.contains("service startup halted"), "{logs}");
}

#[test]
fn missing_centroids_disable_tier2_with_structured_warning() {
    let taxonomy = write_taxonomy("mvp-1");
    let model = temp_path("model.onnx");
    fs::write(&model, b"configured model placeholder").unwrap();
    let output = service_output(&[
        ("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy),
        ("VELVT_ABSTRACTION_MODEL_PATH", &model),
    ]);
    let logs = logs(&output);

    assert!(logs.contains("tier2_centroids_unavailable"), "{logs}");
    assert!(!logs.contains("service startup halted"), "{logs}");
}

#[test]
fn taxonomy_version_mismatch_warns_and_uses_configured_version() {
    let taxonomy = write_taxonomy("custom-v2");
    assert_eq!(
        Taxonomy::from_path(&taxonomy).unwrap().version(),
        "custom-v2"
    );
    let output = service_output(&[("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy)]);
    let logs = logs(&output);

    assert!(logs.contains("taxonomy_version_mismatch"), "{logs}");
    assert!(logs.contains("custom-v2"), "{logs}");
    assert!(!logs.contains("service startup halted"), "{logs}");
}

#[test]
fn missing_database_file_is_created_and_migrated_at_startup() {
    let taxonomy = write_taxonomy("mvp-1");
    let database = temp_path("missing-startup.sqlite3");
    assert!(!database.exists());

    let output = service_output(&[
        ("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy),
        ("VELVT_DATABASE_PATH", &database),
    ]);
    let logs = logs(&output);

    assert!(database.exists(), "{logs}");
    assert!(
        !logs.contains("persistence_initialization_failed"),
        "{logs}"
    );
}
