# IPC Protocol Changelog

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
