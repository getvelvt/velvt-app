# Swift Client Authentication

Swift owns the auth UI and local session persistence. Rust owns cloud auth requests, device registration, token refresh, account deletion, and revocation handling.

## Components

| Component | File | Responsibility |
|---|---|---|
| `AuthViewModel` | `Auth/AuthViewModel.swift` | Drives sign-up, login, logout, and account deletion UI actions |
| `AccountStateManager` | `Auth/AuthModule.swift` | Owns account state, Keychain storage, IPC auth message handling, and message fan-out |
| `KeychainService` | `Auth/AuthModule.swift` | Stores session fields in macOS Keychain |
| `MenuBarAccountControls` | `UI/MenuBarPopoverView.swift` | Presents sign in, sign up, logout, and auth sheet entry points |

## Account States

```swift
public enum AccountState: Equatable, Sendable {
    case loggedOut
    case loggingIn
    case loggedIn(userId: String)
    case loggingOut
    case pendingErasure
}
```

Only valid transitions are accepted through `transition(to:)`. Invalid transitions are logged as safe state names and rejected instead of crashing.

## Sign-Up and Login Flow

1. User enters email and password in the menu bar auth sheet.
2. `AuthViewModel.signUp()` or `logIn()` validates non-empty fields.
3. `AccountStateManager.beginAuthentication(email:)` moves state to `.loggingIn` and temporarily holds the email.
4. Swift sends `.signUp` or `.logIn` over IPC.
5. Rust performs cloud auth and device registration.
6. Rust returns `auth_success` or `auth_failure`.
7. `AccountStateManager` handles the message from its single IPC listener.
8. On success, Swift stores device tokens, user tokens when provided, user ID, device ID, expiries, and email in Keychain, then moves to `.loggedIn`.
9. On failure, Swift returns to `.loggedOut` and `AuthViewModel` surfaces the error message.

Swift does not make any HTTP request in this flow.

## Keychain Storage

Current sessions are encoded into one `velvt.auth_snapshot` Keychain item:

| Snapshot field | Purpose |
|---|---|
| `session` | Device tokens, optional user tokens, their expiries, and the registered device ID |
| `userId` | Current user ID |
| `email` | Local account display email |
| `pendingDeletion` | Relaunch-safe sentinel while account deletion is pending |

Tokens must never be logged or stored outside Keychain.

`AccountStateManager` reads that item once during initialization, then serves
account state, session replay, Settings/App Info email display, and subsequent
session-update persistence from its in-memory snapshot. An unchanged
`auth_session_updated` echo is ignored, so reconnecting to Rust does not trigger
another Keychain authorization prompt or redundant write.

## Session Replay

`AccountStateManager` loads a cached `AuthSession` from Keychain during initialization when all required device-session fields exist. User-token fields are replayed only when the user access token, refresh token, and expiry are all present.

When `IPCClientProtocol.connectionStatus` becomes `.connected`, it sends:

```swift
try await client.send(.authSession(session))
```

This lets a restarted Rust service resume authenticated upload/fetch work using the persisted device session. If the device token is expired, Rust refreshes it with the device refresh token. If the device token has been revoked, Rust uses the persisted user session to refresh user auth if needed and then reissue device-bound credentials. Swift clears Keychain and returns to login only after Rust reports that both recovery paths failed.

## Auth Message Handling

`AccountStateManager` is the sole consumer of `incomingMessages`. It republishes every message to `serverMessages` before handling auth-specific transitions.

Handled server messages:

| Message | Swift behavior |
|---|---|
| `auth_success` | Store tokens/session in Keychain and transition to `.loggedIn` |
| `auth_session_updated` | Update Keychain tokens after Rust refresh/reissue |
| `auth_failure` | Return from `.loggingIn` to `.loggedOut` |
| `account_deletion_accepted` | Clear Keychain and transition to `.loggedOut` |
| `needs_reauth` | Clear Keychain and transition to `.loggedOut` |
| `device_revoked` | Clear Keychain, transition to `.loggedOut`, set `isDeviceRevoked` |

## Logout

Logout is local-first:

1. `AuthViewModel.logOut()` calls `AccountStateManager.logOut()`.
2. Swift clears all Keychain entries and sets state to `.loggedOut`.
3. Swift sends `.logOut` over IPC as best effort.
4. Rust tries to revoke/clear server-side session state.

The user is logged out locally even if the IPC send fails.

## Account Deletion

Account deletion uses `.pendingErasure` to avoid losing state across relaunch:

1. User confirms deletion.
2. Swift transitions to `.pendingErasure` and updates `pendingDeletion` in the auth snapshot.
3. Swift sends `.deleteAccount` over IPC.
4. If sending fails, Swift calls `cancelPendingErasure()` and returns to `.loggedIn` when a user ID is still available.
5. If Rust returns `account_deletion_accepted`, Swift clears Keychain and returns to `.loggedOut`.

## Privacy and Logging

Swift auth logs must not include tokens, passwords, raw event content, URLs, paths, or contacts. Current logs include state names and error descriptions. Any new token- or credential-bearing DTO must be reviewed for accidental string interpolation.

## Tests to Update

When changing Swift auth behavior, inspect:

- `swift-client/Tests/VelvtMacTests/AuthModuleTests.swift`
- `swift-client/Tests/VelvtMacTests/AuthViewModel` coverage if added or nearby
- `swift-client/Tests/VelvtMacTests/MenuBarAccountActionResolverTests.swift`
- IPC DTO tests for auth message shape
- Rust auth tests if message behavior or state semantics change
