# Quickstart

This guide gets a new developer from a clean checkout to local builds, tests, and a runnable Velvt app.

## Repository Layout

Velvt is a monorepo with two active local workspaces:

| Path | Purpose |
|---|---|
| `swift-client/` | Native macOS menu bar app. Captures local activity, relays raw events to Rust over IPC, renders received insights, manages permissions, notifications, and local session state. |
| `rust-service/` | Local Rust service. Owns IPC server, abstraction, SQLite persistence, upload batching, cloud sync, auth token refresh, and insight/history delivery. |
| `proto/` | Canonical newline-delimited JSON IPC contract shared by Swift and Rust. |
| `cloud/` | Reserved for separately scoped backend work. |
| `docs/` | Developer documentation and subsystem deep dives. |

## Prerequisites

- macOS 13 or later for the Swift client target.
- Xcode with command line tools selected:

  ```sh
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  ```

- Swift 5.10 or later.
- Rust/Cargo. The service toolchain is pinned by `rust-service/rust-toolchain.toml`.
- `make`.

No SwiftPM third-party package is currently declared in `swift-client/Package.swift`. The Rust service uses Cargo dependencies declared in `rust-service/Cargo.toml`, including `tokio`, `serde`, `rusqlite`, `reqwest`, `tracing`, and optional ONNX support behind the `onnx` feature.

## First Build

From the repository root:

```sh
make build-all
```

This builds the Rust service and the macOS Swift target without packaging them into one app bundle.

To build the single local runnable app:

```sh
make build-app
```

This produces:

```text
dist/velvt-mac.app
```

The packaged app embeds the Rust helper at:

```text
dist/velvt-mac.app/Contents/Resources/velvt-service
```

When launched as a packaged app, `ServiceProcessLauncher` starts the embedded Rust service and stops it on app quit.

## Running Locally

### Packaged App Path

Build the app:

```sh
make build-app
```

Then open `dist/velvt-mac.app` from Finder or with an approved local GUI launch flow. On first use, macOS may require Accessibility and Notifications permissions.

### Development Path With Separate Processes

Run the Rust service in one terminal:

```sh
cargo run --manifest-path rust-service/Cargo.toml
```

Run the SwiftPM executable in another terminal:

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client Velvt
```

The Swift debug path can read configuration from environment variables when no processed app bundle `Info.plist` is available. Release app builds use `BundleConfigLoader` and values baked into `Info.plist` from `swift-client/Configs/*.xcconfig`.

The Rust service polls velvt-core for live insights while authenticated. The
cloud base URL defaults to the service build configuration and can be
overridden at runtime with `VELVT_API_BASE_URL`; the long-poll path and timing
can also be adjusted:

```sh
VELVT_API_BASE_URL=http://localhost:8000 \
VELVT_INSIGHT_POLL_PATH=/v1/insights/poll \
VELVT_INSIGHT_POLL_TIMEOUT_SECONDS=30 \
VELVT_INSIGHT_POLL_IDLE_SECONDS=1 \
cargo run --manifest-path rust-service/Cargo.toml
```

When a poll returns a new insight, Rust pushes both `insight_payload` and
`notification_payload` to Swift over IPC. In Swift debug builds, open Settings
from the menu bar popover and use "Simulate Insight" to exercise the same local
notification handler without a velvt-core response.

## Tests

Run all tests:

```sh
make test-all
```

Run Rust tests only:

```sh
make test-rust
```

Equivalent direct command:

```sh
cargo test --manifest-path rust-service/Cargo.toml
```

Run Swift tests only:

```sh
make test-swift
```

Equivalent direct command:

```sh
swift test --package-path swift-client
```

## Lint and Format

Rust:

```sh
cargo clippy --manifest-path rust-service/Cargo.toml -- -D warnings
cargo fmt --manifest-path rust-service/Cargo.toml --check
```

Swift:

```sh
cd swift-client
swift format lint --recursive Sources Tests
```

If `swift format` is not installed on your machine, use the project or CI-provided formatting path before opening a PR.

## Configuration Basics

The canonical IPC socket path is in:

```text
proto/ipc_socket_path
```

The canonical IPC protocol version is in:

```text
proto/version
```

Do not hardcode either value in Swift or Rust. Swift app builds receive them through xcconfig-backed `Info.plist` keys. The Rust service loads the default socket path from `proto/ipc_socket_path` and can be overridden with `VELVT_IPC_SOCKET_PATH`.

Cloud base URL and APNs environment have build-time defaults. The Rust service
also accepts `VELVT_API_BASE_URL` as a runtime override for local velvt-core
testing. See `CONFIGURATION.md` for the full flow.

## Common Failure Modes

| Symptom | Likely Cause | Fix |
|---|---|---|
| Swift app shows disconnected service | Rust service is not running, socket path mismatch, or protocol handshake failed | Confirm the service is running and both workspaces use `proto/ipc_socket_path` and `proto/version` |
| `xcodebuild` fails with missing developer tools | Command Line Tools selected instead of full Xcode | Run `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` |
| Rust startup exits immediately | Invalid service configuration, duplicate socket listener, or failed SQLite/taxonomy initialization | Check safe structured logs for error codes such as `duplicate_service_instance` or `persistence_initialization_failed` |
| Auth-required UI appears | Rust service has no valid device session | Sign in through the menu bar app; Swift sends credentials to Rust over IPC, Rust handles cloud calls |

## Privacy Reminder

Raw app names, bundle IDs, window titles, URLs, paths, filenames, contacts, and raw text are local-only. The Rust service is the privacy enforcement boundary before upload. Never add a path that sends these fields to cloud APIs.
