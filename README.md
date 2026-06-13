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

## R1 IPC Server

The Rust service binds the newline-delimited JSON Unix socket contract defined
in `proto/`. Its default socket path comes from `proto/ipc_socket_path` and can
be overridden with `VELVT_IPC_SOCKET_PATH`. `VELVT_IPC_MAX_ERRORS` controls how
many malformed frames a connection may send before closure, and
`VELVT_LOG_LEVEL` configures the structured tracing filter.

Every connection starts with `server_hello`. The client must reply with a
matching `client_hello`; Rust then sends `acknowledged`. A mismatch produces a
typed `version_mismatch` response and a clean close. No other client message is
decoded into its business DTO before the handshake succeeds.

To add an IPC message type, update the canonical schema and protocol version in
`proto/`, add the tagged DTO variant in `rust-service/shared-types`, then update
the Swift DTO contract and contract tests atomically. The R1 default router does
not enumerate normal post-handshake variants, so adding a DTO variant does not
require modifying existing handler or transport files.

## Architecture

Start with [`docs/architecture/`](docs/architecture/) for architecture and IPC
contract documentation. Contributors must also read [`AGENTS.md`](AGENTS.md)
and [`CONTRIBUTING.md`](CONTRIBUTING.md) before making changes.
