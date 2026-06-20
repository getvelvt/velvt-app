# Menu Bar Data and Upload Status

## Scope

Repair the authenticated menu-bar experience so it requests and renders the
available daily history, shows a clear empty state when an insight has not yet
been generated, exposes contextual account actions, and reports the number of
locally queued events that have not been acknowledged by the cloud.

This is a coordinated `proto/`, `rust-service/`, and `swift-client/` change.

## Data Flow

After an authenticated IPC connection is established, Swift requests today's
insight and seven days of history through the existing protocol messages. The
display coordinator tracks insight and history availability independently.

An insight `404` is represented by the existing `cache_empty` response and
renders a compact “Not generated yet” message. A successful history response
renders immediately even when the insight is unavailable. Transport and other
service failures retain the existing service-status treatment rather than being
mistaken for an empty insight.

## Pending Uploads

A new protocol request/response pair reports the number of privacy-safe events
that have not received a successful cloud acknowledgement. The Rust service
combines events held by the in-memory batch assembler with events in persisted
pending or retryable batches. It returns only the aggregate count; no raw event
fields, labels, identifiers, or batch details leave the service.

Swift requests this count after authentication and when the user opens the
Pending Uploads detail. The menu-bar popover presents the count and a compact
detail surface. It does not provide direct database access or expose event
contents.

## Account Actions

The popover footer is contextual: logged-out users get a Sign In / Create
Account action that opens the existing onboarding-auth window; logged-in users
get Log Out. Pending deletion and device revocation continue to use their
existing blocking flows.

## Protocol and Tests

The protocol version is bumped with schemas, Rust/Swift DTOs, router handling,
and contract tests updated atomically. Tests cover:

- requests issued after an authenticated connection;
- history rendering while the insight result is empty;
- empty insight copy versus real service errors;
- pending count including in-memory and persisted unacknowledged events;
- contextual account actions; and
- privacy assertions that the new payload contains only an aggregate count.
