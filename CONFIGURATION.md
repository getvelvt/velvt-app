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

Each xcconfig defines five variables:

| Variable | Description |
|---|---|
| `VELVT_API_BASE_URL` | Base URL for the cloud API (consumed by Rust, present in plist for reference) |
| `VELVT_APNS_ENV` | `development` or `production` — selects APNs push environment |
| `VELVT_SOCKET_PATH` | Unix domain socket path; must match `proto/ipc_socket_path` |
| `VELVT_PROTOCOL_VERSION` | Integer protocol version; must match `proto/version` |
| `VELVT_CLIENT_VERSION` | Set to `$(MARKETING_VERSION)` — stays in sync with `CFBundleShortVersionString` automatically |

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

`ServiceConfig` consumes `env!("VELVT_API_BASE_URL_COMPILED")` — a
compile-time macro that is locked at link time and cannot be overridden by
any runtime environment variable.

Fallback defaults when neither is set (bare `cargo build` or `cargo test`):

| Constant | Default |
|---|---|
| `VELVT_API_BASE_URL_COMPILED` | `https://staging.api.velvt.test` |
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

## ServiceManager version-check and update loop

### How version comparison works

The Xcode Run Script build phase writes the Rust binary's version (from
`cargo metadata`) to `Contents/Resources/velvt-service.version` alongside
the binary in the app bundle.

On each app launch, `ServiceManager`:

1. Reads `Contents/Resources/velvt-service.version` (the bundled version).
2. Reads `~/Library/Application Support/Velvt/velvt-service.version` (the
   installed version, if present).
3. If the strings differ — or if no installed version file exists — it:
   - Sets state to `.updateInProgress`
   - Calls `SMAppService.agent(plistName:).unregister()`
   - Overwrites the binary atomically (remove → copy → `chmod 0755`)
   - Copies the new version sidecar
   - Calls `SMAppService.agent(plistName:).register()`
   - Sets state to `.running`

### What to do if the update loop fails

`ServiceManager` does not throw — errors set `state = .failed(error)` and
surface `ServiceUnavailableView`. The user sees the error description and a
"Try Again" button that re-runs the same `ensureInstalled → ensureUpToDate →
start` sequence.

Common failure modes:

| Symptom | Likely cause | Fix |
|---|---|---|
| `binaryNotFoundInBundle` | Run Script build phase did not run or `cargo build` failed | Rebuild with `make build-app`; check Xcode build log |
| `versionSidecarNotFoundInBundle` | Run Script phase did not write the version file | Rebuild; check the `cargo metadata` step in the Run Script |
| `templateNotFoundInBundle` | `com.velvt.service.plist.template` not in Resources build phase | Verify the file is in the Xcode Resources build phase |
| SMAppService error | LaunchAgent plist missing or malformed; sandbox restrictions | Check `~/Library/LaunchAgents/com.velvt.service.plist`; verify app is not sandboxed |

If the update loop appears stuck (e.g., versions keep mismatching after a
reinstall), delete the installed sidecar and retry:

```sh
rm ~/Library/Application\ Support/Velvt/velvt-service.version
```

---

## `CODE_SIGNING_ALLOWED = NO` and distribution

Both build configurations currently set `CODE_SIGNING_ALLOWED = NO` in
`project.pbxproj`. This is intentional for local development — it avoids
requiring a provisioning profile or Developer ID certificate on every
developer's machine.

**Before distributing outside of direct Xcode installs**, signing must be
enabled:

1. Set `CODE_SIGNING_ALLOWED = YES` and configure a Developer ID certificate
   in each `XCBuildConfiguration`.
2. The "Bundle Rust Service" Run Script phase must codesign the Rust binary
   with Hardened Runtime enabled. Add this line to the script (after the
   `cp` line), guarded by a signing-identity check:

   ```sh
   if [ -n "${EXPANDED_CODE_SIGN_IDENTITY}" ]; then
     codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY}" \
              --options runtime \
              "${RESOURCES_DIR}/velvt-service"
   fi
   ```

3. Add the `com.apple.smjobbless` entitlement (or confirm sandbox is off)
   so `SMAppService.agent(plistName:).register()` succeeds in a signed build.
4. Notarize the `.app` with `xcrun notarytool`. Apple's notarization scanner
   requires every Mach-O to carry a valid Developer ID signature with
   Hardened Runtime — unsigned or ad-hoc-signed helpers will be rejected.

These steps are deferred pending team provisioning. See `DEFERRED.md`.

---

## Version-bump checklist

When bumping the IPC protocol version or socket path (changes to `proto/`):

1. Update `proto/ipc_socket_path` and/or `proto/version`.
2. Update `VELVT_SOCKET_PATH` and `VELVT_PROTOCOL_VERSION` in both
   `swift-client/Configs/Debug.xcconfig` and `Release.xcconfig`.
3. Follow the existing IPC contract change checklist in `CONTRIBUTING.md`.
