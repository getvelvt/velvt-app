//! `velvt-service --dry-run-egress`: what would leave this Mac, printed, with
//! nothing sent and nothing written.
//!
//! The database is opened read-only. Each queued upload batch is rebuilt by
//! `payload_for_queued_batch` and handed to the real `HttpBatchUploader`, whose
//! client here describes the request through `describe_request` — the function
//! `ReqwestHttpClient` hashes and sends from — and then refuses to send it. So
//! the bytes printed are the bytes the next attempt sends, and the SHA-256
//! printed is the one the egress ledger will record for it.

use super::{EgressRecord, ENDPOINTS};
use crate::auth::{
    describe_request, AuthError, HttpClient, HttpRequest, HttpResponse, RedactedString,
};
use crate::persistence::{PersistenceError, SqlitePersistence};
use crate::upload::{
    payload_for_queued_batch, BatchEventPayload, BatchUploader, HttpBatchUploader,
};
use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// What a queued batch is re-sent with: the values `main.rs`'s retry loop and
/// `UploadBatcher::flush_now` pass to the coordinator.
const QUEUED_SCHEMA_VERSION: &str = "1";
const QUEUED_CLIENT_VERSION: &str = crate::build_info::SERVICE_VERSION;

/// The most not-yet-batched events printed. Upload-eligible events are batched
/// within a minute of arriving while the service runs, so more than this means
/// the service has been stopped with a backlog, and the count still says so.
const UNBATCHED_EVENT_LIMIT: usize = 10_000;

/// One request, as the ledger would describe it, and its exact body.
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub record: EgressRecord,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct DryRunReport {
    pub queued_requests: Vec<CapturedRequest>,
    pub unbatched_events: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DryRunError {
    #[error("could not read the Velvt database: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("could not write the report: {0}")]
    Io(#[from] std::io::Error),
}

/// An `HttpClient` that describes each request and sends nothing.
struct CapturingHttpClient {
    base_url: String,
    captured: Mutex<Vec<CapturedRequest>>,
}

impl HttpClient for CapturingHttpClient {
    fn send<'a>(
        &'a self,
        mut request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            // `AuthManager` attaches the device's access token to every upload.
            // The ledger records only that one is attached, never its value, so
            // a placeholder yields the same record the real send will.
            request.authorization = Some(RedactedString::new("not-a-token".to_owned()));
            let (record, body) = describe_request(&self.base_url, &request)?;
            self.captured
                .lock()
                .map_err(|_| AuthError::Transport)?
                .push(CapturedRequest { record, body });
            Err(AuthError::Transport)
        })
    }
}

/// Builds the report for the database at `database_path` and prints it to
/// `out`. Makes no network request and writes nothing to the database.
pub async fn run(
    database_path: &Path,
    base_url: &str,
    out: &mut dyn Write,
) -> Result<DryRunReport, DryRunError> {
    writeln!(
        out,
        "Velvt egress dry run. Nothing below was sent, and nothing was written.\n"
    )?;
    writeln!(out, "Database: {}", database_path.display())?;
    writeln!(out, "API:      {}\n", base_url.trim_end_matches('/'))?;

    let mut report = DryRunReport::default();
    if database_path.is_file() {
        let persistence = SqlitePersistence::open_read_only(database_path)?;
        report.queued_requests = queued_requests(&persistence, base_url).await?;
        report.unbatched_events = unbatched_events(&persistence)?;
    } else {
        writeln!(
            out,
            "There is no database at this path, so nothing has been recorded and nothing is queued.\n"
        )?;
    }

    print_queued(out, &report.queued_requests)?;
    print_unbatched(out, &report.unbatched_events)?;
    print_endpoints(out)?;
    writeln!(
        out,
        "To check what was actually sent, run scripts/prove_egress.sh from the velvt-app repository."
    )?;
    Ok(report)
}

async fn queued_requests(
    persistence: &SqlitePersistence,
    base_url: &str,
) -> Result<Vec<CapturedRequest>, DryRunError> {
    let client = Arc::new(CapturingHttpClient {
        base_url: base_url.to_owned(),
        captured: Mutex::new(Vec::new()),
    });
    let uploader = HttpBatchUploader::new(Arc::clone(&client));
    for batch in persistence.upload_batch_repo().pending_batches()? {
        let payload = payload_for_queued_batch(batch, QUEUED_SCHEMA_VERSION, QUEUED_CLIENT_VERSION);
        // Always an error: the client refuses every send once it has
        // described it.
        let _ = uploader.upload(&payload).await;
    }
    let captured = client
        .captured
        .lock()
        .map(|captured| captured.clone())
        .unwrap_or_default();
    Ok(captured)
}

fn unbatched_events(persistence: &SqlitePersistence) -> Result<Vec<String>, DryRunError> {
    let entries = persistence
        .raw_event_repo()
        .unbatched_events(UNBATCHED_EVENT_LIMIT)?;
    // Oldest first, the order `recover_unbatched` batches them in.
    Ok(entries
        .into_iter()
        .rev()
        .map(|entry| {
            serde_json::to_string(&BatchEventPayload {
                event_id: entry.event_id,
                stable_id: entry.stable_id,
                label: entry.label,
                category: entry.category,
                taxonomy_version: entry.taxonomy_version,
                classification_tier: entry.classification_tier,
                occurred_at: entry.occurred_at,
                duration_seconds: entry.duration_seconds,
            })
            .unwrap_or_default()
        })
        .collect())
}

fn print_queued(out: &mut dyn Write, requests: &[CapturedRequest]) -> std::io::Result<()> {
    writeln!(out, "== Queued upload batches: {}\n", requests.len())?;
    if requests.is_empty() {
        writeln!(out, "Nothing is queued.\n")?;
        return Ok(());
    }
    writeln!(
        out,
        "Each is printed exactly as the next attempt sends it. \"body sha256\" is the\n\
         SHA-256 of the line after \"body:\", and it is what the egress ledger records\n\
         when the request is sent, unless the batch changes first (correcting a\n\
         classification rewrites a queued event's category).\n"
    )?;
    let total = requests.len();
    for (index, request) in requests.iter().enumerate() {
        let record = &request.record;
        writeln!(out, "-- request {} of {total}", index + 1)?;
        writeln!(out, "{} {}", record.method, record.endpoint)?;
        if request.body.is_some() {
            writeln!(out, "content-type: application/json")?;
        }
        if record.bearer {
            writeln!(
                out,
                "authorization: Bearer <this device's access token, never printed>"
            )?;
        }
        writeln!(out, "body bytes: {}", record.body_bytes)?;
        writeln!(out, "body sha256: {}", record.body_sha256)?;
        writeln!(out, "body:")?;
        out.write_all(request.body.as_deref().unwrap_or_default())?;
        writeln!(out, "\n")?;
    }
    Ok(())
}

fn print_unbatched(out: &mut dyn Write, events: &[String]) -> std::io::Result<()> {
    writeln!(
        out,
        "== Recorded events not yet in a batch: {}\n",
        events.len()
    )?;
    if events.is_empty() {
        writeln!(out, "None.\n")?;
        return Ok(());
    }
    writeln!(
        out,
        "These leave in a batch that does not exist yet: it closes every 60 s while\n\
         events arrive, or at 50 events, and its batch_id is minted then. Each event\n\
         will appear in that batch's \"events\" array exactly as:\n"
    )?;
    for event in events {
        writeln!(out, "{event}")?;
    }
    if events.len() == UNBATCHED_EVENT_LIMIT {
        writeln!(out, "(the first {UNBATCHED_EVENT_LIMIT} are shown)")?;
    }
    writeln!(out)
}

fn print_endpoints(out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "== Every request the helper can make\n")?;
    writeln!(
        out,
        "Only the upload batch carries activity. Every request below is recorded in\n\
         the egress ledger before it is sent.\n"
    )?;
    for endpoint in ENDPOINTS {
        writeln!(out, "{:<6} {}", endpoint.method, endpoint.path)?;
        writeln!(out, "       when: {}", endpoint.when)?;
        writeln!(out, "       body: {}", endpoint.body)?;
    }
    writeln!(out)
}
