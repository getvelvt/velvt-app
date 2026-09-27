# Deferred Seams

Seams intentionally left as stubs or partial implementations for the MVP.
Each entry below was reviewed during the MVP integration pass and judged
safe to defer — none of them block a real user installing and running
Velvt today.

**Last reviewed 2026-09-25 against `develop` (protocol 31; the shipped 1.0.11
is protocol 30).** That
review rewrote three entries the shipped code had overtaken — the weekly
report (a local weekly digest shipped in protocol 28), notification
scheduling (there are now two notification kinds), and the helper lifecycle
(orphan recovery is now automatic) — and re-confirmed the rest.

---

## TitleAbstractor passthrough

- **Location:** [`rust-service/src/abstraction/mod.rs`](rust-service/src/abstraction/mod.rs) (`DefaultTitleAbstractor` / `NoOpTitleAbstractor`), wired as the default in [`AbstractionEngineBuilder::new`](rust-service/src/abstraction/engine.rs).
- **What it stubs:** the engine passes the window title through unchanged to the classifier plugins (heuristic, browser-context and embedding rungs read it) without rewriting semantically sensitive tokens into category-scoped abstract labels. Still true on 2026-09-25: `DefaultTitleAbstractor` is a passthrough.
- **Why it's safe to defer:** documented as V1 scope in `README.md` and `CONTRIBUTING.md` since before this integration pass. Window titles never leave the abstraction module or appear in any serialized type — `AbstractedEvent` and `BatchEventPayload` carry only `stable_id`/`label`/`category`. There is no privacy leak today, only a missed opportunity for finer-grained Tier 1 classification.
- **Trigger condition:** a V1 issue implementing a real `TitleAbstractor` (e.g. a small on-device model or rule set) and registering it via `AbstractionEngineBuilder::title_abstractor(...)`.
- **Estimated complexity:** medium — requires a new classification artifact and privacy review of whatever transformation it performs.

## Local analytics engine

- **Location:** none. `AGENTS.md` and `CONTRIBUTING.md` used to describe a feature-flagged `rust-service/src/analytics/` stub; they were corrected on 2026-09-25 to say no such module exists.
- **What it stubs:** nothing — the module does not exist at all, not even as a stub. Confirmed via `grep -r "mod analytics"` returning no matches and `src/lib.rs` not declaring it. The nearest code is `src/behavior/` (declared in `main.rs`): shadow BOCPD and HMM segmentation models and an antecedent miner, validated only on synthetic traces, with no caller in the shipped path. They cannot reach the drift gate, delivery, or any copy surface.
- **Why it's safe to defer:** out of scope, and new engine work is gated (`AGENTS.md`, Scope Boundary). No shipped code path reaches either the missing module or the shadow models, so there is no dormant attack surface beyond the three empty tables `PRIVACY.md` already lists (`out_of_block_run`, `block_antecedent`, `antecedent_finding`).
- **Trigger condition:** a dedicated, separately-scoped issue for local analytics or on-device LLM inference, backed by a dated founder decision, with its own privacy review.
- **Estimated complexity:** large — new module, new privacy boundary, new tests.

## Cloud weekly/monthly report

- **Location:** not implemented. What shipped instead is local: protocol 28 added the **weekly receipts digest** (`request_weekly_digest` / `weekly_digest` / `acknowledge_weekly_digest`, `proto/schema/weekly_digest.json`), produced by `rust-service/src/receipts/` from local aggregates (migration 0024) and rendered as a card by `WorkBlockView` and `MenuBarPopoverView`. `R6`/`S6` fetch daily insight and history (a 7-day window on the background refresh, 14 days on demand).
- **What it stubs:** a cloud-derived, longer-horizon (weekly or monthly) summary from `velvt-core`. The local digest is exact bounded counts for the last completed local week — blocks declared and completed, recoveries, wrong interventions, invitations accepted — and deliberately not a report.
- **Why it's safe to defer:** the weekly surface a user sees exists and needs no account. No partial cloud wiring exists to audit or harden.
- **Trigger condition:** a scoped issue for a cloud weekly/monthly summary, which would need a new proto message, a Rust cache table, and a Swift view — and a reason the local digest does not already answer.
- **Estimated complexity:** medium.

## Cross-device behavioral model

- **Location:** not implemented anywhere.
- **What it stubs:** any model of user behavior that spans more than one registered device.
- **Why it's safe to defer:** explicitly V2 scope. The current device-bound auth model (one `device_id`, one token pair, one local SQLite database) has no multi-device concept at all, so there is nothing partially built to harden.
- **Trigger condition:** a V2 architecture proposal covering cross-device identity and sync, which has its own large privacy-review surface (this single device's privacy guarantees don't trivially extend to "all of a user's devices").
- **Estimated complexity:** large.

## Notification scheduling beyond immediate and relative-interval triggers

- **Location:** [`swift-client/Sources/VelvtMac/Delivery/NotificationScheduler.swift`](swift-client/Sources/VelvtMac/Delivery/NotificationScheduler.swift).
- **What it stubs:** calendar-based or location-based triggers, notification grouping/threading, and any scheduling logic beyond the two paths that exist.
- **What exists (corrected 2026-09-25):** Velvt posts **two** kinds of notification. The daily insight arrives as `notification_payload` and is scheduled with a `UNTimeIntervalNotificationTrigger` when `do_not_disturb_until` is in the future — Rust has populated that field from Velvt's quiet hours since 2026-09-22 (entry below). The in-block drift offer comes from `work_block_state.active_intervention` and is posted immediately through `InterventionNotificationScheduling`, because an offer is only true at the moment its evidence was gathered; it is raised only inside a declared block, at most once per block, and a Focus/DND hold is recorded as `delivery_suppressed_dnd` rather than delivered later. The soft-start invitation and the weekly digest are in-app cards, not notifications. This entry previously said only the daily insight was sent and that no payload set `do_not_disturb_until`; both were out of date.
- **Why it's safe to defer:** both kinds have the semantics they need. Nothing queued needs a calendar or location trigger, and at most one drift offer per block leaves nothing to group.
- **Trigger condition:** a third notification kind whose timing neither path fits, or evidence that grouping is needed.
- **Estimated complexity:** small-medium.

## ~~`do_not_disturb_until` is always sent as absent~~ — RESOLVED 2026-09-22

- **Location:** [`rust-service/src/delivery/fetch.rs`](rust-service/src/delivery/fetch.rs) `push_notification` call site.
- **What it stubs:** quiet-hours / do-not-disturb scheduling for the notification pushed after a fresh insight fetch. The field exists end-to-end in the protocol (`proto/schema/notification_payload.json`, both DTOs) and Swift's scheduler already branches on its presence, but Rust never populates a non-`nil` value.
- **Why it's safe to defer:** the field is optional in the schema and Swift already handles its absence by scheduling immediately — there is no broken contract, just an unimplemented quiet-hours policy.
- **Resolved 2026-09-22.** The policy already existed and was never wired up:
  `FocusManager::in_velvt_quiet_hours` has read a per-user configured window
  (default 22:00-07:00 local) since quiet hours shipped. What was missing was a
  *deadline* rather than a predicate. `FocusManager::quiet_hours_end` now returns
  the instant the current window closes, both it and the predicate derive from
  one private helper so they cannot disagree, and `PollScheduler` carries the
  deadline into `push_notification` through the narrow `QuietHoursSource` trait
  (declared beside its consumer, like `work_block::FocusStateSource`).
- **Semantics, unchanged:** `do_not_disturb_until` means *do not deliver before*.
  An insight raised at 03:00 lands at 07:00; it is never dropped. A user who has
  not configured quiet hours is never deferred.
- **Not covered:** the intervention path. `InterventionNotificationScheduling`
  is a separate seam with no DND window by design — a drift offer is only ever
  raised inside a declared work block, and macOS Focus suppression is already
  handled there by `DeliverySuppressedDnd`.

## Multi-instance / `SMAppService` helper lifecycle

- **Location:** [`swift-client/Sources/VelvtMac/App/ServiceProcessLauncher.swift`](swift-client/Sources/VelvtMac/App/ServiceProcessLauncher.swift), and the orphan recovery in [`swift-client/Sources/VelvtMac/App/AppModule.swift`](swift-client/Sources/VelvtMac/App/AppModule.swift) (`OrphanedHelperReaper`, `AppDelegate.connectRetryingVersionMismatch`).
- **What it stubs:** the bundled `velvt-service` helper is started as a plain child `Process` of the Swift app and stopped with `SIGTERM` on quit. It does not use `SMAppService` and does not survive the Swift app being force-quit independently.
- **Recovery is now automatic (updated 2026-09-25; shipped in 1.0.11).** When the first dial meets a `version_mismatch` — an orphaned helper from an earlier install or crashed run still holding the socket — the app asks that orphan to quit, relaunches its own helper, and dials again, at most twice (`maximumOrphanReclaimAttempts`) before it falls back to an alert with **Retry**. It terminates only a process that runs this bundle's own helper executable, as this user, that this app did not start; `OrphanedHelperRecoveryTests.swift` covers the rule and the sequence. The paragraph below is the pre-1.0.11 analysis, kept because it explains why this recovery had to exist. What remains manual is the case the rule declines on purpose: an orphan launched from a bundle at a *different* path (the app was moved between runs) is not matched, and the person gets the alert; restarting the Mac clears it.
- **Why it's safe to defer:** for a single-user MVP install, one helper process per app launch, torn down on quit, is sufficient. `TokioUnixTransport` fails fast rather than corrupting state if the path is already bound, and the IPC client reconnects with backoff while the socket is merely absent.
- **What that rationale does not cover, corrected 2026-08-31:** an earlier version of this entry justified the deferral with "the IPC client already reconnects with backoff." That is true of every error class except the one this deferral produces. An orphan from a crashed prior run keeps the socket, the newer bundled helper exits on `duplicate_service_instance` (`main.rs`), and the client completes a handshake against the survivor — which answers with a protocol version it does not accept. `versionMismatch` is the only `IPCError` branch that publishes `.disconnected` without arming reconnect (`UnixSocketIPCClient.swift`), so the client stays down until something re-dials `connect()`. Recovery is a deliberate user action, not automatic backoff. The deferral is that recovery is manual; it is not that the state is unreachable.
- **Trigger condition:** real-world reports of the moved-bundle case, or a requirement for the service to keep running across Swift app updates.
- **Estimated complexity:** medium — `SMAppService` registration, login-item UX, and a migration path off the current ad-hoc launcher.

## Upload-batch ownership (`BatchRetentionPolicy`)

- **Location:** [`rust-service/src/upload/coordinator.rs`](rust-service/src/upload/coordinator.rs) (`BatchRetentionPolicy`, `KeepAllBatches`, `UploadCoordinator::with_retention_policy`).
- **What it stubs:** the seam for refusing to upload a batch that was queued under a different account. `resume_pending` and `flush_all_pending` both consult `should_discard` before rebuilding a payload, but nothing in `src/` calls `with_retention_policy`, so the coordinator always runs on `KeepAllBatches` and discards nothing. No rule can be written against the trait as it stands: `UploadBatch` carries no device or user identifier, and neither does the `upload_batch` row behind it, so no implementation can tell one owner from another. `tests/account_deletion.rs` demonstrates the rule working against a test double and says on its face that no such rule runs in production.
- **Why it's safe to defer:** the ownership hazard this seam exists to close is already closed at the point that matters. `ClientMessage::DeleteAccount` destroys the resumable queue once the cloud accepts the deletion, so there is no surviving batch for a later account to inherit. The seam covers only the residual case of a process that dies between the cloud's acceptance and the purge — and `BatchAssembler` already derives a batch id by hashing the device id with the event ids, so the identifier a real rule would need exists and is recoverable without a migration.
- **Trigger condition:** a second account signing in on a Mac that has ever held a queue, or any report of activity arriving against the wrong account; also any change that makes the delete path non-atomic in a new way. Re-checked 2026-09-25: `with_retention_policy` still has no caller in `src/`. A tester who signs into a second account on a shared Mac is exactly this trigger, so revisit it before any cohort uses sign-in.
- **Estimated complexity:** low — a `MintedByThisDevice` policy exists in `tests/account_deletion.rs` and needs the device id threaded to the coordinator plus a `with_retention_policy` call at the one construction site.

---

## Format

Each entry above follows: name/location, what it stubs, why it is safe to
defer for MVP, and the trigger condition + estimated complexity for taking
it on later.
