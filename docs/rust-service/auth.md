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
  -> AccountDeleted
```

`DeviceRevoked` and `AccountDeleted` are terminal: no transition leads from
either to `Authenticated`. A sign-in from either clears the session and the
stored device id first, then registers a new device. `AccountDeleted` is left
by the service itself once it has destroyed what it queued for the account
(`ipc::end_deleted_account_session`).

Normal login flow:

```text
Swift sign_up/log_in IPC
AccountAuthService credential request
cloud returns user tokens
Rust registers or reissues device tokens
Rust stores device tokens and the account id in TokenStore
Rust stores user tokens in TokenStore for device reissue recovery
Rust transitions to Authenticated { device_id }
Rust sends auth_success to Swift
Swift stores host session in Keychain
```

A registration that fails (transport, a refusal, a response without a device)
answers `auth_failure` and keeps nothing; `auth_success` is sent only once a
device exists to upload as. A sign-up's failure says the account was created
and asks the person to log in, because signing up again would be refused as an
email already registered.

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
401 account_deleted -> AccountDeleted -> destroy the queue, clear the session,
  push needs_reauth { reason: account_deleted } to Swift
```

Only an error code the cloud attributes to the session ends it. A 403 with no
code is what Cloudflare's WAF, Bot Fight Mode or a challenge answers with; it,
a 5xx and a 429 are returned to the caller as a status (or `Transport` /
`RateLimited` from a refresh or reissue), the caller backs off, and the session
and Keychain stay. One such 403 used to sign the tester out and wipe the
Keychain (critic-04).

## Token Storage

The Rust service must not store auth tokens in SQLite. The current implementation uses `VolatileTokenStore`, which keeps device access/refresh tokens and optional user access/refresh tokens in memory and emits `auth_session_updated` IPC messages when tokens change.

Swift receives those messages and persists the session in Keychain. On reconnect or relaunch, Swift sends `auth_session` back to Rust so the service can resume authenticated cloud work.

The store also holds the id of the account the session belongs to, from
`auth_success` or from `auth_session.user_id` (protocol 34), and, for a session
handed over without one, from the answer to `GET /v1/auth/session`. The upload
queue is stamped with it: `upload::SessionOwnership` sends a batch only while
its account is signed in, holds every batch while no session is usable, and
discards one recorded under another account once that account has signed in.

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

If a device ID already exists, the service reissues device-bound tokens for that device using a valid user-bound access token, refreshing the user token first when needed. This keeps the cloud upload/fetch path using device-scoped credentials rather than long-lived user credentials. A reissue refused with `404` (the device is another account's) or `403 device_inactive` / `device_revoked` (the device was removed) clears the stored id and registers a new device.

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
| `401 invalid_credentials`, `invalid_token`, `token_expired` or `missing_token` | Refresh once when possible; otherwise enter `NeedsReauth` |
| `401`/`403 device_token_revoked` | Try device-token reissue; enter `NeedsReauth` if reissue is refused with a session code |
| `403 device_revoked`, or `403 device_inactive` from reissue | Enter `DeviceRevoked` |
| `401 account_deleted` | Enter `AccountDeleted` |
| `429` | Return rate-limited error; the session stays |
| Anything else, including `403` without a code and `5xx` | Return the response (or `Transport` from a refresh or reissue); the session stays, and a refresh is not retried for a minute |

## UI Notification of Auth State

`src/main.rs` subscribes to the Rust auth state machine. When state becomes `DeviceRevoked` or `NeedsReauth`, it pushes the corresponding IPC message through `PushAdapter`; when it becomes `AccountDeleted`, it runs `ipc::end_deleted_account_session`, which pushes `needs_reauth` with the reason `account_deleted`. When a session becomes usable, the retry loop makes due every batch put off as `authentication_required` and resumes the queue, and the menu's readiness probe runs again. This is independent of the request that caused the state change, which keeps the menu bar UI accurate even if the user is not actively using an auth screen.

## Tests to Update

When changing auth behavior, inspect and update:

- `rust-service/tests/auth_flow.rs`
- `rust-service/tests/auth_state.rs`
- `rust-service/tests/device_registrar.rs`
- `rust-service/shared-types/src/lib.rs` DTO tests for auth message JSON shape
- Swift auth tests if IPC-visible behavior changes
