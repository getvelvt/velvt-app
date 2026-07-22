# Velvt Release Readiness Report

Date: 2026-07-22
Decision owner: lead release manager
Decision: **NO-SHIP for public, paid, or production distribution**

## Executive decision

Velvt has a credible three-runtime foundation, broad automated coverage, and a
locally installable Release DMG. The integrated tree passes 432 backend tests,
311 Rust tests plus one doc test, and 444 Swift tests (one opt-in screenshot
test skipped). The
final DMG passes integrity, layout, embedded-helper, Release-configuration,
hardened-runtime, architecture, checksum, local code-sign, read-only mount, and
mounted-byte comparison checks. An earlier candidate launched from
`/Applications`; the exact current DMG was not installed over the existing app.

That evidence is not sufficient to ship. Secure in-app updates are now
implemented and locally verified with Sparkle's real signing tools, but two
credential/external-environment P0 gates remain:

1. **No production-trusted distribution artifact exists.** The verified DMG is
   locally signed, has no TeamIdentifier, is not notarized or stapled, and
   deliberately points to `https://dev-api.getvelvt.com`. Production releases
   now require an explicit API URL plus a separately approved exact hostname,
   but the real production value and Developer ID/notary credentials are not
   available. Production verification correctly fails closed.
2. **No production updater execution exists.** Real Sparkle 2.9.4 signatures
   and adversarial mutation/downgrade/publication-order cases pass locally, but
   no production feed/key or Developer-ID-notarized N-to-N+1 install, relaunch,
   rollback, and state-preservation run is available.

The conservative outcome is therefore NO-SHIP. The artifact is suitable only
for local engineering evaluation, not external distribution.

## Architecture and release baseline

```text
Swift macOS app
  Accessibility collection + onboarding + display + notification scheduling
        │ local Unix socket, protocol v20
        ▼
Embedded Rust service
  abstraction + SQLite + auth + batching + cloud fetch/poll + local rehydration
        │ HTTPS, category-scoped event DTOs
        ▼
Python/FastAPI core
  auth + ingestion + summaries + baselines + insight gates + APNs/provider seams
```

The app bundle contains the Rust helper and does not require an external helper
installation. Local SQLite is permission-protected but plaintext. Backend
persistence and workflows have extensive development-mode tests; no deployed
staging rehearsal, production database migration rehearsal, backup/restore
exercise, alerting/SLO evidence, or 24-hour packaged soak was available.

## Release gates

| Gate | Result | Evidence class |
|---|---|---|
| Backend full suite | 432 passed; one dependency warning | Verified in development mode |
| Rust full suite | 311 passed plus one doc test | Verified in development mode |
| Rust format and Clippy (`-D warnings`) | Passed | Verified in development mode |
| Swift full suite | 444 executed; 1 opt-in screenshot skipped; 0 failed | Verified in development mode |
| Xcode Release build | Passed; embedded Rust helper rebuilt from integrated source | Packaged-app verified |
| Local app/DMG verifier | Passed | Packaged-app verified |
| Current DMG mount/layout and byte match | Passed read-only mount, app/helper byte comparison, signature and Applications-link checks | Packaged-app verified on this Mac |
| Production signature/notary gate | Failed: no Developer ID Application identity | Packaged-app verified failure (P0) |
| Enabled updater Release build | Passed; exact universal Sparkle 2.9.4 and strict plist controls embedded | Locally packaged-app verified |
| Real Sparkle archive/feed generation | Passed with pinned 2.9.4 tools; archive and feed signatures cryptographically verified | Local signed-tool verification |
| N-to-N+1 version gate | Candidate build 2 accepted over prior build 1; same/lower fixtures rejected | Local structural/tool verification |
| Production N-to-N+1 update | Not available without Developer ID, notary credentials, production key/feed | Implemented but production-unverified (P0) |
| Clean external Mac, quarantine, and real TCC cycle | Not available | Unverified |

An earlier local candidate received an install-path smoke test on this build
host. The exact current artifact was mounted and byte-verified but was not
copied over an existing `/Applications` installation. Neither is a clean-machine
or first-ever TCC test.

## Final artifact

- App: `velvt-app/dist/Velvt.app`
- DMG: `velvt-app/dist/Velvt.dmg`
- SHA-256: `11745dfad0b9eefae283d11948b41f1793f712c31f7a6bae44e229ecee367ca1`
- Version/build: `0.1.0` / `1`
- Minimum macOS: `13.0`
- Architectures: app and helper are universal `arm64 x86_64`
- API: `https://dev-api.getvelvt.com`
- Signature: valid ad-hoc signature, hardened runtime enabled, TeamIdentifier
  absent
- Notarization/stapling: absent
- DMG presentation: high-resolution Velvt volume icon, branded 660x420 install
  background, positioned app and `/Applications` link; deterministic layout,
  exact root allowlist, Finder metadata/coordinates, image integrity, mounted
  bytes, and checksum verified

This is deliberately a local verification artifact. It must not be published.

## Lead-owned P0/P1 review and resolutions

### Closed in the integrated source and artifact

- **Default JWT secret (P0):** staging/production now reject known default/test
  secrets and values shorter than 32 characters. Tests cover startup failure.
  This is source-verified, not yet verified in a deployed production process.
- **Free-form event ingress (P0):** payload keys and category values are
  allowlisted, identifiers/versions are bounded, malformed values return 422,
  and error envelopes no longer serialize unsafe validator context.
- **App-identifying abstraction upload (P0):** independently reproduced after
  the initial audit. Local labels such as `communication:slack` and
  `video:youtube` are now ignored at the Rust cloud serialization boundary;
  uploaded labels are derived from a fixed category vocabulary. The server
  independently maps every unapproved syntactically valid token—including
  `secret_project_apollo` and `private:merger_plan`—to `system:unknown` before
  event/device persistence, metrics, or audit metadata. Regression tests prove
  the original token is absent. The rebuilt packaged helper contains this fix.
- **Sleep phantom dwell (P1):** Swift now stops collection before system sleep
  and restarts it after wake. A deterministic coordinator regression passes.
- **Unsupported causal product copy (P1):** onboarding now describes observed
  timing/evidence rather than promising causal explanations.
- **Insight quality defects:** full-window novelty comparison, deterministic
  salience, stronger calibration/copy gates, duplicate suppression, generic
  notification suppression, and UTC client/backend date alignment were added
  with deterministic fixtures.
- **Production endpoint policy (P0 constituent):** release now fails before
  build unless both the production URL and separately approved exact hostname
  are supplied. Credentials, query/fragment, non-default ports, localhost,
  dev/staging/test names, mismatches, and incomplete architecture policy are
  rejected by deterministic tests.
- **Universal packaging (P1):** Xcode now builds both Swift slices explicitly;
  the Rust helper cross-builds both targets and is combined with `lipo`. The
  packaged verifier proves both slices in the app and helper.
- **Helper crash recovery (P1):** bounded exponential relaunch, stable-run
  reset, and quit cancellation are implemented and regression-tested.
- **Provider transport and disclosure (P1):** staging/production compatible
  provider endpoints require HTTPS and an exact hostname allowlist; unsafe
  URLs and redirects are rejected. Token-storage, provider-processing, and
  account-erasure disclosures now match implementation.

### Open P0 blockers

1. Activate the implemented updater with a production HTTPS feed and protected
   Ed25519 key; produce two Developer-ID-signed/notarized versions and complete
   the valid and adversarial N-to-N+1 install/relaunch/state-preservation matrix.
2. Produce a production artifact with a Developer ID Application identity,
   timestamped hardened-runtime signatures for nested code, notarization,
   stapling, Gatekeeper/quarantine verification, a production API endpoint, and
   a declared supported architecture set. Re-run the verifier on the exact
   published bytes.

### Open P1 risks

- Backend insight polling marks an insight delivered before Rust/IPC/macOS
  scheduling acknowledgement. A crash, denial, or queue loss can consume the
  only delivery attempt.
- There is no user-local-day or quiet-hours contract. UTC consistency is fixed,
  but human day boundaries and evening-summary scheduling are not.
- Real packaged notification delivery, denial/regrant, sleep/wake, helper-kill,
  offline recovery, and TCC behavior were not exercised on a clean Mac.
- CI Actions are pinned to immutable commits; app CI now runs updater fixtures
  and builds/mount-verifies a universal Release DMG, and backend CI has
  read-only permissions. A hashed Python dependency lock, true cross-repository
  protocol/API/update matrix, artifact attestation, and protected immutable
  release job are still absent.
- No deployed staging, backup/restore, secret rotation, SBOM/provenance,
  alerting ownership, or resource/queue soak evidence exists.
- Product evidence does not yet show that repeated insight value offsets account
  creation, Accessibility permission, background resource use, or eventual
  payment friction.

## Privacy and security determination

The implementation uses Accessibility to observe frontmost application changes
and window-title changes. Those raw strings pass once over a local Unix socket
to the Rust abstraction boundary. No keylogging, screenshots/screen recording,
clipboard collection, microphone capture, camera capture, or unapproved file
content capture was found. The app requests Accessibility and Notifications.

Raw application names, titles, URLs, paths, filenames, and user-provided text
are not permitted in cloud event DTOs. Curated app-specific classifications may
remain local for useful display, while the cloud boundary emits only fixed
category-scoped types. Local SQLite is not encrypted and documentation must not
imply otherwise. These conclusions are source- and test-verified; a live
production network capture and crash-report pipeline were not available.

## Insight and notification quality

Deterministic fixtures cover baseline learning, fragmentation, switching loops,
inactivity/no-signal behavior, novelty, provider failure/offline fallback,
deduplication, UTC boundary selection, notification permission handling, and
native request construction. The audit grades the pipeline substantially safer
than unrestricted LLM copy, but production notification readiness remains
NO-SHIP because acknowledgement, quiet hours/local-time semantics, packaged
delivery proof, and durable end-to-end delivery state are missing. Generic,
unsupported, repetitive, stale, or spammy notifications are treated as
reliability blockers rather than cosmetic defects.

## Product and independent engineering judgments

- Independent product score: **4.2/10**
- Independent startup score: **3.5/10**
- Independent engineering score before the lead's final privacy closure:
  **5.4/10**

The strongest product asset is a restrained, neutral-observer concept backed by
real local-first engineering. The largest churn risk is that the current loop
still offers narrow, sometimes generic observations relative to installation,
account, permission, and background-process costs. The product should not be
positioned as explaining *why* behavior occurs until causal evidence exists.

## Changes integrated during this review

- Production-secret startup validation and safer validation error envelopes.
- Server event/device privacy allowlists and unknown-token scrubbing.
- Rust category-only cloud abstraction serialization with regression tests.
- Insight novelty, salience, calibration, copy, and notification suppression
  fixes with deterministic tests.
- Sleep/wake collection lifecycle repair and less causal onboarding copy.
- Release packaging scripts, hardened-runtime entitlements, local/production
  verification gates, DMG creation, checksum generation, and documentation.
- Reproducible branded DMG presentation using the existing Velvt logo,
  hash-locked Finder-independent layout tooling, a high-resolution volume icon,
  strict root/layout validation, and immutable production output paths.
- Exact Sparkle 2.9.4 app integration, retained updater controller, manual
  **Check for Updates…** UI, strict production-only feed/key configuration, and
  privacy-preserving defaults.
- Immutable update archive generation, real Sparkle archive/feed signing and
  cryptographic feed verification, downgrade/version/host/checksum gates,
  nested-code signing, notarization hooks, tool checksum pinning, fixture tests,
  and archive-first/appcast-last operating documentation.
- Explicit universal Swift/Rust release builds, exact production API host
  approval, compatible-provider HTTPS/host allowlisting and redirect rejection,
  helper crash recovery, successful-schedule replay deduplication, and corrected
  privacy/deletion/provider disclosures.

The pre-existing user edit in
`swift-client/Sources/VelvtMac/UI/HistoryListView.swift` was preserved and is not
claimed as release-review work.

## Reproducible commands

```sh
cd velvt-core
.venv/bin/pytest -q

cd ../velvt-app/rust-service
cargo test -q
cargo fmt --check
cargo clippy -- -D warnings

cd ..
CLANG_MODULE_CACHE_PATH=$PWD/swift-client/.build/clang-module-cache \
  swift test --package-path swift-client --scratch-path $PWD/swift-client/.build \
  --disable-sandbox
make package-release
make prepare-dmg-tool
./scripts/create_dmg.sh dist/Velvt.app dist/Velvt.dmg local
make test-dmg-release
VELVT_RELEASE_ARCHS='arm64 x86_64' ./scripts/verify_release.sh \
  --mode local --app dist/Velvt.app --dmg dist/Velvt.dmg

# Updater fixture and release-tool gates:
make test-update-release
# Run verify_update_readiness.sh against the candidate app, signed appcast,
# immutable archive, and previous installed build as documented in docs/updates.md.
VELVT_RELEASE_ARCHS='arm64 x86_64' ./scripts/verify_release.sh \
  --mode production --app dist/Velvt.app --dmg dist/Velvt.dmg
make release \
  VELVT_PRODUCTION_API_BASE_URL="$APPROVED_PRODUCTION_API_URL" \
  VELVT_APPROVED_PRODUCTION_API_HOST="$APPROVED_PRODUCTION_API_HOST"
```

DMG creation/mount verification and `/Applications` launch require normal host
macOS disk-image/GUI access. Production release additionally requires
`VELVT_CODESIGN_IDENTITY` and `VELVT_NOTARY_PROFILE`.

## Skills and orchestration record

The available skill catalog was inspected before implementation. No available
skill covered repository-wide native macOS inspection/testing, macOS signing
and notarization, security/privacy review, product critique, or release-report
generation. The listed skills were for image generation, OpenAI documentation,
plugin/skill authoring, Canva, Cloudflare, GitHub workflows, Google Drive, and
Slack; invoking them would not advance this repository-local verification.
The required work therefore used repository instructions, native platform
tools, reproducible tests, and eight accountable specialist workstreams. This
limitation is explicit rather than presenting an unrelated skill invocation as
evidence.

Specialist evidence:

- `ARCHITECTURE_AUDIT_HANDOFF.md`
- `CORE_QA_HANDOFF.md`
- `INSIGHT_NOTIFICATION_HANDOFF.md`
- `PRIVACY_SECURITY_HANDOFF.md`
- `DISTRIBUTION_HANDOFF.md`
- `UPDATE_SYSTEM_HANDOFF.md`
- `UPDATER_SECURITY_REVIEW.md`
- `PRODUCT_CRITIC_HANDOFF.md`
- `ENGINEERING_REVIEW_HANDOFF.md`

Each handoff records verdict, commands/evidence, files changed, tests, P0-P3
findings, blockers, confidence, and whether evidence is packaged, development
only, implemented-but-unverified, or proposed. The lead reproduced all P0/P1
release claims that affected the final decision. Where evidence remained
incomplete, the more conservative assessment was retained.

## Activation checklist before reconsidering ship

1. Close both P0s with the exact production bytes and updater N-to-N+1 evidence.
2. Implement acknowledged notification delivery and local-time/quiet-hours
   semantics; run a 24-hour packaged notification ledger.
3. Exercise sleep/wake, helper-kill/backoff exhaustion, offline, malformed-data,
   and restart recovery in the installed app.
4. Complete release CI with a hashed Python lock, cross-repository
   protocol/API/update gates, artifact attestations, and a protected immutable
   publishing job.
5. Test supported Intel and Apple-Silicon configurations, including quarantine,
   Gatekeeper, fresh TCC grants/denials, uninstall/reinstall, and data
   preservation.
6. Rehearse staging migrations, backup/restore, secret rotation, monitoring, and
   rollback; publish provenance/SBOM and an immutable release manifest.
7. Re-run every suite and verifier on the final commit, then repeat independent
   product, privacy, notification, distribution, updater, and engineering
   reviews. Only the lead release manager may change the NO-SHIP decision.
