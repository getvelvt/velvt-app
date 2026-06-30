# Rust Service APIs and Interfaces

The Rust service exposes two categories of interfaces:

1. Local IPC over a Unix domain socket for the Swift client.
2. Outbound HTTP calls to the Velvt cloud API.

The local IPC API is the stable contract inside this monorepo. The cloud HTTP API is consumed by Rust only; Swift does not call cloud endpoints directly.

## Local IPC Transport

IPC uses newline-delimited JSON over the Unix socket path in:

```text
proto/ipc_socket_path
```

Each frame is one JSON object followed by a newline:

```json
{"type":"server_hello","payload":{"protocol_version":11}}
```

All protocol-v3+ messages use this envelope shape:

```json
{
  "type": "message_discriminator",
  "payload": {}
}
```

The current protocol constant is `PROTOCOL_VERSION` in `rust-service/shared-types/src/lib.rs` and must match `proto/version`.

## Handshake

Server starts every connection:

```json
{
  "type": "server_hello",
  "payload": {
    "protocol_version": 11
  }
}
```

Client responds:

```json
{
  "type": "client_hello",
  "payload": {
    "expected_protocol_version": 11,
    "client_version": "0.1.0"
  }
}
```

Server accepts:

```json
{
  "type": "acknowledged",
  "payload": {}
}
```

Or rejects:

```json
{
  "type": "version_mismatch",
  "payload": {
    "server_protocol_version": 11,
    "client_protocol_version": 10
  }
}
```

Swift must not send application messages before `acknowledged`.

## Client Messages

Rust accepts these `ClientMessage` cases:

| Type | Purpose |
|---|---|
| `client_hello` | Protocol negotiation response |
| `raw_event` | Raw local activity event from Swift |
| `request_latest_insight` | Request cached daily insight for a date |
| `request_latest_history` | Request cached history for recent days |
| `sign_up` | Relay account creation credentials |
| `log_in` | Relay login credentials |
| `auth_session` | Apply Swift-persisted session after reconnect/relaunch |
| `log_out` | Best-effort server-side logout notification |
| `delete_account` | Request permanent account deletion |
| `request_menu_status` | Request cloud/local queue status for settings UI |
| `flush_upload_queue` | Force pending upload flush |
| `error_response` | Typed client error envelope |

### Raw Event Example

```json
{
  "type": "raw_event",
  "payload": {
    "event_id": "018f4f5d-0000-7000-8000-000000000001",
    "occurred_at": "2026-06-29T14:15:00Z",
    "app_name": "Example App",
    "window_title": "Quarterly Plan",
    "bundle_id": "com.example.app"
  }
}
```

`app_name`, `window_title`, and `bundle_id` are local-only raw fields. They are valid on the Swift-to-Rust IPC boundary, but must not appear in upload payloads.

Rust replies:

```json
{
  "type": "raw_event_ack",
  "payload": {
    "event_id": "018f4f5d-0000-7000-8000-000000000001",
    "status": "accepted"
  }
}
```

Or:

```json
{
  "type": "raw_event_ack",
  "payload": {
    "event_id": "018f4f5d-0000-7000-8000-000000000001",
    "status": "dropped",
    "drop_reason": "abstraction_failed"
  }
}
```

Drop reasons must be stable safe codes, not raw content.

### Insight Request Example

```json
{
  "type": "request_latest_insight",
  "payload": {
    "date": "2026-06-29"
  }
}
```

Possible response:

```json
{
  "type": "insight_payload",
  "payload": {
    "date": "2026-06-29",
    "text": "Ready-to-display insight text from the service.",
    "confidence_level": "medium",
    "low_confidence": false,
    "generated_at": "2026-06-29T18:00:00Z"
  }
}
```

If no cache entry exists:

```json
{
  "type": "cache_empty",
  "payload": {
    "payload_type": "insight_payload"
  }
}
```

### Menu Status Example

Swift asks:

```json
{
  "type": "request_menu_status",
  "payload": {}
}
```

Rust responds:

```json
{
  "type": "menu_status",
  "payload": {
    "device_id": "dev_123",
    "cloud_ready": true,
    "queued_event_count": 1,
    "queued_events": [
      {
        "label": "document:edit",
        "local_label": "Local display title",
        "category": "document",
        "occurred_at": "2026-06-29T14:15:00Z"
      }
    ]
  }
}
```

`local_label` is device-local menu display data. It is sent over IPC to Swift and must not appear in cloud upload DTOs.

## Server Push Messages

Rust can send messages without a direct request when state changes:

| Type | Trigger |
|---|---|
| `service_status` | Service degraded, upload paused, auth required, or ready |
| `privacy_violation_alert` | Cloud rejected an upload for raw-field exposure |
| `auth_session_updated` | Tokens changed and Swift should update Keychain |
| `needs_reauth` | Session can no longer refresh |
| `device_revoked` | Device is revoked and upload/fetch must stop |
| `notification_payload` | Fresh daily insight should be scheduled as a notification |
| `shutting_down` | Service is terminating gracefully |

Push messages are queued through `PushAdapter` and `ReconnectTracker` so short Swift disconnects do not immediately lose important state transitions.

## Malformed Frames

Invalid client frames receive:

```json
{
  "type": "malformed_message",
  "payload": {
    "code": "invalid_message"
  }
}
```

The service does not echo the invalid payload. After `VELVT_IPC_MAX_ERRORS` malformed frames, transport closes the connection.

## Outbound Cloud HTTP Interfaces

Rust uses an internal `HttpClient` trait and `ReqwestHttpClient` implementation. Requests include:

| Endpoint | Method | Used By | Purpose |
|---|---|---|---|
| `/v1/auth/signup` | POST | `AccountAuthService` | Create account |
| `/v1/auth/login` | POST | `AccountAuthService` | Login |
| `/v1/devices` | POST | `AccountAuthService` | Register current device after first auth |
| `/v1/auth/refresh` | POST | `AuthManager` | Refresh expiring device tokens |
| `/v1/auth/devices/reissue` | POST | `AuthManager` / `AccountAuthService` | Reissue device-bound tokens |
| `/v1/auth/logout` | POST | `AccountAuthService` | Best-effort logout |
| `/v1/account` | DELETE | `AccountAuthService` | Account deletion |
| `/v1/ready` | GET | `MenuStatusProvider` | Cloud readiness indicator |
| `/v1/events/batches` | POST | `HttpBatchUploader` | Upload abstracted event batches |
| History/insight endpoints | GET | `FetchService` | Fetch ready-to-display summaries |

Cloud DTOs must be audited against the forbidden-field list. Tests in `rust-service/tests/upload_batching.rs` are the first place to update when upload shape changes.

## Adding or Changing API Surface

For local IPC:

1. Update `proto/schema/`.
2. Bump `proto/version` when compatibility requires it.
3. Update `rust-service/shared-types/src/lib.rs`.
4. Update `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`.
5. Add exact JSON shape tests.
6. Update this document and any affected Swift/Rust architecture docs.

For cloud HTTP:

1. Keep the call inside Rust.
2. Add or update request/response DTO tests.
3. Confirm token/credential redaction.
4. Confirm no raw event fields can enter the request body.
