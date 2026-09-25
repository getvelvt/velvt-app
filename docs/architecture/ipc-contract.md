# Velvt IPC Contract

> **Reconciled with `proto/` at protocol 31 on 2026-09-25.** This page
> summarizes the contract; `proto/schema/*.json` is authoritative for every
> field, and `proto/CHANGELOG.md` records why each version exists. Before this
> reconciliation the page had last been reconciled on 2026-07-17 (protocol 15–17
> era): its direction lists stopped at `clear_work_block_data`, and its `raw_event`
> entry lacked `duration_seconds` (17), `focused_document_url` (23),
> `declared_app_category` and `document_type_ids` (30).

## 1. Architecture and Ownership

Velvt has two local workspaces with a strict ownership boundary:

- The Swift client owns macOS event capture, Accessibility permission handling,
  the menu bar UI, local notification scheduling, and display of payloads that
  are already ready for users. It reports facts (an event, a Focus/DND
  transition, a tap) and never decides a category, a drift, a time, or copy.
- The Rust service owns raw event ingestion, abstraction, SQLite persistence,
  authentication, upload batching, cloud synchronization, every device-local
  decision (work blocks, the drift gate, Focus/DND evidence, invitations, the
  weekly digest, demotion), and delivery of ready-to-display payloads back to
  Swift.

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
     |<-- privacy_violation_alert ----------------------|
     |<-- insight_payload ------------------------------|
     |<-- history_payload ------------------------------|
     |                                                  |
     |--- request_latest_insight / history ------------>|
     |<-- insight_payload / history_payload / cache_empty|
     |                                                  |
     |--- sign_up / log_in / log_out / delete_account ->|
     |<-- auth_success / auth_failure / needs_reauth ---|
     |<-- account_deletion_accepted / device_revoked ---|
     |                                                  |
     |--- request_menu_status / flush_upload_queue ---->|
     |<-- menu_status ----------------------------------|
     |                                                  |
     |--- start/pause/resume/end work block ----------->|
     |--- work block lifecycle/recovery/clear --------->|
     |<-- work_block_state (incl. active_intervention) -|
     |--- report_intervention_outcome ----------------->|
     |--- intervention_card_seen ---------------------->|
     |                                                  |
     |--- request_local_dashboard --------------------->|
     |<-- local_dashboard ------------------------------|
     |--- corrections / request_correction_history ---->|
     |<-- menu_status / correction_history_page --------|
     |--- request_unclassified_triage ----------------->|
     |<-- unclassified_triage --------------------------|
     |--- set_application_category -------------------->|
     |                                                  |
     |--- focus_state_changed ------------------------->|
     |<-- quiet_hours_offer ----------------------------|
     |--- respond_quiet_hours_offer ------------------->|
     |--- request_initiation_invitation --------------->|
     |<-- initiation_invitation ------------------------|
     |--- request_weekly_digest ----------------------->|
     |<-- weekly_digest --------------------------------|
     |--- request_intervention_explanation ------------>|
     |<-- intervention_explanation ---------------------|
     |--- request_demotion_state ---------------------->|
     |<-- demotion_state -------------------------------|
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

- Rust accepts only (`ClientMessage` in `rust-service/shared-types/src/lib.rs`):
  - handshake and errors: `client_hello`, `error_response`;
  - events: `raw_event`;
  - cloud payloads: `request_latest_insight`, `request_latest_history`;
  - account: `sign_up`, `log_in`, `auth_session`, `log_out`,
    `delete_account`;
  - status and upload: `request_menu_status`, `flush_upload_queue`;
  - corrections: `correct_event_classification`,
    `update_classification_override`, `request_correction_history`,
    `remove_classification_override`, `reset_classification_overrides`,
    `request_unclassified_triage`, `set_application_category`;
  - work blocks: `start_work_block`, `pause_work_block`, `resume_work_block`,
    `end_work_block`, `request_work_block_state`,
    `accept_work_block_recovery`, `report_intervention_outcome`,
    `intervention_card_seen`, `work_block_lifecycle`,
    `clear_work_block_data`;
  - local dashboard: `request_local_dashboard`;
  - Focus/DND and quiet hours: `focus_state_changed`,
    `respond_quiet_hours_offer`;
  - initiation: `request_initiation_invitation`,
    `dismiss_initiation_invitation`, `set_initiation_settings`,
    `request_initiation_settings`;
  - demotion, digest and explanation: `request_demotion_state`,
    `reset_intervention_demotion`, `request_weekly_digest`,
    `acknowledge_weekly_digest`, `request_intervention_explanation`.
- Rust emits only (`ServerMessage`):
  - handshake and errors: `server_hello`, `acknowledged`, `version_mismatch`,
    `malformed_message`, `error_response`, `shutting_down`;
  - events and status: `raw_event_ack`, `service_status`,
    `privacy_violation_alert`, `menu_status`;
  - cloud payloads: `insight_payload`, `history_payload`, `cache_empty`,
    `notification_payload`;
  - account: `auth_success`, `auth_session_updated`, `auth_failure`,
    `account_deletion_accepted`, `needs_reauth`, `device_revoked`;
  - corrections: `correction_history_page`, `unclassified_triage`;
  - work blocks and local surfaces: `work_block_state`, `local_dashboard`,
    `quiet_hours_offer`, `initiation_invitation`, `initiation_settings`,
    `demotion_state`, `weekly_digest`, `intervention_explanation`.
- Swift sends only the Rust inbound set and accepts only the Rust outbound set.
  A test-only `dummy_extension` client variant exists under
  `cfg(test)` / the `extensibility-proof` feature and is never on the wire.

## 3. Message Catalog

Schemas declare their JSON Schema draft and reject undeclared fields with
`additionalProperties: false`.

### Local work-block messages

Direction: Swift to Rust for start, pause, resume, end, state request,
recovery acceptance, lifecycle, and clear; Rust to Swift for
`work_block_state`.

Protocol state version 1 contains the persisted phase and timing, optional
local intention/purpose/intensity, current safe classification evidence, the
live block's anchor category (protocol 31; the drift gate's own anchor, null
until a confident observation has closed and outside an active or paused
block), Rust-authored status line, and optional terminal result. The result
contains planned/elapsed duration, longest stretch, neutral transition and return
counts, confidence/coverage, safe evidence category, Rust-authored observation,
and exactly one next action from a closed registry: `protect_next_10` or, since
protocol 27, `soft_restart_10`. The in-block drift offer (`active_intervention`,
protocol 24) only ever carries `protect_next_10`. Since protocol 26 the result
may also carry `dnd_outcomes` and one Rust-authored `reconciliation` line for
nudges held while Focus/DND was on. These messages are local-socket-only;
none is an upload or notification contract. Full field constraints live in
`proto/schema/*work_block*.json` and the privacy boundary is documented in
`docs/architecture/work-block-loop.md`.

Every message uses a `type` discriminant and a `payload` object. Catalog fields
listed below live inside `payload`.

Optional-field encoding is strict. Omit absent optional properties entirely,
including `drop_reason`, `reason`, `related_event_id`,
`do_not_disturb_until`, and `menu_status.queued_events[].local_label`. The
`raw_event.bundle_id` property is required by the schema but nullable, so an
unknown bundle identifier is encoded as JSON `null`. The protocol-30
`raw_event` fields `declared_app_category`, `document_type_ids` and the
protocol-23 `focused_document_url` are optional: an older client omits them and
its events classify exactly as before.

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
- `duration_seconds`: locally measured dwell, integer 0–1800 (protocol 17);
  both sides enforce the 1,800-second cap
- `app_name`: raw local-only application name
- `window_title`: raw local-only focused-window title
- `bundle_id`: required, nullable raw local-only bundle identifier. Rust
  stores only a domain-separated hash of it (`app_bundle_stable_id`)
- `focused_document_url`: optional nullable raw browser document URL
  (protocol 23). Rust reduces it to a validated hostname and discards it
  before persistence, upload, logging, or any other DTO
- `declared_app_category`: optional nullable `LSApplicationCategoryType`
  from the application's own `Info.plist` (protocol 30)
- `document_type_ids`: optional array of the `LSItemContentTypes` the
  application declares, deduplicated and sorted, at most 256 identifiers of at
  most 64 characters; an oversized list is sent empty rather than truncated
  (protocol 30)

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
- `evidence`: the privacy-safe evidence the insight was rendered from —
  observation, comparison, suggested action, tone stage, metric, safe
  categories, confidence, coverage and baseline (protocol 18; the schema lists
  every field)
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
- `confidence_level`: `none`, `low`, `medium`, or `high`
- `active_seconds`: non-negative active duration
- `focused_seconds`, `meaningful_switch_count`, `longest_uninterrupted_seconds`:
  aggregate Today-surface counts (protocol 18). The cloud API names the last one
  `focus_seconds`; Rust renames it when it parses the API response
- `baseline_status`, `baseline_comparison`, `type_proportions` (per-category
  seconds and proportion)

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

### `privacy_violation_alert`

Direction: Rust to Swift. Purpose: report that the cloud rejected an upload
batch for a terminal privacy violation.

- `type`: literal `privacy_violation_alert`
- `code`: literal `raw_field_rejected`
- `message`: safe server-supplied rejection diagnostic; never the batch payload

### `request_latest_insight`

Direction: Swift to Rust. Purpose: request one cached or freshly fetched daily
insight.

- `type`: literal `request_latest_insight`
- `date`: calendar date formatted `YYYY-MM-DD`

### `request_latest_history`

Direction: Swift to Rust. Purpose: request a ready-to-display history window.

- `type`: literal `request_latest_history`
- `days`: positive number of requested days

### `cache_empty`

Direction: Rust to Swift. Purpose: report that the requested `insight_payload`
or `history_payload` is not cached or generated yet.

- `type`: literal `cache_empty`
- `payload_type`: `insight_payload` or `history_payload`

### Auth and account messages

Direction: Swift to Rust for `sign_up`, `log_in`, `log_out`, and
`delete_account`; Rust to Swift for `auth_success`, `auth_failure`,
`account_deletion_accepted`, `needs_reauth`, and `device_revoked`.

Credentials and tokens are permitted only in these auth-specific wire
messages. Rust redacts credential-carrying DTOs in `Debug`, Swift stores
received tokens in Keychain, and neither workspace may log these payloads.

### `notification_payload`

Direction: Rust to Swift. Purpose: deliver ready-to-schedule notification copy.

- `notification_id`: stable notification identifier
- `title` / `body`: Rust-authored display copy; Swift schedules exactly this
- `insight_date`: calendar date formatted `YYYY-MM-DD`
- `do_not_disturb_until`: optional ISO 8601 UTC timestamp ending in `Z`

### `request_menu_status`

Direction: Swift to Rust. Purpose: request local service/cloud status and a
privacy-safe queued-event snapshot.

Payload is empty.

### `flush_upload_queue`

Direction: Swift to Rust. Purpose: request an immediate flush/retry of queued
privacy-safe upload work, then return a fresh `menu_status`.

Payload is empty and never carries event data.

### `menu_status`

Direction: Rust to Swift. Purpose: report service status for the menu popover.

- `device_id`: optional device identifier
- `cloud_ready`: whether the Rust service's cloud readiness probe succeeded
- `last_successful_sync_at`: timestamp of the last successful sync, or null
- `upload_status`: privacy-safe upload state. Values are `ready`, `pending`,
  `retrying`, `auth_required`, `network_unavailable`, `rate_limited`, and
  `privacy_rejected`.
- `last_upload_error_code`: optional safe error code from an active pending,
  failed, or rejected upload batch. Sent batches must not keep stale retry or
  authentication errors visible.
- `next_upload_attempt_at`: optional timestamp for the next pending/failed
  retry attempt when upload backoff is active
- `pending_upload_batch_count`: count of pending upload batches
- `failed_upload_batch_count`: count of failed upload batches awaiting retry
- `rejected_upload_batch_count`: count of terminal privacy-rejected batches
- `queued_event_count`: aggregate count of queued events
- `queued_events`: newest privacy-safe queue summaries, each containing
  `label`, `category`, optional local-only `local_label`, classification
  status/confidence/source, the compatibility tier, and `occurred_at`
- `correction_history`: a bounded page of device-local correction rules
  (protocol 21)
- `correction_acknowledgment`: optional; set only on the status returned by a
  correction command, never on a polled one (protocol 25)

Upload diagnostics are derived from durable batch metadata only. They must not
include raw event content, upload payloads, URLs, file paths, app names, window
titles, or account secrets. `local_label` is display-only and must never be
copied into upload DTOs or logs. It is always a curated safe label, never a raw
title, URL, or arbitrary app identity.

Classification corrections create exact device-local rules. Removal deletes
one rule by a local stable event identifier; reset deletes all rules. Neither
operation uploads a raw correction target or opaque mapping key. A corrected
event may still sync its already-safe category by event ID through the existing
cloud correction endpoint.

### Messages added from protocol 13 to protocol 31

Every message below is local IPC only: none is uploaded, and no upload DTO has
a field any of them could occupy. Field lists are in `proto/schema/`; the
version in brackets is where the message or field arrived.

**Corrections and triage.**

- `correct_event_classification` [13; optional `local_activity_name` 21]:
  Swift to Rust. Correct one event's category, optionally naming the activity.
- `update_classification_override` [22]: Swift to Rust. Edit a saved rule's
  alias or category after its upload event is gone.
- `request_correction_history` [22] / `correction_history_page` [22; `scope`
  30]: searchable, offset-paginated device-local rules, at most 20 a page.
  Each item's `scope` is `window` or `app`; for an app rule `stable_id` is the
  application's key hash, so a client must read the scope before acting on it.
- `request_unclassified_triage` / `unclassified_triage` [30]: up to 8
  applications Velvt observed but could not classify, ranked by observed time
  and floored at five minutes, each with the local name Velvt already holds,
  seconds observed, and event count. `app_stable_id` is the only identifier:
  no bundle key crosses the socket. No category and no guess. An empty list is
  the good state.
- `set_application_category` [30]: Swift to Rust. The one-tap answer from that
  list: an app-scoped rule keyed by `app_stable_id`, with no event id.

**Local dashboard.**

- `request_local_dashboard` / `local_dashboard` [16; early Today signal 19;
  replaced by two branches 20]: Swift asks with a window and its UTC offset;
  Rust answers with explicit-work-block Focus Fragmentation and Daily
  Activity, one row per local day. Rust owns clipping, transitions, switching
  clusters, recoveries, coverage, comparison, day boundaries and label
  aggregation; Swift only renders. **Known drift:** Rust now emits
  `DAILY_ACTIVITY_DAYS` = 14 rows (`rust-service/src/dashboard.rs`), while
  `proto/schema/local_dashboard.json` still declares `daily_activity` with
  `minItems`/`maxItems` 7 from protocol 20. The schema, not the code, is stale;
  correcting it is a proto change and is not made here.

**Drift offers and outcomes.**

- `work_block_state.active_intervention` [24; `salience` 25]: the in-block
  drift offer, rendered in-app and, at `normal` salience, as a notification
  with Rust-authored copy. A `quiet` offer renders the card without a
  notification.
- `work_block_state.anchor_category` [31]: the broad category the drift gate
  treats as the live block's anchor, computed by the gate's own function.
  Required and nullable: null outside an active or paused block and until a
  confident observation has closed; absent entirely from a pre-31 service.
- `report_intervention_outcome` [24; `was_focused` 25]: Swift to Rust. The
  person's explicit answer to an offer. Silence is not representable; it is
  recorded when the block ends unanswered.
- `intervention_card_seen` [29]: Swift to Rust. The card was actually on
  screen — a delivery fact, never a response.
- `request_intervention_explanation` / `intervention_explanation` [28]: one
  grounded sentence explaining the block's most recent shown offer. Accepts no
  user text.
- `request_demotion_state` / `demotion_state` and
  `reset_intervention_demotion` [28]: the deterministic, versioned
  auto-demotion policy over the rolling wrong-intervention counter, and the
  one-tap resume from it.

**Focus/DND and quiet hours.**

- `focus_state_changed` [26]: Swift to Rust. A coarse Focus/DND transition:
  `active`, the time, and the UTC offset. The Focus mode's name and schedule
  are unrepresentable.
- `quiet_hours_offer` / `respond_quiet_hours_offer` [26]: a next-morning offer
  from the late-night DND rule; accepting configures Velvt's own quiet hours.

**Initiation and weekly receipts.**

- `request_initiation_invitation` / `initiation_invitation` /
  `dismiss_initiation_invitation` [27]: at most one daily invitation to a
  25-minute soft start, schedule-free by construction. Dismissal only ever
  reduces future invitations. `start_work_block.invitation_id` claims one.
- `set_initiation_settings` / `request_initiation_settings` /
  `initiation_settings` [27]: the single Rust-owned opt-out.
- `request_weekly_digest` / `weekly_digest` / `acknowledge_weekly_digest`
  [28]: the weekly receipts digest for the last completed local week, held
  during quiet hours and Focus/DND.

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
updates in both workspaces. The v12 upload diagnostics addition bumps the
protocol because it adds required `menu_status` fields.

## 5. Privacy Boundary

Raw identifying data includes application names, window titles, bundle IDs,
URLs, paths, filenames, contact names, email addresses, and raw text.

Only `raw_event` carries captured raw values — the window title, the URL, the
bundle identifier, and the declared metadata. It travels locally from Swift to
Rust and must never be reused as an upload payload. `raw_event_ack` may
identify the source event only by UUID and must not echo raw values.

A second group of messages is local-only by design and carries device-local
identity or text the person typed, so that the menu bar can show it. They are
never uploaded, logged, or written to crash diagnostics, and each schema says
so in its `$comment`:

- the local display label on `menu_status.queued_events[].local_label` and in
  `local_dashboard`'s `daily_activity` rows (with `suggested_name`);
- the activity names and stable identifiers in
  `correct_event_classification`, `update_classification_override`,
  `request_correction_history` (search text) and `correction_history_page`;
- `unclassified_triage.entries[].display_name`, which can be the raw
  application name Velvt holds for an app it could not classify, with the
  bundle **key hash** (never the identifier) in `bundle_id`, and
  `set_application_category`'s `app_stable_id` and `activity_name`;
- the work-block `intention` in `start_work_block` and `work_block_state`.

All other messages are privacy-safe control messages, derived summaries, or
ready-to-display payloads. Their closed object definitions prevent undeclared
raw fields.

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
5. Encode only `ClientMessage` variants and decode only `ServerMessage`
   variants.
6. Preserve exact schema field names, discriminator values, timestamp formats,
   and optional-field omission rules.
7. Reconnect with exponential backoff and keep raw events only in a bounded
   in-memory ring buffer (`EventRelay`, default 500 events, oldest dropped
   first).
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
events are held in `EventRelay`'s in-memory ring buffer (default capacity 500
events); when it is full the oldest event is dropped and counted. The buffer is
never written to disk, SQLite, or `UserDefaults`. Every successful reconnection
starts with a new handshake.

A `version_mismatch` is the one error the client does not arm a reconnect for.
On the first dial it usually means a helper from an earlier install is still
holding the socket; the app then reclaims the socket from that orphan (only a
process running this bundle's own helper executable, as this user, that this
app did not start), relaunches its own helper, and dials again — at most twice
before showing an alert (`AppDelegate.connectRetryingVersionMismatch`,
`OrphanedHelperReaper` in `swift-client/Sources/VelvtMac/App/AppModule.swift`).

### Clean Shutdown

On shutdown, Rust stops accepting connections, flushes the pending privacy-safe
upload batch, closes active socket connections, and removes its socket file.
Swift closes its connection and tears down active observers. Socket failures
may be logged by safe error code and socket path only; message content must
never be logged.
