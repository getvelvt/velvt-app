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
| Rust service overview | `docs/rust-service/overview.md` | Purpose and role of the Rust service |
| Rust service internals | `docs/rust-service/architecture.md` | Module structure, startup path, persistence, abstraction, delivery, and lifecycle decisions |
| Rust service API | `docs/rust-service/api.md` | IPC contract, cloud HTTP interfaces, message examples, and validation rules |
| Rust service auth | `docs/rust-service/auth.md` | Auth state machine, token handling, device registration, refresh, and revocation |
| Swift client overview | `docs/swift-client/overview.md` | Purpose and role of the macOS app |
| Swift client architecture | `docs/swift-client/architecture.md` | SwiftUI/AppKit structure, composition root, event capture, IPC, and state flow |
| Swift client settings | `docs/swift-client/settings.md` | Settings UI, persisted local preferences, menu status, and configuration sources |
| Swift client auth | `docs/swift-client/auth.md` | Auth UI flow, session state, Keychain persistence, and IPC auth messages |
| IPC contract deep dive | `docs/architecture/ipc-contract.md` | IPC framing, versioning, direction lists, and the message catalog, reconciled through protocol 31 |
| Classification v2 contract | `docs/classification-v2-contract.md` | The design behind bundle-keyed corrections, declared app metadata, triage, and the protocol-30 classifier ladder |
| Event relay deep dive | `docs/architecture/event-relay.md` | Existing detailed guide for Swift event buffering and reconnect behavior |
| Collection agent deep dive | `docs/architecture/collection-agent.md` | Existing detailed guide for macOS Accessibility event collection |
| Auth and onboarding deep dive | `docs/architecture/auth-onboarding.md` | Existing detailed guide for onboarding and authentication behavior |
| Menu bar and notifications deep dive | `docs/architecture/s7-menu-bar-and-notifications.md` | Existing detailed guide for menu bar presentation and notification delivery |
| Release readiness | `docs/macos-signing-and-accessibility.md` | Distribution signing, notarization, clean-Mac acceptance, hosted-backend smoke, checksum, and rollback handoff |
| Release-readiness decision | `docs/release-readiness/RELEASE_READINESS_REPORT.md` | Integrated ship/no-ship decision, evidence, release gates, and specialist audit references |
| Owner shipment checklist | `docs/release-readiness/WHAT_YOU_MUST_DO_TO_SHIP.md` | Only the credentials, authority, hardware, consent, and policy actions that require the release owner |
| Secure application updates | `docs/updates.md` | Updater architecture, trust model, activation checklist, publishing order, and N-to-N+1 verification |
| Local meaningful-work loop | `docs/architecture/work-block-loop.md` | Work-block ownership, state machine, evidence rules, privacy field table, lifecycle, and failure behavior |
