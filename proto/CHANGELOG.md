# IPC Protocol Changelog

## Version 28 - 2026-08-07

- Added `request_demotion_state` (Swift to Rust) and `demotion_state` (Rust
  to Swift): the deterministic, versioned auto-demotion policy over the
  rolling wrong-intervention counter, disclosed as a feature and inspectable
  on demand. The payload carries only the two bounded counts the rate is
  computed from, the versioned policy constants (threshold percent, minimum
  sample, window days, threshold and re-promotion policy versions), the
  current state, the demotion instant when demoted, and Rust-authored
  disclosure copy. No transition history or timeline is representable, and
  the payload exists on the local IPC surface only — it is never uploaded.
- Added `reset_intervention_demotion` (Swift to Rust): the user's explicit
  one-tap resume from the demoted state. Resetting restarts the demotion
  evaluation window from the reset instant; it never edits or discards the
  underlying outcome record, and the wrong-intervention counter itself is
  untouched.
- Added `request_weekly_digest` / `acknowledge_weekly_digest` (Swift to
  Rust) and `weekly_digest` (Rust to Swift): the weekly receipts digest for
  the most recent completed local week, pull-delivered like invitations and
  held during quiet hours and Focus/DND. Every count is read from the same
  stored aggregates the local metrics use; recoveries and completions lead,
  the wrong-intervention count appears exactly once, and no streak, chain,
  or failure tally is representable. Local IPC surface only — never
  uploaded.
- Added `request_intervention_explanation` (Swift to Rust) and
  `intervention_explanation` (Rust to Swift): the one-tap "explain this
  nudge" affordance. Deterministic code selects the claim, evidence, and
  tone from the stored intervention record; the response is exactly one
  grounded sentence that cannot exceed that evidence. The request accepts no
  user text, and no reply, follow-up, or thread exists anywhere on this
  surface (D7). The tap is counted locally as a coarse, content-free weekly
  bucket only.

## Version 27 - 2026-08-07

- Added `request_initiation_invitation` (Swift to Rust): asks the
  deterministic, versioned initiation policy whether one invitation is
  pending. The request carries only the client's UTC offset; every gate
  (good hours, minimum samples, daily cap, quiet hours, Focus/DND, active
  block, opt-out, backoff) is owned and enforced in Rust, and below any
  minimum-sample gate the answer is silence — never a default window.
- Added `initiation_invitation` (Rust to Swift): at most one daily
  invitation to a 25-minute soft start. The payload is schedule-free by
  construction — no good-hours window, weekday, hour bucket, or timing
  evidence is representable — and carries only an opaque invitation id, the
  registered `soft_start_25` action, registered copy, the block duration,
  and the policy version.
- Added `dismiss_initiation_invitation` (Swift to Rust): the one-tap
  dismissal. Dismissal only ever reduces future invitations under the
  versioned backoff policy; repeated dismissal silences invitations
  entirely for a versioned interval.
- Added optional `invitation_id` to `start_work_block`: one tap on an
  invitation starts a declared block through the existing declaration path.
  Rust validates the id and records a content-free origin marker locally;
  the marker never appears in any IPC, cloud, log, or telemetry payload.
- Added `set_initiation_settings` / `request_initiation_settings` (Swift to
  Rust) and `initiation_settings` (Rust to Swift): the single Rust-owned
  opt-out for invitations. Opting out silences invitations and changes
  nothing else.
- Registered `soft_restart_10` as the second closed-registry action:
  `accept_work_block_recovery.action_id` and the work-block result's
  `next_action.action_id` now admit `protect_next_10` or `soft_restart_10`.
  The in-block drift offer (`active_intervention`) still only ever carries
  `protect_next_10`.

## Version 26 - 2026-08-07

- Added `focus_state_changed` (Swift to Rust): a coarse system Focus/DND
  transition report carrying only `active`, the transition time, and the
  client's UTC offset. Swift observes; Rust owns the Focus/DND evidence
  record and every decision derived from it. The Focus mode's name,
  configuration, and schedule are structurally unrepresentable in this
  message and never cross IPC.
- Added the Focus/DND outcome enum values `completed_under_dnd` and
  `delivery_suppressed_dnd`. They appear in the new optional
  `dnd_outcomes` array of the work-block result: a block completed while
  DND was active records `completed_under_dnd` (a success everywhere a
  completed block counts), and each mid-block nudge held because DND was
  active records `delivery_suppressed_dnd`. Held nudges are never delivered
  by another channel, never retried mid-block, and reconcile after the
  block as counts only.
- Added optional `reconciliation` to the work-block result: at most one
  Rust-authored calm post-block line noting what was held. Analyst voice;
  no reference to what the user missed.
- Added `quiet_hours_offer` (Rust to Swift): a deterministic, versioned
  next-morning offer produced by the late-night DND pattern rule. It is an
  offer, never a workaround, and carries only the rule version, a distinct
  day count, the proposed local window, and Rust-authored copy.
- Added `respond_quiet_hours_offer` (Swift to Rust): one-tap acceptance
  configures Velvt's own quiet hours; a decline is remembered locally and
  the offer is not re-asked for a versioned interval.

## Version 25 - 2026-08-08

- Added `was_focused` to `ReportInterventionOutcome.response`. The existing
  vocabulary could not express a false positive: `dismissed` means "not now",
  `not_helpful` concedes the drift happened, and `wrong_classification`
  disputes a label rather than the judgment. Only `was_focused` says the offer
  should never have fired, which makes it the ground-truth input to the
  wrong-intervention rate.
- Added `salience` (`normal` | `quiet`) to `active_intervention`. A quiet offer
  renders the in-app card without sending a notification. Salience only ever
  decreases: it drops after the user pushes an offer away and is restored by an
  offer that lands, never by continued drift.
- Added optional `correction_acknowledgment` to `MenuStatus`. It is set only on
  the status returned by a correction command, never on a polled one, so a
  correction is visibly believed without the confirmation reappearing every
  refresh.
- Documented `active_intervention` in `schema/work_block_state.json` and added
  the missing `schema/report_intervention_outcome.json`; both shipped in 24
  without a schema entry.

## Version 24 - 2026-07-31

- Added `ReportInterventionOutcome` so the user's explicit response to an
  in-session drift offer reaches the service. Silence is deliberately not
  representable: it is recorded when the block ends, never inferred from a
  notification disappearing.
- Added optional `active_intervention` to the work-block snapshot so an offer
  renders in-app. The in-app surface is the primary path; an OS notification
  depends on authorization and is suppressed by Focus.
- Aligned the intervention action registry with 0.1.5 Scope 4:
  `return_to_anchor` is now `protect_next_10`.

## Version 23 - 2026-07-29

- Added optional `focused_document_url` to the sole raw-event IPC message so
  event-driven browser tab changes can be distinguished when Accessibility
  exposes the focused document.
- The URL is local-only raw input. Rust reduces it to a validated hostname for
  classification and stable local identity, then discards it before SQLite,
  upload, telemetry, logging, dashboard, or delivery DTO construction.
- Events collected before authentication are retained for local first value
  with `upload_eligible = false` and never enter the cloud upload queue.

## Version 22 - 2026-07-26

- Added searchable, offset-paginated device-local correction history with a
  maximum page size of 20 so the popover never loads the full local rule set.
- Added local-only editing of persisted activity aliases and categories after
  the associated upload event is no longer present.
- Added bounded local activity-row context for inline corrections. Stable local
  identifiers, suggested names, and bundle identifiers remain local IPC only.

## Version 21 - 2026-07-25

- Added a bounded device-local correction history to `menu_status`, backed by
  persisted personal overrides rather than the transient upload queue.
- `correct_event_classification` can carry an optional local activity name.
  The name stays in the local override/mapping path and is excluded from cloud
  correction requests, upload DTOs, telemetry, logs, and crash diagnostics.

## Version 20 - 2026-07-20

- Replaced the local dashboard contract with exactly two analytical DTO branches:
  explicit-work-block Focus Fragmentation and seven-row Daily Activity.
- Rust now owns 60-minute clipping, deduplicated category transitions, version-1
  five-minute switching clusters, recoveries, coverage, like-for-like comparison,
  local day boundaries, local display-label aggregation, `Other`, and grounded
  segment evidence. Swift only renders these bounded aggregates.
- Local display labels remain local-IPC-only and are absent from cloud, upload,
  telemetry, logging, and crash-diagnostic contracts.

## Version 19 - 2026-07-18

- The device-local dashboard now includes a Rust-authored early Today signal
  with finite evidence progress, actual observation bounds, broad-category
  aggregates, and one modest local action.
- The early signal contains no raw app names, titles, URLs, filenames, paths,
  contacts, local labels, or inferred intentions.

## Version 18 - 2026-07-18

- Daily insights now include the exact privacy-safe evidence layers, approved
  emotional stage, baseline comparison, and suggested action used to render
  the insight. Raw local activity remains forbidden.
- Daily history now carries aggregate focused seconds, meaningful switch
  count, and longest uninterrupted seconds for the Today surface.
- Early-baseline insight payloads remain inspectable but do not trigger a
  user notification.

## Version 17 - 2026-07-17

- Raw activity events now include a bounded, locally measured `duration_seconds`
  dwell interval. This remains local-only until the Rust privacy boundary
  abstracts and uploads the event.
- Both the Swift collector and Rust privacy boundary enforce the 1,800-second
  maximum so unattended time cannot be counted as active use.

## Version 16 - 2026-07-17

- Added the bounded, local-only recent-activity dashboard request and snapshot.
- Dashboard rows contain safe categories and aggregate metrics only; raw app
  names and window titles never cross the Rust privacy boundary.

## Version 15 - 2026-07-16

- Added the device-local meaningful-work loop: start, pause, resume, end,
  lifecycle, recovery, clear-data, and state request/response messages.
- `work_block_state` is Rust-authored and versioned independently at state
  version 1. Free-form intention is permitted only on local IPC and is absent
  from cloud/upload/cache/notification contracts.
- Added a singular bounded `next_action` to the safe local session result;
  exactly one 10-minute recovery action is representable.

## Version 14 - 2026-07-16

- Queued-event summaries now distinguish classification status, confidence,
  and provenance while retaining the legacy tier for compatibility.
- Added device-local removal and reset operations for personal classification
  rules. Neither operation transmits a raw target or local mapping key.

## Version 13 - 2026-07-15

- Added `correct_event_classification` for device-local personal overrides and historical sync.
- Queued-event summaries now carry event/stable identifiers and classification provenance.

## Version 12 - 2026-07-08

- Added required upload diagnostics to `menu_status`: `upload_status`,
  `last_upload_error_code`, `next_upload_attempt_at`,
  `pending_upload_batch_count`, `failed_upload_batch_count`, and
  `rejected_upload_batch_count`.
- This is a coordinated protocol bump because `menu_status` uses a closed
  schema and the new diagnostics fields are required by Rust and Swift DTOs.

## Version 11 - 2026-06-25

- Added `auth_session` client message: the host client supplies a locally
  persisted device auth session to Rust after connection. Rust keeps it in
  memory only.
- Added `auth_session_updated` server message: Rust tells the host client to
  persist refreshed or reissued auth credentials in platform credential storage.
- Added `device_id` to `auth_success`; signup/login now returns a device-bound
  session for host-side persistence.
- Added optional `user_access_token`, `user_refresh_token`, and
  `user_expires_at` to `auth_success`, `auth_session`, and
  `auth_session_updated`. Device tokens remain the default credentials for API
  calls; user tokens are replayed only so Rust can refresh user auth and reissue
  device-bound credentials after relaunch.

## Version 10 - 2026-06-21

- Added optional `local_label` to `menu_status.queued_events`. It is a
  device-local display field for the queue inspector and must never be copied
  into cloud upload payloads or logs.

## Version 9 - 2026-06-21

- Added `flush_upload_queue`, an empty client request for an explicit upload
  queue flush. This version defines the wire contract only; service routing and
  upload behavior are introduced separately.

## Version 8 - 2026-06-20

- Added `request_menu_status` / `menu_status` for privacy-safe menu settings.

## Version 7 - 2026-06-16

- Added `notification_payload` server message: a ready-to-schedule
  notification pushed after a fresh (non-cached) daily insight fetch.
  `notification_id`, `title`, and `body` are Rust-authored display copy —
  Swift schedules exactly this content and never generates notification text
  itself. `insight_date` is the calendar date the insight covers.
  `do_not_disturb_until`, when present, is a future timestamp before which
  Swift must not deliver the notification.
- This message type and its Swift DTO (`NotificationPayload`,
  `ServerMessage.notificationPayload`) existed in `swift-client/` prior to
  this version but had no `proto/schema/` entry, no Rust counterpart, and no
  version bump — a partial protocol update that the version-bump process
  below exists to prevent. This entry retroactively closes that gap.

## Version 6 - 2026-06-15

- Added `sign_up` client message: Swift sends email/password credentials; Rust
  performs the HTTP signup and responds with `auth_success` or `auth_failure`.
- Added `log_in` client message: Swift sends email/password credentials; Rust
  performs the HTTP login and responds with `auth_success` or `auth_failure`.
- Added `log_out` client message: fire-and-forget notification to Rust that the
  client has cleared its local session. Rust revokes the server session.
- Added `delete_account` client message: Swift requests permanent account
  deletion. Rust responds with `account_deletion_accepted`.
- Added `auth_success` server message: carries `user_id`, `access_token`,
  `refresh_token`, and `expires_at`. Swift stores tokens in Keychain.
- Added `auth_failure` server message: carries `code` and `message`. Codes:
  `invalid_credentials`, `network_error`, `server_error`.
- Added `account_deletion_accepted` server message: confirms that the Rust
  service accepted and processed the account deletion request.
- Added `needs_reauth` server message: pushed when the session expires or the
  access token cannot be refreshed. Swift must clear Keychain and show login.
- Added `device_revoked` server message: pushed when the device registration is
  permanently revoked. Swift clears Keychain and shows the Device Revoked screen.

## Version 5 - 2026-06-15

- Added `shutting_down` server message: sent to all connected clients immediately
  before a graceful service shutdown. The `reason` field is `"sigterm"` or
  `"sigint"`. Clients should disconnect and reconnect after the service restarts.

## Version 4 - 2026-06-14

- Added `request_latest_insight` client message: Swift requests the insight for a
  specific date; Rust responds with `insight_payload` or `cache_empty`.
- Added `request_latest_history` client message: Swift requests history for the
  last N days; Rust responds with `history_payload`.
- Added `cache_empty` server message: returned when the requested payload has no
  cached entry yet. `payload_type` identifies which payload was requested.
- `insight_payload` and `history_payload` are now also pushed proactively after a
  successful cloud fetch and after `privacy_violation_alert` events, without a
  corresponding client request.

## Version 3 - 2026-06-14

- Added `privacy_violation_alert` from Rust to Swift for terminal cloud privacy
  rejection notifications.

## Version 2 - 2026-06-13

- Changed version negotiation to server-first `server_hello`, `client_hello`,
  and `acknowledged` or `version_mismatch` messages.

The integer in `version` identifies the IPC protocol version implemented by
both local workspaces. Every connection begins with a version handshake.

## Versioning Policy

### Non-Breaking Changes

Backward-compatible documentation clarifications do not require a version
bump. Additive optional fields may remain within the current version only when
both workspaces can safely ignore them. Because schemas are closed, even an
optional-field addition requires coordinated schema and DTO updates.

### Breaking Changes

Removing or renaming fields, changing field meaning or type, making optional
fields required, changing enum values, changing message direction, or adding
or removing message types is breaking and requires a protocol version bump.

### Version-Bump Process

1. Update the integer in `proto/version`.
2. Update every affected schema in `proto/schema/`.
3. Add a dated changelog entry describing compatibility impact.
4. Update Rust DTOs, dispatch, and contract tests.
5. Update Swift DTOs, dispatch, and contract tests.
6. Verify both workspaces negotiate and reject versions as documented.
7. Land `proto/`, `rust-service/`, and `swift-client/` changes atomically in
   the same commit.

Partial protocol updates are prohibited and must not be merged.

## Version 2

- Changed negotiation to `server_hello` followed by `client_hello`.
- Added typed `acknowledged`, `version_mismatch`, and `malformed_message` responses.
- Wrapped every message body in a tagged `payload` object.

## Version 1

- Initial newline-delimited JSON contract.
