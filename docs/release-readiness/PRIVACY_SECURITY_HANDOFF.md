# Privacy, Security, and macOS Permissions Audit Handoff

## 1. Verdict

**NO-SHIP — P0 privacy-boundary and authentication blockers are present.**

The local Swift-to-Rust activity path is structurally narrow and the Rust upload DTO omits raw fields, but the cloud API independently accepts arbitrary strings in `abstraction_type`/`supported_abstraction_types` and arbitrary values under non-forbidden payload keys. A buggy or hostile client can therefore store raw titles/app names in the cloud despite the canonical `raw_field_rejected` claim. Separately, `velvt-core` can boot in `production` with the public default JWT secret, enabling token forgery if deployment configuration is missing.

Verification labels used below:

- **Packaged-app verified:** inspected `velvt-app/dist/Velvt.app`; it is a Debug, non-distributable, ad-hoc-signed x86_64 app, with no entitlements, and its bundled Rust helper is unsigned.
- **Dev-mode only:** targeted Swift/Rust/backend code and automated-test evidence; no clean production install or live production network capture was performed.
- **Implemented-unverified:** code paths exist but were not verified against live APNs, a production LLM provider, Temporal, or production PostgreSQL.
- **Proposed-only:** remediation recommendations; no production code was changed by this audit.

## 2. Evidence and commands run

### Actual data-flow map

1. **Local collection (dev-mode code verified):** Swift observes the frontmost app's localized name and focused/main-window title, timestamp, and computed dwell duration. `NSWorkspace.didActivateApplicationNotification` supplies app name; Accessibility reads only `kAXMainWindowAttribute` and `kAXTitleAttribute` and subscribes to focused-window/title changes (`velvt-app/swift-client/Sources/VelvtMac/Collection/CollectionModule.swift:11-27,390-433,436-590`).
2. **Transient relay (dev-mode code verified):** raw app/title events are buffered only in memory and sent over `~/.velvt/velvt-service.sock` (`EventRelay.swift:80-88,97-115,137-158,251-259`). The socket parent is forced to mode `0700` (`rust-service/src/ipc/transport.rs:131-153`).
3. **Local abstraction/persistence (dev-mode code verified):** Rust consumes raw strings into a SHA-256 stable-key hash, assigns category-scoped labels, and emits an `AbstractedEvent` without raw fields (`rust-service/src/abstraction/key.rs:3-40`; `engine.rs:18-33,97-164`). SQLite persists hash, abstract labels/categories, timing, safe cache JSON, and optional user-entered work-block intention; database mode is forced to `0600`, parent to `0700` (`persistence/sqlite.rs:49-76`; migrations `0001_initial_persistence.sql:1-79`, `0009_work_blocks.sql`). The DB is plaintext, not encrypted.
4. **Device-to-cloud transmission (dev-mode code verified):** Rust sends auth/device requests, event batches, history/insight reads, and long polling with `reqwest` (`auth/http.rs:100-168`). Serialized events contain `event_id`, timestamp, abstract label, classification tier, duration, and category; despite fields existing internally, `stable_id` and `taxonomy_version` are not serialized (`upload/dto.rs:6-53,74-103`). Packaged launch strips a runtime API-base override (`ServiceProcessLauncher.swift:42-54`), but standalone service configuration accepts arbitrary HTTP URLs (`config/mod.rs:176-187,343-349`).
5. **Cloud storage (dev-mode code verified):** PostgreSQL models store account email/password hash, device metadata and raw APNs token, hashed refresh tokens, abstract event batches/events, summaries/baselines, approved/rejected insight text, prompts, raw provider outputs, provider errors, push delivery metadata, and audit records (`velvt-core/app/models/*`; `docs/privacy-contract.md:8-19`). Default retention is events/summaries/insights 30 days, provider attempts/candidates follow insight retention, APNs delivery 90 days, audit 365 days (`app/core/config.py:79-86`; `retention_service.py:24-99`).
6. **Third-party transmission (implemented-unverified):** when configured beyond the default template provider, aggregate behavioral evidence and allowed template IDs are sent to OpenRouter or arbitrary compatible provider endpoints, and raw provider responses are stored (`insight_prompt_service.py:6-21`; `insight_provider_service.py:24-99,139-163`; `insight_generation_service.py:75-92,252-287`). No raw app/window data is intended in this prompt.
7. **Notification delivery (implemented-unverified):** cloud stores APNs tokens and sends a silent wake/fetch payload; insight text is fetched separately and scheduled locally. APNs token values are stored plaintext in PostgreSQL (`models/device.py:26-29`; `services/apns_service.py:168-222`). The inspected packaged artifact lacks an `aps-environment` entitlement, and source search found no `registerForRemoteNotifications`, so remote-token acquisition is not evidenced.
8. **Deletion:** local data is manually removable and account deletion clears modeled records, devices, refresh tokens, APNs tokens, provider attempts, and insights. It intentionally retains/anonymizes the user row and erasure record, while audit records linked by `user_id`/`actor_id` are not in the deletion model list (`erasure_service.py:22-35,86-119`; `audit_log_entry.py:13-31`).

### Explicit capture determination

- **Keylogging:** none found. No event tap/global keyboard monitor or keystroke API exists in production source.
- **Screenshots/screen recording:** none found. No ScreenCaptureKit/CGWindow image capture exists.
- **Clipboard collection:** none found. The app never reads the pasteboard. A user-invoked **Copy Diagnostics** action writes privacy-safe status/counts to the clipboard (`MenuBarPopoverView.swift:1212-1241`).
- **Microphone/camera:** none found.
- **Content capture:** active app localized name and focused-window title are deliberately captured via Accessibility. Optional work-block intention is explicit user input and local-only. No bundle ID, URL, file, contact, or raw document-body API is directly collected; URLs/filenames may occur incidentally inside window titles.

### macOS permission/entitlement audit

- Source exposes only Accessibility and Notifications permission types (`Permissions/PermissionManager.swift:7-22`) and checks before collection; revocation stops collection (`CollectionModule.swift:145-152,269-289,323-345`).
- The first-run flow checks without prompting at launch; explicit Accessibility prompting is owned by onboarding (`App/AppModule.swift:68-81`; `UI/OnboardingExperience.swift:84-123`). Notification permission is requested only when delivery needs it (`NotificationDeliveryCoordinator.swift:91-105`).
- No microphone, camera, screen-recording, Contacts, Calendars, location, or filesystem TCC usage description/permission exists.
- **Packaged-app verified:** `codesign -d --entitlements - dist/Velvt.app` returned no entitlements. `codesign -dv --verbose=4` reported `Signature=adhoc`, `TeamIdentifier=not set`; helper reported `code object is not signed at all`. `Info.plist` reported Debug, `VelvtDistributable=NO`, localhost API, development APNs.
- Xcode project has no `CODE_SIGN_ENTITLEMENTS`, `ENABLE_HARDENED_RUNTIME`, or App Sandbox configuration in source (`VelvtMac.xcodeproj/project.pbxproj`; targeted `rg`). This is insufficient for production distribution and remote APNs.

### Commands

```text
sed -n '1,240p' velvt-app/AGENTS.md
sed -n '1,240p' velvt-core/AGENTS.md
rg --files ...; targeted rg for collection, network, logging, secrets, TCC, screenshot, keylogging, pasteboard, telemetry
nl -ba <cited Swift/Rust/Python/config/migration/privacy files>
cd velvt-core && .venv/bin/pytest -q tests/test_privacy_security.py tests/test_privacy_ops.py tests/test_security.py tests/test_event_validation_service.py tests/test_events_api.py
cd velvt-app/rust-service && cargo test --test upload_batching --test persistence_contract --test auth_state --test unix_socket_smoke
plutil -p velvt-app/dist/Velvt.app/Contents/Info.plist
codesign -d --entitlements - velvt-app/dist/Velvt.app
codesign -dv --verbose=4 velvt-app/dist/Velvt.app
codesign -d --entitlements - velvt-app/dist/Velvt.app/Contents/Resources/velvt-service
spctl -a -vv --type execute velvt-app/dist/Velvt.app
```

## 3. Files changed

- `PRIVACY_SECURITY_HANDOFF.md` only (this audit artifact).
- No production code, tests, configs, or existing documentation changed.

## 4. Tests added or executed

- **Added:** none; audit mandate was read-only.
- **Backend targeted suites:** **38 passed**, one Starlette deprecation warning. This verifies existing tests but also exposes their limitation: privacy validation tests cover forbidden **keys**, not raw strings placed in allowed top-level fields/values.
- **Rust targeted suites:** **53 passed** (`auth_state` 2, `persistence_contract` 21, `unix_socket_smoke` 2, `upload_batching` 28).
- **Existing proof of unsafe acceptance:** `tests/test_events_api.py:320-332` explicitly expects unknown `abstraction_type` values to be accepted; schema permits any 1-128 character string (`app/schemas/events.py:7-22`). The validator recursively checks keys only (`event_validation_service.py:54-84`).
- **Packaged-app inspection:** signature/entitlement commands above; no launch, TCC grant, APNs delivery, clean install, notarization, or network packet capture was performed by this workstream.

## 5. Findings ranked P0-P3

### P0 — release blockers

1. **Cloud raw-field rejection is bypassable through allowed string fields and values.** `abstraction_type` and `supported_abstraction_types` are unconstrained strings (`schemas/events.py:10-22,47-52`), unknown types are deliberately stored and emitted into metrics/audit (`event_ingestion_service.py:112-151`), and only payload key names are checked (`event_validation_service.py:54-84`). A value such as a raw window title in `abstraction_type`, or raw content under an innocuous payload key, is accepted/stored. This directly contradicts `velvt-app/PRIVACY.md:23-34,80-84` and `velvt-core/docs/privacy-contract.md:3-6`. **Proposed-only fix:** enforce a strict privacy-safe abstraction label grammar/size and registered-or-safe-namespace rules across every top-level and payload field; reject raw-like/unapproved payload shape before metrics/audit; add API tests proving raw strings cannot be persisted via any field.
2. **Production can start with a publicly known JWT signing secret.** `Settings.jwt_secret` defaults to `local-dev-insecure-change-me` and there is no environment-dependent validator (`velvt-core/app/core/config.py:17-45,127-141`). Setting only `VELVT_ENVIRONMENT=production` leaves forged HS256 tokens possible (`services/security.py:77-117`). **Proposed-only fix:** fail startup outside local/test unless a high-entropy secret is explicitly supplied (prefer asymmetric keys/managed secret), reject known defaults, and add production-config tests.

### P1 — release blockers

1. **Canonical Keychain claims do not match implementation.** Privacy docs claim a Rust `KeychainTokenStore` and a `com.velvt.service.auth` Keychain item (`velvt-app/PRIVACY.md:59-63,110-119`; `PRIVACY_AUDIT.md:39`), but no such type exists. Production Rust uses `VolatileTokenStore` (`rust-service/src/main.rs:95-129`; `auth/store.rs:149-277`) and sends sessions, including bearer/refresh tokens, over the protected local socket to Swift Keychain (`auth/store.rs:165-190`; Swift `AuthModule.swift:75-170`). Tokens are not in SQLite, but the stated storage/deletion model is false. Correct docs or implement the claimed store before release.
2. **Account deletion does not match the broad user-facing “any data associated” promise.** Audit rows retain `user_id`/`actor_id` and are excluded from `USER_OWNED_MODELS`; the user and erasure rows also remain (`erasure_service.py:22-35,86-119`; `audit_log_entry.py:13-31`) while app privacy copy says deletion removes the cloud account and associated data (`velvt-app/PRIVACY.md:120-123`). Define the legal retention exception, sever/pseudonymize retained identifiers, and disclose exact retention.
3. **Inspected app artifact is not a secure distribution artifact.** **Packaged-app verified:** host app is ad-hoc signed with no Team ID/entitlements, helper is unsigned, `spctl` fails, artifact is Debug/non-distributable/localhost. Source also lacks hardened-runtime and entitlement configuration. Production release must sign all nested code, enable hardened runtime, use least-privilege entitlements (including APNs only if remote pushes are actually supported), notarize/staple, and re-run permission tests from `/Applications`.

### P2 — important hardening/transparency

1. **Third-party model processing is not disclosed in macOS privacy/onboarding copy.** The backend may send aggregate behavioral evidence to OpenRouter or arbitrary compatible endpoints (`insight_provider_service.py:24-99,139-163`); canonical app privacy text describes cloud sync but not subprocessors. Default is template-only, so activation is configuration-dependent. Disclose provider classes/data/retention and require approved HTTPS endpoints/data-processing controls before enabling.
2. **Standalone local service accepts plaintext or arbitrary API base URLs.** `ServiceConfig` performs no HTTPS/host allowlist validation and tests bless `http://localhost` (`config/mod.rs:176-187,343-349`). Packaged launch removes the override, but standalone/repackaged operation can transmit credentials over HTTP. Permit HTTP only under explicit local/debug mode; require HTTPS and an approved host for distributable builds.
3. **Swift Keychain items omit an explicit accessibility class.** `KeychainService.baseQuery` sets class/service/account only (`AuthModule.swift:163-169`), relying on platform defaults rather than `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` or a documented equivalent. Harden and test migration behavior.
4. **Local SQLite is protected by Unix permissions but not encryption.** It includes derived behavior, cached insight text, stable hashes, and optional free-form intentions. This is acceptable only if “protected” means owner-only file permissions; documentation/UX should not imply encryption (`persistence/sqlite.rs:49-76`; `PRIVACY.md:36-38,101-104`).
5. **Logging violates the repository’s no-path rule.** Swift logs the bundled helper’s absolute path and may log `localizedDescription` containing paths (`ServiceProcessLauncher.swift:63-82`), contrary to `velvt-app/AGENTS.md` logging requirements. Replace with fixed codes/basename-free diagnostics.
6. **The “no way to recover” stable-key claim is overstated.** The local key is deterministic unkeyed SHA-256 of low-entropy app/title inputs (`abstraction/key.rs:29-40`); it is not reversible, but dictionary guessing is possible if the DB is obtained. `PRIVACY.md:94-97` should say one-way/dictionary-resistant only if changed to a keyed device-local construction.

### P3 — documentation/operational gaps

1. `PRIVACY.md:69-72` says uploaded events include `stable_id` and `taxonomy_version`, but custom serialization omits both (`upload/dto.rs:19-52`). Update the field ledger to the exact wire format.
2. There is no crash-reporting/telemetry SDK in inspected production source. Backend structured logs and in-process metrics exist; the privacy policy should explicitly distinguish operational logs/metrics from third-party crash telemetry and identify retention/access controls.
3. APNs implementation stores raw device tokens in PostgreSQL while delivery rows store a token hash (`models/device.py:26-29`; `models/push_delivery.py:39`). This is operationally necessary but should be explicitly classified as a credential-like identifier, encrypted at rest where feasible, access-controlled, and covered by deletion/rotation documentation.

## 6. Open questions or blockers

- No production signing/notarization credentials, production `.app`/DMG, or clean macOS account were available; production TCC identity, hardened runtime, nested signing, and `/Applications` behavior remain unverified.
- No live `dev-api.getvelvt.com`/production environment was accessed. TLS termination, secret injection, database encryption/backups, log aggregation, cloud IAM, audit access, and actual retention workflow scheduling remain unverified.
- No production configuration was provided for `VELVT_INSIGHT_PROVIDER_CHAIN`; third-party transmission status and contractual retention cannot be confirmed.
- APNs entitlements/credentials and remote notification registration are absent from the inspected artifact/source path; silent-push behavior is implemented-unverified.
- The release configuration targets `https://dev-api.getvelvt.com` (`swift-client/Configs/Release.xcconfig:1-3`) while documentation elsewhere names `https://api.getvelvt.com`; release environment ownership must resolve this.
- Required specialized privacy/security and macOS packaging skills were unavailable in the session; audit used direct source inspection, platform tooling, and existing automated tests.

## 7. Confidence level

**High (0.92) for source-level data-boundary, storage, logging, configuration, and inspected-artifact findings.** The two P0s are directly evidenced by executable code and tests/config behavior. **Medium (0.65) for end-to-end production posture** because live cloud controls, secrets, APNs, notarized distribution, clean-machine TCC, provider configuration, and deployed retention were outside accessible evidence.
