# Menu-Only Application Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Velvt a fully functional menu-bar-only macOS application, including onboarding, settings, local/cloud health, and privacy-safe queued-upload visibility.

**Architecture:** Replace the SwiftUI `WindowGroup` lifecycle with an AppKit application main loop and retain a single `NSStatusItem`/`NSPopover` surface. Add a protocol-v8 `request_menu_status`/`menu_status` exchange: Rust owns `/v1/ready` checks and reads only already-abstracted pending upload events; Swift polls it every minute and renders it in an in-popover settings route.

**Tech Stack:** AppKit, SwiftUI, Combine, XCTest, Tokio, Reqwest, Serde, SQLite, Rust integration tests.

---

### Task 1: Define the privacy-safe menu-status contract

**Files:**
- Modify: `proto/version`, `proto/CHANGELOG.md`
- Create: `proto/schema/request_menu_status.json`, `proto/schema/menu_status.json`
- Modify: `rust-service/shared-types/src/lib.rs`, `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`
- Test: `rust-service/shared-types/tests/dto_round_trip.rs`, `swift-client/Tests/VelvtMacTests/IPCModuleTests.swift`

- [ ] Add a failing round-trip test for `request_menu_status` and a `menu_status` response with a device ID, cloud readiness, and abstract label/category/timestamp queue summaries.
- [ ] Bump the protocol from 7 to 8 in both clients and declare JSON schemas that reject raw app, title, URL, path, and text fields.
- [ ] Implement DTO encoding/decoding and run the focused Rust and Swift IPC tests.

### Task 2: Serve cloud and queue status from Rust

**Files:**
- Modify: `rust-service/src/ipc/router.rs`, `rust-service/src/main.rs`
- Modify: `rust-service/src/persistence/traits.rs`, `rust-service/src/persistence/sqlite.rs`
- Test: `rust-service/tests/ipc_connection.rs`

- [ ] Add a failing router test showing `request_menu_status` invokes `GET /v1/ready`, returns `cloud_ready` only for a `{ "status": "ready" }` 2xx response, and limits summaries to ten safe queued events.
- [ ] Add indexed DAL query support for pending upload-batch event summaries ordered newest-first; no raw event table is queried.
- [ ] Add a `MenuStatusProvider` to the router, backed by the persistent token store device ID, pending batch repository, and existing Reqwest HTTP client.
- [ ] Wire it in `main.rs`, then run `cargo test` and `cargo clippy -- -D warnings`.

### Task 3: Remove standalone Swift windows and place onboarding in the popover

**Files:**
- Modify: `swift-client/Sources/VelvtMac/App/AppModule.swift`, `swift-client/Sources/VelvtMac/App/MenuBarController.swift`
- Modify: `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`, `swift-client/Sources/VelvtMac/UI/OnboardingViews.swift`, `swift-client/Sources/VelvtMac/UI/PermissionViews.swift`
- Test: `swift-client/Tests/VelvtMacTests/MenuBarAccountActionResolverTests.swift`, `swift-client/Tests/VelvtMacTests/UIModuleTests.swift`

- [ ] Add a failing route resolver test: initial onboarding, auth, content, and settings resolve only through the popover.
- [ ] Replace `WindowGroup` with a menu-only AppKit `main`, preserving the `LSUIElement` accessory policy and never opening an `NSWindow`.
- [ ] Render welcome, permissions, sign-in/sign-up, completion, recovery, and settings as popover routes; provide a compact back button between routes.
- [ ] Run the focused UI tests.

### Task 4: Build menu settings and status polling

**Files:**
- Create: `swift-client/Sources/VelvtMac/UI/MenuBarSettingsView.swift`, `swift-client/Sources/VelvtMac/Delivery/MenuStatusViewModel.swift`
- Modify: `swift-client/Sources/VelvtMac/App/AppModule.swift`, `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Test: `swift-client/Tests/VelvtMacTests/MenuStatusViewModelTests.swift`

- [ ] Add failing tests for one-minute refresh scheduling, manual refresh messages, and safe queue formatting.
- [ ] Keep only a colored dot in the main header; move verbose local/cloud labels into Settings → App Info.
- [ ] Add Settings routes for App Info and Queued Events, including independent refresh buttons and a scrollable newest-first list limited to ten entries.
- [ ] Run focused Swift tests.

### Task 5: Add custom icon assets and stabilize permissions/credentials

**Files:**
- Create: `swift-client/Resources/Assets.xcassets/AppIcon.appiconset/*`, `swift-client/Resources/Assets.xcassets/VelvtMenuBarIcon.imageset/*`
- Modify: `swift-client/Sources/VelvtMac/App/MenuBarController.swift`, `swift-client/Resources/Info.plist`, `swift-client/VelvtMac.xcodeproj/project.pbxproj`
- Modify: `swift-client/Sources/VelvtMac/Auth/AuthModule.swift`, `swift-client/Sources/VelvtMac/Permissions/PermissionManager.swift`
- Create: `docs/macos-menu-bar-assets.md`
- Test: `swift-client/Tests/VelvtMacTests/MenuBarIconProviderTests.swift`, `swift-client/Tests/VelvtMacTests/AuthModuleTests.swift`, `swift-client/Tests/VelvtMacTests/PermissionModuleTests.swift`

- [ ] Add failing tests that select the custom template image and verify session restoration uses one stable keychain session record rather than three independent reads.
- [ ] Add a monochrome custom menu image asset and a packaged AppIcon asset; use `NSStatusItem.button.image` with template rendering.
- [ ] Consolidate Swift credential storage to a single stable keychain account and cache the loaded session for the process lifetime. Preserve a migration read path for existing keys.
- [ ] Keep the stable bundle ID and use a configured signing identity for distribution; the popover re-checks AX trust without prompting until the user presses the explicit recovery action.
- [ ] Document icon configuration and run all Swift/Rust tests, formatting, linting, and the Xcode build.
