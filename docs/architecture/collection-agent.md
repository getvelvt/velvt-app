# macOS Collection Agent

## Scope

The collection layer is local-only and event-driven. Its only output is:

```swift
RawEvent(appName: String, windowTitle: String, occurredAt: Date, durationSeconds: Int)
```

It sends events only through `EventSink.receive(_:)`. It has no IPC, database,
batching, abstraction, upload, or network responsibilities. Raw application
names and window titles must never be logged.

## Registered Observations

The collection layer registers exactly these three observation types:

1. `NSWorkspace.didActivateApplicationNotification`
2. `kAXFocusedWindowChangedNotification`
3. `kAXTitleChangedNotification`

The two AX notifications are registered on the active application's main-window
element. Adding another observation type requires explicit approval.

## AXObserver Lifecycle

`AXCollectionAgent` owns collection state and delegates platform observation to
`AXApplicationObserver`.

On application activation:

1. Ignore a duplicate activation for the currently observed PID.
2. Stop and release the previous per-process AX observer.
3. Create an AX observer for the new PID.
4. Register the two approved AX notifications.
5. Start a local dwell interval for the new application's initial raw event.

Only one AX observer is active at a time. `stop()` is idempotent and removes the
AX run-loop source and the NSWorkspace subscription at most once. An abrupt app
termination can make the callback element invalid; this is converted into a
safe `CollectionStatus.error` value, the invalid AX observer is removed, and
the NSWorkspace subscription remains active for recovery on the next app
activation.

AX elements are callback-local values. The collection layer does not cache an
`AXUIElement` across callbacks or application switches. A missing or empty AX
title starts an interval with an empty `windowTitle`; it is not skipped.

## Dwell Time

The collection agent does not use a timer or poll for activity. Each observed
app or title boundary closes the preceding local interval and emits that event
with its whole-second `durationSeconds`; the new observation begins the next
interval. `stop()` and a permission revocation close and emit the current
interval as well.

To avoid treating an unattended period as active use, a single interval is
capped at 1,800 seconds (30 minutes). The raw title and app name remain local;
only the resulting duration follows the existing IPC path.

## Threading Model

The AX observer source runs on a private `CFRunLoop`. The AX callback reads the
title while still on that run-loop thread, converts it to a Swift optional
string or safe error code, and dispatches that value to a private serial queue.
No `AXUIElement` crosses the callback boundary.

The agent checks that callback values still belong to the active PID before
emitting them. This suppresses stale events from an observer that was removed
during a rapid application switch.

## Adding a Workspace Notification

Workspace notification registration belongs in `NSWorkspaceActivationObserver`,
not in `AXCollectionAgent`.

To add an explicitly approved notification:

1. Add one `NSWorkspace.notificationCenter.addObserver` subscription in the
   workspace adapter.
2. Add one dedicated handler that converts the notification to a safe,
   non-AX value.
3. Store and remove its subscription token in the workspace adapter.
4. Add adapter-focused tests.

The core AX collection loop and `CollectionAgentProtocol` do not change.

## No-Polling Invariant

The collection layer must not contain `Timer`, `DispatchSourceTimer`, sleep
calls, `while true`, or repeated `DispatchQueue.asyncAfter` scheduling.
Permission and activity changes are handled only through the three approved
notifications, explicit start/stop calls, and AX errors.

Audit commands:

```sh
rg -n "Timer|DispatchSourceTimer|sleep|while true|DispatchQueue\..*asyncAfter" \
  swift-client/Sources/VelvtMac/Collection

rg -n "os_log|Logger|print\(" swift-client/Sources/VelvtMac/Collection

rg -n "addObserver|AXObserverAddNotification|didActivateApplicationNotification|kAXFocusedWindowChangedNotification|kAXTitleChangedNotification" \
  swift-client/Sources/VelvtMac/Collection
```

The first two commands must return no call sites. The observation audit must
show only the three approved observation types.
