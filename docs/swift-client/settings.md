# Swift Client Settings

The current settings surface is a compact menu bar workspace tab, not a standalone Preferences window. Settings are split between local UI preferences stored by Swift, runtime status fetched from Rust, and build/runtime configuration loaded at app startup.

## Settings UI

`MenuBarPopoverView` includes Settings in the same workspace navigation model as Today, Your Week, and Activity:

```swift
public enum MenuBarWorkspaceTab: CaseIterable, Equatable, Hashable {
    case workBlock
    case history
    case recentActivity
    case settings
}
```

The settings tab currently exposes:

- App Info submenu.
- Queued Events submenu.
- Collection Settings submenu.
- Onboarding & Tour submenu, including replay actions for both experiences.
- Contact support… action (since 1.0.15, in the place of the greyed-out Check for Updates…; see below).
- Quit Velvt action.
- App version display.

Submenus are hosted in AppKit popovers through `SubmenuPopoverAnchor`.
Each submenu uses a stable, predetermined viewport and reuses its hosting
controller so live status updates do not resize its controls during the opening
animation.
Submenus are explicitly positioned after AppKit shows the popover: short
submenus, such as Debug, are centered on the source row; taller submenus are
clamped to the top of the settings popover so they do not cover the title bar
or move off screen.

## App Info

The App Info submenu displays:

| Field | Source |
|---|---|
| Version | `CFBundleShortVersionString`, falling back to `VelvtClientVersion`, then `Development` |
| Device ID | `menu_status.device_id` from Rust, or `Not registered` |
| Authentication | `AccountStateManager.accountState` and local account email |
| Local service | `IPCClientProtocol.connectionStatus` |
| Cloud server | `menu_status.cloud_ready` from Rust |

Refreshing sends:

```swift
try await ipcClient.send(.requestMenuStatus)
```

## Copy Diagnostics and Contact support…

**Copy Diagnostics** (App Info) copies `key=value` lines that hold versions,
status codes and counts only: `app_version`, `build_number` (`CFBundleVersion`),
`protocol_version`, `macos_version` (major.minor.patch), `architecture`
(`arm64`, `x86_64`, or `x86_64_rosetta` for an Intel process on Apple silicon),
`local_service`, `collection`, `account`, `backend`, `queued_event_count`,
`last_successful_sync`, `next_retry` and `last_error_code`. No app, title, site,
account email or device identifier is in them. The builders for the build and
system lines are in `UI/SupportDiagnostics.swift`.

**Contact support…** (the Settings footer) opens a `mailto:hi@getvelvt.com`
draft whose body is a "What happened?" line followed by exactly that text. The
person's mail app shows the draft; Velvt sends nothing itself. If no app takes
the draft, the diagnostics are copied and an alert names the address. It took
the place of **Check for Updates…**, which alpha builds showed greyed out
because their updater is off (`docs/updates.md`), so the footer stays at three
buttons.

## Queued Events

The Queued Events submenu displays the first ten queued event summaries from Rust:

```swift
public struct MenuStatus: Codable, Equatable, Sendable {
    public let deviceID: String?
    public let cloudReady: Bool
    public let queuedEventCount: Int
    public let queuedEvents: [QueuedEventSummary]
}
```

The UI renders only the broad category and queued time. `localLabel` remains device-local data returned over IPC and is deliberately excluded from this diagnostic surface as well as every cloud upload DTO.

The "Send All Now" action sends:

```swift
try await ipcClient.send(.flushUploadQueue)
```

Rust responds at once with a `menu_status` showing the queue as it stands and sends the queue in the background; the next `menu_status` shows the outcome. Since 1.0.14 Rust no longer sends `error_response(upload_flush_failed)`; the client still handles it.

## Persisted Local UI State

Swift persists only lightweight UI/session state:

| State | Storage | File |
|---|---|---|
| First-run completion | `UserDefaults` keys `hasCompletedPermissionOnboarding` and `velvt.onboarding.completed_version` | `UI/PermissionViews.swift` |
| Action counter | `UserDefaults` key `velvt.metrics.actions_logged` | `App/AppMetricsStore.swift` |
| Intervention counter | `UserDefaults` key `velvt.metrics.interventions` | `App/AppMetricsStore.swift` |
| Auth session and account email | Keychain | `Auth/AuthModule.swift` |
| Pending deletion sentinel | Keychain key `velvt.pending_deletion` | `Auth/AuthModule.swift` |

No raw event buffer is persisted by Swift. Captured events are held in memory by `EventRelay` only.

## Configuration Sources

`FocusAgentConfig` is the typed Swift configuration:

```swift
public struct FocusAgentConfig: Equatable, Sendable {
    public let socketPath: String
    public let protocolVersion: Int
    public let clientVersion: String
    public let apnsEnvironment: APNSEnvironment
}
```

Release and packaged app builds use `BundleConfigLoader`, which reads values from `Info.plist` keys generated by xcconfig:

- `VelvtSocketPath`
- `VelvtProtocolVersion`
- `VelvtClientVersion`
- `VelvtAPNSEnv`

Debug SwiftPM runs can use `EnvironmentConfigLoader`:

- `VELVT_SOCKET_PATH`
- `VELVT_PROTOCOL_VERSION`
- `VELVT_CLIENT_VERSION`

The canonical values still live in `proto/ipc_socket_path` and `proto/version`. Keep the Swift xcconfig values in sync when changing protocol configuration.

## Adding a Setting

Before adding a setting, decide which category it belongs to:

| Category | Owner |
|---|---|
| Pure UI preference | Swift, usually `UserDefaults` behind a small protocol |
| Auth/session value | Swift Keychain or Rust token store depending on whether it is host UI state or service cloud state |
| Service/runtime status | Rust, exposed through `menu_status` or a new IPC message |
| Upload, abstraction, retention, or cloud behavior | Rust configuration, not Swift UI state |
| IPC schema-visible setting | `proto/` plus coordinated Swift/Rust update |

Do not store tokens, raw events, app names, window titles, URLs, paths, filenames, contacts, or raw text in `UserDefaults`.
