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

## Collection Settings

The Collection Settings submenu holds the Offline Event Collection toggle, the
Initiation Invitations toggle, the toggle for the optional question after a
block, and Clear Local Work Blocks.

The question's toggle (protocol 38) is a Rust-owned setting, like initiation
invitations. `RatingPromptSettingsToggle` (`UI/QuestionAfterABlockView.swift`)
renders `rating_prompt_settings` verbatim, title and subtitle included, and is
drawn only while that message says `available`: once the Mac's v6 primary
period is over (14 days from its first block under drift policy 6, counted
again from the first block after Clear Local Work Blocks), or while the
question is on. Off is the default, and Swift
assumes nothing before the service reports. The submenu asks again each time it
opens, with `request_rating_prompt_settings`, because the switch becomes
available on day 14 without a reconnect. A change sends
`set_rating_prompt_enabled`; its reply carries only the setting, so after a
block has ended Swift also sends `request_work_block_state`, and a question the
change withdrew leaves the result view. The toggle never answers the one-time
invitation. Swift stores none of this.

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
