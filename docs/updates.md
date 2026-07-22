# Secure application updates

## Current status

Velvt now integrates Sparkle for whole-bundle updates and includes fail-closed
release tooling. Production activation remains closed until a Developer-ID
signed, notarized N-to-N+1 run and the adversarial matrix below pass with the
real feed and release credentials. `ServiceManager` remains unrelated helper
installation code; it does not replace `Velvt.app`.

The selected architecture is Sparkle 2.9.4, pinned to an exact package version.
Sparkle is appropriate because Velvt ships as a regular non-sandboxed app bundle
with an embedded Rust executable. Updating the whole bundle keeps the Swift app
and Rust helper on the same IPC protocol version and avoids a second privileged
installer or a custom self-replacement mechanism.

## Trust and data boundaries

An accepted update must pass every layer below:

1. Fetch the appcast and archive over HTTPS.
2. Require a signed appcast (`SURequireSignedFeed=true`), never allow a prior
   signature failure to expire (`SUSignedFeedFailureExpirationInterval=0`), and
   verify before extraction (`SUVerifyUpdateBeforeExtraction=true`). System
   profiling is disabled (`SUEnableSystemProfiling=false`).
3. Verify the archive with Sparkle Ed25519 using the public key embedded as
   `SUPublicEDKey`. The private key must be held in the release operator's
   Keychain or offline secret store, never in this repository or on the update
   host.
4. Verify the replacement app and embedded Rust helper with the same Developer
   ID identity used by the installed release. Both must use hardened runtime and
   the release must be notarized before publication.
5. Let Sparkle perform atomic bundle replacement and relaunch. Do not implement
   app replacement in `ServiceManager` or shell out from application code.

The update replaces only `/Applications/Velvt.app`. User state remains outside
the bundle: Keychain authentication, `~/.velvt/velvt-service.sqlite3`, local
preferences, and other Application Support data are preserved. The new app
relaunches its newly bundled helper through `ServiceProcessLauncher`.

No raw activity data, insight text, token, database content, or device profile
may be added to appcast requests or updater telemetry. Disable profile submission
and do not add custom feed query parameters. Sparkle's default update request is
the only updater-originated network traffic.

## Required integration

1. Keep `https://github.com/sparkle-project/Sparkle` pinned at exact version
   `2.9.4` in both Swift package resolution and the Xcode project.
2. Retain `SPUStandardUpdaterController` for the application lifetime and keep
   the **Check for Updates…** action wired to it. Do not claim automatic updates
   until the packaged end-to-end test below passes.
3. Continue injecting production-only `SUFeedURL`, `SUPublicEDKey`,
   `SURequireSignedFeed=true`, and `SUVerifyUpdateBeforeExtraction=true` into the
   generated Info.plist. Debug/local builds should use an explicit local test
   feed or disable updater startup; they must never silently use production.
4. Provision the Ed25519 key using Sparkle's `generate_keys`. Store only the public
   key in build configuration. Use Sparkle's `generate_appcast` rather than
   hand-authoring signatures or delta metadata.
5. Publish a notarized full update archive before atomically publishing its
   signed appcast. Keep at least the previous full archive available so clients
   can fall back when a delta cannot apply.
6. Run `scripts/verify_update_readiness.sh` against the exact signed app and
   appcast that will be published. This structural gate supplements, but does not
   replace, Sparkle's signature verification and the live N-to-N+1 test.
   `make release` requires `VELVT_APPCAST_PATH` and invokes this gate before DMG
   creation, so distribution cannot pass independently of updater readiness.

## Reproducible release commands

Provision Sparkle's `generate_appcast` from the exact 2.9.4 artifact resolved by
the project. Keep the Ed25519 private key outside this repository with mode 0400
or 0600. Never pass the key contents in an environment variable, command line,
CI log, appcast, or build artifact.

The complete credentialed release is generated with explicit immutable paths:

```sh
make release \
  VELVT_CODESIGN_IDENTITY='Developer ID Application: Example (TEAMID)' \
  VELVT_NOTARY_PROFILE=velvt-notary \
  VELVT_PRODUCTION_API_BASE_URL=https://api.example.com \
  VELVT_APPROVED_PRODUCTION_API_HOST=api.example.com \
  VELVT_RELEASE_VERSION=0.2.0 \
  VELVT_RELEASE_BUILD=42 \
  VELVT_PREVIOUS_RELEASE_BUILD=41 \
  VELVT_UPDATE_FEED_URL=https://updates.getvelvt.com/appcast.xml \
  VELVT_UPDATE_PUBLIC_ED_KEY='<base64-encoded-public-key>' \
  VELVT_UPDATE_ARCHIVE_PATH=dist/updates/Velvt-0.2.0-42.zip \
  VELVT_APPCAST_PATH=dist/updates/appcast-42.xml \
  VELVT_UPDATE_BASE_URL=https://updates.getvelvt.com/ \
  VELVT_SPARKLE_BIN_DIR=/absolute/path/to/Sparkle-2.9.4/bin \
  VELVT_GENERATE_APPCAST_SHA256='<approved-tool-sha256>' \
  VELVT_SIGN_UPDATE_SHA256='<approved-tool-sha256>' \
  VELVT_SPARKLE_PRIVATE_KEY_FILE=/secure/outside/repo/sparkle-private-key
```

The pipeline fails if an archive, checksum, or appcast would be overwritten. It
Developer-ID signs Sparkle nested code inside-out, preserving Downloader.xpc's
required entitlements while reissuing designated requirements under Velvt's
identity; notarizes and staples the app; creates a full-update ZIP and SHA-256
sidecar; invokes Sparkle's own
`generate_appcast`; and binds the appcast URL, build, and byte length back to the
local immutable archive. Both generation and readiness gates require the
embedded signed-feed signature in addition to each archive's Ed25519 signature.
`make test-update-release` exercises negative fixtures.

The signed feed is Sparkle's trailing `sparkle-signatures` comment block. The
gate requires exactly one trailing block and verifies its declared signed byte
length matches the XML bytes before it. Generation also runs Sparkle 2.9.4's
`sign_update --verify` against the external key before accepting the appcast.
Both Sparkle executables must match release-approved SHA-256 values so a PATH or
artifact substitution cannot silently change the publishing tools.
The standalone structural verifier does not
cryptographically verify an Ed25519 signature;
that is Sparkle's responsibility during the packaged N-to-N+1 test. It proves
only that the required key/configuration and signature shape exist and that the
appcast references the locally checksummed archive.

Publication is deliberately separate from artifact generation:

1. Upload the versioned ZIP and `.sha256` without replacing any existing object.
2. Fetch the public ZIP, compare its length and SHA-256 with the local immutable
   artifacts, and confirm it is reachable over HTTPS.
3. Atomically publish the generated appcast only after step 2 passes. The appcast
   is the activation switch and is always published last.
4. Retain this and the previous full archive. Never delete or replace an archive
   while a published appcast references it.

## Credential-free local adversarial harness

After Swift package resolution has installed the pinned Sparkle 2.9.4 artifact,
run the real-tool harness with:

```sh
VELVT_REQUIRE_REAL_SPARKLE_TOOLS=1 \
  scripts/tests/update_local_adversarial_harness_test.sh
```

Override `VELVT_SPARKLE_BIN_DIR` only when validating an explicitly approved
2.9.4 tool directory. The harness uses public, deterministic non-production
test seeds created only in a temporary directory. It does not read Keychain,
contact a network host, modify `/Applications`, or retain a private-key file.

The harness uses the real `sign_update` binary to prove:

- valid N+1 archive and signed-feed verification;
- rejection after archive or feed modification, archive truncation, and wrong
  Ed25519 keys;
- the exact 2.9.4 trailing signed-feed representation;
- acceptance of build 2 over build 1 and rejection of same/lower builds;
- structural rejection of an unsupported simulated minimum OS and a non-HTTPS
  URL;
- archive-first/appcast-last publication ordering and no-overwrite semantics;
- both full archives contain only `Velvt.app`, while a state sentinel outside
  the bundle remains byte-identical across N and N+1 extraction.

`updates.example.invalid` is an IANA-reserved unreachable fixture host. The
harness asserts feed structure but deliberately makes no request. It therefore
does not prove offline download UX, retry behavior, or recovery after a partial
network transfer. Likewise, extraction into temporary directories is packaging
boundary evidence, not proof of Sparkle replacement, relaunch, Keychain/TCC
preservation, helper restart, or database migration behavior.

## Version and compatibility policy

- `CFBundleVersion` is the monotonically increasing update/build identifier.
- `CFBundleShortVersionString` is the user-facing version.
- Each app archive contains exactly one compatible Swift/Rust pair. The embedded
  helper's protocol version must match `VelvtProtocolVersion`; the existing
  release verifier enforces that invariant.
- SQLite migrations must be additive and forward-safe. Before publishing, copy a
  version-N database fixture, update to N+1, and verify N+1 can read it without
  loss. Application rollback after an irreversible local migration is not a
  supported recovery mechanism; ship a forward repair release instead.
- Appcast minimum-system and channel rules must prevent incompatible builds from
  being selected. Use a separate prerelease channel/feed for alpha testing.

## End-to-end release test

Perform this with two Developer-ID-signed, notarized builds and a controlled HTTPS
feed. Local development builds are insufficient evidence.

1. Install version N from its DMG into `/Applications`, launch it, authenticate,
   generate representative local state, and record checksums/counts that do not
   expose content.
2. Serve a signed appcast and notarized N+1 archive from the controlled feed.
3. Trigger **Check for Updates…**. Verify feed acceptance, archive download,
   pre-extraction Ed25519 verification, install-on-relaunch, and successful launch
   of both N+1 app and N+1 embedded helper.
4. Confirm app version, helper version, IPC protocol agreement, Keychain session,
   SQLite migration/state counts, preferences, Accessibility status, and
   notification status are preserved.
5. Repeat with the archive modified after signing, appcast modified after signing,
   wrong Ed25519 key, wrong Developer ID identity, truncated download, offline
   feed, and unavailable destination. Each must fail without replacing N or
   corrupting state, and a later retry must succeed.
6. Test a broken delta with an intact full archive and confirm full-download
   fallback. Test an N+1 launch failure in staging and exercise the documented
   recovery: reinstall the last known-good full DMG without deleting user data,
   or publish a forward repair if migrations are not backward-compatible.

Record appcast URL, archive SHA-256, Ed25519 signature, Developer ID team,
notarization submission ID, N/N+1 versions, fixture counts, Console log excerpts
without private data, and pass/fail for every case.

## Recovery and forward-repair checklist

Before activating an appcast:

1. Retain the prior full archive and DMG, their checksums, notarization evidence,
   migration version, and privacy-safe state counts. Confirm neither artifact
   can be overwritten at its published URL.
2. Exercise N+1 against a copy of the N database and record schema version plus
   row counts without recording content. Restore the copy after each failure
   scenario instead of reusing mutated state.
3. If N+1 fails before an irreversible migration, withdraw only the appcast
   activation and reinstall the retained N DMG without deleting external user
   data. Verify its checksum and Developer ID identity first.
4. If N+1 may have committed an irreversible migration, do not reinstall N.
   Freeze the affected appcast, prepare N+2 as a forward repair, validate it
   against both pristine-N and migrated-N+1 fixtures, then publish its immutable
   archive before atomically activating the repaired appcast.
5. Preserve failure logs containing codes and versions only. Do not capture the
   database, Keychain material, insight text, application labels, or raw events.
6. Assign an incident owner to feed withdrawal, archive retention, customer
   communication, and forward-repair approval before production activation.

## Production activation checklist

- [ ] Sparkle 2.9.4 is pinned, embedded, signed inside-out, and present in the
      notarized application.
- [ ] Production HTTPS feed hostname and retention/availability ownership are
      approved.
- [ ] Ed25519 keypair is provisioned; private-key custody, backup, rotation, and
      incident response are documented.
- [ ] Signed-feed and verify-before-extraction settings are present in the final
      Info.plist.
- [ ] N-to-N+1 and all negative-path tests above pass from `/Applications`.
- [ ] Local data and permissions survive update; the new helper starts and IPC
      reconnects.
- [ ] Publishing is archive-first and appcast-last; rollback/forward-repair owner
      and prior full archive are identified.
- [ ] `scripts/verify_update_readiness.sh` and the production release verifier
      both pass on the immutable release artifacts.
