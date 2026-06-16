# Privacy Audit — velvt-mac MVP Integration

This audit covers the five required checks against the Rust service
(`rust-service/`) and Swift client (`swift-client/`) as merged for the MVP
integration pass, including the device registration, account-auth relay,
raw-event ingestion, and notification-push code added during this pass.

---

## Audit 1 — Raw content boundary

**Examined:** every occurrence of `app_name`, `window_title`, `appName`,
`windowTitle` in `rust-service/src` and `swift-client/Sources`.

| Location | Finding |
|---|---|
| `rust-service/src/abstraction/key.rs` (`RawKey`) | SAFE — `pub(crate)` struct, never leaves `abstraction/`. |
| `rust-service/src/abstraction/engine.rs` (`AbstractionEngine::process`) | SAFE — destructures `RawEvent` locally, only ever returns `AbstractedEvent` (no raw fields). |
| `rust-service/src/abstraction/plugin.rs`, `taxonomy.rs` | SAFE — classification inputs and seed patterns, not transmitted. |
| `rust-service/src/ipc/router.rs` (`handle_raw_event`, new in this pass) | SAFE — receives `RawEvent`, passes it by value into `abstraction_engine.process`, and only the returned `AbstractedEvent`'s `stable_id`/`label`/`category`/`taxonomy_version` are persisted into `RawEventEntry` or forwarded to the upload batcher. The raw `app_name`/`window_title` strings are dropped when `RawEvent` goes out of scope at the end of the function — confirmed no field of `RawEvent` other than `event_id`/`occurred_at` is read outside the `process(...)` call. |
| `swift-client/Sources/VelvtMac/Collection/CollectionModule.swift` (`FocusWindowEvent`) | SAFE — local capture struct, consumed by `EventRelay` only. |
| `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift` (`RawEventMessage`) | INTENTIONAL CROSSING, not a violation — this is the one designed crossing point (Swift → Rust over the local Unix socket). Rust is the documented enforcement boundary and strips these fields before any further processing, per Audit 1 above. |

**Result: zero VIOLATION findings.** No raw field crosses into any
serializable/loggable type, IPC payload, or HTTP request body.

---

## Audit 2 — Token exposure

**Examined:** every use of `access_token`, `refresh_token`, `device_token`,
JWT-like strings, and the new `AccountAuthService`/`HttpDeviceRegistrar`
code added in this pass.

| Location | Finding |
|---|---|
| `rust-service/src/auth/tokens.rs` (`RedactedString`, `TokenPair`) | SAFE — `Debug`/`Display` print `[redacted]`; `expose()` is `pub(crate)`, called only at the two HTTP boundary sites (`auth/http.rs`, `auth/account.rs`). |
| `rust-service/src/auth/http.rs` | SAFE — `.expose()` used only inside the `reqwest` request builder. |
| `rust-service/src/auth/store.rs` (`KeychainTokenStore`) | SAFE — tokens only ever read/written via `security_framework::passwords::*`, never logged. Device ID storage (new in this pass) uses the same Keychain entry mechanism, separate account key (`<account>.device_id`); device ID is not a secret but is still kept out of SQLite per the "Keychain only" rule for anything auth-adjacent. |
| `rust-service/src/auth/device.rs` (`HttpDeviceRegistrar`, new) | SAFE — receives `device_id`/`TokenPair` from the HTTP response and immediately calls `store.store_pair(...)`/`store.store_device_id(...)`; no `tracing::` call in this file references either. |
| `rust-service/src/auth/account.rs` (`AccountAuthService`, new) | SAFE — `.expose()` is called exactly twice, both to populate `AuthSuccess.access_token`/`.refresh_token` for the one IPC message designed to carry them to Swift (matching `proto/schema/auth_success.json`, which documents "Swift stores them in Keychain only"). No `tracing::` call in this file. |
| `rust-service/shared-types/src/lib.rs` (`AuthSuccess`) | SAFE — plain `String` fields are required to match the wire schema, but a hand-written `Debug` impl (added in this pass) redacts `access_token`/`refresh_token` to `[redacted]`; `user_id` and `expires_at` are not secrets and are shown. Covered by `auth_success_debug_redacts_tokens_but_keeps_user_id` test. |
| `rust-service/shared-types/src/lib.rs` (`SignUp`, `LogIn`) | SAFE — `password` carried as plain `String` to match the wire schema (consistent with the existing `email`/`password` convention already in `proto/schema/sign_up.json`/`log_in.json` before this pass); hand-written `Debug` impl redacts both `email` and `password`. Covered by `sign_up_and_log_in_debug_redact_credentials` test. |
| `swift-client/Sources/VelvtMac/Auth/AuthModule.swift` (`KeychainProtocol`) | SAFE — tokens routed through Keychain only, never held as a logged `String`. |

**Result: zero VIOLATION findings.** No location where a token or password
value could appear in a log line, error message, or crash report —
verified both by code inspection and by the redaction unit tests added in
this pass.

---

## Audit 3 — Upload payload verification

**Schema (derived from `rust-service/src/upload/dto.rs`):**

```rust
pub struct BatchEventPayload {
    pub stable_id: String,          // hash-derived, not raw content
    pub label: String,               // e.g. "document:edit"
    pub category: String,            // e.g. "focus_work"
    pub taxonomy_version: String,
    pub occurred_at: DateTime<Utc>,
    pub duration_seconds: u64,
}

pub struct BatchPayload {
    pub batch_id: String,
    pub schema_version: String,
    pub client_version: String,
    pub supported_abstraction_types: Vec<String>,
    pub category_taxonomy_version: String,
    pub events: Vec<BatchEventPayload>,
}
```

`event_id` is marked `#[serde(skip)]` and never serialized.

**Forbidden field check:** titles, app names, bundle IDs, URLs, paths,
filenames, contacts, emails, phone numbers, raw text — none of these types
appear in `BatchEventPayload`/`BatchPayload`, directly or nested. The new
live-ingestion path added in this pass (`R7Router::handle_raw_event` →
`EventIngestor::ingest` → `UploadBatcher::ingest_abstracted`) constructs
`BatchEventPayload` exclusively via
`BatchEventPayload::from_abstracted(event_id, &abstracted_event, duration_seconds)`,
which reads only the five privacy-safe `AbstractedEvent` accessors — there
is no code path in the new wiring that could pass a raw field into this
struct.

**Result:** confirmed by inspection and by the pre-existing
`payload_serialization_contains_only_audited_safe_fields` test in
`tests/upload_batching.rs`, which explicitly asserts `event_id`,
`raw_app_name`, `raw_window_title`, `app_name`, and `window_title` are
absent from the serialized JSON. No VIOLATION.

---

## Audit 4 — Log content review

**Examined:** every `tracing::`, `Logger(`, `print(`, `NSLog(` call site
that interpolates a variable, across both workspaces, including all
call sites added in this pass (`main.rs` device registration/auth-state
watcher, `ipc/router.rs` raw-event handling, `delivery/fetch.rs`
notification push).

| Call site | Variable(s) | Finding |
|---|---|---|
| `main.rs:114-118` (`device_registration_failed`) | `error: DeviceRegistrationError` | SAFE — `thiserror` enum with static messages, no payload content. |
| `main.rs:124-128` (`device_id_load_failed`) | `error: TokenStoreError` | SAFE — same. |
| `ipc/router.rs` (`raw_event_persist_failed`, `raw_event_ingest_failed`, `abstraction_failed`) | `error: PersistenceError` / `CoordinatorError` / `AbstractionError` | SAFE — typed errors, no raw event content. `event_id` itself (a UUID, not user content) is included in the `RawEventAck` response but never logged. |
| `delivery/fetch.rs` (pre-existing `tracing::debug!`/`warn!` near the new `push_notification` call) | `date`, `confidence_level`, `error` | SAFE — unchanged by this pass; no `insight.text` interpolation anywhere. |
| `delivery/push.rs` (all `push_*` methods, including new `push_notification`/`push_device_revoked`/`push_needs_reauth`/`push_service_status`) | message type names, error codes | SAFE — no method logs the payload content it pushes (e.g. `push_notification` never logs `title`/`body`). |
| `swift-client/Sources/VelvtMac/App/ServiceProcessLauncher.swift` (new) | `error.localizedDescription` | SAFE — `Process.run()` failure (e.g. file-not-found), not event or token content. |

**Result: zero findings of user-data-carrying variables interpolated
without redaction.**

---

## Audit 5 — ONNX inference boundary

**Examined:** `rust-service/src/abstraction/plugin.rs` (`EmbeddingSimilarityPlugin`) and `onnx.rs`.

- **Privacy boundary comment present:** confirmed at `plugin.rs:211` ("PRIVACY BOUNDARY: this is the only call site that passes the [...]").
- **Concatenated input string never logged, stored, or returned in an error type:** confirmed — `onnx.rs`'s `OrtEmbeddingModel` methods return typed `EmbeddingError` variants that carry no string payload from the input; no `tracing::` call in `onnx.rs` or the embedding call site in `plugin.rs` interpolates the input text.
- **Stateless inference:** confirmed — `OrtEmbeddingModel::embed` takes `&self` and the input by reference, does not write to any field on `self`, and the `ort` session is not retained across calls in a way that could leak state between requests (each call constructs and consumes its own input tensor).

This module was not touched in this integration pass; findings are
unchanged from the pre-existing implementation.

**Result: no VIOLATION findings.**

---

## Sign-off

This audit was completed on 2026-06-16 against commit `7742c9d` (plus the
uncommitted MVP integration changes described in the accompanying PR:
device registration, account-auth relay, raw-event ingestion wiring,
notification push, and the v6/v7 protocol additions). Re-run this audit
before merging if further changes touch `abstraction/`, `upload/`,
`auth/`, or `ipc/`.
