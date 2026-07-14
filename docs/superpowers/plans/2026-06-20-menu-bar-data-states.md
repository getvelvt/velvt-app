# Menu Bar Data States Implementation Plan

> Historical plan. The current IPC protocol is v10; use repository root
> `README.md` for current run commands.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render useful authenticated menu-bar content when either daily endpoint has no generated payload, while exposing contextual account actions.

**Architecture:** Keep the protocol unchanged: the Rust service already maps unavailable insight and history responses to `cache_empty`. Extend the Swift display coordinator to treat either empty response as a terminal response, maintain availability independently for insight and history, and render each section independently. The menu-bar popover receives account state and IPC dependencies to show sign-in/sign-up or logout controls.

**Tech Stack:** Swift 5.10, SwiftUI, Combine, XCTest, existing Unix-socket IPC contract.

---

### Task 1: Make empty delivery responses terminal UI states

**Files:**
- Modify: `swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift`
- Modify: `swift-client/Tests/VelvtMacTests/DisplayDataCoordinatorTests.swift`

- [ ] **Step 1: Write failing tests**

```swift
func testEmptyInsightTransitionsToPopulatedWithNotGeneratedAvailability() {
    let sut = ConcreteDisplayDataCoordinator()
    sut.handleCacheEmpty(CacheEmpty(payloadType: "insight_payload"))
    XCTAssertEqual(sut.insightAvailability, .notGenerated)
    if case .populated = sut.state {} else { XCTFail("Expected populated state") }
}

func testEmptyHistoryTransitionsToPopulatedWithNotGeneratedAvailability() {
    let sut = ConcreteDisplayDataCoordinator()
    sut.handleCacheEmpty(CacheEmpty(payloadType: "history_payload"))
    XCTAssertEqual(sut.historyAvailability, .notGenerated)
    if case .populated = sut.state {} else { XCTFail("Expected populated state") }
}
```

- [ ] **Step 2: Run the focused test target and confirm the missing history availability fails**

Run: `swift test --package-path swift-client --filter DisplayDataCoordinatorTests`

Expected: compilation failure because `historyAvailability` does not exist, then test failure because `cache_empty` does not transition the display state.

- [ ] **Step 3: Add independent insight/history availability and route both supported `cache_empty` payload types**

```swift
public enum DeliveryAvailability: Equatable {
    case loading
    case available
    case notGenerated
}

public func handleCacheEmpty(_ payload: CacheEmpty) {
    switch payload.payloadType {
    case "insight_payload": insightAvailability = .notGenerated
    case "history_payload": historyAvailability = .notGenerated
    default: return
    }
    transitionToPopulatedIfNeeded()
}
```

- [ ] **Step 4: Re-run the focused test target**

Run: `swift test --package-path swift-client --filter DisplayDataCoordinatorTests`

Expected: PASS.

### Task 2: Render section-level empty states

**Files:**
- Modify: `swift-client/Sources/VelvtMac/UI/VelvtPopoverContentView.swift`

- [ ] **Step 1: Replace the combined populated view branch with independent insight and history sections**

```swift
switch coordinator.insightAvailability {
case .available: InsightCardView(viewModel: insightVM)
case .notGenerated: EmptyDeliveryState(text: "No daily insight generated yet")
case .loading: InsightCardSkeletonView()
}
```

- [ ] **Step 2: Apply the same pattern to history**

```swift
switch coordinator.historyAvailability {
case .available: HistoryListView(viewModel: historyVM)
case .notGenerated: EmptyDeliveryState(text: "No daily history generated yet")
case .loading: HistorySkeletonView()
}
```

- [ ] **Step 3: Build the Swift package**

Run: `swift build --package-path swift-client`

Expected: Build complete.

### Task 3: Add contextual account controls to the popover

**Files:**
- Modify: `swift-client/Sources/VelvtMac/App/AppModule.swift`
- Modify: `swift-client/Sources/VelvtMac/App/MenuBarController.swift`
- Modify: `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Create: `swift-client/Tests/VelvtMacTests/MenuBarAccountActionResolverTests.swift`

- [ ] **Step 1: Write a failing resolver test for logged-out and logged-in actions**

```swift
XCTAssertEqual(
    MenuBarAccountActionResolver.actions(for: .loggedOut),
    [.authenticate(.logIn), .authenticate(.signUp)]
)
XCTAssertEqual(
    MenuBarAccountActionResolver.actions(for: .loggedIn(userId: "user")),
    [.logOut]
)
```

- [ ] **Step 2: Run the focused resolver test and confirm it fails because the resolver is missing**

Run: `swift test --package-path swift-client --filter MenuBarAccountActionResolverTests`

Expected: compilation failure for `MenuBarAccountActionResolver`.

- [ ] **Step 3: Implement the resolver and a compact authentication sheet using the existing `AuthViewModel`**

```swift
case .loggedOut:
    Button("Sign In") { authenticationMode = .logIn; showsAuthentication = true }
    Button("Sign Up") { authenticationMode = .signUp; showsAuthentication = true }
case .loggedIn:
    Button("Log Out", role: .destructive) { authViewModel.logOut() }
```

- [ ] **Step 4: Thread `AccountStateManager` and IPC client from `AppDelegate` through `MenuBarController` into the popover**

```swift
MenuBarController(
    presentation: permissionPresentation,
    displayCoordinator: displayCoord,
    accountStateManager: accountStateManager,
    ipcClient: client,
    connectionStatus: client.connectionStatus
)
```

- [ ] **Step 5: Re-run focused tests**

Run: `swift test --package-path swift-client --filter 'DisplayDataCoordinatorTests|MenuBarAccountActionResolverTests'`

Expected: PASS.

### Task 4: Verify the complete Swift workspace

**Files:**
- Verify only.

- [ ] **Step 1: Run the full package suite**

Run: `swift test --package-path swift-client`

Expected: all tests pass.

- [ ] **Step 2: Build the Xcode target**

Run: `xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -configuration Debug build CODE_SIGNING_ALLOWED=NO`

Expected: `** BUILD SUCCEEDED **`.
