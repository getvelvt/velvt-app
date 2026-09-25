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
- The Swift client sends raw events and commands and renders Rust-authored snapshots (work-block state, dashboard, digest, invitations). It never reads abstraction maps, intermediate processing results, or the SQLite file — those are internal to the Rust service. Swift reports facts; Rust owns every judgement (category, drift, timing, copy).
- `docs/architecture/ipc-contract.md` is the message catalog; `proto/schema/` and `proto/CHANGELOG.md` are authoritative where they differ.
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
- **Dependencies:** Sparkle is the only third-party package (`Package.swift`, exact 2.9.4; the updater is off in alpha builds). There is no Swift-side SQLite and no GRDB — all persistence is in the Rust service.
- **IPC:** Unix domain socket client (no URLSession for local IPC)
- **Notifications:** UserNotifications, local only. Two kinds are posted: the in-block drift offer and the daily insight. There is no APNs registration (`registerForRemoteNotifications` is never called); the APNs environment setting and token-store protocol are unused seams.
- **Permissions:** Accessibility and Notifications only — no screen recording, microphone, camera, or filesystem access

## Project Structure
```
swift-client/
├── Package.swift          # SwiftPM: executable product `Velvt`, test harness
├── VelvtMac.xcodeproj     # Xcode target/scheme `velvt-mac`, product `Velvt.app`
├── Configs/               # Debug/Release xcconfig, entitlements
└── Sources/VelvtMac/
    ├── App/          # AppDelegate, menu bar controller, bundled-helper launcher, orphaned-helper recovery, updater
    ├── Collection/   # AXObserver agent, declared app metadata, Focus/DND observer
    ├── Relay/        # EventRelay: in-memory ring buffer between collection and IPC
    ├── IPC/          # Unix socket client, message types, version handshake, reconnect backoff
    ├── Delivery/     # Display-data coordinator, insight and drift-offer notification delivery
    ├── UI/           # Menu bar popover, work-block view, onboarding, history, digest, corrections
    ├── Auth/         # Account state, Keychain session storage
    ├── Device/       # Local device identity
    ├── Permissions/  # Accessibility and Notifications permission state
    ├── Service/      # Bundled Rust service lifecycle state
    ├── Config/       # Typed build config (socket path, protocol and client version)
    └── Resources/    # Bundled fonts
```

Every new `.swift` file must also join the Xcode target: run `./scripts/verify_pbxproj_membership.sh` (CI runs it on every pull request).

## Key Commands
- Build: `xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' build`
- Tests: `swift test --package-path swift-client`
- Lint: `make lint-swift` (`scripts/lint_swift.sh`: swift-format under `swift-client/.swift-format`, failing on any finding not in `swift-client/.swift-format-baseline`)

## Development Guide

### Collection
- Tear down the prior `AXObserver` and remove its run-loop source on every app activation.
- Handle missing, denied, or revoked Accessibility permission gracefully — collection stops, status updates, UI offers recovery.
- Handle apps that do not expose AX titles without crashing or logging raw content.

### IPC
- Open the Unix socket connection at app launch. Reconnect with exponential backoff if the service is unavailable.
- Buffer raw events in memory only (never disk, SQLite, or `UserDefaults`) while the socket is unavailable: `EventRelay` holds a bounded ring buffer (default 500 events) and drops the oldest on overflow — never accumulate unbounded memory.
- Log socket errors with error code only. Never log message content.

### Delivery
- Insight, history, and work-block payloads are received from the Rust service and held in memory for display; Swift keeps no copy of them on disk (it records only which notification IDs it has scheduled). The client requests 14 days of history, and the local Daily Activity chart covers 14 days, the same horizon as raw-event retention.
- Schedule UserNotifications from received payloads — do not generate notification text in the Swift layer. Drift-offer copy comes from the work-block snapshot's `active_intervention`; daily-insight copy from `notification_payload`.

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
Unix socket IPC server, raw event ingestion, abstraction engine, SQLite persistence, upload batching, cloud sync, and the device-local decisions the shipped product makes: work blocks and the deterministic drift gate, Focus/DND evidence, initiation invitations, the weekly receipts digest, and the local dashboard aggregates.

**The Rust service does NOT:**
- Render any UI
- Request macOS permissions
- Make decisions about notification scheduling (it delivers payloads; Swift schedules)

## Architecture Constraints
- **The service is the privacy enforcement boundary.** Abstraction happens here before any data is written to the upload queue. Raw fields must never appear in `upload_batch` / `batch_event` rows or in any outbound payload; `BatchEventPayload`'s hand-written `Serialize` (`src/upload/dto.rs`) is the only thing that crosses to the cloud.
- **Every policy is deterministic.** The drift gate, initiation, demotion and digest policies are fixed, versioned rules (`DRIFT_POLICY_VERSION` and its siblings); no policy learns or adapts its own thresholds. A drift-gate constant change needs a `DRIFT_POLICY_VERSION` bump, because decisions logged under two policy versions are never pooled.
- **No new behavioral-engine work without a dated decision.** The models in `src/behavior/` (BOCPD, HMM, antecedent miner) are shadow code with no caller in the shipped path; do not wire them into a shipped path, add a new one, or add experiment/randomization machinery unless the task cites a dated founder decision (see [Scope Boundary](#scope-boundary)). Local LLM inference and opaque models over raw activity are out of scope.
- **No full-table scans on hot paths.** Use indexed queries only for event ingestion and batching.
- **Auto-update aware.** The service must support being replaced on disk and restarted by the Swift client's update mechanism. It must not hold exclusive file locks that prevent replacement.

## Technology Stack
- **Language:** Rust (stable toolchain, version pinned in `rust-toolchain.toml`)
- **IPC:** Unix domain socket server (`tokio` async runtime)
- **Persistence:** SQLite via `rusqlite` (bundled) with numbered, embedded migrations
- **HTTP client:** `reqwest` for cloud upload
- **Serialization:** `serde` + `serde_json`; proto schema in `proto/` is the source of truth

## Project Structure
```
rust-service/
├── src/
│   ├── main.rs         # Entry point, service wiring and lifecycle; declares `behavior`
│   ├── lib.rs          # Library crate: every module below except `behavior`
│   ├── ipc/            # Unix socket server, message dispatch, version negotiation
│   ├── abstraction/    # Classification ladder, taxonomy, stable keys, embedding (Tier 2), optional ONNX
│   ├── persistence/    # SQLite DAL: models, traits, the one rusqlite implementation
│   ├── upload/         # Batch assembly, retry, transport; the only cloud-bound DTO
│   ├── auth/           # Account and device auth, token refresh/reissue, credential store
│   ├── delivery/       # Insight/history fetch, cache, polling, push to Swift
│   ├── work_block/     # Work-block state machine, drift gate and offer, outcomes, decision log
│   ├── focus/          # Focus/DND evidence and the quiet-hours offer
│   ├── initiation/     # Good-hours windows and the capped daily soft-start invitation
│   ├── receipts/       # Weekly receipts digest and the explain-tap bucket
│   ├── dashboard.rs    # Focus Fragmentation and Daily Activity aggregates
│   ├── retention/      # RetentionScheduler and its RetentionTarget implementations
│   ├── lifecycle/      # CancellationToken and graceful shutdown
│   ├── config/         # Typed, validated runtime configuration
│   └── behavior/       # Shadow models (BOCPD, HMM, antecedent miner): no caller in the shipped path
├── migrations/         # 0001_…sql onward, embedded by build.rs
├── shared-types/       # IPC DTOs (ClientMessage / ServerMessage)
├── resources/          # Taxonomy JSON (optional model artifacts are not in the repository)
├── tests/              # Integration tests, including published_claims.rs
└── Cargo.toml
```
There is no `src/analytics/` module.

## Key Commands
- Build: `cargo build --release`
- Tests: `cargo test`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` (`make lint-rust`, which CI runs)
- Format: `cargo fmt --all --check`
- onnx feature: `make check-rust-onnx` (type-checks `src/abstraction/onnx.rs`; CI runs it)

## Development Guide

### Abstraction Engine
- Stable keys are domain-separated SHA-256 digests — one over (app name, window context), one over the app name, and one over the bundle identifier — each keyed to the install as HMAC-SHA-256 under `stable_key_salt` (migration 0037). Compute them only through `abstraction/key.rs` with the salt of the store they will be looked up in (`AbstractionMappingStore::stable_key_salt`). The event's stable ID is a random `abs_…` identifier, not a hash.
- Classification order is: window correction → bundle correction → app-name correction → classifier plugins in registry order (`register_builtin_plugins_with_embedding`). `ARCHITECTURE.md` lists the ladder.
- Categories are the taxonomy's (`mvp-2`): `FOCUS_WORK`, `PASSIVE_CONSUMPTION`, `SOCIAL_FEED`, `COMMUNICATION`, `TASK_MANAGEMENT`, `REFERENCE`, `SYSTEM`, and the default `UNLOGGED`. Local labels are `<type>:<behavior>` (`document:code`, `video:youtube`, …).
- Local labels never upload. The upload serializer collapses each event to one category-scoped `abstraction_type` (`cloud_abstraction_type` in `src/upload/dto.rs`, e.g. `document:inferred`); a new uploaded type is a cloud-contract change as well as a local one.
- Abstraction mappings are persisted in SQLite and never leave the device.

### Persistence
- Migrations must be safe and additive. Use versioned migration files.
- The migrations are the schema (0001–0037 on `develop` as of 2026-09-25). Do not keep a table list here: `MIGRATED_TABLES` in `tests/published_claims.rs` is the closed inventory the migrated schema is tested against, and `PRIVACY.md`'s storage table describes every store that holds anything drawn from the Mac. A new table goes in both, in the same commit.
- Migration numbers are sequential and shared across branches: take the next free number when you merge, and never renumber or edit a migration that has shipped.
- `raw_event_buffer.occurred_at` and `raw_event_buffer.created_at` must have explicit indexes. Retention cleanup must use an indexed path.
- Default retention (`src/config/mod.rs`; `PRIVACY.md` is the per-table reference and `published_claims` pins its numbers): raw events 14 days (`VELVT_RAW_EVENT_TTL_HOURS`, tied to the 14-day activity chart); window mappings (`abstraction_map`) 14 days from the window's last observation unless a correction or a buffered event points at them; sent upload batches 30 days; rejected batches 7 days; history cache 10 minutes and insight cache 30 minutes (`VELVT_HISTORY_TTL_SECONDS` / `VELVT_INSIGHT_TTL_SECONDS`).

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
velvt-app/
├── swift-client/        # SwiftUI/AppKit macOS app
├── rust-service/        # Core processing service
├── proto/               # IPC message schema (JSON Schema), socket path, protocol version
├── scripts/             # Build, sign, verify, release, and measurement scripts (+ scripts/tests/)
├── cloud/               # Empty placeholder; the FastAPI backend is the separate, private velvt-core repository
└── docs/
    └── architecture/
```

Cross-workspace changes (anything touching `proto/`) require updating both workspaces atomically in the same commit. Never merge a `proto/` change that leaves one workspace on a stale schema version.

***

# Scope Boundary

**What governs scope.** This repository does not grant scope on its own. The founder's plans in the private Velvt workspace do: `GOAL.md`, `RUNBOOK.md`, `plan/README.md`, and `pivot-engineering/10-BUNDLE-ABSORPTION.md`. The last one rejected, as item GOV-1, the rewrite of this file that pre-authorized local behavioral analytics and experiment modeling; that text cited an implementation master plan that is not one of the governing documents, and it was removed on 2026-09-25. Engine work — new behavioral models, experiments or randomization, or wiring `src/behavior/` into a shipped path — waits on Gate D (`10-BUNDLE-ABSORPTION.md` § 5), which has not been met, or on a dated founder decision that overrides it. If a task asks for such work and cites neither, stop and ask.

**In scope (what ships in 1.0.11):**
- `swift-client/`: passive event capture, IPC relay, menu bar UI, onboarding, permissions, work-block controls and the in-app drift card, drift-offer and daily-insight notifications, 14-day history and daily activity, corrections and unclassified-app triage, the weekly digest card, the soft-start invitation card, local data controls
- `rust-service/`: IPC server, abstraction engine, SQLite persistence, batched upload, auth, device registration, cloud sync, insight payload delivery, work blocks and the deterministic drift gate (`work_block`), Focus/DND evidence (`focus`), initiation invitations (`initiation`), the weekly receipts digest (`receipts`), and the two 0.1.5 display surfaces (`dashboard`)

**Explicitly deferred — do not build:**
- Local LLM inference, opaque general sequence models, autonomous or adaptive intervention policies, and new behavioral analytics or experiment machinery (see above).
- Charts or streak counters beyond the two restrained 0.1.5 surfaces (Focus Fragmentation and Daily Activity) already served by `rust-service/src/dashboard.rs`
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

**Updating Documentation:** When you make changes to architecture, authentication, settings, APIs, or any system-level behavior, check `docs/DOC_INDEX.md` to locate the relevant documentation file(s) and update them as part of the same task. Do not leave documentation out of sync with the code.
