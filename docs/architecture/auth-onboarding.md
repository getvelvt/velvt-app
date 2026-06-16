# Auth & Onboarding Architecture (S5)

## 1. Ownership and constraints

All authentication operations are performed by the Rust service. The Swift
client:

- sends `sign_up`, `log_in`, `log_out`, `delete_account` via IPC
- stores the resulting tokens in the macOS Keychain (never in SQLite or
  UserDefaults)
- receives `auth_success`, `auth_failure`, `account_deletion_accepted`,
  `needs_reauth`, and `device_revoked` server pushes via the same IPC channel

**No `URLSession` calls, no direct HTTP.** The network boundary is the Rust
service.

**Tokens must never appear in `os_log`, `Logger`, `print`, or string
interpolation** outside of `KeychainService` internals. The only log messages
that mention auth context are state-machine transition rejections (which log
enum case names, not values).

---

## 2. AccountState machine

```
                      ┌───────────────────────────────────────┐
                      │               loggedOut                │◄──┐
                      └───────────────┬───────────────────────┘   │
                                      │ signUp() / logIn()         │ authFailure /
                                      ▼                            │ cancelPendingErasure /
                      ┌───────────────────────────────────────┐   │ stream end /
                      │              loggingIn                 │   │ needs_reauth /
                      └──┬────────────────────────────────────┘   │ device_revoked
                         │ authSuccess                             │
                         ▼                                         │
                      ┌───────────────────────────────────────┐   │
              ┌──────►│           loggedIn(userId:)            │───┘ logOut()
              │        └──────────────────┬────────────────────┘
              │   cancelPendingErasure()   │               │
              │ (send failed)              │ deleteAccount │
              │                           ▼               │ logOut()
              │        ┌───────────────────────────────────────┐
              │        │           pendingErasure               │──► loggedOut
              └────────┴───────────────────────────────────────┘
                         (account_deletion_accepted → loggedOut)
```

`AccountStateManager.transition(to:)` validates every move. Invalid transitions
are rejected with a structured `os_log` entry and are never a crash.

### Valid transitions

| From          | To              | Trigger                                  |
|---------------|-----------------|------------------------------------------|
| loggedOut     | loggingIn       | `signUp()` / `logIn()` in AuthViewModel  |
| loggingIn     | loggedIn        | `auth_success` IPC push                  |
| loggingIn     | loggedOut       | `auth_failure` / `needs_reauth` / stream end |
| loggedIn      | loggingOut      | `logOut()` in AuthViewModel              |
| loggingOut    | loggedOut       | `logOut()` completes / stream end        |
| loggedIn      | pendingErasure  | `confirmAccountDeletion()` in AuthViewModel |
| pendingErasure| loggedOut       | `account_deletion_accepted` IPC push     |

### pendingErasure persistence

`pendingErasure` is written to Keychain (`velvt.pending_deletion`) when entered
so that a relaunch mid-deletion restores the correct blocking state. The
sentinel is cleared by:

- `account_deletion_accepted` (via `deleteAll()`)
- `cancelPendingErasure()` (IPC send failed — user can retry)
- `logOut()` (via `deleteAll()`)

On relaunch with `userId` + `pendingDeletion` in Keychain, `AccountStateManager`
restores `.pendingErasure`, and `PermissionRootView` shows `PendingDeletionView`
instead of normal content.

---

## 3. Keychain keys

All three token keys and the deletion sentinel live under the
`com.velvt.mac` Keychain service.

| `KeychainKey` case   | Keychain account name       | Holds                                 |
|----------------------|-----------------------------|---------------------------------------|
| `.accessToken`       | `velvt.access_token`        | Bearer token for API calls (Rust)     |
| `.refreshToken`      | `velvt.refresh_token`       | Long-lived refresh credential (Rust)  |
| `.userId`            | `velvt.user_id`             | Stable user identifier                |
| `.pendingDeletion`   | `velvt.pending_deletion`    | Sentinel `"1"` while erasure is in flight |

### Token rotation

Token refresh is handled entirely by the Rust service. When a new
`auth_success` push arrives (e.g. after a silent refresh), `AccountStateManager`
overwrites the three token keys atomically inside `handleAuthSuccess(_:)`.
The Swift client never initiates a refresh.

### Clearing tokens

`KeychainProtocol.deleteAll()` is called on:
- explicit `logOut()` — transitions to `.loggedOut`
- `account_deletion_accepted` — final erasure confirmation
- `needs_reauth` — server revoked the session
- `device_revoked` — this device's access was terminated

---

## 4. Onboarding flow

```
PermissionRootView
    │
    ├─ isDeviceRevoked ────────────────────► DeviceRevokedView
    │                                        (non-dismissible; "Sign In Again"
    │                                         clears the flag)
    │
    ├─ accountState == .pendingErasure ────► PendingDeletionView
    │                                        (blocks normal use; "Cancel" calls
    │                                         cancelPendingErasure())
    │
    ├─ showsOnboarding || !loggedIn ──────► OnboardingContainer
    │                                        │
    │                                        └─ OnboardingFlowView (NavigationStack)
    │                                              │
    │                                              ├─ WelcomeView (root)
    │                                              │      ▼ advanceFromWelcome()
    │                                              ├─ PermissionsStepView
    │                                              │   (reuses S3 PermissionOnboardingModel)
    │                                              │      ▼ advanceFromPermissions()
    │                                              ├─ AuthStepView
    │                                              │   (email/password, mode toggle,
    │                                              │    observes accountState for loggedIn)
    │                                              │      ▼ authDidComplete()
    │                                              └─ OnboardingCompleteView
    │                                                     ▼ finishOnboarding()
    │                                                   onComplete() → completeOnboarding()
    │
    └─ loggedIn + onboarding done ────────► "Velvt is available from the menu bar."
```

**Auth is never bypassable when logged out.** `AuthStepView` is only reachable
by following the `NavigationStack` path through `WelcomeView` and
`PermissionsStepView`, or via `skipToAuth()` which is only called from
`PermissionRootView` when onboarding was previously completed but the session
has since expired.

**`needs_reauth` re-auth path:**  
`AccountStateManager` clears tokens and transitions to `.loggedOut`.
`PermissionRootView` detects `!loggedIn && !showsOnboarding` and renders
`OnboardingContainer(skipToAuth: true)`, landing the user directly on
`AuthStepView` without repeating the permissions step.

---

## 5. IPC disconnect handling

When `AccountStateManager`'s listener task detects that the `incomingMessages`
stream has ended unexpectedly (i.e. the task was not cancelled):

- `.loggingIn` → `.loggedOut` — prevents the UI from being stuck on a loading
  spinner indefinitely.
- `.loggingOut` → `.loggedOut` — session is considered terminated.
- `.pendingErasure` — **not** reverted. The Rust service may have received the
  deletion request. The sentinel persists until the next connection confirms
  or cancels the erasure.

`AuthViewModel` detects the unexpected revert via its `$accountState` Combine
subscription: if `isLoading` is true and the state goes to `.loggedOut` without
a prior `auth_failure` error message, it sets `errorMessage = "Connection lost.
Please try again."`.

---

## 6. How to add a new auth action

Follow this checklist when adding a new authenticated operation (e.g.
`changePassword`, `addDevice`):

### 6.1 Proto schema

1. Create `proto/schema/<action_name>.json` with `"type": "<action_name>"`,
   `"payload"` fields, `additionalProperties: false`, and `$schema: draft-07`.
2. If the server sends a response, create the matching response schema.
3. Update `proto/CHANGELOG.md` and bump `proto/version` (breaking change if
   removing or renaming fields; otherwise minor).

### 6.2 IPC types (`Sources/VelvtMac/IPC/IPCTypes.swift`)

4. Add the new case to `ClientMessage` (and/or `ServerMessage`).
5. Add the corresponding DTO struct (`Codable`, `Equatable`, `Sendable`,
   `CodingKeys` for snake_case where needed).
6. Add the `type` string to `ClientMessage.init(from:)` and
   `ClientMessage.encode(to:)` (both are exhaustive switches that the compiler
   will flag if you forget).
7. For server messages, add to `ServerMessage.init(from:)` before the `default`
   case. `ServerMessage` is forward-compatible — unknown types fall through to
   `.unknown(type:)`.

### 6.3 Auth module (`Sources/VelvtMac/Auth/AuthModule.swift`)

8. If the operation requires a new `AccountState` transition, add the cases to
   `AccountState` and update `isValidTransition(from:to:)`.
9. If the server responds with a new `ServerMessage` case, add a handler in
   `AccountStateManager.handle(_:)`.

### 6.4 AuthViewModel (`Sources/VelvtMac/Auth/AuthViewModel.swift`)

10. Add a public `async` method that validates inputs, transitions state, and
    sends the IPC message. Follow the `signUp()` pattern:
    - guard against empty required fields before calling `startLoading()`
    - wrap `ipcClient.send()` in a `do/catch` and revert state on failure
    - let `AccountStateManager.serverMessages` deliver the server response
      asynchronously

### 6.5 Tests

11. Add round-trip tests in `AuthIPCContractTests` (discriminator + payload
    keys).
12. Add state-machine tests in `AccountStateManagerTests` (valid path, failure
    path, server-push handling).
13. Add view-model tests in `AuthViewModelTests` (success, field validation,
    IPC send failure, connection-lost revert).

### 6.6 Security checklist

- [ ] No `URLSession` or direct HTTP
- [ ] Credentials / tokens never in `os_log`, `Logger`, `print`
- [ ] Any new tokens stored via `KeychainProtocol`, not `UserDefaults`
- [ ] New `AccountState` cases cannot be reached from invalid prior states
- [ ] `FakeKeychain` / `FakeIPCClient` cover the new paths in tests without
      Keychain entitlements or a live IPC connection
