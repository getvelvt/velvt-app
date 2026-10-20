# Vault v1 sync engine

> **Status:** built in `rust-service/src/vault/sync` (PR A3), and unreachable.
> Nothing in the shipped path constructs a `SyncEngine`: turning sync on, the
> Keychain messages and the device list are protocol 38 (PR A4). Until then no
> vault request is sent and no `vault_state` row is written. The design is
> `docs/architecture/personal-vault-and-devices.md` (PR #87): §4.3, §5.5–§5.7,
> §6.1, §10, §14 and §15.2 row A3. Byte formats are in
> [vault-v1-formats.md](vault-v1-formats.md).

## Modules

| Module | What it does |
|---|---|
| `api` | velvt-core's `/v1/vault` routes and `GET /v1/devices`, parsed strictly from C2's (#46) and C1's (#45) response bodies |
| `verify` | Design §6.1 as pure functions: the vault judgement, `check_reset`, the rotation path, the copy's signature and fork checks |
| `engine` | The state machine of §10.4 over `verify`: the pass, the write, create, the account events, the snapshot |
| `keys` | The Keychain behind the `VaultKeyStore` trait (Swift's `SyncKeyStore` is A4); `MemoryKeyStore` for tests |
| `mirror` | What this Mac writes into the document, and the one register it applies |
| `devices` | The new-sign-in diff over `GET /v1/devices`, and the device list's rows |
| `notices` | The security notices' copy, and the queue that holds them during a work block |
| `host` | The host binding (J8) |
| `preview` | The plaintext document for `--dry-run-egress --vault` |
| `schedule` | The triggers: every 10 minutes, 5 s after the last local change, and on wake or a request when the last pass is over a minute old |

## The row: anchor and session

`vault_state` (migration 0046) has one row per account that has turned sync on
on this Mac.

| Part | Columns | Cleared by |
|---|---|---|
| Trust anchor | `sync_device_id`, `host_binding_salt`, `host_binding`, `key_id`, `write_pk`, `highest_seen_version`, `highest_seen_sha256`, `fetched_at`, `seen_reset_request_id` | Only a verified signed deletion and account deletion (`account_deleted`, below) remove the row. A reset this Mac accepted replaces the trust columns when it adopts or creates the new vault |
| This Mac's own create | `pending_create_key_id` | Set (and saved) before the create's Keychain item is made, so before `POST /v1/vault`; kept while no answer came or the create was refused. Cleared when this Mac trusts the vault made under that key, when the vault is seen under another key (the create lost the race and its item is deleted), and when the item is gone. Log Out, Turn Off and the other session events keep it |
| Session | `enabled`, `server_version`, `ciphertext`, `last_hlc`, `state`, `last_error` | Log Out, Turn Off Sync on This Mac, `403 device_revoked`, and another account signing in |

`ciphertext` is the copy at `highest_seen_version`, still sealed. The
plaintext document exists only in memory. `pending_create_key_id` is how a
restart (an update, a reboot, Log Out and back in) still knows a kept item
for this Mac's own create: the first pass of a session loads that item from
the Keychain again, and the create is sent again under it. Without the
record, the item would count as a live item of unknown origin, and every
pass and every Turn On would stop in `missing_on_server` (or, after a reset,
`reset_completed`) without ever sending the create. `vault_rule_link` and
`sync_known_device` belong to whichever session is open and are cleared with
it. `sync_notice` holds the security notices owed to the signed-in account
(below). Times are Unix milliseconds.

A pass saves the row as soon as it has recorded what the fetch showed
(`fetched_at`, `seen_reset_request_id`), before it writes anything to
velvt-core. If the row cannot be saved, the pass stops in `error`
(`state_not_saved`): a Mac that cannot keep its anchor does not act on it.

The anchor is why a Mac that signed out for five minutes is not as easy to
fool as one offline for three days (F2): `fetched_at` and
`seen_reset_request_id` survive, so `check_reset` still knows this Mac was
fetching during a reset window.

## A pass

1. Ask the Keychain what it holds for the account (key ids and their
   `supersedes` and `retired_by` markers, never key bytes).
2. `GET /v1/vault`, and judge it (`verify::judge_vault`).
3. On the trusted key: `K` into memory, then `GET /v1/vault/blobs/prefs` with
   `If-None-Match`, judged (`verify::judge_blob`) and opened.
4. Write this Mac's registers (below) and, if anything changed, a
   compare-and-swap `PUT`.
5. `GET /v1/devices`: the new-sign-in diff.

Step 5 runs on every pass while sync is on here, whatever steps 1 to 4 did:
with the copy current, read-only on a newer schema, waiting for a key or the
rotated key, paused (`server_inconsistent`, `server_copy_unreadable`,
`error`, or the vault switched off), missing on the server or reset, and
while a 412 storm or a `429` holds the vault's requests back. So no state
hides a `new_sign_in`. The first list after sync turns on (or after a new
session) seeds `sync_known_device` without alerting, as before. Only a
session that has ended lists nothing: sync off here, this Mac removed
(`403 device_revoked`), the account deleted (`401 account_deleted`), or a
request refused for want of a sign-in. Nor does a state that is off: `off`
itself, and `error` on a build without the Keychain entitlement, where §14
says sync stays off. A pass that finds no vault and was not asked to make one
(a Turn On that failed before creating, for example with the Keychain
unavailable) ends the session as Turn Off does, so "Sync is off on this
Mac." means no request at all until the person turns sync on again. Either of the first two answers to
`GET /v1/devices` itself ends the session as it would on a vault request; any
other failure leaves the list for the next pass and changes neither the
copy's state nor the backoff.

## What a Mac believes (§6.1)

What this Mac trusts is its anchor or, with no anchor yet, every key its
Keychain holds live (the *roots*; the engine loads each item to derive its
public key, since an item carries none). A Mac on the same Apple ID that holds
the vault's key and has never synced is therefore checked against that key,
like an anchored Mac. Only a Mac that trusts nothing at all, with no anchor
and no live item, may wait for a key and be offered a code.

| The server says | This Mac does |
|---|---|
| No anchor, and the served key is one this Mac holds live | Adopts it once `K` gives its write key |
| No anchor, nothing held, and a vault | `waiting_for_key`, where a code is offered |
| The trusted key, with its write key | Fetches the copy |
| The trusted key id with another write key | `server_inconsistent` |
| A key an item this Mac holds supersedes or marks retired | `server_inconsistent` (a rollback past a rotation, F1); the key is never adopted |
| A new key, with a chain of stored rotations from the trusted key, each signed by the key before it and the key it introduces | If the new key is in the Keychain: adopt it, mark each old item `retired_by`, post `sync_key_changed`. If not: `waiting_for_rotated_key`, where a code is safe |
| A new key with no verified chain and no reset this Mac can accept | `server_inconsistent`, and no code entry is offered |
| No vault, with a deletion of the trusted vault signed by its key (on a 404, and on a 200 when a new vault was made since, as C2 sends it) | The row and this Mac's own item for that key go; sync is off with "Sync was turned off from another of your Macs." |
| No vault, with a forged deletion | `server_inconsistent`; nothing deleted |
| A pending reset this Mac has not recorded | Recorded as seen only if its window is at least 72 h and it was requested after this Mac's last fetch (10-minute margin). Otherwise the server hid it or backdated it: `server_inconsistent`, not recorded, no `reset_requested` notice. A server cannot show one once and complete it minutes later (T5, T11). The reset's key is not compared: velvt-core keeps the key named when it was requested and names the vault's key when it completes |
| A completed reset of the trusted key | `check_reset`: accepted if the window was at least 72 h and has passed, and this Mac saw it pending or made no fetch after it was requested (10-minute clock margin); otherwise `server_inconsistent` |
| No vault, while this Mac holds a live item or has seen a version | `missing_on_server`; never creates |
| No vault and nothing else | Creates, only when the person turns sync on. On any other pass, sync is off here: the session ends as at Turn Off |
| A copy whose write signature fails, or that names another key | `server_inconsistent` |
| A different copy at the version this Mac last accepted | `server_inconsistent` (a fork) |
| A copy below the highest version seen | Merged with `merge_older_copy` into the copy this Mac last accepted, never cached; the next write goes at `highest_seen_version + 1`, which heals a restored database (F4). The sealed copy in the row is opened first if this run has none, also when the Keychain could not give the key at launch |
| A copy that does not open or validate | `server_copy_unreadable`; never overwritten, no code or delete prompt |
| A higher schema | `read_only_newer_schema`: names and the daily-insight switch are read, nothing is written |

A copy the server made without the key cannot carry a valid write signature,
so another account's copy, a relabelled version and server-made garbage all
end in `server_inconsistent` before the copy is opened. `server_copy_unreadable`
is what a copy a key holder signed but that does not open gets: a buggy Mac's
write, or another account's copy signed with this account's key.

A key delivered by a typed setup code (§7.1 step 7, `adopt_delivered_key`;
A5 claims and opens the code):
- is taken only in a state that offers a code (`waiting_for_key`,
  `waiting_for_rotated_key`, `reset_completed`) or before sync is on here. In
  any other state, `server_inconsistent` above all, it changes nothing and
  nothing is sent: whether a code may be entered is Rust's judgement (I3);
- wins over the server: if the served vault has another key, this Mac trusts
  the typed key (an anchor already on it keeps its rollback guard), stores
  nothing, and pauses;
- for the served vault's own key, is adopted only as a pass would adopt it:
  the key already trusted, nothing trusted yet, a rotation the trusted key
  signed, or a new vault after a reset §6.1 accepts or a signed deletion.
  Otherwise it is `server_inconsistent`, and nothing is stored;
- is stored in the Keychain only then, before the trust changes. A rotated
  key is stored with `supersedes` and the old item is marked `retired_by`,
  so a Mac on another Apple ID that takes the rotated key by code leaves the
  same markers there as a Mac that found it in its Keychain (T14). When trust
  moves to another vault, the old vault's document is dropped first, so only
  this Mac's own registers enter the new one.

## Writes

- The document written is the last opened copy merged with this Mac's
  registers: its own entry (a new entry once; after that its server device id
  if it changed, and `removed: false` if it rejoins), its rule links, and its
  quiet hours and invitations switch when set after the document's register
  was written and different from it.
- `PUT` with `If-Match: "n"` and `new_version = max(n, highest_seen_version) + 1`.
- `412`: fetch, merge, try again, three times in all with jitter, then wait 10
  minutes. The wait holds back the vault's requests only; the device list is
  still fetched. `409 vault_key_mismatch` and `404`: the next pass judges the
  vault.
- After a stored write, a rule link's tombstone row is deleted only if the
  stored copy carries it (or a newer register for the rule, or it is past the
  tombstone horizon). A rule deleted while the write was in flight has a newer
  clock value and waits for the next write.
- Create: `K`, `W` and `person_salt`; the Keychain first, then `POST /v1/vault`
  (§5.5). Never while a live item is held, except the item a verified reset
  names. `409 vault_exists`: `GET /v1/vault` says whose vault it is. Under
  another key (two Macs at once): delete this Mac's own orphan item, and only
  that, and wait for the winner's key. Under this Mac's key (its earlier
  create landed after the answer was lost): keep the item and open the vault.
  No answer at all: keep the item, and send the create again under the same
  key if the next pass still finds no vault and no other item. If the next
  pass finds a vault under another key, the item is the lost race's orphan and
  goes then.
- A create velvt-core refuses with any other answer it parses (C2's `413
  vault_too_large`, `422 vault_blob_invalid`, `vault_key_id_invalid` or
  `vault_signature_invalid`, `409 vault_reset_pending`), or whose first copy
  cannot be built, sealed or signed, keeps its item the same way. None of
  these says a vault exists under another key, so the item lost no race and
  is no orphan, and nothing else on §5.5's list applies; deleting it would
  take the key from every Mac on the Apple ID. The state is `error`
  (`vault_create_refused`, `document_write_refused` or `signing_failed`), and
  the next pass sends the create again under the same key. After a completed
  reset, that item does not block [Set Up Sync] as another live item would:
  it is this create's own key, and the create goes again under it.
- Past 2,000 live rules (§4.1), the rules that do not fit stay in this Mac's
  own tables and are not written: `mirror_rule_links` returns them, and a
  merge's `rules_over_cap` names this Mac's additions it gave up.
  `SyncSnapshot::rules_over_cap` is set, and in `syncing` and `reset_pending`
  the status line ends "Your synced copy is full at 2,000 categories, so new
  ones from this Mac stay on this Mac." Neither names a rule. The flag is
  worked out again on every write, so it goes once there is room.
- Edits (rename, the daily-insight switch) go into the in-memory document and
  are written on the next pass, 5 s after the last one. A restart inside that
  window drops them.

## Held and applied (§10.1, §11)

Applied: Mac names, to the device list, and this Mac's own
`notify_daily_insight`, through `SyncRouting`, which
`delivery::poll::PollScheduler::with_insight_route` reads. With it off, the
insight still reaches the popover and no notification is posted.

Held: category rules, quiet hours and the invitations switch are written into
the document and never applied from it. Nothing in `vault::sync` writes a
local policy table, and a test syncs a copy whose quiet hours, invitations
switch and rules differ and checks every local policy table is unchanged.

## Security notices (J6)

`new_sign_in`, `mac_joined_sync`, `this_mac_removed`, `sync_key_changed`,
`reset_requested`, `reset_completed` and `server_inconsistent`, with the
design's copy. The copy names no Mac where no signed bytes say which one:
neither a rotation nor a deletion statement carries the signing Mac (see the
formats page), and velvt-core does not say which Mac removed this one.

`take_due_notices` returns nothing while a work block is active: clicking a
notification is an app switch the drift gate counts. The device list shows
every notice at once. Each notice is kept in `sync_notice` when it is queued,
with when it was posted and acknowledged, so it is posted once across
restarts, a notice a block is holding when the service restarts is still
posted when the block ends, and it is listed until acknowledged. The row holds
ids and times only. The copy is written when the notice is shown, and a joined
Mac's name is read then from the synced copy ("A Mac" if none is open). Rows
go at Log Out, when another account signs in, with the account's row, and 30
days after they were acknowledged.

## Account events

| Event | What happens |
|---|---|
| `session_started` (launch, sign-in, account switch) | Every other account's session ends; the host binding is checked; a session left on resumes from the sealed cache |
| `log_out` | Session columns, session tables and notices cleared, `K` dropped; the anchor and every Keychain item stay |
| `403 device_revoked` | The same, keeping the notices, and `this_mac_removed` |
| `401 account_deleted` on a vault request | The session ends as at Log Out; the anchor and every Keychain item stay. `AuthManager` gives that answer to every request once told, whichever account's session the request was for, so it alone does not say this account is gone |
| `account_deleted(user, ThisMac)` (this Mac's own Delete Account) | Every key item held for the account deleted, retired ones too, and the row |
| `account_deleted(user, Elsewhere)` (the auth layer saw the account deleted) | The row with its anchor, the session tables and the notices go. Every Keychain item stays: no server claim alone deletes one (I6), and a stray `K` opens nothing once the account's data is erased (§8.6). Design §8.6 and J10 ask for the items to be deleted here too; that tension is a founder question |
| `turn_off` | This Mac's entry marked `removed` and written, then the session ends; anchor and item stay |
| Host binding mismatch (J8) | A new `sync_device_id`, the session ends, `clone_detected`. A binding made while the host UUID was unreadable is bound for real on the first launch that reads it |

## The Keychain trait

`VaultKeyStore` answers `inventory`, `load`, `store` (never overwrites),
`retire` and `delete`. The engine never deletes an item on a server's word
alone: only on this Mac's own Delete Account, a verified deletion (its own item
for that key), a lost create race (its own orphan, once the vault is seen under
another key, and never the item of the vault this Mac trusts: once this Mac
trusts the vault its create made, the create is settled), and 30 days after
the item that superseded it was made. That last
rule needs each item's `created_at`, which A4's `vault_keys_held` must report
beside `supersedes` and `retired_by`.

## velvt-core (C2)

`tests/vault_sync/fake_core.rs` is an in-memory velvt-core written from C2's
`vault_service.py`: the error envelope, the reset and tombstone fields in a
404's `error.details`, `412 vault_version_conflict {current_version}`, the six
sealed sizes, every signature over statements the server builds itself, and
the reset that completes on the first request after it is due. Before A3 the
client and C2 disagreed on three things, and A3's first commit changes the
client to C2's, which is the design's §5.3: the plaintext is padded to the six
sizes (sealed copies are 28 bytes longer), the rotate and delete statements
carry no issuer tag, and times inside statements are whole seconds. Every
published vector was checked against C2's own `vault_formats.py`.

## Egress

The five requests are in `egress::ENDPOINTS` and `PRIVACY.md`, marked as sent
only once sync is on: `GET /v1/vault`, `GET /v1/vault/blobs/prefs`,
`PUT /v1/vault/blobs/prefs`, `POST /v1/vault` and `GET /v1/devices`. The one
exception is J2's probe (`probe_available`): with sync off, `GET /v1/vault`
once after sign-in, and only if the Keychain holds a live key for the
account, since only then can the card show. Each request goes through the
client the engine is given, which in the service is `AuthManager` over
`ReqwestHttpClient`, so each is ledgered before it is sent. Each names the
session's account (`HttpRequest::account`), so `AuthManager` refuses to send
it under another account's session: a pass still running when the person
switches accounts never reads the other account's vault into this anchor. `PUT` and the
conditional headers are new to `HttpRequest` (`Precondition`). Migration 0047
rebuilds `egress_ledger` so its `method` CHECK takes `PUT`: 0038's allowed only
GET, POST, PATCH and DELETE, and `ReqwestHttpClient` sends nothing the ledger
cannot record, so every vault write would have failed closed. The rows are
copied unchanged, with the index and triggers made again.

`velvt-service --dry-run-egress --vault` prints, after the usual report, the
document this Mac would add to the copy, built from its own tables by the
engine's own functions, with `person_salt`, its device id and its name shown
as placeholders.

## What A4 wires

| Protocol 38 | Engine |
|---|---|
| sign-in, `auth_session`, account switch | `session_started(user, device_id)` |
| `set_sync_enabled {enabled: true, default_name}` | `turn_on(default_name)` |
| `set_sync_enabled {enabled: false}` | `turn_off()` |
| `request_sync_state` | `snapshot()`, and `SyncTriggers::nudge` |
| `vault_keys_held`, `vault_key`, `vault_key_missing`, `vault_key_stored`, `vault_key_retired` | An IPC `VaultKeyStore` |
| `rename_device`, `set_notification_routing` | `rename_device`, `set_daily_insight`, then `SyncTriggers::local_change` |
| `acknowledge_sync_notice` | `acknowledge_notice` |
| `sync_notice` | `take_due_notices` when a block ends and after each pass |
| `log_out` | `log_out` |
| Delete Account on this Mac | `account_deleted(user, AccountDeletion::ThisMac)` |
| `AuthState::AccountDeleted` (`SessionWatch`, for the account it saw deleted) | `account_deleted(user, AccountDeletion::Elsewhere)` |
| the J2 card | `probe_available` |

The service spawns `schedule::run` and attaches `SyncRouting` to the poll
scheduler.

## Tests

- Unit tests in each module, including `verify`'s every judgement.
- `tests/vault_sync`: the malicious-server suite, one named test per row of
  §16.1's table except the two setup-code rows (A5), and the two-Mac
  integration test (turn on, join by Keychain and by code, concurrent edits,
  rename, routing applied, held values, removal, signed rotation, signed
  deletion, reset requested and cancelled, a reset accepted and sync set up
  again, account switch, Log Out and back in, account deleted while offline,
  Migration Assistant, a notice held during a work block, Turn Off, restart,
  no Keychain, the vault switched off, a create race, and the privacy check
  over every request body and stored row).
- The PR #91 review's attacks, each a named test: a fresh Mac holding the key
  served a key it did not sign; a backdated pending reset; a code typed while
  paused; a rotated key taken by code, then the pre-rotation copy served on
  that Apple ID; a code for the trusted key while another is served; an older
  copy after a launch without the Keychain; a pass during an account switch;
  `401 account_deleted`; an anchor that cannot be saved; a notice held across
  a restart; a rule deleted while a write is in flight; the J2 probe with no
  key here; and a create whose answer was lost.
- `tests/vault_sync/sign_ins.rs`: a sign-in is noticed (`new_sign_in` once,
  the state unchanged) in each state a pass can leave the copy in: read-only
  on a newer schema, paused on a key change, unreadable, the Keychain
  unavailable, the vault switched off, waiting for the key, waiting for the
  rotated key, missing on the server, reset completed, and writes backing off
  after three 412s (with no vault request sent); and a Mac removed from the
  account, or with sync turned off, lists no devices.
- A refused create (`422`, `413`) and a create whose copy cannot be sealed
  keep their new item and send the create again under its key, also after a
  completed reset; and 2,001 rule links leave one waiting, with the status
  line saying so until a deletion makes room.
- The 2026-10-15 review: a refused create is sent again after a restart (and
  a second Mac on the Apple ID then joins), after Log Out and back in, and
  after a reset and a restart; a recorded create whose item is gone is
  forgotten; and a create that got no answer and lost the race across a
  restart deletes only its own orphan and waits for the winner's key. After a reset, [Set Up Sync Again] whose answer was lost and
  the pass that adopts its vault: a served key no Mac signed deletes no item
  and the copy opens again after a restart, a signed rotation retires the
  old item rather than deleting it, and a later reset makes no vault on a
  plain pass. A Mac left off by a failed Turn On, and a build without the
  entitlement, send nothing and post no `new_sign_in`.
