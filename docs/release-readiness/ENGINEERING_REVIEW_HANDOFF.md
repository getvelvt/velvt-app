# Independent Engineering Review Handoff

Review date: 2026-07-22
Scope: current worktrees of `velvt-app` and `velvt-core`, specialist handoffs,
and the current local Release app/DMG. Production code was reviewed read-only.

## 1 Verdict

**Engineering verdict: NO-SHIP. Overall engineering score: 5.4/10.** The
repositories have unusually broad development-mode tests, clear runtime
boundaries, versioned IPC, additive persistence work, and materially improved
local packaging. Those strengths do not outweigh three P0 release blockers:

1. The privacy contract is not actually enforced as claimed. The shipped
   taxonomy uploads app-identifying labels such as `communication:slack` and
   `video:youtube` while canonical copy says app names never leave and labels
   are human-meaningless. The new server grammar also accepts arbitrary
   lowercase tokens such as `secret_project_apollo`, stores unknown values,
   and emits them into metric/audit metadata.
2. Secure application update remains absent. The packaged app contains no
   Sparkle framework/controller/key/feed, and there is no N-to-N+1 evidence.
3. The only release artifact is locally ad-hoc signed. Local DMG mechanics pass,
   but Developer ID, notarization, stapling, Gatekeeper/quarantine, and
   supported-architecture evidence do not exist.

The prior known-default JWT P0 is **closed at source/test level**: staging and
production now reject the known defaults and secrets shorter than 32
characters. It remains **implemented but unverified in a deployed process**.

### Engineering score rubric

| Dimension | Weight | Score | Evidence-based assessment |
|---|---:|---:|---|
| Architecture and boundaries | 15% | 6/10 | Swift/Rust/cloud and IPC responsibilities are clear, but helper lifecycle is split, user-local day semantics are absent, and client/core compatibility is not exercised together. |
| Code quality and maintainability | 15% | 7/10 | Idiomatic typed modules and focused patches; stale/dead lifecycle paths, documentation drift, and placeholder versioning remain. |
| Test discipline | 15% | 8/10 | 401 backend, 311 Rust, and 422 Swift tests passed in QA; targeted current-diff reruns also pass. No coverage threshold, production dependency matrix, or packaged behavior suite exists. |
| Reliability and recovery | 15% | 5/10 | Retry, persistence, migration, malformed-input, and sleep pause seams are tested; helper crash relaunch and delivery acknowledgement are absent. |
| Privacy and security | 15% | 4/10 | DTO structure, socket/file modes, redacted helper logs, and JWT fail-closed checks are good; taxonomy/unknown-label transmission contradicts the privacy promise, and deletion/provider disclosures remain mismatched. |
| Release/distribution/update | 15% | 3/10 | Local Release DMG passes and launches from `/Applications`; production trust and updater are absent, and CI release verification is still broken. |
| Observability and operations | 10% | 4/10 | Structured logs, audit records, and in-process metrics exist; there is no deployed staging evidence, alerting/SLO proof, backup-restore rehearsal, provenance/SBOM, or 24-hour packaged telemetry ledger. |
| **Weighted total** | **100%** | **5.4/10** | Suitable for continued internal development or a carefully bounded engineering alpha only after privacy blockers are corrected; not release-ready. |

Evidence labels in this handoff:

- **Packaged-app verified:** exercised against `dist/Velvt.app`,
  `dist/Velvt.dmg`, or the installed `/Applications/Velvt.app` evidence.
- **Dev-mode only:** source test/build evidence without signed production use.
- **Implemented but unverified:** production path exists but was not exercised
  under production credentials/environment.
- **Proposed only:** design, documentation, or gate without active runtime
  implementation.

## 2 Evidence and commands run

- Read both `AGENTS.md` files in full before review.
- Read `ARCHITECTURE_AUDIT_HANDOFF.md`, `CORE_QA_HANDOFF.md`,
  `INSIGHT_NOTIFICATION_HANDOFF.md`, `PRIVACY_SECURITY_HANDOFF.md`,
  `PRODUCT_CRITIC_HANDOFF.md`, `DISTRIBUTION_HANDOFF.md`, and
  `UPDATE_SYSTEM_HANDOFF.md`; independently checked the cited P0/P1 source and
  did not rely on unsupported conclusions.
- `git -C velvt-app status --short`, `git -C velvt-core status --short`, both
  `git diff --stat`, both `git diff --check`, and scoped full diffs: no whitespace
  errors. Current dirty files belong to the parallel release workstreams plus a
  pre-existing `HistoryListView.swift` edit.
- `velvt-core/.venv/bin/pytest -q tests/test_pattern_config.py
  tests/test_placeholders.py tests/test_event_validation_service.py
  tests/test_events_api.py tests/test_devices_api.py
  tests/test_insight_quality_service.py
  tests/test_scope2_work_loop_evaluation.py` -> **105 passed**, one dependency
  deprecation warning. **Dev-mode only.**
- `cargo test delivery::` -> **74 passed**; `cargo fmt --check` and
  `cargo clippy -- -D warnings` passed. **Dev-mode only.**
- Swift targeted current-diff runs -> **46 display/notification tests passed**
  plus the sleep/wake coordinator regression passed separately. Compilation
  still emits Swift 6 actor-isolation warnings. **Dev-mode only.**
- `scripts/tests/verify_update_readiness_test.sh` -> passed its structural
  fixtures. `scripts/verify_update_readiness.sh --app dist/Velvt.app ...` ->
  failed: `Sparkle.framework is not embedded`. **Packaged-app verified failure.**
- `VELVT_RELEASE_ARCHS=x86_64 scripts/verify_release.sh --mode local --app
  dist/Velvt.app --dmg dist/Velvt.dmg` -> passed strict local code-sign,
  hardened runtime, checksum, HFS integrity, mount/layout, architecture, and
  app/DMG binary-equality checks. Read-only DMG mounting required approved host
  disk-image access. **Packaged-app verified.**
- The same verifier in `production` mode -> failed because the app is not
  signed with Developer ID Application. **Packaged-app verified failure.**
- Reproduced the server privacy bypass directly: the current validator accepted
  `secret_project_apollo`, `salary`, and `private:merger_plan`. The ingestion
  service stores unknown values and includes them as metric label/audit
  metadata. **Dev-mode source/executable verified.**
- Inspected `rust-service/resources/abstraction-taxonomy-mvp-1.json` and
  `rust-service/src/upload/dto.rs`: labels including `communication:slack`,
  `video:youtube`, `document:notion`, and `reference:github` serialize as
  `abstraction_type`. This conflicts with `PRIVACY.md` and the repository
  privacy mandate. **Source verified; upload serialization dev-mode only.**
- Inspected both CI workflows, helper termination handling, UTC summary/date
  logic, insight polling claim order, and Swift notification scheduling. The
  app CI still verifies `dist/velvt-mac.app` although packaging emits
  `dist/Velvt.app`; backend CI still excludes `main` pushes.

No listed skill directly covered native macOS engineering/release review,
privacy boundary auditing, or repository-wide test assessment. Invoking an
unrelated connector skill would not improve evidence, so repository-native
tools and platform verification were used. Resulting artifact: this handoff.

## 3 Files changed

- Added `/Users/maximkudryashov/Projects/velvt-dev/ENGINEERING_REVIEW_HANDOFF.md`
  only.
- No production source, tests, build configuration, artifact, or existing user
  change was modified by this independent review.

### Current uncommitted-diff integration review

- **No textual conflict found:** distribution owns Makefile/signing/DMG files;
  updater owns updater docs/gates; insight work owns Rust delivery and Swift UTC
  fetch changes; privacy work owns backend schemas/validation/config; reliability
  work owns permission sleep/wake handling. `git diff --check` passes in both
  repositories.
- **Semantic privacy regression remains:** the new privacy docs claim bounded
  token grammar is privacy-safe, but grammar is syntax-only and accepts content.
  Existing app-identifying taxonomy labels make the broader claim false.
- **Semantic day-boundary mismatch remains:** changing the client fetch date to
  UTC aligns it with the backend but does not implement a user's local day. It
  preserves consistency while making the product's human-day semantics
  explicitly UTC.
- **Release-gate collision/gap:** `make release` does not invoke
  `verify_update_readiness.sh`; production distribution automation can reach a
  successful distribution verdict while the required updater is absent. The
  new updater fixture is also not run by CI.
- **CI regression left untouched:** macOS CI's packaged-app path is stale, and
  backend CI omits `main`. These files were outside the implementation agents'
  ownership but remain release-process failures.
- **User change preserved:** `HistoryListView.swift` is a pre-existing UI edit.
  It compiles in the targeted Swift run but has no focused regression/snapshot
  test in this release work.
- **No new dependency collision:** no production dependency was added. Sparkle
  is proposed/documented only.

## 4 Tests added or executed

Tests added by this reviewer: none; mandate was independent/read-only.

Executed by this reviewer:

| Gate | Result | Classification |
|---|---|---|
| Backend current-change regression set | 105 passed | Dev-mode only |
| Rust delivery + fmt + Clippy | 74 passed; standards passed | Dev-mode only |
| Swift display/notification set | 46 passed | Dev-mode only |
| Swift sleep/wake coordinator regression | 1 passed | Dev-mode only |
| Updater structural fixture | Passed | Dev-mode only; structure, not cryptography |
| Actual packaged updater gate | Failed: no Sparkle | Packaged-app verified |
| Actual local app + DMG release gate | Passed on x86_64 | Packaged-app verified, ad-hoc only |
| Actual production release gate | Failed: no Developer ID | Packaged-app verified |

Inherited QA evidence independently reviewed: backend 401 passed, Rust 311
passed, Swift 422 executed/one opt-in screenshot skipped, and an Xcode Debug
build passed. Those runs predated the final integrated tree, so the lead must
still rerun the complete suites after all workstream edits settle.

Not executed: real provider/APNs calls, production PostgreSQL/Temporal,
notarization, Gatekeeper quarantine, Apple Silicon, real TCC grant/revoke,
physical sleep, helper-kill recovery, 24-hour soak, staged client-to-cloud flow,
or N-to-N+1 update.

## 5 Findings P0-P3

### P0

1. **Privacy promise and transmitted taxonomy conflict.** Canonical policy says
   raw application names never leave and describes labels as human-meaningless,
   but the bundled taxonomy maps applications to labels such as
   `communication:slack`, `video:youtube`, `document:notion`, and
   `reference:github`; the upload DTO serializes the label as
   `abstraction_type`. These labels reveal application identity even though
   they are not byte-for-byte raw input. This is a product-claim/implementation
   mismatch and therefore a release blocker. **Source verified; runtime upload
   dev-mode only.** Evidence: `velvt-app/PRIVACY.md:21-34,89-97`;
   `velvt-app/AGENTS.md:42-43,194`;
   `rust-service/resources/abstraction-taxonomy-mvp-1.json`;
   `rust-service/src/upload/dto.rs:19-52`.
2. **Unknown abstraction labels remain a raw-content channel.** The new regex
   accepts arbitrary lower-case tokens; direct execution accepted
   `secret_project_apollo`, `salary`, and `private:merger_plan`. Unknown labels
   are stored unchanged and copied into metrics/audit metadata instead of being
   bucketed or excluded, contrary to the repository rule for unknown safe types.
   **Dev-mode executable/source verified.** Evidence:
   `event_validation_service.py:52-75,97-103`;
   `event_ingestion_service.py:119-151`.
3. **Secure updates are not implemented.** The actual Release app has no
   Sparkle framework, feed, public key, controller, signed archive/appcast, or
   N-to-N+1 state-preservation/failure evidence. **Packaged-app verified.**
4. **No production-trusted artifact exists.** Local Release app/DMG packaging
   passes, but the artifact is ad-hoc signed with no TeamIdentifier and the
   production verifier fails. Developer ID, Apple notarization/stapling,
   quarantine/Gatekeeper, and production clean-machine evidence are absent.
   **Packaged-app verified.**

### P1

1. **Insight claim is not delivery.** The backend marks an insight delivered
   before Rust queues it, before IPC succeeds, and before Swift/macOS schedules
   it. A crash, queue eviction, denied permission, or scheduling failure can
   permanently consume the only attempt. **Implemented but unverified in a
   packaged app.**
2. **No local-day/quiet-hours contract exists.** Backend summaries and workflow
   selection use UTC; the client patch now also requests UTC. No user/device
   timezone exists, Rust emits no `do_not_disturb_until`, and Swift schedules
   immediately. **Implemented but unverified.**
3. **Bundled-helper crash recovery is manual.** The termination handler clears
   the process reference but does not restart. IPC backoff cannot recover a
   dead helper, so a passive product can silently stop until the user chooses a
   restart or relaunches. **Implemented but unverified.**
4. **Release CI provides false/absent signals.** macOS CI verifies a nonexistent
   old artifact name; backend CI does not run on `main`; updater/distribution
   gates are not integrated into one release job; no cross-repository protocol
   and API compatibility run exists. **Source verified.**
5. **Architecture support and production endpoint are unresolved.** Current
   app/helper are thin x86_64 and point to `dev-api.getvelvt.com`. No Apple
   Silicon/universal evidence or explicit production/closed-alpha channel
   decision exists. **Packaged-app verified.**
6. **Privacy/deletion/provider disclosures remain inconsistent.** Docs claim a
   Rust Keychain store that does not exist, deletion language exceeds actual
   retained audit/user identifiers, and optional third-party model processing
   is not disclosed in first-run privacy copy. **Source verified.**
7. **No production operational proof exists.** There is no staged full-path run,
   backup/restore and migration rehearsal, secret-rotation evidence, alerting
   ownership, immutable image provenance/SBOM, or 24-hour resource/queue soak.
   **Proposed only.**

### P2

1. `ServiceProcessLauncher` and dormant `ServiceManager` encode contradictory
   helper lifecycle/install models; only the former is wired in production.
2. Notification dedupe is process-memory/client-window scoped and delivery is
   user-global rather than explicitly per-device. There is no durable client
   scheduling ledger.
3. Swift tests emit actor-isolation warnings documented as errors under Swift 6,
   and the local toolchain cannot run the documented `swift format` command.
4. Release/update versioning remains `0.1.0`/build `1` without an immutable
   manifest tying app, helper, protocol, schema, API compatibility, checksum,
   signing team, and feed.
5. New production JWT validation rejects defaults/short values, but length alone
   does not establish entropy. Deployment secret generation and rotation remain
   operational controls rather than verified behavior.
6. Local SQLite is permission-protected but plaintext, and its stable-key is an
   unkeyed hash susceptible to dictionary guessing. Documentation should avoid
   implying encryption or strong unlinkability.
7. The edited `HistoryListView` compiles, but its hover/accessibility behavior
   lacks a focused test and remains outside the owned release workstreams.

### P3

1. Backend test runs emit the Starlette/httpx deprecation warning; Xcode has
   always-run script-phase warnings.
2. Architecture/protocol, artifact-name, and information-architecture docs are
   stale in places. The new macOS distribution document is not added to the
   document index, while the updater document is.
3. No published coverage threshold or collected-test manifest supports the
   broad test-count claims.

### Prioritized hardening plan

1. Freeze release and settle the privacy contract. Replace app-identifying
   labels with genuinely coarse abstractions or disclose them explicitly;
   permit only registry-backed labels on cloud storage and bucket/exclude unknown
   values before logs, metrics, audit, or persistence. Add adversarial tests for
   lowercase and colon-separated content, not only spaces/URLs.
2. Implement delivery acknowledgement and a timezone-aware delivery contract:
   backend `available -> claimed -> acknowledged` states, retry policy, per-user
   versus per-device semantics, explicit local-day/DST rules, quiet hours, and
   packaged notification tests.
3. Implement bounded automatic helper crash relaunch with loop protection and
   test kill/restart/socket/data behavior in the installed app.
4. Integrate Sparkle fully, pin and embed it, provision signing/feed keys, and
   produce signed/notarized N and N+1 builds with tamper/offline/relaunch/data
   preservation evidence.
5. Produce a Developer-ID-signed, notarized/stapled artifact for an explicit
   endpoint and architecture policy; run Intel and Apple Silicon quarantine,
   Gatekeeper, TCC, sleep/wake, helper-kill, and clean-install tests.
6. Repair both CIs, add shell-gate fixtures, execute release/update checks on the
   exact artifact, and add a staged client/Rust/core compatibility job.
7. Reconcile Keychain/deletion/provider/local-storage disclosures, then run full
   test/lint, migration/restore rehearsal, 24-hour soak, and immutable release
   manifest generation.

## 6 Open questions/blockers

1. Is revealing application identity through reviewed taxonomy labels intended?
   If yes, privacy/onboarding claims must say so plainly; if no, the taxonomy and
   historical cloud data need remediation.
2. Must unknown abstraction types be rejected, stored only as `unclassified`,
   or held in a privacy-review quarantine? Current unchanged storage conflicts
   with the stated bucket/exclude rule.
3. Is this a closed alpha targeting `dev-api.getvelvt.com` and Intel only, or a
   production release? The artifact and documentation do not establish one
   consistent channel policy.
4. What is the product definition of a day and approved local notification
   window, including DST and travel?
5. Is notification delivery per user or per registered device, and what durable
   acknowledgement defines success?
6. Developer ID/notary credentials, Apple Silicon hardware, APNs/provider
   credentials, update keys/feed, and a production-like backend were unavailable;
   the corresponding tests cannot be completed from repository evidence alone.
7. The lead must rerun complete integrated suites after all edits stop changing
   and personally inspect the exact final checksum-addressed release artifact.

## 7 Confidence

**High (0.95)** for the privacy taxonomy/grammar findings, absence of updater,
local versus production artifact status, CI path/trigger failures, helper crash
behavior, UTC-day model, and current-diff test results; each was reproduced or
directly traced through active code/configuration.

**Medium (0.72)** for end-user severity and production operational behavior
because no signed/notarized universal artifact, live cloud/APNs/provider path,
physical sleep, update pair, clean Apple Silicon machine, or deployed
observability environment was available. Incomplete runtime evidence is scored
conservatively and never treated as correctness.
