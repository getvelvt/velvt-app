# Privacy

Velvt is designed so that raw, identifying activity data never leaves your
device. This document is the canonical description of what is collected,
what is stored, what is transmitted, and how to audit or delete it
yourself. See [`PRIVACY_AUDIT.md`](PRIVACY_AUDIT.md) for the code-level
verification behind these claims.

## What is collected

The macOS client observes, locally, via the Accessibility APIs:

- Which application is currently focused (`NSWorkspace.didActivateApplicationNotification`)
- The focused window's title (`kAXFocusedWindowChangedNotification`, `kAXTitleChangedNotification`)
- Timestamps for these events

Nothing else is observed. Velvt does not use screen recording, keylogging,
the microphone, or the camera, and never will — the macOS app only requests
the Accessibility and Notifications permissions.

## What stays local

Raw application names, bundle identifiers, window titles, URLs, file paths,
filenames, and any text drawn from a window title never leave the device.
They exist transiently in the Swift collection layer and are forwarded
once, over a local Unix domain socket, to the Rust service running on the
same machine. The Rust service is the privacy enforcement boundary: its
abstraction engine consumes raw events and produces only an
`AbstractedEvent` — a stable local ID, a category-scoped label
(`document:edit`), a category, a taxonomy version, and a timestamp. The
Rust type system makes it structurally impossible for a raw field to
re-appear downstream: `AbstractedEvent`, the SQLite schema, and the upload
DTO (`BatchEventPayload`/`BatchPayload`) simply have no field that could
hold one.

## What is stored locally, and for how long

All persistence lives in a SQLite database at
`~/.velvt/velvt-service.sqlite3` (configurable via `VELVT_DATABASE_PATH`):

| Table | Contents | Default retention |
|---|---|---|
| `abstraction_map` | stable-key hash → stable ID, label, category, taxonomy version | indefinite (no raw content to expire) |
| `raw_event_buffer` | privacy-safe abstracted event metadata, used for short-lived audit/replay | 72 hours (`VELVT_RAW_EVENT_TTL_HOURS`) |
| `upload_batch` / `batch_event` | privacy-safe events grouped into upload batches | sent batches: 30 days; rejected batches: 7 days (audit window) |
| `history_cache` / `insight_cache` | ready-to-display summaries fetched from the cloud | minutes to tens of minutes, per `VELVT_HISTORY_TTL_SECONDS`/`VELVT_INSIGHT_TTL_SECONDS` |

Despite its name, `raw_event_buffer` never contains raw app names or window
titles — see [`PRIVACY_AUDIT.md`](PRIVACY_AUDIT.md) Audit 1 for the
verification. Auth and device-bound tokens are never stored in SQLite —
they live in the macOS Keychain only, via `KeychainTokenStore` (Rust) and
`KeychainService` (Swift).

## What is transmitted to the cloud, and in what form

Only the following ever leave the device, over HTTPS:

- Abstracted event batches (`POST /v1/events/batches`): `stable_id`,
  `label`, `category`, `taxonomy_version`, `occurred_at`,
  `duration_seconds` — never a raw app name, title, URL, path, or
  filename.
- Device registration and auth (`POST /v1/devices`, `/v1/auth/refresh`,
  `/v1/auth/devices/reissue`, `/v1/auth/signup`, `/v1/auth/login`,
  `/v1/auth/logout`, `/v1/auth/account/delete`): device and account
  credentials, never raw event content.
- History/insight fetch (`GET /v1/history/daily`, `/v1/insights/daily`):
  read-only requests for already-abstracted, server-side-derived summaries.

The cloud independently enforces this boundary and rejects any batch
containing a forbidden field with `raw_field_rejected`; the Rust service
treats that rejection as terminal for the offending batch (it is never
retried) and surfaces a `PrivacyViolationAlert` over IPC so the menu bar UI
can show it.

## What the abstraction engine does and does not preserve

**Preserves:** a stable per-app/title identity (so "the same kind of
activity" can be recognized across events), a coarse category
(`focus_work`, `communication`, `passive_consumption`, `system`,
`unclassified`, ...), a human-meaningless `label` like `document:edit`, and
timing.

**Does not preserve:** the literal application name, the literal window
title, any URL or file path that appeared in a title, or any way to
recover the original raw string from the stable ID (it is a one-way hash
into a local-only mapping table, not a reversible encoding).

## How to audit what is being collected

The SQLite database is a plain file at `~/.velvt/velvt-service.sqlite3`.
Open it with any SQLite browser (`sqlite3 ~/.velvt/velvt-service.sqlite3`)
and inspect the six tables listed above — every column is named in
`rust-service/migrations/`, and the migration SQL itself documents the "no
raw content" invariant inline. The full abstraction and upload code paths
are open source in this repository; `PRIVACY_AUDIT.md` is the line-by-line
verification a security reviewer would otherwise have to redo from
scratch.

## How to delete all local data

1. Quit Velvt.
2. Delete `~/.velvt/` (removes the SQLite database and any other local
   service state).
3. Remove the Keychain entries: open Keychain Access and delete the
   `com.velvt.service.auth` (Rust device/auth tokens) and `com.velvt.mac`
   (Swift session tokens) entries, or run
   `security delete-generic-password -s com.velvt.service.auth` /
   the equivalent for the Swift service name.
4. To also delete your cloud account and any data associated with it, use
   the in-app "Delete Account" action, which sends `delete_account` over
   IPC and the Rust service relays it to the cloud's account-deletion
   endpoint.

## The open-source auditability guarantee

Every line of code that touches a raw event — from the Accessibility
callback in `swift-client/Sources/VelvtMac/Collection/` through the
abstraction engine in `rust-service/src/abstraction/` to the upload DTO in
`rust-service/src/upload/dto.rs` — is in this repository under
[`LICENSE`](LICENSE). There is no closed-source or server-side-only
component standing between your raw activity and the abstraction boundary;
anyone can read, build, and run this exact code to verify the claims in
this document.
