# Privacy Audit — velvt-mac MVP Integration

**Current state: Audit 8 (2026-09-25, protocol 31, migrations 0001–0038).**
Read it first. Audits 2, 3 and 4 below describe code as it was at protocol
v6/v7 and are superseded by Audit 8 §§ 8.5–8.7; they are kept as history, not
as a description of the shipped code. Audit 5 describes a build feature the
distributable does not include (§ 6.5).

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

> **Superseded by Audit 8 § 8.6 (2026-09-25).** The `KeychainTokenStore` row
> below describes a Rust credential store that no longer exists: the service
> has held the session Swift hands it in memory only (`VolatileTokenStore`),
> with no Keychain item of its own, since commit `29c7bf3` (2026-06-25)
> removed the Rust Keychain store.

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

> **Superseded by Audit 8 § 8.5 (2026-09-25).** The struct below is the v6/v7
> DTO. The shipped serializer emits none of `stable_id`, `label` or
> `taxonomy_version`; § 6.2 and § 8.5 give the wire format as captured.

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

> **Superseded by Audit 8 § 8.7 (2026-09-25).** The line numbers and call
> sites below are from protocol v6/v7.

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

## Audit 6 — Re-run at protocol 28 (2026-08-21)

Audits 1-5 above were performed at protocol **v6/v7**. The protocol is now at
**v28** — 21 revisions later — so those findings were stale and are superseded
for the fields listed here. This re-run was performed against commit `3d2af2f`
on branch `integration/d1-truth-fixes`, and against a **live database with
23,026 real events**, not fixtures.

### 6.1 Two device-local columns that Audits 1-5 did not cover

| Column | Contents | Verdict |
|---|---|---|
| `raw_event_buffer.local_name_suggestion` (migration 0011) | The **raw application name, verbatim**, for classifier misses only | DISCLOSED, not a violation |
| `raw_event_buffer.local_display_label` | Friendly UI string (`Gmail`, `YouTube`, `Coding`) | DISCLOSED, not a violation |

**Measured on the live database:** 11,373 of 23,026 rows (49%) carry a non-null
`local_name_suggestion`, holding **15 distinct raw application names**. 22,271 of
23,026 rows (97%) carry a non-null `local_display_label`.

**Two documentation claims were falsified by this measurement and have been
corrected in the same commit as this audit:**

1. `PRIVACY.md` stated *"Despite its name, `raw_event_buffer` never contains raw
   app names or window titles — see PRIVACY_AUDIT.md Audit 1 for the
   verification."* The window-title half is true. **The app-name half was false**,
   and it cited this document as its verification.
2. `ARCHITECTURE.md` R3 stated `local_display_label` is *"forced to `NULL` by the
   DAL and covered by tests."* **It is populated on ~97% of rows.**

Neither is a leak. Both are a documentation failure, which in a product whose
thesis is honest measurement is its own category of defect.

### 6.2 The boundary itself — re-verified by running

- **No raw field is reachable from `upload/`.** `grep -rn` for `app_name`,
  `window_title`, `local_name_suggestion`, `local_display_label`, and
  `focused_document_url` across `rust-service/src/upload/` returns **zero hits**.
- **`BatchEventPayload` emits six fields.** Its hand-written `Serialize` impl
  (`upload/dto.rs:19-56`) holds `stable_id`, `label`, and `taxonomy_version` on
  the struct and emits **none of them**. On the wire: `event_id`, `occurred_at`,
  `abstraction_type` (one of eight), `abstraction_type_version`,
  `classification_tier`, `payload{duration_seconds, category}`.
- **End-to-end, against real data.** `batch_event` — the upload mirror — holds
  **23,024 rows**. Searching every text column for the six most identifying app
  names present in `local_name_suggestion` (`Google Chrome`, `WhatsApp`,
  `DaVinci Resolve`, `RStudio`, `GitHub Desktop`, `Messages`) returns **0 matches
  each**. The table has no `local_name_suggestion` or `local_display_label`
  column at all — the exclusion is structural, not filtered.
- **No log path interpolates them.** No `tracing::` call anywhere in
  `rust-service/src/` references `local_name`, `app_name`, `window_title`, or
  `display_label`. `local_name_suggestion` is additionally redacted to
  `[redacted]` in the `Debug` impl (`persistence/models.rs:83-84`).

### 6.3 `focused_document_url` — a raw field Audits 1-5 predate

Added after v7. Swift sends the focused browser document URL over the local
socket. `AbstractionEngine::process` (`abstraction/engine.rs:146-154`)
destructures it and immediately reduces it via `focused_site_context`
(`abstraction/browser.rs`) to **a validated hostname and nothing else** — paths,
queries, fragments, credentials, and ports discarded, `file://` rejected
outright. The raw URL is never read again. **SAFE**, and now documented in
PRIVACY.md's "What is collected", which previously omitted it while asserting
"Nothing else is observed."

### 6.4 Result

**Zero VIOLATION findings. Two DOCUMENTATION findings, both corrected in this
commit.** The boundary holds; the description of it did not.

### 6.5 Not re-verified in this pass

Audits 2 (tokens), 3, 4, and 5 (ONNX) were **not** re-run at v28. They are 21
protocol revisions stale and should not be cited as current. Note separately
that the ONNX path is not built into the distributable —
`scripts/build_rust_helper.sh` builds without `--features onnx` — so Audit 5
describes code that does not ship.

---

## Audit 7 — the embedding cache, and the tool that could not see it (2026-08-31)

Audit 6 re-ran the raw-content boundary at protocol 28 and found the two
disclosed `raw_event_buffer` columns. It walked `TEXT` columns and it did not
walk `BLOB` columns, so it did not reach `semantic_embedding_cache`. Neither did
`scripts/prove_local.sh`, for exactly the same reason and in the same words:
its `is_text_type` matched `*TEXT*|*CHAR*|*CLOB*`, so both embedding columns
fell into the "other" bucket beside the integers and were never enumerated. The
script's own header said numbers and timestamps "cannot carry a window title."
A blob is neither, and this one can.

### 7.1 What the sketch is

`DefaultTitleAbstractor` (`abstraction/mod.rs`) is a pass-through, and it is the
one installed (`abstraction/engine.rs`). So Tier 2 receives the verbatim window
title, `embedding_input` builds `"{app_name} [SEP] {window_title}"`
(`abstraction/plugin.rs`), and `HashedEmbeddingModel` writes a 256-dimension
sketch of it to `semantic_embedding_cache.embedding` under a SHA-256 of the same
string. Each word is added at weight 1.0 at a SHA-256-derived index and sign,
and every character trigram of `^word$` at 0.25 — roughly a nine-coordinate
joint signature per word.

### 7.2 Recovery, measured on the live database

`HashedEmbeddingModel` was reimplemented from this repository's source and run
against the live database on 2026-08-31. The stored vectors are L2-normalised;
the pre-normalisation scale is recoverable because every coordinate before
normalisation is a multiple of 0.25, so the correct scale is the one that makes
that true of all 256 at once.

| Measure | Result |
|---|---|
| Rows in `semantic_embedding_cache` | 512 |
| Rows yielding at least one dictionary word | **452 (88.3%)** |
| Distinct words recovered across the table | **935** |
| Dictionary used | `/usr/share/dict/words`, 234,143 entries, nothing bespoke |

Recovered words are visibly real title content: `chrome`, `terminal`, `claude`,
`overleaf`, `slack`, `zoom`, `citation`, `annotation`, `accessibility`.

Precision, on a synthetic sensitive title rather than on real data: the
normalised string `divorce attorney consultation booking` returns exactly
`{attorney, booking, consultation, divorce}` — four of 234,143 candidates, no
false positives.

An external review of the same code on the same day, using a different
dictionary, reported 448 of 512 rows and 1,190 distinct words. The two runs
agree.

### 7.3 The hash is unsalted

`add_hashed_feature` is a plain SHA-256 of the token. It is identical on every
install, so the oracle above is computable offline from published source and
needs nothing from the target machine. A per-install random salt would change
that to requiring the salt first. **It is not in the shipped code.** This is the
only recommendation in this audit that is not already closed.

> **Closed 2026-09-14, recorded 2026-09-25.** Commit `5b6ca1e` wired
> `EmbeddingSimilarityPlugin::builtin_salted` into `main.rs` with the
> per-install `embedding_salt` from migration 0031, which also deleted every
> sketch computed before it. The paragraph above was true on 2026-08-31 and
> stale from 2026-09-14; this document and `PRIVACY.md` went on repeating it
> until Audit 8. What the salt does and does not protect is § 8.2.

### 7.4 Verdict

**No VIOLATION.** Nothing about the sketch is uploaded, and the exclusion is
structural rather than filtered: `grep -rn` for `embedding`, `semantic`,
`prototype`, `local_name_suggestion`, `local_display_label`, `window_title`, and
`app_name` across `rust-service/src/upload/` returns **zero hits**, and
`batch_event` — the upload mirror — has no column that could hold one.

**Three DOCUMENTATION findings, all corrected in the same commit as this audit:**

1. `PRIVACY.md` contained zero occurrences of "embedding", "semantic",
   "prototype", or "vector", and asserted the schema "simply ha[s] no field that
   could hold" a raw field and preserves no "way to recover the original raw
   string." Both tables are now in the storage inventory, and the recoverability
   above is stated rather than implied away.
2. `scripts/prove_local.sh` reported `semantic_embedding_cache` as one text
   column and three "other" columns, printed 512 opaque hashes, and declared
   itself a proof. It now names every `BLOB` column, reports it as UNINSPECTED
   with a byte count, and says in its own header and closing caveat that this is
   the one column it cannot read out to you.
3. `out_of_block_run`, `block_antecedent`, and `antecedent_finding` are in the
   schema, are visible to anyone who opens the database, and have no writer
   outside tests. `PRIVACY.md` described the first two as though they were
   accruing and omitted the third. All three now carry the disclosure that
   `migrations/0029` already carried.

Also narrowed: Audit 5 above states the concatenated input string is "never
logged, stored, or returned in an error type." The string itself is not stored.
A hash of it and a sketch derived from it both are, and § 7.1-7.2 are the
current statement.

### 7.5 Not re-verified in this pass

Audits 2 (tokens), 3, and 4 remain stale at v6/v7 and were not re-run. Audit 5
(ONNX) describes code the distributable does not build, per § 6.5, and is
unchanged by this pass.

---

## Audit 8 — Re-run at protocol 31, on a database the service wrote (2026-09-25)

Audits 6 and 7 predate protocols 29, 30 and 31, the salted stable keys
(migration 0037), the egress ledger (0038) and `anchor_category`. Protocol 29
and 30 added bundle-identifier digests, raw declared metadata, a triage query
over unclassified applications, and application-scoped rules. This re-run
covers all of it, and re-does Audits 2, 3 and 4, which had not been run since
protocol v6/v7.

### 8.0 Method

- **Code.** `develop` at `a017d99` (protocol 31, migrations 0001–0038), built as
  a debug `velvt-service`, and again with this audit's changes applied (§ 8.3).
- **Database.** A throwaway database written by that binary through its real
  socket, not a fixture: the service ran with `HOME`, `VELVT_DATABASE_PATH` and
  the socket all inside a fresh temporary directory, so it could not open the
  founder's `~/.velvt`. `VELVT_API_BASE_URL` pointed at a loopback HTTP server
  the audit script ran, which answered like the API and recorded every request
  byte for byte. Nothing left the machine.
- **Input.** 69 synthetic raw events carrying sentinel strings, each a made-up
  token that appears nowhere else: two unclassified applications, one with a
  sentinel bundle identifier, declared category and document types; four
  seed-matched applications; window titles; browser URLs with credentials, a
  port, a path, a query and a fragment; a `file://` URL. Then, through the same
  socket: a failed log-in with a sentinel email and password, a restored
  session with sentinel tokens, a correction with a typed activity name, the
  triage list and an application rule with a second typed name, a work block
  with a sentinel intention fed the planted-drift trace `A-PLANT-4SWITCH-FOCUS`
  from `scripts/traces/` (the gate evaluated eight times and offered once),
  the dashboard, digest and invitation requests, uploads, Clear Local Work
  Blocks, Reset Corrections, and sign-out. The helper logged at `debug`,
  one level more verbose than the shipped default.
- **Surfaces read.** Every value of every column of every table, enumerated from
  `sqlite_master` and `PRAGMA table_info` and read as SQLite values, BLOBs
  included; the raw bytes of the database file before and after the two
  in-app deletions; the helper's complete log output; every request the
  loopback API received, with headers and bodies; every message the service
  sent back over the socket; `velvt-service --dry-run-egress`;
  `scripts/prove_egress.sh` and `scripts/prove_local.sh`; and every file the
  service created under the temporary `HOME`.

What this is not: a real person's usage. The sign-off rule below asks for a
database with real usage in it. The only such database is the founder's, and
this audit did not open it. The counts in Audits 6 and 7 remain the only
real-usage measurements; § 8.10 says what running the two proof scripts on a
real database would add.

### 8.1 The schema

37 tables plus `sqlite_sequence`, 267 columns. Every column now appears by name
in `PRIVACY.md`'s "Every column" table, and
`published_claims::migrated_schema_holds_exactly_the_documented_columns` fails
if the two differ in either direction. Before this audit the inventory was
closed only at the table level, and three columns added since Audit 7 were
named nowhere in the document: `work_block_intervention.card_seen_at` (0032),
`personal_app_override.app_only` (0035) and `personal_override.app_key_hash`
(0036). `raw_event_buffer.app_stable_id` and every `label` column were named
without being described.

### 8.2 Raw content at rest

Where each sentinel was found, by value:

| Sentinel | Columns holding it |
|---|---|
| unclassified application names (2), and `Safari` / `Google Chrome` on pages no rule matched | `raw_event_buffer.local_name_suggestion` only |
| a seed-matched service (`Slack`, `YouTube`) | `raw_event_buffer.label`, `abstraction_map.label`, `batch_event.label` (as `communication:slack`, `video:youtube`); `raw_event_buffer.local_display_label`, `abstraction_map.display_name` (as `Slack`, `YouTube`) |
| window-title tokens | none |
| URL path, query, fragment, credentials, port; the `file://` path | none |
| bundle identifiers | none (only the salted digest, in `raw_event_buffer.app_bundle_stable_id`) |
| declared category and document types | `raw_event_buffer.declared_app_category`, `raw_event_buffer.document_type_ids` |
| typed activity names | `abstraction_map.display_name`, `personal_override.activity_name`, `personal_app_override.activity_name`, and `raw_event_buffer.local_display_label` |
| work-block intention | `work_block.intention` |
| email, password, access and refresh tokens | none |

Everything in that table was disclosed except two things, now in `PRIVACY.md`:
the `label` columns name a service for the services the taxonomy knows by name,
including in the upload queue's `batch_event`; and a typed activity name is also
written to `raw_event_buffer.local_display_label` for the corrected event and
every later event the correction classifies.

Salting, checked by value rather than by reading `main.rs`:
`published_claims::no_column_holds_the_sentinels_outside_the_documented_exceptions`
now builds its router the way `main.rs` does, with `builtin_salted` and the
database's own `embedding_salt`, and asserts that the stored sketch for the
sentinel window is the one that salt produces and not the unsalted one. It
already asserted that no unsalted window, application or bundle digest is on
disk. Both salts sit in the same file as what they key, so they defeat an
offline guess made from this repository and a match between two Macs, and
nothing against someone holding the file. `PRIVACY.md` now says that in one
section rather than calling the sketch unsalted.

### 8.3 Deletion reaches the table but not the file — CODE finding, fixed

After Clear Local Work Blocks, `SELECT` found no work block, but the sentinel
intention was still readable in the raw database file (page 50 of 139), as was
the text of the deleted block result. SQLite without `secure_delete` unlinks a
deleted row and leaves its bytes in free space until the page is reused. The
same applies to every deletion the service makes: Reset Corrections, Undo, the
24-hour intention expiry, and every retention sweep.

Fixed in the same change: `SqlitePersistence::open` sets
`PRAGMA secure_delete = ON`, so SQLite zeroes deleted content.
`published_claims::deleted_text_does_not_survive_in_the_database_file` starts a
block with a sentinel intention, clears it, and reads the file's bytes; it
failed before the pragma and passes with it. Re-running the throwaway audit with
the fix, the only sentinels left in the file after the two deletions were values
still live in a table.

Not changed, and disclosed instead: Reset Corrections and Undo do not rewrite
`raw_event_buffer.local_display_label`, so a typed activity name stays on the
events it was applied to for up to 14 days after the correction is gone (5 rows
of one name and 1 of the other survived the reset here). Clearing it would also
blank the Daily Activity chart's labels for those days, which is a product
decision, not an audit one. Also disclosed, not changed: Velvt does not exclude
`~/.velvt/` from Time Machine.

### 8.4 What crosses the local socket

The Swift app receives, over the socket only: raw application names for
unclassified applications in `unclassified_triage` (`display_name`) and in
`local_dashboard` (Daily Activity labels); on-device labels and typed names in
`menu_status`, `correction_history_page` and `local_dashboard`; the intention in
`work_block_state`; and the session tokens in `auth_session_updated`, which is
how Swift keeps them in the Keychain. That is each schema's stated purpose, and
each of those schemas says "local IPC only". None of it reached a log, a
request, or a column beyond § 8.2.

`unclassified_triage` was documented as carrying an optional bundle key hash per
entry. The Rust type has never had the field and never sent it; the schema, the
protocol changelog, the Classification v2 contract and the Swift type are
corrected to match. A new test,
`rust-service/shared-types/tests/schema_conformance.rs`, builds a maximal and a
minimal instance of every file in `proto/schema/`, parses it as a Rust message,
and validates what Rust sends back. It found three more schemas describing a wire
Rust never produced (`history_payload`'s longest-stretch field, `menu_status`'s
`correction_history[].scope`, and `acknowledged`'s null payload); all four are
listed in `proto/CHANGELOG.md`. The `history_payload` one was a user-visible
bug: Swift read the field under the cloud API's name, which never crosses the
socket, so History showed a longest stretch of 0.

### 8.5 What leaves the device — supersedes Audit 3

The loopback API received 27 requests: 17 insight polls, 4 event batches, and
one each of session check, history fetch, readiness check, classification
sync, log-in and log-out. For every one of them `egress_ledger` holds a row, and
for 26 the row's `body_sha256` is the SHA-256 of the exact bytes received. The
27th is the log-in, whose row hashes the body with the password replaced by
`[redacted]`, as documented; recomputing that hash from the captured body
matched. `scripts/prove_egress.sh` reported the chain intact. In a second run
where the API refused every batch, `--dry-run-egress` printed the four queued
batches, and each printed SHA-256 matched the ledger row of an earlier attempt
to send it.

Every batch body had exactly these keys: `batch_id`, `schema_version`,
`client_version`, `category_taxonomy_version`, `supported_abstraction_types`,
`events`; each event `event_id`, `occurred_at`, `abstraction_type`,
`abstraction_type_version`, `classification_tier`, and
`payload{category, duration_seconds}`. No sentinel appeared in any request
path, header or body except the email and password in the log-in body, which is
where they belong. Headers were `accept`, `host`, `content-type` and
`content-length` on requests with a body, and `authorization` on signed-in
requests; no user agent. The classification sync
body is `{"category": ...}`; the typed name was not in it.

`PRIVACY.md`'s list of what leaves the device was wrong in both directions: it
named `/v1/auth/account/delete`, which nothing sends, and omitted
`DELETE /v1/account`, `GET /v1/auth/session`, `GET /v1/insights/poll`,
`GET /v1/ready` and `PATCH /v1/events/{event_id}/classification`. It now lists
exactly `egress::ENDPOINTS`, and
`published_claims::privacy_document_lists_exactly_the_endpoints_the_helper_can_reach`
compares the two; `egress_ledger.rs` already compares `ENDPOINTS` with the
source. Outside the helper, the Swift app makes no network request of its own
(no `URLSession` in `Sources/`) except Sparkle's appcast check, which is off in
both build configurations and was off in 1.0.11.

### 8.6 Tokens and credentials — supersedes Audit 2

- The Rust service stores no token anywhere: `VolatileTokenStore`
  (`auth/store.rs`) holds the session in memory for the process lifetime. There
  is no Rust Keychain code. No sentinel token or password was found in any
  column, in the file's bytes, or in the log.
- `RedactedString` (`auth/tokens.rs`) prints `[redacted]`; `.expose()` is called
  only where a request is built (`auth/http.rs`), where tokens are handed to
  Swift in `AuthSuccess` / `AuthSession`, and nowhere that logs. `SignUp`,
  `LogIn`, `AuthSession` and `AuthSuccess` have hand-written `Debug`
  implementations that redact the secrets (`shared-types/src/lib.rs`).
- The egress ledger hashes the sign-up, log-in and refresh bodies with the
  password or refresh token replaced by `[redacted]`, and records only whether
  an access token was attached, never its value.
- Swift keeps the session in one Keychain item, service `com.velvt.mac`,
  account `velvt.auth_snapshot`: device and account tokens, their expiries, user
  ID, device ID, email and a pending-deletion flag. The password is not stored.
  Sign-out and an accepted deletion delete every Velvt item under that service.
  `PRIVACY.md`'s deletion procedure named a `com.velvt.service.auth` item that
  no code has written since the Rust Keychain store was removed; corrected.

### 8.7 Logs — supersedes Audit 4

- Rust: 125 `tracing` call sites in `src/`. Their structured fields are error
  codes, typed errors, message-type names, counts, dates, and one configured
  model file path (`centroid_path`, at two sites); none interpolates an application name, title,
  URL, label, typed name, intention, email or token. The debug-level run
  produced 66 lines, 53 of them `hyper` connection-pool messages, and contained
  no sentinel.
- The helper's stdout and stderr reach nothing on disk: the Swift launcher
  reduces each chunk to a byte count and any `error_code` tokens before logging
  it (`ServiceProcessLauncher.redactedPipeDiagnostic`).
- Swift: every interpolation logged with `privacy: .public` is a count, a
  status, a notification surface name, an error domain or code, or that
  redacted pipe diagnostic. Thirteen calls interpolate without an annotation:
  twelve in `Auth/` (account-state descriptions, a message-type name, a user
  ID, an auth failure's code and server message, error descriptions) and one in
  `EventRelay` that logs two buffer counts. `os.Logger` redacts string
  interpolations as private by default, so of those only the counts are
  readable in a collected log.

### 8.8 Outside the database

`PRIVACY.md` said all persistence lives in the SQLite file. It now names the
Keychain item, the app's preferences (`com.velvt.mac`: onboarding state, the
purpose and intensity chosen at onboarding, window size, two counters, up to 256
opaque notification IDs — read from the code, not from a live preferences
file), Notification Center, the socket, exports the user saves, backups, and
`~/.velvt/agent-attention-log.jsonl`. That last file is written only by the
optional Claude Code notification hook in Velvt's private workspace, never by
the app; the hook bounds it to 256 KiB or 14 days per generation, two
generations, owner-only. The service created nothing under the temporary `HOME`
except `~/.velvt/` (0700) and the database (0600).

`PRIVACY.md` also said the app requests only Accessibility and Notifications
and listed only app name, title, URL and time as collected. The client also
reads the bundle identifier and the two `Info.plist` declarations, and asks
(optionally, at onboarding) for Focus status, of which it reads one boolean.
Corrected.

### 8.9 Result

**Zero VIOLATION findings.** No raw application name, bundle identifier, title,
URL part, typed name, intention, password or token reached an upload payload, a
request, or a log. The only request carrying a sentinel was the log-in, carrying
the email and password it exists to send.

**One CODE finding, fixed in the same change:** deleted rows stayed readable in
the database file (§ 8.3).

**Documentation findings, all corrected in the same change:** the egress list
(§ 8.5); the unsalted-sketch claim in `PRIVACY.md` and § 7.3 above; the
seven-versus-five column count and the undescribed `app_stable_id` and `label`
columns; the three unnamed columns (§ 8.1); the typed name in
`local_display_label`; the `com.velvt.service.auth` deletion step; the "all
persistence" sentence and the missing stores outside the database; the
permission and collection lists; and the four IPC schemas (§ 8.4).

**Open, for the founder:** whether Reset Corrections should also clear typed
names from `raw_event_buffer` (§ 8.3), and whether `~/.velvt/` should be
excluded from backups.

### 8.10 Not re-verified in this pass

Audit 5 (ONNX) is unchanged: the distributable still builds without
`--features onnx`. The real-usage counts of Audits 6 and 7 were not re-taken.
Running `scripts/prove_local.sh` and `scripts/prove_egress.sh` on a real
database would show which applications and labels a real Mac accumulates;
nothing in this audit suggests either would find a column this one did not.

---

## Sign-off

Audits 1-5 were completed on 2026-06-16 against commit `7742c9d` at protocol
v6/v7. **They are stale.** Audit 6 re-runs the raw-content boundary at protocol
28 against commit `3d2af2f` and a live 23,026-event database, on 2026-08-21.
Audit 7 extends it to the `BLOB` columns Audit 6 did not walk, against a live
24,160-event database, on 2026-08-31. Audit 8 re-runs every check at protocol
31 against `develop` at `a017d99`, on a database the real service wrote from
synthetic events, with every request captured, on 2026-09-25, and supersedes
Audits 2, 3 and 4.

Re-run this audit before merging if further changes touch `abstraction/`,
`upload/`, `auth/`, or `ipc/` — and **re-run it against a database the real
service wrote**, not fixtures, and against one with real usage in it when one
is available. Every defect corrected in Audits 6 and 7 was invisible to
fixture-based verification and took one SQL query against a real database to
find; the one Audit 8 found took reading the file's bytes rather than its rows.

One method note, since Audit 6 and Audit 7 found the same class of thing twice:
both audits, and `prove_local.sh` with them, walked the columns they knew how to
read. Enumerate from `sqlite_master` and account for every column by declared
type — including the ones the tool cannot render — or the next undisclosed
column will be the next one whose type nobody wrote a branch for.
