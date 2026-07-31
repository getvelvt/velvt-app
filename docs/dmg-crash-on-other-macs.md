# "Velvt quit unexpectedly" when opening the DMG on another Mac

Symptom: `dist/Velvt.dmg` installs and runs on the build machine, but on a
second Mac (e.g. a Mac mini) the app shows the macOS crash dialog "Velvt quit
unexpectedly" immediately on launch.

Related: [`macos-distribution.md`](macos-distribution.md),
[`macos-signing-and-accessibility.md`](macos-signing-and-accessibility.md).

## Diagnosis

The DMG contains an ad-hoc signed, un-notarized app. Verified on
`dist/Velvt.dmg` built 2026-07-27:

```text
Signature=adhoc                  # no certificate, no identity
TeamIdentifier=not set
flags=0x10002(adhoc,runtime)     # hardened runtime is enabled
spctl -a -t exec  →  rejected
xcrun stapler validate  →  "does not have a ticket stapled to it"
```

Architecture is not the cause — the binary is a correct universal build
(`x86_64 arm64`) with `LSMinimumSystemVersion` 13.0.

Why it works on the build machine and not elsewhere: locally the ad-hoc
signature is trusted and the bundle carries no quarantine flag. Once the DMG is
copied to another Mac it gains `com.apple.quarantine`. Under hardened runtime,
library validation requires nested code to share the main binary's Team ID — and
an ad-hoc signature has none. `Sparkle.framework` and the bundled
`velvt-service` helper therefore fail validation at load time and the process is
killed, which surfaces as a crash dialog rather than a Gatekeeper message.

### Confirm the mechanism

Run on the affected Mac immediately after a crash:

```bash
ls -t ~/Library/Logs/DiagnosticReports/Velvt* | head -1 | xargs head -40
```

- `EXC_CRASH (SIGKILL)` with `CODESIGNING` / `Code Signature Invalid`
  → signing, as described above. Proceed with this document.
- `dyld` / `Symbol not found`
  → a different problem: an API newer than the 13.0 deployment target was linked
  without an availability guard. This document does not fix that.

## Root cause in the build

`dist/Velvt.dmg` was produced by the **local** path (`make dmg` →
`scripts/sign_release.sh local`), which ad-hoc signs by design. Local artifacts
are for the build machine only and are not distributable.

Separately, `Makefile:209-211` downgrades to ad-hoc silently:

```make
if ! codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" dist/Velvt.app; then \
    echo "Configured signing identity unavailable; falling back to ad-hoc signing."; \
    codesign --force --deep --sign - dist/Velvt.app; \
fi
```

`make release` is the correct path. It is credential-gated and never falls back
to ad-hoc.

## Unblock a test machine now (internal only)

Acceptable for your own machines and teammates. Not acceptable for real users —
never document this as an install step.

1. Copy `Velvt.app` from the mounted DMG into `/Applications` on the target Mac.
2. On that Mac, strip the quarantine flag:

   ```bash
   xattr -dr com.apple.quarantine /Applications/Velvt.app
   ```

3. Launch Velvt.

If it still crashes, capture the crash report as above — the cause is then not
quarantine.

## Real fix: sign and notarize

### Step 1 — obtain a Developer ID Application certificate

The keychain currently holds only:

```text
"Apple Development: kevinz09302009@gmail.com (5W9BP4J4UF)"
```

`Apple Development` runs on your own registered devices only and is not valid
for distribution. You need a **Developer ID Application** certificate, which
requires Apple Developer Program membership (USD 99/year).

1. Enrol at <https://developer.apple.com/programs/>.
2. In the developer portal, create a **Developer ID Application** certificate.
3. Download and install the `.cer`, then confirm it is present:

   ```bash
   security find-identity -v -p codesigning
   ```

   Expect a line beginning `Developer ID Application: … (TEAMID)`.

### Step 2 — store notarization credentials

Create an app-specific password at <https://appleid.apple.com>, then:

```bash
xcrun notarytool store-credentials VELVT_NOTARY \
  --apple-id "<your-apple-id>" \
  --team-id "<TEAMID>" \
  --password "<app-specific-password>"
```

Verify:

```bash
xcrun notarytool history --keychain-profile VELVT_NOTARY
```

### Step 3 — build the release

`make release` enforces every required variable and fails closed. Set them from
your real values:

```bash
cd velvt-app
make release \
  VELVT_CODESIGN_IDENTITY="Developer ID Application: <NAME> (<TEAMID>)" \
  VELVT_NOTARY_PROFILE=VELVT_NOTARY \
  VELVT_DMG_PATH=dist/Velvt-0.1.0.dmg \
  VELVT_RELEASE_VERSION=0.1.0 \
  VELVT_RELEASE_BUILD=1 \
  ...
```

The target additionally requires the production API, appcast, and Sparkle
variables. Read the `release:` recipe in the `Makefile` for the full list rather
than guessing — each missing variable produces a specific error.

Note that `make release` treats DMG outputs as immutable: pick a new versioned
`VELVT_DMG_PATH` for every attempt.

### Step 4 — verify before distributing

```bash
make verify-release-production \
  VELVT_DMG_PATH=dist/Velvt-0.1.0.dmg
```

Then check the artifact directly. All four must pass:

```bash
codesign -dvvv dist/Velvt.app 2>&1 | grep '^Authority='   # Developer ID Application: …
spctl -a -vvv -t exec dist/Velvt.app                      # accepted
xcrun stapler validate dist/Velvt.app                     # ticket stapled
xcrun stapler validate dist/Velvt-0.1.0.dmg               # ticket stapled
```

`Signature=adhoc` or `TeamIdentifier=not set` in the first command means the
build is still not distributable.

### Step 5 — clean-machine gate

Signing correctness cannot be validated on the machine that produced the build.
Before sending the DMG to anyone:

1. Download the DMG on a Mac that has never built Velvt, through a browser, so
   it carries a genuine quarantine flag.
2. Install and launch without running `xattr`.
3. Confirm the app opens, requests Accessibility permission, and reaches a
   working state.

Test on both an Apple Silicon and an Intel Mac if you intend to support both —
the universal binary makes that meaningful.

## Hardening follow-ups

Worth doing so this cannot recur silently:

- Make the ad-hoc fallback at `Makefile:209-211` opt-in via an explicit flag
  (e.g. `VELVT_ALLOW_ADHOC=1`) instead of a silent `echo`, so an unavailable
  identity fails the build.
- Never publish an artifact from `make dmg`. Reserve `dist/Velvt.dmg` for local
  verification and use versioned filenames from `make release` for anything that
  leaves the machine.
- Add the four verification commands in Step 4 to whatever checklist gates a
  release announcement.
