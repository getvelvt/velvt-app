# Menu Popover UI Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the menu popover onboarding/settings experience with in-popover navigation, privacy-safe status data, and a service-owned manual upload flush.

**Architecture:** Keep the existing menu bar popover and content cards. Add a small Swift navigation model and focused settings views, while the Rust IPC router gains a service-owned `flush_upload_queue` command that flushes in-memory events and retries persisted batches before returning a new `menu_status`. Swift never calls cloud endpoints.

**Tech Stack:** Swift 5.10, SwiftUI, Combine, XCTest, Rust stable, Tokio, serde, JSON Schema, existing SQLite upload queue.

---

## File structure

- `swift-client/Sources/VelvtMac/UI/PopoverNavigation.swift`: pure page/direction/connection presentation types that unit tests can exercise without rendering SwiftUI.
- `swift-client/Sources/VelvtMac/UI/SettingsViews.swift`: Settings, App Info, and Queued Events pages plus reusable row/header views.
- `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`: root composition only; keeps insights, inline account sheet, connection/footer, and page transition host.
- `swift-client/Sources/VelvtMac/UI/OnboardingViews.swift` and onboarding-only portions of `PermissionViews.swift`: removed legacy welcome/permission/auth flow.
- `swift-client/Sources/VelvtMac/Auth/AuthModule.swift` and `AuthViewModel.swift`: store authenticated email in Keychain locally and expose it to App Info.
- `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`, `rust-service/shared-types/src/lib.rs`, and `proto/schema/`: atomic version-9 `flush_upload_queue` contract.
- `rust-service/src/upload/runtime.rs` and `rust-service/src/ipc/router.rs`: manual flush behavior and router dispatch.

### Task 1: Add the version-9 flush protocol contract

**Files:**
- Create: `proto/schema/flush_upload_queue.json`
- Modify: `proto/version`
- Modify: `proto/CHANGELOG.md`
- Modify: `rust-service/shared-types/src/lib.rs`
- Modify: `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`
- Test: `rust-service/shared-types/src/lib.rs`
- Test: `swift-client/Tests/VelvtMacTests/IPCModuleTests.swift`

- [ ] **Step 1: Write the Swift wire-shape test before adding the message case.**

```swift
func testFlushUploadQueueUsesEmptyPayloadWireShape() throws {
    let data = try encoder.encode(.flushUploadQueue)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    XCTAssertEqual(object["type"] as? String, "flush_upload_queue")
    XCTAssertEqual(try XCTUnwrap(object["payload"] as? [String: Any]).count, 0)
}
```

- [ ] **Step 2: Run the focused Swift test and confirm it fails because `flushUploadQueue` is absent.**

Run: `swift test --package-path swift-client --filter IPCModuleTests/testFlushUploadQueueUsesEmptyPayloadWireShape`

Expected: compilation failure naming `ClientMessage.flushUploadQueue`.

- [ ] **Step 3: Add the closed-schema request, bump the protocol, and add matching DTO cases.**

```json
{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","required":["type","payload"],"properties":{"type":{"const":"flush_upload_queue"},"payload":{"type":"object","additionalProperties":false}},"additionalProperties":false}
```

```rust
pub const PROTOCOL_VERSION: u32 = 9;

pub struct FlushUploadQueue {}

// ClientMessage variant
FlushUploadQueue(FlushUploadQueue),
```

```swift
case flushUploadQueue

// decode: case "flush_upload_queue": self = .flushUploadQueue
// encode: `flush_upload_queue` with `EmptyPayload()`.
```

Set `proto/version` to `9`, add a Version 9 changelog entry describing the empty client request and refreshed `menu_status` response, and include a Rust serde round-trip test asserting `{"type":"flush_upload_queue","payload":{}}`.

- [ ] **Step 4: Re-run both focused contract suites.**

Run: `swift test --package-path swift-client --filter IPCModuleTests && cargo test --manifest-path rust-service/Cargo.toml shared_types`

Expected: both commands exit 0; the request encodes with no activity fields.

- [ ] **Step 5: Commit the atomic protocol DTO/schema work.**

```bash
git add proto/version proto/CHANGELOG.md proto/schema/flush_upload_queue.json \
  rust-service/shared-types/src/lib.rs swift-client/Sources/VelvtMac/IPC/IPCTypes.swift \
  swift-client/Tests/VelvtMacTests/IPCModuleTests.swift
git commit -m "feat(proto): add upload queue flush request"
```

### Task 2: Make the upload pipeline flush all queued work on demand

**Files:**
- Modify: `rust-service/src/upload/runtime.rs`
- Modify: `rust-service/src/ipc/router.rs`
- Modify: `rust-service/src/main.rs`
- Test: `rust-service/src/upload/runtime.rs`
- Test: `rust-service/src/ipc/router.rs`

- [ ] **Step 1: Add a failing upload-runtime test proving a manual flush drains an under-threshold assembler and resumes persisted work.**

```rust
#[tokio::test]
async fn flush_now_submits_buffered_events_and_resumes_pending_batches() {
    let repository = Arc::new(FakeUploadBatchRepo::with_pending_batch());
    let uploader = FakeBatchUploader::accepting();
    let mut batcher = UploadBatcher::new(
        BatchAssembler::new("device", 50, Duration::from_secs(60)),
        UploadCoordinator::new(Arc::clone(&repository), uploader.clone(), FakePrivacyAlertSink::default()),
    );
    batcher.ingest_abstracted("event-1", &abstracted_event(), 10, Utc::now()).await.unwrap();

    batcher.flush_now().await.unwrap();

    assert_eq!(uploader.upload_count(), 2);
}
```

- [ ] **Step 2: Run the targeted Rust test and confirm it fails because `flush_now` is unavailable.**

Run: `cargo test --manifest-path rust-service/Cargo.toml flush_now_submits_buffered_events_and_resumes_pending_batches`

Expected: compilation failure naming `flush_now`.

- [ ] **Step 3: Implement `flush_now` through the existing serialized batcher and retry code.**

```rust
pub async fn flush_now(&mut self) -> Result<(), CoordinatorError> {
    self.flush_shutdown().await?;
    self.coordinator.resume_pending(
        "1",
        env!("CARGO_PKG_VERSION"),
        &["document:edit".into()],
    ).await?;
    Ok(())
}

pub trait EventIngestor: Send + Sync {
    fn flush_now<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorError>> + Send + 'a>>;
}
```

Implement the trait method by locking the existing `SharedUploadBatcher` mutex. This uses the same repository backoff and rejection handling as scheduled upload; it must not bypass retry restrictions or expose payloads.

- [ ] **Step 4: Write and run a router test that requests `FlushUploadQueue` and receives a refreshed `ServerMessage::MenuStatus`.**

```rust
let response = router.route(ClientMessage::FlushUploadQueue(FlushUploadQueue {})).await.unwrap();
assert!(matches!(response, Some(ServerMessage::MenuStatus(_))));
assert_eq!(ingestor.flush_now_count(), 1);
```

Run: `cargo test --manifest-path rust-service/Cargo.toml flush_upload_queue`

Expected before implementation: router match is non-exhaustive or the fake ingestor has no `flush_now` call.

- [ ] **Step 5: Route the request and return a safe status snapshot after the attempt.**

```rust
ClientMessage::FlushUploadQueue(_) => {
    if let Err(error) = self.ingestor.flush_now().await {
        tracing::warn!(error_code = "manual_upload_flush_failed", error = %error, "manual upload flush failed");
    }
    Ok(Some(ServerMessage::MenuStatus(self.menu_status.snapshot().await)))
}
```

The warning must not interpolate event content. Ensure `main.rs` passes the same shared batcher to the router; do not construct a second batcher or HTTP client.

- [ ] **Step 6: Re-run the focused tests and format the Rust workspace.**

Run: `cargo test --manifest-path rust-service/Cargo.toml flush_now_submits_buffered_events_and_resumes_pending_batches && cargo test --manifest-path rust-service/Cargo.toml flush_upload_queue && cargo fmt --manifest-path rust-service/Cargo.toml --check`

Expected: tests exit 0 and formatter reports no diff.

- [ ] **Step 7: Commit the service behavior.**

```bash
git add rust-service/src/upload/runtime.rs rust-service/src/ipc/router.rs rust-service/src/main.rs
git commit -m "feat(service): flush queued uploads on IPC request"
```

### Task 3: Replace the legacy onboarding path with testable popover navigation primitives

**Files:**
- Create: `swift-client/Sources/VelvtMac/UI/PopoverNavigation.swift`
- Modify: `swift-client/Sources/VelvtMac/UI/PermissionViews.swift`
- Delete: `swift-client/Sources/VelvtMac/UI/OnboardingViews.swift`
- Test: `swift-client/Tests/VelvtMacTests/PopoverNavigationTests.swift`
- Modify: `swift-client/VelvtMac.xcodeproj/project.pbxproj`

- [ ] **Step 1: Write failing tests for page direction and the three connection labels.**

```swift
func testPushingSettingsUsesForwardDirection() {
    XCTAssertEqual(PopoverNavigationDirection.forward, PopoverPage.main.direction(to: .settings))
}

func testReconnectingUsesConnectingPresentation() {
    XCTAssertEqual(ConnectionPresentation(status: .reconnecting).label, "Connecting")
    XCTAssertEqual(ConnectionPresentation(status: .reconnecting).color, .yellow)
}
```

- [ ] **Step 2: Run the focused test and confirm it fails because navigation types do not exist.**

Run: `swift test --package-path swift-client --filter PopoverNavigationTests`

Expected: compilation failure naming `PopoverPage` and `ConnectionPresentation`.

- [ ] **Step 3: Add the minimal navigation and presentation types.**

```swift
enum PopoverPage: Equatable { case main, settings, appInfo, queuedEvents }
enum PopoverNavigationDirection { case forward, backward }

struct ConnectionPresentation: Equatable {
    let label: String
    let color: PopoverIndicatorColor
    init(status: ConnectionStatus) {
        switch status {
        case .connected: label = "Connected"; color = .green
        case .connecting, .handshaking, .reconnecting: label = "Connecting"; color = .yellow
        case .disconnected: label = "Disconnected"; color = .red
        }
    }
}
```

Delete `PermissionRootView`, `OnboardingContainer`, `PermissionOnboardingModel`, onboarding state storage, and `OnboardingViews.swift`; retain `PermissionPresentationModel` only for permission status/recovery. Remove related tests that assert a first-launch onboarding takeover. Add the new Swift file to the Xcode target using the project’s existing source-file entries.

- [ ] **Step 4: Re-run the navigation tests and the permission test class.**

Run: `swift test --package-path swift-client --filter 'PopoverNavigationTests|PermissionModuleTests'`

Expected: all selected tests exit 0; no API routes to the removed onboarding view remain.

- [ ] **Step 5: Commit the navigation primitives and onboarding deletion.**

```bash
git add swift-client/Sources/VelvtMac/UI/PopoverNavigation.swift \
  swift-client/Sources/VelvtMac/UI/PermissionViews.swift \
  swift-client/Sources/VelvtMac/UI/OnboardingViews.swift \
  swift-client/Tests/VelvtMacTests/PopoverNavigationTests.swift \
  swift-client/Tests/VelvtMacTests/PermissionModuleTests.swift \
  swift-client/VelvtMac.xcodeproj/project.pbxproj
git commit -m "feat(mac): remove standalone onboarding flow"
```

### Task 4: Persist the display-only account email locally

**Files:**
- Modify: `swift-client/Sources/VelvtMac/Auth/AuthModule.swift`
- Modify: `swift-client/Sources/VelvtMac/Auth/AuthViewModel.swift`
- Test: `swift-client/Tests/VelvtMacTests/AuthModuleTests.swift`

- [ ] **Step 1: Write failing tests for email persistence after auth and deletion on logout.**

```swift
func testAuthSuccessPersistsPendingEmailForAppInfo() async {
    let (manager, client, keychain) = makePendingAuthentication(email: "person@example.com")
    client.inject(authSuccess())
    await fulfillLoggedIn(manager)
    XCTAssertEqual(keychain.storedValue(for: .accountEmail), "person@example.com")
}

func testLogoutRemovesAccountEmail() throws {
    let manager = makeLoggedInManager(email: "person@example.com")
    manager.logOut()
    XCTAssertThrowsError(try manager.accountEmail())
}
```

- [ ] **Step 2: Run those tests and confirm they fail because `accountEmail` storage is absent.**

Run: `swift test --package-path swift-client --filter 'AuthModuleTests/testAuthSuccessPersistsPendingEmailForAppInfo|AuthModuleTests/testLogoutRemovesAccountEmail'`

Expected: compilation failure naming `.accountEmail` or the new manager API.

- [ ] **Step 3: Store only the pending authentication email in Keychain on successful auth.**

```swift
public enum KeychainKey: String, CaseIterable, Sendable { case accessToken, refreshToken, userId, accountEmail, pendingDeletion }

public func beginAuthentication(email: String) -> Bool {
    pendingAuthenticationEmail = email.trimmingCharacters(in: .whitespacesAndNewlines)
    // retain existing state transition guard
}

public var authenticatedEmail: String? { try? keychain.load(for: .accountEmail) }
```

Have `AuthViewModel` call `beginAuthentication(email:)`. On `authSuccess`, write the pending email alongside existing Keychain credentials; clear it whenever authentication fails, the stream ends, logout, deletion, reauth, or device revocation occurs. Do not add the email to any status IPC DTO or log statement.

- [ ] **Step 4: Re-run focused auth tests.**

Run: `swift test --package-path swift-client --filter AuthModuleTests`

Expected: all auth state and Keychain lifecycle tests exit 0.

- [ ] **Step 5: Commit local account metadata handling.**

```bash
git add swift-client/Sources/VelvtMac/Auth/AuthModule.swift \
  swift-client/Sources/VelvtMac/Auth/AuthViewModel.swift \
  swift-client/Tests/VelvtMacTests/AuthModuleTests.swift
git commit -m "feat(mac): retain authenticated email for app info"
```

### Task 5: Build the settings page hierarchy and root footer

**Files:**
- Create: `swift-client/Sources/VelvtMac/UI/SettingsViews.swift`
- Modify: `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Modify: `swift-client/Sources/VelvtMac/App/MenuBarController.swift`
- Modify: `swift-client/Sources/VelvtMac/App/AppModule.swift`
- Modify: `swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift`
- Test: `swift-client/Tests/VelvtMacTests/MenuBarAccountActionResolverTests.swift`
- Test: `swift-client/Tests/VelvtMacTests/MenuBarControllerTests.swift`
- Modify: `swift-client/VelvtMac.xcodeproj/project.pbxproj`

- [ ] **Step 1: Write failing tests for refresh/flush IPC dispatch and the injectable quit action.**

```swift
func testSendAllNowRequestsFlushThenStatusRefresh() async {
    let client = FakeIPCClient()
    let model = MenuStatusViewModel(ipcClient: client, messages: Empty().eraseToAnyPublisher())
    await model.sendAllNow()
    XCTAssertEqual(client.sentMessages, [.flushUploadQueue, .requestMenuStatus])
}

func testQuitActionTerminatesApplication() {
    var quitCount = 0
    let controller = MenuBarController(
        presentation: makePresentation(),
        displayCoordinator: ConcreteDisplayDataCoordinator(),
        terminateApplication: { quitCount += 1 }
    )
    controller.requestQuit()
    XCTAssertEqual(quitCount, 1)
}
```

- [ ] **Step 2: Run the selected tests and confirm they fail because the actions do not exist.**

Run: `swift test --package-path swift-client --filter 'MenuBarAccountActionResolverTests|MenuBarControllerTests'`

Expected: compilation failures naming `sendAllNow`, `terminateApplication`, or `requestQuit`.

- [ ] **Step 3: Add the UI pages and direct all actions through existing dependencies.**

```swift
// MenuStatusViewModel
public func refresh() { Task { try? await ipcClient.send(.requestMenuStatus) } }
public func sendAllNow() async {
    try? await ipcClient.send(.flushUploadQueue)
    try? await ipcClient.send(.requestMenuStatus)
}
```

`SettingsViews.swift` must provide full-width button rows with trailing `chevron.right`, a padded `chevron.left` button, and a bottom-separated destructive Quit button. App Info receives `Bundle.main` version, `AccountStateManager`, `ConnectionStatus`, and `MenuStatusViewModel`; it maps the current service status through `ConnectionPresentation` and calls only `model.refresh()`. Queued Events limits its data to `status.queuedEvents`, shows `event.label` as the primary text, displays up to ten rows, and calls `sendAllNow()` from a separated footer.

`MenuBarPopoverView` must retain `VelvtPopoverContentView` and the existing inline `MenuBarAuthenticationView`; remove its onboarding conditional. Its header retains `Velvt` and adds the status label plus dot. Its bottom account row places existing Sign In/Sign Up/Log Out controls on the left and a plain `Settings` button on the right. Replace the gear icon and inline `MenuBarSettingsView` with the new page host.

Add `terminateApplication: @escaping () -> Void = { NSApp.terminate(nil) }` to `MenuBarController`, expose `requestQuit()`, and pass it to the popover. `NSApp.terminate` invokes the existing app delegate shutdown path, which disconnects IPC and sends SIGTERM to the bundled service.

- [ ] **Step 4: Add slide transitions that respect Reduce Motion.**

```swift
@Environment(\.accessibilityReduceMotion) private var reduceMotion

private var transition: AnyTransition {
    direction == .forward
        ? .asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .leading))
        : .asymmetric(insertion: .move(edge: .leading), removal: .move(edge: .trailing))
}

withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.18)) { page = destination }
```

Use the direction model from Task 3. Do not apply the transition to insight/history refreshes or authentication sheets.

- [ ] **Step 5: Re-run focused UI/controller tests and build the Swift package.**

Run: `swift test --package-path swift-client --filter 'PopoverNavigationTests|MenuBarAccountActionResolverTests|MenuBarControllerTests|AuthModuleTests|IPCModuleTests' && swift build --package-path swift-client`

Expected: selected tests and package build exit 0.

- [ ] **Step 6: Commit the complete popover UI.**

```bash
git add swift-client/Sources/VelvtMac/UI/SettingsViews.swift \
  swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift \
  swift-client/Sources/VelvtMac/App/MenuBarController.swift \
  swift-client/Sources/VelvtMac/App/AppModule.swift \
  swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift \
  swift-client/Tests/VelvtMacTests/MenuBarAccountActionResolverTests.swift \
  swift-client/Tests/VelvtMacTests/MenuBarControllerTests.swift \
  swift-client/VelvtMac.xcodeproj/project.pbxproj
git commit -m "feat(mac): overhaul menu popover settings"
```

### Task 6: Validate the complete cross-workspace change

**Files:**
- Verify only.

- [ ] **Step 1: Verify protocol schemas and both workspace suites.**

Run: `cargo fmt --manifest-path rust-service/Cargo.toml --check && cargo clippy --manifest-path rust-service/Cargo.toml -- -D warnings && cargo test --manifest-path rust-service/Cargo.toml && swift test --package-path swift-client`

Expected: every command exits 0.

- [ ] **Step 2: Build the macOS app target without code signing.**

Run: `xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme VelvtMac -configuration Debug build CODE_SIGNING_ALLOWED=NO`

Expected: `** BUILD SUCCEEDED **`.

- [ ] **Step 3: Inspect the final change for the privacy and scope requirements.**

Run: `git diff --check HEAD~4..HEAD && rg -n 'URLSession|/v1/ready|flush_upload_queue|raw_title|app_name|window_title' swift-client rust-service proto`

Expected: no whitespace errors; `/v1/ready` appears only in Rust; Swift contains no cloud client; the flush request has an empty payload and no raw activity fields are added to `MenuStatus`.

- [ ] **Step 4: Commit any verification-only project-file adjustments if required.**

```bash
git status --short
git add swift-client/VelvtMac.xcodeproj/project.pbxproj
git commit -m "chore(mac): register popover UI sources"
```
