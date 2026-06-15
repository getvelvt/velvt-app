# Velvt

Velvt is a privacy-first passive productivity intelligence system. A native
macOS client captures app-activation and focused-window events, then sends
those raw events over a local Unix domain socket to a Rust service. The Rust
service owns abstraction, persistence, upload batching, cloud synchronization,
and delivery of ready-to-display insights back to the client.

Raw app names, bundle IDs, window titles, URLs, paths, filenames, contacts, and
other identifying user data never leave the device. The Rust service is the
final privacy enforcement boundary before any cloud request.

## Workspaces

- `swift-client/`: SwiftUI/AppKit menu bar app for event capture, local IPC,
  permissions, notifications, and insight display. It never calls cloud APIs.
- `rust-service/`: Rust local service for IPC ingestion, abstraction, SQLite,
  upload batching, authentication, cloud sync, and insight delivery.
- `proto/`: canonical newline-delimited JSON IPC contract shared by both
  workspaces.
- `cloud/`: reserved for the separately scoped FastAPI backend.

## Build and Test

Prerequisites are a pinned stable Rust toolchain and a macOS Swift toolchain.

```sh
make build-all
make test-all
make lint-rust
make lint-swift
```

Workspace-specific commands are also available:

```sh
make build-rust
make test-rust
make build-swift
make test-swift
```

### Build Both Local Targets

From the repository root, build the Rust service and macOS-only Swift app
without changing either workspace's build configuration:

```sh
make build-all
```

The native macOS application target is `velvt-mac` in
`swift-client/VelvtMac.xcodeproj`. It produces `velvt-mac.app`; SwiftPM remains
the unit-test harness.

To run the SwiftPM development executable against the local service, source the
canonical socket path and protocol version from `proto/`:

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client velvt-mac
```

Build the native app directly with:

```sh
xcodebuild \
  -project swift-client/VelvtMac.xcodeproj \
  -scheme velvt-mac \
  -destination 'generic/platform=macOS' \
  build
```

## macOS IPC Client

The macOS app communicates with the Rust service exclusively through
`IPCClientProtocol`. `UnixSocketIPCClient` is constructed only in the AppKit
composition root; other production modules and tests depend on the protocol or
`FakeIPCClient`.

```swift
let client: any IPCClientProtocol = FakeIPCClient()
try await client.connect()
try await client.send(.errorResponse(...))

for await message in client.incomingMessages {
    // Route the typed server message without inspecting raw event payloads.
}
```

`connectionStatus` publishes `disconnected`, `connecting`, `handshaking`,
`connected`, and reconnect-attempt state for UI observation. Calls to `send`
before a completed handshake throw `IPCError.notConnected`.

### Version Handshake

IPC uses newline-delimited JSON and protocol version `proto/version`:

1. Rust sends `server_hello`.
2. Swift sends `client_hello` with its protocol and application versions.
3. Rust sends `acknowledged` or `version_mismatch`.

Swift does not publish `connected` or permit public sends before
`acknowledged`. A mismatch throws
`IPCError.versionMismatch(expected:got:)`, closes the connection, and stops
reconnect attempts until the application is updated or restarted.

### Socket Path Configuration

Both workspaces use `proto/ipc_socket_path` as the canonical default. The
macOS app receives the runtime path through `VELVT_SOCKET_PATH`; it also
requires `VELVT_PROTOCOL_VERSION` and `VELVT_CLIENT_VERSION`. The IPC client
expands `~` before connecting and reports a missing socket as a typed
`IPCError.socket` while scheduling reconnect.

### Adding an IPC DTO

IPC contract changes are cross-workspace changes:

1. Add or update the closed JSON schema in `proto/schema/` and bump
   `proto/version` when required.
2. Update the Rust tagged enum and DTO in
   `rust-service/shared-types/src/lib.rs`.
3. Add the Swift DTO and its `ClientMessage` or `ServerMessage` tagged-enum
   case in `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`.
4. Add encode/decode round-trip tests and register the message handler.

Unknown future server discriminators decode as `ServerMessage.unknown(type:)`.
Only the discriminator is retained; unknown payload fields are discarded so
they cannot leak raw values and existing handler switches do not require
exhaustive updates.

Rust reads its default socket path from `proto/ipc_socket_path`.
`VELVT_IPC_SOCKET_PATH` overrides it, `VELVT_IPC_MAX_ERRORS` configures the
malformed-frame threshold, and `VELVT_LOG_LEVEL` configures structured tracing.

All protocol-v3 messages use a tagged `{"type": "...", "payload": {...}}`
envelope. Rust DTOs live in `rust-service/shared-types`; Swift DTOs live in
`swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`.

## Architecture

Start with [`docs/architecture/`](docs/architecture/) for architecture and IPC
contract documentation. Contributors must also read [`AGENTS.md`](AGENTS.md)
and [`CONTRIBUTING.md`](CONTRIBUTING.md) before making changes.

The macOS collection-agent lifecycle, private AX run-loop threading model, and
no-polling rules are documented in
[`docs/architecture/collection-agent.md`](docs/architecture/collection-agent.md).
The two-permission allowlist, onboarding rationale, monitoring behavior, and
recovery steps are documented in
[`docs/architecture/permissions.md`](docs/architecture/permissions.md).

## On-Device Classification

The Rust abstraction engine applies a privacy-preserving three-tier pipeline:

1. **Exact match:** the versioned taxonomy seed dictionary matches exact or
   glob application-name patterns.
2. **Embedding similarity:** an optional local ONNX sentence-embedding model
   compares an app-name/window-title embedding with static category centroids.
3. **Fallback:** unmatched or unavailable Tier 2 requests become
   `unlogged` / `UNLOGGED`.

Plugins run in registry order and the first match wins. The built-in registry
is in `AbstractionEngineBuilder::register_builtin_plugins_with_embedding`:

```rust
// This is the only line to change when registering a new classification plugin.
let builder = builder.register_plugin(NewClassificationPlugin::new(...));
```

`AbstractedEvent` contains only a stable local ID, label, category, taxonomy
version, timestamp, and internal-only classification tier. Raw app names and
window titles never enter this type or its serialized output.

### Model Artifacts

Tier 2 targets an Apache-2.0 licensed, INT8-quantized ONNX export of
`sentence-transformers/all-MiniLM-L6-v2`. The model must be at most 50 MB and
must be accompanied by its `tokenizer.json` and a version-matched centroid
file. Model training, fine-tuning, and artifact generation happen offline and
are intentionally not part of the service.

Install an approved model artifact bundle by placing its files together and
configuring:

```sh
export VELVT_ABSTRACTION_MODEL_PATH=/path/to/model.onnx
export VELVT_ABSTRACTION_CENTROIDS_PATH=/path/to/centroids.bin
export VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS=20
export VELVT_ABSTRACTION_SIMILARITY_THRESHOLD=0.72
```

Do not configure arbitrary downloaded models. The tokenizer, model output
shape, centroid dimension, taxonomy version, and model license must be reviewed
together. If the model or centroids are unavailable or invalid, Tier 2 is
disabled with a structured warning and Tier 1/Tier 3 continue.

### Centroid File

Centroids are static companion data and are never recomputed at runtime. The
binary format is:

```text
"VELVTC01"
taxonomy_version_length: u32 little-endian
taxonomy_version: UTF-8 bytes
embedding_dimensions: u32 little-endian
centroid_count: u32 little-endian
repeated centroid_count times:
  category_length: u32 little-endian
  category: UTF-8 bytes
  embedding_dimensions float32 little-endian values
```

The file taxonomy version must match the configured taxonomy. See
[`CONTRIBUTING.md`](CONTRIBUTING.md#adding-a-classification-category) for the
offline update process.

### Taxonomy And Roadmap

The taxonomy is data loaded from
`rust-service/resources/abstraction-taxonomy-mvp-1.json`, or from
`VELVT_ABSTRACTION_TAXONOMY_PATH`. The API-expected version is currently
`mvp-1`; a configured mismatch emits a structured warning while the configured
version remains attached to results.

`TitleAbstractor` is wired into `AbstractionEngine::process`, with
`DefaultTitleAbstractor` passing titles through locally. V1 will replace
semantically sensitive title tokens with category-scoped abstract labels
without transmitting raw titles.

Performance gates are Tier 1 mean and p95 below 1 ms and Tier 2 p95 below
25 ms. The integration tests report p50, p95, and p99.

## Rust SQLite Persistence

The Rust service owns SQLite persistence. `VELVT_DATABASE_PATH` selects the
database file; `:memory:` uses the same DAL and migration paths for tests. The
production default is `~/.velvt/velvt-service.sqlite3`, and startup creates
missing parent directories and applies every pending embedded migration before
constructing the abstraction engine.

The six feature tables are:

| Table | Purpose |
|---|---|
| `abstraction_map` | Stable-key hash to stable ID, abstract label, category, and taxonomy version |
| `raw_event_buffer` | Short-lived privacy-safe abstracted event metadata |
| `upload_batch` | Idempotent upload batch state |
| `batch_event` | Privacy-safe events assigned to a batch |
| `history_cache` | Date-keyed ready-to-display summary payloads with TTL |
| `insight_cache` | Date-keyed ready-to-display insight payloads with TTL |

Every feature table has an auto-increment primary key and database-defaulted
`created_at`. Time/date lookup columns are indexed. The migration-owned
`schema_migration` table records each applied version, so startup never applies
the same migration twice.

Raw app names, window titles, URLs, bundle IDs, paths, filenames, contacts, and
other raw user content are forbidden in every schema column. Migration SQL
documents this invariant, and integration tests inspect the resulting schema.

### Adding A Migration

1. Add one sequentially numbered SQL file to `rust-service/migrations/`, such
   as `0003_add_feature_table.sql`.
2. Make the migration additive and include required constraints and indexes.
3. Do not edit the migration runner. `rust-service/build.rs` embeds all sorted
   migration files automatically.
4. Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
   `cargo fmt --check` from `rust-service/`.

`0002_harden_indexes_and_probe.sql` is the proof migration: it was added without
runner changes and tests verify it applies to a database containing only
version 1.

### Extending The DAL

Consumers depend on the narrow traits exported by `persistence`, never on
`rusqlite` or concrete SQLite internals. Add a new consumer by defining its
models and trait in `src/persistence/models.rs` and
`src/persistence/traits.rs`, implementing a SQLite repository in
`src/persistence/sqlite.rs`, and injecting only that trait into the consumer.
Multi-table writes belong on the trait and must use an explicit transaction.
No module outside `src/persistence/` may import or reference `rusqlite`.
