# Velvt IPC Contract

## 1. Architecture and Ownership

Velvt has two local workspaces with a strict ownership boundary:

- The Swift client owns macOS event capture, Accessibility permission handling,
  the menu bar UI, local notification scheduling, and display of payloads that
  are already ready for users.
- The Rust service owns raw event ingestion, abstraction, SQLite persistence,
  authentication, upload batching, cloud synchronization, and delivery of
  ready-to-display payloads back to Swift.

The workspaces communicate exclusively through a Unix domain socket using
newline-delimited JSON. `proto/` is the source of truth for every message type,
field name, protocol version, and the canonical socket path. Neither workspace
may invent fields or messages outside that contract.

Swift never calls cloud APIs. Rust is the final privacy enforcement boundary
before data can leave the device.

## 2. Message Flow

Each JSON object is encoded on one line and terminated by a single line-feed
byte (`\n`). A receiver must read one complete line before decoding a message.
Embedded unescaped newline bytes are not valid framing.

```text
Swift Client                                      Rust Service
     |                                                  |
     |--- connect ~/.velvt/velvt-service.sock --------->|
     |<-- server_hello ---------------------------------|
     |--- client_hello -------------------------------->|
     |<-- acknowledged / version_mismatch --------------|
     |                                                  |
     |--- raw_event ----------------------------------->|
     |<-- raw_event_ack --------------------------------|
     |                                                  |
     |<-- service_status -------------------------------|
     |<-- insight_payload ------------------------------|
     |<-- history_payload ------------------------------|
     |                                                  |
     |--- error_response ------------------------------>|
     |<-- error_response -------------------------------|
     |                                                  |
     |--- disconnect ---------------------------------->|
```

The first message on every new connection must be `server_hello`. Rust must
not process later messages until it has received a matching `client_hello`
and sent `acknowledged`.

Direction is enforced by the workspace message envelopes:

- Rust accepts only `client_hello`, `raw_event`, and `error_response`.
- Rust emits only `server_hello`, `acknowledged`, `version_mismatch`,
  `malformed_message`, `raw_event_ack`, `insight_payload`, `history_payload`,
  `service_status`, and `error_response`.
- Swift sends only the Rust inbound set and accepts only the Rust outbound set.

## 3. Message Catalog

All schemas use JSON Schema draft-07 and reject undeclared fields with
`additionalProperties: false`.

Every message uses a `type` discriminant and a `payload` object. Catalog fields
listed below live inside `payload`.

Optional-field encoding is strict. Omit absent `rejection_reason`,
`drop_reason`, `reason`, and `related_event_id` properties entirely. Only
`raw_event.bundle_id` is nullable and may be encoded as JSON `null`.

### `client_hello`

Direction: Swift to Rust. Purpose: respond to the server hello with the
expected protocol version.

- `type`: literal `client_hello`
- `expected_protocol_version`: positive integer matching `proto/version`
- `client_version`: semantic-version string

### `server_hello`

Direction: Rust to Swift. Purpose: declare the server protocol version.

- `type`: literal `server_hello`
- `protocol_version`: positive integer matching Rust's supported version

### `raw_event`

Direction: Swift to Rust. Purpose: deliver one local-only captured macOS event
for abstraction.

- `type`: literal `raw_event`
- `event_id`: UUID v4
- `occurred_at`: ISO 8601 UTC timestamp ending in `Z`
- `app_name`: raw local-only application name
- `window_title`: raw local-only focused-window title
- `bundle_id`: optional nullable raw local-only bundle identifier

### `raw_event_ack`

Direction: Rust to Swift. Purpose: acknowledge receipt or explain a safe drop.

- `type`: literal `raw_event_ack`
- `event_id`: UUID v4 of the acknowledged event
- `status`: `accepted` or `dropped`
- `drop_reason`: required only when status is `dropped`; must contain no raw
  event content

### `insight_payload`

Direction: Rust to Swift. Purpose: deliver one ready-to-display daily insight.

- `type`: literal `insight_payload`
- `date`: calendar date formatted `YYYY-MM-DD`
- `text`: ready-to-display insight copy
- `confidence_level`: `low`, `medium`, or `high`
- `low_confidence`: explicit low-confidence display flag
- `generated_at`: ISO 8601 UTC timestamp ending in `Z`

### `history_payload`

Direction: Rust to Swift. Purpose: deliver a ready-to-display multi-day history.

- `type`: literal `history_payload`
- `days`: non-negative number of requested days
- `summaries`: array of daily summary objects

Each summary contains:

- `date`: calendar date formatted `YYYY-MM-DD`
- `status`: `ready` or `no_data`
- `event_count`: non-negative abstracted-event count
- `focus_score`: derived number or null
- `fragmentation_score`: derived number or null
- `confidence_level`: `low`, `medium`, or `high`
- `active_seconds`: non-negative active duration

### `service_status`

Direction: Rust to Swift. Purpose: notify Swift of service health.

- `type`: literal `service_status`
- `state`: `ready`, `degraded`, `upload_paused`, or `auth_required`
- `reason`: optional safe diagnostic reason

### `error_response`

Direction: either direction. Purpose: provide a typed, safe error envelope.

- `type`: literal `error_response`
- `code`: machine-readable snake_case error code
- `message`: human-readable safe message
- `related_event_id`: optional UUID v4

Error messages and reasons must never contain raw event content, tokens, or
insight text.

## 4. Version Negotiation

The current version is the integer stored in `proto/version`.

### Matching Version

1. Rust sends `server_hello` immediately after Swift connects.
2. Swift sends `client_hello` with its expected protocol version.
3. Rust sends `acknowledged`.
4. Both sides may exchange other messages.

### Mismatched Version

1. Rust sends `version_mismatch` with both numeric protocol versions.
2. Rust does not process later messages on that connection.
3. The connection closes cleanly. Messages must never be silently dropped
   because of a version mismatch.

### Future Versions

Backward-compatible documentation clarifications do not require a bump.
Changes that remove, rename, reinterpret, or newly require fields require a
version bump. New message types also require a version bump. Because schemas
are closed, additive optional fields still require coordinated schema and DTO
updates in both workspaces.

## 5. Privacy Boundary

Raw identifying data includes application names, window titles, bundle IDs,
URLs, paths, filenames, contact names, email addresses, and raw text.

Only `raw_event` may carry raw identifying values. It travels locally from
Swift to Rust and must never be reused as an upload payload. `raw_event_ack`
may identify the source event only by UUID and must not echo raw values.

All other messages are privacy-safe control messages, derived summaries, or
ready-to-display payloads. Their schemas contain a top-level privacy comment,
and their closed object definitions prevent undeclared raw fields.

Rust must abstract raw events before they enter an upload queue. Upload-facing
types must accept abstracted events only. Raw fields must never appear in
abstracted-event tables, upload-batch tables, outbound HTTP payloads, logs, or
error text.

## 6. Extension Policy

To add or change an IPC message:

1. Confirm that the change must cross the IPC boundary.
2. Update or add the draft-07 schema under `proto/schema/`.
3. Keep the schema closed with `additionalProperties: false`.
4. Add the required privacy comment unless the message is the sole raw-event
   carrier.
5. Bump `proto/version` for incompatible changes or new message types.
6. Update `proto/CHANGELOG.md`.
7. Update Rust DTOs and contract tests; register business handling only when
   the issue implementing that behavior is in scope.
8. Update Swift DTOs, dispatch, and tests.
9. Verify forbidden raw fields cannot appear in privacy-safe messages or
   upload payloads.
10. Land proto, Rust, and Swift changes atomically in the same commit.

Partial protocol changes must not be merged.

Before merge, validate each schema against the draft-07 meta-schema, verify
that every non-optional property is listed in `required`, verify all enumerated
strings use `enum`, and audit all property names for raw identifying fields or
synonyms. Add contract tests in both workspaces that encode representative
messages and compare their JSON keys and discriminator values to the schemas.

### R1 Rust IPC Server Implementation Checklist

1. Read the socket path and protocol version from typed configuration sourced
   from `proto/ipc_socket_path` and `proto/version`; do not hardcode either.
2. Bind the Unix domain socket and use newline-delimited JSON framing.
3. Decode only Rust `ClientMessage` variants.
4. Send `server_hello`, then require `client_hello` with an exact
   protocol-version match.
5. On mismatch, send `version_mismatch`, then close cleanly.
6. Do not dispatch `raw_event` until the handshake is accepted.
7. Send only Rust `ServerMessage` variants and omit absent optional fields
   according to the message-catalog rule above.
8. Never log decoded message content or echo raw fields in errors.
9. Add schema-contract tests for every inbound and outbound message.

### R1 Extensibility Proof

`ClientMessage` is non-exhaustive outside `velvt-shared-types`, and the R1
default router validates post-handshake DTOs without enumerating normal
variants. A test-only `dummy_extension` variant in the shared-types unit tests
proves that a tagged DTO variant can be added and serialized without changing
existing service handler or transport files. The same compile proof is
available with `cargo check --workspace --features
velvt-shared-types/extensibility-proof`. Production message additions still
require the coordinated `proto/`, Rust DTO, Swift DTO, and versioning steps
above.

### S1 Swift IPC Client Implementation Checklist

1. Read the socket path, protocol version, and client version from typed
   configuration; do not hardcode them.
2. Connect with a Unix domain socket, not `URLSession`.
3. Send `client_hello` after receiving `server_hello`.
4. Do not report `connected` or send `raw_event` until Rust accepts the
   handshake.
5. Encode only `OutboundIPCMessage` variants and decode only
   `InboundIPCMessage` variants.
6. Preserve exact schema field names, discriminator values, timestamp formats,
   and optional-field omission rules.
7. Reconnect with exponential backoff and keep raw events only in a bounded
   30-second in-memory buffer.
8. Never log message content.
9. Add schema-contract tests for every outbound and inbound message.

## 7. Socket Lifecycle

### Startup

The canonical path is read from `proto/ipc_socket_path`; neither workspace may
hardcode it. Rust expands `~`, creates the parent directory with user-only
permissions, and binds the Unix domain socket. Swift opens the connection at
application launch.

Rust configuration overrides:

- `VELVT_IPC_SOCKET_PATH`: Unix socket path override.
- `VELVT_IPC_MAX_ERRORS`: positive malformed-frame threshold per connection.
- `VELVT_LOG_LEVEL`: structured tracing filter, defaulting to `info`.

### Stale Socket Handling

Before binding, Rust checks whether a socket entry already exists. It first
attempts to connect:

- If connection succeeds, another healthy service owns the socket and startup
  must stop without deleting it.
- If connection fails because no listener exists, Rust removes only that stale
  socket entry and then binds.

Rust must never recursively delete the socket parent directory.

### Reconnect and Buffering

If the service is unavailable, Swift reconnects with exponential backoff. Raw
events may be buffered in memory for at most 30 seconds. Swift drops the oldest
expired events beyond that window and never persists the reconnect buffer to
SQLite. Every successful reconnection starts with a new handshake.

### Clean Shutdown

On shutdown, Rust stops accepting connections, flushes the pending privacy-safe
upload batch, closes active socket connections, and removes its socket file.
Swift closes its connection and tears down active observers. Socket failures
may be logged by safe error code and socket path only; message content must
never be logged.
