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

## Upload Batching

`upload::BatchAssembler` creates deterministic batches from privacy-safe
`BatchEventPayload` values. Count and age thresholds come from
`VELVT_UPLOAD_BATCH_EVENT_LIMIT` and `VELVT_UPLOAD_FLUSH_SECONDS`.
`VELVT_UPLOAD_RETRY_SCAN_SECONDS` controls how often persisted due batches are
checked, and `VELVT_API_BASE_URL` selects the cloud API host.

Batch IDs are SHA-256 digests over the domain separator
`velvt:upload-batch:v1`, the device ID, and the ordered local event IDs.
Recreating the same logical batch after a crash therefore produces the same
ID. The assembler flushes once when either the configured count or age
threshold is reached, and exposes immediate sleep/shutdown flushes.

`UploadCoordinator` persists a batch before sending it, resumes pending or
failed batches after restart, and treats duplicate responses as success.
Rate-limit retry state is host-scoped and respects server-provided
`Retry-After` delays. Without that header it uses exponential backoff starting
at 30 seconds, capped at 15 minutes, with production jitter clamped to +/-10%.
A `raw_field_rejected` response
permanently rejects the batch, emits a structured error, and broadcasts
`privacy_violation_alert` to Swift.

`BatchRetentionPolicy` is the R8 integration seam. R5 defaults to
`KeepAllBatches`; a future retention policy can discard expired batches before
the uploader is invoked without changing transport or retry logic.

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
