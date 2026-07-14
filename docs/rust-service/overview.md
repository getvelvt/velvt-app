# Rust Service Overview

The Rust service is Velvt's local processing and privacy enforcement layer. It listens on a Unix domain socket for messages from the macOS app, accepts raw local activity events, abstracts them into safe labels and categories, persists local state in SQLite, batches uploads to the cloud, and pushes ready-to-display output back to Swift.

It does not render UI, request macOS permissions, or schedule notifications directly.

## Responsibilities

| Area | Responsibility |
|---|---|
| IPC | Unix socket server, newline-delimited JSON framing, version handshake, malformed-message rejection |
| Abstraction | Convert raw `app_name` and `window_title` into stable local identifiers, labels, and categories |
| Persistence | Own SQLite migrations and repositories for abstraction mappings, event buffer, upload batches, cache data, and backoff state |
| Upload | Assemble cloud-safe batches, retry with backoff, handle terminal privacy rejection |
| Auth | Relay sign-up/login, register devices, store process-local tokens, refresh tokens, handle reauth and revocation |
| Delivery | Fetch history and insight payloads, cache them locally, push shaped messages to Swift |
| Lifecycle | Retention scheduling, graceful shutdown, duplicate service detection |

## Why Rust Owns the Privacy Boundary

Swift must see raw app/window information to collect macOS Accessibility events. Rust owns the transition from raw local data to cloud-safe derived data. Centralizing that boundary keeps upload DTOs, database writes, logging rules, and tests in one place.

The critical invariant is:

```text
raw_event -> abstraction_engine -> abstracted_event -> upload DTO
```

Raw fields may enter the service through IPC and may be used by the abstraction engine. They must not appear in upload payloads or logs.

## Runtime Shape

At startup, `src/main.rs`:

1. Loads `ServiceConfig`.
2. Initializes structured tracing.
3. Checks for another service already listening on the socket.
4. Opens SQLite and applies migrations.
5. Loads the abstraction taxonomy and optional Tier 2 embedding plugin.
6. Builds the auth, delivery, upload, retention, and IPC components.
7. Runs until SIGTERM or SIGINT, then performs graceful shutdown.

## Main Inputs

Swift sends:

- `client_hello`
- `raw_event`
- `request_latest_insight`
- `request_latest_history`
- `sign_up`
- `log_in`
- `auth_session`
- `log_out`
- `delete_account`
- `request_menu_status`
- `flush_upload_queue`

All client messages are typed in `rust-service/shared-types/src/lib.rs` and validated by serde. Most corresponding JSON schemas live under `proto/schema/`.

## Main Outputs

Rust sends:

- `server_hello`
- `acknowledged` or `version_mismatch`
- `raw_event_ack`
- `insight_payload`
- `history_payload`
- `notification_payload`
- `menu_status`
- `service_status`
- `needs_reauth`
- `device_revoked`
- `privacy_violation_alert`
- `shutting_down`
- auth success/failure/session update messages

Swift treats these as display, state, notification, or auth events. It does not inspect Rust's internal abstraction map or upload state directly.

## Local Storage

The service stores local state in SQLite at `~/.velvt/velvt-service.sqlite3` by default. `VELVT_DATABASE_PATH` can override this path. Tests can use `:memory:`.

Tokens are intentionally not stored in SQLite. The current implementation uses `VolatileTokenStore` with in-process state and an auth-session update push to Swift, which persists the host session in Keychain.

## Cloud Interfaces

The service talks to the cloud with `reqwest` through small HTTP abstractions. The compiled API base URL comes from build-time configuration. Runtime upload and fetch requests use authenticated device tokens once available.

Cloud-bound event batches must contain only abstracted and derived fields.

## Tests

Important test files include:

- `rust-service/tests/upload_batching.rs` for upload payload shape and privacy fields.
- `rust-service/tests/ipc_connection.rs` and `tests/unix_socket_smoke.rs` for IPC behavior.
- `rust-service/tests/auth_flow.rs` and `tests/auth_state.rs` for auth behavior.
- `rust-service/tests/persistence_contract.rs` and `tests/retention.rs` for database behavior.
- `rust-service/tests/e2e_integration.rs` for integrated paths.
