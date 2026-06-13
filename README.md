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

### Build Both Local Targets

From the repository root, build the Rust service and macOS-only Swift app
without changing either workspace's build configuration:

```sh
make build-all
```

The native macOS application target is `velvt-mac` in
`swift-client/VelvtMac.xcodeproj`. It produces `velvt-mac.app`; SwiftPM remains
the unit-test harness.

To run the SwiftPM development executable against the local service, source the
canonical socket path and protocol version from `proto/`:

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client velvt-mac
```

Build the native app directly with:

```sh
xcodebuild \
  -project swift-client/VelvtMac.xcodeproj \
  -scheme velvt-mac \
  -destination 'generic/platform=macOS' \
  build
```

## macOS IPC Client

The macOS app communicates with the Rust service exclusively through
`IPCClientProtocol`. `UnixSocketIPCClient` is constructed only in the AppKit
composition root; other production modules and tests depend on the protocol or
`FakeIPCClient`.

```swift
let client: any IPCClientProtocol = FakeIPCClient()
try await client.connect()
try await client.send(.errorResponse(...))

for await message in client.incomingMessages {
    // Route the typed server message without inspecting raw event payloads.
}
```

`connectionStatus` publishes `disconnected`, `connecting`, `handshaking`,
`connected`, and reconnect-attempt state for UI observation. Calls to `send`
before a completed handshake throw `IPCError.notConnected`.

### Version Handshake

IPC uses newline-delimited JSON and protocol version `proto/version`:

1. Rust sends `server_hello`.
2. Swift sends `client_hello` with its protocol and application versions.
3. Rust sends `acknowledged` or `version_mismatch`.

Swift does not publish `connected` or permit public sends before
`acknowledged`. A mismatch throws
`IPCError.versionMismatch(expected:got:)`, closes the connection, and stops
reconnect attempts until the application is updated or restarted.

### Socket Path Configuration

Both workspaces use `proto/ipc_socket_path` as the canonical default. The
macOS app receives the runtime path through `VELVT_SOCKET_PATH`; it also
requires `VELVT_PROTOCOL_VERSION` and `VELVT_CLIENT_VERSION`. The IPC client
expands `~` before connecting and reports a missing socket as a typed
`IPCError.socket` while scheduling reconnect.

### Adding an IPC DTO

IPC contract changes are cross-workspace changes:

1. Add or update the closed JSON schema in `proto/schema/` and bump
   `proto/version` when required.
2. Update the Rust tagged enum and DTO in `rust-service/src/ipc/mod.rs`.
3. Add the Swift DTO and its `ClientMessage` or `ServerMessage` tagged-enum
   case in `swift-client/Sources/VelvtMac/IPC/IPCTypes.swift`.
4. Add encode/decode round-trip tests and register the message handler.

Unknown future server discriminators decode as `ServerMessage.unknown(type:)`.
Only the discriminator is retained; unknown payload fields are discarded so
they cannot leak raw values and existing handler switches do not require
exhaustive updates.

## Architecture

Start with [`docs/architecture/`](docs/architecture/) for architecture and IPC
contract documentation. Contributors must also read [`AGENTS.md`](AGENTS.md)
and [`CONTRIBUTING.md`](CONTRIBUTING.md) before making changes.
