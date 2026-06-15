# IPC Protocol Changelog

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
