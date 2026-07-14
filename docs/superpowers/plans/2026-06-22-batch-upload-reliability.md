# Batch Upload Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Send API-conformant event batches, make manual sending bypass retry scheduling, and expose the result to the menu-bar UI.

**Architecture:** Keep internal persistence fields unchanged, but map them into a separate, privacy-safe HTTP request DTO. Return a typed flush result from the event ingestor through the existing IPC response and expose a short-lived result in the Swift view model.

**Tech Stack:** Rust, serde/serde_json, Tokio, Swift, Combine, SwiftUI, XCTest.

---

### Task 1: API request DTO and response semantics

**Files:**
- Modify: `rust-service/src/upload/dto.rs`
- Modify: `rust-service/src/upload/transport.rs`
- Test: `rust-service/tests/upload_batching.rs`

- [ ] **Step 1: Write failing request-shape tests**

Assert that the first request contains exactly `event_id`, `occurred_at`, `abstraction_type`, `abstraction_type_version`, and `payload`, and that `payload` contains only `duration_seconds` and `category`.

- [ ] **Step 2: Run the focused Rust test and verify it fails**

Run: `cargo test --manifest-path rust-service/Cargo.toml upload_batcher_request_shape -- --nocapture`

Expected: FAIL because the current serialized event omits `event_id` and emits internal fields.

- [ ] **Step 3: Implement the external request mapping**

Serialize the endpoint DTO as:

```rust
{
  "event_id": event.event_id,
  "occurred_at": event.occurred_at,
  "abstraction_type": "document:edit",
  "abstraction_type_version": "1",
  "payload": { "duration_seconds": event.duration_seconds, "category": event.category }
}
```

Keep `stable_id`, `label`, and `taxonomy_version` local-only and never serialize them.

- [ ] **Step 4: Accept every successful endpoint response**

Treat any `2xx` response as success, recognizing `status: "duplicate"` as `Duplicate`; preserve explicit handling for `raw_field_rejected` and retryable non-2xx responses.

- [ ] **Step 5: Run focused Rust tests and verify they pass**

Run: `cargo test --manifest-path rust-service/Cargo.toml upload_batcher -- --nocapture`

Expected: PASS.

### Task 2: Manual flush bypasses retry scheduling

**Files:**
- Modify: `rust-service/src/upload/coordinator.rs`
- Modify: `rust-service/src/upload/runtime.rs`
- Test: `rust-service/tests/upload_batching.rs`

- [ ] **Step 1: Write a failing manual-flush test**

Persist a failed batch whose `next_attempt_at` is in the future, invoke the manual flush path, and assert the fake uploader receives it.

- [ ] **Step 2: Run the focused Rust test and verify it fails**

Run: `cargo test --manifest-path rust-service/Cargo.toml manual_flush -- --nocapture`

Expected: FAIL because `resume_pending(Utc::now())` excludes scheduled batches.

- [ ] **Step 3: Add an explicit force-upload path**

Load all pending and failed batches for a manual flush and upload them without applying host-backoff suppression. Automatic submission and retry scans retain existing backoff behavior.

- [ ] **Step 4: Run focused Rust tests and verify they pass**

Run: `cargo test --manifest-path rust-service/Cargo.toml manual_flush -- --nocapture`

Expected: PASS.

### Task 3: Surface manual-flush outcomes to Swift

**Files:**
- Modify: `proto/` only if the existing `ServerMessage::ErrorResponse` cannot carry flush status
- Modify: `rust-service/src/ipc/router.rs`
- Modify: `rust-service/shared-types/src/lib.rs` or equivalent message type definition
- Modify: `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`
- Modify: `swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift`
- Modify: `swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- Test: `rust-service/tests/ipc_connection.rs`
- Test: `swift-client/Tests/VelvtMacTests/UIModuleTests.swift`

- [ ] **Step 1: Write failing IPC and Swift view-model tests**

Assert that a failed manual flush returns a typed error response, and that the Swift model publishes an error rather than silently discarding the failure.

- [ ] **Step 2: Run focused tests and verify they fail**

Run: `cargo test --manifest-path rust-service/Cargo.toml flush_upload_queue -- --nocapture`

Run: `swift test --package-path swift-client --filter MenuStatusViewModel`

Expected: FAIL because the router always returns `MenuStatus` and Swift uses `try?`.

- [ ] **Step 3: Implement outcome propagation**

Return a privacy-safe IPC error for operational flush failures, retain `MenuStatus` for successful processing, and publish a Swift-visible status/error for send failures.

- [ ] **Step 4: Run focused tests and verify they pass**

Run: `cargo test --manifest-path rust-service/Cargo.toml flush_upload_queue -- --nocapture`

Run: `swift test --package-path swift-client --filter MenuStatusViewModel`

Expected: PASS.

### Task 4: Full verification

**Files:**
- Modify only files from Tasks 1–3.

- [ ] **Step 1: Format and validate Rust**

Run: `cargo fmt --check --manifest-path rust-service/Cargo.toml`

Run: `cargo clippy --manifest-path rust-service/Cargo.toml -- -D warnings`

- [ ] **Step 2: Run all affected tests**

Run: `cargo test --manifest-path rust-service/Cargo.toml`

Run: `swift test --package-path swift-client`

- [ ] **Step 3: Inspect the final diff**

Run: `git diff --check && git diff -- rust-service swift-client proto`

Expected: no whitespace errors, no raw fields in the outbound request DTO, and no unrelated-file edits.
