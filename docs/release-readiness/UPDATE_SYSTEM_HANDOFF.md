# Update-System Engineering Handoff

Date: 2026-07-22
Scope: Velvt macOS whole-bundle updates, signing, publishing, and release gates.

## 1. Verdict

**Secure updater implementation: PASS for source and local packaged-tool
verification. Production activation: P0 blocked.** Velvt now embeds exact-pinned
Sparkle 2.9.4, retains `SPUStandardUpdaterController`, and exposes a manual
**Check for Updates…** action. Production configuration fails closed unless the
feed is HTTPS, the Ed25519 public key decodes to 32 bytes, signed feeds and
pre-extraction verification are required, signature-failure expiry is zero, and
system profiling is disabled.

The release pipeline signs nested Sparkle code inside-out, preserves the
Downloader entitlement required by Sparkle's manual-signing guidance, signs and
notarizes the app before creating the update ZIP, generates the archive and
signed feed with checksum-pinned Sparkle 2.9.4 tools, cryptographically verifies
the generated feed, rejects downgrade/same-build candidates, and documents
archive-first/appcast-last publication.

Production remains blocked until real Developer ID/notary credentials, the
production HTTPS host and Ed25519 key, and two immutable notarized versions are
available for the N-to-N+1/adversarial matrix.

## 2. Evidence and commands

- `swift test --package-path swift-client`: **434 executed, 1 skipped, 0
  failures**, including 10 updater tests.
- Enabled Release `xcodebuild`: passed with version `0.2.0` / build `2`, exact
  Sparkle 2.9.4, universal Sparkle binary, HTTPS feed, matching 32-byte test
  public key, signed-feed enforcement, pre-extraction verification, expiry `0`,
  and profiling disabled.
- `scripts/sign_release.sh local <Release Velvt.app>`: passed for the app,
  embedded Rust helper, framework, Updater app, Autoupdate tool, and both XPC
  services. `verify_release.sh --mode local` passed for the host-supported
  x86_64 app/helper architecture.
- `make test-update-release`: both release-script suites passed, including
  downgrade/equal-build, candidate mismatch, HTTP URL, tampered archive,
  insecure/private-key placement, output immutability, version propagation,
  exact top-level `<sparkle:version>`, and real signed-feed serialization
  regressions.
- Real pinned Sparkle 2.9.4 tools were exercised against the locally signed
  Release candidate. `generate_appcast -o` produced an archive signature and
  trailing signed-feed block; `sign_update --verify --ed-key-file` passed;
  `verify_update_readiness.sh` accepted candidate build 2 over previous build 1
  and verified archive checksum, byte length, host, feed signature structure,
  and candidate/app equality.
- `bash -n` and `git diff --check`: passed.

The real-tool run used a temporary local key and `example.invalid` feed. It is
not production credential or publication evidence.

## 3. Files changed

- `velvt-app/swift-client/Package.swift`
- `velvt-app/swift-client/Package.resolved`
- `velvt-app/swift-client/VelvtMac.xcodeproj/project.pbxproj`
- `velvt-app/swift-client/Configs/{Debug,Release}.xcconfig`
- `velvt-app/swift-client/Sources/VelvtMac/App/UpdateController.swift`
- `velvt-app/swift-client/Sources/VelvtMac/App/{AppModule,MenuBarController}.swift`
- `velvt-app/swift-client/Sources/VelvtMac/UI/MenuBarPopoverView.swift`
- `velvt-app/swift-client/Tests/VelvtMacTests/UpdateControllerTests.swift`
- `velvt-app/Makefile`, `CONFIGURATION.md`, `docs/updates.md`
- `velvt-app/scripts/{create_update_archive,generate_update_appcast,notarize_app,sign_release,verify_release,verify_update_readiness}.sh`
- `velvt-app/scripts/tests/{update_release_scripts_test,verify_update_readiness_test}.sh`

The pre-existing user change in `HistoryListView.swift` was preserved and is not
claimed by this work.

## 4. Findings

### P0

- **Open:** no production Developer-ID-signed/notarized N-to-N+1 update and
  adversarial failure matrix has been executed from `/Applications`.
- **Resolved in implementation:** absent updater; stale proposed 2.9.2 pin;
  disabled production updater wiring; missing version propagation; incorrect
  candidate/previous-build comparison; nested Sparkle signing; unsigned feed
  acceptance; unpinned release tools; incorrect `--output` CLI; enclosure-only
  version parsing; and line-anchored signed-feed parsing.

### P1

- Production host ownership, key custody/backup/rotation, publication rehearsal,
  prior-archive retention, migration recovery, and forward-repair ownership are
  operationally unverified.
- ZIP key-loss recovery is narrower than a Developer-ID-signed DMG when
  pre-extraction verification is enabled; document and test the production
  rotation/recovery policy.

### P2

- The current locally verified Rust helper is x86_64 while the Swift app and
  Sparkle framework are universal. Production must declare and satisfy its
  supported architecture set.

## 5. Verification classification

- **Verified in locally packaged app:** framework embedding, plist controls,
  controller/UI wiring, local nested signing, release version propagation.
- **Verified with real local Sparkle tools:** update ZIP inspection/signing,
  signed-feed generation and cryptographic verification, readiness/version
  gates.
- **Implemented but production-unverified:** Developer ID signing, Apple
  notarization/stapling, immutable production publication.
- **Proposed only / externally blocked:** clean-machine production N-to-N+1,
  tamper/offline/interruption/delta/recovery matrix and local-state/TCC
  preservation proof.

## 6. Open blockers

1. Developer ID Application identity and Apple notary profile.
2. Production Sparkle Ed25519 key with approved custody and rotation owners.
3. Approved HTTPS feed/archive host and immutable publication mechanism.
4. Two increasing notarized releases and a clean installation for the complete
   matrix in `velvt-app/docs/updates.md`.

## 7. Confidence

**High (0.95)** for app integration and local release-tool correctness after
using the real pinned 2.9.4 binaries. **No claim** is made that production
updates work until the credentialed packaged matrix passes.
