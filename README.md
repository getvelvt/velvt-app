# Velvt

Velvt is a privacy-first passive productivity intelligence system. A native
macOS client captures app-activation and focused-window events, then sends
those raw events over a local Unix domain socket to a Rust service. The Rust
service owns abstraction, persistence, upload batching, cloud synchronization,
and delivery of ready-to-display insights back to the client.

Raw app names, bundle IDs, window titles, URLs, paths, filenames, contacts, and
other identifying user data never leave the device. The Rust service is the
final privacy enforcement boundary before any cloud request.

## Workspaces

- `swift-client/`: SwiftUI/AppKit menu bar app for event capture, local IPC,
  permissions, notifications, and insight display. It never calls cloud APIs.
- `rust-service/`: Rust local service for IPC ingestion, abstraction, SQLite,
  upload batching, authentication, cloud sync, and insight delivery.
- `proto/`: canonical newline-delimited JSON IPC contract shared by both
  workspaces.
- `cloud/`: reserved for the separately scoped FastAPI backend.

## Build and Test

Prerequisites are a pinned stable Rust toolchain and a macOS Swift toolchain.

```sh
make build-all
make test-all
make lint-rust
make lint-swift
```

Workspace-specific commands are also available:

```sh
make build-rust
make test-rust
make build-swift
make test-swift
```

## Architecture

Start with [`docs/architecture/`](docs/architecture/) for architecture and IPC
contract documentation. Contributors must also read [`AGENTS.md`](AGENTS.md)
and [`CONTRIBUTING.md`](CONTRIBUTING.md) before making changes.
