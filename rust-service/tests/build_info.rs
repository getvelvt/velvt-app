//! What the shipped binary says it is. The Xcode phase writes `--version` into
//! the bundle's `velvt-service.version`, and scripts/verify_release.sh compares
//! both flags with the app's Info.plist, so these outputs are a contract.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_service(flag: &str) -> String {
    let home = std::env::temp_dir().join(format!("velvt-build-info-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_velvt-service"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("VELVT_") {
            command.env_remove(key);
        }
    }
    // Both flags print and exit before any configuration is read. A binary
    // that does not know the flag starts the whole service instead, so the
    // empty HOME keeps that run away from any real database, and the deadline
    // turns it into a failure rather than a hung suite.
    let mut child = command
        .arg(flag)
        .env("HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            std::fs::remove_dir_all(&home).unwrap();
            panic!("velvt-service {flag} did not exit: the binary does not handle the flag");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    std::fs::remove_dir_all(&home).unwrap();
    assert!(status.success(), "{flag} exited {status:?}");
    stdout.trim().to_owned()
}

#[test]
fn version_flag_prints_the_release_version() {
    assert_eq!(
        run_service("--version"),
        velvt_service::build_info::SERVICE_VERSION
    );
}

#[test]
fn source_commit_flag_prints_the_stamped_commit() {
    assert_eq!(
        run_service("--source-commit"),
        velvt_service::build_info::SOURCE_COMMIT
    );
}
