# Deferred Seams

Seams intentionally left as stubs or partial implementations for the MVP.
Each entry below was reviewed during the MVP integration pass and judged
safe to defer — none of them block a real user installing and running
Velvt today.

---

## TitleAbstractor passthrough

- **Location:** [`rust-service/src/abstraction/mod.rs`](rust-service/src/abstraction/mod.rs) (`DefaultTitleAbstractor` / `NoOpTitleAbstractor`), wired as the default in [`AbstractionEngineBuilder::new`](rust-service/src/abstraction/engine.rs).
- **What it stubs:** Tier 1 currently passes the raw window title straight to the seed-dictionary/embedding plugins without rewriting semantically sensitive tokens into category-scoped abstract labels.
- **Why it's safe to defer:** documented as V1 scope in `README.md` and `CONTRIBUTING.md` since before this integration pass. Window titles never leave the abstraction module or appear in any serialized type — `AbstractedEvent` and `BatchEventPayload` carry only `stable_id`/`label`/`category`. There is no privacy leak today, only a missed opportunity for finer-grained Tier 1 classification.
- **Trigger condition:** a V1 issue implementing a real `TitleAbstractor` (e.g. a small on-device model or rule set) and registering it via `AbstractionEngineBuilder::title_abstractor(...)`.
- **Estimated complexity:** medium — requires a new classification artifact and privacy review of whatever transformation it performs.

## Local analytics engine

- **Location:** referenced in `AGENTS.md`/`CONTRIBUTING.md` as `rust-service/src/analytics/`.
- **What it stubs:** nothing — the module does not exist at all, not even as a stub. Confirmed via `grep -r "mod analytics"` returning no matches and `src/lib.rs` not declaring it.
- **Why it's safe to defer:** explicitly out of MVP scope. No code path references it, so there is no dead/dormant attack surface to audit.
- **Trigger condition:** a dedicated, separately-scoped issue for local analytics or on-device LLM inference, with its own privacy review.
- **Estimated complexity:** large — new module, new privacy boundary, new tests.

## Weekly report fetch and display

- **Location:** not implemented; `R6`/`S6` only implement 7-day history and daily insight.
- **What it stubs:** any longer-horizon (weekly/monthly) summary view.
- **Why it's safe to defer:** explicitly V1 scope per `CONTRIBUTING.md` MVP boundary ("7-day insight history"). No partial wiring exists to audit or harden.
- **Trigger condition:** a V1 issue adding a `weekly_payload` proto message, Rust cache table, and Swift view.
- **Estimated complexity:** medium.

## Cross-device behavioral model

- **Location:** not implemented anywhere.
- **What it stubs:** any model of user behavior that spans more than one registered device.
- **Why it's safe to defer:** explicitly V2 scope. The current device-bound auth model (one `device_id`, one token pair, one local SQLite database) has no multi-device concept at all, so there is nothing partially built to harden.
- **Trigger condition:** a V2 architecture proposal covering cross-device identity and sync, which has its own large privacy-review surface (this single device's privacy guarantees don't trivially extend to "all of a user's devices").
- **Estimated complexity:** large.

## Notification scheduling beyond `UNTimeIntervalNotificationTrigger`

- **Location:** [`swift-client/Sources/VelvtMac/Delivery/NotificationScheduler.swift`](swift-client/Sources/VelvtMac/Delivery/NotificationScheduler.swift).
- **What it stubs:** calendar-based or location-based triggers, notification grouping/threading, and any scheduling logic beyond a single relative-interval trigger with `do_not_disturb_until` deferral.
- **Why it's safe to defer:** explicitly called out as may-remain-stubbed in the MVP integration brief. The current trigger is sufficient to deliver the one notification type Velvt sends today (daily insight ready). `do_not_disturb_until` is already wired through `NotificationPayload` (proto v7) even though no payload currently sets it to a non-`nil` value — see "Known limitations" below.
- **Trigger condition:** a follow-up issue once a second notification type (e.g. weekly report, ad-hoc nudges) needs different scheduling semantics.
- **Estimated complexity:** small-medium.

## `do_not_disturb_until` is always sent as absent

- **Location:** [`rust-service/src/delivery/fetch.rs`](rust-service/src/delivery/fetch.rs) `push_notification` call site.
- **What it stubs:** quiet-hours / do-not-disturb scheduling for the notification pushed after a fresh insight fetch. The field exists end-to-end in the protocol (`proto/schema/notification_payload.json`, both DTOs) and Swift's scheduler already branches on its presence, but Rust never populates a non-`nil` value.
- **Why it's safe to defer:** the field is optional in the schema and Swift already handles its absence by scheduling immediately — there is no broken contract, just an unimplemented quiet-hours policy.
- **Trigger condition:** a follow-up issue defining the actual quiet-hours policy (e.g. a per-user configured window) and threading it into `FetchService`.
- **Estimated complexity:** small.

## Multi-instance / `SMAppService` helper lifecycle

- **Location:** [`swift-client/Sources/VelvtMac/App/ServiceProcessLauncher.swift`](swift-client/Sources/VelvtMac/App/ServiceProcessLauncher.swift).
- **What it stubs:** the bundled `velvt-service` helper is started as a plain child `Process` of the Swift app and stopped with `SIGTERM` on quit. It does not use `SMAppService`, does not survive the Swift app being force-quit independently, and does not detect or recover from a second instance already holding the Unix socket (e.g. a prior crashed run).
- **Why it's safe to defer:** for a single-user MVP install, one helper process per app launch, torn down on quit, is sufficient. `TokioUnixTransport` fails fast rather than corrupting state if the path is already bound, and the IPC client reconnects with backoff while the socket is merely absent.
- **What that rationale does not cover, corrected 2026-08-31:** an earlier version of this entry justified the deferral with "the IPC client already reconnects with backoff." That is true of every error class except the one this deferral produces. An orphan from a crashed prior run keeps the socket, the newer bundled helper exits on `duplicate_service_instance` (`main.rs`), and the client completes a handshake against the survivor — which answers with a protocol version it does not accept. `versionMismatch` is the only `IPCError` branch that publishes `.disconnected` without arming reconnect (`UnixSocketIPCClient.swift`), so the client stays down until something re-dials `connect()`. Recovery is a deliberate user action, not automatic backoff. The deferral is that recovery is manual; it is not that the state is unreachable.
- **Trigger condition:** real-world reports of orphaned helper processes after a force-quit or crash, or a requirement for the service to keep running across Swift app updates.
- **Estimated complexity:** medium — `SMAppService` registration, login-item UX, and a migration path off the current ad-hoc launcher.

## Upload-batch ownership (`BatchRetentionPolicy`)

- **Location:** [`rust-service/src/upload/coordinator.rs`](rust-service/src/upload/coordinator.rs) (`BatchRetentionPolicy`, `KeepAllBatches`, `UploadCoordinator::with_retention_policy`).
- **What it stubs:** the seam for refusing to upload a batch that was queued under a different account. `resume_pending` and `flush_all_pending` both consult `should_discard` before rebuilding a payload, but nothing in `src/` calls `with_retention_policy`, so the coordinator always runs on `KeepAllBatches` and discards nothing. No rule can be written against the trait as it stands: `UploadBatch` carries no device or user identifier, and neither does the `upload_batch` row behind it, so no implementation can tell one owner from another. `tests/account_deletion.rs` demonstrates the rule working against a test double and says on its face that no such rule runs in production.
- **Why it's safe to defer:** the ownership hazard this seam exists to close is already closed at the point that matters. `ClientMessage::DeleteAccount` destroys the resumable queue once the cloud accepts the deletion, so there is no surviving batch for a later account to inherit. The seam covers only the residual case of a process that dies between the cloud's acceptance and the purge — and `BatchAssembler` already derives a batch id by hashing the device id with the event ids, so the identifier a real rule would need exists and is recoverable without a migration.
- **Trigger condition:** a second account signing in on a Mac that has ever held a queue, or any report of activity arriving against the wrong account; also any change that makes the delete path non-atomic in a new way.
- **Estimated complexity:** low — a `MintedByThisDevice` policy exists in `tests/account_deletion.rs` and needs the device id threaded to the coordinator plus a `with_retention_policy` call at the one construction site.

---

---

## Format

Each entry above follows: name/location, what it stubs, why it is safe to
defer for MVP, and the trigger condition + estimated complexity for taking
it on later.
