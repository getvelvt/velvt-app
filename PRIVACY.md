# Privacy

Velvt is designed so that raw, identifying activity data never leaves your
device. This document is the canonical description of what is collected,
what is stored, what is transmitted, and how to audit or delete it
yourself. See [`PRIVACY_AUDIT.md`](PRIVACY_AUDIT.md) for the code-level
verification behind these claims.

## What is collected

The macOS client observes, locally, via the Accessibility APIs:

- Which application is currently focused (`NSWorkspace.didActivateApplicationNotification`)
- The focused window's title (`kAXFocusedWindowChangedNotification`, `kAXTitleChangedNotification`)
- The focused document URL, when the focused application is a browser that
  exposes one over the Accessibility APIs
- Timestamps for these events

That list is exhaustive. Velvt does not use screen recording, keylogging,
the microphone, or the camera, and never will — the macOS app only requests
the Accessibility and Notifications permissions.

The focused document URL is reduced to a **hostname and nothing else** the
moment it reaches the Rust service, by `focused_site_context`
(`rust-service/src/abstraction/browser.rs`). Paths, query strings, fragments,
credentials, and ports are discarded; `file://` URLs are rejected outright, so
a local file path is never reduced to anything at all. The raw URL is
destructured out of the event in `AbstractionEngine::process` and is never read
again.

## What stays local

Raw application names, bundle identifiers, window titles, URLs, file paths,
filenames, and any text drawn from a window title never leave the device.
They exist transiently in the Swift collection layer and are forwarded
once, over a local Unix domain socket, to the Rust service running on the
same machine. The Rust service is the privacy enforcement boundary: its
abstraction engine consumes raw events and produces only an
`AbstractedEvent` — a stable local ID, an on-device classification label,
a category, a taxonomy version, and a timestamp. Some on-device labels and
display names can identify the classified application so the local UI remains
useful. The upload DTO collapses every such label to a fixed category-scoped
cloud vocabulary (for example, `communication:inferred`) before serialization.
The Rust type system makes it structurally impossible for a raw field to reach
the network: `AbstractedEvent` and the upload DTO
(`BatchEventPayload`/`BatchPayload`) have no field that could hold one.

The SQLite file on disk is a weaker claim, and it is made separately below
rather than folded into this one. Several of its columns deliberately hold an
application name, a digest of an application's bundle identifier, the metadata an
application publishes about itself, or text you typed, and two hold a sketch
derived from a window title. Every one of them is named in the table in the next
section.

An optional work-block intention also stays on the Mac. It crosses only the
local Unix socket, is stored in the protected SQLite file for at most 24 hours,
and is never added to upload JSON, cloud/cache payloads, logs, telemetry,
crash-safe diagnostics, or notification identifiers. Safe work-block results
are local-only aggregates. See
[`docs/architecture/work-block-loop.md`](docs/architecture/work-block-loop.md)
for the exact per-field boundary.

## What is stored locally, and for how long

All persistence lives in a SQLite database at
`~/.velvt/velvt-service.sqlite3` (configurable via `VELVT_DATABASE_PATH`):

| Table | Contents | Default retention |
|---|---|---|
| `abstraction_map` | stable-key hash → stable ID, label, category, taxonomy version, and `display_name` — the activity name you typed when you renamed a classification. The key is an HMAC under this install's `stable_key_salt` (migration 0037), described below | a mapping is swept once 14 days pass with no further observation of its window, unless a correction you made or an event still in `raw_event_buffer` points at it — so a window you corrected keeps its mapping until you undo that correction. `display_name` also goes when you undo that correction, or with Reset Corrections, which nulls the column on every row |
| `raw_event_buffer` | abstracted event metadata for short-lived audit/replay and the local 14-day activity chart, plus six device-local columns that can name or identify an application, all described below (`app_stable_id`, `local_display_label`, `local_name_suggestion`, `app_bundle_stable_id`, `declared_app_category`, `document_type_ids`) | 14 days (`VELVT_RAW_EVENT_TTL_HOURS`) |
| `upload_batch` / `batch_event` | privacy-safe events grouped into upload batches | sent batches: 30 days; rejected batches: 7 days (audit window); pending and failed batches: 30 days, the same horizon as sent |
| `personal_override` | one correction you made to a single window: the stable-key hash, the category you chose, and `activity_name`, the name you typed for it | until you undo that correction or use Reset Corrections. No sweep expires it |
| `personal_app_override` | the same correction applied to a whole application rather than one window: app-key hash, category, `activity_name`, and since migration 0034 `bundle_key_hash` — the same bundle-identifier digest described under `raw_event_buffer` below, NULL on every rule taught before that migration | until you undo the correction it came from or use Reset Corrections. No sweep expires it |
| `semantic_embedding_cache` | one hashed sketch per application-and-title pair the classifier has scored, keyed by a hash of the pair. The sketch is derived from the raw application name and the raw window title, and individual words are partially recoverable from it — described below | the 512 most recently observed pairs; a pair is swept once 14 days pass with no further observation of it. The clock restarts on every observation, so a window you keep returning to is never swept |
| `personal_semantic_prototype` | a copy of that same sketch, kept for a category you corrected so the classifier can recognise the activity again | the 64 most-corrected pairs, at most 12 per category; removed by undoing that correction or by Reset Corrections. No sweep expires it |
| `history_cache` / `insight_cache` | ready-to-display summaries fetched from the cloud | minutes to tens of minutes, per `VELVT_HISTORY_TTL_SECONDS`/`VELVT_INSIGHT_TTL_SECONDS` |
| `work_block` | local state and optional free-form intention | intention: 24 hours; the safe state until Clear Local Work Blocks |
| `work_block_observation` | safe category/status/confidence spans only | removed with its block, and by Clear Local Work Blocks |
| `work_block_result` | safe local duration, transition, recovery, coverage, evidence, observation, and one next action | removed with its block, and by Clear Local Work Blocks |
| `intervention_decision_log` | every moment the drift policy was evaluated and what it decided, including the times it decided to stay silent: broad anchor category, switch count, elapsed and remaining seconds, and the verdict. No label, no app identity, no window title, no URL, no intention text | removed with the block it belongs to, and by Clear Local Work Blocks, which also deletes any row logged without a block |
| `out_of_block_run` | activity outside a declared work block, as broad category plus coarse time: start floored to a five-minute bucket, duration, local hour, and local date. Structurally cannot hold an application name, a label, a stable ID, a window title, or a URL — it is deliberately less informative than the 14-day `raw_event_buffer` it would be derived from. **Nothing writes it today** — see the note below the table | 90 days once written |
| `block_antecedent` | the bounded window of activity immediately before a block started, recorded once and never updated: the *set* of broad categories present (not their order), a switch count, a dominant category and its dwell, weekday/weekend, and hour bucket. The window is capped at 30 minutes by the database schema, so it cannot be widened by a setting. **Nothing writes it today** — see the note below the table | removed with its block, and by Clear Local Work Blocks |
| `antecedent_finding` | a discovered pattern about what precedes a block: a key from a closed compile-time registry (a time bin, a day type, a broad category, a coarse elapsed bucket), an effect size, and a q-value. The registry has no constructor that could mint an application name, a label, a window title, or a URL. **Nothing writes it today** — see the note below the table | no sweep, no cascade, and no in-app action removes a row — deleting `~/.velvt/` is the only removal. Empty today |

Three of those tables are empty on every install, and their rows above say so
rather than describing a store that exists only in the schema. `out_of_block_run`,
`block_antecedent`, and `antecedent_finding` were created by migrations 0027,
0028, and 0029; the retention sweep for `out_of_block_run` is registered and
runs. No shipped code path constructs a row for any of the three. The only
callers of their write methods are tests. They are listed here because the
tables exist on your disk and you will see them if you open the file, and
because the retention figures above are what will apply if a writer lands —
not a description of data being collected today.

`raw_event_buffer` holds no window titles and no URLs. It does hold five
device-local columns that can name or identify an application, and all five are
disclosed here rather than hidden behind the table's name. Two have been there
since the buffer existed; three were added by migration 0033, and this list was
corrected on 2026-09-24 to name them:

- **`local_display_label`** — a friendly display string for the local UI, such
  as `Coding`, `Gmail`, or `YouTube`. Derived, not raw, but it can identify a
  service.
- **`local_name_suggestion`** — the **raw application name**, retained only when
  the classifier did not match a seed rule or one of your own corrections, so
  the app can offer you a one-tap rename instead of showing `Unclassified`.
  Generated by `responsible_local_name_suggestion`
  (`rust-service/src/abstraction/engine.rs`), which returns nothing for
  seed-matched or user-corrected apps and nothing for generic names.
- **`app_bundle_stable_id`** — a SHA-256 digest of the application's **bundle
  identifier** (`com.microsoft.VSCode` and the like), computed under its own
  domain separator so it cannot collide with the name hash beside it. The column
  holds the digest, never the identifier — but the digest is not a secret. Since
  migration 0037 it is keyed with this install's `stable_key_salt`, so it differs
  from Mac to Mac and a list hashed from public sources matches nothing. The salt
  sits in the same file, though, and the set of macOS bundle identifiers is
  small, public and enumerable, so anyone holding the whole file can hash a list
  of known identifiers under it and read off which applications you ran. Treat
  this column as naming the application. What it cannot reveal is anything about a document: no window
  title, no URL, no file name, no path, and nothing you typed. NULL when macOS
  reported no bundle identifier, and on every row written by a client older than
  protocol 30.
- **`declared_app_category`** — the raw `LSApplicationCategoryType` string out of
  the application's own `Info.plist`, for example
  `public.app-category.developer-tools`. Developer-authored public metadata from
  a closed Apple vocabulary, not text you wrote and not derived from your
  activity: it is identical on every Mac with that application installed. It
  reveals what kind of application it is, and alongside the digest above it
  narrows which one. NULL when the key is absent or the plist could not be read.
- **`document_type_ids`** — the file types the application *says* it can open:
  the `LSItemContentTypes` declared across its `CFBundleDocumentTypes`,
  deduplicated, sorted, and joined with single spaces, e.g. `public.plain-text
  public.source-code`. This is raw declared text and it is printed verbatim by
  the audit script, so it is worth being exact about what it is not. It records
  **no file you opened** — not a name, not a path, not a type, not a count, not a
  time. It is a property of the developer's build, the same for every copy of
  that application everywhere. Its privacy weight is that a distinctive list
  (Xcode declares 152 types) fingerprints which application the row is about.
  NULL when the application declared none, or the plist could not be read.

None of the five exists anywhere in the upload path: `BatchEventPayload`'s
hand-written `Serialize` implementation (`rust-service/src/upload/dto.rs`) emits
six fields, and none of them is a label, a stable ID, a name suggestion, a bundle
digest, a declared category, or a document type. All five expire with the rest of
the buffer after 14 days. None can reach a log: `local_name_suggestion` is
redacted to `[redacted]` in the `Debug` implementation
(`rust-service/src/persistence/models.rs`), and the three added columns travel
in a `DeclaredAppMetadata` whose hand-written `Debug`, in the same file, redacts
all three on the same terms — the bundle digest prints as
`[local_identifier]`, the declared category as `[redacted]`, and the document
types as a count of how many there were.

Two of those three are written and not yet read. Classification reads the
declared category and the document types off the event as it arrives, never back
out of the buffer; the only added column anything reads again today is
`app_bundle_stable_id`, used by the bundle-keyed override lookup and by the
triage query that lists the applications Velvt could not classify. The other two
are kept so a later reading of the same evidence does not need the event back,
and they are named here because they are on your disk now — not because
something is using them.

### The embedding sketch, and what can be read back out of it

`semantic_embedding_cache` is the one store on this list that is derived from a
window title. Tier 2 classification builds the string
`app name [SEP] window title`, turns it into a 256-number sketch, and keeps the
sketch — not the string — under a hash of the string. Nothing about it is
uploaded: `BatchEventPayload` has no field it could occupy.

The sketch is lossy and it is not the title. It is also not one-way. Each word
contributes at one hashed coordinate, and each character trigram of that word at
another, so a word leaves roughly a nine-coordinate signature and a dictionary
run over the same hash recovers a meaningful share of the words in a title.
Reimplemented from this repository's own source and run against a live database
on 2026-08-31, it recovered at least one dictionary word from 452 of 512 rows,
935 distinct words in total. On the synthetic title
`divorce attorney consultation booking` it returned exactly those four words out
of a 234,143-word dictionary and nothing else. `PRIVACY_AUDIT.md` Audit 7 is the
method and the numbers. That is the honest description, and it replaces the
sentence this section used to carry.

The hash is unsalted. `add_hashed_feature`
(`rust-service/src/abstraction/plugin.rs`) is a plain SHA-256 of the token,
computed identically on every install, so that run needs nothing from this Mac
— the source in this repository is enough to build the oracle offline. A
per-install random salt would raise the cost to reading the salt off this
machine first, and it is not in the shipped code today. It is named here
because a mitigation that does not exist should not be written down as though
it does.

`personal_semantic_prototype` holds copies of the same sketches for categories
you corrected, and everything above applies to it unchanged.

The cache's horizon is not the buffer's. `record_embedding`
(`rust-service/src/persistence/sqlite.rs`) rewrites `updated_at` on every write,
and `delete_expired_semantic_embeddings` deletes on `updated_at`, so the 14 days
run from the last observation of a pair rather than from the first, while a
`raw_event_buffer` row is swept on the age of the row itself. In practice a
window you open every week keeps its sketch for as long as you keep opening it,
and the 512-row cap is the only bound that still applies to it. The sweep
reaches a pair you stopped observing; it never reaches one you keep observing.

### The rest of the database

The table above is every store that holds something drawn from your Mac. It is
not every table in the file. A database with every shipped migration applied
holds 35 tables, plus SQLite's own `sqlite_sequence`; the 18 that are not in
that table hold counters, settings, keys, and feature state. They are listed here for
the same reason the three empty ones are — you will see them if you open the
file.

| Table | What it holds | Retention |
|---|---|---|
| `classification_telemetry` | one counter per taxonomy version and classification tier: how many events that tier classified. No app identity and no label; the only time it holds is the counter's own `updated_at` | no sweep and no in-app removal; the counters persist until `~/.velvt/` is deleted |
| `classifier_artifact_telemetry` | the same shape for the classifier artifact: one counter per artifact version, such as `builtin-hash-v1` | no sweep and no in-app removal |
| `embedding_salt` | a 32-byte random value generated on this device by migration 0031, provisioned as the per-install key for the embedding feature hash. No activity data. What the shipped classifier does with it is the subject of the section above | singleton, and never rewritten: a second salt would invalidate every sketch stored under the first. No sweep |
| `stable_key_salt` | a second 32-byte random value generated on this device, by migration 0037: the per-install key every stable key, application key, and bundle digest in this file is an HMAC under. It stops a guess being checked offline from this repository alone and stops two Macs' keys matching; it does not stop someone who holds this whole file, because it is in it. No activity data | singleton, and never rewritten while it exists: a second salt would orphan every correction keyed under the first, so if the row is deleted by hand Velvt mints a new one and removes the corrections and mappings it can no longer match. No sweep |
| `upload_host_backoff` | one row per upload host — the configured API hostname, its consecutive-failure count, and the earliest time a next attempt is allowed. No event content | removed for a host as soon as a batch upload to it succeeds; otherwise it persists |
| `work_block_intervention` | the drift offer a block received: broad anchor category, switch count, window length, salience, when it was offered, and the outcome you gave it. The block id is the primary key, so a block holds at most one. No label, app identity, window title, URL, or intention text | removed with its block, and by Clear Local Work Blocks |
| `work_block_category_correction` | when you answer an offer with "wrong classification", the broad category that counts as focus work for that block. Categories only | removed with its block, and by Clear Local Work Blocks |
| `intervention_demotion_state` | one row recording whether interventions are currently demoted, when that happened, and when you last reset it. The current state only, never a history | singleton; removed by Clear Local Work Blocks |
| `focus_state_evidence` | coarse macOS Focus/DND transitions: active or inactive, the time floored to a five-minute bucket, and a local hour and date. No Focus mode name, schedule, or configuration is representable | 14 days, pruned when the next transition is reported rather than on a timer; removed by Clear Local Work Blocks |
| `focus_observer_state` | one row holding the client's most recent UTC offset, so a local-hour rule never needs a locale or an identity | singleton, overwritten in place; removed by Clear Local Work Blocks |
| `quiet_hours_offer_state` | one row remembering whether the quiet-hours offer was triggered, offered, accepted, or declined, when, and under which rule version | singleton; removed by Clear Local Work Blocks |
| `velvt_quiet_hours` | Velvt's own quiet-hours window, if you accepted that offer: start and end local minutes, the rule version, and when it was configured. Not the macOS Focus configuration, which Velvt never reads or writes | singleton; no sweep, and it survives Clear Local Work Blocks — it is a setting you chose, not evidence |
| `initiation_invitation` | one row per soft-start invitation: when it was offered, its local date, the fixed action id, and its outcome. No app identity, title, or intention text | no sweep; removed by Clear Local Work Blocks |
| `initiation_settings` | the single invitations on/off switch | singleton; no sweep, and it survives Clear Local Work Blocks for the same reason |
| `weekly_digest` | one row per completed local week: bounded counts — blocks declared and completed, recoveries, wrong interventions, invitations accepted, withheld — and when the digest was shown and closed. No categories, copy, or per-day breakdown is representable | 12 completed weeks, pruned when the next digest is generated; removed by Clear Local Work Blocks |
| `explain_probe_week` | one tap counter per local week, for the explain-tap metric. Which nudge was explained is not representable | pruned on the same 12-week rule; removed by Clear Local Work Blocks |
| `schema_migration` | one row per applied migration: its version, its file name, and when it ran. Created by the migration runner rather than by a migration file | no sweep; one row is added per migration and none is removed |
| `persistence_migration_probe` | nothing. Migration 0002 created it to prove that a new migration file is embedded and applied, and no code path, shipped or test, inserts a row | empty on every install; no sweep |

### What the app's destructive actions actually remove

Several rows above name an in-app action. Each one is a specific button, and
this is the whole of what it reaches:

- **Reset Corrections** deletes `personal_override`, `personal_app_override`,
  and `personal_semantic_prototype`, and sets `abstraction_map.display_name` to
  NULL on every row. The per-correction **Undo** does the same for one window:
  its own override row and prototype, the application-scoped override behind it,
  and the `display_name` on that window and on the other windows of the same
  application.
- **Clear Local Work Blocks** deletes `work_block` and everything that cascades
  from it — `work_block_observation`, `work_block_result`,
  `work_block_intervention`, `work_block_category_correction`,
  `block_antecedent`, and that block's `intervention_decision_log` rows — plus
  `intervention_demotion_state`, `focus_state_evidence`, `focus_observer_state`,
  `quiet_hours_offer_state`, `initiation_invitation`, `weekly_digest`, and
  `explain_probe_week`. It does not reach the classification and correction
  tables, `out_of_block_run`, or `antecedent_finding`.
- **Delete Account** is a request to the cloud. The Rust service marks an open
  invitation expired, relays the deletion to the account-deletion endpoint, and
  on acceptance clears the stored device and session tokens. As of 2026-08-31 it
  deletes nothing from this database (`ClientMessage::DeleteAccount`,
  `rust-service/src/ipc/router.rs`).

There is no in-app action that clears everything. Deleting `~/.velvt/` is the
only complete removal, and the procedure for it is at the end of this document.

### Corrections to earlier versions of this section

A previous version of this document claimed `raw_event_buffer` "never contains
raw app names or window titles." The window-title half was true; the app-name
half was not, and it is corrected here on 2026-08-21.

A previous version also gave no account of `semantic_embedding_cache` at all —
the words "embedding", "semantic", "prototype", and "vector" did not appear in
this document. Corrected here on 2026-08-31.

A previous version of this section said `raw_event_buffer` holds "two
device-local columns that can name an application" and listed two. Migration 0033
had already added three more — `app_bundle_stable_id`, `declared_app_category`,
and `document_type_ids` — so the count and the list were both wrong. All five are
enumerated above. Corrected here on 2026-09-24.

This document and `PRIVACY_AUDIT.md` Audit 7 both said `scripts/prove_local.sh`
"reports the two embedding columns as UNINSPECTED with a byte count." The script
did not: it listed textual columns only, so a `BLOB` appeared nowhere but the
table inventory's "other" count. Rather than weaken the sentence, the script was
changed on 2026-09-24 to do what both documents say — it now names every `BLOB`
column with a byte count, the salt included. Recorded here because the claim was
wrong on disk in the meantime.

A previous version of the storage table said a `semantic_embedding_cache` row is
swept "on the same horizon as `raw_event_buffer`." The two clocks start at
different instants, and the cache's is the one ordinary use pushes forward
indefinitely. Corrected here on 2026-08-31.

A previous version described 15 of the 34 tables in the file, said nothing about
the other 17, and named clearing "all data" as a way to remove four of them.
There is no such action. The second table above completes the inventory, and the
section before this one names what each in-app action does remove. Corrected
here on 2026-08-31.

Auth and device-bound tokens are never stored in SQLite. Swift
persists the session in the macOS Keychain through `KeychainService`; after IPC
connects it provides the active session to Rust, where `VolatileTokenStore`
holds it in memory only for the service process lifetime.

## What is transmitted to the cloud, and in what form

Only the following ever leave the device, over HTTPS:

- Abstracted event batches (`POST /v1/events/batches`): event ID, a fixed
  category-scoped abstraction type, classification tier, category, timestamp,
  and duration — never a stable local ID, app-specific label, raw app name,
  title, URL, path, or filename.
- Device registration and auth (`POST /v1/devices`, `/v1/auth/refresh`,
  `/v1/auth/devices/reissue`, `/v1/auth/signup`, `/v1/auth/login`,
  `/v1/auth/logout`, `/v1/auth/account/delete`): device and account
  credentials, never raw event content.
- History/insight fetch (`GET /v1/history/daily`, `/v1/insights/daily`):
  read-only requests for already-abstracted, server-side-derived summaries.

The service may be configured to generate insight text through an approved
external model provider. In that mode, it sends a privacy-safe derived prompt,
not raw Mac activity, and stores the prompt, provider attempt, raw provider
output, and quality-gate metadata under the configured insight retention
policy. The production operator is responsible for naming the active provider
and its data-processing terms before enabling it.

The cloud independently enforces this boundary and rejects any batch
containing a forbidden field with `raw_field_rejected`; the Rust service
treats that rejection as terminal for the offending batch (it is never
retried) and surfaces a `PrivacyViolationAlert` over IPC so the menu bar UI
can show it.

## What the abstraction engine does and does not preserve

**Preserves locally:** a stable per-app/title identity (so "the same kind of
activity" can be recognized across events), an on-device display label, and a coarse category
(`focus_work`, `communication`, `passive_consumption`, `system`,
`unclassified`, ...), plus timing. The cloud receives only an allowlisted
category-scoped abstraction type; unapproved values are replaced with
`system:unknown` before persistence, metrics, or audit metadata.

**Does not preserve:** the literal window title, and no URL or file path that
appeared in one.

**Corrected 2026-09-24.** This section previously said the stable ID "is a
one-way hash into a local-only mapping table". That was wrong about the
mechanism. `abstraction/engine.rs` mints it as `abs_` followed by a random
UUIDv4 — it is not derived from the window title at all, by any function. The
guarantee is therefore *stronger* than the sentence claimed, and the sentence
was still false: a random identifier carries no relationship to the string it
stands for, so there is nothing in it to reverse. What does the lookup is the
separate stable *key*, which lives in `abstraction_map` on this device. Since
migration 0037 it is an HMAC-SHA-256 of (app name, window context) under
`stable_key_salt`; before that it was a plain unsalted SHA-256, testable against
any Velvt database from this repository's source alone. The salt ends that, and
ends two Macs sharing a key, but it is stored beside the keys, so anyone holding
the whole database file can still confirm a guessed title one hash at a time.
What limits that is retention: a mapping is swept 14 days after its window was
last observed, unless a correction or a buffered event still points at it.

**Does keep on disk, named here rather than left to be discovered:** the raw
application name, in `raw_event_buffer.local_name_suggestion`; a digest of the
application's bundle identifier, in `raw_event_buffer.app_bundle_stable_id` and
`personal_app_override.bundle_key_hash`, which a holder of the whole file can
reverse by hashing known bundle identifiers under the salt stored beside it; the metadata an application publishes
about itself, in `raw_event_buffer.declared_app_category` and
`raw_event_buffer.document_type_ids`; the names you
type when you correct a classification, in `abstraction_map.display_name`,
`personal_override`, and `personal_app_override`; and a hashed sketch of
`app name [SEP] window title`, in `semantic_embedding_cache` and
`personal_semantic_prototype`. The sketch is not the title and cannot be
turned back into one, but individual words are partially recoverable from it —
the section above says how, and says that the hash it uses is unsalted.

## How to audit what is being collected

The SQLite database is a plain file at `~/.velvt/velvt-service.sqlite3`.
Open it with any SQLite browser (`sqlite3 ~/.velvt/velvt-service.sqlite3`)
and inspect the tables listed above — every column is named in
`rust-service/migrations/`, with the one exception the inventory names:
`schema_migration` is created by the migration runner in
`rust-service/src/persistence/sqlite.rs`. Most of those files carry the
invariant inline;
`0013_personal_semantic_learning.sql`, which creates the two embedding tables,
carries no comment at all, which is why the description of those columns lives
here instead of beside the schema.

`scripts/prove_local.sh` reads the same file and prints every textual column by
name, including the ones that hold application names, the bundle digest, and the
declared metadata above; each of those carries a one-line note saying what it is.
It reports every `BLOB` column — the two embedding sketch columns and the
32-byte `embedding_salt` — as UNINSPECTED with a byte count: it is bash and
sqlite3, it cannot decode a sketch, and a proof that silently omitted the column
would be worth less than one that names what it could not read. What can be read
back out of a sketch is the section above rather than anything the script
prints. The full abstraction and
upload code paths are open source in this repository; `PRIVACY_AUDIT.md` is the
line-by-line verification a security reviewer would otherwise have to redo from
scratch.

## How to delete all local data

1. Quit Velvt.
2. Delete `~/.velvt/` (removes the SQLite database and any other local
   service state).
3. Remove the Keychain entries: open Keychain Access and delete the
   `com.velvt.service.auth` (Rust device/auth tokens) and `com.velvt.mac`
   (Swift session tokens) entries, or run
   `security delete-generic-password -s com.velvt.service.auth` /
   the equivalent for the Swift service name.
4. To also delete your cloud account and any data associated with it, use
   the in-app "Delete Account" action, which sends `delete_account` over
   IPC and the Rust service relays it to the cloud's account-deletion
   endpoint.

## The open-source auditability guarantee

Every line of code that touches a raw event — from the Accessibility
callback in `swift-client/Sources/VelvtMac/Collection/` through the
abstraction engine in `rust-service/src/abstraction/` to the upload DTO in
`rust-service/src/upload/dto.rs` — is in this repository under
[`LICENSE`](LICENSE). There is no closed-source or server-side-only
component standing between your raw activity and the abstraction boundary;
anyone can read, build, and run this exact code to verify the claims in
this document.
