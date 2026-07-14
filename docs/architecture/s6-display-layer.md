# S6 Display Layer

## Scope

The display layer sits between the IPC delivery system and the SwiftUI view
hierarchy. It:

1. Receives `InsightPayload` and `HistoryPayload` from the `serverMessages`
   fan-out relay on `AccountStateManager`.
2. Formats raw payload fields into display-ready strings (dates, durations,
   confidence labels).
3. Maintains `DisplayState` — a three-value enum that drives the view hierarchy.
4. Exposes stable `InsightViewModel` and `HistoryViewModel` instances that
   SwiftUI observes directly.

The display layer is **read-only**. It never makes IPC calls, never writes to
disk, and never modifies insight text.

## DisplayState Model

```
.loading
    │
    ├─ first payload push (insight OR history) ──► .populated(insight:, history:)
    │
    └─ disconnect after at least one .connected ──► .error(String)
           │
           └─ .connected ──► .loading
```

| State | When entered | View rendered |
|---|---|---|
| `.loading` | Application start; after reconnect from `.error` | Skeleton shimmer (placeholder rows + blurred text blocks) |
| `.populated(insight:, history:)` | First payload push from Rust | `InsightCardView` + `HistoryListView` |
| `.error(String)` | Socket disconnect after at least one successful connection | Skeleton + muted `IPCStatusBanner` label |

### Invariants

- **Initial disconnected status is ignored.** `FakeIPCClient` (and the real
  client before the first connection attempt) emits `.disconnected` on startup.
  `ConcreteDisplayDataCoordinator` guards with `hasConnectedAtLeastOnce` so
  that initial status never drives the UI to `.error`.
- **Populated state is sticky on disconnect.** If a disconnect arrives while
  state is `.populated`, the coordinator does nothing — data already on screen
  stays visible.
- **View model instances are stable.** The same `InsightViewModel` and
  `HistoryViewModel` objects live for the coordinator's entire lifetime.
  SwiftUI `@ObservedObject` bindings remain valid across state transitions
  because the enum holds references, not copies.

## View Hierarchy

```
MenuBarPopoverView
└── VelvtPopoverContentView(coordinator: ConcreteDisplayDataCoordinator)
    ├── .loading  →  InsightCardSkeletonView + HistorySkeletonView
    ├── .populated →  InsightCardView(viewModel: InsightViewModel)
    │                 HistoryListView(viewModel: HistoryViewModel)
    └── .error    →  InsightCardSkeletonView + HistorySkeletonView
                     + IPCStatusBanner (muted label, not an alert)
```

`VelvtPopoverContentView` switches on `coordinator.state`. It does not observe
`InsightViewModel` or `HistoryViewModel` directly — it passes them as
dependencies to child views that observe them.

## Adding a New Display Field

To surface a new field from a Rust payload to the UI:

1. **Add the field to the payload type** (e.g., `InsightPayload` in the IPC
   contract layer). This is a Rust-side change; the Swift model is generated or
   updated alongside the contract.

2. **Expose a formatted property on the view model.** In `InsightViewModel` (or
   `HistoryViewModel`/`DaySummaryViewModel`), add a `@Published` property and
   set it inside `update(from:)`:
   ```swift
   @Published public private(set) var myNewField: String = ""

   public func update(from payload: InsightPayload) {
       // existing assignments …
       myNewField = Self.formatMyNewField(payload.myNewField)
       isLoading = false
   }

   static func formatMyNewField(_ raw: SomeType) -> String {
       // formatting logic
   }
   ```
   Make the formatting helper `static` and `internal` so it can be unit-tested
   without instantiating the view model.

3. **Render in the view.** Add `Text(viewModel.myNewField)` (or equivalent) to
   the relevant view. No changes to `DisplayState`, `DisplayDataCoordinating`,
   or `ConcreteDisplayDataCoordinator` are required unless you are adding a
   completely new payload type.

4. **Add a new payload type** (rare). If Rust introduces a third payload kind
   (e.g., `AlertPayload`), add a new case to `ServerMessage`, route it in
   `ConcreteDisplayDataCoordinator.start(serverMessages:connectionStatus:)`, and
   add a corresponding `updateAlert(_:)` method to `DisplayDataCoordinating`.
   Extend `DisplayState` only if the new data needs its own loading/error
   lifecycle separate from the existing insight+history pair.

5. **Test the formatting helper in isolation**, then test that `update(from:)`
   propagates the value. See `InsightViewModelTests` and `HistoryViewModelTests`
   for the pattern.

## Insight Text Passthrough

Insight text is rendered exactly as received from Rust. The display layer must
not post-process, shorten, emoji-inject, or otherwise alter the string.

| Layer | Behavior |
|---|---|
| `InsightViewModel.update(from:)` | `text = payload.text` — direct assignment, no transformation |
| `InsightCardView` | `Text(viewModel.text).fixedSize(horizontal: false, vertical: true)` — no `.lineLimit`, no `.truncationMode`, no `.clipped` |

Adding any post-processing here violates the privacy contract. Insight copy
belongs to the Rust service; the Swift layer is a transparent display pipe.

## Brand Voice for UI Chrome Copy

These rules apply to labels, banners, and other UI chrome that the Swift layer
authors (not insight text, which is verbatim from Rust):

| Rule | Good | Avoid |
|---|---|---|
| Lowercase, factual | "early data" · "no data" · "ready" | "Early Data" · "No Data Available" |
| Neutral on connection state | "Service unavailable" | "Can't connect!" · "Something went wrong 😞" |
| No gamification | — | "Great work!" · "Streak: 5 days" · "You crushed it" |
| No praise language | — | "Awesome!" · "Congrats!" · "Well done" |
| No score comparisons | show raw number | "Your best score yet!" · "↑ vs yesterday" |
| Muted error framing | `IPCStatusBanner` label only | Full-screen alert for IPC disconnect |

The `IPCStatusBanner` in `VelvtPopoverContentView` is the only error surface the
display layer should show. It renders as a dim `Label` alongside the normal
skeleton shimmer — not a modal, not a sheet, not a prominent warning.

## History Padding

When Rust sends fewer summaries than the requested day count (e.g., a new user
on day 2 with a 7-day window), `HistoryViewModel.update(from:)` pads the front
of the list with synthetic `no_data` rows dated chronologically before the
earliest real summary. This ensures the history list always shows exactly
`payload.days` rows.

Padding is computed by `HistoryViewModel.padded(_:toCount:)` (internal, testable
statically). If the payload has zero summaries, no padding occurs — there is no
anchor date to compute backward from.

## Threading Model

All display-layer types are `@MainActor`-isolated:

- `ConcreteDisplayDataCoordinator`: `@MainActor ObservableObject`
- `InsightViewModel`: `@MainActor ObservableObject`
- `HistoryViewModel`: `@MainActor ObservableObject`

Combine subscriptions in `ConcreteDisplayDataCoordinator.start(…)` use
`.receive(on: RunLoop.main)` to hop to the main run loop before calling view
model methods.

## Testability

`DisplayDataCoordinating` is a protocol, letting tests substitute
`ConcreteDisplayDataCoordinator` or a fake. Tests use two access patterns:

| Pattern | Use case |
|---|---|
| **Direct call** (`sut.updateInsight(_:)`) | Isolating view model formatting, state transitions, and field values — no IPC or Combine machinery needed |
| **IPC injection** (`client.inject(.insightPayload(…))`) | Verifying end-to-end routing from `FakeIPCClient` through `AccountStateManager.serverMessages` to coordinator |

For IPC-injection tests, bind `AccountStateManager` to a named variable (not
`_`) to keep it alive for the test's scope. Its internal `Task` loop exits when
the manager is released, which would prevent messages from reaching
`serverMessages`.
