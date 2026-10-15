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

Velvt posts three kinds of notification, and Rust decides and words every one:
the daily insight (`notification_payload`, below), the in-block drift offer
(`work_block_state.active_intervention`, "Drift Offer Delivery" below), and,
since protocol 33, the needs-a-category reminder (`category_prompt`,
"Needs-a-Category Reminder" below).

`MenuBarController` is the **only** type that creates or touches an
`NSStatusItem`. No other module should import `NSStatusItem` directly.

## MenuBarState Derivation

```swift
public enum MenuBarState: Equatable, Sendable, CaseIterable {
    case normal
    case offerWaiting
    case collectionPaused
    case ipcDisconnected
    case deviceRevoked
}
```

`MenuBarStateResolver.resolve(collectionStatus:connectionStatus:accountState:isDeviceRevoked:isOfferWaitingUnshown:)`
is a pure function with the following precedence:

| Precedence | Condition | Resolved state | Icon (SF Symbol) |
|---|---|---|---|
| 1 (highest) | `isDeviceRevoked == true` | `.deviceRevoked` | `exclamationmark.triangle.fill` |
| 2 | `connectionStatus != .connected` | `.ipcDisconnected` | `wifi.slash` |
| 3 | `collectionStatus == .permissionRevoked` | `.collectionPaused` | `pause.circle` |
| 4 | `isOfferWaitingUnshown == true` | `.offerWaiting` | `text.bubble` |
| 5 (default) | none of the above | `.normal` | `circle.fill` |

`.offerWaiting` (1.0.14 simulation, drift-no-fallback) is the passive "offer
waiting" state: a normal-salience drift offer is live and nothing of it
reached the screen outside the popover, because notifications are off or
unanswered, the alert style for Velvt is None, or it was too old to post
(`stale`). `InterventionNotifier.isOfferWaitingUnshown` drives it, and is false
while the first attempt for an offer is still settling and once a banner was
posted, so a delivered offer never shows it. A quiet offer (a dismissal's
backoff, or Velvt's quiet hours) never shows it either: quiet means the in-app
card alone, and `analyze_cohort.py` compares quiet offers with normal ones, so
a glyph on them would change what the quiet arm receives. The glyph is a new
delivery surface for normal offers that were not posted visibly, and the 1.4
pre-registration amendment names it. The glyph is the only change:
no sound, no badge count, no animation. Until it existed, the menu bar icon
had no state for a live offer at all, and an offer that did not ring existed
only behind a click on it. Its tooltip and VoiceOver description read "Velvt
— a nudge is waiting in the Velvt window".

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
   `accountStateManager.$isDeviceRevoked` with `Publishers.CombineLatest4`,
   and that with the notifier's `isOfferWaitingUnshown`.
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
  normally. The bundle intentionally does not set `LSUIElement`; that keeps the
  installed app visible to LaunchServices/Launchpad while preserving menu-bar
  behavior at runtime.
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
action. It calls
`NotificationDeliveryCoordinator.simulateDebugInsightReceipt()` to route an
immediate `NotificationPayload` through the same native macOS
scheduler/permission path used by real Rust IPC pushes. It does not inject an
insight directly into the in-app display and does not contact velvt-core. The
popover closes immediately after the action so it cannot obscure the native
macOS banner.

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

## Drift Offer Delivery and What It Logs

The drift offer reaches Swift inside the `work_block_state` snapshot as
`active_intervention`; `InterventionNotifier` posts it once per offer
(`velvt.intervention.<block>.<offered_at>`) after a notifications-permission
check. Since protocol 32 the service makes the offer when the person arrives
in the away app, from the dwell's in-progress report, and withdraws it when
they come back — so the offer is live for as long as they are away. Before,
it was made from the away dwell's closed report, which is sent at the moment
they come back, and withdrawn by the next report about a second later.

Every attempt that settles also reports what became of the offer to the
service (protocol 35): `WorkBlockCoordinator.reportInterventionDelivery`
sends `intervention_delivery {block_id, channel}`, which Rust stores on the
offer's row (`work_block_intervention.delivery_channel`, migration 0043) and
answers with nothing. `channel` is `notification_posted`, `alert_style_none`
(posted, and the alert style for Velvt is None, read from
`UNNotificationSettings.alertStyle` after the post), `permission_denied`,
`permission_unknown` (`unknown` or `restricted`), `rejected`, `withdrawn`,
`quiet` or `stale`. It is sent once per change for the current offer, so the
retry a republished snapshot makes for a denied offer does not repeat it. Only
the attempt that holds the one-at-a-time slot settles the current offer: one
that `forgetCurrentOffer` cancelled can return after the same offer was seen
again, and says nothing about it. `quiet` is sent only for an offer that was
never tried, so an offer that turns quiet after an attempt keeps what the
attempt found. A post that lands after its offer ended is reported as
`withdrawn`, not posted: the outcome was decided before the banner existed,
and the banner is taken down at once (its log line is still
`notification_delivered`, since the centre did accept it). In Rust a posted channel is final,
`withdrawn` and `stale` fill only an empty channel (so a cancelled retry
cannot replace a denial, and a relaunch cannot replace what an earlier process
reported), and any other is replaced by the next report. A posted channel is
not proof the person saw it: a Focus mode Velvt could not detect holds it in
Notification Center, and it is still `notification_posted`. The
cohort export carries it (`delivery_channel`, `notification_delivery`) and
`analyze_cohort.py` reports it as a descriptive breakdown beside the
pre-registered figures, whose denominators it does not change. Until it
existed, an offer that never rang was recorded exactly like one that did.

Once the offer is over (answered, withdrawn by a return, or the block ended),
`forgetCurrentOffer` takes its notification out of Notification Center with
`removeDeliveredNotifications(withIdentifiers:)`, under the offer's own
identifier, which also removes one an earlier process posted for it. An offer
replaced by another without a snapshot between them is removed the same way,
and so is a post that lands after its offer was withdrawn. An offer this
process first sees more than `InterventionNotifier.maximumPostingAge` (120
seconds) after its `offered_at` is not posted: the banner would be a claim
about a moment that has passed, and a later grant of permission does not
revive it. `offered_at` is the time of the evidence the gate decided on (the
start of the dwell that tripped it), not of the decision, so this happens on
a relaunch while the offer is live and also in a running app whenever the
gate decided on a long dwell's closed report, after `EventRelay.sendLive`
dropped the in-progress one. It is reported as `stale`, so the export does not
call it `not_reported`.

Every attempt ends in exactly one line under subsystem `com.velvt.mac`,
category `NotificationDelivery`, at a persisted level, so
`log show --last 1d --predicate 'subsystem == "com.velvt.mac" AND category == "NotificationDelivery"'`
answers after the fact what happened (`scripts/watch_notifications.sh` wraps
it):

| Line | Level | Meaning |
|---|---|---|
| `notification_delivered` | notice | `UNUserNotificationCenter.add(_:)` accepted the request |
| `notification_delivered alert_style=none` | notice | accepted, and the alert style for Velvt is None: listed, never shown on screen |
| `notification_skipped_stale` | notice | the offer was over two minutes past its `offered_at` when this process first saw it; not posted, reported as `stale` |
| `notification_suppressed_by_salience` | notice | quiet offer: card only, by design |
| `notification_withdrawn_before_delivery` | notice | the offer ended while the permission check was in flight; nothing posted |
| `error_code=notification_permission_blocked status=…` | error | notifications not authorised; the offer stays eligible |
| `error_code=notification_centre_rejected` | error | the centre refused the request |
| `notification_presented_while_active presentation=banner` | notice | Velvt was active; shown as a banner as usual |
| `notification_presented_while_active presentation=list_behind_visible_card` | notice | Velvt was active with the menu-bar window in front, which already draws the card: listed in Notification Center, no banner or sound |

`notification_delivered` with no banner on screen means macOS held it — Do
Not Disturb or another Focus — and it went to Notification Center. Velvt reads
Focus state only through Focus status (`INFocusStatusCenter`); when it cannot,
the service cannot hold the offer as `suppressed_dnd`, and the offer is
recorded as `notification_posted`. The next section says when it cannot.

### Focus status: what a build can read (protocol 36)

Apple gives Focus status only to an app signed with the Communication
Notifications capability. Three things have to hold, and the third is the one
Velvt's release builds through 1.0.13 lack:

1. `NSFocusStatusUsageDescription` in `Info.plist` (present: the xcconfigs set
   `INFOPLIST_KEY_NSFocusStatusUsageDescription`).
2. The person allows Focus status (the onboarding step) and notifications.
3. The `com.apple.developer.usernotifications.communication` entitlement.
   It is a restricted entitlement, so a Developer ID build carries it only
   with a Developer ID provisioning profile embedded at
   `Velvt.app/Contents/embedded.provisionprofile`. `Release.entitlements` is
   an empty dict and the 1.0.13 bundle has no profile
   (`codesign -d --entitlements :- /Applications/Velvt.app` prints an empty
   dict).

Sources: Apple's Focus-status sample code ("Requires UserNotifications and
FocusStatus to be authorized and Communication Notifications capability
enabled for the app's target"; "Unauthorized apps don't receive a Focus status
value"), an Apple engineer on developer forums thread 682143 ("Focus Status
will only be available to apps that have the Communication Notifications
capability") and thread 685744 (the usage description, notification
authorization, and the entitlement), TN3125 (restricted entitlements need a
profile, embedded at `Contents/embedded.provisionprofile`), and Supported
capabilities (macOS) (Communication Notifications is available to Developer
ID). Without the entitlement the person can still allow Focus status, and
`focusStatus.isFocused` still answers, `false`, but `donotdisturbd` refuses
the read ("App is missing Communication Notifications entitlement"), so the
value says nothing about the person's Focus.

What the app does about it:

- **Launch self-check.** One line at launch under subsystem `com.velvt.mac`,
  category `FocusStatus`, values only:
  `focus_status_self_check authorization_status=… is_focused=… communication_notifications_entitlement=present|absent availability=…`.
  `log show --last 1h --predicate 'subsystem == "com.velvt.mac" AND category == "FocusStatus"'`
  answers it on a signed build; `scripts/watch_notifications.sh` shows it too.
- **Availability fact.** `FocusStateObserver` reports
  `focus_status_availability` on every connection and whenever it changes
  (`available`, `unavailable`, `denied`, `not_determined`). The entitlement
  decides first (`FocusStatusEntitlement.isPresentInThisProcess`, read from the
  code signature), so an authorized but unentitled build is `unavailable`. It
  sends `focus_state_changed` only while `available`: 1.0.13 sent a fabricated
  `active: false` on every reconnect.
- **The decision row says what was known.** Rust records
  `intervention_decision_log.focus_state` on every drift decision: `on` or
  `off` only while the app reports `available` and an edge is recorded,
  `unknown` otherwise (migration 0044). No decision changes: the Focus hold
  reads the edges as before, and an unentitled build's edges never held
  anything.
- **The cohort export and the analysis.** The decisions file carries
  `focus_state` (export format 6), and `analyze_cohort.py` reports offers by it
  as a descriptive breakdown. An offer whose `focus_state` is `unknown` has
  unknown delivery: a Focus may have held it in Notification Center.

What gets a nudge through a Focus today: the person adds Velvt to that Focus's
Allowed Apps (System Settings > Focus > the Focus > Allowed Notifications). An
allowed app is not silenced, and per Apple an app on the Allowed Apps list
does not see the person as focused, so with the entitlement the hold would
not apply either. Velvt never uses time-sensitive notifications to get
around a Focus.

A daily insight notification does not touch the local interventions counter
(`AppMetricsStore`, shown in Debug builds): it is not a nudge inside a block,
and counting it as one inflated that number (critic-05). Only a posted drift
offer counts.

## Needs-a-Category Reminder

The third kind (protocol 33). Swift pulls `request_category_prompt` with its
UTC offset on the `.connected` edge, on wake, and on the 60-second menu-status
cadence; Rust always answers with `category_prompt`. Rust owns every rule
(`rust-service/src/category_prompt/mod.rs`, `CATEGORY_PROMPT_POLICY_VERSION`
1): nothing at all while a work block is active or paused; the in-app `card`
while any of the first eight entries of the last seven days' needs-a-category
list is unanswered; and a `notification` at most once per local day, only for
an entry among those eight that no earlier reminder or answer has reached,
never in Velvt's quiet hours (read at the offset the request carries) or a
known Focus, and paused for seven days after three reminders in a row, within
30 days, that were not followed by an `opened` answer. A reminder and an
answer reach the entries listed below the eight as well, so an entry moving up
into the eight is not news.

A `notification` is claimed in Rust when it is handed over and is never handed
over again, so Swift posts it at once or not at all: only if notifications are
already authorised (it never asks for permission for it), with no
`insight_date`, and without touching the interventions counter. Its copy is
counts only ("2 sites and 1 app you used this week don't have a category
yet. Choose once in Velvt."), because Notification Center keeps a
notification's text. A tap, or the card's primary action, opens the
needs-a-category list in Settings and sends `acknowledge_category_prompt`
with `opened`; the card's secondary action sends `not_now`. Either answer
quiets the card until a new entry joins the list.

In Swift:

- `CategoryPromptCoordinator` (`Delivery/`) does the pulling, holds the card
  (`prompt`), answers it, and posts a reminder through
  `CategoryPromptNotificationScheduling.scheduleCategoryPrompt(title:body:)`.
  It shares `MenuStatusViewModel.cadence`, the menu status's own 60-second
  timer, rather than running one. A tap on the reminder after its card was
  closed still answers `opened`, with the last card's id: the service records
  `opened` against the latest reminder. A tap when no card id is known in
  this process (after a relaunch, or a tap that launched the app) asks for
  the card and answers `opened` for the one the next reply carries; a reply
  with no card ends it. A card it has answered, and any reminder a reply
  carries for that same card, is not shown again until a reply carries no card
  or another one. An answer whose send fails is kept and sent first when the
  socket next connects, before the request for the card.
- The reminder's `userInfo` is `["velvt_category_prompt": true]` and nothing
  else: no prompt id, no text. It is posted under one identifier,
  `velvt.category_prompt`, so a new reminder replaces the last in
  Notification Center. Its delivery lines carry `surface=category_prompt`.
- `CategoryPromptCardView` (`UI/CategoryPromptCard.swift`) draws the card in
  the panel's proactive stack, after `WorkBlockProactiveCards`, and draws
  nothing while the block phase is active or paused, even with a card in
  hand. `presentedPrompt` is the testable list.
- A tap reaches `NotificationResponseRouter`'s `openNeedsACategory`, which
  answers the card and calls `MenuBarController.showNeedsACategory()`.
  Opening the panel resets it to the Now tab (`popoverWillOpen`), so the
  destination is held in `MenuBarDestinationRequests` and taken by that
  reset (`MenuBarPopoverNavigator.resetForPopoverOpening(requested:)`); a
  panel that is already open takes it at once.

## Notification Tap → Popover Scroll-to-Date

```swift
@MainActor
public final class NotificationResponseRouter: NSObject, UNUserNotificationCenterDelegate {
    public init(
        openPopover: () -> Void,
        scrollToDate: ScrollToDateAction,
        isDriftCardInFront: () -> Bool,
        openNeedsACategory: () -> Void,
        reporter: any NotificationDeliveryReporting)
    func handle(userInfo: [AnyHashable: Any])  // drift offer: openPopover(); reminder: openNeedsACategory(); else "insight_date": openPopover() + scrollToDate(date)
    func presentationWhileActive(for surface: NotificationDeliverySurface) -> UNNotificationPresentationOptions
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

The needs-a-category reminder (protocol 33) is the worked example of a kind
whose decision lives in Rust: a pulled `ServerMessage` carrying Rust-authored
copy, claimed once in a Rust ledger, and a sibling scheduler method with its own
`userInfo` marker and delivery surface in Swift. For a variant of the insight:

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
