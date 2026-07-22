# Core QA and Reliability Handoff

## 1 Verdict

**Verdict: NO-SHIP pending a verified current Release artifact and clean-install evidence.**

The development-mode reliability baseline is strong: all 401 backend tests, all 311 Rust tests, and all 422 executed Swift tests passed. The current Xcode project also builds successfully as a universal Debug app with the embedded Rust helper. However, the only repository-local `dist/Velvt.app` is not a releasable artifact: the repository verifier rejects it because `VelvtBuildConfiguration` is `Debug`. No current artifact was installed into `/Applications`, launched there, or exercised across a real sleep/wake cycle.

Result classifications:

- **Dev-mode only:** backend, Rust, and Swift automated suites; Xcode Debug app build.
- **Packaged-app verified:** code-signature integrity of the pre-existing local app only (`codesign --verify` passed), but the app failed the project release gate and is not accepted as a release package.
- **Implemented-unverified:** none from this workstream.
- **Proposed-only:** rerun the integrated suites against the final Release build, then install and launch the notarized/stapled DMG copy from `/Applications` and perform real permission, restart, and sleep/wake checks.

## 2 Evidence and commands run

Environment:

- macOS 14.4, Xcode 15.3 (15E204a), Swift 5.10, Rust/Cargo 1.74.1, backend venv Python 3.12.13.
- Both repository instruction files were reviewed before testing: `velvt-app/AGENTS.md` and `velvt-core/AGENTS.md`.
- The workspace root is not a Git repository; `velvt-app` and `velvt-core` are separate Git repositories.

Commands and outcomes:

```text
cd velvt-core && make test
401 passed, 1 warning in 68.91s

cd velvt-core && make test-api
77 passed, 1 warning in 27.90s

cd velvt-core && make lint
All checks passed; 182 files already formatted

cd velvt-app/rust-service && cargo test
311 passed, 0 failed (including unit, integration, and doc tests)

cd velvt-app/rust-service && cargo clippy -- -D warnings
Finished successfully

cd velvt-app/rust-service && cargo fmt --check
Finished successfully

cd velvt-app && CLANG_MODULE_CACHE_PATH="$PWD/swift-client/.build/clang-module-cache" \
  swift test --package-path swift-client \
  --scratch-path "$PWD/swift-client/.build/test-qa" --disable-sandbox
422 executed, 1 opt-in screenshot test skipped, 0 failures in 12.526s

cd velvt-app && xcodebuild \
  -project swift-client/VelvtMac.xcodeproj \
  -scheme velvt-mac -configuration Debug \
  -destination 'generic/platform=macOS' \
  -derivedDataPath /private/tmp/velvt-qa-derived-20260722 \
  VELVT_API_BASE_URL='http://localhost:8000' build
** BUILD SUCCEEDED **

cd velvt-app/swift-client && swift format lint --recursive Sources Tests
error: unable to invoke subcommand: swift-format (No such file or directory)

cd velvt-app && codesign --verify --deep --strict --verbose=2 dist/Velvt.app
dist/Velvt.app: valid on disk; satisfies its Designated Requirement

cd velvt-app && ./scripts/verify_release.sh dist/Velvt.app
ERROR: artifact configuration is 'Debug', expected Release.
```

The backend warning in both pytest runs is a dependency deprecation emitted from `fastapi/testclient.py`: Starlette's current `httpx` integration is deprecated in favor of `httpx2`.

## 3 Files changed

- `CORE_QA_HANDOFF.md` — this evidence handoff.
- No production code or test files changed.
- Pre-existing user change preserved: `velvt-app/swift-client/Sources/VelvtMac/UI/HistoryListView.swift` was already modified and was not edited by this workstream.
- Test/build outputs were written only to ignored Swift build storage and `/private/tmp/velvt-qa-derived-20260722`.

## 4 Tests added/executed

No new tests were added. Existing suites already provide direct automated coverage for the requested reliability areas, and no reproducible defect justified a speculative patch.

| Area | Evidence | Classification |
|---|---|---|
| Launch/startup | Rust `startup_hardening`, service lifecycle, missing socket, readiness wait; fresh Xcode project build | Dev-mode only |
| Onboarding | Swift first-run, completion persistence, migration, replay, permission sequencing | Dev-mode only |
| Permissions | Accessibility grant/denial/revocation/regrant, notifications statuses, foreground refresh | Dev-mode only; system APIs mocked |
| Persistence/event integrity | SQLite repository contracts, transaction rollback, indexed queries, auth snapshot, metrics/onboarding persistence | Dev-mode only |
| Restart/reconnect | Pending upload replay, in-flight shutdown replay, auth restoration, event relay restart, IPC reconnect | Dev-mode only |
| Sleep/wake | Rust work-block pause/wake semantics, immediate upload flush, Swift event-driven lifecycle commands and waking UI state | Dev-mode only; no physical sleep cycle |
| Malformed data | IPC malformed frames, parser failures, unknown messages, malformed auth refresh, raw-field rejection | Dev-mode only |
| Migrations | Idempotent/current schema migration, v8-to-current work-block migration with row preservation, missing DB creation | Dev-mode only |
| Failure recovery | Network retries, persisted backoff, device revocation/reissue, service disconnect, cache preservation, retention under slow DB | Dev-mode only |
| Packaged app | Existing app's signature is internally valid, but release verifier rejects Debug configuration | Packaged-app checked and failed release gate |

## 5 Findings P0-P3

### P0

- None observed in the executed automated suites.

### P1

- **No acceptable current release artifact.** `./scripts/verify_release.sh dist/Velvt.app` exits 1 because the artifact reports `VelvtBuildConfiguration=Debug`. Release status: **verified failure in packaged app**. A fresh signed/notarized/stapled Release artifact must replace it and pass `verify_release.sh` before ship.
- **Clean-install behavior remains unverified.** No DMG-to-`/Applications` install, first launch, real Accessibility/Notification consent, relaunch, or actual sleep/wake cycle was executed. Status: **proposed-only release gate**, pending the distribution workstream's final artifact and installation authority.

### P2

- **Swift format gate cannot run on this machine.** `swift format` fails because the `swift-format` subcommand is absent. This is a reproducibility/toolchain gap, not a source failure. Status: **verified tooling failure**.
- **Swift 6 migration warnings.** Swift test compilation repeatedly warns that multiple `@MainActor` XCTestCase subclasses have actor isolation differing from their nonisolated superclass; the compiler states this becomes an error in Swift 6. Status: **verified warning in dev mode**.

### P3

- **Backend test dependency deprecation.** All backend tests pass, but FastAPI/Starlette emits one warning that the current `httpx` TestClient integration is deprecated. Status: **verified warning in dev mode**.
- **Xcode build-phase dependency warnings.** Three run-script phases execute every build because they do not declare outputs. This affects incremental build efficiency, not runtime correctness. Status: **verified warning in dev mode**.
- **Opt-in screenshot test not executed.** `Scope3SyntheticSnapshotTests` skipped because `VELVT_SCOPE3_SCREENSHOT_DIR` was unset. Status: **verified skip**, non-blocking for reliability.

## 6 Open questions/blockers

- Distribution must provide the final Release app/DMG after signing, notarization, and stapling; QA must rerun `verify_release.sh` against that exact artifact.
- A clean install into `/Applications` and real UI/system-permission exercise need interactive macOS access. Those checks must include denial, later grant, permission revocation, app relaunch, service recovery, and one real sleep/wake cycle.
- The final artifact should be tested on both Apple Silicon and Intel, or the release owner should document why one architecture cannot be exercised. The Xcode Debug build produced a universal Swift binary, while the pre-existing `dist/Velvt.app` inspected before rejection was thin x86_64.
- Confirm whether CI installs `swift-format`; if not, the documented `lint-swift` gate is currently non-reproducible.
- No live backend/APNs environment or credentials were used; cloud outage and push delivery behavior is verified through deterministic test doubles only.

## 7 Confidence

**High (0.93)** for development-mode suite/build results and the rejection of the current Debug artifact. **Medium (0.60)** for end-user reliability because real TCC prompts, `/Applications` launch, physical sleep/wake, notarization/Gatekeeper, live backend/APNs, and version-to-version update behavior were outside the available verified path.
