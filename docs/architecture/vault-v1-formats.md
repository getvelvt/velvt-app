# Vault v1 wire formats

> **Status:** built in `rust-service/src/vault` (PR A1). Nothing in the shipped
> path calls it, nothing it does leaves the Mac, and it adds no table, IPC
> message or endpoint. The design is `docs/architecture/personal-vault-and-devices.md`
> (PR #87), §5.1–§5.4 and §10.3. This page records the formats byte for byte,
> as built, for velvt-core's validator (PR C2), the security review (R1) and any
> second implementation. Where the design left a detail open, the choice made
> here is listed in [Decisions the design left open](#decisions-the-design-left-open).

**Test vectors:** [`vault-v1-test-vectors.json`](vault-v1-test-vectors.json).
Every value in it is reproducible from the fixed inputs it states (Ed25519
signatures are deterministic). `cargo test vault::vectors` fails if the file and
the code disagree by one byte. The vectors were also re-derived independently
with Python `cryptography` (OpenSSL) from the design text alone.

## Primitives

All from `ring` 0.17.14, which the default build already compiled in through
rustls.

| Need | Primitive | `ring` API |
|---|---|---|
| AEAD | ChaCha20-Poly1305 (RFC 8439), random 96-bit nonce | `aead::LessSafeKey` with `CHACHA20_POLY1305` |
| KDF | HKDF-SHA256 (RFC 5869), empty salt | `hkdf::Salt::new(HKDF_SHA256, &[])` |
| Signatures | Ed25519 (RFC 8032) from a 32-byte seed | `Ed25519KeyPair::from_seed_unchecked`; verify with `UnparsedPublicKey` |
| MAC | HMAC-SHA256 | `hmac` |
| Hash | SHA-256 | `digest::SHA256` |
| RNG | the OS CSPRNG | `rand::SystemRandom` |

Secret buffers are `zeroize::Zeroizing` and are wiped on drop. Secret
comparisons use `subtle::ConstantTimeEq`. `VaultKey`, `SetupCode` and
`HandoffContents` have redacted `Debug` and no `Serialize`; a test fails to
compile if one gains it. `person_salt` serializes, but only inside the
document, which is written into one wiped buffer allocated at the largest
document size and never grown. A newer-schema document is parsed without ever
holding its `person_salt` or rules.

Inside the crate as well as outside it: `W` signs only the six statements
(raw signing is private to `vault::crypto`); `K`'s bytes, HKDF and the AEAD are
visible only inside `vault`; and sealing with a chosen nonce exists only in
test builds.

## Encoding rules

- A signed or authenticated structure is a NUL-terminated tag, then big-endian
  integers and fixed-length arrays.
- The only variable-length fields are `user_id` and the blob `name`, each as
  `lp(x) = u16_be(len(x)) ‖ x`.
- `key_id` is 8 raw bytes inside signed and authenticated bytes, and 16
  lowercase hex characters in JSON and in the database.
- A parser rejects a wrong tag, a short input and any trailing byte.
- `user_id` is 1–64 bytes of printable ASCII (velvt-core's ids are UUID
  strings). `name` is 1–32 bytes of `[a-z0-9_]`; v1 uses only `prefs`.

## Keys

```
K          32 random bytes
K_doc    = HKDF-SHA256(ikm = K, salt = ∅, info = "velvt-vault-doc-v1",   L = 32)
W_seed   = HKDF-SHA256(ikm = K, salt = ∅, info = "velvt-vault-write-v1", L = 32)
write_pk = the Ed25519 public key of seed W_seed
key_id   = SHA-256("velvt-vault-key-id-v1\0" ‖ write_pk)[0..8]
fingerprint = u64_be(SHA-256("velvt-vault-fpr-v1\0" ‖ write_pk)[0..8]) mod 10^12,
              shown as twelve digits in three groups: "4821 9133 0577"
K_issuer = HKDF-SHA256(ikm = K, salt = ∅, info = "velvt-vault-issuer-v1", L = 32)
issuer_tag(sync_device_id) = HMAC-SHA256(K_issuer, sync_device_id (16 raw bytes))[0..16]
```

`K` travels as `key_b64`: standard base64, padded. An `issuer_tag` names a Mac
to holders of `K` only: a Mac finds the signer of a rotation or deletion by
computing the tag for each device in its last opened document (no match: the
copy names no Mac).

## The sealed document

```
plain  = u32_be(len(json)) ‖ json ‖ 0x00…
nonce  = 12 random bytes
aad    = "velvt-vault-doc-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id ‖ u16_be(schema) ‖ u64_be(version)
sealed = nonce ‖ ChaCha20-Poly1305(K_doc, nonce, aad, plain)      (the 16-byte tag last)
```

- `len(sealed)` is exactly one of 16, 32, 64, 128, 256 or 512 KiB: the
  smallest that holds `json` plus 32 bytes (the prefix, the nonce and the tag).
  So `len(plain) = len(sealed) − 28`.
- The longest JSON is 524,256 bytes. A larger document is refused by the writer
  (`TooLarge`) and by every reader.
- Opening refuses any other length before decrypting, refuses a nonzero padding
  byte or a length prefix past the end, and requires the decrypted document's
  `schema` to equal the `schema` in the AAD.

## Signed statements

Each is signed with Ed25519 under `W`. The stored form, which velvt-core keeps
and serves back (`vaults.rotations`, `vault_tombstones.statement`), is the
statement's bytes followed by its signature, or for a rotation by the old key's
signature and then the new key's.

| Statement | Bytes | Signed by | Checks besides the signature |
|---|---|---|---|
| put | `"velvt-vault-put-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id ‖ u16_be(schema) ‖ u64_be(base_version) ‖ u64_be(new_version) ‖ SHA-256(sealed)` | `W` | the account is this one; `key_id` is the trusted key's |
| create | `"velvt-vault-create-v1\0" ‖ lp(user_id) ‖ key_id ‖ write_pk ‖ SHA-256(sealed)` | the new `W` | `key_id` is the one `write_pk` derives |
| rotate | `"velvt-vault-rotate-v1\0" ‖ lp(user_id) ‖ old_key_id ‖ new_key_id ‖ new_write_pk ‖ u64_be(new_version) ‖ issuer_tag` | old `W`, then new `W` | `old_key_id` is the trusted key's; `new_key_id` is the one `new_write_pk` derives and differs from `old_key_id` |
| delete | `"velvt-vault-delete-v1\0" ‖ lp(user_id) ‖ key_id ‖ u64_be(at_version) ‖ i64_be(requested_at_ms) ‖ issuer_tag` | `W` | `key_id` is the trusted key's |
| handoff | `"velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id ‖ SHA-256(payload) ‖ i64_be(expires_at_ms)` | `W` | the account is this one |
| cancel reset | `"velvt-vault-reset-cancel-v1\0" ‖ lp(user_id) ‖ key_id ‖ reset_request_id` | `W` | `key_id` is the trusted key's |

Times are Unix milliseconds. `reset_request_id` is velvt-core's UUID as its 16
raw bytes. `issuer_tag` is the signing Mac's 16-byte tag (see [Keys](#keys)):
under the old `K` in a rotation, which every Mac that verifies it holds, and
under the current `K` in a deletion. It is what lets J6 (`sync_key_changed`)
and J9 ("Sync was turned off from “Studio Mac”") name the Mac from signed
bytes, after the document is gone or unreadable. velvt-core treats it as
opaque. No tag is a prefix of another. With a 36-character `user_id`, a stored
rotation is 260 bytes and a stored deletion 164, inside the 512- and 256-byte
columns of design §4.2.

### What velvt-core checks against the request (C2)

A signature proves only that `W` signed some bytes. velvt-core must also tie
those bytes to the request they arrive with, or a valid statement could be
replayed onto another request. It **rebuilds** the put, create and handoff
statements from the request and never accepts statement bytes for them. It
**parses** the rotate, delete and cancel-reset statements that arrive as bytes
(`*_stmt_b64`), strictly, and requires each field below to equal its request
counterpart. In every case `user_id` is the authenticated account's id, never a
request field.

| Endpoint | Statement | Built or checked from |
|---|---|---|
| `POST /v1/vault` | create | built: `key_id` and `write_pk` from the body (`key_id` must equal `f(write_pk)`, else 422), `SHA-256(base64-decoded blob.ciphertext_b64)`; verified under the body's `write_pk` |
| `POST /v1/vault` | put (`put_sig_b64`) | built: `name` = `blob.name` (`prefs`), the body's `key_id`, `blob.schema`, `base_version` = 0, `new_version` = 1, the same SHA-256; verified under the body's `write_pk`. The blob is sealed with version 1 in its AAD |
| `PUT /v1/vault/blobs/{name}` | put | built: `name` = the path's `{name}`; `key_id` from the body, which must equal `vaults.key_id` (else 409 `vault_key_mismatch`); `schema` from the body; `base_version` = the `If-Match` version, which must be the current version (else 412); `new_version` from the body, with `base_version < new_version ≤ base_version + 2^32`; the SHA-256 of the uploaded bytes. Verified under `vaults.write_pk` |
| `POST /v1/vault/rotate` | rotate | parsed from `rotate_stmt_b64`: `old_key_id` = the body's `old_key_id` = `vaults.key_id` (else 409); `new_key_id` = the body's `new_key_id` = `f(new_write_pk)` ≠ `old_key_id`; `new_write_pk` = the body's `new_write_pk_b64`; `new_version` = `blob.new_version`. `old_sig_b64` verified under `vaults.write_pk`, `new_sig_b64` under `new_write_pk`. Stored in `vaults.rotations` as statement ‖ old signature ‖ new signature |
| `POST /v1/vault/rotate` | put (`blob.put_sig_b64`) | built: `name` = `blob.name`, `key_id` = `new_key_id`, `blob.schema`, `base_version` = the current version `n` at the moment the rotation is applied, `new_version` = `blob.new_version` (`n < new_version ≤ n + 2^32`), the SHA-256 of `blob.ciphertext_b64`. Verified under `new_write_pk`. The blob is sealed under `K'` with `key_id'` and `new_version` in its AAD |
| `DELETE /v1/vault` | delete | parsed from `delete_stmt_b64`: `key_id` = the body's `key_id` = `vaults.key_id` (else 409); `at_version` = the current blob version; `requested_at_ms` is not compared with the server's clock. Verified under `vaults.write_pk`; stored as the tombstone (statement ‖ signature) |
| `POST /v1/vault/handoffs` | handoff | built: `handoff_id` = the 16 bytes of the body's 32-hex `handoff_id`, `SHA-256(base64-decoded payload_b64)`, `expires_at_ms` = the body's `expires_at` in Unix milliseconds (at most 600 s ahead). Verified under `vaults.write_pk` |
| `DELETE /v1/vault/reset-requests/{id}` | cancel reset | parsed from `cancel_stmt_b64`: `reset_request_id` = the path's `{id}`, which must be this account's pending request; `key_id` = `vaults.key_id` = the request's `key_id`. Verified under `vaults.write_pk` |

The vectors have one of each put: `put` (an ordinary `PUT`, base 6 to 7),
`create_put` (base 0 to 1, over the version-1 blob the `create` statement also
hashes) and `rotate_put` (under the next key, base 7 to 8, the rotation's
`new_version`).

## The setup code and the handoff payload

- `s` is 100 random bits. It is shown as 20 Crockford base32 characters
  (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`), most significant first, then one Luhn mod
  32 check character, grouped 4-4-4-4-5: `K7PQ-M2XD-9HTW-4RZN-8CJ5H`. (The
  design's J3 and §5.3 show `…8CJ5X`, whose check character is wrong; a test
  pins the example here.)
- Reading what was typed: case is ignored; `O` reads as `0`, `I` and `L` as
  `1`; ASCII hyphens, the Unicode dashes U+2010–U+2015 and U+2212, and white
  space are ignored; `U` and anything else is refused. The check character
  catches every single wrong character and every swap of neighbours except `0`
  with `Z` (tested over every pair of characters at every position).
- `s` enters HKDF as 13 bytes, big-endian, top four bits zero:

```
handoff_id ‖ K_wrap = HKDF-SHA256(ikm = s, salt = ∅, info = "velvt-vault-handoff-v1", L = 48)
aad     = "velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id
plain   = 0x01 ‖ key_id ‖ K ‖ issuer_sync_device_id (16 raw bytes)          (57 bytes)
payload = nonce ‖ ChaCha20-Poly1305(K_wrap, nonce, aad, plain)                (85 bytes)
```

Opening requires exactly 85 bytes, version `0x01`, a `key_id` equal to the id
of the `K` inside, and an issuer that is a version-4 UUID. `handoff_id` is 32
lowercase hex characters on the wire.

## Person keys

```
person_key(kind, digest) = HMAC-SHA256(person_salt, "velvt:person-key:v1" ‖ u8(kind) ‖ digest)
kind: 1 site, 2 bundle, 3 app name
```

`digest` is the unsalted 32-byte digest `abstraction::key` computes; the
function never sees a raw site, bundle id or app name. A rule key in the
document is `site:`, `bundle:` or `app:` and the person key in 64 lowercase hex
characters. Nothing in A1 computes one from real input; that is PR A6.

## The `prefs` document, schema 1

The JSON of design §4.1, serialized compactly (no white space outside
strings), with each object's fields in a fixed order and the two maps sorted.
The vector's `json` shows it:

- the document: `schema`, `person_salt`, `devices`, `rules`, `settings`;
- a device entry: `joined_via`, `joined_at_ms`, `server_device_id`, `name`,
  `notify_daily_insight`, `removed`; a register: `value`, `hlc`;
- a rule: `category`, `deleted`, `hlc`;
- `settings`: `invitations_enabled`, then `quiet_hours`, each absent until
  written; a quiet-hours value: `start_local_minutes`, `end_local_minutes`,
  `rule_version`;
- `devices` sorted by `sync_device_id` and `rules` by rule key, both as
  strings (byte order).

A reader does not depend on this order. A second writer must follow it only to
reproduce the vector's `sealed_b64` byte for byte, since signatures cover the
sealed bytes, not the JSON. A schema-1 document is read strictly:

- every object rejects unknown fields, and the `devices` and `rules` objects
  reject a repeated key;
- ids are canonical lowercase UUIDs (sync device ids must be version 4);
- an HLC is `<13 digits>.<4 digits>.<sync_device_id>`, the milliseconds
  without a leading zero;
- `category` is one of the eight `mvp-2` categories a local rule can hold;
- `joined_via` is `created`, `icloud_keychain` or `code`;
- a name is 1–64 characters with no control character, line or paragraph
  separator, or bidirectional formatting character;
- quiet-hours minutes are 0–1439 and `rule_version` is at least 1;
- at most 32 live devices, 256 device entries and 2,000 live rules.

`settings.invitations_enabled` and `settings.quiet_hours` are absent until a Mac
writes one, and a quiet-hours `value` of `null` means quiet hours are off.

A higher schema is read only for each device's `name` and
`notify_daily_insight` register; anything else, and any of those two that does
not validate, is ignored. It decodes to a separate type with no encoder, so it
cannot be written back.

**Size, measured.** A rule costs about 187 bytes of JSON, not the 110 that
design §4.1 estimates, and a device entry with a 64-character name about 565
bytes. So 2,000 live rules take about 366 KiB, and the caps alone do not keep a
document under the 524,256-byte limit: 2,000 live rules with 1,000 tombstones,
or with 256 device entries whose names use three-byte characters, pass it. The
writer refuses such a document rather than write it. How the mirror should stay
under the limit is a question for PR A6 (see the PR).

## HLC and merge

- HLCs compare as strings. Every part has a fixed width, so the string order is
  the order of (milliseconds, counter, device).
- A Mac issues `max(now, highest seen)` and a counter: the counter goes up when
  the milliseconds do not move, and starts at 0 when they do. When the next
  counter would be 10000, the milliseconds move on by one and the counter starts
  at 0, so a Mac's own writes keep winning.
- A merge takes, per register, the one with the higher HLC. Equal HLCs with
  different values (only in a damaged copy) resolve to the larger value, so the
  merge stays commutative. "Larger" is: `false` < `true`; names by UTF-8 byte
  order; server device ids by their 16 bytes (the same as string order);
  quiet hours `null` < any value, values by `start_local_minutes`, then
  `end_local_minutes`, then `rule_version`. A rule is one register over
  `{category, deleted}`, ranked by `(hlc, category, deleted)` with the
  categories in this order: `FOCUS_WORK`, `PASSIVE_CONSUMPTION`, `SOCIAL_FEED`,
  `COMMUNICATION`, `TASK_MANAGEMENT`, `REFERENCE`, `SYSTEM`, `UNLOGGED`, and
  `deleted: false` < `true`.
- `joined_via` and `joined_at_ms` are written once by the Mac itself; if two
  copies disagree, the earlier join is kept.
- `person_salt`: the server copy's wins. If this Mac's copy has another salt,
  its rules are dropped from the merge (their person keys were made under the
  other salt) and the merge reports it, so the caller can clear its rule links.
- On write, a deleted rule or a removed device entry whose register is more
  than 180 days old is dropped.
- **Caps.** The merged copy never exceeds 2,000 live rules, 32 live Macs or
  256 device entries. Past one, the merge gives up this Mac's additions (entries
  live only because of this Mac's copy), from the highest key down, and keeps
  the server's register for each, or no entry; it reports what it gave up
  (`rules_over_cap`, `devices_over_cap`). The server's copy passed validation,
  so the result always does. Within the caps the merge is symmetric.
- **An older copy** (a version below `highest_seen_version`, §6.1) is merged by
  `merge_older_copy`, which never adds a device or rule the Mac's current
  document lacks. After tombstone collection an old copy holds removed Macs and
  deleted rules as they were before; a server replaying one, validly signed
  under the current key, would otherwise revive them on every Mac.

## Decisions the design left open

Each is a choice made here that velvt-core (C2) and the reviewer should see.

| Detail | Choice | Why |
|---|---|---|
| What the six sizes measure | The sealed bytes (nonce and tag included), not the padded plaintext | A 512 KiB plaintext seals to 512 KiB + 28 bytes, past velvt-core's 512 KiB limit |
| How `s` enters HKDF | 13 bytes, big-endian, top four bits zero | The design names 100 bits but no byte encoding |
| Time fields in statements | Unix milliseconds | Matches `joined_at_ms` and the HLC |
| `reset_request_id` in the cancel statement | 16 raw UUID bytes | The encoding rules prefer fixed-length arrays |
| The stored form of a signed statement | Statement, then its signature(s) | The design stores "signed statements" but gives no layout |
| Fingerprint digits | First 8 hash bytes, big-endian, mod 10^12 | A bias below 10^-7 per value |
| HKDF salt | Empty, as RFC 5869 §2.2 defines it (32 zero bytes) | The design writes `HKDF(K, info=…)` |
| Name characters | Also no bidirectional formatting characters or U+2028/U+2029 | A name is quoted inside security notices |
| Equal HLCs, different values | The larger value wins, in the orders under [HLC and merge](#hlc-and-merge) | Keeps the merge commutative for any input |
| Quiet hours turned off | A register whose value is `null` | A clear has to propagate like any other write |
| Naming the Mac that rotated or deleted | `issuer_tag`, a 16-byte HMAC of its `sync_device_id` under a key from `K`, at the end of the rotate and delete statements | J6 and J9 name that Mac, and after a deletion or an unadoptable rotation only signed bytes remain. A plain `sync_device_id` would give velvt-core a device id that survives sign-out (design §4.2 stores none); the tag means nothing without `K`, and changes with it |
| The put statement of `POST /v1/vault` | `base_version` 0, `new_version` 1, the new `key_id`, signed by the new `W`; the blob's AAD has version 1 | §8.3 returns version 1 but did not say what the put statement binds |
| The put statement of the rotation's blob | `key_id'`, `base_version` = the current version `n`, `new_version` = the rotate statement's `new_version`, signed by `W'`; the blob is sealed under `K'` | §5.6 step 4 seals under `K'` but did not say which key's put signs it or what its versions are. velvt-core refuses a `base_version` other than its current version as it refuses a stale `If-Match` |
| JSON field order | Struct fields in declaration order, the maps sorted | Signatures cover the sealed bytes, so readers ignore order; it matters only to reproduce a vector |
| A merge past a cap | This Mac's additions wait, highest key first | §4.1: "a writer never produces a document that fails validation"; otherwise two Macs each adding the 2,000th rule leave a copy no Mac can write |
| A served copy older than the newest seen | Merged without adding any entry | §6.1 merges it; after tombstone collection a replayed copy would otherwise revive removed Macs and deleted rules |

## Tests and fuzzing

- `cargo test vault` runs the design's §16.1 cryptography and merge tests,
  including every permutation of three and four Macs' concurrent edits.
- `rust-service/fuzz` holds four cargo-fuzz targets: the document decoder, the
  setup-code parser, the statement parsers and the handoff payload opener. It is
  its own workspace and no crate depends on it.
- `tests/vault_fuzz_corpus.rs` replays every committed corpus input on each
  `cargo test`, with no nightly toolchain.
- The statements target also verifies whatever parses (rotations, deletions,
  cancels, handoffs and puts) against the vectors' key and a key taken from the
  input; the document target also merges each decoded document with itself and
  with an empty copy, both ways, and ticks a clock that has observed it.
- `.github/workflows/fuzz.yml` runs each target with libFuzzer: 10 minutes each
  on every push to `develop` that touches the vault, the fuzz targets or the
  lockfile, and nightly against `develop` once the workflow is on `main`
  (GitHub schedules only from the default branch); 1 minute each on a pull
  request that touches them. A failing run keeps the crashing inputs as an
  artifact. Locally: `make fuzz-vault`, which writes new inputs to a gitignored
  scratch corpus and only reads the committed one.
