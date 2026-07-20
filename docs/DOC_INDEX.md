# Documentation Index

- [Scope 3 Focus/Activity handoff](scope3-minimal-dashboard-handoff.md)
- [Focus Fragmentation and Daily Activity architecture](architecture/focus-activity-surfaces.md)

Use this file to locate the correct documentation file before making edits.
Open the relevant file, make your changes, and do not modify files unrelated to your task.

| Topic / Area | File Path | Description |
|---|---|---|
| Monorepo overview | `docs/architecture.md` | High-level structure, subproject relationships, privacy boundary, and data flow |
| Quickstart | `docs/quickstart.md` | Prerequisites, install steps, build commands, tests, and local run paths |
| Private-beta guide | `docs/private-beta-guide.md` | Canonical participant install, onboarding, privacy, recovery, account, uninstall, support, and limitations path |
| Contribution workflow | `docs/contributing.md` | Documentation-aware contribution checklist and review expectations |
| Rust service overview | `docs/rust-service/overview.md` | Purpose and role of the Rust service |
| Rust service internals | `docs/rust-service/architecture.md` | Module structure, startup path, persistence, abstraction, delivery, and lifecycle decisions |
| Rust service API | `docs/rust-service/api.md` | IPC contract, cloud HTTP interfaces, message examples, and validation rules |
| Rust service auth | `docs/rust-service/auth.md` | Auth state machine, token handling, device registration, refresh, and revocation |
| Swift client overview | `docs/swift-client/overview.md` | Purpose and role of the macOS app |
| Swift client architecture | `docs/swift-client/architecture.md` | SwiftUI/AppKit structure, composition root, event capture, IPC, and state flow |
| Swift client settings | `docs/swift-client/settings.md` | Settings UI, persisted local preferences, menu status, and configuration sources |
| Swift client auth | `docs/swift-client/auth.md` | Auth UI flow, session state, Keychain persistence, and IPC auth messages |
| IPC contract deep dive | `docs/architecture/ipc-contract.md` | Existing detailed guide for IPC framing and protocol versioning |
| Event relay deep dive | `docs/architecture/event-relay.md` | Existing detailed guide for Swift event buffering and reconnect behavior |
| Collection agent deep dive | `docs/architecture/collection-agent.md` | Existing detailed guide for macOS Accessibility event collection |
| Auth and onboarding deep dive | `docs/architecture/auth-onboarding.md` | Existing detailed guide for onboarding and authentication behavior |
| Menu bar and notifications deep dive | `docs/architecture/s7-menu-bar-and-notifications.md` | Existing detailed guide for menu bar presentation and notification delivery |
| Release readiness | `docs/macos-signing-and-accessibility.md` | Distribution signing, notarization, clean-Mac acceptance, hosted-backend smoke, checksum, and rollback handoff |
| Local meaningful-work loop | `docs/architecture/work-block-loop.md` | Work-block ownership, state machine, evidence rules, privacy field table, lifecycle, and failure behavior |
