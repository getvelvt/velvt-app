use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use uuid::Uuid;
use velvt_service::abstraction::Taxonomy;

fn service_output(env: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_velvt-service"));
    command.env("VELVT_LOG_LEVEL", "info");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
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
