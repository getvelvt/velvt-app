# Menu Popover UI Overhaul Design

## Scope

This change keeps Velvt menu-bar-only. Every user-facing route is hosted in the existing `NSPopover`; no onboarding, authentication, settings, or information window is created. The work spans `swift-client/`, `rust-service/`, and `proto/` because the user-triggered upload flush is a new IPC capability.

## Navigation and interaction model

The root popover retains its current insight/history content and inline authentication sheet. First-run and logged-out states do not replace the root content with the legacy onboarding flow. The current standalone onboarding flow (welcome, permission sequence, auth, completion) is removed rather than replaced with a placeholder.

The root footer has a status group on the left and a plain `Settings` text button aligned right. The status group is a colored dot and one of `Connected`, `Connecting`, or `Disconnected`; connecting includes both socket connect/handshake/reconnect states.

Navigation is represented by a page stack: `main`, `settings`, `appInfo`, and `queuedEvents`. Forward navigation slides the destination in from the right; back navigation moves the current page right and restores its parent. The animation is 180ms ease-in-out. When the SwiftUI accessibility setting for reduced motion is enabled, pages swap immediately.

## Settings pages

Settings has a minimal top row: a full-hit-area back button on the left (at least 8pt padding) and `Settings` right-aligned. Its body is an extensible full-width row list: `App Info` and `Queued Events (N)`, each with a trailing chevron. Authentication controls do not appear here. A separated destructive `Quit velvt` action terminates the app, invoking existing application termination so `AppDelegate.applicationWillTerminate` stops the bundled Rust service with SIGTERM.

App Info uses the same back/title header. It displays app version, registered device ID, authentication state and email, local-service status, and cloud-service status. Both connectivity rows contain a manual Refresh action. The Swift UI requests status only through IPC; it has no HTTP client.

Queued Events is a peer page rather than an App Info child. Its title includes the current count and it displays at most ten newest privacy-safe queued-event summaries, showing the source label from the service. A separated `Send All Now` action requests the service flush and then refreshes menu status.

## IPC and service behavior

`request_menu_status` remains the only status-refresh request. `MenuStatusViewModel` requests it at startup and every 60 seconds. The Rust `MenuStatusProvider` performs `GET /v1/ready`, so the existing periodic request already satisfies cloud polling while preserving the privacy boundary.

The protocol gains `flush_upload_queue`. On receipt, the Rust router delegates to the shared upload batcher: it flushes buffered abstracted events into durable batches, immediately runs the existing upload coordinator against pending batches, and responds with a fresh `menu_status` snapshot. The command never accepts raw data and never returns upload payloads. Failure is represented through safe status/error information only; rejected batches remain permanently rejected under the existing privacy policy.

The associated account email is held locally in the Swift Keychain after successful sign-up/login and cleared with logout/account deletion. It is only used for the App Info display, is never logged, and is not included in upload batches or the new status payload.

## Testing and privacy checks

Swift tests cover status-label mapping, route direction and reduced-motion behavior, settings row/action availability, and email keychain lifecycle. IPC encoding tests prove the new request uses an empty payload. Rust tests cover command routing, immediate batch flush/upload invocation, status refresh, and schema decoding. Protocol schema/version/changelog changes land atomically with both implementations. Existing upload payload privacy tests remain required and new tests assert the flush command/status response do not carry raw activity fields.
