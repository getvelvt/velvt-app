# Architecture

This is the canonical architecture reference for velvt-mac. For deep dives
into individual subsystems, see [`docs/architecture/`](docs/architecture/);
this document ties them together and reflects the system as integrated for
the MVP, not any individual issue branch.

## System diagram

```
                          ┌─────────────────────────────────────────┐
                          │         Swift Client (swift-client/)     │
                          │                                           │
  AXObserver / NSWorkspace│  Collection ──▶ EventRelay ──▶ IPC Client │
        (focus events)    │  (ring buffer, drop-oldest while offline) │
                          └───────────────────┬───────────────────────┘
                                               │ Unix domain socket
                                               │ (newline-delimited JSON,
                                               │  proto/ contract)
                          ┌────────────────────▼──────────────────────┐
                          │         Rust Service (rust-service/)      │
                          │                                            │
                          │  IPC Server ──▶ AbstractionEngine          │
                          │       │              │                    │
                          │       │              ▼                    │
                          │       │         SQLite (abstraction_map,  │
                          │       │          raw_event_buffer)        │
                          │       │              │                    │
                          │       │              ▼                    │
                          │       │       UploadBatcher/Coordinator   │
                          │       │              │                    │
                          │       ▼              ▼                    │
                          │  AuthManager ──▶ HTTPS ──▶ velvt-core API │
                          └────────────────────┬──────────────────────┘
                                               │ IPC push
                                               │ (insight/history/alerts/
                                               │  auth/notification)
                          ┌────────────────────▼──────────────────────┐
                          │         Swift Client (swift-client/)      │
                          │  IPC Client ──▶ AccountStateManager ──▶    │
                          │  DisplayDataCoordinator ──▶ ViewModels ──▶ │
                          │  Menu Bar UI / Notifications               │
                          └────────────────────────────────────────────┘
```

## Return path

`velvt-core` → Rust `FetchService` (on a 7-day rolling schedule, or
on-demand via `request_latest_insight`/`request_latest_history`) → SQLite
`history_cache`/`insight_cache` → `PushAdapter` → IPC `PushQueue` → Swift
`AccountStateManager` → `DisplayDataCoordinator` → `InsightViewModel` /
`HistoryViewModel` (`@Published` properties) → `MenuBarPopoverView`.

Daily insights additionally produce a `notification_payload` IPC push (see
"IPC framing and versioning" below), consumed by
`NotificationDeliveryCoordinator` → `UNNotificationScheduler`.

## Module responsibility table

| Module | Language | Responsibility | Key protocols/traits | Privacy role |
|---|---|---|---|---|
| R1 — IPC transport | Rust | Unix socket framing, version handshake, malformed-frame rejection | `MessageRouter`, `IpcTransport` | Rejects unparseable frames without echoing content |
| R2 — Abstraction | Rust | Three-tier classification (seed dictionary, embedding similarity, unlogged fallback) | `ClassificationPlugin`, `TitleAbstractor` | **The privacy enforcement boundary** — raw fields cannot leave this module |
| R3 — Persistence | Rust | SQLite schema, migrations, DAL | `*Repo` traits in `persistence::traits` | Writes persist abstracted fields plus two disclosed device-local display columns on `raw_event_buffer`: `local_display_label` (friendly UI string, e.g. `Gmail`) and `local_name_suggestion` (the raw app name, only for classifier misses, for the one-tap rename). Neither is reachable from `upload/`; see PRIVACY.md. **Correction 2026-08-21:** this row previously said `local_display_label` is "forced to `NULL` by the DAL" — it is populated on ~97% of rows in a live database, and that claim was false |
| R4 — Auth | Rust | Device registration, token refresh/reissue, auth state machine | `DeviceRegistrar`, `HttpClient`, `TokenStore` | Tokens never touch SQLite or logs (`RedactedString`, Keychain) |
| R5 — Upload | Rust | Batch assembly, retry/backoff, privacy-rejection handling | `BatchUploader`, `EventIngestor` | Constructs the one DTO that crosses to the cloud; enforces `raw_field_rejected` is terminal |
| R6 — Delivery (fetch) | Rust | History/insight fetch, caching, proactive push | `CacheManager`, `Fetchable` | Read-only from cloud; never re-derives raw content |
| R7 — Delivery (push) | Rust | IPC push queue, reconnect-aware delivery, account-auth relay, raw-event ingestion | `PushAdapter`, `AccountAuthService` | Routes `sign_up`/`log_in` credentials to the cloud without ever persisting them |
| R8 — Lifecycle | Rust | Retention scheduling, graceful shutdown | `RetentionTarget`, `CancellationToken` | Enforces TTLs so abstracted data does not accumulate indefinitely |
| S1 — IPC scaffold | Swift | Unix socket client, version handshake | `IPCClientProtocol` | Sends raw events to Rust (the one designed crossing point); never calls the cloud |
| S2 — Collection | Swift | AXObserver lifecycle, focus/title capture | `CollectionAgentProtocol`, `EventSink` | Captures raw content but never persists or logs it |
| S3 — Permissions | Swift | Accessibility/Notifications permission state | `PermissionManaging` | Gates collection start on granted permission |
| S4 — Event relay | Swift | In-memory ring buffer while IPC is offline | `EventRelayProtocol` | Drops oldest on overflow; never spills to disk |
| S5 — Auth/onboarding | Swift | Sign up/log in/log out/delete account UI, Keychain session storage | `AccountStateManaging`, `KeychainProtocol` | Session tokens in Keychain only, never SQLite |
| S6 — Display | Swift | History/insight view models and views | `DisplayDataCoordinating` | Renders only abstracted, server-derived summaries |
| S7 — Menu bar & notifications | Swift | Menu bar state, notification scheduling | `NotificationScheduling` | Schedules exactly the Rust-authored copy; never generates notification text itself |

## The three-tier classification pipeline

1. **Tier 1 — exact match:** `SeedDictionaryPlugin` matches the app name
   against the versioned taxonomy's glob patterns. Deterministic,
   sub-millisecond (measured p95 ≈ 5 µs — see `PERFORMANCE_REPORT.md`).
2. **Tier 2 — embedding similarity:** `EmbeddingSimilarityPlugin` embeds the
   app name and window title and compares against static category centroids.
   Which embedder runs depends on the build, and the difference is stated here
   rather than left to be discovered:

   - **Shipped builds run `builtin-hash-v1`.** `main.rs` chains
     `.or_else(EmbeddingSimilarityPlugin::builtin(...))`, so when no ONNX model
     is configured Tier 2 does not turn off — it runs with
     `HashedEmbeddingModel`, a 256-dimension hashed sketch over seven built-in
     category phrase sets. `scripts/build_rust_helper.sh` builds the
     distributable without `--features onnx`, so this is what every DMG does.
     The live founder install has classified 7,009 events under
     `builtin-hash-v1` and zero under anything else.
   - **Developer builds may run MiniLM**, when a Tier 2 model and centroid file
     are configured and valid and the binary was built with `--features onnx`.
     Classification quality is better; nothing else about the pipeline changes.

   An operator who explicitly configures a model that then fails to load gets a
   structured warning and a `ServiceStatus::Degraded` IPC push. Never
   configuring one is not a degradation and does not raise it.

   Tier 2 writes: the sketch is cached in `semantic_embedding_cache`, keyed by a
   hash of `"{app_name} [SEP] {window_title}"`. Individual words are partially
   recoverable from that sketch. See `PRIVACY.md` and `PRIVACY_AUDIT.md`
   Audit 7 — this is the one place in the pipeline where a window title leaves a
   durable trace, and it is on disk only.
3. **Tier 3 — fallback:** `UnloggedFallbackPlugin` classifies anything
   unmatched as `unclassified`/`UNLOGGED` rather than dropping the event.

Plugins run in registry order; the first match wins. `AbstractedEvent` is
the only output type, and it structurally cannot carry a raw field.

## The IPC framing and versioning protocol

Newline-delimited JSON over a Unix domain socket at the path in
`proto/ipc_socket_path`. Every message is a tagged
`{"type": "...", "payload": {...}}` envelope. The current breaking-change
version is in `proto/version` (authoritative — do not trust prose copies of
the number); a version bump requires
coordinated updates to `proto/schema/`, `rust-service/shared-types`,
`swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`, and
`swift-client/Configs/{Debug,Release}.xcconfig` in the same commit (see
`proto/CHANGELOG.md` "Version-Bump Process" — and its own changelog entry
for v7 describing a real instance where that process was *not* followed
and had to be retroactively closed during this MVP integration pass).
Unknown future server discriminators decode as `ServerMessage.unknown(type:)`
on the Swift side so older clients degrade gracefully rather than crashing.

## The auth state machine

```
Unauthenticated ──(device registered, tokens issued)──▶ Authenticated{device_id}
Authenticated ──(token near expiry)──▶ RefreshInFlight ──▶ Authenticated
Authenticated ──(401 invalid_credentials/token_expired)──▶ NeedsReauth
Authenticated ──(403 device_token_revoked)──▶ [reissue attempt] ──▶ Authenticated | DeviceRevoked
Authenticated ──(403 device_revoked, or reissue failure)──▶ DeviceRevoked (terminal)
NeedsReauth ──(successful login)──▶ Authenticated
```

`DeviceRevoked` and `NeedsReauth` are also pushed to Swift over IPC
(`device_revoked`/`needs_reauth` messages) independent of any in-flight
request, so the UI can react even if the user isn't actively triggering a
network call.

On relaunch, Swift replays any Keychain-backed `auth_session` only after the
IPC client reports `.connected`. This prevents the session handoff from being
lost during the app/service startup race and keeps Rust's upload/fetch path in
sync with Swift's local account state.

## The graceful shutdown sequence

On SIGTERM/SIGINT: push `ShuttingDown` to all connected clients (urgent,
ahead of any queued payload) → cancel the shared `CancellationToken` →
flush the in-flight upload batch → wait (bounded by
`VELVT_SHUTDOWN_DEADLINE_SECONDS`, default 10s) for the fetch, upload-retry,
IPC server, retention, and flush tasks to finish → drop the SQLite
connection (clean close, no dangling WAL). Verified by
`tests/e2e_integration.rs::path7_graceful_shutdown_delivers_shutting_down_before_socket_close`
and `tests/lifecycle.rs`.

## Performance budget table

See [`PERFORMANCE_REPORT.md`](PERFORMANCE_REPORT.md) for full methodology,
caveats, and the testing environment. Summary of what was actually
measured in this pass:

| Budget | Measured | Enforced by | Status |
|---|---|---|---|
| Tier 1 p95 < 1 ms | 4.86 µs | `make test-rust`, every pull request | PASS |
| Tier 2 p50 < 10 ms | 21.6 µs (fake model) | `make test-rust`, every pull request, against both the fake model and the builtin one | PASS |
| Tier 2 p95 < 25 ms | 42.3 µs (fake model) | `make bench-rust`, the `bench` job on pushes to `main` | PASS, real-model latency not independently verified |
| Idle CPU < 0.5% | 0.0% over a 7s sample | nothing automated | PASS (shorter window than the 60s target) |
| Rust RSS < 50 MB | 6.7–6.9 MB over a 7s sample | nothing automated | PASS (shorter window than the 10-min target) |
| IPC round-trip p95 < 50 ms | not measured | nothing automated | infrastructure gap, not a failure |
| SQLite queries < 5 ms p95 at 30-day scale | not measured | nothing automated | infrastructure gap, not a failure |
| Swift RSS < 80 MB | not measured | nothing automated | no GUI session available in this environment |

The three tier rows are the only ones a build can fail on; the rest are
measurements from one pass and nothing re-runs them. The tests are
`tests/abstraction_engine.rs::tier1_is_deterministic_and_completes_under_one_millisecond`
and, in `tests/embedding_similarity.rs`,
`tier2_median_is_under_ten_milliseconds_with_available_model`,
`builtin_tier2_median_is_under_ten_milliseconds` and
`tier2_p95_is_under_twenty_five_milliseconds_with_available_model`. The tail
bound is `#[ignore]`d out of the correctness suite and measured after merge: a
25 ms wall-clock budget on a shared runner fails under a busy neighbour about
as readily as under a regression. The cost of that placement is that a Tier 2
tail regression is caught on `main` within one merge rather than before it
lands. `make bench-rust` fails when it measures nothing, so deleting the test
does not quietly retire the number, and it builds `--release`, so what it
measures is the build that ships.

The Measured column is the fake model in both Tier 2 rows, which is what the
pass in `PERFORMANCE_REPORT.md` ran. `builtin_tier2_median_is_under_ten_milliseconds`
is the one assertion here that times `HashedEmbeddingModel`, the model `main.rs`
loads when no ONNX artifacts are configured. Its cost is not independent of
window-title length — `classify` hashes the untruncated app-and-title into a
cache key before `embed` truncates to 4096 characters — so the table's Tier 2
numbers describe an ordinary title, not a long one.
