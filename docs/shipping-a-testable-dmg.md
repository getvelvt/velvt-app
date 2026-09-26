# Shipping a DMG a tester can actually install

Goal: a `.dmg` that installs and runs on a Mac that has never built Velvt.

`VELVT_ALLOW_LOCAL_DMG=1 make dmg` produces an ad-hoc signed bundle. It works on the build machine
and crashes on every other Mac — see
[`dmg-crash-on-other-macs.md`](dmg-crash-on-other-macs.md) for why. The fix is a
Developer ID certificate plus notarization, which requires paid Apple Developer
Program membership. There is no way around the membership; nothing else in this
document matters until it exists.

Budget roughly **USD 99/year** and **24–48 hours** for enrolment approval. The
build itself takes about 15 minutes once credentials are in place.

---

## Step 1 — Enrol in the Apple Developer Program

The keychain currently holds only an `Apple Development` certificate, which is
valid on your own registered devices and **not** for distribution.

1. Go to <https://developer.apple.com/programs/> and choose **Enroll**.
2. Sign in with the Apple ID that should own the app. Prefer a company account
   over a personal one — moving an app between teams later is painful.
3. Choose entity type:
   - **Individual / Sole Proprietor** — fastest, approval often same-day. Your
     personal legal name appears as the signer.
   - **Organization** — requires a D-U-N-S number and legal entity verification,
     typically several days to two weeks. The company name appears as the signer.
4. Pay the USD 99 annual fee.
5. Wait for the approval email. You cannot create a Developer ID certificate
   until the account is active.

> If Velvt is already an incorporated entity and you plan to raise, enrol as an
> Organization. Testers see the signer name in the Gatekeeper prompt, and
> switching from a personal name later means re-signing every artifact.

## Step 2 — Create a Developer ID Application certificate

The certificate type matters. `Apple Development` and `Apple Distribution` will
both fail here — only **Developer ID Application** is valid for software
distributed outside the Mac App Store.

1. In Xcode: **Settings → Accounts → your team → Manage Certificates**.
2. Click **+** and choose **Developer ID Application**.
3. Confirm it landed in the keychain:

   ```bash
   security find-identity -v -p codesigning
   ```

   Expect a line beginning `Developer ID Application: … (TEAMID)`. Copy that
   entire string — it is your `VELVT_CODESIGN_IDENTITY`.

The private key exists only on the Mac that created it. **Export it now**
(Keychain Access → export as `.p12`, strong password) and store it somewhere
durable. Losing it means re-issuing and re-signing everything.

## Step 3 — Store notarization credentials

Notarization uploads the app to Apple, which scans it and issues a ticket.
Without a stapled ticket, Gatekeeper blocks the app on first launch.

1. Create an app-specific password at <https://appleid.apple.com> under
   **Sign-In and Security → App-Specific Passwords**. This is not your Apple ID
   password.
2. Store it as a `notarytool` keychain profile:

   ```bash
   xcrun notarytool store-credentials VELVT_NOTARY \
     --apple-id "you@example.com" \
     --team-id "TEAMID" \
     --password "xxxx-xxxx-xxxx-xxxx"
   ```

3. Verify:

   ```bash
   xcrun notarytool history --keychain-profile VELVT_NOTARY
   ```

   An empty history is fine. An authentication error is not.

`VELVT_NOTARY` is the profile name, and it is your `VELVT_NOTARY_PROFILE`.

## Step 4 — Build the DMG

Use `make alpha-dmg`. It signs, notarizes, staples, and verifies, and needs only
the two credentials above:

```bash
make alpha-dmg \
  VELVT_CODESIGN_IDENTITY="Developer ID Application: YOUR NAME (TEAMID)" \
  VELVT_NOTARY_PROFILE=VELVT_NOTARY \
  VELVT_DMG_PATH=dist/Velvt-0.1.0-alpha1.dmg
```

Notarization usually returns in 2–15 minutes. The target fails closed at every
stage, so if it completes, the artifact is distributable.

**Build from a commit, at a new version.** Before it builds anything,
`make alpha-dmg` refuses uncommitted or untracked changes, a version or build
passed on the command line that differs from
`swift-client/Configs/Version.xcconfig`, a version already tagged at another
commit, and a build number that is not above every build in
[`RELEASES.md`](RELEASES.md). So: raise the version and build in
`Version.xcconfig`, commit, and build from that commit. The app and the helper
both record the commit, and on success the target tags `v<version>` locally.
Push the tag and add the release to `RELEASES.md`.

**Pick a new `VELVT_DMG_PATH` for every attempt.** DMG outputs are treated as
immutable and the target refuses to overwrite one.

### Which target to use

| Target | Signing | Installs elsewhere? | Use for |
|---|---|---|---|
| `VELVT_ALLOW_LOCAL_DMG=1 make dmg` | ad-hoc | **No** — crashes | Local verification only |
| `make alpha-dmg` | Developer ID + notarized | **Yes** | Testing alpha, private beta |
| `make release` | Developer ID + notarized + Sparkle appcast | Yes | Public releases with auto-update |

`make release` additionally requires the full Sparkle pipeline — appcast, Ed25519
signing key, update feed URL, seventeen variables in total. You do not need any
of that to put a build in a tester's hands. Defer it until auto-update matters.

## Step 5 — Verify before sending it to anyone

`make alpha-dmg` runs `verify-release-production` for you. To check by hand:

```bash
codesign -dvvv dist/Velvt-0.1.0-alpha1.dmg 2>&1 | grep '^Authority='
spctl -a -vvv -t exec dist/Velvt.app
xcrun stapler validate dist/Velvt.app
xcrun stapler validate dist/Velvt-0.1.0-alpha1.dmg
```

All four must pass. `Signature=adhoc` or `TeamIdentifier=not set` means the build
is still not distributable.

## Step 6 — The clean-machine gate

**Signing correctness cannot be validated on the machine that produced the
build.** This step is not optional; skipping it is exactly how the current
broken DMG shipped.

1. Upload the DMG somewhere and **download it through a browser** on a Mac that
   has never built Velvt, so it carries a genuine `com.apple.quarantine` flag.
2. Install and launch it **without** running `xattr`.
3. Confirm it opens, requests Accessibility, and reaches a working state.

If you must unblock a machine you own during development:

```bash
xattr -dr com.apple.quarantine /Applications/Velvt.app
```

Never put that in instructions for a real tester. If they need it, the build is
broken.

Test on both Apple Silicon and Intel if you intend to support both — the build
is universal, so it is worth confirming.

---

## Verifying notifications actually fire

Signing and notarization get the app installed. Notifications are a separate
chain, and every link has to hold.

### The chain

There are two notification kinds and they take different paths. The daily
insight: Rust pushes a `notification_payload` over the local IPC socket. The
drift offer: Rust sets `active_intervention` on the `work_block_state`
snapshot and Swift posts its Rust-authored copy immediately through
`InterventionNotificationScheduling`. Both reach macOS the same way:

1. Rust decides something is worth saying and sends it over the local IPC
   socket.
2. `NotificationDeliveryCoordinator.handle` checks notification authorization,
   requesting it if the user has never been asked.
3. `NotificationScheduler` hands it to `UNUserNotificationCenter`.
4. macOS displays it, subject to Focus / Do Not Disturb.

Link 2 was broken until recently: the coordinator only *checked* authorization
and dropped anything not already granted, in silence. On a fresh install the
status is `notDetermined`, and the launch sequence never asked — so **no
notification could ever be delivered**, including the daily insight. It now
requests once, at the moment there is something worth showing.

### Fastest check — the debug harness

On a Debug build: **Settings → Debug/Testing → simulate insight**. This drives
the same path as a real Rust push and reports `scheduled`, `permissionDenied`,
or `schedulingFailed`. `Debug/Testing` is `#if DEBUG` only and never appears in
a Release build, so this is unavailable in the DMG.

### End-to-end check — an in-session intervention

Device-local, deterministic, and independent of the cloud, so it works on a
fresh install with no account and no baseline history:

This is drift policy v3 (`DRIFT_POLICY_VERSION = 3`, protocol 32, velvt-app
PR #56). The gate constants are in `rust-service/src/work_block/mod.rs`:
≥ 3 confident switches away from the anchor inside a rolling 10-minute window,
after ≥ 3 minutes elapsed, with ≥ 2 minutes remaining. They are v2's (PR #40,
1.0.9 to 1.0.11); v3 changes when a switch reaches the gate, which is now the
moment it happens. Under v2 the gate heard of a switch only when you came back,
and the offer was withdrawn before a notification could be posted. The first
gate, v1, was ≥ 4 switches after 5 minutes; it no longer ships.

1. Start a work block of **25 minutes** (anything that leaves 2 minutes after
   the switches works).
2. Work in one **FOCUS_WORK** app (VS Code, Xcode, Cursor, Zed) for at least
   **3 minutes** — this establishes the anchor and clears the warm-up.
3. Switch to a different category and back **three or more times within 10
   minutes**, spending long enough in each for a confident classification.
   COMMUNICATION, SOCIAL_FEED and PASSIVE_CONSUMPTION all count; a one-second
   flick does not.
4. On the third switch away, stay in the other app. A notification and an
   in-app card should appear while you are still there, within a second or
   two of arriving. Coming back to the anchor withdraws the offer. The body is
   one of four frozen wordings, for example:

   > Velvt observed 3 switches away from focus work in the last 10 minutes.
   > Protect the next 10 minutes for the work you chose.

   The number is the count actually observed, so it is 3 or more.

At most **one** offer is made per block, by design. An offer is also held back
while Velvt is demoted and while Focus/DND is on (recorded as suppressed, not
delivered later).

### When nothing appears

| Check | How |
|---|---|
| Authorization granted | System Settings → Notifications → Velvt |
| Focus / DND not suppressing it | Control Centre → Focus |
| Service running and connected | Menu bar shows a connected state, not "Collection paused" |
| Accessibility granted | System Settings → Privacy & Security → Accessibility |
| Gates actually met | ≥3 confident switches in 10 min, ≥3 min elapsed, ≥2 min remaining (policy v3) |
| Delivery outcome | `bash scripts/watch_notifications.sh` reports `notification_delivered` or why it was not |
| Already offered this block | One per block — start a new one |

If authorization was denied once, macOS will not prompt again. Re-enable it in
System Settings → Notifications → Velvt, or reinstall to reset the state.

The daily-insight notification is separate: it is cloud-derived and suppressed
until the baseline matures at **3 observed days**. Do not use it to test whether
notifications work — use an in-session intervention, which has no such gate.

---

## Cost and time summary

| Item | Cost | Time |
|---|---|---|
| Apple Developer Program | USD 99/year | 1–2 days (individual), up to 2 weeks (org) |
| Developer ID certificate | included | minutes |
| Notarization credentials | included | minutes |
| `make alpha-dmg` | — | ~15 min per build |
| Clean-machine verification | — | ~10 min, requires a second Mac |

The membership is the only hard blocker, and it is the cheapest item on the
critical path to putting Velvt in front of a real user.
