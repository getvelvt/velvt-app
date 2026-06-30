# S7 Menu Bar and Notifications

## Scope

S7 adds the menu bar status item, its popover (hosting the S6 display
content), and notification scheduling from Rust-pushed `NotificationPayload`
messages. It:

1. Creates and owns the single `NSStatusItem` and its `NSPopover`
   (`MenuBarController`).
2. Derives a single `MenuBarState` from three independent status sources and
   reflects it as the status item icon.
3. Schedules `UNNotificationRequest`s from `NotificationPayload` IPC pushes,
   gated on the notifications `PermissionStatus` and `do_not_disturb_until`.
4. Routes a tapped notification back to "open the popover, scroll to the
   insight date."

`MenuBarController` is the **only** type that creates or touches an
`NSStatusItem`. No other module should import `NSStatusItem` directly.

## MenuBarState Derivation

```swift
public enum MenuBarState: Equatable, Sendable, CaseIterable {
    case normal
    case collectionPaused
    case ipcDisconnected
    case deviceRevoked
}
```

`MenuBarStateResolver.resolve(collectionStatus:connectionStatus:accountState:isDeviceRevoked:)`
is a pure function with the following precedence:

| Precedence | Condition | Resolved state | Icon (SF Symbol) |
|---|---|---|---|
| 1 (highest) | `isDeviceRevoked == true` | `.deviceRevoked` | `exclamationmark.triangle.fill` |
| 2 | `connectionStatus != .connected` | `.ipcDisconnected` | `wifi.slash` |
| 3 | `collectionStatus == .permissionRevoked` | `.collectionPaused` | `pause.circle` |
| 4 (default) | none of the above | `.normal` | `circle.fill` |

`accountState` (the full `AccountState` enum, not just the revoked flag) is
part of the resolver's signature for completeness, but only `isDeviceRevoked`
currently affects the decision — `accountState`'s value itself does not
change the resolved icon. All icons are SF Symbol-backed `NSImage`s with
`isTemplate = true`, so they adapt automatically to light/dark menu bar
appearance. `MenuBarIconProvider` also supplies a non-empty
`accessibilityDescription` per state for VoiceOver/Accessibility Inspector.

### Avoiding combine-latest glitches

`MenuBarController.observe(...)` does not subscribe to the three status
publishers directly. It builds the stream via `MenuBarStateStream.make(...)`,
which:

1. Combines `collectionStatus`, `connectionStatus`,
   `accountStateManager.$accountState`, and
   `accountStateManager.$isDeviceRevoked` with `Publishers.CombineLatest4`.
2. Maps each combination through `MenuBarStateResolver`.
3. **Debounces the resolved `MenuBarState`** (default 50ms) before
   `removeDuplicates()` and delivery to the icon.

`CombineLatest` re-emits on *every* upstream emission using the latest cached
value from the other publishers. When two of the four sources change as part
of one logical event but arrive as separate emissions — e.g.
`AccountStateManager`'s `device_revoked` handler sets `accountState` and
`isDeviceRevoked` in two separate statements — the first of those two
emissions can momentarily combine a stale value with a fresh one, resolving
to an incorrect transient state. Debouncing the *resolved output* (not any
one input) coalesces that burst into a single, correct, settled emission
before it ever reaches the icon. See `MenuBarStateStreamTests` for a
reproduction using a real `AccountStateManager`-driven `device_revoked` push.

## MenuBarController

```swift
@MainActor
public final class MenuBarController: NSObject {
    public init(presentation:, displayCoordinator:, activateApp: @MainActor () -> Void = ...)
    public func install()       // creates the NSStatusItem; sets .accessory activation policy
    public func remove()        // tears down the NSStatusItem
    public func observe(collectionStatus:, connectionStatus:, accountStateManager:)
    public func togglePopover()
    public func showPopover()   // activates the app first, then shows
    public func closePopover()
    public var isPopoverShown: Bool
}
```

- `install()` sets `NSApp.activationPolicy = .accessory` (no Dock icon) and
  creates the status item with an initial `.normal` icon. This does not
  prevent the app from showing windows (e.g. onboarding) or from terminating
  normally.
- The popover's `NSHostingController` wraps `MenuBarPopoverView`, which embeds
  the unmodified S6 `VelvtPopoverContentView` (`InsightCardView` +
  `HistoryListView`). Pushing a new `InsightPayload`/`HistoryPayload` through
  `ConcreteDisplayDataCoordinator` while the popover is open updates the
  existing view models in place — the coordinator never closes or reopens the
  popover, and `updateInsight`/`updateHistory` reuse the same `InsightViewModel`
  /`HistoryViewModel` instances once populated.
- `showPopover()` calls `activateApp()` before showing. The default
  implementation calls `NSApp.unhide(nil)` and
  `NSApp.activate(ignoringOtherApps: true)`, so a notification tap arriving
  while the app is hidden still brings it to the foreground and opens the
  popover correctly. `activateApp` is injectable for tests.
- Popover dismissal: `behavior = .transient` (closes on click-outside) plus an
  explicit `onExitCommand` in `MenuBarPopoverView` that calls `closePopover()`
  (Escape).

## Notification Scheduling Flow

```
Rust pushes ServerMessage.notificationPayload(NotificationPayload)
        │
        ▼
AccountStateManager.serverMessages (fan-out, same relay S6 uses)
        │
        ▼
NotificationDeliveryCoordinator.handle(_:)
        │
        ├─ cancels any pending task for the same insight date
        ├─ starts a new debounced task (default 250ms)
        │      │
        │      ├─ Task.sleep(debounceInterval)        ◄── absorbs a burst
        │      ├─ if cancelled, stop (superseded)
        │      ├─ permissionManager.checkStatus(.notifications)
        │      ├─ if not .granted, stop — silent discard
        │      └─ scheduler.schedule(payload)
        ▼
NotificationSchedulerProtocol (UNNotificationScheduler in production)
        │
        ├─ builds UNMutableNotificationContent (title, body, userInfo: insight_date)
        ├─ do_not_disturb_until in the future?  → UNTimeIntervalNotificationTrigger(remaining)
        ├─ otherwise (nil, or already elapsed)   → trigger = nil (deliver immediately)
        └─ UNUserNotificationCenter.add(request)
```

Notification copy (`title`/`body`) is exactly what Rust sends — the Swift
layer never generates or edits insight text. `NotificationDeliveryCoordinator`
is the **only** place that checks notifications `PermissionStatus`; denied,
restricted, or undetermined status discards the payload silently (no crash,
no retry, no re-request).

In debug builds, Settings includes a Debug submenu with a "Simulate Insight"
action. It calls `NotificationDeliveryCoordinator.simulateDebugInsightReceipt()`,
which creates a representative `NotificationPayload` and routes it through the
same scheduler/permission path used by real Rust IPC pushes. This is a local
test harness only; it does not contact velvt-core.

### Burst de-duplication

`NotificationDeliveryCoordinator` keeps one pending `Task` per insight date
(`pendingTasksByDate`). A new payload for a date that already has pending work
cancels that work *before* it has done anything observable. Combined with the
debounce sleep at the start of each task's body, this guarantees that a rapid
burst of corrected payloads for the same date — however the IPC listener loop
interleaves their delivery — results in **at most one scheduled system
notification: the most recently received payload.** Payloads for different
insight dates are independent and each schedules its own notification.

### `do_not_disturb_until` enforcement

Enforced entirely in `UNNotificationScheduler.schedule(_:)`, at schedule time:

| `doNotDisturbUntil` | Trigger | Behavior |
|---|---|---|
| `nil` | `nil` | Delivered immediately |
| In the future | `UNTimeIntervalNotificationTrigger(timeInterval: remaining, repeats: false)` | Delivered after the remaining interval |
| In the past (already elapsed) | `nil` | Delivered immediately — **not** skipped |

`now()` is injectable (`UNNotificationScheduler.init(center:now:)`) so tests
can assert exact trigger intervals without real-clock flakiness.

### Insight text is never persisted by the scheduler

`UNNotificationScheduler.schedule(_:)` builds `content`/`request` as local
variables; both — and the `payload` reference itself — fall out of scope once
`center.add(request)` returns. Nothing is cached on `self`, written to disk,
or logged. `NotificationDeliveryCoordinator` only ever holds a payload inside
the closure of its own in-flight `Task` for that one schedule attempt; it is
released once the task completes or is superseded.

## Notification Tap → Popover Scroll-to-Date

```swift
@MainActor
public final class NotificationResponseRouter: NSObject, UNUserNotificationCenterDelegate {
    public init(openPopover: () -> Void, scrollToDate: ScrollToDateAction)
    func handle(userInfo: [AnyHashable: Any])  // extracts "insight_date", calls openPopover() + scrollToDate(date)
}
```

- `userNotificationCenter(_:didReceive:withCompletionHandler:)` is
  `nonisolated` (required by the protocol) and hops to the main actor to call
  the isolated `handle(userInfo:)`. `handle` takes the already-extracted
  `userInfo` dictionary rather than a `UNNotificationResponse`, since the
  latter cannot be constructed in unit tests — this is the testable seam.
- If `userInfo["insight_date"]` is missing or not a `String`, the tap is
  ignored (no crash).
- `openPopover` is wired in `AppDelegate` to `menuBarController.showPopover()`,
  which — per the `MenuBarController` section above — activates the app first,
  so this works correctly even when the app was hidden when the tap occurred.
- `ScrollToDateAction` (`@MainActor` callable struct) wraps
  `HistoryViewModel.scrollToDate(_:)`. `HistoryViewModel.scrollTarget`
  publishes the requested date; `HistoryListView` tags each row with
  `.id(day.id)` so a `ScrollViewReader` can anchor to it.

## Adding a New Notification Type

1. Add one field (or, if structurally distinct, one new payload variant) to
   `NotificationPayload` / `proto/schema/`, following the existing
   "Adding an IPC DTO" process.
2. Add one case to a switch inside `UNNotificationScheduler.schedule(_:)` (or
   a sibling scheduler method) to build the right `UNMutableNotificationContent`.
3. `MenuBarController` and `MenuBarState` never reference notification
   content — no changes needed there.

## Keyboard Navigation

The popover is keyboard-accessible:

- **Tab order**: insight card → history row 1 → … → history row 7, following
  SwiftUI declaration order. Each focusable element opts in with
  `.focusable()`:
  - `InsightCardContentView` (in `InsightCardView.swift`)
  - `HistoryDayRowView` (in `HistoryListView.swift`), one per row
  No shared `@FocusState`/enum is needed — SwiftUI's default macOS Tab
  traversal follows view-hierarchy order for focusable elements.
- **Escape**: `MenuBarPopoverView.onExitCommand` calls
  `MenuBarController.closePopover()`. The popover's `.transient` behavior also
  closes on click-outside.
- **Accessibility labels**: both focusable elements set
  `.accessibilityElement(children: .combine)` plus an explicit
  `.accessibilityLabel`/`.accessibilityValue` (e.g. `"Insight for <date>"` /
  the insight text, confidence, and timestamp; `"<date>, <status>"` / active
  time and scores) so VoiceOver and Accessibility Inspector report one
  meaningful element per row instead of a flat list of child `Text` views.
  The status item's icon carries a non-empty `accessibilityDescription` per
  `MenuBarState` (`MenuBarIconProvider.accessibilityDescription`).

## Testability

| Concern | Test seam | No real system access needed because… |
|---|---|---|
| `MenuBarState` derivation | `MenuBarStateResolver` (pure function) | No AppKit, no Combine |
| Combine-latest glitch hardening | `MenuBarStateStream.make(...)` with injectable `debounceInterval` | Uses `PassthroughSubject`/`CurrentValueSubject` directly, no real status sources |
| Notification scheduling | `NotificationSchedulerProtocol` + `FakeNotificationScheduler` | Never touches `UNUserNotificationCenter` |
| DND trigger math | `UNNotificationScheduler` + `FakeUNUserNotificationCenter` + injectable `now()` | `UNUserNotificationCenterProtocol` seam |
| Permission gating / burst de-dup | `NotificationDeliveryCoordinator` + `FakePermissionManager` + `FakeNotificationScheduler`, injectable `debounceInterval` | No real permission prompts |
| Notification tap routing | `NotificationResponseRouter.handle(userInfo:)` | Bypasses `UNNotificationResponse`, which cannot be constructed in tests |
| Status item / popover | `MenuBarController` with injectable `activateApp` | `NSStatusItem`/`NSPopover` work headlessly under `swift test` on macOS; `activateApp` avoids mutating the process-wide `NSApp.isHidden` state in tests |

All fakes follow the existing `Fake*` protocol-double convention
(`FakeIPCClient`, `FakeKeychain`, `FakePermissionManager`).
