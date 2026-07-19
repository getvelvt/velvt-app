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

Each xcconfig defines seven variables:

| Variable | Description |
|---|---|
| `VELVT_API_BASE_URL` | Base URL for the cloud API (consumed by Rust, present in plist for reference) |
| `VELVT_APNS_ENV` | `development` or `production` — selects APNs push environment |
| `VELVT_SOCKET_PATH` | Unix domain socket path; must match `proto/ipc_socket_path` |
| `VELVT_PROTOCOL_VERSION` | Integer protocol version; must match `proto/version` |
| `VELVT_CLIENT_VERSION` | Set to `$(MARKETING_VERSION)` — stays in sync with `CFBundleShortVersionString` automatically |
| `VELVT_DISTRIBUTABLE` | `YES` only for Release; activates hosted-endpoint preflight |
| `VELVT_BUILD_CONFIGURATION` | Artifact marker verified as `Release` before distribution |

These are promoted to `Info.plist` keys (`VelvtAPIBaseURL`, `VelvtAPNSEnv`,
`VelvtSocketPath`, `VelvtProtocolVersion`, `VelvtClientVersion`) via
`INFOPLIST_KEY_*` build settings in the same xcconfig files.
`BundleConfigLoader` reads those keys at startup.

#### Local development (`swift run`)

When running via SwiftPM (`swift run --package-path swift-client velvt-mac`),
there is no processed `Info.plist`, so `BundleConfigLoader` has nothing to
read. The `#if DEBUG` branch selects `EnvironmentConfigLoader` instead, which
reads:

```sh
VELVT_SOCKET_PATH="$(cat proto/ipc_socket_path)" \
VELVT_PROTOCOL_VERSION="$(cat proto/version)" \
VELVT_CLIENT_VERSION="0.1.0" \
swift run --package-path swift-client velvt-mac
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

It builds `dist/velvt-mac.app` with
`VELVT_API_BASE_URL=http://localhost:8000`. Override
`VELVT_LOCAL_API_BASE_URL` only when your local API is listening elsewhere.
The target signs the app ad-hoc by default. Set
`VELVT_CODESIGN_IDENTITY="<identity>"` only when you need to test with a real
local development certificate.

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
validity, and absence of preview/debug dylibs. Ad-hoc signing is sufficient for
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
executes `velvt-service --protocol-version` and compares the result with the
app's processed Info.plist, so a stale sidecar cannot hide a binary mismatch.

If the helper becomes unavailable after launch, Settings offers **Restart Local
Service**. This sends a graceful termination, waits briefly, and starts the
same embedded helper again; it does not clear SQLite, Keychain, UserDefaults,
permissions, accounts, or caches.

---

## Credentialed signing and notarization

The Xcode build itself does not require a provisioning profile or Developer ID
certificate for local verification. The packaging target signs the completed
bundle after copying it out of DerivedData and supports ad-hoc signing with
`VELVT_CODESIGN_IDENTITY=-`.

**Before distributing outside of direct Xcode installs**, signing must be
enabled:

1. Run `make package-release VELVT_CODESIGN_IDENTITY="Developer ID Application: …"`.
2. Re-sign with Hardened Runtime and the release entitlements if the selected
   identity policy does not already apply them:

   ```sh
   if [ -n "${EXPANDED_CODE_SIGN_IDENTITY}" ]; then
     codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY}" \
              --options runtime \
              "${RESOURCES_DIR}/velvt-service"
   fi
   ```

3. Archive the verified app, submit it with `xcrun notarytool`, wait for
   acceptance, and staple the ticket. Apple's notarization scanner
   requires every Mach-O to carry a valid Developer ID signature with
   Hardened Runtime — unsigned or ad-hoc-signed helpers will be rejected.

These credential-dependent steps are deferred pending team provisioning. No
local acceptance check requires access to signing or notarization credentials.

---

## Version-bump checklist

When bumping the IPC protocol version or socket path (changes to `proto/`):

1. Update `proto/ipc_socket_path` and/or `proto/version`.
2. Update `VELVT_SOCKET_PATH` and `VELVT_PROTOCOL_VERSION` in both
   `swift-client/Configs/Debug.xcconfig` and `Release.xcconfig`.
3. Update the Xcode scheme environment values if they are present.
4. Follow the existing IPC contract change checklist in `CONTRIBUTING.md`.
