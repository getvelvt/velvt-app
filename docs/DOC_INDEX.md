# Documentation Index

- [Focus Fragmentation and Daily Activity architecture](architecture/focus-activity-surfaces.md)

Use this file to locate the correct documentation file before making edits.
Open the relevant file, make your changes, and do not modify files unrelated to your task.

| Topic / Area | File Path | Description |
|---|---|---|
| Monorepo overview | `docs/architecture.md` | High-level structure, subproject relationships, privacy boundary, and data flow |
| Quickstart | `docs/quickstart.md` | Prerequisites, install steps, build commands, tests, and local run paths |
| Alpha cohort participant path | `plan/04-alpha-cohort-kit.md` in the private Velvt workspace (not in this repository) | **Canonical** for the first cohort: build choice, qualifier, consent, install and update messages, making the nudge fire, and what to collect |
| Private-beta guide (superseded) | `docs/private-beta-guide.md` | Historical 0.1.5 participant guide; superseded by the workspace cohort kit on 2026-09-25 |
| Closed-alpha release plan (superseded) | `docs/closed-alpha-release-plan.md` | Historical facilitation script; superseded by the workspace cohort kit on 2026-09-25 |
| Shipping a testable DMG | `docs/shipping-a-testable-dmg.md` | Developer ID signing, notarization, `make alpha-dmg`, clean-machine check, and testing that notifications reach the user |
| Contribution workflow | `docs/contributing.md` | Documentation-aware contribution checklist and review expectations |
| Toolchain pins and lint gates | `docs/toolchains-and-lint.md` | Rust, Xcode and Python pins and where each is enforced; what `make lint-rust` and `make lint-swift` check; the swift-format baseline and its follow-up |
| Rust service overview | `docs/rust-service/overview.md` | Purpose and role of the Rust service |
| Rust service internals | `docs/rust-service/architecture.md` | Module structure, startup path, persistence, abstraction, delivery, and lifecycle decisions |
| Rust service API | `docs/rust-service/api.md` | IPC contract, cloud HTTP interfaces, message examples, and validation rules |
| Rust service auth | `docs/rust-service/auth.md` | Auth state machine, token handling, device registration, refresh, and revocation |
| Swift client overview | `docs/swift-client/overview.md` | Purpose and role of the macOS app |
| Swift client architecture | `docs/swift-client/architecture.md` | SwiftUI/AppKit structure, composition root, event capture, IPC, and state flow |
| Swift client settings | `docs/swift-client/settings.md` | Settings UI, persisted local preferences, menu status, and configuration sources |
| Swift client auth | `docs/swift-client/auth.md` | Auth UI flow, session state, Keychain persistence, and IPC auth messages |
| IPC contract deep dive | `docs/architecture/ipc-contract.md` | IPC framing, versioning, direction lists, and the message catalog, reconciled through protocol 38 |
| Classification v2 contract | `docs/classification-v2-contract.md` | The design behind bundle-keyed corrections, declared app metadata, triage, and the protocol-30 classifier ladder |
| Event relay deep dive | `docs/architecture/event-relay.md` | Existing detailed guide for Swift event buffering and reconnect behavior |
| Collection agent deep dive | `docs/architecture/collection-agent.md` | Existing detailed guide for macOS Accessibility event collection |
| Auth and onboarding deep dive | `docs/architecture/auth-onboarding.md` | Existing detailed guide for onboarding and authentication behavior |
| Menu bar and notifications deep dive | `docs/architecture/s7-menu-bar-and-notifications.md` | Existing detailed guide for menu bar presentation and notification delivery |
| Release readiness | `docs/macos-signing-and-accessibility.md` | Distribution signing, notarization, clean-Mac acceptance, hosted-backend smoke, checksum, and rollback handoff |
| Release-readiness decision | `docs/release-readiness/RELEASE_READINESS_REPORT.md` | Integrated ship/no-ship decision, evidence, release gates, and specialist audit references |
| Owner shipment checklist | `docs/release-readiness/WHAT_YOU_MUST_DO_TO_SHIP.md` | Only the credentials, authority, hardware, consent, and policy actions that require the release owner |
| Release ledger | `docs/RELEASES.md` | Every notarized release with build, protocol, source commit or tag, DMG sha256 and recipients; how to cut the next one |
| Secure application updates | `docs/updates.md` | Updater architecture, trust model, activation checklist, publishing order, and N-to-N+1 verification |
| Field and retention inventory | `docs/field-inventory.md` | Column classes (no free text), retention and destructive-action reach for tables that hold a person's own answers; checked with every action's reach by `rust-service/tests/field_inventory.rs` |
| Local meaningful-work loop | `docs/architecture/work-block-loop.md` | Work-block ownership, state machine, evidence rules, classification provenance and the golden replay that holds decisions fixed, privacy field table, lifecycle, and failure behavior |
| Vault v1 wire formats | `docs/architecture/vault-v1-formats.md` | Byte formats of the Sync v1 vault as built in `rust-service/src/vault` (keys, sealed document, signed statements, setup code, person keys, HLC merge), the published test vectors, and fuzzing |
| Vault v1 sync engine | `docs/architecture/vault-v1-sync.md` | `vault::sync` as built (PR A3): the trust anchor and session in `vault_state`, what a Mac believes of velvt-core (§6.1), the state machine, compare-and-swap writes, held and applied registers, security notices held during a work block, account events, the C2 contract it was tested against, and what protocol 38 (A4) wires |
