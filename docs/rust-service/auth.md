# Rust Service Authentication

Rust owns cloud authentication behavior for the local service. Swift owns the user-facing UI and persists a copy of the host session in Keychain, but Swift does not call cloud auth endpoints directly.

## Components

| Component | File | Responsibility |
|---|---|---|
| `AccountAuthService` | `src/auth/account.rs` | Handles `sign_up`, `log_in`, `log_out`, `delete_account`, first device registration, and device-token reissue after user auth |
| `AuthManager` | `src/auth/manager.rs` | Wraps authenticated HTTP requests, refreshes tokens, handles 401/403/429 responses |
| `AuthStateMachine` | `src/auth/state.rs` | Tracks unauthenticated, authenticated, refresh, reauth, and revoked states |
| `TokenStore` | `src/auth/store.rs` | Trait for token/device storage |
| `VolatileTokenStore` | `src/auth/tokens.rs` | Current process-local token store with session-update push support |
| `HttpClient` | `src/auth/http.rs` | Testable abstraction over outbound HTTP |

## State Machine

```text
Unauthenticated
  -> Authenticated { device_id }
  -> RefreshInFlight
  -> NeedsReauth
  -> DeviceRevoked
```

Normal login flow:

```text
Swift sign_up/log_in IPC
AccountAuthService credential request
cloud returns user tokens
Rust registers or reissues device tokens
Rust stores device tokens in TokenStore
Rust stores user tokens in TokenStore for device reissue recovery
Rust transitions to Authenticated { device_id }
Rust sends auth_success to Swift
Swift stores host session in Keychain
```

Refresh flow:

```text
Authenticated request
tokens near expiry or 401 token error
AuthManager enters RefreshInFlight
POST /v1/auth/refresh
store fresh tokens
return to Authenticated
retry original request when appropriate
```

Terminal/device flow:

```text
403 device_revoked -> DeviceRevoked -> push device_revoked to Swift
403 device_token_revoked -> try device token reissue -> Authenticated or NeedsReauth
invalid/expired token that cannot refresh -> NeedsReauth -> push needs_reauth to Swift
```

## Token Storage

The Rust service must not store auth tokens in SQLite. The current implementation uses `VolatileTokenStore`, which keeps device access/refresh tokens and optional user access/refresh tokens in memory and emits `auth_session_updated` IPC messages when tokens change.

Swift receives those messages and persists the session in Keychain. On reconnect or relaunch, Swift sends `auth_session` back to Rust so the service can resume authenticated cloud work.

Authenticated API calls use the device token pair. User tokens are retained only so Rust can refresh user auth and call `/v1/auth/devices/reissue` if the device token pair is revoked after relaunch. If neither device refresh nor user-backed device reissue succeeds, Rust enters `NeedsReauth` and Swift clears Keychain.

This arrangement keeps the local helper stateless enough to restart and avoids placing secrets in the service database.

## Credentials

Credentials enter Rust only through local IPC:

```json
{
  "type": "log_in",
  "payload": {
    "email": "person@example.com",
    "password": "not logged"
  }
}
```

`SignUp`, `LogIn`, and token-bearing DTOs require redaction discipline. Logs may include message type and safe error codes, but not credentials, tokens, or raw event content.

## Device Registration

Device registration cannot happen at service startup because `/v1/devices` requires a logged-in user's access token. `AccountAuthService` therefore registers the device after the first successful sign-up or login if no device ID is already stored.

If a device ID already exists, the service reissues device-bound tokens for that device using a valid user-bound access token, refreshing the user token first when needed. This keeps the cloud upload/fetch path using device-scoped credentials rather than long-lived user credentials.

## Auth-Related IPC Messages

Swift to Rust:

- `sign_up`
- `log_in`
- `auth_session`
- `log_out`
- `delete_account`

Rust to Swift:

- `auth_success`
- `auth_failure`
- `auth_session_updated`
- `account_deletion_accepted`
- `needs_reauth`
- `device_revoked`

`log_out` is fire-and-forget from Swift's perspective. Swift clears Keychain immediately; Rust makes a best-effort cloud logout.

## Response Handling Rules

`AuthManager` treats responses as follows:

| Response | Behavior |
|---|---|
| `200` refresh/reissue with tokens | Store tokens and return to authenticated state |
| `401 invalid_credentials`, `invalid_token`, or `token_expired` | Refresh once when possible; otherwise enter `NeedsReauth` |
| `403 device_token_revoked` | Try device-token reissue; enter `NeedsReauth` if reissue fails |
| `403 device_revoked` | Enter `DeviceRevoked` |
| `429` | Return rate-limited error without logging sensitive payloads |

## UI Notification of Auth State

`src/main.rs` subscribes to the Rust auth state machine. When state becomes `DeviceRevoked` or `NeedsReauth`, it pushes the corresponding IPC message through `PushAdapter`. This is independent of the request that caused the state change, which keeps the menu bar UI accurate even if the user is not actively using an auth screen.

## Tests to Update

When changing auth behavior, inspect and update:

- `rust-service/tests/auth_flow.rs`
- `rust-service/tests/auth_state.rs`
- `rust-service/tests/device_registrar.rs`
- `rust-service/shared-types/src/lib.rs` DTO tests for auth message JSON shape
- Swift auth tests if IPC-visible behavior changes
