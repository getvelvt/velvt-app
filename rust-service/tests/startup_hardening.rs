use std::{
    fs,
    io::{self, Read},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use uuid::Uuid;
use velvt_service::abstraction::Taxonomy;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(12);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let path = PathBuf::from(format!(
            "/tmp/velvt-startup-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir(&path).expect("failed to create isolated startup test directory");
        Self { path }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Default)]
struct CapturedBytes(Arc<Mutex<Vec<u8>>>);

impl CapturedBytes {
    fn snapshot(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl CapturedOutput {
    fn logs(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    fn diagnostics(&self) -> String {
        format!(
            "child status: {}\nredacted stdout:\n{}\nredacted stderr:\n{}",
            self.status, self.stdout, self.stderr
        )
    }
}

struct ServiceProcess {
    child: Child,
    status: Option<ExitStatus>,
    stdout: CapturedBytes,
    stderr: CapturedBytes,
    stdout_reader: Option<JoinHandle<io::Result<()>>>,
    stderr_reader: Option<JoinHandle<io::Result<()>>>,
    socket: PathBuf,
    redactions: Vec<String>,
}

impl ServiceProcess {
    fn spawn(directory: &TestDirectory, env: &[(&str, &Path)]) -> Self {
        let database = directory.path("startup.sqlite3");
        let socket = directory.path("startup.sock");
        let mut command = Command::new(env!("CARGO_BIN_EXE_velvt-service"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("VELVT_") {
                command.env_remove(key);
            }
        }
        command
            .env("VELVT_LOG_LEVEL", "info")
            .env("VELVT_DATABASE_PATH", &database)
            .env("VELVT_IPC_SOCKET_PATH", &socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut redactions = vec![
            directory.path.to_string_lossy().into_owned(),
            database.to_string_lossy().into_owned(),
            socket.to_string_lossy().into_owned(),
        ];
        for (key, value) in env {
            command.env(key, value);
            redactions.push(value.to_string_lossy().into_owned());
        }

        let mut child = command.spawn().expect("failed to spawn velvt-service");
        let stdout = CapturedBytes::default();
        let stderr = CapturedBytes::default();
        let stdout_reader = capture_pipe(
            child.stdout.take().expect("child stdout was not piped"),
            stdout.clone(),
        );
        let stderr_reader = capture_pipe(
            child.stderr.take().expect("child stderr was not piped"),
            stderr.clone(),
        );

        Self {
            child,
            status: None,
            stdout,
            stderr,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            socket,
            redactions,
        }
    }

    fn wait_for_exit(mut self, expected: &str) -> CapturedOutput {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if self.poll_status().is_some() {
                return self.collect_output();
            }
            if Instant::now() >= deadline {
                panic!("{}", self.timeout_diagnostics(expected));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn wait_until_ready(&mut self, expected: &str) {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if UnixStream::connect(&self.socket).is_ok() {
                return;
            }
            if let Some(status) = self.poll_status() {
                panic!(
                    "child exited before {expected}\n{}",
                    self.diagnostics(Some(status))
                );
            }
            if Instant::now() >= deadline {
                panic!("{}", self.timeout_diagnostics(expected));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn assert_not_ready_for(&mut self, duration: Duration, expected: &str) {
        let deadline = Instant::now() + duration;
        loop {
            if UnixStream::connect(&self.socket).is_ok() {
                panic!(
                    "child became ready while {expected}\n{}",
                    self.diagnostics(None)
                );
            }
            if let Some(status) = self.poll_status() {
                panic!(
                    "child exited while {expected}\n{}",
                    self.diagnostics(Some(status))
                );
            }
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn terminate_and_collect(mut self) -> CapturedOutput {
        self.terminate_and_reap();
        self.collect_output()
    }

    fn poll_status(&mut self) -> Option<ExitStatus> {
        self.update_status()
            .expect("failed to query velvt-service child status")
    }

    fn terminate_and_reap(&mut self) {
        if self.update_status().ok().flatten().is_none() {
            let term_sent = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !term_sent {
                let _ = self.child.kill();
            }

            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            while self.update_status().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(POLL_INTERVAL);
            }
        }

        if self.status.is_none() {
            let _ = self.child.kill();
            self.status = self.child.wait().ok();
        }
    }

    fn update_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self.child.try_wait()?;
        }
        Ok(self.status)
    }

    fn collect_output(&mut self) -> CapturedOutput {
        self.join_readers(true);
        CapturedOutput {
            status: self.status.expect("child status was not collected"),
            stdout: redact(&self.stdout.snapshot(), &self.redactions),
            stderr: redact(&self.stderr.snapshot(), &self.redactions),
        }
    }

    fn join_readers(&mut self, fail_on_error: bool) {
        for reader in [&mut self.stdout_reader, &mut self.stderr_reader] {
            let Some(reader) = reader.take() else {
                continue;
            };
            let result = reader.join();
            if fail_on_error {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => panic!("failed to capture child output: {error}"),
                    Err(_) => panic!("child output capture thread panicked"),
                }
            }
        }
    }

    fn timeout_diagnostics(&mut self, expected: &str) -> String {
        let status = self.poll_status();
        format!(
            "timed out after {STARTUP_TIMEOUT:?} waiting for {expected}\n{}",
            self.diagnostics(status)
        )
    }

    fn diagnostics(&self, status: Option<ExitStatus>) -> String {
        let child_status = status.map_or_else(|| "running".to_owned(), |value| value.to_string());
        format!(
            "child status: {child_status}\nredacted stdout:\n{}\nredacted stderr:\n{}",
            redact(&self.stdout.snapshot(), &self.redactions),
            redact(&self.stderr.snapshot(), &self.redactions)
        )
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        self.terminate_and_reap();
        self.join_readers(false);
    }
}

fn capture_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    captured: CapturedBytes,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            let read = pipe.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            captured
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(&buffer[..read]);
        }
    })
}

fn redact(bytes: &[u8], redactions: &[String]) -> String {
    let mut output = String::from_utf8_lossy(bytes).into_owned();
    for value in redactions.iter().filter(|value| !value.is_empty()) {
        output = output.replace(value, "<redacted>");
    }
    output
}

fn filesystem_sockets_available() -> bool {
    let directory = TestDirectory::new();
    match UnixListener::bind(directory.path("preflight.sock")) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping startup subprocess assertion: filesystem sockets are unavailable");
            false
        }
        Err(error) => panic!("failed to bind startup preflight socket: {error}"),
    }
}

fn write_taxonomy(directory: &TestDirectory, version: &str) -> PathBuf {
    let path = directory.path("taxonomy.json");
    fs::write(
        &path,
        format!(
            r#"{{
                "category_taxonomy_version":"{version}",
                "default_category":"UNLOGGED",
                "categories":["FOCUS_WORK","UNLOGGED"],
                "seed_applications":[
                    {{"app_name_pattern":"SyntheticEditor","label":"document:edit","category":"FOCUS_WORK"}}
                ]
            }}"#
        ),
    )
    .expect("failed to write startup taxonomy fixture");
    path
}

#[test]
fn missing_taxonomy_halts_with_structured_error() {
    if !filesystem_sockets_available() {
        return;
    }
    let directory = TestDirectory::new();
    let missing = directory.path("missing-taxonomy.json");
    let output =
        ServiceProcess::spawn(&directory, &[("VELVT_ABSTRACTION_TAXONOMY_PATH", &missing)])
            .wait_for_exit("the missing-taxonomy startup failure to exit");
    let logs = output.logs();

    assert!(
        logs.contains("abstraction_taxonomy_load_failed"),
        "{}",
        output.diagnostics()
    );
    assert!(
        logs.contains("service startup halted"),
        "{}",
        output.diagnostics()
    );
}

#[test]
fn missing_centroids_disable_tier2_with_structured_warning() {
    if !filesystem_sockets_available() {
        return;
    }
    let directory = TestDirectory::new();
    let taxonomy = write_taxonomy(&directory, "mvp-1");
    let model = directory.path("model.onnx");
    fs::write(&model, b"configured model placeholder").expect("failed to write model placeholder");
    let mut service = ServiceProcess::spawn(
        &directory,
        &[
            ("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy),
            ("VELVT_ABSTRACTION_MODEL_PATH", &model),
        ],
    );
    service.wait_until_ready("the Unix socket to accept connections");
    let output = service.terminate_and_collect();
    let logs = output.logs();

    assert!(
        logs.contains("tier2_centroids_unavailable"),
        "{}",
        output.diagnostics()
    );
    assert!(
        !logs.contains("service startup halted"),
        "{}",
        output.diagnostics()
    );
}

#[test]
fn taxonomy_version_mismatch_warns_and_uses_configured_version() {
    if !filesystem_sockets_available() {
        return;
    }
    let directory = TestDirectory::new();
    let taxonomy = write_taxonomy(&directory, "custom-v2");
    assert_eq!(
        Taxonomy::from_path(&taxonomy).unwrap().version(),
        "custom-v2"
    );
    let mut service = ServiceProcess::spawn(
        &directory,
        &[("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy)],
    );
    service.wait_until_ready("the Unix socket to accept connections");
    let output = service.terminate_and_collect();
    let logs = output.logs();

    assert!(
        logs.contains("taxonomy_version_mismatch"),
        "{}",
        output.diagnostics()
    );
    assert!(logs.contains("custom-v2"), "{}", output.diagnostics());
    assert!(
        !logs.contains("service startup halted"),
        "{}",
        output.diagnostics()
    );
}

#[test]
fn missing_database_file_is_created_and_migrated_at_startup() {
    if !filesystem_sockets_available() {
        return;
    }
    let directory = TestDirectory::new();
    let taxonomy = write_taxonomy(&directory, "mvp-1");
    let database = directory.path("missing-startup.sqlite3");
    assert!(!database.exists());

    let mut service = ServiceProcess::spawn(
        &directory,
        &[
            ("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy),
            ("VELVT_DATABASE_PATH", &database),
        ],
    );
    service.wait_until_ready(
        "database migrations to finish and the Unix socket to accept connections",
    );
    let output = service.terminate_and_collect();

    assert!(database.exists(), "{}", output.diagnostics());
    let connection = rusqlite::Connection::open(&database)
        .expect("startup database should open after service shutdown");
    let migration_count: u32 = connection
        .query_row("SELECT COUNT(*) FROM schema_migration", [], |row| {
            row.get(0)
        })
        .expect("startup database should contain the migration ledger");
    assert!(migration_count > 0, "{}", output.diagnostics());
    assert!(
        !output.logs().contains("persistence_initialization_failed"),
        "{}",
        output.diagnostics()
    );
}

#[test]
fn readiness_wait_survives_startup_longer_than_500_milliseconds() {
    if !filesystem_sockets_available() {
        return;
    }
    let directory = TestDirectory::new();
    let taxonomy = write_taxonomy(&directory, "mvp-1");
    let database = directory.path("locked-startup.sqlite3");
    let lock = rusqlite::Connection::open(&database).expect("failed to create locked database");
    lock.execute_batch("BEGIN EXCLUSIVE")
        .expect("failed to hold startup migration lock");

    let started = Instant::now();
    let mut service = ServiceProcess::spawn(
        &directory,
        &[
            ("VELVT_ABSTRACTION_TAXONOMY_PATH", &taxonomy),
            ("VELVT_DATABASE_PATH", &database),
        ],
    );
    service.assert_not_ready_for(
        Duration::from_millis(650),
        "the test holds SQLite's exclusive migration lock",
    );
    lock.execute_batch("COMMIT")
        .expect("failed to release startup migration lock");
    service
        .wait_until_ready("the migration lock to clear and the Unix socket to accept connections");
    assert!(started.elapsed() > Duration::from_millis(500));
    let output = service.terminate_and_collect();

    assert!(
        !output.logs().contains("persistence_initialization_failed"),
        "{}",
        output.diagnostics()
    );
}

/// `main.rs` calls `socket_already_in_use` before doing any other startup
/// work and exits immediately if it returns `true`, instead of silently
/// failing to bind deep inside a spawned task whose result nobody awaits
/// (the old behavior: a zombie process with no working IPC listener).
#[tokio::test]
async fn detects_a_live_listener_but_not_a_stale_or_absent_socket_path() {
    let directory = TestDirectory::new();
    let socket = directory.path("duplicate.sock");

    assert!(
        !velvt_service::ipc::transport::socket_already_in_use(&socket).await,
        "nothing is listening yet"
    );

    let listener = match tokio::net::UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping live-listener assertion: filesystem sockets are unavailable");
            return;
        }
        Err(error) => panic!("failed to bind test socket: {error}"),
    };
    assert!(
        velvt_service::ipc::transport::socket_already_in_use(&socket).await,
        "a real listener is bound at this path"
    );

    drop(listener);
    fs::remove_file(&socket).expect("failed to remove stale test socket");
    assert!(
        !velvt_service::ipc::transport::socket_already_in_use(&socket).await,
        "the listener is gone and the path no longer exists"
    );
}
