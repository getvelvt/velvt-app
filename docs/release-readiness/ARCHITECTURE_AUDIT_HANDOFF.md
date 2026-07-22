# Repository and Architecture Audit Handoff

Audit date: 2026-07-22
Scope: `velvt-app/` (Swift client, Rust service, IPC protocol, packaging) and `velvt-core/` (FastAPI, PostgreSQL, Temporal, insight/delivery pipeline).
Method: read-only source/configuration inspection plus read-only inspection of the existing packaged artifact. The only file created is this handoff.

## 1 Verdict

**NO-SHIP baseline.** The repository contains a substantial, well-separated three-runtime implementation and broad unit/integration coverage, but the available evidence does not establish a releasable product. There are two independently sufficient P0 blockers:

1. **No distributable macOS release artifact exists in the workspace.** The sole `dist/Velvt.app` is explicitly `Debug`, `VelvtDistributable=NO`, points to `http://localhost:8000`, is ad-hoc signed, has no Developer ID team, fails the repository's release verifier, and has no DMG/notarization/stapling evidence. This is **packaged-app verified**.
2. **Production backend configuration does not fail closed on the known development JWT secret.** `Settings.jwt_secret` defaults to `local-dev-insecure-change-me`, and its only model validator checks modeling thresholds; a production process can therefore start with a forgeable signing key. This is **implemented-unverified** (source-proven; no production environment was mutated).

At minimum, close those P0s and the P1 sleep/activity-integrity, crash recovery, timezone-day-boundary, CI release-signal, updater, and production-deployment validation gaps before reconsidering ship.

### Architecture map

```text
macOS process: SwiftUI/AppKit menu-bar app
  AppDelegate
    -> PermissionManager (Accessibility + Notifications)
    -> AXCollectionAgent (NSWorkspace activation + AX focused-window/title events)
    -> EventRelay (bounded in-memory buffer)
    -> UnixSocketIPCClient (newline-delimited JSON, protocol v20)
    -> AccountStateManager (Keychain-backed session, sole inbound IPC consumer)
    -> DisplayDataCoordinator / WorkBlockCoordinator / NotificationDeliveryCoordinator
    -> SwiftUI popover + UNUserNotificationCenter
       |
       | Unix domain socket ~/.velvt/velvt-service.sock
       v
bundled Rust process: velvt-service
  TokioUnixTransport / R7Router
    -> AbstractionEngine (seed taxonomy -> optional ONNX -> unclassified fallback)
    -> SQLite (~/.velvt/velvt-service.sqlite3; 9 embedded migrations)
    -> UploadBatcher / UploadCoordinator / AuthManager / reqwest
    -> FetchScheduler + long-poll PollScheduler
    -> PushAdapter -> IPC insight/history/notification/status messages
       |
       | HTTPS /v1
       v
cloud: velvt-core
  FastAPI API -> PostgreSQL (14 Alembic revisions)
    -> auth/device/ingestion/history/insight/report/privacy/admin routes
  Temporal worker
    -> DailySummaryWorkflow -> DailyInsightWorkflow
    -> RetentionTtlWorkflow / AccountErasureWorkflow
    -> quality-gated provider chain (configured providers -> mandatory template fallback)
    -> long-poll delivery (`delivered_at`); APNs dispatch code retained but disabled in workflow
```

Primary evidence: `velvt-app/ARCHITECTURE.md:8-76`; `velvt-app/swift-client/Sources/VelvtMac/App/AppModule.swift:68-140`; `velvt-app/rust-service/src/main.rs:92-442`; `velvt-core/app/main.py:16-46`; `velvt-core/workers/main.py:30-84`; `velvt-core/workers/workflows/daily_summary.py:16-63`; `velvt-core/workers/workflows/daily_insight.py:13-70`.

### Release baseline by subsystem

| Subsystem | Baseline | Evidence state |
|---|---|---|
| Swift app lifecycle | Starts bundled helper directly with `Process`, starts permission monitoring, IPC, display, collection, and notifications; helper is terminated synchronously on app exit. | Implemented-unverified: `AppModule.swift:68-140`; `ServiceProcessLauncher.swift:35-125` |
| Event capture | Event-driven AX/NSWorkspace collection, observer replacement, pending dwell measurement capped at 30 minutes. | Dev-mode only through tests/source: `CollectionModule.swift:103-179,231-289,347-350` |
| IPC contract | Shared JSON schemas and protocol file at version 20; handshake and malformed-message tests exist. | Dev-mode only: `proto/version`; `rust-service/tests/ipc_connection.rs:139-171`; xcconfigs line 5 |
| Local privacy/persistence | Rust DTO structurally omits raw fields; SQLite permissions are set 0700/0600; migrations are embedded and transactional. | Dev-mode only: `upload/dto.rs:6-52,74-103`; `persistence/sqlite.rs:49-109`; `migrations/0001_initial_persistence.sql:1-79` |
| Upload | Count/age/shutdown flush, persistent retry/backoff, raw-field rejection terminal handling. | Dev-mode only: `rust-service/src/main.rs:301-384,465-490`; `upload/coordinator.rs:115-213` |
| Auth | Swift stores auth snapshot in Keychain; Rust uses a volatile process store and session replay. | Dev-mode only: `AuthModule.swift:77-163,226-256`; `auth/store.rs:149-193`; `auth/account.rs:64-95` |
| Cloud ingestion/modeling | Device-authenticated, rate-limited, idempotent per device/batch; daily summaries and insights persist in PostgreSQL. | Dev-mode only: `app/api/v1/events.py:22-36`; `event_ingestion_service.py:29-186` |
| Insight delivery | Approved insights become long-poll ready; Rust shapes an insight and notification; Swift schedules received copy. Early-stage insights suppress notifications. | Dev-mode only: `daily_insight.py:44-57`; `delivery/poll.rs:335-354`; `NotificationDeliveryCoordinator.swift:54-84` |
| Release app | Existing artifact is Debug/local/ad-hoc x86_64 and fails `verify_release.sh`. | **Packaged-app verified** |
| Signing/notarization/DMG | Developer-ID/hardened-runtime/notarization are documented as deferred; no DMG target/artifact found. | Proposed only: `CONFIGURATION.md:163-191`; Makefile lacks DMG/notary targets |
| Updater | No Sparkle/feed/update-signing implementation found. `ServiceManager` only replaces a bundled helper and is not wired by `AppDelegate`. | Proposed only / dead implementation seam: source-wide `rg`; `ServiceManager.swift:63-71`; `AppModule.swift:43,68-72` |
| Cloud deployment | Dockerfile, Compose, Alembic, API/worker commands documented; no deployed staging/production evidence or live service check in this audit. | Implemented-unverified: `Dockerfile:1-20`; `docker-compose.yml`; `docs/deployment.md` |

### Skills used

No available skill matched repository architecture, native macOS distribution, privacy/security auditing, test execution, or report authoring. The available catalog was dominated by image/OpenAI/plugin creation and SaaS/Cloudflare/GitHub connector workflows. Invoking one would not advance this read-only audit, so native `rg`, `find`, build metadata, plist, `codesign`, `spctl`, `file`, and repository scripts were used. Resulting artifact: this handoff.

## 2 Evidence and commands run

All commands ran from `/Users/maximkudryashov/Projects/velvt-dev` unless noted.

- `cat velvt-app/AGENTS.md && cat velvt-core/AGENTS.md` — read both repository instruction files in full before inspection.
- `rg --files ...`, `find velvt-app ...`, `find velvt-core ...` — enumerated sources, tests, migrations, docs, release assets, and build files.
- `git -C velvt-app status --short; git -C velvt-core status --short` — found a pre-existing user edit at `velvt-app/swift-client/Sources/VelvtMac/UI/HistoryListView.swift`; this audit did not touch it. `velvt-core` was clean at the time checked.
- `nl -ba` over the Swift lifecycle/collection/IPC/delivery modules, Rust main/router/persistence/upload/auth/delivery modules, Python app/config/services/workflows/models, Makefiles, CI, Docker, migrations, xcconfigs, scripts, and docs — line-addressable source evidence.
- `rg -n "Sparkle|SUFeed|update|notari|stapl|..." ...` — found no app updater/feed/signing implementation and no automated notarization/DMG pipeline.
- `rg -n "flush_sleep|willSleep|didWake|..." ...` — proved sleep/wake reaches UI/work-block code but not `AXCollectionAgent` or the live upload batcher; Rust `flush_sleep` is otherwise only an unwired API/test seam.
- `rg -n "timezone|time_zone|..." velvt-core/...` — proved summaries and schedules use UTC calendar days and user/device models have no timezone field.
- Test inventory: 28 Swift test files, 19 Rust integration-test files, 50 Python test files; approximately 1,072 test declarations across unit/integration sources (simple declaration-pattern count, not collected-test count).
- `velvt-app/scripts/verify_release.sh velvt-app/dist/Velvt.app` — **failed**: `artifact configuration is 'Debug', expected Release`.
- PlistBuddy reads of `VelvtBuildConfiguration`, `VelvtDistributable`, `VelvtAPIBaseURL`, `VelvtProtocolVersion` — returned `Debug`, `NO`, `http://localhost:8000`, `20`.
- `codesign -dv --verbose=4 ...` — returned `Signature=adhoc`, `TeamIdentifier=not set`; `spctl --assess` did not produce an accepted Gatekeeper assessment.
- `file` and `lipo -info` on app/helper — both are thin `x86_64`; host is `x86_64`. No cross-architecture artifact was built.
- `find velvt-app -maxdepth 2 ( *.dmg | *.zip | *.pkg )` — no distribution container artifact.

Evidence labels used in this report mean:

- **Packaged-app verified:** inspected or executed against `dist/Velvt.app`.
- **Dev-mode only:** covered by source tests or development seams, not a signed installed app.
- **Implemented-unverified:** production code exists, but this audit did not execute the behavior.
- **Proposed only:** documentation/TODO/seam exists without an active, verified production path.

## 3 Files changed

- Added `/Users/maximkudryashov/Projects/velvt-dev/ARCHITECTURE_AUDIT_HANDOFF.md` (this file).
- No production code, tests, manifests, lockfiles, repositories, or existing user changes were modified.

## 4 Tests added or executed

- Tests added: none (read-only architecture mandate).
- Automated unit/integration suites executed: none; Core QA owns the independent runnable baseline and packaged behavior matrix.
- Release check executed: `velvt-app/scripts/verify_release.sh velvt-app/dist/Velvt.app` — failed immediately because the artifact is Debug.
- Packaged-artifact metadata/signature inspection executed as listed in section 2.

### Missing-test map

| Release behavior | Existing evidence | Missing release evidence | Priority |
|---|---|---|---|
| Clean install from DMG to `/Applications` and first launch under Gatekeeper | None | DMG layout, drag-install, quarantine, Developer-ID/notary/staple, clean account | P0 |
| Version N -> N+1 secure update | No updater implementation | signed feed/manifest, download authentication, install/restart, data preservation, rollback | P0 |
| Production config fail-closed | Config unit tests cover many fields | production rejects default JWT/admin/provider secrets and unsafe endpoints | P0 |
| Sleep/wake event integrity | Work-block/UI lifecycle tests; `flush_sleep` unit test | collection interval closure, upload flush, no phantom dwell, repeated sleep/wake in packaged app | P1 |
| Helper crash recovery | Launcher unit tests cover start/stop | kill helper under packaged app and prove relaunch/reconnect without user action/data loss | P1 |
| User-local day and timezone/DST | UTC modeling tests | explicit product contract plus local-day, DST forward/back, travel/timezone-change E2E | P1 |
| Full real client -> core path | Rust e2e explicitly uses no live core (`rust-service/tests/e2e_integration.rs:12`) | packaged Swift -> Rust -> staged FastAPI/Postgres/Temporal -> notification | P1 |
| Production migration/rollback | migration order and SQLite upgrade tests | snapshot/restore or tested downgrade on production-like Postgres with current data volume | P1 |
| Architecture coverage | Separate repo CIs | atomic cross-repo compatibility matrix for protocol/schema/client API versions | P1 |
| macOS architecture support | current thin x86_64 app only | declared support policy and Intel + Apple Silicon or universal artifacts | P1 |
| Real permission/TCC lifecycle | extensive fakes/unit tests | Accessibility grant/revoke/regrant and notification permission in signed `/Applications` build | P1 |
| Long-running performance | short historical measurements; IPC/SQLite/Swift RSS explicitly unmeasured | 24h background CPU/RSS, queue growth, socket churn, SQLite at retention-scale | P2 |

## 5 Findings P0-P3

### P0

1. **No valid distributable artifact.** Existing `dist/Velvt.app` is Debug, non-distributable, localhost-bound, ad-hoc signed, thin x86_64, and fails repository release verification. No DMG/notarization/stapling artifact exists. **Packaged-app verified.** Evidence: artifact inspection commands; `scripts/verify_release.sh:19-32,57`; `Makefile:64-88`; `CONFIGURATION.md:163-191`.
2. **Production can boot with a publicly known JWT signing secret.** `jwt_secret` defaults to `local-dev-insecure-change-me`; `validate_modeling_relationships` has no environment/security checks. A mistakenly underconfigured production instance would accept attacker-forged tokens. **Implemented-unverified.** Evidence: `velvt-core/app/core/config.py:17-45,127-141`; production checklist merely instructs operators at `docs/deployment.md:262-274`.
3. **Secure in-app update path is absent.** No updater dependency/feed/signature/install path is present; `ServiceManager` is only a dormant bundled-helper copier and cannot update the `.app`. Given the required release gate, claiming updates would be false. **Proposed only.** Evidence: source-wide updater search; `swift-client/Package.swift:1-23`; `ServiceManager.swift:63-71`; `AppModule.swift:43,68-72`.

### P1

1. **Sleep can create false activity.** `AXCollectionAgent` retains `pendingDwellEvent` across sleep and caps the next elapsed interval at 30 minutes; its source has no sleep/wake observer. Sleep notifications only drive connection presentation and work-block lifecycle. This can materially corrupt summaries and insights. **Implemented-unverified.** Evidence: `CollectionModule.swift:113-118,135,269-289,347-350`; `WorkBlockCoordinator.swift:55-65`; source-wide `flush_sleep` search.
2. **Bundled helper has no automatic crash relaunch.** `ServiceProcessLauncher` clears `process` on termination but does not restart it; IPC retries cannot restore a dead helper. The file explicitly defers a managed login item. **Implemented-unverified.** Evidence: `ServiceProcessLauncher.swift:4-13,61-83`. This is especially serious for a passive background product.
3. **Daily boundaries are UTC, not user-local, and no timezone is modeled.** Workflows pick yesterday by Temporal UTC time; summary loading/clipping and sessionization use UTC calendar days; user/device models have no timezone. Notifications and “day” insights can therefore be assigned to the wrong human day, especially near midnight/DST/travel. **Implemented-unverified.** Evidence: `workers/workflows/daily_summary.py:20-22`; `daily_summary_service.py:33-53,205-231`; `analytics/sessionization.py:52`; `models/user.py:10-38`; `models/device.py:10-48`.
4. **macOS CI's packaged-app verification path is wrong.** CI builds `dist/Velvt.app`, then runs `codesign` against `dist/velvt-mac.app`; that step cannot validate the produced artifact. **Implemented-unverified.** Evidence: `velvt-app/Makefile:89-107`; `.github/workflows/ci.yml:37-40`.
5. **Backend CI does not trigger on `main`.** It runs on PRs, `develop`, and `feature/**`; a direct main push has no configured run. **Implemented-unverified.** Evidence: `velvt-core/.github/workflows/ci.yml:3-9`.
6. **Release configuration labeled production targets the development API.** `Release.xcconfig` says “production API” but sets `https://dev-api.getvelvt.com`; the root Makefile also defaults releases to that host. Resolve whether this is a closed-alpha intent or a release misroute before shipping. **Implemented-unverified.** Evidence: `Release.xcconfig:1-3`; `Makefile:15`.
7. **No demonstrated production deployment/migration rollback.** Code and docs exist, but there is no live staging evidence, immutable image provenance/SBOM, backup-restore rehearsal, or end-to-end migration rollback; docs explicitly delegate rollback to external backup/restore unless separately tested. **Proposed/implemented-unverified.** Evidence: `docs/deployment.md:154-204`.
8. **Cross-repository compatibility is not enforced in one CI graph.** Each repo has independent CI; Rust integration tests explicitly avoid a live core. Protocol/API drift can pass both isolated suites. **Dev-mode only.** Evidence: both CI files; `rust-service/tests/e2e_integration.rs:12`.

### P2

1. **Architecture documentation is stale on protocol version.** Canonical `ARCHITECTURE.md` says version 12 while `proto/version` and both xcconfigs are 20. Evidence: `ARCHITECTURE.md:96-107`; `proto/version`; xcconfigs line 5.
2. **Two contradictory helper lifecycle implementations exist.** Production `AppDelegate` uses direct `Process`; the separately tested `ServiceManager` installs to Application Support and registers `SMAppService` but has no production caller. This increases maintenance/confusion and updater false-confidence. Evidence: `AppModule.swift:43,68-72`; `ServiceManager.swift:63-71`; production source search found no `ServiceManager(` call.
3. **Early Rust startup failures are silent.** Invalid config/filter/log initialization return before structured logging is active. Users may see an indefinitely unavailable service without a diagnostic. Evidence: `rust-service/src/main.rs:27-41`.
4. **Release verification is too shallow even when it passes.** It checks strict code-sign validity but not Developer ID identity/team, hardened runtime, entitlements, Gatekeeper acceptance, notarization/stapling, architecture policy, or installed launch. Evidence: `scripts/verify_release.sh:19-66`.
5. **Notification dedupe is process-memory/pending-window scoped.** Rust remembers only the last delivered ID in memory; Swift removes its per-date pending entry after scheduling. Correctness depends on backend `delivered_at` and has no packaged restart/replay evidence. Evidence: `delivery/poll.rs:321-353`; `NotificationDeliveryCoordinator.swift:64-84`.
6. **Release/version surfaces remain placeholder-like.** Swift marketing/build version and Rust crate version are both `0.1.0`; no release manifest ties app, helper, core API compatibility, schema, checksum, and update feed together. Evidence: `Release.xcconfig:13-14`; `rust-service/Cargo.toml:1-4`; `velvt-core/pyproject.toml:1-4`.

### P3

1. **Docs and artifact names drift.** Some documentation/CI still refers to `velvt-mac.app`, while packaging emits `Velvt.app`. Evidence: `CONFIGURATION.md:98`; `.github/workflows/ci.yml:40`; `Makefile:61-88`.
2. **Test-count visibility is informal.** The repository has broad coverage, but no collected test manifest/coverage thresholds are published in CI, making regression-surface claims difficult to audit.

### Recommended task ordering

1. **Freeze release and close P0 security/configuration gates:** production `Settings` must reject known defaults/missing required secrets; add tests and deployment preflight.
2. **Define the release channel and endpoint:** closed alpha vs production, supported macOS versions/architectures, versioning policy, backend compatibility window.
3. **Fix core data integrity:** sleep/wake closes/flushed dwell state; add timezone/local-day contract and migration; validate notification truthfulness against fixtures.
4. **Choose one helper lifecycle:** wire managed crash relaunch or implement bounded automatic direct-process restart; delete/deprecate the unused contradictory path after migration.
5. **Build distribution + updater together:** Developer ID, hardened runtime/entitlements, universal or declared per-arch artifacts, notarized/stapled DMG, checksums, secure signed updater, rollback/data preservation.
6. **Repair CI release signals:** correct artifact path, add main/release triggers, combined client-core compatibility suite, production config tests, migration rehearsal, artifact verification.
7. **Run integrated gates:** full test/lint, staged core with Postgres/Temporal, packaged app installed at `/Applications`, permissions, 24h reliability, N->N+1 update, offline/recovery, clean-machine acceptance.
8. **Reconcile docs and produce immutable release manifest:** versions, commit SHAs, checksums, signing/notary IDs, supported architectures, API endpoint, schema/protocol compatibility, rollback instructions.

## 6 Open questions/blockers

1. Is `dev-api.getvelvt.com` intentionally the closed-alpha Release endpoint, or should Release target `api.getvelvt.com`?
2. Which macOS architectures are release-supported? Current evidence is x86_64 only; ONNX behavior differs by architecture.
3. What is the product definition of a “day”: UTC or the user's local calendar day? Current implementation is unambiguously UTC but UI/product language appears human-day oriented.
4. Are Developer ID, App Store Connect/notary credentials, APNs production credentials, update-signing keys, and production backend secrets provisioned? None were available or required for this read-only track.
5. What deployment platform hosts FastAPI/Postgres/Temporal, and where are backup/restore, secret rotation, TLS termination, network policy, alerting, and image provenance evidenced?
6. Is automatic background operation required after app/helper crash and user login? The current bundled helper lifetime ends with the app and has no crash relaunch.
7. The workspace had a pre-existing Swift UI edit. Integration owners must preserve/review it when accepting parallel patches.

Stop conditions encountered: credentialed signing/notarization, clean-machine installation, deployed production checks, and a real update path require credentials/external state and were not attempted.

## 7 Confidence

**High (0.92)** for repository architecture, current artifact state, absence of updater/DMG automation, CI path mismatch, production-secret default, UTC-day design, and sleep/lifecycle wiring because these are directly evidenced by code/configuration and artifact metadata.

**Medium (0.78)** for user-visible severity of sleep phantom activity, timezone boundaries, notification replay, and crash recovery because those paths were not exercised in a signed `/Applications` build in this track. Per the release protocol, incomplete runtime evidence is assessed conservatively.
