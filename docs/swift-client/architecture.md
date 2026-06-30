# Swift Client Architecture

The Swift client is a SwiftUI/AppKit macOS app with an AppKit composition root and SwiftUI views hosted inside a menu bar popover. Most subsystems are protocol-driven so tests can replace IPC, Keychain, permission, notification, and collection dependencies.

## Directory Map

| Path | Role |
|---|---|
| `Sources/VelvtMac/App/` | App entry point, AppDelegate composition, menu bar controller, service launcher, metrics |
| `Sources/VelvtMac/Auth/` | Account state manager, Keychain storage, auth view model |
| `Sources/VelvtMac/Collection/` | Accessibility collection agent and collection status |
| `Sources/VelvtMac/Config/` | Bundle and debug environment configuration loaders |
| `Sources/VelvtMac/Delivery/` | Display coordinators, menu status loader, notification delivery |
| `Sources/VelvtMac/Device/` | Device/APNs-adjacent module seams |
| `Sources/VelvtMac/IPC/` | Unix socket client, typed DTOs, reconnect backoff, fake client |
| `Sources/VelvtMac/Permissions/` | Accessibility and notification permission monitoring |
| `Sources/VelvtMac/Relay/` | In-memory event relay and event sink protocol |
| `Sources/VelvtMac/Service/` | Service manager/update support |
| `Sources/VelvtMac/UI/` | SwiftUI views and menu bar popover content |

## Composition Root

`AppDelegate.applicationDidFinishLaunching` wires the app in dependency order:

```text
ServiceProcessLauncher
PermissionManager
IPCClientProtocol
AccountStateManager
ConcreteDisplayDataCoordinator
MenuBarDataLoader
MenuStatusViewModel
EventRelay
AXCollectionAgent
PermissionCollectionCoordinator
MenuBarController
NotificationDeliveryCoordinator
NotificationResponseRouter
```

The ordering matters:

- `AccountStateManager` starts listening to IPC first because it is the only consumer of `incomingMessages`.
- It republishes messages through `serverMessages` for display, auth UI, notification, and settings consumers.
- Display and notification layers subscribe to the fan-out publisher rather than racing to consume the async stream.

## AppKit and SwiftUI Boundary

The app uses AppKit for process and menu bar integration:

- `NSApplicationDelegate` for startup/shutdown.
- `NSStatusItem` through `MenuBarController`.
- `NSPopover` hosting SwiftUI content.
- Accessory activation policy so no dock window is created.

SwiftUI owns rendered content inside the popover and sheets. `MenuBarPopoverView` routes between the main view and settings. Auth uses a sheet driven by `AuthViewModel`.

## State Flow

IPC state flows through one fan-out source:

```text
UnixSocketIPCClient.incomingMessages
AccountStateManager.handle
AccountStateManager.serverMessages
DisplayDataCoordinator / MenuStatusViewModel / AuthViewModel / NotificationDeliveryCoordinator
SwiftUI observable state
```

This avoids multiple tasks iterating the same `AsyncStream`.

Connection state flows separately through `IPCClientProtocol.connectionStatus`, which drives service status UI, request timing, and reconnect behavior.

## Event Capture and Relay

Collection produces `RawEvent` values and sends them to an `EventSink`. The runtime sink is a fan-out:

```text
AXCollectionAgent
EventSinkFanout
EventRelay          -> IPC raw_event
CurrentActivityModel -> immediate local popover display
```

`EventRelay` is the only component that sends captured events to Rust. While disconnected, it buffers in memory and drops oldest events once capacity is exceeded. It does not write raw events to disk.

`CurrentActivityModel` is local UI state. Because it can contain raw app/window text, it must remain local and must not be logged or uploaded.

## IPC Client

`UnixSocketIPCClient` implements `IPCClientProtocol`:

```swift
public protocol IPCClientProtocol: AnyObject {
    var incomingMessages: AsyncStream<ServerMessage> { get }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> { get }
    func connect() async throws
    func disconnect()
    func send(_ message: ClientMessage) async throws
}
```

The client performs server-first handshake and only allows public sends after `acknowledged`. Version mismatches are surfaced as `IPCError.versionMismatch`.

Tests use `FakeIPCClient` to drive state and messages without a real Unix socket.

## Delivery and Notifications

`ConcreteDisplayDataCoordinator` updates `InsightViewModel` and `HistoryViewModel` from `insight_payload`, `history_payload`, and `cache_empty`.

`MenuBarDataLoader` requests today's insight and seven days of history once the account is logged in and the socket is connected.

`NotificationDeliveryCoordinator` listens for `notification_payload` and schedules a user notification through `NotificationScheduling`. Swift does not create notification copy; the service payload is already ready to display.

## Permissions

`PermissionManager` monitors Accessibility and notification permission state. `PermissionCollectionCoordinator` starts and stops collection based on Accessibility status. Missing or revoked permission stops collection and surfaces recovery UI.

The app does not request screen recording, microphone, camera, or filesystem permissions.

## Testing Strategy

Swift tests cover:

- App composition and menu bar state.
- IPC handshake, reconnect, and typed message behavior.
- Auth state transitions and Keychain interactions through fakes.
- Display coordinator and view model state.
- Notification scheduling and response routing.
- Event relay buffering and drop behavior.
- Permissions and collection module behavior.

When adding a new runtime dependency, prefer a protocol and a fake implementation so tests can remain fast and local.
