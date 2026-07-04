# Swift Client Overview

The Swift client is Velvt's native macOS menu bar app. It captures local activity signals, relays raw events to the Rust service over a Unix domain socket, receives ready-to-display payloads, manages user permissions, schedules notifications, and owns user-facing authentication screens.

It does not perform abstraction, upload to the cloud, run analytics, or generate insight text.

## Responsibilities

| Area | Responsibility |
|---|---|
| App lifecycle | Start/stop app-owned services, install menu bar item, launch bundled Rust helper in packaged app builds |
| Collection | Observe app activation and focused-window title changes through macOS APIs |
| Relay | Buffer raw events in memory while IPC is unavailable and send them to Rust when connected |
| IPC | Unix socket client, handshake, reconnect behavior, typed messages |
| UI | Menu bar popover, insight/history display, settings, permission recovery, auth sheets |
| Permissions | Accessibility and notification permission status |
| Auth UI | Sign-up, login, logout, account deletion, local session state |
| Local persistence | Keychain for auth session, `UserDefaults` for lightweight UI state and metrics |
| Notifications | Schedule user notifications from Rust-provided notification payloads |

## What Swift Does Not Own

- Raw-to-abstract mapping.
- SQLite persistence for abstracted events or upload state.
- Cloud HTTP calls.
- Upload retry/backoff.
- Insight or notification text generation.
- Local analytics or LLM inference.

Those responsibilities belong to `rust-service/`.

## Runtime Composition

`AppDelegate` in `Sources/VelvtMac/App/AppModule.swift` is the composition root. At launch it:

1. Starts the bundled Rust helper when running as a packaged `.app`.
2. Starts permission monitoring.
3. Creates the IPC client from bundle or debug environment configuration.
4. Starts `AccountStateManager` as the sole consumer of incoming IPC messages.
5. Creates display, menu status, collection, relay, menu bar, and notification coordinators.
6. Starts event relay and collection after permission state is known.
7. Connects the IPC client and handles version mismatch alerts.

## Event Capture

The collection layer observes macOS events rather than polling. It uses application activation and Accessibility window/title notifications to produce `RawEvent` values.

Raw values may include app name, bundle ID, and focused-window title. They are sent only to the local Rust service and must not be logged or stored by Swift.

## User Interface

The UI is menu-bar first:

- `MenuBarController` owns `NSStatusItem` and popover lifecycle.
- `MenuBarPopoverView` renders the main and settings routes.
- `VelvtPopoverContentView`, `InsightCardView`, and `HistoryListView` render delivery payloads.
- `PermissionViews` render onboarding and recovery states.
- Auth UI is shown as a sheet from the menu bar popover.

There is no normal `WindowGroup`; the app runs with accessory activation policy
at runtime. The bundle does not declare `LSUIElement`, so macOS can index it as
a normal app when it is copied into `/Applications`.

## Local State

Swift persists:

- Auth tokens, user ID, device ID, expiry, account email, and pending-deletion sentinel in Keychain.
- Permission onboarding completion in `UserDefaults`.
- Local app metrics counters in `UserDefaults`.

Swift does not persist raw activity events. `EventRelay` buffers events in memory only.

## Build Targets

The Swift package defines executable target `VelvtMac` and test target `VelvtMacTests`. The Xcode project defines the native app scheme `velvt-mac`.

Common commands:

```sh
swift test --package-path swift-client
xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' build
```

Packaged app builds use the root `Makefile`:

```sh
make build-app
```
