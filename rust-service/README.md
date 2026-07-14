# Velvt Rust Service

The Rust service owns authentication, the final privacy boundary, SQLite
persistence, abstraction, cloud requests, and IPC delivery to the macOS client.

## Authentication Privacy

Device and user access/refresh tokens are supplied by the host client over IPC
and kept only in the service's in-memory `VolatileTokenStore`. Authenticated
cloud requests use device tokens; user tokens are retained only to refresh user
auth and reissue device credentials when a device token pair is revoked.
Platform-specific secure storage belongs to the host client (Keychain on macOS,
an equivalent credential store elsewhere). Tokens must never be stored in
SQLite, plaintext files, environment variables, logs, tracing fields, or error
messages.

All token-carrying fields use `RedactedString`. Its `Debug` and `Display`
implementations emit only `[redacted]`. The underlying value is exposed only
inside concrete HTTP authorization/body construction and IPC session handoff.
`RedactedString` intentionally does not implement `Serialize`,
preventing unrelated code from serializing tokens.

Tests use `FakeTokenStore`; they never access platform credential stores.

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
Transient transport failures preserve the existing in-memory token record and retry
on the next request cycle. Invalid credentials transition to `NeedsReauth`.

`device_token_revoked` attempts `/v1/auth/devices/reissue` once with a valid
user access token, refreshing the user token first when needed.
`device_revoked` or `device_not_found` transitions to `DeviceRevoked`; failed
refresh/reissue transitions to `NeedsReauth`; subsequent authenticated upload
attempts are rejected before reaching HTTP.

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

## Long-Poll Insight Delivery

The service also runs a live long-poll loop while authenticated.  It calls
`GET /v1/insights/poll` by default, where velvt-core either returns `200` with
a JSON insight and `id`, or `204 No Content` when no insight is ready.

`PollScheduler` parses `200` responses into a typed insight, suppresses an
immediate duplicate by remembering the last delivered insight ID, and forwards
the result to Swift through the existing IPC push queue as both
`insight_payload` and `notification_payload`.  Swift never calls velvt-core.

Configuration:

| Env var | Default | Description |
|---|---|---|
| `VELVT_INSIGHT_POLL_PATH` | `/v1/insights/poll` | Path appended to the configured velvt-core base URL |
| `VELVT_INSIGHT_POLL_TIMEOUT_SECONDS` | `30` | Client-side timeout for one held request |
| `VELVT_INSIGHT_POLL_IDLE_SECONDS` | `1` | Delay after `204 No Content` |
| `VELVT_INSIGHT_POLL_INITIAL_BACKOFF_SECONDS` | `1` | Initial retry delay after transport, timeout, or non-2xx failure |
| `VELVT_INSIGHT_POLL_MAX_BACKOFF_SECONDS` | `60` | Maximum retry delay |

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

## Retention & Lifecycle (R8)

### Retention scheduler

`RetentionScheduler` drives a list of `RetentionTarget` objects on a
configurable interval.  Each target performs exactly **one batched DELETE per
scheduler cycle** — no looping within a single call.  If a table still has rows
to clean up, the scheduler calls the same target again on the next tick.

Pending and in-flight upload batches (`status = 'pending'` or `'failed'`) are
**never** touched by any retention target.  Only `sent` and `rejected` batches
are eligible.

#### Retention configuration

| Env var | Default | Description |
|---|---|---|
| `VELVT_RAW_EVENT_TTL_HOURS` | `72` | Raw events older than this are eligible for expiry |
| `VELVT_RAW_EVENT_EXPIRY_INTERVAL_MINUTES` | `30` | How often the expiry scheduler runs |
| `VELVT_RETENTION_BATCH_SIZE` | `500` | Max rows deleted per target per cycle |
| `VELVT_SENT_BATCH_RETENTION_DAYS` | `30` | How long sent upload batches are kept |
| `VELVT_REJECTED_BATCH_AUDIT_DAYS` | `7` | How long rejected batches are kept for audit |
| `VELVT_CACHE_EXPIRY_GRACE_SECONDS` | `3600` | Extra window after TTL before cache rows are deleted |

#### Extending retention

To add a new retention target without modifying the scheduler:
1. Add a DAL method on the relevant trait in `persistence/traits.rs`.
2. Implement it in `persistence/sqlite.rs`.
3. Create a struct implementing `RetentionTarget` in `retention/targets.rs`.
4. Register it in `main.rs` with `scheduler.add_target(...)`.

The scheduler core (`RetentionScheduler`) is never modified.

### Graceful shutdown sequence

When `SIGTERM` or `SIGINT` is received:

```
1. push_adapter.push_shutting_down(reason)   ← ShuttingDown enqueued at queue front
2. token.cancel()                             ← all subscribers see shutdown = true
3. tokio::time::timeout(deadline, async {     ← race: tasks stop OR deadline fires
       fetch_task.await
       recovery_task.await
       server_task.await    ← drains queue (delivers ShuttingDown) then returns
       retention_task.await
   }).await
4. drop(persistence)                          ← closes SQLite connection
```

`push_shutting_down()` **must** be called before `token.cancel()`.  The
connection task's shutdown select arm drains the push queue — including
the pre-queued `ShuttingDown` message — before it returns.

`SIGTERM` received twice is safe: `CancellationToken::cancel()` is idempotent.

| Env var | Default | Description |
|---|---|---|
| `VELVT_SHUTDOWN_DEADLINE_SECONDS` | `10` | Maximum time to wait for tasks to stop cleanly |

### Reconnect window

When a Swift client disconnects unexpectedly, the push queue is **preserved** for
a configurable window.  If the client reconnects within that window,
`ReconnectTracker::acquire()` returns the same `Arc<PushQueue>` so any
messages buffered during the disconnect are delivered.

If the window elapses without a reconnect, the queue is dropped and
`acquire()` returns a fresh empty queue on the next connection.

The version counter inside `ReconnectTracker` prevents a race where a slow
cleanup task clears the queue for a connection that already reconnected.

| Env var | Default | Description |
|---|---|---|
| `VELVT_RECONNECT_WINDOW_SECONDS` | `30` | How long a push queue survives after a client disconnect |
| `VELVT_PUSH_QUEUE_CAPACITY` | `50` | Maximum messages buffered in the queue |

## Device Registration Seam

Device registration depends only on `DeviceRegistrar::register()`.
`NoOpDeviceRegistrar` is the production placeholder until the onboarding issue
wires the concrete registrar. Tests can replace it with a recording fake only
at the composition/wiring site without changing consumers.
