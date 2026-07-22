# macOS Distribution Engineer Handoff

Audit/build date: 2026-07-22
Host: macOS 14.4, Intel `x86_64`, Xcode 15.3, Swift 5.10
Scope: `velvt-app` packaging, signing, notarization gates, DMG, verification, and distribution documentation

## 1 Verdict

**NO-SHIP for public distribution; local packaging gate passes.**

The actual Release app and drag-to-Applications DMG now build reproducibly and
pass local ad-hoc packaged-artifact verification on this Intel host. The DMG
was installed without overwriting an existing app and launched from
`/Applications/Velvt.app`; the app process was observed and then terminated.

Production release remains blocked because this machine has zero signing
identities and no notarization profile. The new `make release` path fails
closed without those credentials, requires Developer ID Application signing,
hardened runtime, Apple acceptance, stapling, Gatekeeper assessments, and a
post-mutation checksum. No claim of Developer ID signing or notarization is
made.

Evidence labels:

- **Packaged-app verified:** actual `dist/Velvt.app`, mounted
  `dist/Velvt.dmg`, and installed `/Applications/Velvt.app` exercised.
- **Implemented but unverified:** credentialed Developer ID/notary/stapling
  path; no credentials existed.
- **Proposed only:** Intel + Apple Silicon clean-machine matrix and universal
  release policy.

## 2 Evidence and commands run

Run from `/Users/maximkudryashov/Projects/velvt-dev/velvt-app` unless noted.

- Read `velvt-app/AGENTS.md` and `ARCHITECTURE_AUDIT_HANDOFF.md` before edits.
- `security find-identity -v -p codesigning` -> `0 valid identities found`.
- `uname -m; sw_vers` -> `x86_64`, macOS 14.4 (23E214).
- `bash -n scripts/*.sh`; `make -n dmg`; `git diff --check` -> passed.
- `make dmg` -> Xcode Release build succeeded and Rust release helper built;
  sandboxed `hdiutil create` could not access disk-image services, so the exact
  DMG create/verify commands were rerun with approved macOS disk-image access.
- `scripts/sign_release.sh local dist/Velvt.app` -> app and helper ad-hoc signed
  inside-out with hardened runtime; strict deep verification passed.
- `scripts/create_dmg.sh dist/Velvt.app dist/Velvt.dmg` -> UDZO/HFS+ DMG created,
  `hdiutil verify` passed, SHA-256 written.
- `VELVT_RELEASE_ARCHS=x86_64 scripts/verify_release.sh --mode local --app
  dist/Velvt.app --dmg dist/Velvt.dmg` -> passed configuration, HTTPS endpoint,
  helper/taxonomy, protocol 20, no debug dylibs, strict signing, hardened
  runtime, architecture, checksum, disk-image integrity, Applications link,
  and DMG/direct-app binary equality.
- `shasum -a 256 -c dist/Velvt.dmg.sha256` -> OK.
- Clean install guard: `test ! -e /Applications/Velvt.app` passed. Mounted the
  exact DMG read-only, copied its app to `/Applications/Velvt.app`, detached,
  and ran `open /Applications/Velvt.app`. `ps` observed PID 10240 executing
  `/Applications/Velvt.app/Contents/MacOS/Velvt`. The process was then
  terminated with `SIGTERM`; final process inspection was empty. The installed
  copy remains at `/Applications/Velvt.app`; no prior app was overwritten.
- Installed artifact inspection: strict codesign passed; Info.plist reported
  Release and `https://dev-api.getvelvt.com`; embedded helper reported protocol
  20. This launch was local ad-hoc, not a Gatekeeper/notarization acceptance
  test.
- `make release` without credentials -> exit 2 before building:
  `ERROR: set VELVT_CODESIGN_IDENTITY ...`.
- Production verifier against the ad-hoc artifact -> exit 1:
  `ERROR: production app is not signed with a Developer ID Application identity.`

Artifact ledger:

| Artifact | Evidence | SHA-256 / metadata |
|---|---|---|
| `velvt-app/dist/Velvt.dmg` | Packaged-app verified | `992a291a95fa93cdd4104a7e2e6b86a6eb39350a75ce88898ebaf49213b1ecd3`, 6,205,527 bytes |
| `dist/Velvt.app/Contents/MacOS/Velvt` | Packaged-app verified | `a64f9a200059a41301aa0c06acd719af0c943099ae9728f7365068be61660d5b`, thin x86_64 |
| `dist/Velvt.app/Contents/Resources/velvt-service` | Packaged-app verified | `45fcc8d2019bc2a64452eb6db07639c84d3181759d975aeb8b3af2c105c04336`, thin x86_64 |
| App metadata | Packaged-app verified | version 0.1.0 (build 1), Release, protocol 20, dev API |
| App/helper signatures | Packaged-app verified | ad-hoc, hardened runtime, no TeamIdentifier |
| Developer ID/notary result | Not performed | no identity/profile; no `notarization-result.plist` exists |

## 3 Files changed

Distribution-owned changes:

- `velvt-app/Makefile`
- `velvt-app/scripts/sign_release.sh` (new, executable)
- `velvt-app/scripts/create_dmg.sh` (new, executable)
- `velvt-app/scripts/notarize_release.sh` (new, executable)
- `velvt-app/scripts/verify_release.sh`
- `velvt-app/swift-client/Configs/Release.entitlements` (new, reviewed empty set)
- `velvt-app/docs/macos-distribution.md` (new)
- `velvt-app/CONFIGURATION.md`
- `DISTRIBUTION_HANDOFF.md` (this handoff)

The pre-existing/user `HistoryListView.swift` change was preserved. Parallel
changes in Rust delivery, `DisplayDataCoordinator`, its tests, and the docs
index were observed and not edited as part of this track.

Generated/untracked build artifacts are under `velvt-app/dist/`. A clean local
install now also exists at `/Applications/Velvt.app` as explicitly reported
above.

## 4 Tests added or executed

No product unit tests were added because this track owns release tooling, not
business logic. Executed release tests:

| Test | Result | Evidence class |
|---|---|---|
| Xcode Release app build + embedded Cargo release helper | Pass | Packaged-app verified |
| Release config/API/protocol/resources/debug-dylib gate | Pass | Packaged-app verified |
| App + helper hardened-runtime ad-hoc signing | Pass | Packaged-app verified |
| Strict deep code-sign verification | Pass | Packaged-app verified |
| Required `x86_64` architecture | Pass | Packaged-app verified |
| DMG create, checksum, HFS integrity, mount, layout | Pass | Packaged-app verified |
| DMG binaries equal directly verified app binaries | Pass | Packaged-app verified |
| Non-overwriting install into `/Applications` | Pass | Packaged-app verified |
| Launch from `/Applications` and observe app process | Pass | Packaged-app verified |
| Developer ID, Gatekeeper, notary, staple | Blocked by credentials | Implemented but unverified |
| Apple Silicon/universal artifact | Not run on x86_64 host | Proposed only |

The app launched, but this short smoke did not establish permission prompts,
backend connectivity, or a long-lived helper process. Those remain integrated
QA gates.

## 5 Findings ranked P0-P3

### P0

1. **No production signing/notarization evidence.** Zero code-sign identities
   are installed and no notary Keychain profile was provided. The available
   artifact is ad-hoc and cannot ship publicly. The production path is
   implemented but unverified and intentionally fails closed.

### P1

1. **Architecture support is unresolved.** Both app and helper are thin
   x86_64. `VELVT_RELEASE_ARCHS` makes the policy enforceable, but an arm64 or
   universal build and Intel/Apple Silicon clean-machine matrix are absent.
2. **Release endpoint is still the development API.** The locally verified
   Release artifact targets `https://dev-api.getvelvt.com`. The lead must decide
   whether that is the intended closed-alpha channel; a production release must
   pass an explicit production URL.
3. **No notarized clean-machine Gatekeeper/TCC test.** Installation and launch
   worked locally with an ad-hoc artifact, but stable Accessibility identity,
   quarantine/Gatekeeper first launch, macOS 13, and current Apple Silicon are
   not verified.

### P2

1. The DMG provides the conventional app + Applications-link presentation but
   no custom Finder background/window coordinates. This is adequate functionally
   but visually minimal.
2. The current reviewed entitlement set is empty. That matches local
   notifications and an unsandboxed Accessibility observer, but any later APNs,
   sandbox, login item, or other capability must update and re-audit the
   entitlement policy before release.
3. App version remains placeholder-like at 0.1.0 build 1; production release
   manifest/version ownership is not yet demonstrated.

### P3

1. `hdiutil` prints host locale warnings during `shasum`; checksum verification
   still succeeds and the artifact is unaffected.

## 6 Open questions or blockers

1. Provide a valid `Developer ID Application` identity and a `notarytool`
   Keychain profile, then run the exact `make release` gate.
2. Confirm the canonical Release API URL and whether `dev-api` is intentionally
   the closed-alpha endpoint.
3. Declare supported architectures: separate Intel/Apple Silicon artifacts or
   universal. The Rust ONNX feature currently differs by host architecture.
4. Run clean-machine acceptance on macOS 13 and a current macOS release,
   including quarantine, Gatekeeper, Accessibility and Notifications lifecycle,
   helper startup/restart, quit, and relaunch.
5. Decide whether the minimal DMG visual presentation is acceptable or whether
   branded artwork/Finder layout is a launch requirement.
6. `/Applications/Velvt.app` was installed by this track and intentionally not
   deleted. Remove it only with explicit approval if the lead no longer needs
   the integrated packaged-app checks.

Stop conditions honored: no credentials were requested or fabricated, no
external upload/notary call ran, no existing `/Applications` app was
overwritten, and no production release claim was made.

## 7 Confidence

**High (0.94)** for the local app/DMG build, signing flags, checksum, mount,
layout, x86_64 architecture, installed-app launch, and fail-closed credential
gates because they were executed against the actual artifacts.

**Medium (0.70)** for production automation because command structure and
failure gates are implemented and statically checked, but Developer ID signing,
Apple submission, stapling, Gatekeeper, and clean-machine behavior could not be
executed without credentials and additional hardware.

## Skills used

No available skill covered native macOS packaging, code signing, notarization,
security assessment, or release report authoring. Using an unrelated skill
would not advance evidence. Native Xcode, Cargo, `codesign`, `security`,
`hdiutil`, `notarytool`, `stapler`, `spctl`, `shasum`, and repository scripts
were used instead. Resulting evidence/artifacts are the scoped scripts, docs,
Release app, DMG, checksum, installed-app smoke, and this handoff.
