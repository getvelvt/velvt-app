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
xcodebuild                              # build
xcodebuild test -scheme velvt-mac       # run tests (verify scheme name)
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
- **No full-table scans on hot paths.** Required indexes: `raw_events.retention_expiry` (and any fields used in batch assembly).
- **Graceful shutdown.** Flush the pending upload queue and close the socket cleanly on `SIGTERM`.

### Build & test
```bash
cargo build --release       # release build
cargo test                  # all tests
cargo clippy -- -D warnings # lint (must pass clean)
cargo fmt --check           # format check
```

### Database migrations
Migrations are versioned files in `rust-service/migrations/`. They must be **safe and additive** — no destructive schema changes without an explicit migration path. Required tables: `raw_events`, `abstraction_mappings`, `abstracted_events`, `upload_batches`, `device_state`, `cached_daily_summaries`, `cached_daily_insights`.

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
4. Package the resulting vector into `centroids.bin` using the `VELVTC01`
   format documented in `README.md`.
5. Keep the centroid category key, dimensions, and taxonomy version aligned
   with the taxonomy file. Increment the taxonomy version for category-set
   changes.
6. Add exhaustive seed, centroid-loader, below-threshold, and extension tests.
7. Run `cargo test --all-features`, `cargo clippy --all-targets --all-features
   -- -D warnings`, and `cargo fmt --check`.

Centroid computation is offline artifact production, not service behavior.
Never add runtime centroid recomputation, training, fine-tuning, or online
learning. New classification strategies implement `ClassificationPlugin` and
are added only at the registry call site documented in `README.md`; the engine
core and existing plugins must remain unchanged.

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
