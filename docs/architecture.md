# Monorepo Architecture

Velvt is a privacy-first passive productivity intelligence system. It is split into a native macOS client and a local Rust service so raw local activity can be captured for immediate processing without allowing sensitive fields to cross the cloud boundary.

## System Boundaries

```text
swift-client/                         rust-service/                         cloud
macOS app                             local service                         backend
-------------------------------       -------------------------------       -----------------
NSWorkspace / AXObserver events  ->   IPC server
raw event relay                       abstraction engine
menu bar UI                           SQLite persistence
permissions                           upload batching                 ->    abstract events API
notifications                   <-    insight/history fetch + push     <-    insight/history API
Keychain session cache          <->   process-local token/session sync
```

The Swift client captures raw local signals and renders already-shaped output. The Rust service owns processing, persistence, cloud communication, and final privacy enforcement.

## Active Workspaces

| Workspace | Language | Responsibility |
|---|---|---|
| `swift-client/` | Swift | macOS event capture, IPC client, menu bar UI, onboarding, permissions, notifications, local Keychain session cache |
| `rust-service/` | Rust | Unix socket IPC server, raw event ingestion, abstraction, SQLite, upload queue, auth refresh, cloud sync, insight/history delivery |
| `proto/` | JSON Schema and plain text config | IPC message schemas, protocol version, canonical socket path |
| `cloud/` | Reserved | Future or separately scoped backend work |

## Why This Split Exists

The most important architectural decision is that the Rust service is the last gate before data leaves the device. Swift is allowed to observe raw app names and window titles because it is the macOS process integrated with AppKit and Accessibility APIs. Rust is responsible for turning that raw input into abstracted labels and cloud-safe payloads before any HTTP request is made.

This split gives Velvt three useful properties:

1. The UI stays thin and platform-native.
2. Privacy-sensitive processing is centralized in one service boundary.
3. The Rust service can be tested heavily around serialization, persistence, batching, and privacy invariants without driving a macOS UI.

## IPC Contract

Swift and Rust communicate over a Unix domain socket using newline-delimited JSON envelopes:

```json
{
  "type": "raw_event",
  "payload": {
    "event_id": "018f4f5d-0000-7000-8000-000000000001",
    "occurred_at": "2026-06-29T14:15:00Z",
    "app_name": "Example App",
    "window_title": "Example Window",
    "bundle_id": "com.example.app"
  }
}
```

Raw IPC events are local-only. The contract source of truth is:

```text
proto/schema/
proto/version
proto/ipc_socket_path
```

Connection startup is versioned:

1. Rust sends `server_hello`.
2. Swift sends `client_hello`.
3. Rust replies with `acknowledged` or `version_mismatch`.

Any change to `proto/` is a cross-workspace change. Update schema files, Rust DTOs in `rust-service/shared-types`, Swift DTOs in `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`, and version/config references together.

## Raw Event Flow

1. `AXCollectionAgent` observes app activation and focused-window title changes.
2. `EventRelay` receives `RawEvent` values and sends them to the IPC client.
3. If IPC is unavailable, `EventRelay` buffers in memory and drops old events when capacity is exceeded. It never writes raw events to disk.
4. Rust `R7Router` receives `raw_event`, passes it through `AbstractionEngine`, writes a privacy-safe audit row, and offers the abstracted event to the upload batcher.
5. Rust acknowledges the event with `raw_event_ack`.

The raw event exists on the IPC boundary and inside the Rust abstraction path. It must not appear in upload DTOs, logs, or cloud requests.

## Upload Flow

Rust batches abstracted events by count, age, and shutdown:

- Count threshold defaults to 50 events.
- Age threshold defaults to 60 seconds.
- Shutdown flush runs during graceful termination.

Upload payloads are constructed from abstracted labels, categories, timestamps, durations, and device metadata. If the cloud rejects a payload with `raw_field_rejected`, Rust treats that batch as permanently failed and surfaces a safe `privacy_violation_alert` to Swift.

## Delivery Flow

Rust fetches cloud-generated history and insight data, validates and shapes it, caches it in SQLite, and pushes ready-to-display IPC messages to Swift:

- `insight_payload`
- `history_payload`
- `notification_payload`
- `cache_empty`
- `service_status`

Swift does not generate insight text. It renders payloads and schedules notifications from Rust-authored copy.

## Authentication Boundary

Swift owns user-facing auth UI and persists a local session copy in Keychain. Rust owns cloud calls, device registration, token refresh, logout, deletion, and device revocation handling. Device-scoped tokens are used for normal API calls; user-scoped tokens are persisted only as recovery material for user refresh and device-token reissue after relaunch.

Credentials flow from Swift to Rust only over local IPC messages such as `sign_up` and `log_in`. Rust relays credentials to cloud auth endpoints and returns `auth_success` or `auth_failure`. Tokens are redacted in Rust logging and are not stored in SQLite.

## Persistence Boundary

Rust owns SQLite persistence. Swift currently persists only local UI/session state through Keychain and `UserDefaults`.

Rust database responsibilities include:

- `abstraction_map`
- `raw_event_buffer`
- `upload_batch`
- `batch_event`
- `history_cache`
- `insight_cache`
- `upload_host_backoff`

Migrations live in `rust-service/migrations/` and must be additive unless a migration path is explicitly designed.

## Deferred Areas

The repo contains seams for future work, but MVP behavior must not activate these by default:

- Local analytics engine or local LLM inference.
- Advanced dashboards, graphs, streak counters, and automation.
- Cross-platform client implementations.
- Backend work under `cloud/` unless explicitly scoped.

## Documentation Map

Start with `docs/DOC_INDEX.md` before changing docs. For subsystem-specific details, use:

- `docs/rust-service/architecture.md`
- `docs/swift-client/architecture.md`
- `docs/rust-service/api.md`
- `docs/swift-client/auth.md`
- `docs/architecture/` existing deep dives
