# Configuration

This document describes how build-time constants flow through Velvt, how to
change them for new environments, and the status of code signing.

---

## How build-time constants are set

### Swift client

Constants are defined in xcconfig files and injected into `Info.plist` at
link time by Xcode. No runtime environment variable reads occur in Release
builds.

| File | Environment |
|---|---|
| `swift-client/Configs/Debug.xcconfig` | Local development, CI Debug |
| `swift-client/Configs/Release.xcconfig` | Distribution / production |
| `swift-client/Configs/Version.xcconfig` | `MARKETING_VERSION` and `CURRENT_PROJECT_VERSION`, included by both; the only place the version lives |

The Makefile reads its default version and build from `Version.xcconfig`, and
`rust-service/build.rs` stamps the helper with the same version, so the app,
its `Info.plist` and `velvt-service --version` agree. The release targets also
stamp the source commit into `Info.plist` as `VelvtSourceCommit` and into the
helper (`velvt-service --source-commit`). See
[`docs/RELEASES.md`](docs/RELEASES.md) for the release ledger and how to cut a
release.

Each xcconfig defines seven core application variables plus the updater build
switch, feed URL, and public key described in the release section below:

| Variable | Description |
|---|---|
| `VELVT_API_BASE_URL` | Base URL for the cloud API (consumed by Rust, present in plist for reference) |
| `VELVT_APNS_ENV` | `development` or `production` — selects APNs push environment |
| `VELVT_SOCKET_PATH` | Unix domain socket path; must match `proto/ipc_socket_path` |
| `VELVT_PROTOCOL_VERSION` | Integer protocol version; must match `proto/version` |
| `VELVT_CLIENT_VERSION` | Set to `$(MARKETING_VERSION)` — stays in sync with `CFBundleShortVersionString` automatically |
| `VELVT_DISTRIBUTABLE` | `YES` only for Release; activates hosted-endpoint preflight |
| `VELVT_BUILD_CONFIGURATION` | Artifact marker verified as `Release` before distribution |

These are written into `Info.plist` keys (`VelvtAPIBaseURL`, `VelvtAPNSEnv`,
`VelvtSocketPath`, `VelvtProtocolVersion`, `VelvtClientVersion`,
`VelvtBuildConfiguration`, `VelvtDistributable`, and the updater keys) by the
target's "Inject Custom Info.plist Keys" run-script phase, from the same
resolved build settings. `INFOPLIST_KEY_*` build settings cannot do this:
Xcode's generator silently drops custom keys. `BundleConfigLoader` reads those
keys at startup.

#### Local development (`swift run`)

When running via SwiftPM (`swift run --package-path swift-client Velvt` — the
SwiftPM executable product is `Velvt`),
there is no processed `Info.plist`, so `BundleConfigLoader` has nothing to
read. The `#if DEBUG` branch selects `EnvironmentConfigLoader` instead, which
reads:

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client Velvt
```

### Rust service

`build.rs` reads `VELVT_API_BASE_URL` and `VELVT_APNS_ENV` from the **build
environment** (not the runtime environment) via `option_env!()` and emits
them as:

- `VELVT_API_BASE_URL_COMPILED`
- `VELVT_APNS_ENV_COMPILED`

`ServiceConfig` consumes `env!("VELVT_API_BASE_URL_COMPILED")`. Standalone
developer service runs may override it explicitly, but `ServiceProcessLauncher`
removes that override for a bundled helper so a distributed app always uses
the endpoint compiled into its signed artifact.

Fallback defaults when neither is set (bare `cargo build` or `cargo test`):

| Constant | Default |
|---|---|
| `VELVT_API_BASE_URL_COMPILED` | `https://api.getvelvt.com` |
| `VELVT_APNS_ENV_COMPILED` | `development` |

The Xcode "Bundle Rust Service" Run Script phase exports these from Xcode's
build settings before calling `cargo build --release`, ensuring the Rust
binary is always compiled with the same constants as the Swift binary.

`build.rs` also reads `VELVT_PEER_SIGNATURE_CHECK` (`enforce` or `off`, default
`off`) and emits `VELVT_PEER_SIGNATURE_CHECK_COMPILED`. It decides whether the
helper checks the code signature of the process that connects to its socket
(see "Socket peer check" below). `scripts/build_rust_helper.sh` sets it from
`VELVT_DISTRIBUTABLE`: `enforce` for a Release app, `off` for a Debug one,
whatever the calling shell had. A bare `cargo build` or `cargo test` is off.

---

## Changing the API base URL for a new environment

### Swift

1. Open `swift-client/Configs/Debug.xcconfig` or `Release.xcconfig`.
2. Change `VELVT_API_BASE_URL = <new-url>`.
3. Clean build (`⇧⌘K` in Xcode or `make clean && make build-swift`).

The new URL is picked up by the Xcode Run Script phase which passes it to
`cargo build --release` as `VELVT_API_BASE_URL`, embedding it in the Rust
binary. No code changes required in either workspace.

For local debugging against the default `velvt-core-api` address, use the
packaging target instead of editing xcconfig files:

```sh
make build-app-local-core
```

It builds `dist/Velvt.app` (Debug) with
`VELVT_API_BASE_URL=http://localhost:8000`. Override
`VELVT_LOCAL_API_BASE_URL` only when your local API is listening elsewhere.
The target signs with `VELVT_CODESIGN_IDENTITY`, which has no default. If that
identity is unset or unavailable the target fails; it no longer falls back to
ad-hoc signing on its own. Pass `VELVT_ALLOW_ADHOC=1` (or
`VELVT_CODESIGN_IDENTITY=-`) to accept a bundle that runs on the build machine
only.

### CI

Set `VELVT_API_BASE_URL` and `VELVT_APNS_ENV` as CI environment variables
before invoking xcodebuild. Xcode's Run Script phase exports all build
settings to the environment, so the variables reach `cargo build` automatically.

For standalone `cargo build` on CI (without Xcode), set them explicitly:

```sh
VELVT_API_BASE_URL=https://api.getvelvt.com \
VELVT_APNS_ENV=production \
cargo build --release
```

The `cargo:rerun-if-env-changed` directives in `build.rs` ensure Cargo
invalidates the cached binary when these values change between builds.

---

## Release packaging and verification

Create the distributable artifact with:

```sh
make package-release VELVT_CODESIGN_IDENTITY=-
make verify-release
```

`package-release` uses Xcode's Release configuration, rejects loopback API
URLs, embeds the release Rust helper and taxonomy, signs the complete bundle,
and runs `scripts/verify_release.sh`. Verification checks the Release marker,
hosted API URL, embedded helper, app/helper protocol match, strict codesign
validity, the socket peer check's signing identifiers and requirements, and
absence of preview/debug dylibs. Ad-hoc signing is sufficient for
this local verification and does not alter the stable `com.velvt.mac` identity.

The explicitly named `make build-app-local-core` command is the only packaging
path intended to embed a localhost backend; it builds Debug and is never a
distribution artifact.

## Bundled helper lifecycle

The app uses `ServiceProcessLauncher` to start the bundled helper directly from
`Contents/Resources/velvt-service`, points it at the bundled taxonomy, and
terminates it gracefully when Velvt quits. Users do not install or start Rust,
Python, Docker, Terminal, or a separate local backend.

The Xcode phase writes `velvt-service.version` and
`velvt-service.protocol-version` beside the helper. Release verification also
executes `velvt-service --protocol-version`, `--version` and `--source-commit`
and compares the results with the app's processed Info.plist, so a stale
sidecar cannot hide a binary mismatch. It asks every slice of the universal
helper, each through `/usr/bin/arch`, so the perl or `arch` first on `PATH`
cannot change which slice answers. Apple Silicon runs both slices, the x86_64
one under Rosetta 2; an Intel Mac cannot run the arm64 one. A local
verification warns about a slice the build Mac cannot run. A production
verification (`alpha-dmg`, `release`) refuses, so tester builds are cut on
Apple Silicon with Rosetta 2, and both targets check for it before they build
(`scripts/check_release_host.sh`); install Rosetta with
`softwareupdate --install-rosetta --agree-to-license`.

### Socket peer check

In a Release build the app and the helper each check the other's code
signature on the socket before anything crosses it, so another process
running as the same person can neither pose as the helper nor connect to it
(`docs/architecture/ipc-contract.md`, "Peer Code-Signature Check"). The app
requires a helper signed `com.velvt.service`, and the helper an app signed
`com.velvt.mac`, each by the team in its own signature, or by identifier alone
when it has no team (an ad-hoc local build). Debug builds skip both checks.

- `scripts/sign_release.sh` signs the helper with
  `--identifier com.velvt.service`. The app keeps its bundle identifier.
- `velvt-service --peer-requirement` prints `none` (a Debug app's helper) or
  the requirement it holds the app to.
- `scripts/verify_release.sh` refuses a Release app whose helper says `none`,
  whose app or helper is signed under another identifier, or whose app or
  helper does not satisfy the requirement the other side checks
  (`codesign --test-requirement`). Its summary prints both requirements.

A refused connection logs an error code only and the app shows the service
as unavailable while it keeps reconnecting.

If the helper becomes unavailable after launch, Settings offers **Restart Local
Service**. This sends a graceful termination, waits briefly, and starts the
same embedded helper again; it does not clear SQLite, Keychain, UserDefaults,
permissions, accounts, or caches.

---

## Credentialed signing and notarization

Use `make dmg` for an ad-hoc local Release/DMG verification build. It is not a
distributable trust artifact. Use the credential-gated `make release` target
for Developer ID signing, notarization, stapling, Gatekeeper verification, and
the final checksum. The production target fails closed and never falls back to
ad-hoc signing.

See `docs/macos-distribution.md` for required variables, outputs, architecture
policy, and the clean-machine gate.

### Secure update publishing

Production release creation also requires exact version/build expectations, a
new immutable update ZIP path, a new appcast path, an HTTPS archive base URL,
Sparkle 2.9.4's tool directory, and an Ed25519 private-key file outside the
repository. The private key must have mode 0400 or 0600. See
`docs/updates.md` for the complete command and archive-first/appcast-last
publishing procedure.

`make test-update-release` runs the release-script fixtures without production
credentials. `make verify-update-release` validates configuration and binds the
appcast to the local ZIP and SHA-256, but live Sparkle verification still
requires the documented packaged N-to-N+1 test.

---

## Version-bump checklist

When bumping the IPC protocol version or socket path (changes to `proto/`):

1. Update `proto/ipc_socket_path` and/or `proto/version`.
2. Update `VELVT_SOCKET_PATH` and `VELVT_PROTOCOL_VERSION` in both
   `swift-client/Configs/Debug.xcconfig` and `Release.xcconfig`.
3. Update the Xcode scheme environment values if they are present.
4. Follow the existing IPC contract change checklist in `CONTRIBUTING.md`.
