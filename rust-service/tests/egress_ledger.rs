//! The egress ledger against a real socket.
//!
//! Every test here that sends does so through `ReqwestHttpClient` to a server
//! on 127.0.0.1 that this file controls, so what the ledger recorded can be
//! compared with the bytes that actually arrived.

use chrono::{TimeZone, Utc};
use serde_json::json;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;
use velvt_service::auth::{
    AuthError, HttpClient, HttpRequest, HttpResponse, RedactedString, ReqwestHttpClient,
};
use velvt_service::egress::{
    dry_run, sha256_hex, verify_chain, EgressCheckpoint, EgressLedgerEntry, EgressRecord,
    ENDPOINTS, REDACTED,
};
use velvt_service::persistence::{
    BatchEvent, EgressLedgerRepo, NewUploadBatch, PersistenceError, RawEventEntry,
    SqlitePersistence,
};
use velvt_service::retention::{EgressLedgerRetentionTarget, RetentionTarget};
use velvt_service::upload::{FakePrivacyAlertSink, HttpBatchUploader, UploadCoordinator};

// ---------------------------------------------------------------------------
// A one-request-per-connection HTTP server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Arrival {
    request_line: String,
    headers: String,
    body: Vec<u8>,
    /// How many ledger entries existed when the request reached the server.
    ledger_entries_on_arrival: usize,
}

struct TestServer {
    base_url: String,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
}

impl TestServer {
    async fn start(response: &'static str, ledger: Option<Arc<dyn EgressLedgerRepo>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&arrivals);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = Vec::new();
                let header_end = loop {
                    let mut chunk = [0_u8; 4096];
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break None;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(end) = find(&buffer, b"\r\n\r\n") {
                        break Some(end);
                    }
                };
                let Some(header_end) = header_end else {
                    continue;
                };
                let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                let mut body = buffer[header_end + 4..].to_vec();
                while body.len() < content_length {
                    let mut chunk = [0_u8; 4096];
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..read]);
                }
                let ledger_entries_on_arrival = ledger
                    .as_ref()
                    .map_or(0, |ledger| ledger.entries().unwrap().len());
                let (request_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
                sink.lock().unwrap().push(Arrival {
                    request_line: request_line.to_owned(),
                    headers: headers.to_owned(),
                    body,
                    ledger_entries_on_arrival,
                });
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Self { base_url, arrivals }
    }

    fn arrivals(&self) -> Vec<Arrival> {
        self.arrivals.lock().unwrap().clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

const OK_ACCEPTED: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 21\r\nconnection: close\r\n\r\n{\"status\":\"accepted\"}";
const REDIRECT: &str = "HTTP/1.1 307 Temporary Redirect\r\nlocation: /elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// Attaches a bearer token the way `AuthManager` does for every authenticated
/// request, so a test can send what the upload path sends.
struct WithBearer(ReqwestHttpClient);

impl HttpClient for WithBearer {
    fn send<'a>(
        &'a self,
        mut request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        request.authorization = Some(RedactedString::new("device-access-token"));
        self.0.send(request)
    }
}

struct RefusingLedger;

impl EgressLedgerRepo for RefusingLedger {
    fn append(
        &self,
        _record: &EgressRecord,
        _recorded_at: chrono::DateTime<Utc>,
    ) -> Result<EgressLedgerEntry, PersistenceError> {
        Err(PersistenceError::LockUnavailable)
    }
    fn entries(&self) -> Result<Vec<EgressLedgerEntry>, PersistenceError> {
        Ok(Vec::new())
    }
    fn checkpoint(&self) -> Result<Option<EgressCheckpoint>, PersistenceError> {
        Ok(None)
    }
    fn prune(
        &self,
        _cutoff: chrono::DateTime<Utc>,
        _max_entries: u64,
        _batch_size: usize,
        _now: chrono::DateTime<Utc>,
    ) -> Result<u64, PersistenceError> {
        Ok(0)
    }
}

struct ScratchDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl ScratchDatabase {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("velvt-egress-ledger-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("velvt-service.sqlite3");
        Self { directory, path }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

// ---------------------------------------------------------------------------
// Recording before sending
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_request_is_recorded_before_it_reaches_the_server() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ledger = persistence.egress_ledger_repo();
    let server = TestServer::start(OK_ACCEPTED, Some(Arc::clone(&ledger))).await;
    let client = WithBearer(ReqwestHttpClient::new(
        server.base_url.clone(),
        Arc::clone(&ledger),
    ));

    let mut first = HttpRequest::post("/v1/events/batches");
    first.json_body = Some(json!({ "batch_id": "b-1", "events": [] }));
    let response = client.send(first).await.unwrap();
    assert_eq!(response.status, 200);
    client.send(HttpRequest::get("/v1/ready")).await.unwrap();

    let arrivals = server.arrivals();
    let entries = ledger.entries().unwrap();
    assert_eq!(arrivals.len(), 2);
    assert_eq!(entries.len(), 2);
    for (index, arrival) in arrivals.iter().enumerate() {
        assert_eq!(
            arrival.ledger_entries_on_arrival,
            index + 1,
            "request {index} reached the server before its ledger entry existed"
        );
    }

    let upload = &entries[0];
    assert_eq!(upload.method, "POST");
    assert_eq!(
        upload.endpoint,
        format!("{}/v1/events/batches", server.base_url)
    );
    assert_eq!(
        upload.body_sha256,
        sha256_hex(&arrivals[0].body),
        "the ledger hash must be of the bytes that arrived"
    );
    assert_eq!(upload.body_bytes as usize, arrivals[0].body.len());
    assert!(upload.bearer);
    assert!(!upload.body_redacted);
    assert!(arrivals[0]
        .request_line
        .starts_with("POST /v1/events/batches "));

    let ready = &entries[1];
    assert_eq!(ready.method, "GET");
    assert_eq!(ready.body_bytes, 0);
    assert_eq!(ready.body_sha256, sha256_hex(b""));
    assert_eq!(ready.prev_hash, upload.entry_hash);

    let summary = verify_chain(None, &entries).unwrap();
    assert_eq!(summary.head_hash, ready.entry_hash);
}

#[tokio::test]
async fn a_request_the_ledger_cannot_record_is_not_sent() {
    let server = TestServer::start(OK_ACCEPTED, None).await;
    let client = ReqwestHttpClient::new(server.base_url.clone(), Arc::new(RefusingLedger));

    let mut request = HttpRequest::post("/v1/events/batches");
    request.json_body = Some(json!({ "batch_id": "b-1" }));
    let result = client.send(request).await;

    assert!(matches!(result, Err(AuthError::Transport)));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        server.arrivals().is_empty(),
        "a request with no ledger entry reached the network"
    );
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ledger = persistence.egress_ledger_repo();
    let server = TestServer::start(REDIRECT, None).await;
    let client = ReqwestHttpClient::new(server.base_url.clone(), Arc::clone(&ledger));

    let response = client
        .send(HttpRequest::get("/v1/insights/poll"))
        .await
        .unwrap();

    assert_eq!(response.status, 307);
    assert_eq!(
        server.arrivals().len(),
        1,
        "a redirect is a send the ledger never saw"
    );
    assert_eq!(ledger.entries().unwrap().len(), 1);
}

#[tokio::test]
async fn a_password_is_sent_but_never_hashed_into_the_ledger() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ledger = persistence.egress_ledger_repo();
    let server = TestServer::start(OK_ACCEPTED, None).await;
    let client = ReqwestHttpClient::new(server.base_url.clone(), Arc::clone(&ledger));

    let mut request = HttpRequest::post("/v1/auth/login");
    request.json_body = Some(json!({ "email": "a@example.test", "password": "correct horse" }));
    client.send(request).await.unwrap();

    let arrival = &server.arrivals()[0];
    let entry = &ledger.entries().unwrap()[0];
    assert!(String::from_utf8_lossy(&arrival.body).contains("correct horse"));
    assert!(entry.body_redacted);
    assert_ne!(entry.body_sha256, sha256_hex(&arrival.body));
    assert_eq!(
        entry.body_sha256,
        sha256_hex(
            &serde_json::to_vec(&json!({ "email": "a@example.test", "password": REDACTED }))
                .unwrap()
        )
    );
    assert_eq!(entry.body_bytes as usize, arrival.body.len());
    assert!(!entry.bearer);
}

// ---------------------------------------------------------------------------
// The dry run
// ---------------------------------------------------------------------------

fn queued_event(id: &str) -> BatchEvent {
    BatchEvent {
        event_id: id.into(),
        stable_id: format!("abs_{id}"),
        label: "document:edit".into(),
        category: "FOCUS_WORK".into(),
        taxonomy_version: "mvp-2".into(),
        classification_tier: "exact_match".into(),
        occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
        duration_seconds: 42,
    }
}

fn unbatched_event(id: &str) -> RawEventEntry {
    RawEventEntry {
        event_id: id.into(),
        stable_id: format!("abs_{id}"),
        label: "document:edit".into(),
        local_display_label: Some("Local Only Label".into()),
        local_name_suggestion: Some("Raw Application Name".into()),
        category: "REFERENCE".into(),
        taxonomy_version: "mvp-2".into(),
        classification_tier: "exact_match".into(),
        classification_status: "classified".into(),
        classification_confidence: "high".into(),
        classification_source: "seed".into(),
        occurred_at: Utc.timestamp_opt(1_800_000_100, 0).unwrap(),
        duration_seconds: 7,
        upload_eligible: true,
        app_stable_id: None,
        app_scope_eligible: true,
    }
}

/// The body line and the hash the dry run printed for its only request.
fn printed_request(report: &str) -> (Vec<u8>, String) {
    let lines: Vec<&str> = report.lines().collect();
    let sha = lines
        .iter()
        .find_map(|line| line.strip_prefix("body sha256: "))
        .expect("the dry run printed a body hash")
        .to_owned();
    let body_index = lines
        .iter()
        .position(|line| *line == "body:")
        .expect("the dry run printed a body");
    (lines[body_index + 1].as_bytes().to_vec(), sha)
}

async fn dry_run_report(path: &Path, base_url: &str) -> String {
    let mut out = Vec::new();
    dry_run::run(path, base_url, &mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn the_dry_run_prints_the_bytes_the_next_attempt_sends_and_the_hash_the_ledger_records() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    let batches = persistence.upload_batch_repo();
    batches
        .insert_batch_with_events(
            &NewUploadBatch {
                batch_id: "batch-1".into(),
            },
            &[queued_event("e-1"), queued_event("e-2")],
        )
        .unwrap();
    let server = TestServer::start(OK_ACCEPTED, None).await;

    let report = dry_run_report(&scratch.path, &server.base_url).await;
    let (printed_body, printed_sha) = printed_request(&report);
    assert_eq!(sha256_hex(&printed_body), printed_sha);
    assert!(report.contains("== Queued upload batches: 1"));
    assert!(server.arrivals().is_empty(), "the dry run sent something");
    assert!(
        persistence
            .egress_ledger_repo()
            .entries()
            .unwrap()
            .is_empty(),
        "the dry run wrote to the ledger"
    );
    assert_eq!(
        batches.pending_batches().unwrap()[0].attempt_count,
        0,
        "the dry run changed the queue"
    );
    for local_only in ["abs_e-1", "document:edit"] {
        assert!(
            !String::from_utf8_lossy(&printed_body).contains(local_only),
            "{local_only} is local-only and is not in the wire body"
        );
    }

    // Now send it for real, through the path the retry loop uses.
    let ledger = persistence.egress_ledger_repo();
    let http = Arc::new(WithBearer(ReqwestHttpClient::new(
        server.base_url.clone(),
        Arc::clone(&ledger),
    )));
    let coordinator = UploadCoordinator::new(
        Arc::clone(&batches),
        HttpBatchUploader::new(http),
        FakePrivacyAlertSink::default(),
    )
    .with_host("127.0.0.1");
    coordinator
        .flush_all_pending("1", env!("CARGO_PKG_VERSION"))
        .await
        .unwrap();

    let arrival = &server.arrivals()[0];
    let entry = &ledger.entries().unwrap()[0];
    assert_eq!(
        arrival.body, printed_body,
        "the dry run printed different bytes"
    );
    assert_eq!(entry.body_sha256, printed_sha);
    assert!(arrival
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer"));
    assert!(entry.bearer);
}

#[tokio::test]
async fn the_dry_run_shows_events_not_yet_batched_in_their_wire_shape() {
    let scratch = ScratchDatabase::new();
    let persistence = SqlitePersistence::open(&scratch.path).unwrap();
    persistence
        .raw_event_repo()
        .insert(&unbatched_event("e-9"))
        .unwrap();

    let report = dry_run_report(&scratch.path, "https://api.example.test").await;

    assert!(report.contains("== Recorded events not yet in a batch: 1"));
    assert!(report.contains("\"event_id\":\"e-9\""));
    assert!(report.contains("\"category\":\"REFERENCE\""));
    for local_only in ["Raw Application Name", "Local Only Label", "abs_e-9"] {
        assert!(
            !report.contains(local_only),
            "{local_only} is local-only and must not appear as egress"
        );
    }
}

#[tokio::test]
async fn the_dry_run_without_a_database_sends_nothing_and_says_so() {
    let scratch = ScratchDatabase::new();
    let report = dry_run_report(&scratch.path, "https://api.example.test").await;
    assert!(report.contains("There is no database at this path"));
    assert!(report.contains("== Queued upload batches: 0"));
    assert!(!scratch.path.exists(), "the dry run created a database");
    for endpoint in ENDPOINTS {
        assert!(
            report.contains(endpoint.path),
            "{} is not listed",
            endpoint.path
        );
    }
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

#[test]
fn retention_prunes_the_oldest_entries_and_the_rest_still_verify() {
    let persistence = SqlitePersistence::open_in_memory().unwrap();
    let ledger = persistence.egress_ledger_repo();
    for _ in 0..8 {
        ledger
            .append(
                &EgressRecord::new("GET", "https://h/v1/ready", &[], None, false),
                Utc::now(),
            )
            .unwrap();
    }
    let target = EgressLedgerRetentionTarget::new(Arc::clone(&ledger), 30, 3, 500);
    assert_eq!(target.name(), "egress_ledger");
    assert_eq!(target.run_cleanup().unwrap().deleted, 5);

    let entries = ledger.entries().unwrap();
    assert_eq!(entries.len(), 3);
    let checkpoint = ledger.checkpoint().unwrap().unwrap();
    assert_eq!(checkpoint.through_seq, 5);
    verify_chain(Some(&checkpoint), &entries).unwrap();
}

// ---------------------------------------------------------------------------
// Source guards
// ---------------------------------------------------------------------------

fn source_files(directory: &Path, found: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            found.push((path, text));
        }
    }
}

fn service_sources() -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    source_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    found
}

/// The ledger is only complete if nothing else can reach the network.
#[test]
fn reqwest_http_client_is_the_only_network_client() {
    const NETWORK_PRIMITIVES: &[&str] = &[
        "reqwest::Client",
        "reqwest::get(",
        "reqwest::blocking",
        "reqwest::Request",
        "ClientBuilder",
        "TcpStream",
        "UdpSocket",
        "std::net::",
        "hyper::",
    ];
    for (path, text) in service_sources() {
        if path.ends_with("src/auth/http.rs") {
            continue;
        }
        for primitive in NETWORK_PRIMITIVES {
            assert!(
                !text.contains(primitive),
                "{} uses {primitive}. Every network send must go through \
                 ReqwestHttpClient (src/auth/http.rs), which records it in the \
                 egress ledger first",
                path.display()
            );
        }
    }
}

fn normalized_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut normalized = String::new();
    let mut in_placeholder = false;
    for character in path.chars() {
        match character {
            '{' => {
                in_placeholder = true;
                normalized.push_str("{}");
            }
            '}' => in_placeholder = false,
            _ if in_placeholder => {}
            _ => normalized.push(character),
        }
    }
    normalized
}

/// `ENDPOINTS` is what the dry run tells a reader the helper can reach. It must
/// be exactly the set of API paths the source builds.
#[test]
fn the_endpoint_list_is_every_api_path_in_the_source() {
    let mut in_source = std::collections::BTreeSet::new();
    for (path, text) in service_sources() {
        if path
            .components()
            .any(|component| component.as_os_str() == "egress")
        {
            continue;
        }
        let mut rest = text.as_str();
        while let Some(start) = rest.find("\"/v1/") {
            let literal = &rest[start + 1..];
            let end = literal.find('"').unwrap();
            in_source.insert(normalized_path(&literal[..end]));
            rest = &literal[end..];
        }
    }
    let listed: std::collections::BTreeSet<String> = ENDPOINTS
        .iter()
        .map(|endpoint| normalized_path(endpoint.path))
        .collect();
    assert_eq!(
        in_source, listed,
        "egress::ENDPOINTS no longer matches the API paths in src/. A new \
         request belongs in that list, with when it is sent and what its body holds"
    );
}

/// `text` with every `#[cfg(test)] mod … { … }` block removed, by brace depth.
fn without_test_modules(text: &str) -> String {
    let mut shipped = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("#[cfg(test)]") {
        shipped.push_str(&rest[..start]);
        let after = &rest[start + "#[cfg(test)]".len()..];
        let item = after.trim_start();
        if !item.starts_with("mod ") {
            shipped.push_str("#[cfg(test)]");
            rest = after;
            continue;
        }
        let open = after.find('{').unwrap();
        let mut depth = 0_usize;
        let mut end = after.len();
        for (index, character) in after[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + index + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &after[end..];
    }
    shipped.push_str(rest);
    shipped
}

/// A request built from a variable path is invisible to the literal scan above,
/// so the number of request constructors outside tests is pinned too.
#[test]
fn every_request_construction_site_is_accounted_for() {
    const CONSTRUCTORS: &[&str] = &[
        "HttpRequest::get(",
        "HttpRequest::post(",
        "HttpRequest::patch(",
        "HttpRequest::delete(",
    ];
    let mut sites = 0;
    for (_, text) in service_sources() {
        let shipped = without_test_modules(&text);
        sites += CONSTRUCTORS
            .iter()
            .map(|constructor| shipped.matches(constructor).count())
            .sum::<usize>();
    }
    assert_eq!(
        sites, 16,
        "the number of HttpRequest constructions in shipped code changed. Check \
         the new or removed request against egress::ENDPOINTS, then update this count"
    );
}
