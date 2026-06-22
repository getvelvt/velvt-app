# Menu Popover Overhaul Implementation Plan

> Historical plan. The current IPC protocol is v10; use repository root
> `README.md` for current run commands.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the menu-bar popover the sole authentication and settings UI, with polished page navigation, live status, and local-only queued-event labels.

**Architecture:** The Swift route view owns headers per route, preserving the existing single `NSPopover`. `menu_status` remains the service-owned 60-second cloud-health poll and queue snapshot; it gains a display-only local event label sourced from the raw-event buffer and explicitly excluded from upload DTOs.

**Tech Stack:** SwiftUI/AppKit, Combine, Rust/Tokio, SQLite/rusqlite, serde JSON Schema.

---

### Task 1: Define local queued-event display metadata

**Files:**
- Modify: `proto/schema/menu_status.json`
- Modify: `rust-service/shared-types/src/lib.rs`
- Modify: `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`
- Test: `rust-service/src/ipc/router.rs`

- [ ] Add a failing Rust assertion that a menu-status snapshot returns the latest local display label while upload batch event serialization retains only abstracted labels.
- [ ] Add `local_label` to the menu-status queued-event schema and shared Swift/Rust DTOs; retain existing abstract `label` for category-safe fallback.
- [ ] Run the focused Rust test and confirm it passes.

### Task 2: Persist local-only event labels safely

**Files:**
- Modify: `rust-service/src/persistence/models.rs`
- Modify: `rust-service/src/persistence/traits.rs`
- Modify: `rust-service/src/persistence/sqlite.rs`
- Modify: `rust-service/src/ipc/router.rs`
- Test: `rust-service/tests/e2e_integration.rs`

- [ ] Add a failing integration test that orders pending events newest-first and returns their local labels in `menu_status`.
- [ ] Add an additive SQLite migration and indexed read query joining pending batch events to local raw-event metadata by event ID; populate this field from the incoming raw event before abstraction.
- [ ] Keep these fields out of `AbstractedEvent`, `BatchEvent`, upload DTOs, logs, and cloud requests; fall back to the abstract label only if no local record exists.
- [ ] Run the focused integration test and the upload privacy tests.

### Task 3: Refine popover navigation and settings pages

**Files:**
- Modify: `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Test: `swift-client/Tests/VelvtMacTests/UIModuleTests.swift`

- [ ] Add failing navigation/status mapping tests for forward/backward routes and Connected/Connecting/Disconnected labels.
- [ ] Make the main page own the Velvt/status header, with colored status text immediately before its dot and Settings as a plain trailing bottom-row button.
- [ ] Make each settings route own a compact back button with an 8-point padded hit target and a trailing title. Use full-width list rows for App Info and Queued Events, a separated destructive Quit button, per-status refresh controls, a live queue count in the queue title, and local display labels in the queue list.
- [ ] Preserve 180ms directional slide transitions and disable them under Reduce Motion.
- [ ] Run focused Swift UI tests.

### Task 4: Verify lifecycle and protocol behavior

**Files:**
- Modify: `swift-client/Tests/VelvtMacTests/DeliveryModuleTests.swift`
- Modify: `rust-service/tests/e2e_integration.rs`

- [ ] Add a failing Swift test that manual refresh and Send All Now issue the expected IPC messages.
- [ ] Confirm Quit still reaches `NSApp.terminate`, which invokes `AppDelegate.applicationWillTerminate` and stops the bundled service process.
- [ ] Run `swift test`, `cargo test`, `cargo fmt --check`, and `cargo clippy -- -D warnings`.
