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

Prerequisites are:

- Rust/Cargo on `PATH` (`rust-service/rust-toolchain.toml` pins the toolchain).
- Swift 5.10 or later. Release builds and CI use Xcode 16.3 (`.xcode-version`).
- Python 3.13 (`.python-version`) for `scripts/`. Pins and lint gates:
  `docs/toolchains-and-lint.md`.
- Full Xcode selected with `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`
  for `xcodebuild`/`.app` targets. SwiftPM tests can run with Command Line
  Tools, but the Xcode targets cannot.

```sh
make build-all
make test-all
make lint-rust
make lint-swift
```

## Run the Local MVP

The device-local loop — collection, classification, work blocks and the drift
offer — needs no account and no backend. Build and open the app with
`make build-app` (below) and it runs on its own.

Sign-in, upload and cloud insights need a `velvt-core` API. `velvt-core` is a
separate, private repository and is not part of this one. Maintainers who have
the private Velvt workspace checkout (the directory that holds `velvt-app/` and
`velvt-core/` side by side) can bring up both with the workspace's
`run_velvt_local.sh`, run from that workspace root:

```sh
./run_velvt_local.sh --reset-local-cache
```

It opens Docker Desktop if needed, **starts** the existing `velvt-core` Docker
Compose containers (it never creates or rebuilds them), waits for readiness,
clears the local history/insight caches, and launches
`velvt-app/dist/Velvt.app`. `--rebuild` first replaces `dist/Velvt.app` with a
Debug build pointed at the local backend (`make build-app-local-core`), and
`--no-open --smoke` checks the artifact and backend without opening anything.
Without that workspace, use `make build-app-local-core` against a `velvt-core`
API you run yourself.

Workspace-specific commands are also available:

```sh
make build-rust
make test-rust
make build-swift
make test-swift
```

### Build a Single Runnable App

```sh
make build-app
```

Produces `dist/Velvt.app` — one double-clickable artifact with both the
Swift UI and the Rust service binary embedded at
`Contents/Resources/velvt-service`. It is a universal (arm64 + x86_64) Release
build compiled against `VELVT_API_BASE_URL` (default
`https://dev-api.getvelvt.com`; `scripts/preflight_distribution.sh` refuses a
non-HTTPS, local or private-network URL), then signed **ad hoc** by
`scripts/sign_release.sh local` and checked by `scripts/verify_release.sh`. An
ad-hoc bundle runs on the machine that built it and nowhere else; a build for
another Mac is `make alpha-dmg` (Developer ID signing plus notarization, see
[`docs/shipping-a-testable-dmg.md`](docs/shipping-a-testable-dmg.md)).
`AppDelegate` launches the bundled helper at
startup (see `ServiceProcessLauncher.swift`) and stops it on quit, so this
is the one command a real user (or you, verifying locally) needs to get a
working install — no exported environment variables, no separate terminal
running the Rust service. `Info.plist` ships built-in `VELVT_SOCKET_PATH`/
`VELVT_PROTOCOL_VERSION`/`VELVT_CLIENT_VERSION` defaults for exactly this
reason; keep them in sync with `proto/` per the version-bump checklist in
`CONTRIBUTING.md`.

To build the same packaged app for debugging against a local `velvt-core-api`
on its default port, use:

```sh
make build-app-local-core
```

This bakes `VELVT_API_BASE_URL=http://localhost:8000` (override with
`VELVT_LOCAL_API_BASE_URL`) into the bundled Rust service and the app's
processed `Info.plist`, as a Debug build. The target then signs with
`VELVT_CODESIGN_IDENTITY`, which has no default: if that identity is unset or
not in your keychain the target **fails** rather than silently falling back to
ad-hoc signing, because a silently ad-hoc bundle once ran on the build machine
and crashed everywhere else. To accept a build-machine-only bundle, opt in:

```sh
make build-app-local-core VELVT_ALLOW_ADHOC=1
# or, equivalently, VELVT_CODESIGN_IDENTITY=-
```

Copy `dist/Velvt.app` to `/Applications` when you want it to appear in
Launchpad.

### Build Both Local Targets (development)

From the repository root, build the Rust service and macOS-only Swift app
without changing either workspace's build configuration:

```sh
make build-all
```

The native macOS application target and scheme are named `velvt-mac` in
`swift-client/VelvtMac.xcodeproj`; the product it builds is `Velvt.app`
(`PRODUCT_NAME = Velvt` in `swift-client/Configs/*.xcconfig`). Its
"Bundle Rust Service" build phase compiles the Rust helper and embeds it at
`Contents/Resources/velvt-service`, so `make build-swift` leaves
`swift-client/.build/Velvt.app` with the helper inside. `make build-app` is the
packaged, signed and verified path. SwiftPM remains the unit-test harness.

To run the SwiftPM development executable against a service you started
yourself, source the canonical socket path and protocol version from `proto/`.
The SwiftPM executable product is `Velvt` (`swift-client/Package.swift`):

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client Velvt
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

Every IPC message (since protocol 3; the current version is in
`proto/version`) uses a tagged `{"type": "...", "payload": {...}}` envelope.
Rust DTOs live in `rust-service/shared-types`; Swift DTOs live in
`swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`. The message catalog is
[`docs/architecture/ipc-contract.md`](docs/architecture/ipc-contract.md).

## Event Relay

The `EventRelay` actor sits between the collection agent and the IPC layer. It
implements `EventSink` and is the sole downstream of `AXCollectionAgent`.

### Ring buffer and drop policy

While the IPC socket is unavailable, events are held in an in-memory
`CircularBuffer<RawEvent>` (default capacity: 500). When the buffer is full, the
**oldest** event is dropped and counted. Nothing is ever written to disk,
SQLite, or `UserDefaults`; events that overflow the buffer are permanently lost.

### Reconnect flush

On reconnect the relay logs a single count-only line (no raw event content):

```
Relay flushing N buffered events; dropped M events since disconnect.
```

It then drains the ring buffer in FIFO order before forwarding new events,
preserving chronological order. If the connection drops again mid-flush, the
remaining buffered events stay at the head of the ring buffer and resume on the
next reconnect.

### Back-pressure safety

`EventSink.receive(_:)` is `nonisolated` and returns in O(1) time — it holds
an `NSLock` only for a single `AsyncStream.Continuation.yield` call. It never
blocks the AX callback thread regardless of IPC speed or buffer state.

See [`docs/architecture/event-relay.md`](docs/architecture/event-relay.md) for
the full threading model, stream lifecycle, and privacy invariants.

## Architecture

Start with [`ARCHITECTURE.md`](ARCHITECTURE.md) for the system diagram, IPC
contract, auth state machine, and module responsibility table, and
[`docs/architecture/`](docs/architecture/) for subsystem deep dives.
Contributors must also read [`AGENTS.md`](AGENTS.md) and
[`CONTRIBUTING.md`](CONTRIBUTING.md) before making changes. See
[`PRIVACY.md`](PRIVACY.md) for what is collected, stored, and transmitted,
and [`DEFERRED.md`](DEFERRED.md) for seams intentionally left for post-MVP
work.

## Verifying the ONNX model is loaded

Tier 2 classification is optional. At startup, check the structured log:

- No `tier2_*` warning at all, and Tier 2 events classify with a real
  category instead of falling through to `unclassified` → the model is
  loaded.
- `tier2_model_unavailable` / `tier2_centroids_unavailable` /
  `tier2_centroids_invalid` → Tier 2 is disabled; Tier 1/3 continue.
- If you explicitly configured `VELVT_ABSTRACTION_MODEL_PATH` and it failed
  to load, the service additionally pushes a `service_status` IPC message
  with `state: "degraded"` so the menu bar UI can surface this rather than
  leaving you silently on the Tier 1/3 path indefinitely.

## Running the Test Suite

```sh
make test-all          # test-rust, test-swift and test-measurement
make test-rust         # cargo test (includes the 7-path
                        # end-to-end integration suite in
                        # rust-service/tests/e2e_integration.rs)
make test-swift         # swift test --package-path swift-client
make bench-rust         # the #[ignore]d wall-clock latency budgets,
                        # kept out of test-rust so a loaded machine
                        # cannot fail a correctness run
make test-measurement   # scripts/tests/run_measurement_tests.sh: the
                        # measurement and evidence scripts and the
                        # pbxproj membership guard (python3 + sqlite3)
```

## Smoke-Testing Against a Local velvt-core Instance

1. Run `make build-app-local-core` (with `VELVT_ALLOW_ADHOC=1` if you have no
   signing identity; see above), then open `dist/Velvt.app`.
   This points the bundled Rust service at `http://localhost:8000`.
2. Start your local `velvt-core-api` if it is not already running.
3. Grant Accessibility and Notifications when prompted, and sign in. Events
   collected before sign-in stay local with `upload_eligible = false` and are
   never uploaded retroactively (protocol 23).
4. Switch applications a few times, then check
   `~/.velvt/velvt-service.sqlite3`'s `upload_batch` table for a `sent` row,
   and your local `velvt-core` logs for the corresponding
   `POST /v1/events/batches` call.

The macOS collection-agent lifecycle, private AX run-loop threading model, and
no-polling rules are documented in
[`docs/architecture/collection-agent.md`](docs/architecture/collection-agent.md).
The two-permission allowlist, onboarding rationale, monitoring behavior, and
recovery steps are documented in
[`docs/architecture/permissions.md`](docs/architecture/permissions.md).
The event relay ring-buffer, drop policy, flush sequence, and privacy invariants
are documented in
[`docs/architecture/event-relay.md`](docs/architecture/event-relay.md).
The menu bar status item, `MenuBarState` derivation, notification scheduling
flow, `do_not_disturb_until` enforcement, and popover keyboard navigation are
documented in
[`docs/architecture/s7-menu-bar-and-notifications.md`](docs/architecture/s7-menu-bar-and-notifications.md).

## On-Device Classification

The Rust abstraction engine (`rust-service/src/abstraction/engine.rs`)
classifies every event on the device. It was a three-tier pipeline (seed
match, embedding, fallback) until Classification v2 (protocol 30, 1.0.11); it
is now an ordered ladder, most specific evidence first:

1. **Your corrections.** A rule for this exact window (`personal_override`),
   then for this application by bundle identifier, then for this application
   by name (both `personal_app_override`). A correction always outranks a
   classifier.
2. **Classifier plugins**, in registry order; the first one that answers wins:
   1. `BrowserContextPlugin` — for a browser, the focused site (a hostname
      Rust derives from the tab URL; the URL itself is discarded) and the
      title.
   2. `BundleSeedPlugin` — the taxonomy's bundle-identifier seeds.
   3. `SeedDictionaryPlugin` — the taxonomy's application-name seeds. A
      pattern matches the whole normalized name, or a `*` glob; there is no
      fuzzy or substring matching, which is why macOS's `Code` for VS Code
      needed the bundle seed above.
   4. `LocalPurposeHeuristicPlugin` — curated keyword families over the name
      and title.
   5. `DocumentTypePlugin` — the document types the application declares in
      its own `Info.plist`.
   6. `DeclaredCategoryPlugin` — the application's declared
      `LSApplicationCategoryType`, through a whitelist of unambiguous values.
   7. `EmbeddingSimilarityPlugin` (Tier 2) — a local embedding of name and
      title compared with versioned category prototypes, plus the bounded
      device-local prototypes your corrections create. Multiple prototypes
      may represent distinct modes of one category.
   8. `GenericBrowserPriorPlugin` — a browser whose site said nothing becomes
      an explicitly ambiguous `REFERENCE`.
   9. `UnloggedFallbackPlugin` — anything left is captured as `UNLOGGED`
      rather than dropped.

`docs/classification-v2-contract.md` is the design this ladder implements, and
`ARCHITECTURE.md` carries the same order. The built-in registry is in
`AbstractionEngineBuilder::register_builtin_plugins_with_embedding`, and
registration order is the arbitration order:

```rust
// This is the only line to change when registering a new classification plugin.
let builder = builder.register_plugin(NewClassificationPlugin::new(...));
```

`AbstractedEvent` serializes only a stable local ID, label, category, taxonomy
version, timestamp, and classification status, confidence and source; the
classification tier and the device-local display fields are skipped. Raw app
names and window titles never enter this type or its serialized output.

### Model Artifacts

Tier 2 targets an Apache-2.0 licensed, INT8-quantized ONNX export of
`sentence-transformers/all-MiniLM-L6-v2`. The model must be at most 50 MB and
must be accompanied by its `tokenizer.json` and a version-matched centroid
file. Model training, fine-tuning, and artifact generation happen offline and
are intentionally not part of the service.

The service always includes the portable, deterministic `builtin-hash-v1`
token/subword embedder and reviewed multi-prototype phrases, so Tier 2 remains
available without downloads or architecture-specific native libraries. The
bundled macOS service additionally prefers ONNX when an approved artifact set
named `abstraction-model.onnx`, `tokenizer.json`, and
`abstraction-prototypes.bin` is copied from `rust-service/resources/` into the
app bundle when present and discovered beside the configured taxonomy at
runtime. Environment variables may select a reviewed development artifact set.
Absent or invalid ONNX artifacts fall back to the built-in classifier. The
service never downloads a model or sends raw activity to a remote embedding API.

Explicit corrections also create high-threshold device-local semantic
prototypes. They are limited to 12 per category and 64 total, decay over 90
days, and require both a 0.90 similarity radius and a 0.08 winning margin.
Remove and reset controls delete these prototypes with their exact rules.
Only embeddings and context hashes are persisted; raw classifier input is
never stored in the semantic-learning tables. The hashes are HMAC-SHA-256 over
low-entropy inputs under a per-install salt kept in the same database file
(migration 0037), so they resist a reader who does not hold the file, and a
table precomputed from this source, and do not resist one who holds the file —
`PRIVACY.md` states the bound precisely.
They never leave the device.

Install an approved model artifact bundle by placing its files together and
configuring:

```sh
export VELVT_ABSTRACTION_MODEL_PATH=/path/to/model.onnx
export VELVT_ABSTRACTION_CENTROIDS_PATH=/path/to/abstraction-prototypes.bin
export VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS=20
export VELVT_ABSTRACTION_SIMILARITY_THRESHOLD=0.72
```

Do not configure arbitrary downloaded models. The tokenizer, model output
shape, centroid dimension, taxonomy version, and model license must be reviewed
together. If the ONNX model or prototypes are unavailable or invalid, that
adapter is disabled with a structured warning; the built-in Tier 2 classifier
remains active alongside Tier 1 and Tier 3.

### Classifier Prototype File

Classifier prototypes are static companion data and are never recomputed at
runtime. `VELVTC01` remains supported for one-centroid-per-category artifacts.
New artifacts use this format:

```text
"VELVTP02"
taxonomy_version_length: u32 little-endian
taxonomy_version: UTF-8 bytes
artifact_version_length: u32 little-endian
artifact_version: UTF-8 bytes
embedding_dimensions: u32 little-endian
prototype_count: u32 little-endian
repeated prototype_count times (category identifiers may repeat):
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
`VELVT_ABSTRACTION_TAXONOMY_PATH`. The file name is historical: the version
inside it is `mvp-2` (Classification v2 added bundle seeds and deleted the
unreachable browser seeds), and `API_EXPECTED_TAXONOMY_VERSION` is `mvp-2`. A
configured mismatch emits a structured warning while the configured version
remains attached to results.

`TitleAbstractor` is wired into `AbstractionEngine::process`, with
`DefaultTitleAbstractor` passing titles through locally. V1 will replace
semantically sensitive title tokens with category-scoped abstract labels
without transmitting raw titles.

Performance gates, each with the thing that can fail on it, because a published
number nothing runs is not a gate:

- Tier 1 mean and p95 below 1 ms:
  `rust-service/tests/abstraction_engine.rs::tier1_is_deterministic_and_completes_under_one_millisecond`,
  in `make test-rust`, on every pull request.
- Tier 2 p50 below 10 ms, twice — once against the test suite's zero-delay
  stand-in and once against `HashedEmbeddingModel`, which is what `main.rs`
  actually loads:
  `rust-service/tests/embedding_similarity.rs::tier2_median_is_under_ten_milliseconds_with_available_model`
  and `::builtin_tier2_median_is_under_ten_milliseconds`, both in
  `make test-rust`, on every pull request.
- Tier 2 p95 below 25 ms:
  `rust-service/tests/embedding_similarity.rs::tier2_p95_is_under_twenty_five_milliseconds_with_available_model`,
  `#[ignore]`d out of the correctness suite and run by `make bench-rust`, which
  CI runs in the `bench` job on pushes to `develop` and `main` and on demand.

The tail bound is off the pull-request path on purpose: a 25 ms wall-clock
budget on a shared runner fails under a busy neighbour about as readily as
under a regression. What that costs is that a Tier 2 tail regression is caught
on `develop` within one merge rather than before it lands. The median bounds are
what the pull request carries, and runner load does not move a median of 500
samples. All of them report p50, p95, and p99.

`make bench-rust` builds `--release`, because a budget for a shipped binary has
to be measured on the build that ships; the median bounds run unoptimized with
the rest of `make test-rust`, which is why they are set where a debug build
clears them by a wide margin. `make bench-rust` also runs the real-model p95,
but only where the `onnx` feature builds and `VELVT_ABSTRACTION_MODEL_PATH` and
`VELVT_ABSTRACTION_CENTROIDS_PATH` point at artifacts; the artifacts are not in
the repository and no CI job downloads them, so no CI run has ever measured the
real model. See [`PERFORMANCE_REPORT.md`](PERFORMANCE_REPORT.md).

## Rust SQLite Persistence

The Rust service owns SQLite persistence. `VELVT_DATABASE_PATH` selects the
database file; `:memory:` uses the same DAL and migration paths for tests. The
production default is `~/.velvt/velvt-service.sqlite3`, and startup creates
missing parent directories and applies every pending embedded migration before
constructing the abstraction engine.

The numbered files in `rust-service/migrations/` are the schema, and they are
the only enumeration of it that cannot go stale — this page carried a list of
six tables for as long as there were more than six. `PRIVACY.md` lists the
tables that hold anything drawn from your Mac, with what each one holds and for
how long, and `MIGRATED_TABLES` in `rust-service/tests/published_claims.rs` is
the closed inventory a test holds the migrated schema to. Time and date lookup
columns are indexed. The migration-owned `schema_migration` table records each
applied version, its file name and (since migration 0039) a checksum of its
SQL, so startup never applies the same migration twice, and refuses to open a
database that applied a different file under a version number this build uses
(a reused or renumbered migration). The checksum catches an edited migration,
which keeps its name: it covers every statement and leaves comments out, so
correcting a migration's header is not an edit. A debug build refuses a
database whose recorded checksum differs from its own; a release build opens
it, logs `migration_checksum_mismatch`, and reports itself degraded to the app,
because refusing would stop local collection on a tester's Mac over a defect
in the build.

The privacy invariant is narrower than this page used to state it, and the
narrow version is the one that is true. Nothing writes a window title, a URL, a
file path, a filename, or a contact into any column. `raw_event_buffer` holds
six device-local columns that can name or identify an application, all
disclosed in `PRIVACY.md`: `local_name_suggestion` (the raw application name,
kept only when neither a seed rule nor one of your corrections matched),
`local_display_label`, `app_stable_id` (a hash of the application name), and,
since migration 0033, `app_bundle_stable_id` (a hash of the bundle
identifier), `declared_app_category` and `document_type_ids` (what the
application declares about itself in its own `Info.plist`). None of them is
reachable from `upload/`. Four more columns hold text you typed yourself:
`abstraction_map.display_name`,
`personal_override.activity_name`, `personal_app_override.activity_name`, and
`work_block.intention`. `semantic_embedding_cache` holds a hashed sketch built
from the application name and the window title — not the title, and not
recoverable as one, but individual words are partially recoverable from it,
which `PRIVACY.md` describes rather than leaves to be found.

A new column that would hold raw content gets the same treatment migration 0001
already prescribes for the one that does: named in the migration header, named
in `PRIVACY.md`, and shown unable to reach `upload/`.

### Adding A Migration

1. Add one sequentially numbered SQL file to `rust-service/migrations/`, such
   as `0003_add_feature_table.sql`.
2. Make the migration additive and include required constraints and indexes.
3. Do not edit the migration runner. `rust-service/build.rs` embeds all sorted
   migration files automatically.
4. Add the file's line to `rust-service/migrations/CHECKSUMS`. The failing test
   `every_migration_matches_its_line_in_checksums` prints it. Once merged, a
   migration's statements never change: correct its comments if they are
   wrong, and put any other change in a new migration.
5. Run `cargo test`, `cargo clippy --workspace --all-targets -- -D warnings`,
   and `cargo fmt --all --check` from `rust-service/`.

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
