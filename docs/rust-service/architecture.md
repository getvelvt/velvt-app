# Rust Service Architecture

The Rust service is organized around narrow modules that own one runtime concern each. `src/main.rs` is the composition root: it wires concrete implementations together, starts background tasks, and handles shutdown. Library code under `src/` stays testable through traits and DTOs.

## Module Map

| Path | Role |
|---|---|
| `src/main.rs` | Runtime composition, service startup, task spawning, signal handling, graceful shutdown |
| `src/config/` | Validated runtime/build configuration |
| `src/ipc/` | Unix socket transport, framing, version negotiation, reconnect tracking, message routing |
| `src/abstraction/` | Taxonomy loading, classification plugins, stable key generation, abstracted event creation |
| `src/persistence/` | SQLite opening, migrations, repository traits, database models |
| `src/upload/` | Batch assembly, queueing, retry/backoff, cloud upload transport |
| `src/auth/` | Account auth relay, token store traits, auth state machine, refresh/reissue logic |
| `src/delivery/` | Fetching history/insight, cache management, payload shaping, IPC push adapter |
| `src/retention/` | Scheduled cleanup for raw events, sent/rejected batches, and caches |
| `src/lifecycle/` | Cancellation token used by long-running tasks |
| `shared-types/` | IPC DTOs and protocol constants shared by service tests and Swift-equivalent schemas |

## Startup Path

Startup is intentionally fail-fast. If required configuration, SQLite, taxonomy, or abstraction setup fails, the service logs a safe error code and exits instead of running partially initialized.

High-level startup sequence:

```text
ServiceConfig::load
tracing init
duplicate socket listener check
SqlitePersistence::open
Taxonomy::from_path
AbstractionEngine::builder
Auth/token/device state setup
PushAdapter + ReconnectTracker setup
FetchScheduler task
Upload retry task
Age-based flush task
RetentionScheduler task
TokioUnixTransport task
signal wait
graceful shutdown
```

This composition keeps cross-module dependencies visible in one place.

## IPC Layer

`ipc::transport` owns socket listening, connection lifecycle, server hello, handshake, per-frame decoding, write timeouts, and shutdown behavior. `ipc::router` owns the meaning of validated post-handshake messages.

The split matters because transport correctness and business routing fail differently:

- Transport rejects malformed or unsupported protocol traffic without echoing payload content.
- Router handles valid messages and returns typed `ServerMessage` values.

`ReconnectTracker` preserves a push queue across short Swift reconnects. This allows Rust to push service/auth/delivery events without assuming the UI is continuously connected.

## Abstraction Engine

The abstraction engine is the privacy boundary. It accepts `RawEvent` and returns an `AbstractedEvent` that structurally exposes only safe fields such as stable ID, abstract label, category, taxonomy version, and timestamp.

Classification is plugin-based:

1. Seed dictionary exact/glob matching from `resources/abstraction-taxonomy-mvp-1.json`.
2. Optional embedding similarity when the `onnx` feature and model/centroid paths are configured.
3. Unlogged fallback to `unclassified`/`UNLOGGED`.

The first matching plugin wins. Optional Tier 2 failure is degraded, not fatal, unless initialization of required baseline pieces fails.

## Persistence

SQLite access is centralized under `persistence/`. Repositories expose domain-oriented operations instead of allowing callers to build SQL strings.

Design decisions:

- Migrations are explicit SQL files in `rust-service/migrations/`.
- Hot-path cleanup and batching use indexed queries.
- Raw event storage is limited to privacy-safe audit/display fields after abstraction.
- Upload DTO construction reads from persisted abstracted events and upload batches, not from original raw IPC payloads.

The `local_display_label` field exists for local menu display and is never used in cloud DTOs.

## Upload Pipeline

The upload path has three layers:

1. `EventIngestor` receives abstracted events from the router.
2. `UploadBatcher` groups events by count, age, and shutdown.
3. `UploadCoordinator` sends batches through `HttpBatchUploader`, records status, and manages retry/backoff.

Batch IDs are stable for idempotent retry. `raw_field_rejected` is terminal because retrying a privacy-invalid payload would be incorrect.

## Delivery Pipeline

Delivery is the return path from cloud to UI:

```text
FetchScheduler / on-demand IPC request
FetchService
history_cache / insight_cache
shaper validation
PushAdapter
IPC push queue
Swift display coordinators
```

Rust shapes payloads before sending them to Swift so the UI does not need to know cache schema details or cloud response formats.

## Auth Architecture

Auth has two related flows:

- `AccountAuthService` handles user-triggered sign-up, login, logout, account deletion, and initial device registration.
- `AuthManager` wraps authenticated cloud requests and handles token refresh, token reissue, needs-reauth, and device-revoked transitions.

The auth state machine emits terminal transitions to Swift through IPC pushes. This lets the UI respond to `needs_reauth` or `device_revoked` even when the user did not initiate the request currently failing.

## Lifecycle and Shutdown

The service handles SIGTERM/SIGINT by:

1. Pushing `shutting_down` to connected clients.
2. Cancelling background tasks through `CancellationToken`.
3. Flushing the in-flight upload batch.
4. Waiting up to `VELVT_SHUTDOWN_DEADLINE_SECONDS`.
5. Dropping SQLite so connections and WAL files close cleanly.

This sequence exists because the Swift app can replace or restart the helper. The service must not keep locks or background tasks alive after termination.

## Testing Strategy

Tests exercise behavior at module and integration levels:

- DTO exact JSON shape in shared types.
- IPC handshake and malformed-message behavior.
- Abstraction stability and category fallback.
- Persistence migrations and retention targets.
- Upload privacy payloads and retry behavior.
- Auth refresh, reissue, device revocation, and account flows.
- End-to-end paths through IPC, abstraction, upload, delivery, and shutdown.
