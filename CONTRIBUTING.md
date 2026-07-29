# Contributing to Velvt

Velvt is a monorepo containing two primary workspaces. Read this guide before opening a PR.

***

## Repository Structure

```
velvt/
├── swift-client/        # SwiftUI/AppKit macOS app (L1 capture + L4 delivery)
├── rust-service/        # Core processing service (abstraction, persistence, upload)
├── proto/               # IPC message schema, socket path, protocol version
├── cloud/               # Python FastAPI backend
└── docs/
    └── architecture/
```

Most contributions will touch exactly one of `swift-client/` or `rust-service/`. Changes to `proto/` are cross-workspace and require coordinated updates to both (see [IPC Contract Changes](#ipc-contract-changes) below).

***

## Architecture in One Paragraph

The Swift client captures raw macOS events (app focus, window title changes) via Accessibility APIs and forwards them over a **Unix domain socket** to the Rust service. The Rust service owns everything after that: abstraction, SQLite persistence, upload batching, and cloud sync. The Swift client never touches abstracted data or makes cloud calls — it only receives ready-to-display insight payloads back from the Rust service. This split keeps the UI layer stable and the processing layer independently updatable.

***

## Privacy Boundary — Non-Negotiable

This is the most critical invariant in the codebase. A violation here is a P0 bug, not a style issue.

| Data type | Stays local | Cloud allowed |
|---|---|---|
| Raw app names, bundle IDs | ✅ | ❌ |
| Raw window titles, URLs, paths | ✅ | ❌ |
| Abstracted labels (`document:edit`, `tab:A`) | ✅ | ✅ |
| Coarse categories, timestamps, durations | ✅ | ✅ |
| Session summaries, derived metadata | ✅ | ✅ |

**The Rust service is the enforcement boundary.** Raw fields must never appear in `abstracted_events`, `upload_batches`, or any outbound HTTP payload. The cloud rejects violations with `raw_field_rejected`. Tests in `rust-service/` must prove this invariant programmatically.

Auth tokens: **Keychain only** (Swift) / **platform credential store** (Rust). Never SQLite.

***

## IPC Contract (`proto/`)

The `proto/` directory is the source of truth for the message schema between `swift-client/` and `rust-service/`.

- `proto/schema/` — JSON Schema definitions for all IPC message types
- `proto/ipc_socket_path` — canonical socket path (never hardcode in either workspace)
- `proto/version` — current protocol version integer

**IPC Contract Changes**

If your change requires a new or modified message type:

1. Update `proto/schema/` first.
2. Bump `proto/version` if the change is not backward-compatible.
3. Update `rust-service/` to handle the new schema.
4. Update `swift-client/` to send/receive the new schema.
5. All four changes must land in **the same PR and commit**. Do not merge partial proto changes.

The Swift client declares its supported protocol version on every socket connection open. The Rust service must negotiate gracefully and never silently drop messages from mismatched versions.

***

## Swift Client (`swift-client/`)

**What it owns:** event capture, IPC relay to Rust, insight payload display, menu bar UI, onboarding, permissions.

**What it does NOT own:** abstraction logic, SQLite for abstracted data, cloud uploads, analytics.

### Stack
- Swift, SwiftUI + AppKit, `NSStatusItem`
- GRDB.swift — UI read cache only (insight history, cached summaries)
- Unix domain socket client for IPC
- UserNotifications + APNs
- Permissions: Accessibility and Notifications only

### Key constraints
- **No polling.** Event-driven only: `NSWorkspace.didActivateApplicationNotification`, `kAXFocusedWindowChangedNotification`, `kAXTitleChangedNotification`.
- **AXObserver is per-process.** Tear down and recreate the observer on every app activation.
- **No cloud calls from Swift.** All outbound traffic routes through the Rust service.
- **Notification text comes from the Rust service payload.** Do not generate insight copy in Swift.

### Build & test
```bash
swift test --package-path swift-client
xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' build
```

***

## Rust Service (`rust-service/`)

**What it owns:** IPC server, abstraction engine, SQLite (all tables), upload batching, cloud HTTP, auth token management, insight payload delivery, and (future, feature-flagged) local analytics.

**What it does NOT own:** any UI, macOS permission requests, notification scheduling.

### Stack
- Rust (stable, version pinned in `rust-toolchain.toml`)
- `tokio` — async runtime
- `sqlx` or `rusqlite` — SQLite with versioned migrations
- `reqwest` — cloud HTTP client
- `serde` / `serde_json` — serialization (proto schema is the contract)
- `tracing` — structured logging

### Key constraints
- **Abstraction happens before any data hits the upload queue.** No raw fields in `abstracted_events` or `upload_batches`.
- **Analytics is a deferred stub.** `src/analytics/` exists but is feature-flagged off in all MVP builds. Do not activate it.
- **No full-table scans on hot paths.** Retention and batch-assembly queries must use indexed paths, including `raw_event_buffer.occurred_at` / `created_at` and batch status/time columns.
- **Graceful shutdown.** Flush the pending upload queue and close the socket cleanly on `SIGTERM`.

### Build & test
```bash
cargo build --release       # release build
cargo test                  # all tests
cargo clippy -- -D warnings # lint (must pass clean)
cargo fmt --check           # format check
```

### Database migrations
Migrations are versioned files in `rust-service/migrations/`. They must be **safe and additive** — no destructive schema changes without an explicit migration path. Current feature tables are `abstraction_map`, `raw_event_buffer`, `upload_batch`, `batch_event`, `history_cache`, `insight_cache`, and `upload_host_backoff`.

***

## Logging Rules (Both Workspaces)

Logs must **never** include:
- Raw window titles, app names, bundle IDs
- URLs, file paths, filenames
- Contact names, email addresses
- Insight text or notification copy

Safe to log: timestamps, error codes, HTTP status codes, socket error types, abstracted labels (not the raw-to-abstract mapping).

Debug builds may enable verbose safe diagnostics. Release builds must not be noisy.

***

## PR Guidelines

- **Scope PRs to one workspace** whenever possible. Cross-workspace PRs are acceptable only for proto changes or tightly coupled fixes — explain the coupling in the PR description.
- **Tests are required** for any change to abstraction logic, IPC message handling, upload batching, or privacy boundary enforcement. New features without tests will not be merged.
- **No new third-party dependencies** without prior discussion in an issue. This applies to both `Package.swift` and `Cargo.toml`.
- **Pass lint before opening PR.** `cargo clippy -- -D warnings` for Rust; Swift lint config in CI.
- PR titles follow: `[swift-client]`, `[rust-service]`, `[proto]`, or `[cloud]` prefix.

## Adding A Classification Category

Category changes remain data-only in the runtime classification pipeline:

1. Add the category identifier to `categories` in the versioned taxonomy JSON.
2. Add or update exact/glob seed entries with privacy-safe `<type>:<behavior>`
   labels where deterministic Tier 1 mappings are appropriate.
3. In the offline model-artifact pipeline, run the approved centroid
   computation script against reviewed representative names. The script must
   mean-pool and normalize embeddings using the exact shipped model/tokenizer.
4. Package reviewed representative vectors into
   `abstraction-prototypes.bin` using the `VELVTP02` format documented in
   `README.md`. Assign a new classifier artifact version for every changed
   prototype set. A category may have multiple prototypes; the canonical
   category set remains controlled by the taxonomy.
5. Keep the centroid category key, dimensions, and taxonomy version aligned
   with the taxonomy file. Increment the taxonomy version for category-set
   changes.
6. Add exhaustive seed, centroid-loader, below-threshold, and extension tests.
7. Run `cargo test --all-features`, `cargo clippy --all-targets --all-features
   -- -D warnings`, and `cargo fmt --check`.

Prototype computation is offline artifact production, not service behavior.
Never add unattended runtime taxonomy mutation, centroid recomputation,
training, or fine-tuning. Explicit user corrections may update only the
bounded, resettable device-local semantic prototype store. New classification
strategies implement `ClassificationPlugin` and
are added only at the registry call site documented in `README.md`; the engine
core and existing plugins must remain unchanged.

***

## Adding A New Abstraction Type

MVP supports `document:edit` only; adding a new type (e.g. `tab:A`) is
cross-cutting:

1. Add the type identifier to the IPC contract: `proto/schema/raw_event.json`
   if it changes what Swift sends, and confirm `BatchPayload.supported_abstraction_types`
   in `rust-service/src/upload/dto.rs`/`assembly.rs` lists it.
2. Implement or extend the `ClassificationPlugin` that produces it (see
   "Adding A Classification Category" below for the category side).
3. Add a privacy boundary test proving the new label is reachable from a
   raw event without ever re-exposing the raw event's content.
4. Update `ARCHITECTURE.md`'s classification pipeline section.

## Adding A New IPC Message Type

See "IPC Contract Changes" above for the five-step process. In addition:

- Add the message to both `ClientMessage`/`ServerMessage` enums in
  `rust-service/shared-types/src/lib.rs` **and** the corresponding Swift
  enum case in `IPCTypes.swift` in the same commit — `proto/version` v6→v7
  in this repository's history is a real example of what happens when this
  is skipped (a message type existed in Swift for an entire release with no
  Rust counterpart, no schema entry, and no version bump; see
  `proto/CHANGELOG.md` "Version 7").
- If the new message can carry a credential or token, give it a
  hand-written `Debug` impl that redacts the sensitive field (see
  `SignUp`/`LogIn`/`AuthSuccess` in `shared-types/src/lib.rs` for the
  pattern) — the type's derived `Serialize`/`Deserialize` still needs the
  real field for the wire format, so the type wrapper approach
  (`RedactedString`) used internally doesn't apply at the DTO layer.
- Add a round-trip test asserting the exact JSON shape (see
  `v6_auth_contract`/`v7` tests in `shared-types/src/lib.rs`), not just that
  serialization succeeds.

## Adding A New Retention Target

1. Implement `RetentionTarget` in `rust-service/src/retention/targets.rs`,
   following the existing `RawEventRetentionTarget`/`UploadBatchRetentionTarget`/`CacheRetentionTarget`
   pattern: a `run_cleanup` method that deletes at most `batch_size` rows
   older than a cutoff and returns the count deleted.
2. Register it via `RetentionScheduler::add_target` at the call site in
   `main.rs` — do not modify `RetentionScheduler` itself.
3. Add a test proving only expired rows are deleted and fresh rows survive
   (mirror `tests/retention.rs`).

## Privacy Review Checklist

Any PR touching the abstraction engine (`rust-service/src/abstraction/`),
the upload batcher (`rust-service/src/upload/`), or the IPC layer
(`rust-service/src/ipc/`, `proto/`, or `swift-client/Sources/VelvtMac/IPC/`)
must confirm, in the PR description:

- [ ] No new struct field carrying raw app names, window titles, URLs,
      paths, filenames, contacts, emails, or phone numbers crosses out of
      `abstraction/` into a serializable or loggable type.
- [ ] No new `tracing::`/`Logger(` call site interpolates a variable that
      could carry raw event content or a token/credential without
      redaction.
- [ ] If the change adds or modifies a field on `BatchEventPayload` or
      `BatchPayload`, it is checked against the forbidden-field list in
      `tests/upload_batching.rs::payload_serialization_contains_only_audited_safe_fields`.
- [ ] If the change adds a token- or credential-carrying type, it has
      either a `RedactedString` field (Rust-internal types) or a
      hand-written `Debug` impl that redacts it (wire DTOs — see
      `shared-types/src/lib.rs`).
- [ ] Re-run the relevant section of `PRIVACY_AUDIT.md` and update it if a
      finding changed.

***

## MVP Scope

**In scope:**
- `swift-client/`: passive event capture, IPC relay, menu bar UI, onboarding, notification display, 7-day insight history, local retention controls
- `rust-service/`: IPC server, abstraction engine, SQLite persistence, batched upload, auth, device registration, insight payload delivery

**Deferred — do not build in this repo:**
- Local analytics engine or local LLM inference (`rust-service/src/analytics/` stub only)
- Dashboard, graphs, streak counters
- Unabstracted cloud personalization
- Cross-platform Swift client
- Advanced automations

***

## Documentation

When contributing changes that affect architecture, APIs, authentication, or significant behavior, update the relevant files under `/docs/`. Consult `docs/DOC_INDEX.md` to find the right file quickly.
