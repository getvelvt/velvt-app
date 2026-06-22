# Velvt — Agent Guide

You are an experienced developer working on the **Velvt monorepo** — a privacy-first passive productivity intelligence system. Tasks will be scoped to one of two primary workspaces:

| Workspace | Language | Scope |
|---|---|---|
| `swift-client/` | Swift | Layers 1 & 4: macOS event capture, abstraction relay, UI, notifications |
| `rust-service/` | Rust | Layers 1B & 2: IPC server, abstraction engine, SQLite, upload batching, cloud sync |

Before starting any implementation, you MUST review this guide. Identify which workspace your task touches before writing a single line of code. Cross-workspace changes require explicit scope confirmation.

***

# Core Mandates

- **Conventions:** Rigorously adhere to existing project conventions. Analyze surrounding code, tests, and configuration first.
- **Libraries/Frameworks:** NEVER assume a library or framework is available. Verify established usage (`Package.swift` for Swift, `Cargo.toml` for Rust) before employing it. Do not introduce new third-party dependencies unless explicitly requested.
- **Style & Structure:** Mimic the style (formatting, naming), structure, framework choices, typing, and architectural patterns of existing code.
- **Idiomatic Changes:** Ensure changes feel native to their workspace — Swifty Swift, idiomatic Rust. Do not import patterns from one workspace into the other.
- **Comments:** Add code comments sparingly. Focus on *why*, not *what*. Only add high-value comments. Do not edit comments unrelated to your change. *NEVER* communicate with the user through code comments.
- **Proactiveness:** Fulfill the request thoroughly, including reasonable implied follow-up actions.
- **Confirm Ambiguity:** Do not take significant actions beyond the clear scope of the request without confirming. If asked *how* to do something, explain first.
- **Explaining Changes:** After completing a code modification or file operation, provide a brief summary.
- **Do Not Revert:** Do not revert changes unless they caused an error or the user explicitly asks.

***

# Tone and Style

- **Concise & Direct:** Professional, direct, and concise. Suitable for a chat environment.
- **Minimal Output:** Fewer than 3 lines of text per response (excluding tool use/code) whenever practical.
- **No Chitchat:** No preambles or postambles. Get straight to the action.
- **Formatting:** GitHub-flavored Markdown. Responses render in monospace.
- **Handling Inability:** State briefly (1–2 sentences) if unable to fulfill a request. Offer alternatives if appropriate.

***

# Privacy Boundary — Non-Negotiable

The most critical invariant of the entire project. Violations are fatal bugs.

- **Local only (never leaves the device):** raw app names, bundle IDs, window titles, URLs, filenames, paths, raw text, contacts.
- **Cloud allowed:** abstracted labels, coarse categories, timestamps, durations, session summaries, derived event metadata.
- **The Rust service owns the privacy enforcement boundary.** It is the last gate before any data leaves the device. The Swift client MUST NOT perform its own upload to the cloud — all outbound traffic flows through the Rust service.
- **Never upload forbidden raw fields.** The cloud will reject them with `raw_field_rejected`. Unit tests in `rust-service/` must prove forbidden fields cannot appear in upload payloads.
- Auth and refresh tokens go in **Keychain only** (Swift) or the **platform credential store** (Rust). Never SQLite.

***

# Architecture — IPC Boundary

The Swift client and Rust service communicate over a **Unix domain socket**. This boundary is the contract between the two workspaces.

```
Swift Client                         Rust Service
────────────────────────────────     ────────────────────────────────────────
NSWorkspace / AXObserver events  →   Raw event ingestion
                                 ←   Abstracted event confirmations
                                 ←   Ready-to-display insight payloads
```

**IPC rules:**
- The socket path is defined in `proto/ipc_socket_path` — never hardcode it in either workspace.
- Message schema is defined in `proto/` as JSON Schema. Both workspaces must conform to the version declared in `proto/version`.
- The Rust service sends `server_hello`, then the Swift client declares its supported protocol version in `client_hello` on every connection.
- The Rust service must negotiate gracefully — reject unsupported versions with a clear error code, never silently drop messages.
- The Swift client sends raw events and reads confirmations/payloads. It never reads abstraction maps, analytics state, or intermediate processing results — those are internal to the Rust service.
- Do not add new message types to the IPC protocol without updating `proto/` and confirming the change spans both workspaces.

***

# Swift Client (`swift-client/`)

## Scope
Passive event capture via macOS Accessibility APIs, IPC relay of raw events to the Rust service, receipt and display of insight payloads, menu bar UI, onboarding, and permissions.

**The Swift client does NOT:**
- Perform abstraction (that is Rust's job)
- Write to SQLite directly for abstracted events or upload state
- Make outbound network calls to the cloud
- Run any analytics or LLM inference

## Architecture Constraints
- **No polling.** Use `NSWorkspace.didActivateApplicationNotification`, `kAXFocusedWindowChangedNotification`, and `kAXTitleChangedNotification` only.
- **AXObserver is per-process.** Tear down the previous observer and its run-loop source on every app activation before registering a new one.
- **No local analytics engine.** No local LLM, no full-DB scans, no pandas/scipy-equivalent computation.
- **UI reads from lightweight local summary tables only** (cached payloads received from the Rust service).

## Technology Stack
- **Language:** Swift
- **UI:** SwiftUI + AppKit, `NSStatusItem` for menu bar
- **Local cache:** SQLite via GRDB.swift — for UI read cache and Keychain-adjacent state only
- **IPC:** Unix domain socket client (no URLSession for local IPC)
- **Notifications:** UserNotifications + APNs
- **Permissions:** Accessibility and Notifications only — no screen recording, microphone, camera, or filesystem access

## Project Structure
```
swift-client/
├── App/           # Entry point, lifecycle, AppDelegate, NSStatusItem
├── Collection/    # AXObserver agent, workspace notification handling
├── IPC/           # Unix socket client, message serialization, version handshake
├── Delivery/      # Insight payload receipt, notification scheduling, history cache
├── UI/            # Menu bar popover, onboarding, permission status, privacy disclosure
├── Auth/          # Auth client, token management, Keychain storage
├── Device/        # Device registration, APNs token management
└── Config/        # Typed build config (socket path, APNs environment, client version)
```

## Key Commands
- Build: `xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' build`
- Tests: `swift test --package-path swift-client`
- Lint: `cd swift-client && swift format lint --recursive Sources Tests`

## Development Guide

### Collection
- Tear down the prior `AXObserver` and remove its run-loop source on every app activation.
- Handle missing, denied, or revoked Accessibility permission gracefully — collection stops, status updates, UI offers recovery.
- Handle apps that do not expose AX titles without crashing or logging raw content.

### IPC
- Open the Unix socket connection at app launch. Reconnect with exponential backoff if the service is unavailable.
- Buffer raw events in memory (not SQLite) for up to 30 seconds if the socket is unavailable. Drop oldest events beyond that window — do not accumulate unbounded memory.
- Log socket errors with error code only. Never log message content.

### Delivery
- Insight payloads are received from the Rust service and cached locally for UI display.
- Schedule UserNotifications from received payloads — do not generate notification text in the Swift layer.
- Cache up to 7 days of insight history for UI display.

### Logging
- Logs must never include raw window titles, app names, bundle IDs, URLs, paths, filenames, contacts, emails, or insight text.
- Network/IPC errors: endpoint or socket path, status/error code only.
- Debug builds may enable verbose safe diagnostics. Release builds must not be noisy.

### Testing
- Abstraction relay tests: verify raw event structs sent over IPC match `proto/` schema exactly.
- IPC client tests: cover connection lifecycle, reconnect backoff, and version handshake.
- Collection lifecycle tests: cover AX observer teardown/re-registration on app activation.
- Privacy boundary tests: verify no forbidden raw fields appear in any IPC message payload.

***

# Rust Service (`rust-service/`)

## Scope
Unix socket IPC server, raw event ingestion, abstraction engine, SQLite persistence, upload batching, cloud sync, and (future) local analytics.

**The Rust service does NOT:**
- Render any UI
- Request macOS permissions
- Make decisions about notification scheduling (it delivers payloads; Swift schedules)

## Architecture Constraints
- **The service is the privacy enforcement boundary.** Abstraction happens here before any data is written to the upload queue. Raw fields must never appear in `abstracted_events` or `upload_batches` tables.
- **No analytics in MVP.** Analytics modules may exist as stubbed feature-flagged modules, but must not execute in the default build. Gate behind a compile-time or runtime feature flag.
- **No full-table scans on hot paths.** Use indexed queries only for event ingestion and batching.
- **Auto-update aware.** The service must support being replaced on disk and restarted by the Swift client's update mechanism. It must not hold exclusive file locks that prevent replacement.

## Technology Stack
- **Language:** Rust (stable toolchain, version pinned in `rust-toolchain.toml`)
- **IPC:** Unix domain socket server (`tokio` async runtime)
- **Persistence:** SQLite via `sqlx` or `rusqlite` with explicit migrations
- **HTTP client:** `reqwest` for cloud upload
- **Serialization:** `serde` + `serde_json`; proto schema in `proto/` is the source of truth

## Project Structure
```
rust-service/
├── src/
│   ├── main.rs         # Entry point, service lifecycle
│   ├── ipc/            # Unix socket server, message dispatch, version negotiation
│   ├── abstraction/    # Raw-to-abstract mapping engine, category assignment
│   ├── persistence/    # SQLite schema, migrations, DAL
│   ├── upload/         # Batch assembly, retry logic, cloud HTTP client
│   ├── auth/           # Token storage (platform credential store), refresh logic
│   ├── delivery/       # Insight fetch, payload formatting, push to Swift client
│   └── analytics/      # Feature-flagged stub — DO NOT activate in MVP
├── tests/              # Integration tests
└── Cargo.toml
```

## Key Commands
- Build: `cargo build --release`
- Tests: `cargo test`
- Lint: `cargo clippy -- -D warnings`
- Format: `cargo fmt --check`

## Development Guide

### Abstraction Engine
- Build a stable key from `app_name::window_title`. Hash to a stable local identifier.
- Assign abstract labels (`document:edit`, `tab:A`, etc.) and categories (lower-case: `document`, `communication`, `reference`, `passive_consumption`, `focus_work`, `system`, `unclassified`).
- MVP supports `document:edit` abstraction type. Do not add new types without a corresponding `proto/` contract update.
- Abstraction mappings are persisted in SQLite and never leave the device.

### Persistence
- Migrations must be safe and additive. Use versioned migration files.
- Current feature tables: `abstraction_map`, `raw_event_buffer`, `upload_batch`, `batch_event`, `history_cache`, `insight_cache`, and `upload_host_backoff`.
- `raw_event_buffer.occurred_at` and `raw_event_buffer.created_at` must have explicit indexes. Retention cleanup must use an indexed path.
- Default retention: raw cache 7 days, uploaded abstracted events compacted after 7 days, cached insights 7 days.

### Upload
- Batch every 60 seconds while active, after 50 pending abstracted events, or on service shutdown signal.
- Batch IDs are stable UUIDs for retry idempotency. `2xx` on a duplicate batch ID is success.
- `raw_field_rejected` → mark batch permanently failed, stop retrying, log rejected field name only (never payload content).
- `401` → trigger token refresh. `403 device_revoked` → pause upload, surface status to Swift client via IPC.

### Logging
- Same rules as Swift: no raw titles, app names, bundle IDs, URLs, paths, or insight text in logs.
- Use structured logging (`tracing` crate). Log level must be configurable at runtime.

### Testing
- Abstraction engine: stable mapping, category fallback, forbidden-field exclusion.
- Upload payload: exact API shape for `/v1/events/batches`, no forbidden fields.
- IPC server: version negotiation, malformed message rejection, reconnect handling.
- SQLite migrations: verify required indexes, additive safety.

***

# Monorepo Structure

```
velvt/
├── swift-client/        # SwiftUI/AppKit macOS app
├── rust-service/        # Core processing service
├── proto/               # IPC message schema (JSON Schema), socket path, protocol version
├── cloud/               # Python FastAPI backend (separate agent scope)
└── docs/
    └── architecture/
```

Cross-workspace changes (anything touching `proto/`) require updating both workspaces atomically in the same commit. Never merge a `proto/` change that leaves one workspace on a stale schema version.

***

# MVP Scope Boundary

**In scope:**
- `swift-client/`: passive event capture, IPC relay, menu bar UI, onboarding, permissions, notification display, 7-day history display, daily insight display, local retention controls
- `rust-service/`: IPC server, abstraction engine, SQLite persistence, batched upload, auth, device registration, cloud sync, insight payload delivery

**Explicitly deferred — do not build:**
- Local analytics engine or local LLM inference (`rust-service/src/analytics/` is a stub only)
- Dashboard, graphs, or streak counters
- Unabstracted cloud personalization
- Cross-platform Swift client (Windows/Linux)
- Advanced automations

***

# Primary Workflow

1. **Identify workspace scope** — `swift-client/`, `rust-service/`, `proto/`, or cross-workspace.
2. **Understand** the request and relevant codebase context.
3. **Plan** — share a concise plan if it clarifies your approach. Flag cross-workspace impact immediately.
4. **Implement** — follow all conventions above strictly.
5. **Verify (Tests)** — run applicable tests for the affected workspace(s).
6. **Verify (Standards)** — run lint/format checks. `cargo clippy` and `cargo fmt` for Rust; lint config for Swift.
