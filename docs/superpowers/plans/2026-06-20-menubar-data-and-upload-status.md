# Menu Bar Data and Upload Status Implementation Plan

> Historical plan. The current IPC protocol is v10; use repository root
> `README.md` for current run commands.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render available authenticated data in the menu bar, show missing insights as an empty state, and expose contextual account and pending-upload controls.

**Architecture:** Swift restores the existing v4 request messages and uses an authentication- and connection-gated loader to request insight, history, and upload status. Rust v8 adds a count-only request that combines the in-memory assembler buffer with persisted pending and failed batches. Swift renders insight, history, account, and pending-upload state independently.

**Tech Stack:** Swift 5.10, SwiftUI/AppKit, Combine, XCTest; Rust stable, Tokio, serde, rusqlite; JSON Schema.

---

## Files

- `proto/version`, `proto/CHANGELOG.md`, `proto/schema/*.json`: v8 source contract.
- `rust-service/shared-types/src/lib.rs`: Rust wire DTOs and version.
- `rust-service/src/upload/{assembly,runtime}.rs`: buffered-event count.
- `rust-service/src/persistence/{traits,sqlite}.rs`: persisted-event aggregate.
- `rust-service/src/ipc/router.rs`: count-only IPC response.
- `rust-service/src/main.rs`: pass the repository to the IPC router.
- `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`: v4 and v8 Swift DTOs.
- `swift-client/Sources/VelvtMac/Delivery/MenuBarDataLoader.swift`: gated request orchestration.
- `swift-client/Sources/VelvtMac/{App,UI,Delivery}/`: wiring and rendering.

### Task 1: Restore v4 data-request support in Swift

**Files:**
- Modify: `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`
- Modify: `swift-client/Tests/VelvtMacTests/IPCModuleTests.swift`

- [ ] **Step 1: Write failing codec tests.**

```swift
func testLatestInsightRequestUsesV4WireShape() throws {
    let request = ClientMessage.requestLatestInsight(.init(date: "2026-06-20"))
    let data = try encoder.encode(request)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    XCTAssertEqual(object["type"] as? String, "request_latest_insight")
}

func testCacheEmptyRoundTrips() throws {
    let message = ServerMessage.cacheEmpty(.init(payloadType: "insight_payload"))
    XCTAssertEqual(try decoder.decode(ServerMessage.self, from: encoder.encode(message)), message)
}
```

- [ ] **Step 2: Run `swift test --package-path swift-client --filter IPCModuleTests/testLatestInsightRequestUsesV4WireShape`; expect a compile failure for the missing variants.**

- [ ] **Step 3: Implement the existing v4 contract.**

```swift
public struct RequestLatestInsight: Codable, Equatable, Sendable { public let date: String }
public struct RequestLatestHistory: Codable, Equatable, Sendable { public let days: Int }
public struct CacheEmpty: Codable, Equatable, Sendable {
    public let payloadType: String
    private enum CodingKeys: String, CodingKey { case payloadType = "payload_type" }
}
// Add requestLatestInsight/requestLatestHistory to ClientMessage and cacheEmpty to ServerMessage.
// Encode/decode discriminators: request_latest_insight, request_latest_history, cache_empty.
```

- [ ] **Step 4: Run `swift test --package-path swift-client --filter IPCModuleTests`; expect PASS.**

- [ ] **Step 5: Commit with `git add swift-client/Sources/VelvtMac/IPC/IPCTypes.swift swift-client/Tests/VelvtMacTests/IPCModuleTests.swift && git commit -m "fix(swift): restore cache request IPC messages"`.**

### Task 2: Define the v8 pending-upload aggregate contract atomically

**Files:**
- Modify: `proto/version`, `proto/CHANGELOG.md`
- Create: `proto/schema/request_pending_upload_count.json`, `proto/schema/pending_upload_count.json`
- Modify: `rust-service/shared-types/src/lib.rs`, `rust-service/shared-types/tests/dto_round_trip.rs`
- Modify: `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`, `swift-client/Tests/VelvtMacTests/IPCModuleTests.swift`

- [ ] **Step 1: Write failing Rust and Swift round-trip tests.**

```rust
#[test]
fn pending_upload_count_round_trips_without_event_fields() {
    let message = ServerMessage::PendingUploadCount(PendingUploadCount { event_count: 12 });
    assert_round_trip(message.clone());
    assert_eq!(serde_json::to_value(message).unwrap(), json!({
        "type": "pending_upload_count", "payload": { "event_count": 12 }
    }));
}
```

```swift
func testPendingUploadCountUsesOnlyAggregateEventCount() throws {
    let message = ServerMessage.pendingUploadCount(.init(eventCount: 12))
    let data = try encoder.encode(message)
    let payload = try XCTUnwrap((JSONSerialization.jsonObject(with: data) as? [String: Any])?["payload"] as? [String: Any])
    XCTAssertEqual(payload, ["event_count": 12])
}
```

- [ ] **Step 2: Run `cargo test --manifest-path rust-service/shared-types/Cargo.toml pending_upload_count_round_trips_without_event_fields` and the Swift test; expect missing-symbol failures.**

- [ ] **Step 3: Bump `proto/version` to `8` and define the new request/response.**

```rust
pub const PROTOCOL_VERSION: u32 = 8;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestPendingUploadCount {}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingUploadCount { pub event_count: u64 }
// Add ClientMessage::RequestPendingUploadCount and ServerMessage::PendingUploadCount.
```

```swift
public struct RequestPendingUploadCount: Codable, Equatable, Sendable {}
public struct PendingUploadCount: Codable, Equatable, Sendable {
    public let eventCount: Int
    private enum CodingKeys: String, CodingKey { case eventCount = "event_count" }
}
```

Each schema permits only `event_count`; append a dated changelog entry stating no event or batch detail may be returned.

- [ ] **Step 4: Run `cargo test --manifest-path rust-service/shared-types/Cargo.toml && swift test --package-path swift-client --filter IPCModuleTests`; expect PASS.**

- [ ] **Step 5: Commit protocol files and both DTO implementations together with `git commit -m "feat(proto): add pending upload count"`.**

### Task 3: Count all unacknowledged events in Rust

**Files:**
- Modify: `rust-service/src/upload/assembly.rs`, `rust-service/src/upload/runtime.rs`
- Modify: `rust-service/src/persistence/traits.rs`, `rust-service/src/persistence/sqlite.rs`
- Modify: `rust-service/src/ipc/router.rs`
- Modify: `rust-service/src/main.rs`
- Modify: `rust-service/tests/upload_batching.rs`, `rust-service/tests/persistence_contract.rs`

- [ ] **Step 1: Write failing count tests.**

```rust
#[test]
fn buffered_event_count_tracks_unflushed_events() {
    let mut assembler = BatchAssembler::new("device-1", 50, Duration::from_secs(60));
    assembler.push(event("one", 0), Utc::now());
    assembler.push(event("two", 0), Utc::now());
    assert_eq!(assembler.buffered_event_count(), 2);
}

#[test]
fn pending_event_count_excludes_sent_and_rejected_batches() {
    // Insert one event into each of pending, failed, sent, and rejected batches.
    assert_eq!(repository.pending_event_count().unwrap(), 2);
}
```

- [ ] **Step 2: Run `cargo test --manifest-path rust-service/Cargo.toml buffered_event_count_tracks_unflushed_events`; expect missing-method failures.**

- [ ] **Step 3: Implement count-only APIs and router response.**

```rust
// assembly.rs
pub fn buffered_event_count(&self) -> u64 { self.events.len() as u64 }
// EventIngestor exposes async buffered_event_count; SharedUploadBatcher locks inner and delegates.
// UploadBatchRepo exposes pending_event_count.
// sqlite.rs executes:
// SELECT COUNT(*) FROM batch_event JOIN upload_batch USING(batch_id)
// WHERE upload_batch.status IN ('pending', 'failed')
```

```rust
ClientMessage::RequestPendingUploadCount(_) => Ok(Some(ServerMessage::PendingUploadCount(
    PendingUploadCount {
        event_count: self.ingestor.buffered_event_count().await
            .saturating_add(self.upload_batch_repo.pending_event_count()?),
    },
))),
```

Extend `R7Router::new` with `Arc<dyn UploadBatchRepo>` and pass `Arc::clone(&upload_batch_repo)` from `main.rs`. The router emits a safe `ErrorResponse` on a persistence failure. It must not materialize, log, or serialize event details.

- [ ] **Step 4: Add a router test with two buffered plus three pending/failed persisted events; assert `event_count == 5`.**

- [ ] **Step 5: Run `cargo fmt --manifest-path rust-service/Cargo.toml --check && cargo test --manifest-path rust-service/Cargo.toml && cargo clippy --manifest-path rust-service/Cargo.toml -- -D warnings`; expect PASS.**

- [ ] **Step 6: Commit with `git commit -m "feat(service): report pending upload event count"`.**

### Task 4: Request data only after authentication and connection

**Files:**
- Create: `swift-client/Sources/VelvtMac/Delivery/MenuBarDataLoader.swift`
- Modify: `swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift`, `swift-client/Sources/VelvtMac/App/AppModule.swift`
- Modify: `swift-client/Tests/VelvtMacTests/DisplayDataCoordinatorTests.swift`
- Create: `swift-client/Tests/VelvtMacTests/MenuBarDataLoaderTests.swift`

- [ ] **Step 1: Write failing loader and partial-content tests.**

```swift
func testAuthenticatedConnectionRequestsAllMenuBarData() async {
    let (loader, client, account) = makeLoader(loggedIn: true)
    client.setConnectionStatus(.connected)
    await eventually { client.sentMessages.contains(.requestLatestHistory(.init(days: 7))) }
    XCTAssertTrue(client.sentMessages.contains(.requestPendingUploadCount(.init())))
    _ = loader; _ = account
}

func testInsightCacheEmptyDoesNotHideSuccessfulHistory() {
    let coordinator = ConcreteDisplayDataCoordinator()
    coordinator.handleCacheEmpty(.init(payloadType: "insight_payload"))
    coordinator.updateHistory(makeHistoryPayload(dayCount: 7))
    XCTAssertEqual(coordinator.insightAvailability, .notGenerated)
    XCTAssertFalse(coordinator.historyViewModel.isLoading)
}
```

- [ ] **Step 2: Run the focused tests; expect missing `MenuBarDataLoader`, `handleCacheEmpty`, and `insightAvailability` APIs.**

- [ ] **Step 3: Implement `MenuBarDataLoader`.**

```swift
@MainActor final class MenuBarDataLoader {
    func start(accountState: AnyPublisher<AccountState, Never>, connectionStatus: AnyPublisher<ConnectionStatus, Never>)
    // Combine latest; send one insight(today), 7-day history, and pending-count request
    // for each transition into (loggedIn, connected). Inject `today` for tests.
}
enum InsightAvailability: Equatable { case loading, available, notGenerated }
```

Route `ServerMessage.cacheEmpty` and `ServerMessage.pendingUploadCount` in `DisplayDataCoordinator`; only an `insight_payload` empty result becomes `.notGenerated`, and `pendingUploadCount.eventCount` updates its published aggregate. Wire and retain the loader in `AppDelegate` after all three dependencies are created.

- [ ] **Step 4: Run `swift test --package-path swift-client --filter MenuBarDataLoaderTests && swift test --package-path swift-client --filter DisplayDataCoordinatorTests`; expect PASS.**

- [ ] **Step 5: Commit with `git commit -m "fix(swift): load menu bar data after authentication"`.**

### Task 5: Render partial data and contextual controls

**Files:**
- Modify: `swift-client/Sources/VelvtMac/UI/VelvtPopoverContentView.swift`, `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Modify: `swift-client/Sources/VelvtMac/App/MenuBarController.swift`, `swift-client/Sources/VelvtMac/App/AppModule.swift`
- Modify: `swift-client/Tests/VelvtMacTests/UIModuleTests.swift`, `swift-client/Tests/VelvtMacTests/MenuBarControllerTests.swift`

- [ ] **Step 1: Write failing presentation/action tests.**

```swift
func testMissingInsightUsesEmptyCopyWhileHistoryRemainsVisible() {
    let presentation = MenuBarContentPresentation(insightAvailability: .notGenerated, pendingEventCount: 4)
    XCTAssertEqual(presentation.insightMessage, "Not generated yet")
    XCTAssertEqual(presentation.pendingUploadsTitle, "Pending uploads: 4")
}

func testLoggedInAccountActionLogsOut() {
    var calls = 0
    let controller = makeMenuBar(onLogOut: { calls += 1 })
    controller.performAccountAction(for: .loggedIn(userId: "u1"))
    XCTAssertEqual(calls, 1)
}
```

- [ ] **Step 2: Run the focused UI tests; expect missing presentation/action APIs.**

- [ ] **Step 3: Implement the compact views.**

```swift
// Insight empty state
Label("Not generated yet", systemImage: "sparkles")
    .font(.caption).foregroundStyle(.secondary)
// Footer action
Button(isLoggedIn ? "Log Out" : "Sign In or Create Account", action: accountAction)
Button("Pending uploads: \\(pendingEventCount)", action: onShowPendingUploads)
```

`AppDelegate` supplies `onOpenAccount` by unhide/activate of the existing `WindowGroup`; logged-out `PermissionRootView` already selects the auth flow. It supplies `onLogOut` through the existing `AuthViewModel`/account-manager flow. The pending detail displays only the aggregate number and privacy explanation.

- [ ] **Step 4: Run `swift test --package-path swift-client && swift build --package-path swift-client`; expect PASS and `Build complete!`.**

- [ ] **Step 5: Commit with `git commit -m "feat(swift): add contextual menu bar controls"`.**

### Task 6: Final coordinated verification

- [ ] **Step 1: Run `rg -n 'PROTOCOL_VERSION|^8$' rust-service/shared-types/src/lib.rs proto/version`; expect both contract sources to report v8.**
- [ ] **Step 2: Run `cargo fmt --manifest-path rust-service/Cargo.toml --check && cargo test --manifest-path rust-service/Cargo.toml && cargo clippy --manifest-path rust-service/Cargo.toml -- -D warnings && swift test --package-path swift-client && swift build --package-path swift-client`; expect all exit codes 0.**
- [ ] **Step 3: Run `git diff --check` and inspect the new pending schemas and Swift UI for raw-field names. The new response must contain only `event_count`.**
