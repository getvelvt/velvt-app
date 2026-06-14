# Velvt Rust Service

The Rust service owns authentication, the final privacy boundary, SQLite
persistence, abstraction, cloud requests, and IPC delivery to the macOS client.

## Authentication Privacy

Access and refresh tokens are stored together as one record in macOS Keychain
through `KeychainTokenStore`. They must never be stored in SQLite, plaintext
files, environment variables, logs, tracing fields, or error messages.

All token-carrying fields use `RedactedString`. Its `Debug` and `Display`
implementations emit only `[redacted]`. The underlying value is exposed only
inside the private Keychain serializer and concrete HTTP authorization/body
construction. `RedactedString` intentionally does not implement `Serialize`,
preventing unrelated code from serializing tokens.

Tests use `FakeTokenStore`; they never access the real Keychain.

## Auth State Machine

Auth state changes only through `AuthStateMachine::transition`.

```text
Unauthenticated
    |
    | tokens supplied
    v
Authenticated { device_id }
    |             |                 |
    | expiry      | invalid token   | device_revoked
    v             v                 v
RefreshInFlight  NeedsReauth    DeviceRevoked
    |   |   |
    |   |   +-- device_revoked / exhausted reissue --> DeviceRevoked
    |   +------ invalid credentials / invalid response --> NeedsReauth
    +---------- refresh success or transient transport failure --> Authenticated

NeedsReauth -- tokens supplied --> Authenticated
NeedsReauth -- sign out -------> Unauthenticated
DeviceRevoked is terminal until a future onboarding recovery flow.
```

Before every authenticated request, `AuthManager` checks token expiry against
the configured refresh buffer. Refresh is single-flight: concurrent callers
wait for the active refresh and reuse its atomically replaced token pair.
Transient transport failures preserve the existing Keychain record and retry
on the next request cycle. Invalid credentials transition to `NeedsReauth`.

`device_token_revoked` attempts `/v1/auth/devices/reissue` once before
transitioning to `DeviceRevoked`. `device_revoked`, `device_not_found`, failed
reissue, or repeated revocation transitions to `DeviceRevoked`; subsequent
authenticated upload attempts are rejected before reaching HTTP.

## Device Registration Seam

Device registration depends only on `DeviceRegistrar::register()`.
`NoOpDeviceRegistrar` is the production placeholder until the onboarding issue
wires the concrete registrar. Tests can replace it with a recording fake only
at the composition/wiring site without changing consumers.
