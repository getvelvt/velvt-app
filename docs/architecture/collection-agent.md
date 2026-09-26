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

1. Ignore a duplicate activation for the application already observed at
   window level. Any other activation, including another one for the
   application in front when it is not observed at window level, registers.
2. Stop and release the previous per-process AX observer.
3. Create an AX observer for the new PID and register the focused-window
   notification on the application element.
4. If the application has a focused (or main) window, register its title
   notification and the browser adapter's optional document-change
   notifications when applicable, and start a window-level dwell for it.
5. If it has none yet (`kAXErrorNoValue`, -25212: an application still
   launching, one whose windows are all closed, a panel not yet key), keep the
   observer registered and start an application-level dwell. The first window
   to gain focus arrives as a focused-window notification and replaces it with
   a window-level dwell: that is the retry, and it needs no polling.
6. If the observer cannot be registered at all, start an application-level
   dwell. The next activation of that application registers again.

Whatever the registration finds, the previous dwell closes at the activation
instant and the activated application has a dwell of its own from that
instant. An application-level dwell carries what the workspace reports about
the application in front, which never needs Accessibility: its name, bundle
identifier and declared metadata, an empty `windowTitle` and no
`focusedDocumentURL`. It is not a guess at a window, and no title is made up.
Until 2026-09-26 an application that could not be observed at window level got
no dwell at all: the dwell before it stayed open and absorbed its time at the
next switch (up to the 30-minute cap), so the time went to the wrong
application and a departure to it never reached the drift gate. The service
treats the change as drift policy version 4
([work-block-loop.md](work-block-loop.md)).

Velvt's own window is not special-cased: activating it registers against
Velvt's own process like any other application, and the service classifies
Velvt as `SYSTEM`, which the drift gate never counts as a switch or as the
anchor. Activated before its panel is key, it gets an application-level dwell
like any other application.

Only one AX observer is active at a time. `stop()` is idempotent and removes the
AX run-loop source and the NSWorkspace subscription at most once. An observer
that fails after registering (an abrupt app termination can make the callback
element invalid, or a new window can refuse the title notification) is removed;
the application is still in front, so the rest of its time is an
application-level dwell until the next activation, and the NSWorkspace
subscription remains active for recovery.

Status follows the same split. `CollectionStatus.running` means the
application in front is observed at window level. `.limited(code)` means
collection continues but the application in front is observed at application
level only, with a fixed code: `ax_observer_registration_failed:<AXError>` for
a registration that found no window (-25212) or failed, and
`ax_observer_failed:<AXError>` for an observer that failed later. It returns to
`.running` when a window is reached, by the focused-window notification or by
the next activation that registers. Only a stop is `.idle`,
`.permissionRevoked` or `.error`.

The adapter retains only the active application's element and its current
focused-window element, if any, for the lifetime of that per-process observer.
Both are discarded on application switches and observer teardown. No AX
element crosses into the collection agent's serial event queue. A missing or
empty AX title starts an interval with an empty `windowTitle`; it is not
skipped. Before any window has gained focus, a notification reports nothing:
the element it names may be the application itself or a control, and its title
is not a window title.

## Dwell Time

The collection agent does not use a timer or poll for activity. Each observed
app or title boundary closes the preceding local interval and emits that event
with its whole-second `durationSeconds`; the new observation begins the next
interval. `stop()` and a permission revocation close and emit the current
interval as well.

The new observation is also handed to the sink at once, through
`EventSink.activityBegan(_:)`, always after the interval it closes. The relay
sends it as an in-progress `raw_event` (protocol 32), so the service's drift
gate sees a departure while it is happening rather than when the person comes
back. Neither `flushPendingDwell(at:)` nor `stop()` begins an activity, so
neither reports one.

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

The first command must return no call sites. The second must show only
`CollectionStatusLog`, the one status-transition line, which carries fixed
codes and never an application name, bundle identifier or title. The
observation audit must show only the documented activation, window/title, and
optional browser document-change notification types.
