# macOS Collection Agent

## Scope

The collection layer is local-only and event-driven. Its only output is:

```swift
RawEvent(appName: String, bundleIdentifier: String?, windowTitle: String,
         focusedDocumentURL: String?, occurredAt: Date, durationSeconds: Int)
```

It sends events only through `EventSink.receive(_:)`. It has no IPC, database,
batching, abstraction, upload, or network responsibilities. Raw application
names, window titles, and focused document URLs must never be logged. The URL is
captured only for recognized browsers and remains inside the Swift-to-Rust local
privacy boundary.

## Registered Observations

The collection layer always registers these observation types:

1. `NSWorkspace.didActivateApplicationNotification`
2. `kAXFocusedWindowChangedNotification`
3. `kAXTitleChangedNotification`

The AX notifications are registered against the active application and focused
window as supported by that process. For recognized browsers, the adapter also
registers focused-element and value/selection changes so same-title navigation
is observed without polling. These optional notifications feed the same bounded
activity callback and do not add persistence or network access.

## AXObserver Lifecycle

`AXCollectionAgent` owns collection state and delegates platform observation to
`AXApplicationObserver`.

On application activation:

1. Ignore a duplicate activation for the currently observed PID.
2. Stop and release the previous per-process AX observer.
3. Create an AX observer for the new PID.
4. Register focused-window/title notifications and the browser adapter's
   optional document-change notifications when applicable.
5. Start a local dwell interval for the new application's initial raw event.

Only one AX observer is active at a time. `stop()` is idempotent and removes the
AX run-loop source and the NSWorkspace subscription at most once. An abrupt app
termination can make the callback element invalid; this is converted into a
safe `CollectionStatus.error` value, the invalid AX observer is removed, and
the NSWorkspace subscription remains active for recovery on the next app
activation.

The adapter retains only the active application's element and its current
focused-window element for the lifetime of that per-process observer. Both are
discarded on application switches and observer teardown. No AX element crosses
into the collection agent's serial event queue. A missing or empty AX title
starts an interval with an empty `windowTitle`; it is not skipped.

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
title and, for recognized browsers, the focused document URL while still on
that run-loop thread. It converts them to Swift optional strings or a safe error
code and dispatches only those values to a private serial queue. No
`AXUIElement` crosses the adapter boundary.

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
Permission and activity changes are handled only through registered workspace
and AX notifications, explicit start/stop calls, and AX errors.

Audit commands:

```sh
rg -n "Timer|DispatchSourceTimer|sleep|while true|DispatchQueue\..*asyncAfter" \
  swift-client/Sources/VelvtMac/Collection

rg -n "os_log|Logger|print\(" swift-client/Sources/VelvtMac/Collection

rg -n "addObserver|AXObserverAddNotification|didActivateApplicationNotification|kAXFocusedWindowChangedNotification|kAXTitleChangedNotification" \
  swift-client/Sources/VelvtMac/Collection
```

The first two commands must return no call sites. The observation audit must
show only the documented activation, window/title, and optional browser
document-change notification types.
