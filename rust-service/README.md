# Velvt Rust Service

The Rust service owns authentication, the final privacy boundary, SQLite
persistence, abstraction, cloud requests, and IPC delivery to the macOS client.

## Authentication Privacy

Access and refresh tokens are stored together as one record in macOS Keychain
through `KeychainTokenStore`. They must never be stored in SQLite, plaintext
files, environment variables, logs, tracing fields, or error messages.

All token-carrying fields use `RedactedString`. Its `Debug` and `Display`
implementations emit only `[redacted]`. The underlying value is exposed only
inside the private Keychain serializer and concrete HTTP authorization/body
construction. `RedactedString` intentionally does not implement `Serialize`,
preventing unrelated code from serializing tokens.

Tests use `FakeTokenStore`; they never access the real Keychain.

## Upload Batching

`upload::BatchAssembler` creates deterministic batches from privacy-safe
`BatchEventPayload` values. Count and age thresholds come from
`VELVT_UPLOAD_BATCH_EVENT_LIMIT` and `VELVT_UPLOAD_FLUSH_SECONDS`.
`VELVT_UPLOAD_RETRY_SCAN_SECONDS` controls how often persisted due batches are
checked, and `VELVT_API_BASE_URL` selects the cloud API host.

Batch IDs are SHA-256 digests over the domain separator
`velvt:upload-batch:v1`, the device ID, and the ordered local event IDs.
Recreating the same logical batch after a crash therefore produces the same
ID. The assembler flushes once when either the configured count or age
threshold is reached, and exposes immediate sleep/shutdown flushes.

`UploadCoordinator` persists a batch before sending it, resumes pending or
failed batches after restart, and treats duplicate responses as success.
Rate-limit retry state is host-scoped and respects server-provided
`Retry-After` delays. Without that header it uses exponential backoff starting
at 30 seconds, capped at 15 minutes, with production jitter clamped to +/-10%.
A `raw_field_rejected` response
permanently rejects the batch, emits a structured error, and broadcasts
`privacy_violation_alert` to Swift.

`BatchRetentionPolicy` is the R8 integration seam. R5 defaults to
`KeepAllBatches`; a future retention policy can discard expired batches before
the uploader is invoked without changing transport or retry logic.

## Auth State Machine

Auth state changes only through `AuthStateMachine::transition`.

```text
Unauthenticated
    |
    | tokens supplied
    v
Authenticated { device_id }
    |             |                 |
    | expiry      | invalid token   | device_revoked
    v             v                 v
RefreshInFlight  NeedsReauth    DeviceRevoked
    |   |   |
    |   |   +-- device_revoked / exhausted reissue --> DeviceRevoked
    |   +------ invalid credentials / invalid response --> NeedsReauth
    +---------- refresh success or transient transport failure --> Authenticated

NeedsReauth -- tokens supplied --> Authenticated
NeedsReauth -- sign out -------> Unauthenticated
DeviceRevoked is terminal until a future onboarding recovery flow.
```

Before every authenticated request, `AuthManager` checks token expiry against
the configured refresh buffer. Refresh is single-flight: concurrent callers
wait for the active refresh and reuse its atomically replaced token pair.
Transient transport failures preserve the existing Keychain record and retry
on the next request cycle. Invalid credentials transition to `NeedsReauth`.

`device_token_revoked` attempts `/v1/auth/devices/reissue` once before
transitioning to `DeviceRevoked`. `device_revoked`, `device_not_found`, failed
reissue, or repeated revocation transitions to `DeviceRevoked`; subsequent
authenticated upload attempts are rejected before reaching HTTP.

## History & Insight Delivery (R6)

`delivery::FetchService` is the concrete implementation of `CacheManager` — the
only interface R7 depends on.  R7 must not import anything from
`delivery::fetch`, `delivery::parser`, or `delivery::scheduler`; use
`Arc<dyn CacheManager>` everywhere.

### TTL configuration

| Env var | Default | Description |
|---|---|---|
| `VELVT_HISTORY_TTL_SECONDS` | `600` | How long a history summary is considered fresh |
| `VELVT_INSIGHT_TTL_SECONDS` | `1800` | How long a fetched insight is considered fresh |
| `VELVT_INSIGHT_NEGATIVE_TTL_SECONDS` | `300` | How long a 404 (no insight) is cached before retrying |
| `VELVT_CACHE_READ_TIMEOUT_MS` | `200` | Maximum time a blocking SQLite read may hold the IPC path; returns a cache miss if exceeded |
| `VELVT_FETCH_INTERVAL_SECONDS` | `600` | Minimum wall-clock time between proactive background fetches |

### Negative cache behaviour

When `GET /v1/insights/daily?date=YYYY-MM-DD` returns 404, `FetchService`
writes a *negative cache entry* (`not_found = 1`) instead of re-querying the
API on every scheduler tick.  `daily_insight` returns `Ok(None)` for both a
live 404 and a cached negative entry — callers cannot distinguish the two.
The negative entry expires after `insight_negative_ttl` (default 5 minutes),
after which the API is contacted again.

### Fetch scheduler pause conditions

`FetchScheduler` runs a background loop that calls `FetchService::refresh_all`
whenever the minimum fetch interval has elapsed AND the device is in the
`Authenticated` state.  The scheduler pauses (silently skips the tick) on:

- `AuthState::DeviceRevoked` — device has been revoked; no outbound calls.
- `AuthState::Unauthenticated` — no token available.
- `AuthState::NeedsReauth` — credentials expired; waits for re-auth.
- `AuthState::RefreshInFlight` — refresh in progress; wait for its outcome.

On shutdown, send `true` on the `watch::Sender<bool>` passed to
`FetchScheduler::new`.

### Concurrent-fetch deduplication

`daily_history` and `daily_insight` use a per-key `tokio::sync::Mutex` to
deduplicate in-flight requests.  If two callers request the same date
simultaneously, only one HTTP call is made; the second caller blocks on the
mutex, re-checks the cache after the first finishes, and returns the cached
result.

### Privacy invariant

Insight `text` is never written to any log or tracing field.  Only `date` and
`confidence_level` are recorded at `DEBUG` level when an insight is stored.

## Push Delivery to Swift (R7)

R7 closes the last mile: after R6 fetches or caches history and insight data,
R7 shapes it and delivers it proactively to the connected Swift client over the
IPC socket.  Push failures are fire-and-forget — they never propagate back to
the cache or fetch layers.

### Push trigger events

| Trigger | Payload type | Adapter method |
|---|---|---|
| Positive insight cache write (R6 slow path) | `InsightPayload` | `PushAdapter::push_insight` |
| History fetch completes (R6 slow path) | `HistoryPayload` | `PushAdapter::push_history` |
| `RequestLatestInsight` from Swift (on-demand) | `InsightPayload` or `CacheEmpty` | `R7Router` → direct response |
| `RequestLatestHistory` from Swift (on-demand) | `HistoryPayload` or `CacheEmpty` | `R7Router` → direct response |
| Privacy violation detected (R5 upload boundary) | `PrivacyViolationAlert` | `PushAdapterAlertSink::alert` |

On-demand requests receive a synchronous pull response; the push queue is only
used for proactive (unsolicited) delivery and privacy alerts.

### Queue configuration

| Env var | Default | Description |
|---|---|---|
| `VELVT_PUSH_QUEUE_CAPACITY` | `50` | Maximum messages buffered while Swift is disconnected; oldest dropped when full |
| `VELVT_PUSH_WRITE_TIMEOUT_MS` | `500` | Per-message write timeout; slow clients are disconnected when exceeded |

The queue survives client disconnects.  Messages not yet sent when a client
disconnects remain in the queue and are delivered when the client reconnects.

### ValidatedPayload<T> pattern

Every message that enters the push queue must pass through
`delivery::shaper::ValidatedPayload<T>`.  The type-system contract has two
layers:

1. **`ValidatedPayload::new` is the only constructor** — it calls
   `ValidatePayload::validate_fields` plus a JSON serialisation round-trip.
   A payload with an empty `text`, `code`, or `payload_type` field is
   rejected at this point, logged (type name only, no content), and silently
   dropped.

2. **`PushQueue::enqueue` is `pub(super)`** — only `PushAdapter`, which lives
   in the same module, can write to the queue.  The IPC connection layer
   (`ipc/connection.rs`) receives only `Arc<PushQueue>` for draining (`try_pop`,
   `notify`); it has no write access.

To add a new push type: implement `ValidatePayload` for the DTO in
`delivery/shaper.rs`, add a shaper function, and add a `push_*` method to
`PushAdapter`.  No changes to the transport or cache layers are required.

### Privacy invariant

Insight `text`, history `summaries`, and privacy alert `message` fields must
never appear in any log or tracing output — not even truncated.  Only the
following are safe to log: `date`, `days`, message type names (as
`message_type`), and error codes (as `error_code`).

## Device Registration Seam

Device registration depends only on `DeviceRegistrar::register()`.
`NoOpDeviceRegistrar` is the production placeholder until the onboarding issue
wires the concrete registrar. Tests can replace it with a recording fake only
at the composition/wiring site without changing consumers.
