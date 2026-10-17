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
device exists to upload as. That includes a sign-in that reissues a stored
device id and falls through to registering: the new account's user tokens are
stored only once the reissue or the registration succeeds. Swift refuses an
`auth_success` with an empty device id and sends `log_out`, so the service
drops whatever it stored. A sign-up's failure says the account was created
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
Keychain (critic-04). A refresh, a reissue, or the user-token refresh a reissue
needs, answered that way or not answered at all, is not tried again for a
minute: requests in that minute use the unexpired token in hand, and one whose
token has expired, or that is answered `device_token_revoked`, fails at once
with `Transport`.

Every authenticated request is bound to an account: the one it names
(`HttpRequest::account`; `HttpBatchUploader` names the batch's owner), else the
one signed in when it starts. `AuthManager` checks the account after it reads
the tokens it is about to send with, and again before a refresh-and-retry or a
reissue-and-retry, and fails the request as `NeedsReauth`, changing no state,
when the account has changed. A batch the ownership policy judged account A's,
waiting on the refresh lock while A logged out and B signed in, was otherwise
POSTed under B's token. The check holds because every path that puts another
account's session in place (`AccountAuthService::store_session`) clears the old
account id before the tokens change and stores the new one after them. A
session handed over with no account id binds nothing until
`GET /v1/auth/session` names it.

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
| `401`/`403 device_token_revoked` | Try device-token reissue; enter `NeedsReauth` if reissue is refused with a session code. Requests refused together share one reissue: one that finds the stored tokens already replaced by a reissue since it read them retries with those instead of sending another |
| `403 device_revoked`, or `403 device_inactive` from reissue | Enter `DeviceRevoked` |
| `401 account_deleted` | Enter `AccountDeleted` |
| `429` | Return `AuthError::RateLimited { retry_after }`, carrying the response's `Retry-After`; the session stays. The upload queue and the insight poll wait that long (protocol 37; before, the header was dropped here) |
| Anything else, including `403` without a code and `5xx` | Return the response (or `Transport` from a refresh or reissue); the session stays, and a refresh or reissue is not retried for a minute |

A sign-up or login answered `429` answers `auth_failure` with `rate_limited` (protocol 37) and Rust's words for the wait, from `Retry-After` rounded up to the minute: "Too many attempts. Try again in 15 minutes." ("in a minute" under one, in hours from 120 minutes), or "Too many attempts. Try again later." when the server sent no usable `Retry-After`, as the server deployed before velvt-core #41 does (`rate_limited_message` in `auth/account.rs`). The server's own message is not used, so the words are the same from either server. A `429` from the device registration that completes a sign-in (`POST /v1/devices`, which #41 limits to 10 an hour per user) or from the reissue of the stored device answers `rate_limited` the same way (`registration_rate_limited`): "This Mac could not be registered yet. Too many attempts. Try again in 60 minutes.", or after a sign-up "Your account was created, but this Mac could not be registered yet. Too many attempts. Log in to try again in 60 minutes." It used to be `server_error`, "This Mac could not be registered. Please try again.", which sent the person straight back into the limit.

## Client Version Report

Registration records the app version once. Once per launch, after a restored session is validated, the router's background task spawns a task of its own that calls `SessionValidator::report_client_version` (so a report the server is slow to answer never drops the next session's validation), and `AuthManager` sends `PATCH /v1/devices/{device_id}` with `{"client_version": <release version>}` (velvt-core #41, install-release-09). Any answer ends it for the launch: `2xx` (recorded), or any other status, including the `404` or `405` of a server without the route, which is not an error and changes no state. No answer (no usable session, a transport failure, `429`, `5xx`) leaves it for the next restored session. It never holds the IPC connection, and nothing new leaves the Mac: the version is the one registration, every batch and the `User-Agent` already carry.

A sign-up or login answered `422 validation_error` on the email or the password (`details[].loc` ends in `email` or `password`) answers `auth_failure` with `invalid_credentials` and Rust's own words: "Enter a valid email address.", or for the password, by `details[].type`, "Use at least 12 characters for your password.", "Use at most 256 characters for your password." or "Use a password of 12 to 256 characters." (`credential_validation_message` in `auth/account.rs`). velvt-core's own message, "Request validation failed.", named neither the field nor the rule. Nothing else in a detail is read; velvt-core echoes the submitted value in `details[].input`. A validation error on any other field keeps the server's message.

## UI Notification of Auth State

`src/main.rs` subscribes to the Rust auth state machine. When state becomes `DeviceRevoked` or `NeedsReauth`, it pushes the corresponding IPC message through `PushAdapter`; when it becomes `AccountDeleted`, `ipc::SessionWatch` runs `ipc::end_deleted_account_session`, which pushes `needs_reauth` with the reason `account_deleted`. When a session becomes usable, the retry loop makes due every batch put off as `authentication_required` and resumes the queue, and `SessionWatch` runs the menu's readiness probe again. This is independent of the request that caused the state change, which keeps the menu bar UI accurate even if the user is not actively using an auth screen.

## Tests to Update

When changing auth behavior, inspect and update:

- `rust-service/tests/auth_flow.rs`
- `rust-service/tests/account_switch.rs` (whose session a queued batch or a waiting request is sent under)
- `rust-service/tests/auth_state.rs`
- `rust-service/tests/device_registrar.rs`
- `rust-service/shared-types/src/lib.rs` DTO tests for auth message JSON shape
- Swift auth tests if IPC-visible behavior changes
