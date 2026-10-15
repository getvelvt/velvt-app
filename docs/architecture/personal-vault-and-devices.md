# Personal vault and device registry

> **Status:** design for founder review, 2026-10-15. The first seven PRs are
> built and open, none merged: velvt-core #44 (C0), #45 (C1) and #46 (C2), and
> velvt-app #88 (A0), #89 (A1), #90 (A2) and #91 (A3). Nothing here is approved
> until the founder answers §17 and §17.1.
>
> **Scope:** report §6 Phase 3 (steps 3.1–3.5: the in-app device list and the
> end-to-end-encrypted personal vault), and the device-trust layer that Phase 4
> (phone notifications) runs on. The source report is
> `work-logs/2026-10-14-simulation-report.md` §5–6 in the private workspace.
>
> **Grounding:** velvt-app `origin/develop` @ 51ce105 (protocol 35, migrations
> 0001–0043) and velvt-core `origin/develop` @ 6135547. The design composes
> with these open PRs:
> - velvt-core #41 (token lifecycle, Alembic 0018), #42 (cross-device
>   aggregation, 0019) and #43 (per-device delivery, 0020);
> - velvt-app #84 (protocol 36, migration 0044) and #86 (protocol 37).
>
> Protocol and migration numbers below are placeholders. Each PR takes the
> next free number when it merges, as AGENTS.md requires.
>
> **Revised 2026-10-15** after an adversarial review. §18 lists each finding
> and the section that now resolves it.
>
> **Revised again 2026-10-15** after the build reviews of those seven PRs.
> Where the built code and this document disagreed, this document now says
> what the code does; the choices that need the founder are in §17.1. The
> changelog at the top of §18 lists both.

---

## 0. The design on one page

The work ships in two layers: **Sync v1** in Phase 3, then **device trust v2**
on top of it in Phase 4.

**Sync v1 (Phase 3: about 3 weeks of build, plus the security review)**

- Each account has at most one encrypted document, `prefs`. It holds:
  - Mac names;
  - per-Mac notification routing;
  - category rules, as keyed fingerprints and never names;
  - quiet hours and the invitations switch.
- velvt-core stores the document as an opaque, versioned blob with
  compare-and-swap writes. It never holds the key and plays no part in handing
  it out.
- The Rust helper makes the 32-byte vault key `K`.
  - Swift stores `K` as a synchronizable item in the data-protection Keychain,
    so iCloud Keychain carries it to the person's other Macs with no UI.
  - A Mac that cannot get it that way gets it from a one-time 21-character
    **setup code** read off a Mac that has it. Examples: another Apple ID,
    iCloud Keychain off, a managed Mac.
- Every write is signed with a key derived from `K`, and velvt-core checks the
  signature. Someone holding only the password cannot overwrite or delete the
  copy. A reset without the key waits 72 hours and alerts every Mac.
- Synced values that could change what a pre-registered policy decides on
  another Mac are stored and **held**: category rules, quiet hours, the
  invitations switch. Only Mac names and the daily-insight notification switch
  apply at once.

**Device trust v2 (Phase 4, before any phone receives a message)**

- A signed, append-only device chain, verified by a pure Rust crate
  (`velvt-trust`).
  - Every ADD is co-signed by the device being added.
  - Every device pins the chain's root.
  - Vault-key commitments live in the chain.
- Pairing cannot be clicked through:
  - a phone scans a QR code that carries a 256-bit secret;
  - Mac to Mac, each Mac types the code the other one shows;
  - or the person uses a recovery secret.
- The vault key becomes per-epoch. It is wrapped to each chain member with HPKE
  and rotated on every removal.
- The phone relay seals each message to that phone's own key, taken from the
  verified chain.

| Property | Sync v1 | Device trust v2 |
|---|---|---|
| velvt-core reads vault contents | No | No |
| velvt-core adds a device that can decrypt | No. It has no role in key distribution | No. An ADD needs a member's signature **and** the new device's co-signature, after a pairing the person cannot skip |
| Someone with only the password overwrites, deletes or replaces the copy | No. Writes need the write key; a reset without it waits 72 h and alerts every Mac | Same |
| A removed device stops getting new data | Its tokens are revoked and the server stops serving it. `K` does not rotate, so a Mac still in the person's iCloud Keychain circle keeps it (the UI says so) | Yes. The vault key rotates on every removal |
| A website hostname, app name or bundle id leaves the Mac | No, not even encrypted | No |
| Sync changes drift-gate inputs on another Mac | No: held behind three locks (§11) | No |
| New Rust crates | None (`ring`, `base64`, `zeroize` and `subtle` are already compiled into today's build through `reqwest` and `rustls`) | Two, plus their transitive crates (`x25519-dalek` and an HPKE crate), and `uniffi` for the iOS bindings |

### 0.1 Where each part comes from

Three candidate designs were judged by three independent reviewers:
**minimal-first** (one encrypted blob, iCloud Keychain or a setup code),
**security-first** (a `velvt-trust` crate and a device chain), and
**UX- and phone-first** ("Pair Once", a Swift CryptoKit package). All three
reviewers chose minimal-first as the base. This document starts from it,
adds the ideas the reviewers agreed were best, and resolves every flaw they
called fatal.

| Part of this design | Source |
|---|---|
| One `prefs` blob with CAS; `K` carried by iCloud Keychain or a setup code; held-versus-applied boundary; person keys for rules; const gate | minimal-first |
| Socket peer code-signature check before any secret crosses the socket (PR A0) | security-first |
| Person keys computed at labeling time, while `local_site_name` still holds the host | security-first |
| Erasure removes or anonymizes audit rows | security-first |
| Provenance columns added now, while dark | security-first and Pair Once |
| Per-Mac toggle as a second lock; the apply rung compiled out with `cfg`; no needs-a-category prefill from synced rules | Pair Once |
| Clone detection (Migration Assistant) | Pair Once |
| "New sign-in" and "a Mac joined sync" alerts | Pair Once and security-first |
| Recent re-authentication for removing a device and deleting synced data | security-first and Pair Once |
| A key change treated as a security event; delayed, cancellable reset | security-first |
| Egress dry-run vault mode, fuzzing, a malicious-server test double, test vectors | security-first |
| Sync-key fingerprint shown in the device list | security-first |
| Phase 4 chain: co-signed ADDs, pinned root, epoch commitments, rotation on revoke | security-first |
| Phase 4 QR enrollment-token flow, Approve with iPhone, Log Out keeps membership, cap eviction skips chain members, journey copy | Pair Once |
| **New in this synthesis:** a write key derived from `K` that velvt-core verifies; signed rotation and deletion statements; a "server inconsistent" state that never offers code entry; the Phase 4 trust layer reusing the v1 blob store rather than a per-record feed | resolving the judges' findings |

### 0.2 The flaws judged fatal, and how they are resolved

| Flaw (design) | Resolution here |
|---|---|
| Comparing the pairing code was optional. One careless click on **Add** let a key-swapping server or a password thief in. (Pair Once) | No path adds a decrypting device on a click. In v1, `K` arrives only from the person's own iCloud Keychain or a code typed by hand. In v2, a phone must scan a QR code that carries a 256-bit secret, and Mac to Mac each side must type the other's code. Approval while away from the new device works only by QR or the recovery secret (§7.6) |
| Chain verification, vault-key custody and decryption sat in Swift, served to anyone who could reach the socket. (Pair Once) | Rust owns every judgement and all cryptography: the `vault` module in v1, `velvt-trust` in v2. Swift only stores secrets in the Keychain and hands them to Rust, the same way `auth_session` works today. PR A0 adds a peer check on both ends of the socket before any secret crosses it: each end serves only the process the launch relates it to, and checks its code signature (§9.3) |
| Synced rule bodies carried `activity_name`, which for rules taught from the needs-a-category list is the application's own local name. (security-first) | No activity name is ever synced (§10.2). If a later version syncs typed names, it carries `activity_name_source` and never queues `app_local_name` |
| Synced quiet hours and the invitations switch were applied directly on another Mac. (security-first) | Both are held, as are category rules. So is reminder routing, which CATEGORY_PROMPT_POLICY_VERSION governs (§11) |
| Audit rows with a `user_id` outlive erasure. (all three) | PR C0: erasure deletes the account's sync and device audit rows, and nulls the user and device ids on the rest (§8.6) |
| A password holder could PUT an unreadable blob or one under a new key, then steer the person to "Enter a code" or "Delete synced data". (minimal-first) | velvt-core accepts a write only if it carries a signature by the write key. A key change needs a signature by the old key, or a 72 h public waiting period. A key change the Mac cannot verify raises a security alert and never offers code entry (§6.1) |
| The Keychain item keyed by `user_id` alone let a raced or forged 404 overwrite or delete the real key on every Mac. (minimal-first) | The item's account is `user_id.key_id`. A Mac never creates a vault while it holds any sync item for the account. A 404 never triggers automatic creation (§5.5) |
| `notify.category_reminder` applied at once changed the labeling rate, and so the drift gate's evidence. (minimal-first) | Reminder routing is not in v1. It arrives with a CATEGORY_PROMPT_POLICY_VERSION bump that records the routing gate on every reminder decision (§11) |
| A relay key derived from `K` with HKDF cannot revoke a lost phone. (minimal-first) | Not adopted. Phase 4 seals relay messages per device with HPKE, using keys from the verified chain (§5.9) |

---

## 1. Goals, non-goals and invariants

### 1.1 Goals

- **G1. Choices follow the person.** Everything a person sets up follows them to
  every Mac they add, and velvt-core cannot read it:
  - in v1: Mac names, notification routing, category rules from the
    needs-a-category list and corrections, quiet hours and the invitations
    switch;
  - later: the per-person return ledger and phone routing.
- **G2. The server cannot add a reader.** velvt-core cannot add a device that
  can decrypt, swap a device's keys, or give two devices different vault keys
  without that being detected.
- **G3. A password is not enough.** Someone who holds only the account password
  cannot read, overwrite or silently replace the vault.
- **G4. Removal is visible and real.** A removed device stops receiving new
  data. In v2 that is cryptographic. In v1 it is enforced by the server, and
  the UI states the iCloud Keychain caveat.
- **G5. Erasure is complete.** Account erasure leaves no vault, handoff,
  device-trust or audit row that holds the user's id on the server once the
  worker's next erasure sweep has run (at most 5 minutes after the erasure).
  Database backups taken before the erasure keep the pre-erasure rows until
  they age out (at most 14 days); the vault part of them is ciphertext only.
- **G6. Measurement is untouched.** No synced value changes a drift-gate,
  initiation, demotion, reminder or digest input on any Mac until a
  pre-registered policy version says so.
- **G7. Phones reuse the same layer.** Phase 4 reuses the same identities,
  envelopes and blob store. Phones get no second trust system.
- **G8. Small first release.** v1 ships with no new Rust crate and no new trust
  state machine, in about three weeks plus the review.

### 1.2 Non-goals

- Hiding metadata from velvt-core and Cloudflare: that sync is used, when the
  copy changes, its padded size, device count and IP addresses.
- Availability against a malicious server. It can refuse, delay, withhold or
  delete. Every Mac keeps its own local tables, so nothing is lost.
- Fork detection in v1. A server that shows two Macs different versions is
  accepted for preferences. v2 adds witnessing.
- Protection from malware running as the user on a trusted Mac.
- Syncing measurement data (work blocks, decisions, Focus evidence, outcomes),
  window-level corrections, activity names or history. History is already
  stored per user on the server.
- CloudKit as storage. It is Apple-only, needs the same Apple ID, and gives
  velvt-core's erasure and account model nothing.
- An LLM in the cloud, or any classification off the Mac.

### 1.3 Invariants (each one has a test, listed in §16)

- **I1.** velvt-core never holds `K`, a vault key of any epoch, a device private
  key, vault plaintext, relay plaintext or a device name.
- **I2.** No hostname, URL, app name, bundle id, window title, activity name or
  unsalted digest appears in vault plaintext, in SQLite outside the tables
  that already hold them, in IPC sync messages or in egress bodies.
- **I3.** Rust performs all vault and trust cryptography and makes every trust
  judgement. Swift stores secrets and reports facts: what the Keychain holds,
  what the person typed or clicked, the computer name.
- **I4.** A vault secret crosses the local socket only after both ends have
  passed the peer check (PR A0) in release builds: the helper's peer is its
  parent and the app's peer is its child, and each satisfies the other's code
  requirement.
- **I5.** With `VAULT_POLICY_INPUTS_VERSION = None`, the drift decisions, the
  decision log and the observations over a recorded trace are byte-identical
  with and without a populated vault.
- **I6.** A Mac never overwrites a server copy it could not open and validate.
  It never creates a vault on its own because of a 404, and never adopts a new
  key without a signed rotation or the person's explicit action after an alert.
  It never adopts a key that a Keychain item it holds supersedes or marks
  retired, and no server claim alone deletes or rewrites a Keychain item.
- **I7.** All synced-data copy is Rust-authored and passes
  `scripts/check_banned_copy.py`. No "learn" in any form, so no "Learn more"
  links; use "How this works".

---

## 2. User journeys

Copy shown in quotes is Rust-authored unless it is a static Swift label. Every
string passes the banned-copy check. The Settings section is called **Your
Macs** in v1 and **Your devices** once a phone can join (v2).

### J1. Turn on sync on the first Mac

1. Settings › Your Macs › toggle **Keep my choices in sync across my Macs**.
   It is off by default.
2. Under the toggle (static): "Your Mac names, notification choices, category
   rules, quiet hours and invitation setting are kept in an encrypted copy on
   Velvt's server. The key stays on your Macs and in your iCloud Keychain, so
   Velvt can't read it. Site and app names are never in it. How this works"
3. Turning it on takes under a second: Rust creates the vault (§10.4), and the
   section shows the list in J4.

### J2. Second Mac, same Apple ID, iCloud Keychain on (zero-touch)

| t | New Mac |
|---|---|
| 0:00 | Signs in with email and password, as today |
| 0:05 | Rust sees that a vault exists and Swift finds the key in the Keychain. The popover shows a one-time card: **"Your other Mac keeps your choices in sync. Turn on here?"** [Turn On] [Not Now] |
| 0:07 | [Turn On] fetches and opens the copy. Line: "Synced. Mac names and notification choices are here. Categories, quiet hours and the invitation setting from your other Macs are saved; this Mac uses its own for now." |

The other Macs post the alert **"A Mac joined your sync"**: "“Mac mini” turned
on sync at 10:42 using your iCloud Keychain. Not you? Remove it in Settings ›
Your Macs."

### J3. Second Mac without the key: setup code

The key can be missing because of iCloud lag, a different Apple ID, iCloud
Keychain being off, or MDM.

1. The new Mac shows: "Waiting for your sync key from iCloud Keychain. Or add
   this Mac with a code from a Mac that already syncs." [Enter a Code]. Swift
   checks the Keychain again on activation and every 60 s for 10 minutes.
   - The panel also says when the copy was made: "Sync was set up on your
     account on 12 Oct. Not you? Start a New Synced Copy." Someone with the
     password could set up a copy under their own key before the person ever
     turns sync on. They still cannot read anything, but without this line the
     person would only see a key that never arrives. `POST /v1/vault` also
     emails the account (§8.3).
2. On a syncing Mac: Your Macs › **Add a Mac with a code**. The sheet shows
   `K7PQ-M2XD-9HTW-4RZN-8CJ5H` in large monospace with a 10-minute countdown,
   and: "Type this on your other Mac. It works once, for 10 minutes. Velvt never
   sends codes by email or message."
3. On the new Mac: **Enter a code** › the person types it. Above the field:
   "Only type a code shown on the screen of one of your own Macs."
4. Errors:
   - typo: "Check the code for a typo." Caught by the check character, with no
     network call.
   - unknown or expired: "That code didn't work. Codes last 10 minutes and work
     once."
5. Success shows the line from J2. Every other Mac posts: "“Mac mini” turned on
   sync at 10:42 using a setup code. Not you? Remove it in Settings › Your
   Macs."

### J4. The device list (Settings › Your Macs)

```
Your Macs
Synced choices are end-to-end encrypted. Velvt's server can't read them.
Sync key 4821 9133 0577 — the same on every Mac that syncs.

  Studio Mac        This Mac · Syncing                      [Rename]
  MacBook Pro       Active today · Syncing                  [Rename] [Remove…]
  Mac               Signed in 3 days ago · Not syncing       [Remove…]
  Mac mini          Needs Velvt 1.1 or later to sync         [Remove…]

  Notifications on this Mac:  ☑ Daily insight
  Categories, quiet hours and the invitation setting from your other Macs are
  saved in your encrypted copy. Using them on this Mac is off for now.

  [Add a Mac with a code]          [Turn Off Sync on This Mac]   [Delete Synced Data…]
```

- **Rows** come from joining `GET /v1/devices` (server facts: id, status,
  `last_seen_at` from #41, `client_version`, `is_current`) with the document's
  `devices` map (names, routing, `removed`). Rust builds the rows; Swift
  renders them.
- **Last seen** uses buckets: Active now (within 15 minutes, #41's write
  interval), Today, Yesterday, N days ago, Over a month ago.
- **Sync key fingerprint:** 12 digits from the write public key (§5.3). Every
  Mac holding the same key shows the same digits, so a person can check two
  Macs by eye.
- **Rename** writes that Mac's `name` register. Any syncing Mac can rename any
  Mac.
- **Remove…** (another Mac):
  - Sheet: "Remove “MacBook Pro”? It will be signed out of Velvt and stop
    syncing. If it uses your Apple ID, it still has the sync key in iCloud
    Keychain: sign it out of iCloud, or erase it in Find My, if you no longer
    have it."
  - Field: "Enter your Velvt password to continue." Checkbox (v1.1, PR A8):
    "It was lost or stolen: change the sync key".
- **Turn Off Sync on This Mac** marks this Mac's entry removed and ends its
  sync session, keeping the trust anchor (J7). The key stays in the Keychain,
  because deleting a synchronizable item deletes it from every Mac.
- **Delete Synced Data…** (J9).

### J5. A lost or stolen Mac

"Lost a Mac?" panel (static):

> Remove it from this list. It's signed out the next time it connects, and
> Velvt's server stops sending it your synced copy. If it's still signed in to
> your Apple ID it keeps the sync key, so remove it from your Apple ID in Find
> My first, then choose “change the sync key” when you remove it here.

With the change-the-sync-key option (PR A8), the remover makes `K'` and
re-seals the copy. It signs the change with the old write key (a signed
rotation, §5.6) and stores `K'` in iCloud Keychain.
- Other Macs on the same Apple ID pick up `K'` without a prompt, because the
  rotation verifies.
- A Mac on another Apple ID posts J6's `sync_key_changed`: "Another of your
  Macs changed the sync key at 10:42. Add this Mac with a code from one of
  your other Macs."

### J6. Security notices (a fourth notification kind)

AGENTS.md lists three notification kinds. This adds a fourth, the **security
notice**, and AGENTS.md is updated in the same PR. Rust decides when one is
due and writes its copy, and Swift posts it.

Rules for posting:
- It is never time-sensitive, and it respects macOS Focus.
- It is outside the one-offer-per-block rule and the daily needs-a-category
  cap.
- It is **never posted during a work block.** Rust holds it until the block
  ends, like the needs-a-category reminder. Clicking a notification is an app
  switch, and the drift gate counts app switches, so a notice mid-block would
  change drift evidence. The device list and status line show it at once.
  The longest hold is one block, which is far shorter than the 72-hour reset
  window it may be warning about.
- Each notice is posted once, and the device list shows it until it is
  acknowledged.

| Kind | Title | Body |
|---|---|---|
| `new_sign_in` | New sign-in to your Velvt account | "A Mac signed in at 10:42. If this wasn't you, change your password and remove it in Settings › Your Macs." Names no device: the server holds no names |
| `mac_joined_sync` | A Mac joined your sync | "“Mac mini” turned on sync at 10:42 using a setup code. Not you? Remove it in Settings › Your Macs." When the joined Mac has no name in the last opened copy: "A Mac turned on sync at 10:42 using a setup code. …" |
| `this_mac_removed` | This Mac was removed | "This Mac was removed from your Velvt account. Its own categories and settings stay on this Mac. Sign in to keep using Velvt." Names no Mac: velvt-core does not say which Mac removed this one, and no longer serves it the copy |
| `sync_key_changed` | Your sync key changed | "Another of your Macs changed the sync key at 10:42. This Mac picked up the new key." or "…Add this Mac with a code from one of your other Macs." Names no Mac: no rotation statement says which Mac signed it (§5.3; the issuer tag is §17.1 D13) |
| `reset_requested` | Someone asked to reset your synced copy | "A reset of your synced copy was requested at 10:42 without your sync key. It happens on 18 Oct at 10:42 unless you cancel. If this wasn't you, cancel it and change your password." [Cancel Reset] |
| `reset_completed` | Your synced copy was reset | "Your synced copy was reset on 18 Oct after a 3-day notice. This Mac kept its own choices." [Set Up Sync Again] |
| `server_inconsistent` | Sync paused to keep your data safe | "Velvt's server showed this Mac a change to your synced copy that none of your Macs signed. Nothing on this Mac changed. Sync is paused." [How this works] |
| `server_copy_unreadable` | (status line only, no notification) | "Sync paused: the copy on Velvt's server can't be opened on this Mac. Nothing on this Mac changed." |

### J7. Sign out and back in

- **Log Out** ends this Mac's sync session and drops `K` from memory. It never
  deletes the Keychain item. It keeps this account's **trust anchor** (the
  trusted `key_id` and `write_pk`, the highest version seen, the reset
  bookkeeping and this Mac's `sync_device_id`; §4.3). The anchor holds no
  secret and no content. Without it, a Mac that logged out for five minutes
  would be as easy to fool as one offline for 72 hours (§6.1), and every
  re-sign-in would add a new device entry to the document.
- Signing back in to the same account shows the J2 card again. One click turns
  sync back on, and §6.1 checks the server's copy against the kept anchor.
- Signing in to **another** account uses that account's own `vault_state`
  row. Rows and Keychain items are per account, so nothing crosses accounts.

### J8. Migration Assistant, a restored backup or a new logic board

Migration Assistant copies the SQLite database, and with it `vault_state` and
`sync_device_id`.
- Rust keeps a **host binding**: an HMAC, under a random per-database salt, of
  the host UUID that `gethostuuid(2)` returns. Only the HMAC is stored, and it
  never leaves the Mac.
- On a mismatch at launch, Rust mints a new `sync_device_id`, turns sync off on
  this Mac, and shows a card: "This Mac was set up from another Mac's backup.
  Turn on sync here?" [Turn On]. The source Mac's entry stays its own.

### J9. Delete synced data, and a reset without the key

- **With the key** (any syncing Mac):
  - Settings › Delete Synced Data… asks for the password. Copy: "Removes the
    encrypted copy from Velvt's server and the sync key from your iCloud
    Keychain. Choices saved on each Mac stay on that Mac."
  - The deletion is signed with the write key and takes effect immediately.
  - Other Macs verify the signed tombstone and turn sync off quietly with the
    line "Sync was turned off from another of your Macs." The delete statement
    names no Mac (§5.3; the issuer tag is §17.1 D13).
- **Without the key** (every Mac lost, and no iCloud Keychain):
  - The new Mac sees `waiting_for_key`. The panel "I don't have my other Macs"
    offers **Start a New Synced Copy…**, which needs the password.
  - Copy: "Velvt can't open your existing copy without a Mac that has the key.
    A new copy starts from this Mac's choices. To protect you if someone else
    has your password, the old copy is kept for 3 days and every Mac that
    still syncs is told. We'll also email you."
  - After 72 hours the old copy is deleted and this Mac can create a new one.
  - During those 72 hours any Mac holding the key can cancel the reset (J6
    `reset_requested`).
  - **Any sign-out a Mac did not ask for pushes the reset out.** That is:
    - removal from another Mac (`DELETE /v1/devices/{id}`);
    - `POST /v1/auth/revoke-devices`;
    - eviction past the device cap of a device that was still signed in
      (`device_limit_reached`);
    - a reissue of a signed-in device's token
      (`POST /v1/auth/devices/reissue`).

    A reset takes effect 72 hours after the later of the request and the
    account's most recent removal of one device by another or cap eviction.
    Each of the sign-outs above, while the reset is pending, moves it to 72
    hours after that sign-out. Only removal needs a re-authentication; the
    other three need only the password. Otherwise someone with the password
    could sign every Mac out (they then stop polling and cannot cancel) before
    or late in the window. A removed Mac posts `this_mac_removed`, and any
    signed-out Mac can sign back in within the window and cancel.
  - **A password reset cancels it.** Completing a password reset through the
    emailed link cancels any vault reset requested before it, and the email
    says so. Whoever requested the reset knew the password, but a password
    reset proves control of the email.

### J10. Delete account

This is the existing flow, extended:
- Rust also asks Swift to delete this account's sync key items.
- Other Macs receive `account_deleted` (#34's state). They clear their sync
  state and delete the same items. Deleting an iCloud item that is already
  gone is a no-op.
- **Open (§17.1 D22).** This conflicts with I6, and the A3 build (#91) follows
  I6: a server-reported `account_deleted` deletes the account's sync rows and
  keeps the Keychain items; only this Mac's own Delete Account deletes them.

### J11. (Phase 4) Add an iPhone

1. Mac: Settings › Your devices › **Add an iPhone…** shows a QR code with: "Scan
   this with your iPhone's camera. No Velvt app yet? The code opens the App
   Store; install Velvt, then scan again from the app. The code works once, for
   10 minutes."
2. iPhone: **Scan Code on Your Mac** › "Connecting to “Studio Mac”… Confirm on
   your Mac."
3. Mac sheet: **Add “Kevin's iPhone”?** "It just scanned the code on this Mac."
   [Not Mine] [Add iPhone].
   - No code comparison is needed: the QR is the out-of-band channel.
   - The tap is still required, because someone could photograph the QR.
4. iPhone: **What should reach this iPhone?**
   - Weekly receipt (on). Work block results (on). Soft-start invitations
     (off). Daily insight (on).
   - Static line: "Drift nudges stay on your Mac, where you are when they
     happen." This holds until report step 4.5 has its own pre-registration.
5. iPhone: **Seeing Velvt twice on your Mac?** "Your Mac can show iPhone
   notifications. To keep one copy, turn off Show on Mac for Velvt in iPhone
   Settings › Notifications › Velvt."

### J12. (Phase 4) A new Mac when the trust layer is on

The person picks one of these routes:
- **iCloud Keychain** (if the founder keeps that path, §17 Q15): automatic.
  Every device posts "“Mac mini” joined using your iCloud Keychain."
- **Approve with iPhone:** the new Mac shows a QR code, and the iPhone scans it
  and taps [Add].
- **Two codes:** each Mac shows six digits, and the person types each one on the
  *other* Mac. Copy on the old Mac: "Type the code shown on “Mac mini”." On the
  new Mac: "Type the code shown on “Studio Mac”."
- **Recovery key**, if the person made one.

There is no "do the codes match? [Add]" button anywhere.

---

## 3. Architecture

```
 Mac A                                                          Mac B
 ┌──────────────────────────────────┐                          ┌──────────────────────────────────┐
 │ Velvt.app (Swift)                │   iCloud Keychain (E2E)  │ Velvt.app (Swift)                │
 │  Sync/SyncKeyStore ──────────────┼◀════════════════════════▶┼── Sync/SyncKeyStore              │
 │   data-protection Keychain,      │  (synchronizable item,   │  Sync/YourMacsSection,           │
 │   item com.velvt.sync/<uid>.<kid>│   access group           │  EnterCodeSheet, SyncCoordinator │
 │  Sync/YourMacsSection, sheets    │   <TEAM>.com.velvt.shared)│                                 │
 │  IPC client + peer check (A0)    │                          │  IPC client + peer check (A0)    │
 │        ▲  protocol 38            │                          │        ▲                         │
 │ velvt-service (Rust)             │                          │ velvt-service (Rust)             │
 │  ipc + peer check (A0)           │                          │                                  │
 │  vault::crypto  (ring)           │                          │  vault::…                        │
 │  vault::doc / merge / code       │                          │                                  │
 │  vault::sync   (loop, CAS, verify)│── HTTPS, ciphertext ──┐ │  vault::sync                     │
 │  vault::mirror (person keys)     │   + write signatures  │ │  vault::mirror                   │
 │  vault::apply  (routing; held)   │                       │ │                                  │
 │  egress ledger + ENDPOINTS       │                       ▼ │                                  │
 │  SQLite: vault_state,            │        velvt-core (FastAPI/Postgres, behind Cloudflare)    │
 │   vault_rule_link,               │        vaults · vault_blobs · vault_handoffs ·             │
 │   sync_known_device              │        vault_tombstones · vault_reset_requests ·           │
 └──────────────────────────────────┘        devices (+ list, remove another) · auth/reauth      │
                                                                                                 │
 Phase 4 adds: velvt-trust crate (chain, HPKE wraps, pairing), /v1/trust/*, /v1/relay/*, iOS app + NSE
```

### 3.1 Division of labour (AGENTS.md: "Swift reports facts; Rust owns every judgement")

| Concern | Owner |
|---|---|
| Creating `K`, the write key, sealing, opening, signing, verifying, code derivation, merging, deciding what to write and when, the state machine, every state line and notification's copy | Rust (`rust-service/src/vault/`) |
| Storing `K` in the data-protection Keychain; reporting which key ids it holds; the computer name; what the person typed or clicked; posting notifications | Swift (`swift-client/Sources/VelvtMac/Sync/`) |
| Storing ciphertext, version CAS, verifying write signatures, the handoff mailbox, the delayed reset, the device list, revocation, erasure | velvt-core |
| Every cloud call | Rust only, through `ReqwestHttpClient`, the egress ledger and `egress::ENDPOINTS`. Swift makes no cloud call |

Why `K` is stored by Swift and not Rust: the Keychain is a Swift API, and the
codebase already keeps durable credentials there. Rust holds the session Swift
gives it in memory only (`VolatileTokenStore`), and `K` follows the same
pattern. The tokens that cross the socket today get the same protection from
PR A0.

### 3.2 Flags (everything is off by default)

| Flag | Where | Effect while off |
|---|---|---|
| `SYNC_UI_ENABLED` | Swift build setting, off in Release, on in Debug | No Your Macs section, no card; the helper is never asked to turn sync on, so no vault endpoint is ever called |
| `VELVT_VAULT_ENABLED` | velvt-core setting, default `false` | `/v1/vault/*` answers 404 `feature_disabled` |
| `vault-policy-inputs` cargo feature, plus `VAULT_POLICY_INPUTS_VERSION: Option<u32> = None` | `rust-service` | The apply rung and the held appliers are not compiled in (§11) |
| `VELVT_DEVICE_TRUST_ENABLED` (Phase 4) | velvt-core | `/v1/trust/*` and `/v1/relay/*` answer 404 |

---

## 4. Data model

### 4.1 The `prefs` document, v1 (plaintext exists only in Rust memory)

```json
{
  "schema": 1,
  "person_salt": "<base64, 32 random bytes; written once at creation, never changed>",
  "devices": {
    "<sync_device_id, uuid v4>": {
      "joined_via": "created | icloud_keychain | code",
      "joined_at_ms": 1760500000123,
      "server_device_id": {"value": "<velvt-core device id>", "hlc": "…"},
      "name":                 {"value": "Studio Mac", "hlc": "…"},
      "notify_daily_insight": {"value": true,         "hlc": "…"},
      "removed":              {"value": false,        "hlc": "…"}
    }
  },
  "rules": {
    "site:<64 hex person key>":   {"category": "SOCIAL_FEED",   "deleted": false, "hlc": "…"},
    "bundle:<64 hex person key>": {"category": "FOCUS_WORK",    "deleted": false, "hlc": "…"},
    "app:<64 hex person key>":    {"category": "COMMUNICATION", "deleted": false, "hlc": "…"}
  },
  "settings": {
    "invitations_enabled": {"value": false, "hlc": "…"},
    "quiet_hours": {"value": {"start_local_minutes": 1320, "end_local_minutes": 420, "rule_version": 1}, "hlc": "…"}
  }
}
```

- **The only free text is a Mac's `name`.** It is 1–64 characters with no
  control characters. The default is the computer name, which is editable.
- **HLC** is `<13-digit unix ms>.<4-digit counter>.<sync_device_id>` and is
  compared as a string. Each Mac issues `max(now_ms, highest_seen_ms)` plus a
  counter, so its own write always beats what it has already seen.
- **Validation on every open.** A failure leaves the blob untouched and is
  never overwritten.
  - `schema == 1`. A higher schema puts this Mac in read-only mode: "Update
    Velvt on this Mac to keep syncing." A schema-1 document is parsed
    strictly. A higher schema is parsed leniently, and only for the two
    applied fields (names and the daily-insight switch); nothing else from it
    is read.
  - `category` is in the `mvp-2` enum.
  - Person keys are 64 lowercase hex characters.
  - Minutes are 0–1439.
  - `joined_via` is in its enum.
  - At most 32 live devices, 256 device entries in all, and 2,000 live rules.
  - Unknown top-level or entry fields are rejected (`deny_unknown_fields`).
- **A writer never produces a document that fails validation.** Past 2,000
  live rules, the Mac stops mirroring new rules and the status line says so,
  and a merge never exceeds 2,000 live rules, 32 live Macs or 256 entries:
  past a cap, this Mac's additions wait, highest key first (A1's
  `rules_over_cap` and `devices_over_cap`). Otherwise one large rule set, or
  two Macs each adding the 2,000th rule, would make the copy unreadable on
  every Mac.
- **Size.** Deleted rules, and device entries marked `removed`, are dropped on
  write once their register is older than 180 days. The padded document stays
  under 512 KiB: 2,000 rules at about 110 bytes each is about 220 KiB.
- **Reserved for schema 2,** with the policy-version bumps in §11:
  `notify_category_reminder`, and a typed `activity_name` with
  `activity_name_source` if the founder ever approves name sync.

### 4.2 velvt-core tables, v1 (Alembic `…_0021_vault`, after #41–#43's 0018–0020)

```
vaults                                   -- one row per account that uses sync
  user_id            FK users.id, PK
  key_id             CHAR(16) NOT NULL   -- hex; = SHA-256("velvt-vault-key-id-v1\0" ‖ write_pk)[0..8]
  write_pk           BYTEA(32) NOT NULL  -- Ed25519 public key derived from K (§5.3)
  rotations          JSONB NOT NULL DEFAULT '[]'  -- every signed rotation statement for this vault, oldest first (≤ 512 B each)
  created_at, updated_at TIMESTAMPTZ NOT NULL

vault_blobs
  user_id            FK vaults.user_id, part of PK
  name               VARCHAR(32), part of PK  -- 'prefs' only in v1; namespaced for the return ledger later
  version            BIGINT NOT NULL          -- chosen by the writer, strictly greater than the current one (§8.3)
  base_version       BIGINT NOT NULL          -- the version this one replaced (0 for the first); the put signature binds it
  schema             SMALLINT NOT NULL
  ciphertext         BYTEA NOT NULL           -- exactly one of six lengths: 16, 32, 64, 128, 256 or 512 KiB + 28 B (§5.3)
  put_sig            BYTEA(64) NOT NULL       -- the writer's signature over put_stmt, handed back on every read (§6.1)
  updated_at         TIMESTAMPTZ NOT NULL

vault_handoffs                           -- the setup-code mailbox
  id                 CHAR(32) PK         -- hex(16 B) derived from the code; meaningless without it
  user_id            FK users.id, UNIQUE -- one live handoff per account
  payload            BYTEA NOT NULL      -- ≤ 256 B; sealed under a key only the code yields
  expires_at         TIMESTAMPTZ NOT NULL, indexed
  created_at         TIMESTAMPTZ NOT NULL

vault_tombstones                         -- lets other Macs verify a signed deletion
  user_id            FK users.id, PK
  key_id             CHAR(16) NOT NULL
  statement          BYTEA NOT NULL      -- signed deletion statement, ≤ 256 B
  deleted_at         TIMESTAMPTZ NOT NULL, indexed   -- swept after 30 days

vault_reset_requests                     -- a reset without the key, visible to every Mac
  id                 UUID PK
  user_id            FK users.id, UNIQUE where status = 'pending'
  key_id             CHAR(16) NOT NULL   -- the key it would replace
  status             VARCHAR(16)         -- pending | cancelled | completed
  requested_at, effective_at, resolved_at TIMESTAMPTZ
```

These tables store no device id, device name, IP address or content-derived
metadata. The device that acted is recorded only in the content-free audit
event, which erasure removes (§8.6). The `devices` table gets no new columns in
v1. As built in C2 (#46, Alembic `20261015_0021`), a reset request's `id` is
its UUID as text, and one pending reset per account is a partial unique index.

### 4.3 Helper SQLite, v1

Two migrations. Each is additive, joins `MIGRATED_TABLES`
(`tests/published_claims.rs`), PRIVACY.md's storage and "Every column" tables,
and `migrations/CHECKSUMS` in the same commit.

**`00NN_decision_provenance.sql` (PR A2, independent of sync)**

```sql
ALTER TABLE work_block_observation ADD COLUMN classification_source TEXT
  CHECK (classification_source IS NULL OR classification_source IN
    ('seed','heuristic','embedding','user_rule','declared_document_types',
     'declared_app_category','fallback','vault_rule'));
ALTER TABLE intervention_decision_log ADD COLUMN synced_rule_evidence INTEGER NOT NULL DEFAULT 0
  CHECK (synced_rule_evidence >= 0);
```

`classification_source` is written for every new observation from the existing
`ClassificationSource`. A report that the open row absorbs (same category,
status and confidence, so no row of its own) sets the row's source when its
rung outranks the row's: `vault_rule`, then `user_rule`, then every other
rung, which never replaces the opener. A row therefore names a rule whenever a
rule classified any report it holds. (As built in A2, #90; the meaning is
§17.1 D21.) `vault_rule` is accepted by the CHECK but cannot be
produced while the apply rung is compiled out. `synced_rule_evidence` stays 0
until then.

**`00NN_vault_state.sql` (PR A3)**

```sql
CREATE TABLE vault_state (                   -- one row per account that has used sync on this Mac
  user_id TEXT PRIMARY KEY,
  -- the trust anchor: kept across Log Out, Turn Off Sync, removal and account switch (J7)
  sync_device_id TEXT NOT NULL CHECK (length(sync_device_id) = 36),
  host_binding_salt BLOB NOT NULL CHECK (length(host_binding_salt) = 32),
  host_binding BLOB NOT NULL CHECK (length(host_binding) = 32),   -- HMAC(salt, host UUID); J8
  key_id TEXT CHECK (key_id IS NULL OR length(key_id) = 16),
  write_pk BLOB CHECK (write_pk IS NULL OR length(write_pk) = 32),
  highest_seen_version INTEGER NOT NULL DEFAULT 0,               -- rollback guard
  highest_seen_sha256 BLOB CHECK (highest_seen_sha256 IS NULL OR length(highest_seen_sha256) = 32),
                                             -- hash of the blob at that version; a different blob at the same version is a fork
  fetched_at INTEGER,                        -- this Mac's clock at its last successful fetch
  seen_reset_request_id TEXT,                -- the last pending reset this Mac saw, for reset checks (§6.1)
  -- the session: cleared on Log Out, Turn Off Sync, removal and account switch
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  server_version INTEGER,
  ciphertext BLOB,                           -- last opened blob, still encrypted; no plaintext at rest
  last_hlc TEXT,
  state TEXT NOT NULL DEFAULT 'off',         -- the §10.4 state machine
  last_error TEXT                            -- an error code only
);
CREATE TABLE vault_rule_link (               -- which local rule became which vault entry
  local_kind TEXT NOT NULL CHECK (local_kind IN ('site','bundle','app')),
  local_key_hash TEXT NOT NULL CHECK (length(local_key_hash) = 64),
  person_key TEXT NOT NULL CHECK (length(person_key) = 64),
  category TEXT NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0,1)),
  hlc TEXT NOT NULL,
  PRIMARY KEY (local_kind, local_key_hash)
);
CREATE TABLE sync_known_device (             -- for the new-sign-in notice
  server_device_id TEXT PRIMARY KEY,
  first_seen_at INTEGER NOT NULL,
  alerted_at INTEGER                         -- when the notice was queued (sync_notice keeps it until posted)
);
CREATE TABLE sync_notice (                   -- J6 notices owed, so one a work block holds survives a restart
  user_id TEXT NOT NULL,
  notice_id TEXT NOT NULL,
  kind TEXT NOT NULL,                        -- the J6 kinds that post a notification
  subject TEXT,                              -- an id (device, sync device, key, reset, or an error code), never a name
  at INTEGER NOT NULL,                       -- the time the copy names
  until_at INTEGER,                          -- a reset's effective time
  flag INTEGER NOT NULL DEFAULT 0,           -- the one switch the copy reads (adopted, while offline, joined by code)
  queued_at INTEGER NOT NULL,
  posted_at INTEGER,
  acknowledged_at INTEGER,
  PRIMARY KEY (user_id, notice_id)
);
```

The copy of a notice is written when it is shown, and a joined Mac's name is
read then from the sealed copy, so `sync_notice` stores no name (A3, #91,
migration 0046).

**Retention**
- A `vault_state` row's session columns are cleared on Log Out, on Turn Off
  Sync, on `403 device_revoked` and when another account signs in. Its anchor
  columns stay.
- The whole row is deleted only on `account_deleted`, on Delete Synced Data,
  and on verifying a signed tombstone. It is replaced when this Mac adopts a
  new vault after a reset it verified (§6.1).
- `sync_known_device` is cleared with the session.
- `sync_notice` is cleared at Log Out, an account switch and account deletion;
  an acknowledged row goes after 30 days.
- `vault_rule_link` rows follow the local rule: they become tombstones, and are
  deleted once the tombstone is pushed. They are also cleared on Log Out, and
  whenever this Mac opens a document whose `person_salt` differs from the one
  its links were made under (a new vault after a reset). Rules then re-mirror
  at their next ladder match (§5.4).
- No hostname, app name, bundle id or unsalted digest is written to any of
  these tables. `person_key` is an HMAC under the in-document `person_salt`,
  which exists on disk only inside the ciphertext.

### 4.4 Phase 4 additions (data model)

**velvt-core** (`…_trust`, `…_relay`):

| Table | Columns (all carry `user_id` FK and an index) |
|---|---|
| `device_chain_entries` | `seq`, `prev_hash`, `entry_hash`, `kind`, `signed_entry` bytea, `created_at`; UNIQUE(user_id, seq) |
| `device_key_bindings` | `device_id` FK devices, `member_id`, `bound_at`, `proof` bytea; UNIQUE(device_id) |
| `pairing_sessions` | `id`, `kind` (qr, codes), `initiator_device_id`, `enrollment_token_hash`, `state`, `messages` jsonb (opaque blobs ≤ 2 KB each), `claimed_device_id`, `tokens_issued_at`, `expires_at`, `attempts` |
| `vault_key_wraps` | `epoch`, `recipient_member_id`, `issuer_member_id`, `envelope` bytea; UNIQUE(user_id, epoch, recipient_member_id) |
| `relay_messages` | `recipient_device_id` FK, `envelope` bytea (exactly 560 B), `collapse_id`, `expires_at`, `delivered_at` |
| `devices` (new columns) | `platform` (macos, ios, android), `push_provider` (none, apns, fcm). The existing `apns_*` columns hold the token |

`vaults` gains `vault_id`, `epoch` and `head_seq`. `vault_blobs` gains
`epoch`.

**Helper SQLite:** `trust_state` (single row: `root_hash`, `head_seq`,
`head_hash`, `my_member_id`, `epoch`, `halted_reason`), `trust_chain_entry`
(public data only), `trust_member`, `trust_alert`, and later `relay_outbox`
and `relay_inbox_cursor`.

---

## 5. Cryptography

### 5.1 Primitives, crates and APIs

**v1 uses one Rust crate for the cryptography: `ring`.** It is already in
`Cargo.lock` (0.17.14, through rustls) and becomes a direct dependency, with
no new crate in the lockfile. So do `base64`, `zeroize` and `subtle`, which
are already compiled into today's build. AGENTS.md asks for the founder's
approval of each new direct dependency (§17 Q4).

| Need | Choice | API |
|---|---|---|
| AEAD | ChaCha20-Poly1305 (RFC 8439), random 96-bit nonce | `ring::aead::{LessSafeKey, UnboundKey, CHACHA20_POLY1305}` |
| KDF | HKDF-SHA256 (RFC 5869) | `ring::hkdf` |
| Write signatures | Ed25519 (RFC 8032) from a 32-byte seed | `ring::signature::Ed25519KeyPair::from_seed_unchecked`; verify with `UnparsedPublicKey` |
| MAC | HMAC-SHA256 | `ring::hmac` for new code. `abstraction/key.rs` keeps its own HMAC and is unchanged |
| RNG | OS CSPRNG | `ring::rand::SystemRandom` |
| Secret hygiene | zeroize on drop; constant-time comparison | `zeroize` (1.9.0, already compiled in through `rustls`). Tags are checked with `ring::hmac::verify`; any other secret comparison uses `subtle::ConstantTimeEq` (2.6.1, already compiled in through `rustls`). Not `ring::constant_time`: it is deprecated in 0.17.14, and `cargo clippy -D warnings` would fail on it |
| Encoding | base64 | `base64` (in the lockfile, 0.22.1) |
| Server verify | Ed25519 | Python `cryptography` (already a velvt-core dependency, `>=43,<46`): `Ed25519PublicKey.from_public_bytes(pk).verify(sig, msg)` |
| Swift | Keychain only | Security.framework. No CryptoKit and no new package |

The nonce bound: at a few hundred writes a year per key, random 96-bit nonces
are far below the collision bound.

**Phase 4 adds exactly what `ring` lacks.** `ring` has no static X25519 private
keys, and an HPKE recipient needs one.

| Need | Choice | Notes |
|---|---|---|
| Static X25519 | `x25519-dalek` 2.x (`static_secrets`) on `curve25519-dalek` ≥ 4.1.3 | 4.1.3 fixes RUSTSEC-2024-0344 |
| HPKE base mode, RFC 9180, suite DHKEM(X25519, HKDF-SHA256) / HKDF-SHA256 / ChaCha20-Poly1305 | `hpke` 0.13 (exact pin), or `hpke-rs` with the libcrux provider if the reviewer prefers formally verified primitives | Checked against RFC 9180 Appendix A vectors, and in differential tests against CryptoKit HPKE (macOS 14+, iOS 17+, same suite) |
| Everything else | `ring`, as in v1 | Signatures, AEAD, HKDF and HMAC stay on one reviewed implementation |
| iOS and Android bindings | `velvt-trust` through UniFFI (Mozilla) | One protocol implementation everywhere. Founder approval with the Phase 4 crates |

**Rejected**
- **CryptoKit as the Mac's protocol engine.** It moves trust judgements into
  Swift and makes Swift a decryption oracle on the socket.
- **libsodium.** A C dependency.
- **A PAKE crate.** There is no audited Rust PAKE that fits. The v1 code
  carries 100 bits and travels eye to keyboard, so no PAKE is needed. The v2
  Mac-to-Mac codes use commit-then-reveal with the primitives above.
- **A vault key derived from the password.** The server holds the password
  hash and could attack weak passwords offline, and a password reset would
  destroy the vault.
- **Server escrow of any key.**

### 5.2 Key hierarchy

```
v1 (Phase 3)
K  (32 B random, Rust; stored by Swift in the synchronizable Keychain item)
├── K_doc   = HKDF(K, info="velvt-vault-doc-v1",   32 B)       → seals the prefs document
├── W_seed  = HKDF(K, info="velvt-vault-write-v1", 32 B)       → Ed25519 write key (W_sk, write_pk)
│        key_id      = hex(SHA-256("velvt-vault-key-id-v1\0" ‖ write_pk)[0..8])
│        fingerprint = u64_be(SHA-256("velvt-vault-fpr-v1\0" ‖ write_pk)[0..8]) mod 10^12, as 12 digits
└── person_salt (32 B random, stored inside the document, not derived from K)
         person_key(kind, digest) = HMAC-SHA256(person_salt, "velvt:person-key:v1" ‖ kind ‖ digest)

setup code s (100 bits)  → HKDF(s, info="velvt-vault-handoff-v1", 48 B) = handoff_id (16 B) ‖ K_wrap (32 B)
host binding             = HMAC-SHA256(host_binding_salt, host UUID)        (never leaves the Mac)

v2 (Phase 4) adds
device seed D (32 B, per device and account; Swift Keychain, ThisDeviceOnly, Secure Enclave-sealed where possible)
├── sign_sk = Ed25519 from HKDF(D, "velvt-device-v1\0sign")
└── kem_sk  = X25519  from HKDF(D, "velvt-device-v1\0kem")
member_id = SHA-256("velvt-member-v1\0" ‖ sign_pk ‖ kem_pk)[0..16]
VK_e (32 B random per epoch e) → HPKE-wrapped to each member, issuer-signed
├── K_doc,e   = HKDF(VK_e, salt=vault_id, info="velvt-vault-doc-v2")
└── W_seed,e  = HKDF(VK_e, salt=vault_id, info="velvt-vault-write-v2")   (the server-checked write key, per epoch)
R (32 B, optional recovery secret: in iCloud Keychain and/or a printed key) → the keys of a chain member of kind `recovery`
```

**Domain separation.** Every derivation and signature has its own tag that
starts with `velvt-`. A signature made for one purpose never verifies as
another, which mirrors the egress ledger's `velvt-egress-v1|…` pattern.

### 5.3 Formats (v1)

**Encoding rules**
- Every signed or authenticated structure is a fixed binary layout: a
  NUL-terminated tag, then big-endian integers, then fixed-length arrays.
- Every variable-length field is `lp(x) = u16_be(len(x)) ‖ x`.
- Inside signed and authenticated bytes, `key_id` is its 8 raw bytes. In JSON
  and in the database it is 16 lowercase hex characters.
- `kind` in a person key is one byte: 1 site, 2 bundle, 3 app name.
- Unknown versions and trailing bytes are rejected.
- As built in A1/A3 and C2, and pinned by both sides' vectors:
  - `user_id` is the account id's UTF-8 text (its 36-character UUID), as
    `lp(user_id)`;
  - times inside statements (the delete statement's `requested_at`, the
    handoff's `expires_at`) are whole Unix seconds;
  - a reset request id is its UUID's 16 raw bytes;
  - a signed statement stored and handed back is the statement followed by
    its signature: a rotation is `rotate_stmt ‖ old_sig ‖ new_sig`, a
    tombstone `delete_stmt ‖ sig`;
  - every HKDF here uses an empty salt (RFC 5869 §2.2), and `s` enters it as
    13 big-endian bytes, top four bits zero.

  The byte-for-byte reference is velvt-app
  `docs/architecture/vault-v1-formats.md` (with
  `vault-v1-test-vectors.json`), and for the server's checks velvt-core
  `docs/api.md` "Personal Vault" (§8.3).

**Sealing the document at version `v`** (`v = max(n, highest_seen_version) + 1`, where `n` is the server's current version; §10.4)

```
plain      = u32_be(len(json)) ‖ json ‖ 0x00…      (padded to the next power of two, at least 16 KiB:
                                                    16, 32, 64, 128, 256 or 512 KiB)
nonce      = 12 random bytes
aad        = "velvt-vault-doc-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id(8 B) ‖ u16_be(schema) ‖ u64_be(v)
ciphertext = nonce ‖ ChaCha20-Poly1305(K_doc, nonce, aad, plain)
```

The six sizes measure the padded plaintext. The ciphertext is 28 bytes longer
(the nonce and the tag): one of six lengths from 16,412 to 524,316 bytes.
velvt-core stores exactly these six, answers any other length with
`422 vault_blob_invalid`, and one past the largest with `413 vault_too_large`.

**Write signature** (checked by velvt-core on every create and PUT, and by
clients on every fetch):

```
put_stmt = "velvt-vault-put-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id ‖ u16_be(schema)
           ‖ u64_be(base_version) ‖ u64_be(new_version) ‖ SHA-256(ciphertext)
sig      = Ed25519(W_sk, put_stmt)
```

**What each binding stops**
- Another account's blob fails to open, because `user_id` is in the AAD and in
  the signature.
- An old blob relabelled with a new version fails, because the version is in
  the AAD.
- An old blob served at its old version never replaces the cached document,
  because its version is below `highest_seen_version`. It may be merged, which
  is harmless: every value is a register with an HLC, so an older document
  cannot undo a newer register this Mac has seen (§6.1).
- A different blob at the same version is a fork, caught by
  `highest_seen_sha256`.
- Power-of-two padding shows the server only which of six size steps the
  copy is in. Fixed 4 KiB steps would have shown the rule count to within
  about 37 rules, and every few dozen labels as a size change.
- A write from someone without `K` is refused by the server, because the
  signature fails against the stored `write_pk`.

**Signed statements** (each is stored by the server and handed back to clients,
which verify it with the `write_pk` they already trust):

| Statement | Bytes signed | Signed by |
|---|---|---|
| create | `"velvt-vault-create-v1\0" ‖ lp(user_id) ‖ key_id ‖ write_pk ‖ SHA-256(ciphertext)` | the new `W`, as proof of possession |
| rotate | `"velvt-vault-rotate-v1\0" ‖ lp(user_id) ‖ old_key_id ‖ new_key_id ‖ new_write_pk ‖ u64_be(new_version)` | the **old** `W` (authorization) **and** the new `W` (possession) |
| delete | `"velvt-vault-delete-v1\0" ‖ lp(user_id) ‖ key_id ‖ u64_be(at_version) ‖ i64_be(requested_at)` | `W` |
| handoff | `"velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id ‖ SHA-256(payload) ‖ i64_be(expires_at)` | `W` |
| cancel reset | `"velvt-vault-reset-cancel-v1\0" ‖ lp(user_id) ‖ key_id ‖ reset_request_id` | `W` |

No statement names the Mac that signed it, so J6's `sync_key_changed` and J9's
"turned off" line name no Mac. C2 rebuilds the rotate statement and parses the
delete statement at exactly these lengths. A1 (#89) as first built appended a
16-byte issuer tag to the rotate and delete statements, which C2 refuses; A3
(#91, commit `413cd0b`) removed it again. Whether to add it on both sides is
§17.1 D13.

**Setup code (handoff)**
- `s` is 100 random bits. It is written as 20 Crockford base32 characters plus
  one Luhn mod 32 check character, shown in groups:
  `K7PQ-M2XD-9HTW-4RZN-8CJ5H`. The check character catches every
  single-character error and every adjacent swap except `0`↔`Z`, before any
  network call.
- Normalization: upper-case; map `O→0`, `I/L→1`; reject `U` (not in the alphabet); ignore separators
  and spaces.
- `(handoff_id ‖ K_wrap) = HKDF-SHA256(ikm=s, salt=∅, info="velvt-vault-handoff-v1", L=48)`.
- `payload = nonce ‖ ChaCha20-Poly1305(K_wrap, nonce, aad = "velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id, plain = 0x01 ‖ key_id ‖ K ‖ issuer_sync_device_id(16 B))`.
- The receiver derives `write_pk` and `key_id` from the `K` it opened and
  requires that `key_id` to equal the one in the payload.
- The server releases the payload once, within 10 minutes.

No PAKE is needed: the 100-bit secret travels eye to keyboard and never passes
through the server, so neither offline search (2^100) nor a man in the middle
is possible.

### 5.4 Person keys for category rules

**Why a second key is needed**
- Install keys are `HMAC(stable_key_salt, SALTED_KEY_DOMAIN ‖ unsalted digest)`
  (`abstraction/key.rs`). They differ on every Mac by design.
- After a site rule is saved, `save_site_scope_override` deletes the
  `local_site_name` row in the same transaction.
- The raw app name and bundle id are never stored.

**The person key**
- `person_key(kind, d) = HMAC-SHA256(person_salt, "velvt:person-key:v1" ‖ kind ‖ d)`.
- `d` is the unsalted `site_digest`, `bundle_digest` or `app_digest` from
  `key.rs`. Those functions become `pub(crate)` and are otherwise unchanged.
- The salt lives in the document, not derived from `K`, so a key rotation does
  not re-key every rule.

**When a person key is computed.** The raw value is in hand at these points,
and only these:

| When | Kinds | Where |
|---|---|---|
| **Labeling time**, from the needs-a-category list | site | `set_site_category` → before `save_site_scope_override` runs, read `local_site_name.host` for that `site_key_hash`, compute the person key, then save the rule (which deletes the host) |
| Labeling time | app by name | Only if the triage entry holds the same app-name string that `app_digest` hashes. PR A6 checks this against the triage query, and falls back to the next match if not |
| **Next ladder match** (backfill, and every rule made before sync was on) | site, bundle, app | `abstraction/engine.rs` ~243–260, where `personal_site_override`, `personal_app_override(bundle)` and `personal_app_override(app name)` are consulted with the raw site, bundle id and app name in hand. If sync is unlocked and `(kind, install key)` is missing from the in-memory link set, or its category differs, compute the person key, upsert `vault_rule_link` and mark the document dirty |
| Rule edit, remove, reset | all | `update_classification_override`, `remove_classification_override`, `reset_classification_overrides` and the site-rule editors update or tombstone the link directly. The link already knows the person key |

The cost while sync is off is one boolean check per classification.

### 5.5 Keychain storage of `K` (Swift `SyncKeyStore`)

| Attribute | Value |
|---|---|
| Class | `kSecClassGenericPassword` |
| `kSecAttrService` | `com.velvt.sync` |
| `kSecAttrAccount` | `<user_id>.<key_id>`. **Never `user_id` alone.** A raced or forged 404 then cannot overwrite or delete the real key on every Mac |
| `kSecUseDataProtectionKeychain` | `true` |
| `kSecAttrSynchronizable` | `true`. Queries pass `kSecAttrSynchronizableAny` |
| `kSecAttrAccessGroup` | `$(TeamIdentifierPrefix)com.velvt.shared`, shared later with the iOS app and its NSE |
| `kSecAttrAccessible` | `kSecAttrAccessibleAfterFirstUnlock`. A synchronizable item cannot be `ThisDeviceOnly` |
| `kSecAttrLabel` | `Velvt sync key` |
| Value | JSON `{"v":1,"key_id":…,"key_b64":…,"created_at":…,"supersedes":<key_id or null>,"retired_by":<key_id, "v2" or null>}` |

**Rules**
- **Never create while a live item exists.** Before creating a vault, Rust asks
  Swift for every key item it holds for this `user_id`. If any item is not
  retired, Rust tries to adopt it and never creates a new vault. The one
  exception is an item whose `key_id` a completed reset names, and only when
  §6.1 verified that reset or the person started it on this Mac.
- **Keychain first, server second.** Rust sends `store_vault_key`, waits for
  `vault_key_stored{ok}`, and only then creates the vault on the server. A blob
  never exists whose key was not saved.
- **Rotation is recorded in the Keychain, not only on the server** (§5.6). The
  new item carries `supersedes: <old key_id>` in the same value as the new key,
  so the two arrive on another Mac together. The old item is rewritten with
  `retired_by: <new key_id>`.
  - Rust never adopts a retired item.
  - Rust never adopts a key that another item it holds supersedes. If the
    server serves such a key, that is a rollback: `server_inconsistent`.
  - Without these two rules, a malicious server could serve the pre-rotation
    copy to a fresh Mac on the same Apple ID for the 30 days the old item
    lives. That Mac would then write its choices under the key the lost Mac
    still holds.
- **Never delete on Log Out.** Deleting a synchronizable item deletes it from
  every Mac on the Apple ID. An item is deleted only:
  - on Delete Account (every item for the account);
  - on Delete Synced Data (every item for the account, including retired
    ones, so a later Turn On can create);
  - on verifying a signed tombstone (the deleted `key_id` only; this is how a
    Mac on another Apple ID drops its copy of the key);
  - when an orphan item lost a create race (its own `key_id` only);
  - 30 days after a verified rotation (the old `key_id` only).
- **No server claim alone deletes or rewrites an item.** A completed reset,
  a 404 or an unreadable copy never touches the Keychain. Deleting a
  synchronizable item on a forged signal would remove the key from every Mac.

**Entitlements**
- Apple TN3137: a data-protection Keychain access group comes from an
  entitlement that a provisioning profile must authorize.
- The release build today has an empty `Release.entitlements` (`<dict/>`) and
  no embedded profile.
- v1 needs `keychain-access-groups` (Keychain Sharing) in a Developer ID
  provisioning profile. That is the same profile #84's route (a) needs for
  Communication Notifications, so both can be done in one founder action.
- PR S0 proves this on two Macs before anything depends on it.
  `verify_release.sh` gains a check that the entitlement and profile are
  present.
- The existing file-keychain token items (`com.velvt.mac`) are out of scope and
  unchanged.

### 5.6 Rotation

**v1 rotation** (PR A8; offered on "lost or stolen" removal)
1. Pull and open the latest document.
2. Make `K'` and derive `W'`, `key_id'`.
3. Store `K'` in the Keychain under `<user_id>.<key_id'>` with
   `supersedes: <key_id>`, and wait for the acknowledgement.
4. Seal the document under `K'` at the next version `v` (§5.3).
5. Send `POST /v1/vault/rotate` with the rotate statement, signed by the old
   `W` and by `W'`. velvt-core verifies both, swaps `key_id` and `write_pk`, and
   stores the blob atomically.
6. On success, rewrite the old item with `retired_by: <key_id'>` (§5.5). Every
   Mac that verifies a rotation away from a key does the same (it is
   idempotent), so a crash between steps 5 and 6 heals on the next fetch. If
   the server refuses the rotation for good (409, because the key changed
   underneath), the rotating Mac deletes its own `K'` item as an orphan. A Mac
   that saw that orphan re-evaluates on its next fetch.
7. Other Macs see a new `key_id`. They verify the rotation statement against
   the old `write_pk` they trust, then look for `K'` in the Keychain:
   - found: adopt silently, post `sync_key_changed`, and delete the old item
     after 30 days;
   - not found (another Apple ID): state `waiting_for_rotated_key`, and offer
     code entry. Offering a code is safe here because the rotation is
     verifiably by a holder of the old key.

**v1 limit, stated in the UI.** A Mac still in the person's iCloud Keychain
circle receives `K'` too. Rotation protects against Macs paired by code on
other Apple IDs, and against a Mac the person has already removed from their
Apple ID.

**v2 rotation** happens on every REVOKE and on the ROTATE that follows a
LEAVE:
1. Make `VK_{e+1}` and record its commitment in the chain entry.
2. Wrap it to every remaining member, including the recovery member.
3. Re-seal the blob under the new epoch.
4. Post one `POST /v1/trust/rotate`. velvt-core applies it in a single
   transaction: append the entry, store the wraps, replace the blob, delete
   epoch-e wraps, and revoke the target's bound device rows and tokens.

### 5.7 Revocation

| Layer | v1 | v2 |
|---|---|---|
| Account access | `DELETE /v1/devices/{id}` with a recent re-authentication. Uses `device_service.revoke_device(actor="user", reason="removed_from_other_device")`, which moves #41's token generation on and revokes refresh tokens | Same, routed through `/v1/trust/rotate` when the target is a chain member |
| Vault access | The server stops serving the blob to a revoked device. `removed: true` in the document. A rotation is optional and limited by iCloud (§5.6) | The vault key rotates. Everything written afterwards is under a key the removed device never held |
| Phone pushes | n/a | The push token is cleared, and no relay message is sealed to a removed member |
| What the removed device sees | `403 device_revoked` → sync state cleared, the `this_mac_removed` notice | Its own REVOKE in the chain → wipe, discard the seed, alert |

### 5.8 Recovery options, and the recommended default

| Option | v1 | v2 | Recommendation |
|---|---|---|---|
| **R1. iCloud Keychain.** A new Mac on the same Apple ID finds `K` | Yes, zero-touch | `R` in iCloud Keychain gives a `recovery` chain member. Joining through it alerts every device | **Default on** |
| **R2. A setup code** from any Mac that has the key | Yes | Replaced by the two typed codes, or QR | **Default** |
| **R3. Start a new copy.** No device and no key: a new vault is built from this Mac's own local rules. Other Macs re-mirror theirs when they rejoin (§5.4) | Yes, with a 72 h public waiting period when unsigned | Same | **Default** (the last resort) |
| **R4. A printed recovery key.** 128 bits plus a checksum, shown once, an offline chain member | No | Yes, optional | **Offer in v2**, not required |
| Server escrow, or a password-derived key | Never | Never | Never |

**What can be lost.** Rules that lived only on a Mac that is gone, when no
other Mac and no iCloud Keychain holds `K`. Every Mac's local tables stay
authoritative for that Mac, so losing the vault never loses a Mac's own setup.

### 5.9 Phase 4 formats (summary; the full chain is in §6.2)

- **Key wrap.** `WrapTBS = "velvt-wrap-v1\0" ‖ account_binding ‖ vault_id ‖ u32 epoch ‖ recipient(16) ‖ issuer(16) ‖ u32 chain_seq ‖ enc ‖ ct`.
  - The wrap uses HPKE base mode with `info = "velvt-hpke-v1\0vault-key-wrap"`.
    Its AAD is the TBS without `enc` and `ct`. The plaintext is `VK_e`.
  - The issuer signs `WrapTBS`.
  - A recipient accepts a wrap only if all of these hold: the issuer was active
    at `chain_seq`, the epoch matches, the recipient is itself, and
    `SHA-256("velvt-vk-commit-v1\0" ‖ vault_id ‖ u32 epoch ‖ VK_e)` equals the
    chain's `vk_commitment` (the same tagged formula as §6.2, never a bare
    hash of `VK_e`).
  - It also requires the `W_seed,e` public key derived from `VK_e` to equal
    the chain's `epoch_write_pk` for that epoch.
- **The document in v2** keeps the one-blob model. It is sealed under
  `K_doc,e`, and the AAD adds `vault_id` and `epoch`.
  - A reader takes as current only a document at the verified chain's current
    epoch. An older-epoch blob may have been written by a member removed since,
    so it is never taken as current; it may only be merged, as in v1.
  - The plaintext adds `writer` (member id), `chain_seq` and `entry_hash`
    (witnessing).
  - It also carries an Ed25519 signature by the writer's member key, inside the
    AEAD. One compromised member therefore cannot forge another member's
    writes.
  - The server keeps checking the per-epoch write key.
- **Relay envelope.**
  - Header: `v`, sender member, recipient member, `msg_id` (16 B random),
    `expires_at`, `chain_seq`, `entry_hash`.
  - The body is sealed with HPKE (`info = "velvt-hpke-v1\0relay"`,
    `aad = header`) and signed by the sender over the header, `enc` and `ct`.
  - The plaintext is padded to 512 B and holds
    `{kind, ref, title, body, actions[]}`, built from the copy Rust already
    renders.
  - The relay never uses a key derived from the shared vault key.
  - **Replay.** A recipient drops an envelope past `expires_at` and any
    `msg_id` it has already seen. It keeps seen ids until their `expires_at`
    (at most 12 h), in the NSE's shared container on iOS. Otherwise the server
    could replay a sealed, signed message inside its TTL and show it twice.
- **Account binding.** `account_binding = SHA-256("velvt-account-v1\0" ‖ user_id)`
  is in every v2 signature and AAD.

---

## 6. Verification

### 6.1 What a v1 Mac verifies on every fetch (there is no chain in v1)

```
trusted = this account's vault_state anchor (kept across Log Out and removal, §4.3),
          or, with no anchor yet, every key the Keychain holds live (each item's key_id and
          the write_pk derived from its K); `waiting_for_key` and code entry only when it holds none
now, fetched_at = this Mac's own clock; every comparison below allows a 10-minute margin

on GET /v1/vault (200 or 404; both bodies carry pending_reset?, completed_reset? and tombstone?)
and GET /v1/vault/blobs/prefs:
  if pending_reset is present:
      if pending_reset.id == seen_reset_request_id: as before (already recorded and posted); offer [Cancel Reset]
      elif pending_reset.effective_at − pending_reset.requested_at ≥ 72 h
           and (fetched_at is unset or fetched_at < pending_reset.requested_at + 10 min):
          record seen_reset_request_id = pending_reset.id; post `reset_requested` once; offer [Cancel Reset]
      else → `server_inconsistent` (this Mac fetched after the request and was not shown it, or the
             window is short): do not record it; no `reset_requested` notice
  elif seen_reset_request_id is set and no completed_reset carries that id:
      clear seen_reset_request_id   (it was cancelled; a later "completed" claim for it must pass the fetched_at test)
  if completed_reset is present and its key_id == trusted key_id:
      check_reset(completed_reset)                                       (below; also on a 404)
  if 404:
      if a tombstone is returned and its delete statement verifies under trusted write_pk:
          → state off ("Sync was turned off from another of your Macs."), delete this account's vault_state row,
            delete this Mac's own Keychain items for that key_id (§5.5; a no-op on the deleting Mac's Apple ID)
      elif check_reset accepted a completed reset → `reset_completed`; [Set Up Sync Again] may create (§5.5)
      elif we hold a live key for this account or highest_seen_version > 0:
          → state `missing_on_server` (neutral copy); never auto-create; [Start a New Synced Copy] needs an explicit click
      else → state `off` / `available_elsewhere = false`
  if a held Keychain item supersedes the served key_id, or the served key_id's item is retired:
      → `server_inconsistent` (a rollback past a rotation, §5.5)
  if key_id == trusted key_id:
      require write_pk == trusted write_pk                                (fail → `server_inconsistent`)
      verify the write signature over put_stmt with trusted write_pk      (fail → `server_inconsistent`)
      if version == highest_seen_version and SHA-256(ciphertext) != highest_seen_sha256
          → `server_inconsistent` (two different blobs at one version: a fork)
      open with K_doc and the AAD; validate the schema                    (fail → `server_copy_unreadable`, never overwrite)
      if version < highest_seen_version:
          merge it into ours without adding any device or rule ours lacks (merge_older_copy): after
          180-day tombstone collection an old copy would revive removed Macs and deleted rules;
          never cache it as current; the next write goes at highest_seen_version + 1 (§10.4)
      else: cache it; highest_seen_version, highest_seen_sha256 = version, SHA-256(ciphertext)
  if key_id != trusted key_id:
      if a tombstone (C2 returns it on a 200 too, once a new vault follows a deletion) verifies under
         a trusted write_pk → the trusted vault was deleted: adopt the new vault only through the J3 routes
      elif the rotation statements form a verified path from our trusted key_id to the served key_id
         (each signed by the key before it and by the key it introduces) → adopt per §5.6
      elif check_reset accepted a completed reset for the trusted key_id
          → adopt the new vault only through the J3 routes; replace the anchor once a key arrives
      else → `server_inconsistent` notice; sync paused; **no code entry offered**

check_reset(r):   accepted only if all hold
      r.effective_at − r.requested_at ≥ 72 h   (a client-side constant)
      r.effective_at ≤ now
      and either seen_reset_request_id == r.id                 → `reset_completed` notice
          or fetched_at < r.requested_at                         → `reset_completed`, "while this Mac couldn't reach Velvt"
      otherwise → `server_inconsistent`   (this Mac fetched during the window and never saw it pending)
      An accepted reset never deletes or rewrites a Keychain item (§5.5).
```

A server that wants a polling Mac to accept a fake reset would have to claim it
was requested after that Mac's last fetch. The 72-hour minimum then puts
`effective_at` in the future, and the client refuses it. The first sight of a
pending reset faces the same test as a completed one this Mac never saw, so a
server cannot backdate `requested_at`, show the reset once and complete it
minutes later (A3, #91). Only a Mac that made
no successful fetch for at least 72 hours can be shown a reset it never saw
pending (T5). That includes a Mac the server itself refused to serve for 72
hours: withholding is outside the threat model (§1.2), so this case ends in
the J3 routes, where only a code typed from the person's own screen helps.

Two earlier gaps are closed here. The 404 body now carries the reset fields,
so a Mac that comes back after a completed reset, before anyone has made a new
copy, sees `reset_completed` and not `missing_on_server`. And because the
anchor survives Log Out, a Mac that signed out and back in is checked against
what it last trusted, not against nothing.

**Why this closes the "garbage blob" and "new key" attacks**
- Someone holding only the password cannot produce a valid write signature, so
  velvt-core refuses the PUT.
- A reset without the key is public for 72 hours, so every Mac that polls
  during the window alerts and can cancel.
- Only a malicious *server* can still serve a fake new key. A Mac that was
  online through the claimed window, and never saw it, treats that as the
  server lying. It pauses, and never invites the person to type a code an
  attacker might supply.
- In every unreadable case, Rust never steers the person to Delete Synced Data
  or Enter a Code. The status line says nothing on this Mac changed, and offers
  **Replace the Server Copy with This Mac's** after 7 days. That is a signed
  repair: it rebuilds from local tables, the last good document and the cached
  `person_salt`, and writes at `highest_seen_version + 1`.

**Residual risk.** Fork and withholding go undetected. The server can show two
Macs different valid versions, or stop serving updates. This is accepted for
preferences. Merges converge once it stops, and v2 adds witnessing.

### 6.2 Phase 4: the device chain

**Entry format (v1 of the chain)**

```
TBS:
  "velvt-devchain-v1\0"
  u8   format = 1
  u8   kind            0 GENESIS | 1 ADD | 2 REVOKE | 3 LEAVE | 4 ROTATE | 5 KEY_UPDATE | 6 TERMINATE
  u32  seq
  [32] prev_hash       zeros for GENESIS
  [32] account_binding
  [16] vault_id
  u32  epoch           the epoch in force after this entry
  [32] vk_commitment   SHA-256("velvt-vk-commit-v1\0" ‖ vault_id ‖ u32 epoch ‖ VK_epoch)
  [32] epoch_write_pk  the Ed25519 public key of W_seed,epoch (§5.2); changes iff epoch changes.
                       velvt-core reads the per-epoch write key it enforces from here, so the key it
                       checks is one a member signed, not one a request merely asserted
  u64  created_at_ms   informational only
  u16  body_len, body
    GENESIS, ADD : u8 member_kind (1 mac, 2 iphone, 3 android, 4 recovery) ‖ [32] sign_pk ‖ [32] kem_pk
                   ADD also ‖ [16] adder_member_id ‖ u8 via (1 codes, 2 qr, 3 recovery, 4 icloud_recovery)
    REVOKE       : [16] target ‖ u8 reason (1 no_longer_used, 2 lost_or_stolen, 3 server_removed, 4 key_unavailable)
    LEAVE        : [16] member
    ROTATE       : [16] follows (the member whose LEAVE prompted it) or zeros
    KEY_UPDATE   : [16] member ‖ [32] new_kem_pk
    TERMINATE    : (empty)
SignedEntry = TBS ‖ u8 n ‖ n × ([16] signer_member_id ‖ [64] Ed25519 signature over TBS)
entry_hash  = SHA-256(TBS)        (signatures excluded, so co-signers sign a fixed hash)
root_hash   = entry_hash(seq 0)   (the chain's identity; every device pins it)
```

**Signature rules** (every client checks them, and velvt-core checks them
again as defence in depth)

| Kind | Required signers | Epoch |
|---|---|---|
| GENESIS | the genesis member | 1 |
| ADD | an active adder **and** the added member (proof of possession) | unchanged; the adder posts a wrap of the current VK for the new member |
| REVOKE | one active member other than the target | +1 |
| LEAVE | the leaving member | unchanged; the next active member to come online appends ROTATE |
| ROTATE | one active member | +1 |
| KEY_UPDATE | the member, with its current signing key | unchanged |
| TERMINATE | one active member | final |

**Verification algorithm** (in `velvt-trust`; pure, with no I/O)

```
verify(entries, pinned_root, stored_head, user_id) -> Result<ChainState, ChainError>:
  require len(entries) ≤ 4096
  for i, e in entries:
    parse strictly: tag, format, known kind, lengths ≤ caps, body ≤ 256 B, no trailing bytes
    require e.seq == i
    require e.prev_hash == (i == 0 ? zeros : entry_hash(entries[i-1]))
    require e.account_binding == SHA-256("velvt-account-v1\0" ‖ user_id)
    require e.vault_id == entries[0].vault_id
    require (i == 0) == (e.kind == GENESIS)
    signers = the required set for e.kind (table above), from members active at seq i-1
    every signature: ed25519 verify over TBS, with no duplicate signers
    epoch rule: +1 on REVOKE and ROTATE, unchanged otherwise; vk_commitment and epoch_write_pk change iff epoch changes
    member rules: revoked or left members never sign again and are never re-added;
                  ≤ 20 active members; ≤ 1 recovery member of each source (printed, icloud)
    fold e into ChainState
  require entry_hash(entries[0]) == pinned_root
  require entries[stored_head.seq] exists and entry_hash(it) == stored_head.hash   (monotonic head)
  return ChainState { head, members, epoch, vk_commitment }
```

**Pinning, rollback and forks**

- **Pinning.** A device pins `root_hash` when it joins. It receives it through an
  authenticated channel: the QR, the typed codes, or a recovery member's
  co-signature, which commits to `prev_hash` and so to everything before it.
  The server's word is never enough.
- **Monotonic head.** A device stores `(head_seq, head_hash)` and accepts only
  chains that contain that exact entry. A shorter or different chain halts sync
  with a `chain_conflict` alert.
- **Witnessing.**
  - Every document write and relay message carries the writer's
    `(chain_seq, entry_hash)`.
  - A reader that has that seq must have that hash. Otherwise it raises a fork
    alert and halts.
  - A reader that does not have it fetches up to it. If the server cannot
    supply it within 24 h, the device list shows "Your other device reports
    device changes this Mac can't see."
- **Head attestations.** Each device writes `{head_seq, head_hash, epoch, at}`
  into its own register in the document on every sync. A member more than 24 h
  behind a REVOKE gets a "hasn't seen the latest device changes" badge.
- **Residual.** A server that partitions devices forever can hide a revocation
  from the partitioned devices. This is the limit of one server without key
  transparency, and it is stated in the threat model.
- **A reset in v2** (R3, no device and no recovery secret left) ends the old
  chain. A device that pinned the old root treats a new root exactly as §6.1
  treats a new `key_id`: it accepts it only after `check_reset` passes, and
  otherwise raises `chain_conflict`. It re-pins only through a pairing route
  (§7.6), never on the server's word.

**Failing closed without stranding testers**
- A verification error halts sync only. Classification, work blocks, nudges
  and history are untouched.
- The halt shows its reason code, **Try Again**, and **Turn Off Sync on This
  Mac**. The second clears the trust state, so the Mac can re-pair.
- A version the client does not know means "Update Velvt to keep syncing"; it
  is never parsed past.

---

## 7. Pairing protocols

### 7.1 v1: setup code, step by step

| # | Mac A (has `K`) | velvt-core | Mac B (signed in, no `K`) |
|---|---|---|---|
| 1 | Person clicks **Add a Mac with a code**. Rust draws `s` (100 bits) and derives `handoff_id` and `K_wrap` | | |
| 2 | Seals `payload` (§5.3) and signs the handoff statement with `W` | | |
| 3 | `POST /v1/vault/handoffs {handoff_id, payload_b64, expires_at, sig}` | Verifies `sig` against `write_pk`, so a password holder cannot replace the live handoff. Stores it with a 10-minute TTL. One live handoff per account: a new one replaces the old | |
| 4 | Shows the code and a countdown | | Person types it. Swift sends `redeem_pairing_code {code}` |
| 5 | | | Rust normalizes it and checks the check character (a typo fails locally), then derives `handoff_id` and `K_wrap` |
| 6 | | `POST /v1/vault/handoffs/{id}/claim` → returns the payload and deletes the row (single use). Another account's id gets 404 | |
| 7 | | | B accepts a typed key only in a state that offers code entry (`waiting_for_key`, `waiting_for_rotated_key`, `reset_completed`) or before sync is on; in any other state the code changes nothing. Opens the payload (AAD binds `user_id` and `handoff_id`) and checks that the `key_id` derived from `K` equals the payload's. Checks that it also matches `GET /v1/vault`. If it does not, B keeps the key the person typed (in memory: no Keychain item is written while the server disputes it), enters `server_inconsistent` and never adopts the served key: the code came from the person's own screen, and the server's answer did not. When it matches, B adopts it only if §6.1 would (the trusted key, nothing trusted yet, a rotation the trusted key signed, a verified reset or signed deletion). B then sends `store_vault_key` (synchronizable, so B's own iCloud circle gets it too) with, when the key is a rotation's, `supersedes` = the last rotation step's old `key_id`, marks the old item `retired_by`, and writes its device entry with `joined_via: code` |
| 8 | Next sync: sees B's new entry and posts `mac_joined_sync` | | |

### 7.2 Phase 4: adding a phone by QR, step by step

Roles: **M** is a trusted member Mac. **P** is a phone with the Velvt app
installed and nothing else; it has no account session yet.

1. **M creates a session.** The person clicks **Add an iPhone…**. Rust calls
   `POST /v1/trust/pairing-sessions {kind: "qr"}` with M's device token. It
   gets back `{session_id, enrollment_token (32 B, stored by the server only as
   a hash), expires_at = now + 10 min}`. Limit: 5 an hour per account.
2. **M builds the QR.** M draws a one-time secret `k` (32 B) and encodes:
   ```
   https://getvelvt.com/pair#1.<session_id>.<enrollment_token>.<k>.<root_hash>.<head_seq>.<head_hash>.<M member_id>
   ```
   - It is base64url, about 270 characters, which fits QR version 10–12.
   - The fragment is never sent to any web server.
   - A universal link delivers the full URL to the app, and opens the App Store
     if the app is missing. This needs an `apple-app-site-association` file on
     getvelvt.com (a velvt-web PR).
   - When the app is missing, Safari opens `/pair` with the fragment in
     `location.href`. That page is static HTML with **no script at all** (no
     analytics, no tag manager, no error reporter) and a
     `Content-Security-Policy: script-src 'none'`. Many analytics scripts send
     the full URL, fragment included, which would hand `k` and the enrollment
     token to a third party. A velvt-web test checks the header and the absence
     of `<script>`.
3. **P verifies the chain first.**
   - P parses the QR and calls `GET /v1/trust/chain` with
     `Authorization: Enrollment <token>`.
   - P verifies the chain from the pinned `root_hash` (§6.2). It checks that
     `(head_seq, head_hash)` is in it, and that M is an active member.
   - P now holds M's authentic `kem_pk`.
4. **P makes its keys and claims.**
   - P generates its device seed and derives its keys.
   - It sends `POST /v1/trust/pairing-sessions/{id}/claim`, under the
     enrollment token, with:
     - `platform: ios`, `sign_pk` and `kem_pk`;
     - `proof = Ed25519(P, "velvt-pair-claim-v1\0" ‖ session_id ‖ sign_pk ‖ kem_pk)`;
     - `sealed_name = HPKE(M.kem_pk, info="velvt-hpke-v1\0name", "Kevin's iPhone")`;
     - `mac = HMAC-SHA256(k, "velvt-qr-join-v1\0" ‖ session_id ‖ sign_pk ‖ kem_pk ‖ M member_id ‖ root_hash ‖ SHA-256(sealed_name))`.
       The name is inside the MAC because anyone can seal to `M.kem_pk`;
       without it the server could swap the name M shows in its sheet;
     - optionally its push token.
   - velvt-core creates a `pending` device row with **no tokens** and answers
     202.
   - **A session takes one claim.** A second claim marks the session
     `contested`, and M shows "Two devices scanned this code, so nothing was
     added." with no Add button. Someone who photographed the QR and claimed
     first therefore cannot sit behind a sheet the person expects to be their
     own phone.
5. **M checks the claim.** The sync poll brings the claim to M. Rust checks
   `mac` with `k` in constant time and checks `proof`. It opens
   `sealed_name`, then shows the sheet **Add “Kevin's iPhone”?** [Not Mine]
   [Add iPhone].
   - **Not Mine** declines: the pending row is revoked and the session closed.
6. **M builds the ADD.** On [Add iPhone], M builds `ADD(P)` on its head. The TBS
   travels to P through the session's message slots, sealed to P's `kem_pk`.
7. **P co-signs.** P checks that the entry adds exactly its keys, on the head it
   verified in step 3, and co-signs. The co-signature goes back the same way.
8. **M appends.** M signs and calls `POST /v1/trust/chain {expected_head_seq,
   signed_entry, wraps: [issuer-signed wrap of VK_e for P]}`. velvt-core
   validates and appends atomically. It then marks the device row active and
   binds `member_id`.
9. **P gets its tokens.** P polls
   `GET /v1/trust/pairing-sessions/{id}` under the enrollment token. It gets
   `approved` with its device tokens **once**: tokens are issued only after the
   chain entry lands.
   - The request that collects the tokens also carries
     `Ed25519(P.sign_sk, "velvt-pair-collect-v1\0" ‖ session_id ‖ server_nonce)`,
     checked against the claimed `sign_pk`. The enrollment token is in the QR,
     so anyone who photographed it could otherwise race P for the tokens. They
     could not decrypt anything, but device tokens read the account's history
     on the server.
10. **P finishes.** P fetches the chain and verifies that the head is its own
    ADD. It unwraps `VK_e`, opens the document (for routing), and registers its
    push token.
11. **Alerts.** Every other member posts "“Kevin's iPhone” was added to your
    Velvt devices by “Studio Mac”. Not you? Remove it in Velvt › Your devices."

**Why the server cannot insert itself**
- `k` never reaches the server.
- The phone authenticates M and the chain through the QR.
- M authenticates the phone through `mac`.
- The co-signature proves P holds the key being added.
- A photographed QR still needs a tap on M within 10 minutes. It works once,
  and tokens are issued only after approval.

### 7.3 Phase 4: Approve with iPhone (the reverse QR)

This is for a new Mac **N** when the old Mac is not nearby.

1. N signs in with its password and generates its member keys. It shows a QR
   with `session_id`, N's `sign_pk`, `kem_pk` and a fresh `k`.
2. The member phone **P** scans it. P now authenticates N's keys out of band.
3. P sends `root_hash`, `(head_seq, head_hash)` and its own member id to N,
   with `HMAC(k, "velvt-qr-approve-v1\0" ‖ transcript)`. N verifies the HMAC
   (the server does not know `k`), and pins the root.
4. P shows **Add “Mac mini”? It's the Mac showing this code.** [Add]. Steps 6–11
   of §7.2 follow, with P as the adder.

### 7.4 Phase 4: Mac to Mac with two typed codes

Roles: **A** is a member. **B** is the new Mac, signed in.

1. A creates a `codes` session and generates an ephemeral X25519 pair
   `(ea, EA)` and a nonce `nA`.
2. B joins and generates `(eb, EB)`, `nB` and its member keys. It posts
   `commit = SHA-256("velvt-pair-commit-v1\0" ‖ EB ‖ nB ‖ sign_pk_B ‖ kem_pk_B)`
   **before** it sees anything from A.
3. A reveals `EA`, `nA`, its member keys, `root_hash` and `(head_seq, head_hash)`.
4. B reveals its values. A checks them against `commit`.
5. Both compute `ss = X25519`, `T = SHA-256(transcript)` and
   `okm = HKDF(ss, salt=T, "velvt-pair-v1", 64)`. The transcript is every
   commit and reveal in order, with both sides' member keys, `root_hash` and
   `(head_seq, head_hash)` in it, so the codes also authenticate the chain
   head B will pin. From `okm` come a pairing AEAD key and two codes:
   - `code1` = u32(okm[0..4]) mod 10^6, **shown on A and typed on B**;
   - `code2` = u32(okm[4..8]) mod 10^6, **shown on B and typed on A**.
6. Each side compares the typed code in constant time.
   - A wrong code aborts the session on both sides and raises the alert "The
     codes didn't match, so nothing was added. If you didn't start this, change
     your password."
   - Limits: one attempt per session, and 3 sessions an hour per account.
7. B sends its signature over `T`, under the pairing key, as proof of
   possession, together with its name.
8. Steps 6–11 of §7.2 follow: A builds the ADD, B co-signs, A appends and
   wraps.

**Why this resists a malicious server**
- B commits before A reveals, so a man in the middle matches each code with
  probability 10^-6.
- A refuses to sign an ADD unless the person typed `code2`, which only the real
  B shows.
- B refuses a chain unless the person typed `code1`, which only the real A
  shows.
- Typing the *other* Mac's code, instead of clicking "they match", means a
  hurried person cannot skip the check.

### 7.5 Phase 4: joining through a recovery member

The recovery secret `R` is either a synchronizable iCloud Keychain item or a
printed key.

**`R` is always fresh, never v1's `K`.** Every Mac that ever synced in v1 holds
`K`, including a lost Mac paired by code on another Apple ID. If `K` became
`R`, that Mac could sign itself in as the recovery member, and v2's rotation on
removal would protect nothing against it. So the v1 → v2 migration (T3) draws
a new 32-byte `R`, stores it in a new item, re-seals everything under `VK_1`,
deletes the v1 blob, and marks the v1 item `retired_by: "v2"`. Each device
from v1 joins v2 through a route in §7.6. A same-Apple-ID Mac can use the new
`R`, which alerts every device.

1. The new Mac derives the recovery member keys:
   `HKDF(R, salt = "velvt-recovery-v1", info = vault_id)`.
2. It fetches the chain and finds the recovery ADD whose co-signature verifies
   under the derived key. That co-signature commits to `prev_hash`, which
   authenticates everything before it, so no earlier pin is needed.
3. It checks every later entry.
4. The recovery member signs `ADD(new Mac)` with `via = recovery` or
   `icloud_recovery`. The new Mac unwraps the key through the recovery
   member's wrap.
5. Every device posts "“Mac mini” joined using your iCloud Keychain" or "…using
   your recovery key". After a printed key is used, Velvt asks the person to
   replace it.

### 7.6 Why there is no compare-and-click path

| Situation | Allowed routes | Not allowed |
|---|---|---|
| Both devices in front of the person | v1 setup code; v2 typed codes or QR | "Do the codes match?" [Add] |
| Approving later, away from the new device | QR scanned by a member phone (§7.3), or the recovery secret | Approving a device on the strength of its self-chosen name and a sign-in time |
| A new device on the person's Apple ID | iCloud Keychain (v1: `K`; v2: the recovery member, with an alert to every device) | |

A notification can still say a device is **waiting**, and the device list
shows it as "Waiting for approval". Approving it always opens one of the
allowed routes.

---

## 8. Server API (velvt-core)

### 8.1 Conventions

- All routes are under `/v1`. They return the house error envelope and never
  log bodies.
- Pydantic models use `extra = "forbid"`, base64 fields with length caps, and
  hex patterns.
- Every 429 carries `Retry-After`, following #41.
- Every vault route except `GET` is under a per-user advisory lock
  (`SELECT … FOR UPDATE` on `vaults`), following #41's practice of locking the
  device before refresh tokens. `POST /v1/vault` has no row to lock yet; the
  `vaults` primary key makes the second of two racing creates fail with 409.
- **Limits that are security properties are counted in Postgres**, not by
  `InMemoryRateLimiter` (`rate_limit_service.py`), which is per process and
  forgets on every deploy. That covers one live handoff, one pending reset,
  resets, creates and rotations per day, and the claim budget. They are counted
  from the vault tables and the content-free audit rows. The in-memory limiter
  stays for request smoothing.
- `VELVT_VAULT_ENABLED=false` answers every `/v1/vault/*` route with
  404 `feature_disabled`, before the token, the body or the database is
  looked at. As built in C2 this is a gate in front of routing
  (`app.middleware.vault_gate`), which also answers `413 vault_too_large` for
  a body over 1 MiB without reading the rest.

### 8.2 Auth dependencies

| Dependency | Source | Used for |
|---|---|---|
| `require_active_device` | develop | Every vault read and write, handoffs, the device list |
| `require_user_session` | #41; since C1 it also refuses (401 `invalid_token`) a user access token whose session's refresh chain has ended, so a removed or logged-out Mac cannot register or reissue itself with the access token it still holds | `POST /v1/devices`, `POST /v1/auth/devices/reissue`, `POST /v1/auth/revoke-devices` |
| **`require_reauth`** (new) | PR C1 | A `reauth` token at most 300 s old, bound to the calling device and user. Used to remove another device, Delete Synced Data, and request a reset |

**`POST /v1/auth/reauth`**
- Request: `{password}` with a device token.
- Response: `{reauth_token, expires_in: 300}`. It is a JWT of type `reauth`
  with no refresh token.
- It shares the per-email login rate limit and lockout, and successful
  re-authentication does not count against it, as in #41.
- The helper sends the password once and never stores it. `password` is
  already in the egress ledger's `SECRET_FIELDS`, and `reauth_token` is added.

### 8.3 Endpoints, v1

**The normative wire contract is velvt-core `docs/api.md`, "Personal Vault"**
(C2, #46), with velvt-app `docs/architecture/vault-v1-formats.md` for the
byte layouts and the per-endpoint table of what velvt-core checks against each
request (A1, #89; reconciled with C2 in A3, #91). Where this table and those
differ, they win; this table is kept in step with them as of this revision.

| Method and path | Auth | Request | Response | Errors and notes |
|---|---|---|---|---|
| `GET /v1/vault` | device | — | 200 `{key_id, write_pk_b64, created_at, rotations_b64[], pending_reset?: {id, key_id, requested_at, effective_at}, completed_reset?: {id, key_id, requested_at, effective_at, resolved_at}, tombstone?: {key_id, deleted_at, statement_b64}}` | 404 `vault_not_found` with the same three optional fields in `error.details`. The reset fields are in the 404 body too, because a completed reset leaves no vault until someone makes a new one (§6.1). The 200 carries `tombstone` too (30 days), so a Mac that was offline across Delete Synced Data and a new Turn On can verify the deletion under the key it trusts |
| `POST /v1/vault` | device | `{key_id, write_pk_b64, blob: {name:"prefs", schema, ciphertext_b64}, create_sig_b64, put_sig_b64}` | 201 `{version: 1}` | `put_sig` binds `base_version` 0 and `new_version` 1 under the new key, and the blob is sealed at AAD version 1. 409 `vault_exists`; 409 `vault_reset_pending`; 422 `vault_key_id_invalid` if `key_id ≠ f(write_pk)`; 422 `vault_signature_invalid` if either signature fails. 10 a day. Emails the account "Sync was turned on for your Velvt account" (names no device). Audit `vault.created` |
| `GET /v1/vault/blobs/{name}` | device | `If-None-Match: "<v>"` | 200 `{version, key_id, schema, ciphertext_b64, put_sig_b64, base_version, updated_at}` with an ETag; 304 | `name` must be `prefs` in v1 (any other: 404 `vault_blob_not_found`). 600 an hour, shared with `GET /v1/vault` |
| `PUT /v1/vault/blobs/{name}` | device | `If-Match: "<n>"` (the current server version); `{key_id, schema, new_version, ciphertext_b64, put_sig_b64}` | 200 `{version: new_version}` | The server requires `n < new_version ≤ n + 2^32`, and the signed `put_stmt` binds both. The writer picks `max(n, highest_seen_version) + 1`, so a database restored from backup, whose versions went backwards, heals on the first write instead of looping on 412 against every Mac's rollback guard. 428 `vault_if_match_required` without `If-Match`, 400 `vault_if_match_invalid` for a malformed one; 412 `vault_version_conflict {current_version}`; 422 `vault_version_invalid` for a `new_version` out of range; 409 `vault_key_mismatch`; 403 `vault_write_unauthorized` (bad signature, audited `vault.signature_rejected`; past the refused-signature cap, 429 and no row); 422 `vault_blob_invalid` for any ciphertext length but the six (§5.3), 413 `vault_too_large` past the largest. 120 an hour. **No audit event per successful write** |
| `POST /v1/vault/rotate` | device | `If-Match: "<n>"`, as for `PUT`; `{old_key_id, new_key_id, new_write_pk_b64, rotate_stmt_b64, old_sig_b64, new_sig_b64, blob: {…, new_version, put_sig_b64}}` | 200 `{version}` | The blob's put is signed by `W'` under `key_id'`, with `base_version` = the server's current version `n` and `new_version` = the rotate statement's `new_version`; a stale `base_version` gets 412 `vault_version_conflict`. 422 `vault_statement_invalid` if the statement is not exactly the one the request describes; 422 `vault_key_id_invalid`; 409 `vault_key_mismatch`; 403 on any of the three signatures. Deletes a live handoff (it carries the old key); a pending reset's `key_id` follows the vault to the new key. 10 a day. Audit `vault.rotated` |
| `DELETE /v1/vault` | device + `require_reauth` | `{key_id, delete_stmt_b64, sig_b64}` | 204 | Deletes the `vaults` row, the blobs and the handoffs, writes the tombstone (30 days), and cancels a pending reset (reason `vault_deleted`) so Turn On can create again. The statement's `requested_at` must be within ±600 s of the server's clock (else 422 `vault_statement_invalid`), so a tombstone cannot be replayed later; its `at_version` must be the current version (else 412). 409 `vault_key_mismatch`; 403 without a valid signature or re-authentication. Audit `vault.deleted` |
| `POST /v1/vault/reset-requests` | device + `require_reauth` | `{}` | 201 `{id, effective_at}` | `effective_at` = 72 h after the later of now and the account's most recent removal of one device by another or cap eviction; any later sign-out of a Mac it did not ask for moves it again (below, and J9). 409 `vault_reset_pending` if one is pending; 404 `vault_not_found` without a vault; 3 a day. Emails the account (names no device). Audit `vault.reset_requested`, later `vault.reset_postponed`, `vault.reset_cancelled` or `vault.reset_completed` |
| `DELETE /v1/vault/reset-requests/{id}` | device | `{cancel_stmt_b64, sig_b64}` | 204 | Signed by the current `W`. Any Mac holding `K` can cancel. 404 `vault_reset_not_found` for an id that is not this account's pending reset, including one already due; 422 `vault_statement_invalid`; 403 `vault_write_unauthorized`. Audit `vault.reset_cancelled` (reason `key_holder`) |
| `POST /v1/vault/handoffs` | device | `{handoff_id (32 hex), payload_b64 (≤ 256 B), expires_at (integer Unix seconds, as signed), sig_b64}` | 201 `{expires_at}`: the expiry kept, `min(expires_at, now + 600)` | Signed by `W`. One live handoff per account; a new one replaces it. 422 `vault_handoff_expired` for an expiry already past; 409 `vault_handoff_conflict`; 404 `vault_not_found`. 10 an hour. Audit `vault.handoff_created` |
| `POST /v1/vault/handoffs/{id}/claim` | device, same user | — | 200 `{payload_b64}`; the row is deleted | 404 `handoff_not_found` (unknown, expired, another user's, or already claimed). 10 per 10 minutes per account, every claim counted, answered or not (§17.1 D10). Audit `vault.handoff_claimed` (success or failure) |
| `GET /v1/devices` (new) | device | — | 200 `[{id, status, created_at, last_seen_at, client_version, is_current}]` | The caller's account only; revoked rows from the last 30 days included with `status`. No APNs fields, no names |
| `DELETE /v1/devices/{id}` (extended) | device for itself (unchanged); device + `require_reauth` for another device of the same user | — | 200 | `revoke_device(actor="user", reason="removed_from_other_device")`. Another user's id gets 404. If a reset is pending, its `effective_at` moves to 72 h after this removal. Audit `device.removed_by_other` |
| `POST /v1/auth/revoke-devices`, `POST /v1/devices` (past the cap), `POST /v1/auth/devices/reissue` (existing routes, extended) | user session | unchanged | unchanged | Each needs only the password. When it signs out a Mac that was signed in (revoke-all with any device signed in; a cap eviction of a device with a live refresh token; a reissue that revokes a live refresh token), a pending reset's `effective_at` moves to 72 h after it, as for a removal. Audit `vault.reset_postponed` with reason `device_tokens_revoked_all`, `device_limit_reached` (actor `system`) or `device_token_reissued`. A Mac signing back in after Log Out holds no live refresh token and moves nothing |

**The reset lifecycle (server)**
- When `effective_at` passes, the reset has taken effect. The first vault
  request for the account after it completes the request (it deletes the
  vault, blobs and handoffs, and marks the request `completed`), and
  `enforce_retention` completes the rest, but only while `VELVT_VAULT_ENABLED`
  is on (§17.1 D9). A reset already due is no longer pushed out by a sign-out,
  cancelled by a key holder, or cancelled by a password reset.
- The completed request stays visible in `GET /v1/vault` (200 or 404) for 30
  days, so Macs that were offline can tell what happened (§6.1).
- While the request is pending, `POST /v1/vault` answers 409 `vault_reset_pending`.
  Once it is completed, a new vault can be created.
- `confirm_password_reset` cancels every pending reset requested before it
  (status `cancelled`, audit `vault.reset_cancelled` with reason
  `password_reset`). Today a password reset already revokes every device token
  (`auth_service.revoke_device_tokens`), so without this rule the person's
  Macs would all be signed out exactly when they need to cancel.

**Account email.** `password_reset_delivery.py` has a Resend deliverer that
sends only the reset-link message. C2 generalizes it into an account-notice
sender with three fixed templates: vault created, reset requested, and reset
cancelled by a password reset. Q18 would add a fourth, new sign-in. Each names
no device and carries nothing from the vault. The `Unconfigured` and `Logging`
deliverers keep their current behaviour. As built (C2), the Resend request is
`email_delivery.ResendEmailSender`, shared by the reset deliverer and the
notice sender, and the three texts are in `app/services/account_notice.py`
(their approval is §17.1 D11).

**Wire-contract choices as built (C2, #46), which A1 and A3 match.** C2's
decisions 1–11 and the three its review added:
- **Encodings inside signed bytes** as in §5.3: `user_id` is the UUID text,
  times are whole Unix seconds, a reset request id is 16 raw bytes, and a
  stored statement is the statement followed by its signature(s).
- **`vault_blobs` stores `base_version` and `put_sig`** (§4.2), which the blob
  response and §6.1's client check need.
- **Ciphertext sizes:** only the six padded steps plus 28 bytes (§5.3).
- **Rotation** takes `If-Match`, deletes a live handoff, and moves a pending
  reset's `key_id` to the new key.
- **`GET /v1/vault`'s 200 carries `tombstone`** (§6.1 handles it).
- **Setup-code expiry:** integer Unix seconds; the server keeps
  `min(expires_at, now + 600)`; a past expiry is 422.
- **When a reset takes effect:** at `effective_at`, even before anything
  deletes it (above).
- **A signed deletion cancels a pending reset** (reason `vault_deleted`).
- **Header errors:** a missing `If-Match` is 428 `vault_if_match_required`, a
  malformed one 400 `vault_if_match_invalid`.
- **Budgets:** the two reads share one budget; `PUT`, rotation, deletion and
  reset cancel share one write budget; failed claims count toward the claim
  budget (§8.4).
- **Audit events beyond the list above:** `vault.reset_postponed`,
  `vault.reset_completed`, `vault.signature_rejected` (with the operation)
  and `vault.notice_sent` (with the delivery status). All are deleted at
  erasure (C0).
- **New from C2's review:** a delete statement's `requested_at` must be
  within ±600 s of the server's clock (422 `vault_statement_invalid`); a
  refused write past the hourly refused-signature cap is 429, not 403; and
  the read, write and setup-code budgets are counted per device (§8.4,
  §17.1 D8).

### 8.4 Quotas and rate limits (v1)

| Resource | Limit |
|---|---|
| Blobs per account | 1 (`prefs`) |
| Blob size | 512 KiB padded |
| Reads | 600 an hour per user (the poll is every 10 minutes, plus wake and popover open) |
| Writes | 120 an hour per user (debounced 5 s after a change) |
| Creates, rotations | 10 a day each |
| Handoff creates / claims | 10 an hour / 10 per 10 minutes |
| Reset requests | 3 a day; one pending |
| Re-authentication | the login budget |
| Removing another device | 10 re-authenticated removals an hour per account, counted only after the re-authentication check |

The removal row records an order the code depends on (C1, #45): a request
without a valid re-authentication spends nothing, so a stolen Mac that holds a
device token but not the password cannot use up the budget and keep the owner
from removing it.

**Open (§17.1 D8).** As built, C2 (#46) counts reads (600 an hour), signed
writes (120 an hour: `PUT`, rotate, delete, reset cancel) and setup codes
made (10 an hour) per device, not per user, and caps the refused-signature
audit rows at 10 an hour per account (past it, 429 and no row). Until the
founder confirms that, the rows above are the design's.

### 8.5 Retention (v1)

| Store | Kept | Removed by |
|---|---|---|
| `vaults`, `vault_blobs` | While the account exists. Founder option (§17 Q16): 13 months with no device syncing | `DELETE /v1/vault`, a completed reset, erasure |
| `vault_handoffs` | At most 10 minutes; deleted on claim | Claim, `enforce_retention` on indexed `expires_at`, refused on read once expired, erasure |
| `vault_tombstones` | 30 days | `enforce_retention`, erasure |
| `vault_reset_requests` | 30 days after resolution | `enforce_retention`, erasure |
| Audit events | `audit_retention_days` (365) | Erasure (§8.6) |
| Database backups | At most 14 days (`backup_database.sh`) | Age-out only. A backup taken before an erasure still holds that account's pre-erasure rows, including un-anonymized audit rows; its vault rows are ciphertext, unreadable without `K`. State both in `privacy-contract.md` |
| Account-notice email (Resend) | The provider's own delivery-log retention | Not reachable by erasure. Each message holds the address, a fixed subject and a fixed body with no device name and nothing from the vault. Name the provider and this limit in `/privacy` and `privacy-contract.md` |
| Application logs | Rotated by size, 5 × 10 MB per service, with no age limit | Not reachable by erasure. Some lines carry a user id, and access-log paths name device ids (`/v1/devices/{id}`). Listed next to backups and Resend in `privacy-contract.md` (C0) |

### 8.6 Erasure

- `Vault`, `VaultBlob`, `VaultHandoff`, `VaultTombstone` and
  `VaultResetRequest` join `USER_OWNED_MODELS`, children first and before
  `Device`. `verify_account_erasure` counts them.
- #43's guard test (`test_every_table_holding_a_user_id_is_erased_with_the_account`)
  picks the new tables up automatically.

**Audit rows (PR C0)**
- Today `audit_log_entries` is not in `USER_OWNED_MODELS`, and #43's guard test
  keeps it on purpose (`kept = {"audit_log_entries", "account_erasure_requests"}`).
  Rows carrying the user's id therefore survive erasure for 365 days.
- C0 changes `delete_user_owned_records`:
  1. Delete that user's audit rows whose `event_type` starts with `vault.`,
     `sync.`, `trust.`, `pairing.`, `relay.` or `device.`.
  2. On every other audit row for the user, set `user_id`, `device_id`,
     `actor_id` and `request_id` to NULL, and set `resource_id` to NULL where
     it names a user-owned row. `request_id` goes too, because it can be
     joined to request logs. `event_type`, `outcome` and `created_at` stay, so
     security totals still add up. The match is on `user_id` and also on
     `actor_id = user_id`, since some events record the user only as actor.
  3. `account.erasure_completed` metadata carries only a total deleted count,
     not per-table counts, so the record does not show that the account used
     sync.
  4. `execute_account_erasure` writes `account.erasure_completed` *after* the
     anonymization step, and today it sets `user_id`. C0 writes it, and
     `account.erasure_started`, with `user_id` NULL and `resource_id` = the
     erasure request id. The link to the person then lives only in
     `account_erasure_requests`.
  5. The `velvt_erasure_records_deleted_total` metric is labelled per table
     today. C0 folds the vault, sync and trust tables into one `sync` label.
     Otherwise a `vaults` increment at the moment of an erasure shows that the
     account used sync, in a store erasure never reaches. (One bit remains: a
     `sync` increment at that moment still shows the account used sync; §17.1
     D1.)
  6. Late writers. A request in flight when the erasure runs can write an
     audit row after it: an open insight long poll ends at its next check,
     or, when a newer poll has overtaken it, at its timeout; the
     password-reset delivery record is written once Resend answers. A poll's
     audit row for an account that is no longer `active` carries no
     `user_id`, `device_id` or `resource_id`. The worker's erasure sweep
     (`VELVT_ACCOUNT_ERASURE_SWEEP_INTERVAL_SECONDS`, 300 s) runs
     `erase_audit_entries` again for every completed erasure whose user id
     still appears in `audit_log_entries.user_id`. That sweep also cleans
     accounts erased before C0 and folds their old per-table completion
     counts into one total.
- The guard test changes so that no audit row holds the user id after
  erasure and the next sweep. The only rows that still reference the opaque
  user id are the anonymized `users` row and `account_erasure_requests`, the
  durable completion record.

**Mac side**
- Delete Account sends `delete_vault_key` for every key id held for the
  account, retired ones included.
- Other Macs receive `account_deleted` and do the same. **Open (§17.1 D22):**
  this conflicts with I6, and the A3 build (#91) follows I6 instead: other
  Macs delete the account's sync rows and keep their Keychain items.
- The account's `vault_state` row (anchor included), `vault_rule_link`,
  `sync_known_device` and `sync_notice` are deleted.
- A Mac on another Apple ID that never connects again keeps its copy of `K`.
  This leaks nothing, because nothing server-side is left for `K` to open
  once the backups age out.

### 8.7 Composition with the in-flight Phase 2 PRs

| PR | What this design uses or changes |
|---|---|
| **#41** token lifecycle | Uses generation-checked device tokens on every vault call; `require_user_session` as the model for `require_reauth`; `last_seen_at` for the list; `revoke_device` for removal. The cap's eviction surfaces as a revoked row, and the evicted Mac rejoins on its next sign-in. v1 needs no cap change. Phase 4 amends eviction to skip chain members and answer 409 `device_limit_reached` when every active row is a member. `install_id` stays out (v1 needs none; Phase 4's member key is the one disclosure, §17 Q14) |
| **#42** cross-device aggregation | No interaction. "Your history follows you" works on day one, with no vault |
| **#43** per-device delivery | `notify_here` stays the server's switch for **server-originated** pushes. For Macs it is left at its default (true), and the per-Mac daily-insight switch lives in the vault, applied by Rust (it omits `notification_payload`). The server stays blind to Mac routing. Phase 4 phones use `notify_here` for the content-free insight alert. C0 edits #43's erasure guard test, so C0 merges after #43 |
| velvt-app **#84**, **#86** | They take protocols 36 and 37 and migration 0044. This design's first IPC change is protocol 38, and its migrations start at 0045 |
| velvt-app **#72** (shadow return ledger) | The `blobs/{name}` namespace leaves room for a `ledger` blob. Syncing it needs Gate D and its own pre-registration |

### 8.8 Phase 4 endpoints (summary)

All of these sit behind `VELVT_DEVICE_TRUST_ENABLED`, with per-user limits.

| Method and path | Purpose |
|---|---|
| `GET /v1/trust/chain?after_seq=` | Chain entries. Device token, or an enrollment token during pairing |
| `POST /v1/trust/chain` | Compare-and-append `{expected_head_seq, signed_entry_b64, wraps?[]}`. Errors: 409 `chain_conflict {head_seq, head_hash}`, 422 `chain_entry_invalid`, 403 `member_not_active`. At most 20 entries a day |
| `POST /v1/trust/bindings` | Binds a device row to a member: `proof` = Ed25519 over `"velvt-bind-v1\0" ‖ account_binding ‖ device_id ‖ server_nonce` |
| `POST /v1/trust/pairing-sessions` | `{kind: qr \| codes}` → `{session_id, enrollment_token?, expires_at}`. 5 an hour |
| `GET /v1/trust/pairing-sessions/open` | Open `codes` sessions, for the joining Mac |
| `POST …/{id}/messages`, `GET …/{id}/messages?after=` | Opaque relay of up to 2 KB per step; a 25 s long poll during pairing only |
| `POST …/{id}/claim` | Enrollment token: the phone's claim, which creates a pending device row with no tokens |
| `GET …/{id}` | Enrollment token: `pending \| approved` (device tokens, **issued once**) `\| declined \| expired` |
| `POST …/{id}/decline`, `DELETE …/{id}` | Not Mine, or abort |
| `GET /v1/vault/keys?epoch=` | The caller's wrap |
| `POST /v1/trust/rotate` | Atomically: entry, wraps, re-sealed blob, delete old wraps, revoke the target's rows |
| `POST /v1/relay/messages` | `{messages: [{recipient_member_id, envelope_b64 (exactly 560 B), ttl_seconds ≤ 43200, collapse_id}]}`. APNs alert with `mutable-content: 1` and generic text, `apns-expiration` = TTL, random collapse id. 120 an hour per device |
| `GET /v1/relay/inbox/poll`, `DELETE /v1/relay/inbox/{id}` | Replies for the Mac, which needs no APNs |

**Phase 4 erasure.** `RelayMessage`, `PairingSession`, `VaultKeyWrap`,
`DeviceKeyBinding` and `DeviceChainEntry` join `USER_OWNED_MODELS` before
`Device`.

**Phase 4 retention**

| Data | Kept for |
|---|---|
| Pairing sessions | 1 h after expiry |
| Relay messages | Until delivery or TTL, at most 12 h |
| Past-epoch wraps | Deleted inside the rotation transaction |
| Bindings | 30 days after their device row is revoked |
| Chain entries | The vault's lifetime (they are the security log) |

---

## 9. IPC (helper ↔ app)

### 9.1 Rules

Each protocol bump follows AGENTS.md, all in one commit:
- `proto/version`;
- a JSON Schema per message in `proto/schema/`;
- a `proto/CHANGELOG.md` entry;
- both xcconfigs and `PROTOCOL_VERSION`;
- `docs/architecture/ipc-contract.md` and `DOC_INDEX`;
- version-mismatch tests on both sides.

`ClientMessage` rejects unknown variants, so every new message needs the bump.

### 9.2 Protocol 38 (Sync v1)

**Swift → Rust**

| Message | Payload | Notes |
|---|---|---|
| `request_sync_state` | `{}` | Answered with `sync_state` |
| `set_sync_enabled` | `{enabled, default_name?}` | `default_name` is a fact (the computer name), at most 64 characters |
| `vault_keys_held` | `{user_id, keys[] {key_id, supersedes?, retired_by?}, reason?: keychain_unavailable \| entitlement_missing}` | The answer to `vault_key_inventory_request`. Key ids and their rotation markers (§5.5), never key bytes. Rust, not Swift, decides what the markers mean |
| `vault_key_retired` | `{key_id, ok}` | The answer to `retire_vault_key` |
| `vault_key` | `{user_id, key_id, key_b64}` | The answer to `vault_key_request`. **A secret:** sent only to a peer-verified helper (A0). Rust holds it in a redacted, zeroizing type |
| `vault_key_missing` | `{user_id, key_id, reason: not_found \| keychain_unavailable \| entitlement_missing}` | |
| `vault_key_stored` | `{key_id, ok, reason?}` | |
| `rename_device` | `{sync_device_id, name}` | |
| `set_notification_routing` | `{sync_device_id, daily_insight}` | |
| `remove_device` | `{server_device_id, password, change_sync_key}` | The password is relayed once to `/v1/auth/reauth` and never stored or logged. `change_sync_key` arrives with PR A8 |
| `create_pairing_code`, `cancel_pairing_code` | `{}` | |
| `redeem_pairing_code` | `{code}` | The raw typed text |
| `delete_synced_data` | `{password}` | |
| `request_sync_reset` | `{password}` | The lost-key path |
| `cancel_sync_reset` | `{}` | |
| `replace_server_copy` | `{}` | The signed repair (§6.1), offered after 7 days |
| `acknowledge_sync_notice` | `{notice_id}` | |

**Rust → Swift**

| Message | Payload |
|---|---|
| `vault_key_inventory_request` | `{user_id}` |
| `vault_key_request` | `{user_id, key_id}` |
| `store_vault_key` | `{user_id, key_id, key_b64, supersedes?}`. **A secret**, with the same rule as `vault_key` |
| `retire_vault_key` | `{user_id, key_id, retired_by}`. Rewrites the item's value with the marker and keeps its key bytes |
| `delete_vault_key` | `{user_id, key_id}` |
| `sync_state` | Rust-authored snapshot, answering every command above: `state` (`off \| available_elsewhere \| waiting_for_key \| syncing \| waiting_for_rotated_key \| reset_pending \| reset_completed \| missing_on_server \| server_copy_unreadable \| server_inconsistent \| read_only_newer_schema \| clone_detected \| error`), `status_line`, `fingerprint`, `devices[] {sync_device_id?, server_device_id?, name?, is_this_mac, status, last_seen_bucket, can_remove, notify_daily_insight?}`, `pairing {code, expires_at} \| null`, `held_summary {site_rules, app_rules, settings[]}`, `pending_reset {effective_at} \| null`, `notices[] {notice_id, kind, title, body}`, `last_synced_at?`, `error_code?` |
| `sync_notice` | `{notice_id, kind, title, body, actions[]}`. Rust-authored copy; Swift posts it as the fourth notification kind |

**Account events**
- `needs_reauth.reason = account_deleted`: Rust deletes the account's sync rows
  and sends `delete_vault_key` for each held key id. (Open, §17.1 D22: A3
  sends `delete_vault_key` only for this Mac's own Delete Account,
  `account_deleted(user, AccountDeletion::ThisMac)`.)
- `log_out`: clears the session columns of `vault_state`, keeps the anchor
  (§4.3), and drops `K` from memory. It never deletes a Keychain item.

**Privacy-boundary tests** cover the new messages. No raw field can appear in
them, and the key fields use redacted types in both Rust (like
`RedactedString`) and Swift (a type whose `description` is redacted).

### 9.3 Secrets on the socket, and PR A0

Today `ipc/transport.rs` sets the socket directory to `0700` and checks nothing
else. Tokens already cross the socket, and v1 adds `K` in both directions.
PR A0 lands first.

**Rust checks Swift**
- On accept, read the peer's audit token with `getsockopt(SOL_LOCAL,
  LOCAL_PEERTOKEN)`.
- Require the token's process (`audit_token_to_pid`) to be the helper's parent
  (`getppid()`), and the parent not to be launchd: the app starts the helper
  as its child, so an orphaned helper serves nobody.
- Get its code object with `SecCodeCopyGuestWithAttributes(kSecGuestAttributeAudit)`.
- Check it with `SecCodeCheckValidity` against the requirement
  `anchor apple generic and certificate leaf[subject.OU] = "<TEAM_ID>" and identifier "com.velvt.mac"`.
- This needs a small hand-written FFI to Security.framework and CoreFoundation,
  with no new crate, costed at about two days.
- The alternative is the `security-framework` crate (3.7.0). Its
  `os::macos::code_signing` module already has `GuestAttributes::set_audit_token`,
  `SecCode::copy_guest_with_attribues` and `check_validity`. It is in
  `Cargo.lock` only through the optional `onnx` feature (`ort` → `native-tls`),
  so in the default build it would add `security-framework`,
  `security-framework-sys` and `core-foundation`. That needs founder approval.
  A0 as built (#88) uses the hand-written FFI, plus `audit_token_to_pid` from
  libbsm, and adds no crate (§17.1 D19).

**Swift checks Rust**
- Require the serving process to be the app's own child (`proc_pidinfo`
  `pbi_ppid == getpid()`), which the helper `ServiceProcessLauncher` started
  is.
- The same signature check on the server end of the socket, against
  `identifier "com.velvt.service"`.
- `scripts/sign_release.sh` gains `--identifier com.velvt.service` for the
  helper. Today it signs the helper with the default identifier.
- `verify_release.sh` checks both identifiers.

**In every build**
- Debug builds skip both checks.
- A failed check drops the connection, logs an error code only, and shows
  `service_status` as unavailable.
- Until A0 is in the build, the sync messages that carry secrets are refused
  at compile time (`cfg`).

**Why the pin.** `LOCAL_PEERTOKEN` describes whatever image the peer's PID
runs when it is read. A process can connect (or bind the path), fork a child
that keeps the socket, and exec a genuinely signed binary (Velvt's own), and
the signature check passes while the child reads. Both checks use one token,
pin first: the token's pid version, which `SecCodeCheckValidity` verifies,
makes the pinned process the one whose signature passed. Checks run per
connection, off the accept loop.

The signature-only check this section first specified does not hold, and both
of A0's reviews showed it. The security-review packet (D1, R1) cites this
amended section and the tests in §16.1 (IPC), not the signature-only text.
XPC (for example `xpc_connection_set_peer_code_signing_requirement`) remains
the longer-term alternative and is not required for T17 as scoped.

### 9.4 Phase 4 messages (summary)

| Direction | Messages |
|---|---|
| Swift → Rust | `device_secret {user_id, secret_b64, protection: secure_enclave \| keychain}`, sent after `auth_session` on each connection; `device_secret_unavailable {reason}`; `start_pairing {method: qr \| codes \| approve_with_phone \| recovery}`; `enter_pairing_code {session_id, code}`; `respond_pairing_claim {session_id, decision}`; `create_recovery_key {password}`; `join_with_recovery_key {code}`; `remove_device` (as v1, with `member_id`); `log_out.remove_this_device: bool` |
| Rust → Swift | `device_secret_required {user_id}` (Swift generates the seed with `SecRandomCopyBytes`, seals it and stores it); `device_secret_discard`; `pairing_state {session_id, phase, show_code?, qr_payload?, peer_name?, failure?}`; `device_list` v2; `trust_alert`; `recovery_key_created {code}`, shown once and the only secret that flows Rust → Swift in v2 |

Relay routing adds `surface` to `intervention_card_seen` and
`intervention_delivery` (report 4.2).

---

## 10. What syncs in v1, and how merges resolve

### 10.1 What syncs, and what each receiving Mac does with it

| Field | Class | On the receiving Mac in v1 |
|---|---|---|
| Mac names and device entries | Presentation | **Applied** to the device list |
| `notify_daily_insight` for this Mac | Routing for a surface that is not a measured treatment (report 4.4) | **Applied.** Rust omits `notification_payload` on this Mac when off. The insight still shows in the popover. If a pre-registration ever names insight notifications as an exposure, this register moves to the held class under that registration |
| Category rules (site, bundle and app-name person keys) | Classification input | **Held.** Stored in the document only. The ladder never consults them |
| Quiet hours (`velvt_quiet_hours`) | Policy and delivery input. Inside the window, a drift offer drops to `InterventionSalience::Quiet`, an in-app card with no OS notification (`ipc/router.rs`); initiation invitations, the weekly digest and the needs-a-category reminder are held (`initiation`, `receipts`, `category_prompt`); and the daily insight carries `do_not_disturb_until` (`delivery/poll.rs`) | **Held** |
| `invitations_enabled` | Initiation policy input | **Held**, including an opt-out (§17 Q6) |

The local values of quiet hours and the invitations switch are written into the
document, so the copy stays current for the day the gate opens. They are never
applied from it.

### 10.2 Not synced in v1

| Not synced | Why |
|---|---|
| Activity names (typed names, and the application's own local name that triage-taught app rules store, per PRIVACY.md) | A typed name may identify something, and the local name *is* an app name. If names ever sync, each carries `activity_name_source` (typed or `app_local_name`), and `app_local_name` is never queued |
| `local_site_name` hosts, app names, bundle ids, window titles | The privacy boundary. Rules travel only as person keys |
| Window-level corrections (`personal_override`) | Keyed by window titles |
| Work blocks, decisions, observations, outcomes, Focus evidence, initiation windows, digests, demotion state | Each Mac's measurement stays its own |
| Needs-a-category list entries and suggestions | A suggestion would change the answers (§11) |
| History | Already stored per user on the server (#42) |
| Reminder routing (`notify_category_reminder`) | Held for CATEGORY_PROMPT_POLICY_VERSION 2 (§11) |

### 10.3 Merge rules

- **Registers.** Every mutable value is a register `{value, hlc}`: each device
  field, each setting, each rule's `{category, deleted}`. Merging two documents
  takes, for each register, the one with the higher HLC.
- **Write-once fields.** `person_salt` is set once and the server copy's value
  wins. `joined_via` and `joined_at_ms` are set once by the device itself.
- **The merge is commutative, associative and idempotent.** Any set of Macs
  converges on the same document whatever order they sync in. A property test
  checks every permutation of up to four concurrent edits.
- **Deletes are tombstones** (`deleted: true` with a newer HLC). Tombstones
  older than 180 days are dropped on write. A Mac offline for longer than that
  could bring a deleted rule back. That is documented, and it is harmless,
  since rules are held.
- **Per-field registers on device entries.** Mac A renaming Mac B, while B
  changes its own routing, keeps both changes.
- **Clock skew.** The HLC never goes backwards on a Mac. A Mac whose clock runs
  fast wins concurrent edits for as long as it is ahead, which is acceptable
  for preferences.
- **Counter overflow.** The counter is four digits, so the string order holds
  only while it stays at four. When the next counter would be 10000, the Mac
  advances the millisecond field by one and resets the counter to 0. Without
  this, a Mac that adopted a far-future `highest_seen_ms` from a Mac with a
  bad clock would wrap to `…10000…` after 10,000 writes, which sorts *below*
  `…9999…`. Its own writes would then start losing.

### 10.4 The sync engine (Rust `vault::sync`)

**States** (`vault_state.state`; the copy comes from Rust)

```
off ──turn on──▶ (GET /v1/vault)
   404, no key held ─────────────▶ create: K, W, person_salt → store_vault_key → POST /v1/vault → syncing
   404, key held or seen before ─▶ missing_on_server   (explicit [Start a New Synced Copy] only)
   404, verified completed reset ▶ reset_completed     ([Set Up Sync Again] may create, §5.5)
   200, key in Keychain ─────────▶ syncing
   200, key not here ────────────▶ waiting_for_key ──key arrives / code──▶ syncing
syncing ─ key_id changed ─▶ (§6.1) waiting_for_rotated_key | reset_completed | server_inconsistent
syncing ─ pending reset ──▶ reset_pending (notice; [Cancel Reset])
syncing ─ open/validate fails ─▶ server_copy_unreadable (never overwrite)
syncing ─ schema > 1 ─▶ read_only_newer_schema (apply known fields; never write)
any ─ host binding mismatch ─▶ clone_detected (sync off; card)
```

**Triggers**
- service start once the key arrives;
- wake;
- `request_sync_state`, if more than 60 s have passed since the last fetch;
- every 10 minutes;
- 5 s after a local change, debounced.

**Fetch**
- `GET /v1/vault` (it is small), then `GET /v1/vault/blobs/prefs` with
  `If-None-Match`. A 304 is the common case.
- On a 200: verify (§6.1), store the ciphertext and version, then apply (§10.1).

**Write**
1. Build this device's local registers: its own entry, the `vault_rule_link`
   rows, and the settings whose local `configured_at`/`updated_at` is newer
   than the document register's HLC wall time and whose value differs.
2. Merge them into the last opened document. If nothing changed, stop.
3. Seal at `v = max(n, highest_seen_version) + 1`, where `n` is the server's
   current version, sign, and `PUT` with `If-Match: "n"` and `new_version: v`
   (§8.3).
4. On 412: refetch, re-merge and retry. After three tries with jitter, back off
   10 minutes.

**The device list and new-sign-in notices**
- With sync on, the 10-minute pass also calls `GET /v1/devices`.
- When sync turns on (and after each new session), the first list seeds
  `sync_known_device` without alerting. Otherwise turning sync on would post
  one `new_sign_in` for every device already on the account.
- After that, an active server device that is not in `sync_known_device` (and
  is not this Mac) produces `new_sign_in` once.
- A new `sync_device_id` in the document produces `mac_joined_sync`.

**The document is a mirror.** Every Mac's local tables stay authoritative for
that Mac. This makes reset and recovery self-healing, and keeps the damage a bad
document can do small.

**Egress**
- Every new endpoint joins `egress::ENDPOINTS` with `when` and `body` text. For
  example: "PUT /v1/vault/blobs/prefs — when your synced choices change — an
  encrypted copy of your sync settings and a signature; Velvt cannot read it."
- The hash-chained ledger records each request as it does today. `K` is never
  in any request.
- `velvt-service --dry-run-egress` gains a vault mode. It prints the plaintext
  document Rust is about to seal, so a tester can see exactly what syncs before
  turning it on.

---

## 11. Measurement integrity

**The rule.** If a synced value can change what the drift gate, initiation,
demotion, reminder or digest policy decides on another Mac, it is held. Applying
any held value needs its own pre-registration, and it never happens in the
middle of a wave.

**Why it matters**
- A synced site category changes what classifies as a site on another Mac.
  That changes the classified share, and through `is_confident`
  (`work_block/mod.rs`) the drift gate's evidence under
  `DRIFT_POLICY_VERSION = 6` (develop; #83 amends v6).
- Reminder routing changes how often people label, which changes the same
  share.
- Synced quiet hours would make drift offers quiet (in-app card only), and
  would hold invitations, the digest and the reminder (§10.1).
- A synced invitations switch silences invitations.
- A sync notification posted during a block is an app switch waiting to happen.
  This is why security notices wait for the block to end (J6).

### 11.1 Three locks on vault-driven classification

| Lock | What it is | State in v1 |
|---|---|---|
| 1. Compile-time | The apply rung (after the four local rungs and before the plugins, so a local rule always wins) and the held appliers sit behind the cargo feature `vault-policy-inputs`, off in every release build. With it, `pub const VAULT_POLICY_INPUTS_VERSION: Option<u32>` must equal a version registered in the pre-registration ledger. A `published_claims` test fails if the const is `Some` without a ledger entry. A test cannot see which features a release was built with (CI runs `cargo test` in debug, and a feature is a build flag, not a profile). So the release check lives in the scripts: `build_rust_helper.sh` never passes `--features vault-policy-inputs`, the helper gains a `--policy-inputs` flag next to `--protocol-version` that prints `none` or the version, and `verify_release.sh` refuses a build whose answer does not match the build's registered policy versions | Off; `None` |
| 2. Per Mac | Settings › Categories: "Use categories from my other Macs on this Mac", off by default. Note under it: "Changes which activity counts as work on this Mac, and so when Velvt offers a nudge." Its state is recorded on every decision | Not shown (compiled out) |
| 3. Provenance | `work_block_observation.classification_source` (with `vault_rule`) and `intervention_decision_log.synced_rule_evidence` (a count of confident observations in the window that a vault rule classified, counted from the observation rows, which record a vault rule's absorbed reports as well as the ones that open a row) are added **now**, in PR A2, so the logged data itself shows zero contamination. `export_cohort_evidence.sh` names every column it selects, so A2 does not change the export mid-wave. The flip PR adds these columns and the toggle's state to the export and bumps its format, so the analysis can stratify or exclude | Columns live; `vault_rule` cannot occur; evidence always 0 |

### 11.2 Further rules

- **No suggestion leaks.** The needs-a-category list never prefills or suggests
  from synced rules. A suggestion changes the answer, so it rides lock 1.
- **The reminder.** `notify_category_reminder` waits for
  `CATEGORY_PROMPT_POLICY_VERSION = 2`. That bump records the routing gate on
  each reminder decision in the category-prompt ledger.
- **Quiet hours and invitations** wait for the same amendment as rules, with
  their own named inputs in it.
- **The upload contract.** When it is on, `ClassificationSource::VaultRule` uses
  `ClassificationTier::ExactMatch`, like `UserRule`. So the upload contract
  (`cloud_abstraction_type`) does not change. The local source is what the
  analysis reads.
- **The IPC vocabulary.** `ClassificationSource` lives in `shared-types` and is
  serialized over the socket (`QueuedEventSummary.classification_source`). The
  `VaultRule` variant therefore sits behind the same feature, and the G1 build
  that turns it on carries its own protocol bump. Protocol 38 never carries it.
- **Never mid-wave.**
  - The feature and the const change only in a build cut between waves, under a
    new `DRIFT_POLICY_VERSION` registered before the wave starts.
  - Builds inside a running wave keep the feature off.
  - Decisions logged under two policy versions are never pooled, as today.
- **Testers in a wave may use sync** (data flow only) once it is reviewed and
  disclosed, because no policy input changes. Whether sync is on is not a
  policy input and is not recorded on decisions (§17 Q13).

### 11.3 Proof (tests that gate merge)

**Golden replay**
- A recorded trace runs through the engine, the drift gate and the decision log
  twice: with an empty vault, and with a populated vault that has a rule for
  every site and app in the trace, held quiet hours covering the whole day,
  and invitations held off.
- The test asserts byte-identical decisions, `intervention_decision_log` rows
  and `work_block_observation` rows.
- As built in A2 (#90, `tests/golden_replay.rs`), the harness replays a
  six-block trace that reaches no Tier 2 match, reports no Focus state and
  sets no quiet hours. The populated-vault arm (A7) therefore needs a second
  trace and golden, recorded on the code before A7: it must reach Tier 2 (the
  vault rung sits before the plugins), have Focus on during a block, and set
  quiet hours, with `embedding` added to its expected sources. A2's golden
  stays as it is.

**Gate tests**
- With the const `None`, a vault rule for a site never changes its category.
- Held quiet hours never hold an offer.
- Held invitations-off never suppresses an invitation.
- Synced rules never appear in `unclassified_triage`.
- No `sync_notice` reaches Swift while a work block is active. It arrives
  after the block ends.

---

## 12. Privacy and consent text to update

Before any tester can turn sync on, the founder approves the wording (§17 Q1).
The PR that turns on `SYNC_UI_ENABLED` carries all of these.

| Where | Change |
|---|---|
| Consent (cohort kit, `/privacy`) | New opt-in line (below) |
| velvt-web `/privacy` §2 | "Sync across your Macs" paragraph (below) and the list of what the server can see |
| velvt-app `PRIVACY.md` | A new section "Sync across your Macs"; rows for `vault_state`, `vault_rule_link` and `sync_known_device` in the storage table and "Every column"; the two provenance columns; the Keychain item; the egress rows; the host-binding HMAC |
| velvt-app `PRIVACY_AUDIT.md` | A dated note: what changed, and why no raw field leaves the Mac |
| `tests/published_claims.rs` | `MIGRATED_TABLES`; the const and feature claim (§11.1); a structural test that the document type's only free-text field is `name`; ENDPOINTS equality |
| `rust-service/src/egress/mod.rs` | ENDPOINTS rows; `reauth_token` in `SECRET_FIELDS` |
| velvt-core `docs/privacy-contract.md`, `docs/retention.md`, `docs/api.md` | Vault tables, retention, erasure (including audit anonymization, completed by the worker's sweep within minutes), what pre-erasure backups still hold for 14 days (§8.5), the account-notice emails and their provider, and application logs as a store erasure cannot reach |
| velvt-app `AGENTS.md` | Notifications: four kinds, adding the security notice. Privacy boundary: a third tier, "end-to-end encrypted to the person's own devices: may leave the Mac only as vault ciphertext under keys velvt-core never holds, with contents limited to the closed `prefs` schema" |
| velvt-core `AGENTS.md` | The same tier, and "the server never stores a vault key or plaintext" |
| `ARCHITECTURE.md` | A ladder note: the dark apply rung |
| Tester kit, `RELEASES.md` | One line each: sync exists, is off by default, and what it sends |

**Draft consent line** (new, optional)

> **Sync across your Macs (optional, off until you turn it on).** Velvt keeps
> an encrypted copy of your choices on its server so your other Macs can use
> them: Mac names, notification choices, category rules (as keyed
> fingerprints, never site or app names), quiet hours and the invitation
> setting. The key stays on your Macs and in your iCloud Keychain. Velvt can't
> read the copy. Velvt's server can see that you use sync, when the copy
> changes, its approximate size, and when a setup code was made and used. You
> can delete the copy at any time.

**Draft `/privacy` paragraph**

> **What the server can see from sync.** That your account uses sync; when
> the encrypted copy changes, and which of six size steps (16 KB to 512 KB) it
> is in; which of your signed-in devices sent each change (not stored); when a
> setup code was made and claimed, and by which signed-in device; whether a
> reset was requested; and the device list it already keeps (which devices are
> signed in and when each was last seen). It never sees Mac names, categories,
> sites, apps, settings or notification choices. Velvt emails you, through
> Resend, when sync is turned on and when a reset is requested; those emails
> name no device. Deleting your account deletes the copy, and within minutes
> of the deletion removes your account's id from every audit record. Three
> stores are out of the deletion's reach: backups taken before it, kept for up
> to 14 days; the email provider's own delivery records; and application logs,
> which are rotated by size.

**Phase 4 adds:**
- per-device public keys (a random per-account identifier, made only on devices
  that turn sync on);
- device-chain events (when devices join or leave, and which device acted);
- platform;
- relay timing, padded size and push token.

These rewrite consent line 2 (report 4.3).

---

## 13. Threat model

| # | Adversary or event | Attack | Mitigation | Residual |
|---|---|---|---|---|
| T1 | Database read, backup leak, Cloudflare at the TLS edge | Read preferences | Ciphertext only; `K` is never on the server; power-of-two padding (16–512 KiB) leaves six size steps | Write timing and the size step |
| T2 | Malicious or compromised velvt-core | Add a device that can decrypt | v1: the server has no role in key distribution (iCloud Keychain or a typed code). v2: co-signed ADDs verified from a pinned root; pairing that cannot be skipped (§7.6) | None for content, unless the person types a code that did not come from their own screen (T13) |
| T3 | " | Swap in another user's blob | `user_id` in the AAD and the signature | None |
| T4 | " | Roll back the version, or the key | AAD and `put_stmt` bind the version; `highest_seen_version` and its hash survive Log Out; an older version is never taken as current, and is merged into ours without adding any device or rule ours lacks (`merge_older_copy`): after 180-day tombstone collection an old copy would revive removed Macs and deleted rules; a served key that a held Keychain item supersedes or marks retired is `server_inconsistent` (§5.5) | A Mac with no anchor and no superseding item can be shown an old copy, which it merges. No key rollback |
| T5 | " | Serve a fake new key to phish code entry | A key change needs a verifiable rotation or a reset window this Mac could observe; otherwise `server_inconsistent` with no code entry (§6.1). The anchor survives Log Out and removal, so "could observe" is measured from the last fetch this Mac actually made. A key delivered by a typed code always beats the server's word (§7.1 step 7) | A Mac with no successful fetch for the whole 72 h window cannot tell, including one the server refused to serve. The notice says "while this Mac couldn't reach Velvt", and only a code from the person's own screen helps |
| T6 | " | Serve garbage or 404 | Never overwrite; never auto-create; neutral copy; signed repair only after 7 days | Denial of sync |
| T7 | " | Fork or withhold | v1: accepted. v2: witnessing and head attestations | v1 preferences only |
| T8 | " | Substitute a handoff | AEAD under a code-derived key | None |
| T9 | Network attacker | Any | TLS, plus everything above | None |
| T10 | Someone with the account password | Read the vault | Needs `K`: the person's iCloud Keychain or a code on their screen | — |
| T11 | " | Overwrite, delete or replace the vault; DoS by a garbage blob; squat sync before the person turns it on | Writes, deletes and handoffs need the write key. A reset without it waits 72 h, is emailed, alerts every Mac, and can be cancelled by any Mac with `K`. Any sign-out of a Mac it did not ask for (removal, revoke-all, cap eviction, reissue) pushes a pending reset to 72 h after it, so "sign every Mac out, then reset" still leaves those Macs 72 h to sign back in and cancel. A password reset cancels it. A squatted copy is emailed and dated in the J3 panel | If ignored for 72 h, a new empty vault (no confidentiality loss; this Mac keeps its choices) |
| T12 | " | Remove the person's Macs, or sign them out | Removal needs re-authentication; revoke-all, cap eviction and reissue need only the password, and each pushes a pending reset out (T11). Removed Macs post `this_mac_removed` and keep their trust anchor | Nuisance until the password changes |
| T13 | " | Phish the person into typing an attacker's code | Codes carry no meaning outside the person's own screens: "Only type a code shown on one of your own Macs. Velvt never sends codes by email or message." After a reset, code entry is offered only once the reset is verified | Social engineering |
| T14 | Stolen or sold Mac, still in the Apple ID | Keep reading new preferences | Remove it: tokens revoked, the server stops serving it. Copy tells the person to remove it from the Apple ID; A8 rotates the key, and the `supersedes` and `retired_by` markers stop a server from steering a fresh Mac back to the old key | v1: an Apple ID circle member keeps `K`. v2 fixes this with rotation and a fresh recovery secret that v1 devices never held (§7.5), except through the iCloud recovery path, which alerts |
| T15 | Migration Assistant or backup clone | Two Macs share one identity | Host-binding HMAC → new `sync_device_id`, sync off, card | False positive after a logic-board swap: one extra click |
| T16 | Apple and iCloud Keychain | Read `K` | iCloud Keychain is end-to-end encrypted, and joining the circle needs the Apple ID and a device passcode | Trust in Apple's circle. Stated |
| T17 | Another process as the same user | Spoof the socket and obtain `K` or tokens | A0: each end serves only the process the launch relates it to (the helper its parent, the app its child) and checks that process's code signature from the same audit token; hardened runtime with library validation and no `get-task-allow`; data-protection Keychain limited to the team access group | Code running inside the signed app or helper, or administrator rights (out of scope). An ad-hoc local build's requirement is identifier-only and authenticates nothing; there only the pin holds |
| T18 | A buggy or old Mac | Corrupt the document | Strict validation; read-only mode for a newer schema; limits; never overwrite what fails to open | None |
| T19 | Privacy regression | A raw name enters the document | Person keys only; the document's only string is `name`; tracer tests over the ladder | None if the tests hold |
| T20 | Measurement contamination | A synced value changes drift evidence | Three locks; provenance columns; golden replay (§11) | None if the tests hold |
| T21 | Erasure gap | Audit rows, metrics labels or third-party mail outlive the account | C0 deletes sync and device audit rows, anonymizes the rest (request ids included), writes the erasure records without the user id, and folds sync tables into one metric label | Backups taken before the erasure keep pre-erasure rows, audit rows with the user id among them, for up to 14 days. The vault part is ciphertext. Resend keeps its delivery log of the notice emails. A late audit row can hold the id for up to one sweep interval; application logs (some user ids, and access-log paths that name device ids; rotated by size, 5 × 10 MB per service) are out of erasure's reach |
| T22 | (v2) Server substitutes keys during pairing | Get its key into the chain | Commit-then-reveal with two typed codes (10^-6); the QR carries a 256-bit secret | A person who types the code of a session they did not start, during an active attack |
| T23 | (v2) Server gives devices different vault keys | Split the vault | VK commitments in the chain; issuer-signed wraps bound to epoch, recipient and seq | None |
| T24 | (v2) Server hides a revocation | Keep a removed device reading | Monotonic head, witnessing, attestations, fork alerts | A permanent partition |
| T25 | (v2) Photographed QR | Pair a rogue phone, or collect the real phone's tokens | Single use, 10 minutes, one claim per session (a second claim aborts), the name bound into the MAC, a tap on the Mac, tokens only after approval and only to the holder of the claimed signing key; `/pair` runs no script, so the fragment reaches no third party | A careless tap on a sheet for a phone that is not in hand |
| T26 | (v2) Lost phone | Read relay messages | Remove → REVOKE, rotation, push token cleared, nothing sealed to it again | Messages already on it |
| T27 | (v2) Apple or Google push | Read or replay nudges | HPKE-sealed, signed, 512 B padded; generic fallback text; `msg_id` replay cache and `expires_at` (§5.9) | Timing, size, push token (report §5) |
| T28 | Implementation bugs | Any of the above | Small v1 surface; fuzzing; vectors; a malicious-server suite; an external review before any tester | The main residual risk |
| T29 | Supply chain | A malicious crate | v1 adds no crate. v2 uses exact pins, `cargo deny check advisories`, a reviewed lockfile diff | Upstream compromise |

---

## 14. Failure modes

| Failure | Behaviour |
|---|---|
| Provisioning profile missing (`errSecMissingEntitlement`, -34018) | `vault_keys_held{reason: entitlement_missing}`. Sync stays off with "This Velvt build can't use iCloud Keychain". `verify_release.sh` stops such a build shipping |
| iCloud Keychain slow or off | `waiting_for_key`, with the code offered at once |
| Keychain item deleted by the person | `waiting_for_key`, then a code or a reset |
| Two Macs turn sync on at the same moment | `POST /v1/vault` lets one win (409 `vault_exists` for the other). The loser adopts the winner's vault (its key arrives by Keychain or code) and deletes **its own** orphan item `<user_id>.<its key_id>` only |
| 404 after this Mac saw a vault | `missing_on_server`, a verified tombstone (→ off), or a completed reset that §6.1 accepts (→ `reset_completed`). Never auto-create |
| Decrypt or validation failure | `server_copy_unreadable`. Never overwrite; no delete or code prompt; signed repair after 7 days |
| Newer schema | `read_only_newer_schema`. Fetch and apply the known applied fields, never write |
| 412 storm | Three retries with jitter, then a 10-minute backoff |
| velvt-core down or `VELVT_VAULT_ENABLED=false` | Sync pauses silently. Local behaviour is unchanged. The menu status does not turn red for sync |
| Mac offline for weeks | Merges by HLC on return. A rule deleted elsewhere more than 180 days earlier can come back (documented; it is held anyway) |
| Account switch on one Mac | The other account uses its own `vault_state` row; this account's session columns are cleared and its anchor kept. Items are per account |
| Cap eviction by #41 | That Mac disappears from the list and rejoins on its next sign-in. Its entry's `server_device_id` register is updated |
| Old Velvt on another Mac | Unaffected. Listed as "Needs Velvt 1.1 or later to sync" from `client_version` |
| Reset requested by someone else | Every polling Mac posts `reset_requested`. Any Mac with `K` cancels with one click |
| Clock skew | HLC tolerates it, with the overflow rule in §10.3. The server sets the reset window. The client's acceptance check (§6.1) compares it with the client's own clock and last-fetch time, with a 10-minute margin |
| velvt-core restored from a backup (vault versions go backwards) | Each Mac merges the older copy and never takes it as current. The first write goes at `highest_seen_version + 1`, which the server accepts because `new_version` only has to exceed its current version (§8.3). No 412 loop. Changes made after the backup survive on the Macs that made them |
| A device entry for every re-sign-in | Does not happen: `sync_device_id` is part of the kept anchor (J7). Removed entries are dropped after 180 days, and the live cap is 32 |
| Peer check fails (A0) | The connection is dropped and the app shows the helper as unavailable. Sync secrets never cross. Error code only in logs |
| (v2) Chain verification fails | Halt sync only (§6.2): Try Again or Turn Off Sync on This Mac. Everything else works |
| (v2) Keychain or Secure Enclave unavailable | `device_secret_unavailable`. Sync paused with "This Mac's key isn't available". Other devices see the Mac as stale and can remove it |
| (v2) Pairing expired, cancelled or wrong code | Nothing appended; the reason shown; a wrong code also posts the alert |
| (v2) NSE before first unlock, or a stale chain cache | Generic text, or a one-shot chain fetch inside the NSE time budget |

---

## 15. Phased PR plan

Each PR can be merged on its own and changes nothing a tester sees until the
founder flips `SYNC_UI_ENABLED` in a release. Sizes are dev-days with agent
help.

### 15.1 Prerequisites

| Prerequisite | Needed by |
|---|---|
| velvt-core #41 merged and deployed | C1, C2 |
| velvt-core #43 merged | C0, because it edits #43's guard test |
| velvt-core #42 | Independent |
| velvt-app #84 → #86 merged | A4 (protocol 38 follows #86's 37) |
| Founder: the App ID with Keychain Sharing (plus Communication Notifications for #84 route a) and a Developer ID provisioning profile | S0, A4 |
| Founder answers Q1–Q5 (§17) | A4 and later |

### 15.2 Phase 3: Sync v1

| PR | Repo | Scope | Depends on | Off by default via | Size |
|---|---|---|---|---|---|
| **C0** | core | Erasure deletes the account's sync and device audit rows and anonymizes the rest (`request_id` included); the erasure start and completion records carry no user id and a total only; one `sync` metric label; a poll's audit row for an account no longer `active` names no account or Mac, and the worker's erasure sweep re-runs the audit erasure for late writers (§8.6 step 6); #43's guard test asserts no audit row keeps the user id after erasure and the next sweep. `docs/retention.md`, `privacy-contract.md` (pre-erasure backups, Resend) | #43 | — (a privacy fix; founder approval Q12) | 1 |
| **C1** | core | `POST /v1/auth/reauth` + `require_reauth`; `GET /v1/devices`; `DELETE /v1/devices/{id}` for another device with re-authentication; cross-user tests; `docs/api.md` | #41 | New routes with no caller | 1.5 |
| **C2** | core | Vault models, Alembic `…_vault`, `vault_service` (CAS, write-signature verification, create, rotate, signed delete and tombstone, delayed reset with email and sweep, the sign-out push-out (removal, revoke-all, cap eviction, reissue) and the password-reset cancel, handoffs), the account-notice sender generalized from the Resend reset deliverer, `api/v1/vault.py`, security limits counted in Postgres (§8.1), an index on `audit_log_entries.actor_id` in the same Alembic migration (erasure matches `user_id OR actor_id`, and one reviewer estimates about 1M poll audit rows a year per always-on Mac; C0 stays migration-free; not yet in #46's `20261015_0021`), `USER_OWNED_MODELS` and verify, `enforce_retention` sweeps, content-free audit, docs. Tests: CAS, 412, 413, `new_version` bounds, signatures, single-use and expired handoffs, reset window and cancel, the sign-out push-out, password-reset cancel, the reset fields in the 404 body, cross-user isolation, erasure count zero, `make check-migrations` on Postgres 16 | #41, C0, C1 | `VELVT_VAULT_ENABLED=false` | 3.5 |
| **A0** | app | Socket peer checks in both directions (§9.3): the parent/child pin, then the code signature, from one audit token; `--identifier com.velvt.service` in `sign_release.sh`; `verify_release.sh` checks; tests with an unsigned peer and with a peer that execs a signed binary after connecting; Debug bypass | — | Release builds only; protects today's tokens too | 2.5 |
| **S0** | app (spike) | A Developer ID-signed probe with an embedded profile carrying `keychain-access-groups`: a synchronizable data-protection item on two Macs on one Apple ID; propagation time; iCloud Keychain off; profile missing. Outcome: go, or code-only (the item is stored non-synchronizable) | Founder profile | Not merged | 1 |
| **A1** | app (Rust) | `vault::{crypto, doc, merge, code}`: `VaultKey` (redacted `Debug`, zeroize, no `Serialize`), seal and open, HKDF, write key and statements, padding, AAD, the setup code with its check character, schema and validation, the HLC merge with counter overflow. Direct dependencies `ring`, `base64`, `zeroize`, `subtle`. A published test-vector file. `cargo-fuzz` targets for the document decoder, the code parser and the statement parsers (a CI job, not a crate dependency) | Founder Q4 | No callers | 2.5 |
| **A2** | app (Rust) | Provenance migration (§4.3); writes `classification_source` on observations; `synced_rule_evidence` stays 0; the golden-replay harness over decisions, decision log and observations | — | Measurement-neutral; ships only in a registered build | 1.5 |
| **A3** | app (Rust) | Vault migration (§4.3); `vault::sync` (fetch, verify per §6.1, CAS with writer-chosen versions, retries, rollback guard and fork hash, rotation with Keychain markers, tombstone and reset handling); the per-account trust anchor that survives Log Out and removal; settings mirror (held); the routing apply (daily insight); host binding; account, Log Out and switch clearing; `GET /v1/devices` diff seeded without alerts; security notices held until a work block ends; `egress::ENDPOINTS`; the dry-run vault mode; the malicious-server suite; a two-Mac integration test with an in-memory server | A1; C2's contract (a fake server until C2 deploys) | Unreachable: nothing can turn sync on without IPC | 4 |
| **A4** | app (protocol 38, Swift and Rust) | The §9.2 messages; `SyncKeyStore`; entitlements, profile and `verify_release.sh`; `SyncCoordinator`; the Your Macs section; the first-run card; security notices (fourth kind; AGENTS.md); the password relay for re-authentication; `verify_pbxproj_membership.sh` | A0, A3, C1, C2, S0, #86 | `SYNC_UI_ENABLED` off in Release | 3.5 |
| **A5** | app | Setup codes: Rust create and claim; Swift sheets; normalization and typo handling | A4 | Same | 1.5 |
| **A6** | app (Rust) | `vault::mirror`: person keys at labeling time (§5.4) and the ladder backfill; `vault_rule_link`; tombstones from the edit, remove and reset handlers; the tracer privacy test | A3 | Sync off by default; one boolean check | 1.5 |
| **A7** | app (Rust) | The dark apply rung and the held appliers behind the `vault-policy-inputs` feature and `VAULT_POLICY_INPUTS_VERSION = None`; `ClassificationSource::VaultRule` (behind the feature, so not on the wire); the per-Mac toggle's plumbing (not shown); `published_claims` tests; the helper's `--policy-inputs` flag and its `verify_release.sh` check; golden replay with a populated vault | A2, A6 | Compiled out | 1 |
| **A8** | app | "Lost or stolen: change the sync key" (signed rotation); the reset-without-key flow and Cancel Reset; the signed repair; notices | A5, C2 | Same as A4 | 1.5 |
| **D1** | app, core, web, workspace | Everything in §12, plus the security-review packet: this document, the crypto spec, the vectors and code pointers. The packet cites the amended §9.3 (the parent/child pin) and its tests, not the signature-only text | All above | Merges with the release that turns sync on | 1 |
| **R1** | external | Security review of `vault`, A0, `SyncKeyStore` and the server's authorization. Fix the findings. Then the founder sets `SYNC_UI_ENABLED` on in a release | All above | — | 1–2 weeks |

**Totals:** about 6 core days and 21.5 app days (including S0). That is about
3 calendar weeks with core, Rust and Swift work running in parallel, plus the
review.

**Later, separate:** **G1** is the pre-registration amendment, then a build that
turns on `vault-policy-inputs`. That build has a new `DRIFT_POLICY_VERSION`, the
registered const, the per-Mac toggle shown, the export bump, and
CATEGORY_PROMPT_POLICY_VERSION 2 for reminder routing. It is cut between
waves only.

### 15.3 Phase 4: device trust v2 and phones (after R1 and the report's step 4.0)

| PR | Repo | Scope | Depends on | Off via | Size |
|---|---|---|---|---|---|
| T0 | founder | Approve the crates (§17 Q14); the APNs `.p8` key; the iOS App ID; answer report §5's three questions | — | — | — |
| T1 | app | `velvt-trust` crate (pure, no I/O, no build script): chain codec and verifier, wraps, document v2 sealing, the pairing state machines (codes, QR, approve-with-phone, recovery), recovery derivation. RFC 8032, RFC 9180 and Wycheproof vectors; our own golden vectors exported as JSON; proptest; fuzzing; the malicious-server scenarios (§16.2) | T0 | No callers | 10 |
| T2 | core | Registry v2 columns; chain store with compare-and-append and a validator using the shared vectors; the per-epoch write key read from `epoch_write_pk`; bindings; pairing sessions with enrollment tokens, one claim per session and signed token collection; wraps; rotate; cap eviction skips chain members (amends #41); erasure and retention | T1 vectors, #41–#43 | `VELVT_DEVICE_TRUST_ENABLED=false` | 7 |
| T3 | app | Device-seed custody (Swift: data-protection, ThisDeviceOnly, Secure Enclave-sealed where available); the v1 → v2 migration (genesis by the first v2 Mac holding `K`; `VK_1`; a **fresh** iCloud recovery secret `R` if Q15 says keep, never `K` itself; the v1 blob deleted and the v1 item retired, §7.5); the two-typed-codes flow; device list v2; rotation on remove; alerts; IPC bump | T1, T2 | Flag | 10 |
| T4 | core | Alert sender and relay (report 4.1); the content-free insight alert | T2 | `VELVT_APNS_ENABLED=false` | 6 |
| T5 | app | The Rust relay builder from existing copy; routing (the Focus hold first, then one surface per offer); `surface`; inbox replies (report 4.2) | T3, T4, B5 | Flag | 5 |
| T6 | mobile | iOS app: `velvt-trust` through UniFFI; QR pairing; the NSE; actionable categories; receipt and insight viewers; account deletion; demo account (report 4.3) | T3, T4 | TestFlight only | 12 |
| T7 | external | Second security review: `velvt-trust`, pairing, the NSE, the relay, server authorization. A hard gate before any tester pairs a phone | T1–T6 | — | 1–2 weeks |
| T8 | founder | Turn on the 4.4 surfaces. Drift offers to the phone only under a new pre-registered delivery-surface policy (4.5) | T7 | — | — |
| T9, T10 | app, mobile | Bluetooth LE spike (4.6), authenticated with chain keys; Android with Tink HPKE or the crate over JNI, and FCM (4.7) | T7 | — | — |

---

## 16. Test plan

### 16.1 Sync v1

**Cryptography (Rust unit tests)**
- Round trip.
- Tampering with each AAD field fails: user, name, `key_id`, schema, version.
- A wrong key fails.
- Each statement's signature fails when any byte changes.
- `key_id` and the fingerprint are fixed functions of `write_pk`.
- Nonce uniqueness over 10,000 seals.
- Padding buckets.
- HKDF, handoff and statement vectors, published for the reviewer.
- The check character catches every single substitution, and the expected share
  of adjacent swaps, in a sample.
- `Debug` is redacted; zeroize on drop (best effort).

**Merge (property tests)**
- Every permutation of three or four concurrent edits converges.
- Delete versus edit.
- Tombstone garbage collection.
- `person_salt` stability.
- Concurrent edits to separate fields of one device entry.
- A newer schema stays read-only.
- HLC counter overflow keeps string order: the HLC after `…9999…` sorts
  above it.
- Device entries: re-sign-in reuses `sync_device_id`; removed entries age out;
  a writer never exceeds 32 live devices or 2,000 live rules.

**Fuzzing:** `cargo-fuzz` on the document decoder, the code parser, the
statement parsers and the handoff payload opener. The corpora are committed and
replayed on every `cargo test`. As built (A1, `.github/workflows/fuzz.yml`), CI
runs each target for 10 minutes on every push to `develop` that touches the
vault and for 1 minute on such a pull request. The nightly run starts only
once the workflow reaches `main`, because GitHub schedules only from the
default branch.

**Malicious-server suite** (an in-memory server that can lie). Each attack is a
named test.

| Attack | Expected client behaviour |
|---|---|
| Serve another user's blob | Open fails; `server_copy_unreadable`; no overwrite |
| Serve an older blob at its old version | Ignored (rollback guard) |
| Serve an old blob relabelled with a new version | AEAD fails |
| Serve a valid-looking blob with a bad write signature | `server_inconsistent` |
| A new `key_id` with no rotation and no reset window this Mac saw while online | `server_inconsistent`; no code entry offered |
| A new `key_id` with a forged rotation statement | Refused; `server_inconsistent` |
| A forged tombstone | Refused; `server_inconsistent` |
| A 404 while this Mac holds a key | `missing_on_server`; no automatic create |
| Garbage bytes | `server_copy_unreadable`; no delete or code prompt |
| A substituted handoff payload | "That code didn't work."; nothing stored |
| A replayed, claimed handoff | 404 |
| A different blob at the version this Mac last accepted | `server_inconsistent` (fork hash) |
| The pre-rotation copy served to a fresh Mac holding both the old item and the new item marked `supersedes` | `server_inconsistent`; the old key is never adopted |
| The same, with the old item marked `retired_by` | `server_inconsistent` |
| A completed reset shown in a 404 body to a Mac that polled through the window and never saw it pending | `server_inconsistent` |
| The same reset shown to a Mac that logged out and in during the window | `server_inconsistent` (the anchor survived Log Out) |
| A code-delivered key whose `key_id` differs from the served one | The typed key wins; `server_inconsistent` |
| Versions that go backwards (a restored database) | Merged, never taken as current; the next write at `highest_seen_version + 1` succeeds |
| Withheld updates (a fork) | Documented: converges once withholding stops |

**Server (pytest)**
- Every row of §8.3.
- Writes without a valid signature give 403; writes under another `key_id` give
  409.
- The reset window, its cancellation, its completion by the sweep, and the
  email.
- Re-authentication expiry and binding (a token for device A fails for device
  B).
- Cross-user isolation: a device of user A against every route with user B's
  ids.
- Rate limits with `Retry-After`.
- Erasure verification including the new tables and C0's audit rule: after
  erasure no audit row has the user id in `user_id` or `actor_id`, the
  erasure records carry none, and no metric label names a vault table. An
  audit row written after the erasure by a request in flight (an open poll, a
  late reset record) holds no account id once the sweep has run, and a
  poll's row for a non-active account holds none at all.
- `PUT` accepts `new_version` only in `(current, current + 2^32]`, and the
  signature must bind it.
- A removal of another device, a revoke-all, a cap eviction of a signed-in
  device and a reissue of a signed-in device's token each move a pending
  reset's `effective_at`; a self-removal, an eviction of signed-out rows and a
  re-sign-in after Log Out move nothing; a password reset cancels a reset
  requested before it.
- Security limits hold across two API processes and a restart (they are
  counted in Postgres).
- Retention sweeps.
- A structural "no plaintext" test: vault tables hold only ids, integers,
  timestamps and `bytea`.
- Postgres 16 migration check.

**Password-holder scenario (end to end)**
- Remove every Mac, then request a reset: `effective_at` lands 72 h after
  the last removal; a removed Mac that signs back in sees `reset_requested`
  and cancels.
- Request a reset, then sign every Mac out late in the window by
  `POST /v1/auth/revoke-devices`, by registering past the device cap, or by
  reissuing each Mac's token: `effective_at` moves to 72 h after that
  sign-out, and a Mac that signs back in cancels.
- Set up sync on a fresh account before the person does: the account gets
  the email, and the person's first Mac shows the dated J3 panel.
- Register a fresh device with the password.
- Try PUT, handoff create, delete and rotate. Each is refused.
- A reset request becomes pending. Every syncing Mac's next pass posts
  `reset_requested`, and Cancel works.

**Privacy (Rust)**
- Real hostnames, app names and bundle ids flow through the ladder with sync
  on.
- Assert that none of them, and none of their unsalted digests, appears in the
  plaintext document, SQLite outside the existing tables, IPC sync payloads,
  egress bodies or logs.
- The document type's serde has no free-text field but `name`.
- `egress::ENDPOINTS` equals the paths the source builds.

**Measurement:** golden replay and the gate tests (§11.3).

**IPC**
- Schema conformance for every new message, on both sides.
- Version mismatch.
- `vault_key` and `store_vault_key` are refused while the peer check is
  failing (A0).
- A0's pin (I4): a peer that connects and then execs a signed binary is
  refused; a token read before an exec does not validate after it; the app
  refuses a squatter that is not its child.
- Key fields are redacted in `Debug` and `description`.

**Swift**
- `SyncKeyStore` against a fake: the attributes are asserted (data protection,
  synchronizable, access group, account format).
- Log Out never deletes an item.
- Delete Account deletes every held key id.
- Snapshot of Your Macs.
- The banned-copy check over every new string.

**Two-Mac integration test (Rust).** Two `SqlitePersistence` "Macs" against one
fake server:
- turn on → second Mac by key → by code;
- concurrent rule edits;
- rename the other Mac;
- remove a device;
- signed rotation;
- signed deletion;
- reset requested and cancelled;
- account switch;
- Log Out and back in;
- account deleted while the other Mac is offline;
- a Migration Assistant clone, by swapping the host binding;
- a security notice that comes due during a work block, posted only when the
  block ends;
- Delete Synced Data seen by a Mac on another Apple ID: its own key item is
  deleted after the tombstone verifies.

**Manual matrix before review**
- Two Macs on one Apple ID (zero-touch).
- Two Macs on different Apple IDs (code).
- iCloud Keychain off.
- The item deleted in Keychain Access.
- A build with the profile stripped: it must refuse and say why.
- An unsigned process connecting to the socket: refused.
- Delete Account on Mac A while Mac B is offline, then Mac B comes online.
- The egress dry run shows only the document a tester expects.

### 16.2 Phase 4 additions

**Vectors**
- RFC 8032, RFC 9180 Appendix A and Wycheproof (Ed25519, X25519,
  ChaCha20-Poly1305, HKDF).
- Our golden vectors for every format, consumed by velvt-core's validator and by
  a Swift CryptoKit interop test.

**Chain properties**
- Every generated valid chain verifies.
- Every one-byte change, reorder, truncation, removed signature, swapped signer,
  duplicate signer, wrong epoch step or wrong commitment fails.

**Malicious-server scenarios**
- Inject an ADD without a co-signature.
- Substitute keys at each pairing step.
- Wrong codes on each side.
- Replay an old session.
- Serve a forked chain to one device.
- Roll back the head.
- Serve a stale or cross-epoch wrap.
- Invent an epoch.
- Swap the blob across epochs and accounts.
- Withhold a REVOKE for 24 hours.
- Replay a relay envelope inside its TTL (dropped by `msg_id`).
- Claim a QR session twice (contested, nothing added); collect tokens
  without the claimed key's signature (refused); swap `sealed_name` (MAC
  fails).
- A v1 `K` holder tries to act as the recovery member after migration
  (no chain member derives from `K`).
- A wrap whose `VK_e` matches a bare SHA-256 commitment but not the tagged
  one (refused).

Each must be refused or alerted, and no key is ever wrapped to a non-member.

**End to end:** local velvt-core in Docker with two helpers and an iOS
simulator:
- genesis → QR pairing → routing → a relay message → the NSE shows decrypted
  text;
- remove the phone → rotation → the phone cannot open new messages;
- recovery join → alerts;
- erasure → every server table empty and every device wiped.

**On real devices**
- The NSE on a locked iPhone after first unlock.
- The generic text before first unlock.
- The 512 B padding observed at APNs.

---

## 17. Open questions for the founder (with recommended answers)

| # | Question | Recommended answer |
|---|---|---|
| Q1 | Do you approve the new disclosure (§12): opt-in sync stores an encrypted copy of your choices that Velvt cannot read and that holds no site or app names; the server can see that sync is used, when the copy changes, its approximate size, and when codes and resets happen? It touches the consent line, `/privacy`, PRIVACY.md, `published_claims`, the egress registry and the tester kit | **Yes**, with the §12 wording |
| Q2 | Will you create the App ID with Keychain Sharing (and Communication Notifications for #84 route a), and a Developer ID provisioning profile, so both ship in one profile? Is the Apple Developer team an Organization or an Individual? | **Yes, one profile.** Organization if possible (it also sets the iOS seller name) |
| Q3 | Sync off by default with an explicit toggle, or offered as on right after sign-in? | **Off by default.** A one-time card on a second Mac |
| Q4 | Approve `ring`, `base64`, `zeroize` and `subtle` as direct dependencies of `rust-service`? All four are already compiled into today's build through `reqwest` and `rustls` | **Yes.** No new crate enters the lockfile or the build |
| Q5 | Who does the security review, and is it a hard gate before any tester turns sync on? | **External, 1–2 weeks** (for example Radically Open Security, Least Authority, Cure53 or NCC Group). **Hard gate.** A second review before phones (T7) |
| Q6 | May a synced opt-out (invitations off on Mac A) apply at once on other Macs as reduce-only? | **No. Hold it** with the other policy inputs. Reduce-only still changes the invitation rate |
| Q7 | Vault-driven policy inputs: one later amendment that turns on rules, quiet hours, invitations and reminder routing together, behind the per-Mac toggle (off by default), or hold them indefinitely? | **Hold through the current waves.** Then one amendment, cut between waves, with the per-Mac toggle off by default and provenance on every decision |
| Q8 | v1 recovery is iCloud Keychain, a setup code, or "start a new copy" (rules rebuild from each Mac's local rules). Acceptable with no printed recovery key until Phase 4? | **Yes.** Offer an optional printed key in Phase 4 |
| Q9 | v1 removal is not cryptographic: a removed Mac still signed in to your Apple ID keeps the key until it leaves iCloud or is erased. Acceptable, with the copy in J4 and the A8 "change the sync key" option? | **Yes for v1.** Phase 4 rotates on every removal |
| Q10 | Reset without the key: a 72-hour public waiting period plus an email, or 7 days? | **72 hours plus email.** Any Mac with the key can cancel |
| Q11 | Per-Mac routing only in the encrypted vault (server-blind), leaving #43's `notify_here` at its default for Macs? | **Yes.** `notify_here` serves phones' server-originated alerts in Phase 4 |
| Q12 | At erasure, delete the account's sync and device audit rows and anonymize the rest, with a total-only completion record (C0)? | **Yes.** Otherwise "erasure removes everything server-side" is not literally true |
| Q13 | May testers in a pre-registered wave use sync? It changes no policy input while the gate is closed, but it is a new disclosed data flow | **Yes, after R1 and the disclosure.** Do not record sync state on decisions, because it is not an input |
| Q14 | Phase 4: approve `x25519-dalek`, an HPKE crate (`hpke`, or `hpke-rs` with libcrux) and `uniffi`, with `cargo deny` in CI? And should device keys persist across Log Out (a random per-account identifier, made only on devices that turn on sync), which also replaces #41's deferred `install_id`? | **Yes to both.** Log Out keeps membership, with "Also remove this Mac from my devices" as an option |
| Q15 | Phase 4: keep iCloud Keychain as a disclosed join path (a recovery member that alerts every device whenever it is used), or require a pairing ceremony for every new device? | **Keep it, on by default,** with an account switch to turn it off and a "Lost or stolen" prompt to remove the Mac from the Apple ID first |
| Q16 | Should velvt-core delete a vault after 13 months with no device syncing, stated in PRIVACY.md? | **Yes** |
| Q17 | Security notices (new sign-in, a Mac joined, reset requested): respect macOS Focus and DND like every other notification? | **Yes.** Never time-sensitive. Never posted during a work block (they wait for it to end, J6). Exempt only from Velvt's own one-offer-per-block and daily reminder caps |
| Q18 | Email the account on a new device sign-in, as well as on vault creation and a reset request (both required)? All go through Resend, which keeps its own delivery log that erasure cannot reach | **Yes, all three.** No device name in any email; name Resend in `/privacy` |
| Q19 | Report §5, which still gates Phase 4: is the relay's timing disclosure acceptable, or should phones start Bluetooth-only? Should drift offers ever reach the phone? | **The relay as a separate opt-in.** No drift offers to the phone until a pre-registered delivery-surface policy (4.5) |
| Q20 | Accept that v1 does not detect a server that shows two Macs different valid versions (fork or withholding), for preferences only? | **Yes.** v2 adds witnessing |

### 17.1 Decisions raised by the build (2026-10-15)

The build reviews of the first seven PRs raised these. None is decided here.
Where a section of this document depends on one, it says so in place. "Blocks"
names the PR that should not merge (or, where it says so, the step that should
not happen) until the decision is made.

| # | Decision | Options | Recommended | Raised by; blocks |
|---|---|---|---|---|
| D1 | Q12 (§17), now with its residual. C0's single `sync` label on `velvt_erasure_records_deleted_total` still shows one bit: a `sync` increment at the moment of an erasure says the erased account used sync, in a store erasure never reaches (§8.6 step 5) | (a) Yes to Q12, and accept the bit; (b) Yes to Q12, and make that metric total-only (no table label), which removes the bit and the per-table operational count; (c) No to Q12 | **Yes to Q12, with (b).** It applies step 3's total-only rule to the metric too. (Neither reviewer recommended an option for the bit; this is this revision's recommendation) | velvt-core #44; blocks #44 |
| D2 | C1 decision 1: removing a Mac also ends the user session that signed it in (`devices.user_session_refresh_token_id`). Without it, J4's "It will be signed out of Velvt" is false: the removed Mac keeps a 30-day user refresh token | Confirm; or revoke only the device | **Confirm** | velvt-core #45; blocks #45 |
| D3 | C1 decision 2: `DELETE /v1/devices/{id}` naming another device now answers `403 reauth_required`, not `403 device_forbidden`. Neither velvt-app nor the helper references `device_forbidden` | Confirm; or keep the old code | **Confirm** | velvt-core #45; blocks #45 |
| D4 | C1 decision 3: a reauth token can be reused within its 300 s, so one password prompt can remove two lost Macs | Reusable; or single use | **Reusable.** The design does not ask for single use | velvt-core #45; blocks #45 |
| D5 | C1 decision 4: the header is `X-Velvt-Reauth-Token`; the defaults are 120 device-list calls an hour per device and 10 re-authenticated removals an hour per account (§8.4) | Confirm; or change | **Confirm** | velvt-core #45; blocks #45 |
| D6 | C1 decision 5, a change deployed Macs can see: `POST /v1/devices`, `POST /v1/auth/devices/reissue` and `POST /v1/auth/revoke-devices` refuse a user access token whose session has ended, with `401 invalid_token` (§8.2). After a password reset, a Mac whose user access token is under 15 minutes old goes to sign-in instead of reissuing its device silently | Confirm; or revert to accepting the token until it expires | **Confirm.** Before it, a stolen Mac could undo the owner's password reset the same way | velvt-core #45; blocks #45 |
| D7 | Optional, C1's declined nit: removing a leftover row that shares the remover's sign-in also ends the remover's user session, which costs one password prompt at that Mac's next reissue | Keep; or skip the revocation in that case | **Keep.** Skipping it weakens J4 and T14 when a sign-in was copied from another Mac | velvt-core #45; blocks nothing |
| D8 | Per-device request smoothing (§8.4), as built in C2: reads 600 an hour per device; writes 120 an hour per device (`PUT`, rotate, delete, reset cancel); handoff creates 10 an hour per device; and a new row "Refused signatures recorded: 10 an hour per account; past it 429 with no audit row" | (a) Confirm, and §8.4 takes those four rows; (b) per user, as §8.4 says now, where one device of the account, someone else's holding the password among them, can spend the person's Macs' reads until they stop seeing a pending reset | **(a)**, C2's recommendation | velvt-core #46; blocks #46 |
| D9 | `VELVT_VAULT_ENABLED` switched off while a reset is pending. The reset's clock keeps running, and the first request after re-enabling completes a reset that is already due | (a) Keep the operator rule (velvt-core `docs/configuration.md`, "Turning the vault off": do not switch it off while a reset is pending); (b) push pending resets out on re-enabling, which needs a stored re-enable time | **(a) for v1.** The flag has never been on, and the rule is documented. Revisit if the vault ever has to be switched off in production | velvt-core #46; blocks nothing while the flag is off |
| D10 | The setup-code claim budget, counted per account (10 per 10 minutes, failures included). Someone holding the password can keep spending it, so the person's new Mac cannot claim a code (J3) until the password changes. A visible nuisance, not a bypass | (a) Keep it per account; (b) per device, which loosens the guess limit | **(a)** | velvt-core #46; blocks nothing |
| D11 | C2 decision 12: the three account-notice email texts in velvt-core `app/services/account_notice.py` (sync turned on; reset requested, with the time it takes effect; reset cancelled by a password reset) | Approve; or edit | **Approve after reading them.** Each names no device, carries nothing from the vault and avoids the banned words | velvt-core #46; blocks turning the vault on |
| D12 | §17 Q4: `ring`, `base64`, `zeroize` and `subtle` as direct dependencies of `rust-service`. `Cargo.lock` gains no package | Approve; or reject | **Approve** (§17 Q4) | velvt-app #89; blocks #89, and #91, which carries it |
| D13 | The issuer tag. A1 ended the rotate and delete statements with `issuer_tag = HMAC-SHA256(K_issuer, sync_device_id)[0..16]`, `K_issuer = HKDF-SHA256(K, info="velvt-vault-issuer-v1")`, so J6 and J9 could name the Mac to holders of `K`. C2 refuses those statements, and A3 removed the tag (commit `413cd0b`) | (a) No tag in v1: §5.3 stays as it is, and the copy names no Mac (J5, J6, J9 and §6.1 as in this revision); #89 takes `413cd0b`, or merges first and lets #91 bring it. (b) Approve: §5.2 adds `K_issuer` and `issuer_tag`; §5.3's rotate row becomes `"velvt-vault-rotate-v1\0" ‖ lp(user_id) ‖ old_key_id ‖ new_key_id ‖ new_write_pk ‖ u64_be(new_version) ‖ issuer_tag(16 B)` and its delete row `"velvt-vault-delete-v1\0" ‖ lp(user_id) ‖ key_id ‖ u64_be(at_version) ‖ i64_be(requested_at) ‖ issuer_tag(16 B)`; C2 parses the longer statements and A3 restores the tag; J6 and J9 name the Mac, with "another of your Macs" when the tag matches no Mac in the last opened copy | **(a) for v1**, A3's recommendation, and the contract C2 and A3 already share. A1 recommended (b) | velvt-app #89; blocks #89, whose statements disagree with C2 until one of the two lands |
| D14 | Optional: clamp how far an observed far-future HLC can move a Mac's clock | Clamp; or no clamp | **No clamp in v1.** A1 declined it: clamping breaks §4.1's rule that a Mac's own write beats everything it has seen, and a stall needs a key holder whose clock was within a day of 2286-11-20 | velvt-app #89; blocks nothing |
| D15 | #88 Q1: an ad-hoc local build (`make package-release`, signed with `-`) requires only `identifier "com.velvt.mac"` / `identifier "com.velvt.service"`, which authenticates nothing; there only the parent/child pin protects the socket, and `verify_release.sh --mode local` warns | Accept; or refuse every peer in such a build, which makes local Release builds unusable | **Accept** | velvt-app #88; blocks #88 |
| D16 | #88 Q2: on the first dial, a refused peer gets the same orphan reclaim as a version mismatch (at most twice, no alert). A same-version orphan is now replaced instead of used | Keep; or drop | **Keep** | velvt-app #88; blocks #88 |
| D17 | #88 Q3: a Release helper refuses other local clients: `scripts/measure_first_value.py`, `scripts/measure_warm_reopen.py` and the workspace's Claude Code attention hook. They still work against a Debug or `cargo build` helper | Accept; or give them a path | **Accept.** If the hook is ever switched on against a tester build, it needs its own path | velvt-app #88; blocks #88 |
| D18 | #88 Q4: PRIVACY.md, under "The socket": "In a release build the app talks only to the service it started, the service only to the app that started it, and each checks the other's code signature before anything crosses the socket. Another program running as you therefore can't pose as the service to the app or receive anything from the service over it. This does not stand up to a program running with administrator rights, or to one that has already got its own code running inside Velvt." | Approve; or drop it until a build with A0 ships | **Approve.** It describes A0 as built, and the earlier wording, which rested on the signature alone, was false. (#88 made no recommendation; this is this revision's) | velvt-app #88; blocks #88 |
| D19 | #88 Q5: A0 uses about 200 lines of hand-written FFI, not the `security-framework` crate | FFI, no crate; or the crate, which needs crate approval | **FFI, no crate** | velvt-app #88; blocks nothing |
| D20 | Which build first carries migration 0045 (A2) | (a) A build cut between waves; (b) any build, since every decision and the export are shown unchanged | **(a)**, as §15.2 already says ("ships only in a registered build"). (b) is defensible: the golden replay and the byte-identical export show 0045 changes nothing measured | velvt-app #90; blocks the release that carries #90, and #91, whose 0046–0047 follow it |
| D21 | A2's option (a), the meaning of `classification_source`: a report the open row absorbs sets the row's source by precedence `vault_rule` > `user_rule` > the opener's rung, so A7 can count `synced_rule_evidence` from the observation rows (§4.3, §11.1 lock 3, as built) | (a) As built; (b) rows record only the opener, and A7 counts from reports at decision time | **(a)** | velvt-app #90; blocks #90 |
| D22 | I6 against §8.6 and J10: on a server-reported `account_deleted`, do other Macs delete their Keychain key items? A3 follows I6 | (a) I6, as built. §8.6's Mac side becomes "Other Macs delete the account's `vault_state` row, `vault_rule_link`, `sync_known_device` and `sync_notice`, and keep their Keychain items (I6); items are deleted only by this Mac's own Delete Account", and J10's "They clear their sync state and delete the same items" changes the same way. (b) Keep §8.6 and J10, and amend I6 to allow it | **(a).** `AuthManager` gives that answer to every request once it has been told, whichever account the request was for, and deleting a synchronizable item deletes it from every Mac on the Apple ID. A stray `K` opens nothing once the account is erased | velvt-app #91; blocks #91 |

---

## 18. Changelog and adversarial review

### Changelog

**2026-10-15, revision 2: the build reviews.** The seven built PRs (velvt-core
#44–#46, velvt-app #88–#91) were read against this document. Each correction
below was checked in the PR's code before it was applied, and makes this
document say what the code does.
- **Erasure (C0, #44):** late writers and the erasure sweep (§8.6 step 6 and
  the guard sentence), G5, T21's residual, the §16.1 erasure test, application
  logs (§8.5, §12), and "within minutes" in the §12 `/privacy` draft. The
  §15.2 C2 row gains an index on `audit_log_entries.actor_id` (not yet in
  #46).
- **Auth (C1, #45):** `require_user_session` refuses a session that has ended
  (§8.2); the removal limit and the order it depends on (§8.4).
- **Vault store (C2, #46):** velvt-core `docs/api.md` "Personal Vault" is the
  normative wire contract (§8.3), and §4.2, §5.3, §8.1 and §8.3 now match it.
  Every sign-out a Mac did not ask for pushes a pending reset out (J9, §8.3,
  T11, T12, §15.2, §16.1).
- **Socket (A0, #88):** the parent/child pin and why it is needed (§9.3, I4,
  T17, §16.1, the D1 row).
- **Cryptography (A1, #89):** `merge_older_copy` (§6.1, T4), the merge caps
  (§4.1), the create and rotation puts (§8.3), the example code `…8CJ5H` (J3,
  §5.3), the fingerprint and byte encodings (§5.2, §5.3), the fuzz schedule
  (§16.1).
- **Provenance (A2, #90):** what an absorbed report records (§4.3, §11.1
  lock 3); the second golden-replay trace A7 needs (§11.3).
- **Sync engine (A3, #91):** the trust roots with no anchor and the
  first-sight test for a pending reset (§6.1), the states that accept a typed
  key (§7.1 step 7), `sync_notice` (§4.3, §8.6), and copy that names no Mac
  where nothing signed says which Mac acted (J5, J6, J9, §6.1).
- **Decisions:** the 22 choices the builds raised are in §17.1. Sections that
  depend on an open one (D8, D13, D21, D22) say so in place.

**2026-10-15, revision 1: the adversarial review.** Findings F1–F30, below.

### Adversarial review, 2026-10-15

Each finding was checked against velvt-app `origin/develop` @ 51ce105 and
velvt-core `origin/develop` @ 6135547 before it was fixed here.

| # | Finding | Class | Resolved in |
|---|---|---|---|
| F1 | During the 30 days a rotated key's item lives on, a malicious server could serve the pre-rotation copy to a fresh Mac on the same Apple ID. That Mac would adopt the old key, which the lost Mac still holds | Key rollback | §5.5 `supersedes` / `retired_by`, §5.6 steps 3 and 6, §6.1, T4, T14 |
| F2 | Log Out, Turn Off Sync and removal cleared `vault_state`. A Mac that signed out for five minutes lost its rollback guard and its reset bookkeeping, so it could be shown a fake reset as if it had been offline for 72 h. Every re-sign-in also added a new device entry, and after 32 the document failed validation on every Mac | Rollback, availability | J7, §4.3 (per-account anchor), §4.1 device caps, §14 |
| F3 | Someone with the password could remove every Mac (they stop polling and cannot cancel) and then request a reset. A person's own password reset signed out every Mac without cancelling the reset | Password holder | J9, §8.3 (removal push-out, password-reset cancel), T11, T12 |
| F4 | A database restore makes versions go backwards. Each Mac ignored the restored copy, then wrote with `If-Match` of its own higher version and got 412 forever | Availability, buildability | §8.3 `PUT` `new_version`, §5.3, §6.1 merge rule, §10.4, §14 |
| F5 | A completed reset left a 404 whose body had no reset fields, so returning Macs showed `missing_on_server`, and the "never create while an item exists" rule then blocked the new copy | Logic | §8.3 404 body, §6.1 `check_reset`, §5.5 create rule, §10.4 |
| F6 | Delete Synced Data deleted one key id, so retired items blocked a later Turn On. A Mac on another Apple ID kept its item forever. "Old key retired" on an unverifiable reset could be read as deleting a synchronizable item, which would remove the key from every Mac | Erasure, destructive action | §5.5 deletion list, §6.1 |
| F7 | A different blob at the same version (a fork) passed the `≥` check | Rollback | §4.3 `highest_seen_sha256`, §6.1 |
| F8 | A code-delivered key that disagreed with the server's `key_id` had no defined outcome | Server trust | §7.1 step 7, T5 |
| F9 | 4 KiB padding showed the rule count to within about 37 rules, and was described as hiding it | Metadata | §5.3, T1, §12 |
| F10 | v2 reused v1's `K` as the iCloud recovery secret, so every v1 device, including a lost code-paired Mac, could join v2 as the recovery member | Crypto | §7.5, §15.3 T3, T14 |
| F11 | The server-checked per-epoch write key was not in any signed chain entry | Crypto | §6.2 `epoch_write_pk`, §5.9 |
| F12 | §5.9 checked a bare `SHA-256(VK_e)` against a commitment that §6.2 defines with a tag, `vault_id` and epoch | Crypto | §5.9 |
| F13 | v2 readers had no rule against taking an older-epoch blob as current | Rollback | §5.9 |
| F14 | Relay envelopes had no replay check inside their TTL | Crypto | §5.9, T27 |
| F15 | QR pairing: `sealed_name` was outside the MAC (the server could swap the name in the approval sheet); a photographer of the QR could race for the phone's device tokens or claim first; `/pair` could leak the fragment, with `k` in it, to analytics | Pairing | §7.2 steps 2, 4 and 9, T25 |
| F16 | The two-code transcript did not say it covers the chain head B pins | Pairing | §7.4 step 5 |
| F17 | A v2 reset (new genesis) had no defined handling on devices that pinned the old root | Logic | §6.2 |
| F18 | Erasure: `account.erasure_completed` is written after anonymization with `user_id` set; `request_id` was not nulled; a per-table metric showed sync use; backups were said to hold "only ciphertext"; Resend's delivery log was not disclosed | Erasure, privacy promise | §8.6 C0 steps 2–5, §8.5, G5, T21, §12 |
| F19 | Security notices could post during a work block, and a click on one is an app switch the drift gate counts | Measurement | J6, §11, §11.3, Q17 |
| F20 | Quiet hours were described as setting `do_not_disturb_until` on drift offers. In code they drop the offer to `Quiet` salience (`ipc/router.rs`), hold invitations, the digest and the reminder, and defer the daily insight (`delivery/poll.rs`) | Measurement accuracy | §10.1, §11 |
| F21 | A `published_claims` test cannot see which features a release was built with | Buildability | §11.1 lock 1 (`--policy-inputs`, `verify_release.sh`) |
| F22 | `ClassificationSource` crosses the socket, so `VaultRule` changes the IPC vocabulary | Buildability | §11.2 |
| F23 | `ring::constant_time` is deprecated in 0.17.14 and fails `clippy -D warnings` | Buildability | §5.1 (`ring::hmac::verify`, `subtle`), Q4 |
| F24 | `security-framework` is in `Cargo.lock` only through the optional `onnx` feature, so it is not "no new crate" for the default build | Buildability | §9.3 |
| F25 | Per-day security limits relied on `InMemoryRateLimiter`, which is per process and forgets on deploy | Buildability | §8.1 |
| F26 | Reset and creation email assumed a generic mail sender; the code has a reset-link-only Resend deliverer | Buildability | §8.3 "Account email", C2 |
| F27 | Turning sync on would post `new_sign_in` for every existing device | UX | §10.4 |
| F28 | HLC counter overflow broke string ordering | Logic | §10.3 |
| F29 | A higher schema was to be read as "apply known fields" under `deny_unknown_fields`; a writer could exceed the 2,000-rule cap and make the copy unreadable everywhere | Logic | §4.1 |
| F30 | A password holder could set up sync before the person did, leaving the person waiting for a key that never arrives | Password holder | J3, §8.3 create email, T11 |
