# Privacy

Velvt is designed so that raw, identifying activity data never leaves your
device. This document is the canonical description of what is collected,
what is stored, what is transmitted, and how to audit or delete it
yourself. See [`PRIVACY_AUDIT.md`](PRIVACY_AUDIT.md) for the code-level
verification behind these claims; Audit 8 there is the most recent, run on
2026-09-25 against protocol 31.

## What is collected

The macOS client observes, locally:

- Which application is currently focused (`NSWorkspace.didActivateApplicationNotification`),
  its name and its bundle identifier (`com.microsoft.VSCode` and the like)
- The focused window's title (`kAXFocusedWindowChangedNotification`, `kAXTitleChangedNotification`)
- The focused document URL, when the focused application is a browser that
  exposes one over the Accessibility APIs
- Two things the focused application declares about itself in its own
  `Info.plist`: its `LSApplicationCategoryType` and the document types
  (`LSItemContentTypes`) it says it can open. These are properties of the
  application's build, the same on every Mac, not of anything you did in it
- Whether a macOS Focus mode is on, if you allow Focus status during
  onboarding: one yes-or-no value, never which Focus mode or its schedule
- Timestamps for these events

That list is exhaustive. Velvt does not use screen recording, keylogging,
the microphone, or the camera, and never will. The macOS app requests the
Accessibility and Notifications permissions, and offers the optional Focus
status permission during onboarding; it requests nothing else.

The focused document URL is reduced to a **hostname and nothing else** the
moment it reaches the Rust service, by `focused_site_context`
(`rust-service/src/abstraction/browser.rs`). Paths, query strings, fragments,
credentials, and ports are discarded; `file://` URLs are rejected outright, so
a local file path is never reduced to anything at all. The raw URL is
destructured out of the event in `AbstractionEngine::process` and is never read
again. Audit 8 sent a URL with credentials, a port, a path, a query and a
fragment through a running service and found none of those parts in the
database, the logs, or any request.

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
derived from a window title. Every one of them is named in the tables in the
next section, and every column of every table is listed at the end of it.

An optional work-block intention also stays on the Mac. It crosses only the
local Unix socket, is stored in the protected SQLite file for at most 24 hours,
and is never added to upload JSON, cloud/cache payloads, logs, telemetry,
crash-safe diagnostics, or notification identifiers. Safe work-block results
are local-only aggregates. See
[`docs/architecture/work-block-loop.md`](docs/architecture/work-block-loop.md)
for the exact per-field boundary.

## What is stored locally, and for how long

Everything the Rust service stores lives in one SQLite database at
`~/.velvt/velvt-service.sqlite3` (configurable via `VELVT_DATABASE_PATH`). The
file is created owner-only (`0600`) inside an owner-only `~/.velvt/` (`0700`).
The Swift app keeps a small amount of state elsewhere, and one optional
integration writes a file beside the database; both are described under
"Stored outside the database" below.

This section describes the source in this repository, which is ahead of the
current build. Velvt 1.0.11 has migrations 0001–0036. Four things described
below start with the first build after 1.0.11: the per-install
`stable_key_salt` that keys the stored digests (migration 0037), `egress_ledger`
(migration 0038), the migration checksum in `schema_migration` (migration
0039), and SQLite's `secure_delete`. Each is scoped again where it is
described.

| Table | Contents | Default retention |
|---|---|---|
| `abstraction_map` | stable-key hash → stable ID, the on-device `label` (such as `video:youtube`), category, taxonomy version, classification provenance, and `display_name` — a friendly name for the window, or the activity name you typed when you renamed a classification. The key is an HMAC under this install's `stable_key_salt` (migration 0037), described below | a mapping is swept once 14 days pass with no further observation of its window, unless a correction you made or an event still in `raw_event_buffer` points at it — so a window you corrected keeps its mapping until you undo that correction. `display_name` also goes when you undo that correction, or with Reset Corrections, which nulls the column on every row |
| `raw_event_buffer` | abstracted event metadata for short-lived audit/replay and the local 14-day activity chart, plus seven device-local columns that can name or identify an application, all described below (`label`, `local_display_label`, `local_name_suggestion`, `app_stable_id`, `app_bundle_stable_id`, `declared_app_category`, `document_type_ids`) | 14 days (`VELVT_RAW_EVENT_TTL_HOURS`) |
| `upload_batch` / `batch_event` | the upload queue: each batch's id, status, attempt count and last error code, and per queued event its id, timestamp, duration, category, classification tier and taxonomy version — plus two device-local columns that are never sent, the random stable ID and the on-device `label` (which can name a service, as `communication:slack` does); the serializer turns the label into the category-scoped type that is sent | sent batches: 30 days; rejected batches: 7 days (audit window); pending and failed batches: 30 days, the same horizon as sent |
| `personal_override` | one correction you made to a single window: the window's stable-key hash, the category you chose, `activity_name` (the name you typed for it), and since migration 0036 `app_key_hash` — the application key the correction also taught, the same salted app-name digest as `raw_event_buffer.app_stable_id`, so Undo can remove that application rule without reading the event buffer | until you undo that correction or use Reset Corrections. No sweep expires it |
| `personal_app_override` | the same correction applied to a whole application rather than one window: app-key hash, category, `activity_name`, a correction count, since migration 0034 `bundle_key_hash` — the same bundle-identifier digest described under `raw_event_buffer` below, NULL on every rule taught before that migration — and since migration 0035 `app_only`, which records whether you taught the rule for the whole application from the triage list (1) or it was written beside a single-window correction (0) | until you undo the correction it came from or use Reset Corrections. No sweep expires it |
| `semantic_embedding_cache` | one hashed sketch per application-and-title pair the classifier has scored, keyed by the same salted window key as `abstraction_map`. The sketch is derived from the raw application name and the raw window title, computed under this install's `embedding_salt`, and individual words are partially recoverable from it by someone holding the whole file — described below | the 512 most recently observed pairs; a pair is swept once 14 days pass with no further observation of it. The clock restarts on every observation, so a window you keep returning to is never swept |
| `personal_semantic_prototype` | a copy of that same sketch, kept for a category you corrected so the classifier can recognise the activity again | the 64 most-corrected pairs, at most 12 per category; removed by undoing that correction or by Reset Corrections. No sweep expires it |
| `history_cache` / `insight_cache` | ready-to-display summaries fetched from the cloud | minutes to tens of minutes, per `VELVT_HISTORY_TTL_SECONDS`/`VELVT_INSIGHT_TTL_SECONDS` |
| `work_block` | local state and optional free-form intention | intention: 24 hours; the safe state until Clear Local Work Blocks |
| `work_block_observation` | safe category/status/confidence spans only | removed with its block, and by Clear Local Work Blocks |
| `work_block_result` | safe local duration, transition, recovery, coverage, evidence, observation, and one next action | removed with its block, and by Clear Local Work Blocks |
| `intervention_decision_log` | every moment the drift policy was evaluated and what it decided, including the times it decided to stay silent: policy version, broad anchor category, switch count, elapsed and remaining seconds, the verdict, the propensity (always 1.0: the policy is deterministic), and, once its horizon passes, whether the anchor category was seen again within 600 seconds. No label, no app identity, no window title, no URL, no intention text | removed with the block it belongs to, and by Clear Local Work Blocks, which also deletes any row logged without a block |
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

`raw_event_buffer` holds no window titles and no URLs. It does hold seven
device-local columns that can name or identify an application, and all seven
are disclosed here rather than hidden behind the table's name:

- **`label`** — the on-device classification label, `<type>:<behavior>`, such
  as `document:code`, `communication:slack`, or `video:youtube`. For a service
  the taxonomy knows by name, the label names it. It is never sent: the upload
  serializer replaces it with a category-scoped type such as
  `communication:inferred`. The same label is kept in `abstraction_map` and in
  `batch_event`.
- **`local_display_label`** — a friendly display string for the local UI, such
  as `Coding`, `Gmail`, or `YouTube`. Derived, not raw, but it can identify a
  service. **It can also hold a name you typed.** When you correct a
  classification and give the activity a name, that name becomes the
  `local_display_label` of the event you corrected and of every later event
  the correction classifies. Undo and Reset Corrections remove the correction
  and its names from `abstraction_map`, `personal_override`, and
  `personal_app_override`, but they do not rewrite rows already in this
  buffer: those keep the name until they expire, 14 days after they were
  written. Audit 8 measured this on a throwaway database.
- **`local_name_suggestion`** — the **raw application name**, retained only when
  the classifier did not match a seed rule or one of your own corrections, so
  the app can offer you a one-tap rename instead of showing `Unclassified`.
  Generated by `responsible_local_name_suggestion`
  (`rust-service/src/abstraction/engine.rs`), which returns nothing for
  seed-matched or user-corrected apps and nothing for generic names. A browser
  showing a page no rule recognises is a classifier miss too, so its name
  (`Safari`, `Google Chrome`) can be retained this way.
- **`app_stable_id`** — the application key: an HMAC-SHA-256 of the application
  name under this install's `stable_key_salt` (migration 0037), the key the
  application-wide corrections and the triage list are looked up by. It holds
  the digest, never the name, but an application name is a guessable input:
  anyone holding the whole file has the salt too, and can hash a list of
  application names under it to read off which ones you ran. Treat it as
  naming the application.
- **`app_bundle_stable_id`** — a digest of the application's **bundle
  identifier** (`com.microsoft.VSCode` and the like), computed under its own
  domain separator so it cannot collide with the name key beside it. The column
  holds the digest, never the identifier — but the digest is not a secret. Since
  migration 0037 (the first build after 1.0.11) it is keyed with this install's
  `stable_key_salt`, so it differs from Mac to Mac and a list hashed from public
  sources matches nothing; in 1.0.11 it is unsalted and the same on every Mac. The salt
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

None of the seven exists anywhere in the upload path: `BatchEventPayload`'s
hand-written `Serialize` implementation (`rust-service/src/upload/dto.rs`) emits
six fields, and none of them is a label, a stable ID, a name suggestion, an
application key, a bundle digest, a declared category, or a document type. All
seven expire with the rest of the buffer after 14 days. None can reach a log:
`local_name_suggestion` is redacted to `[redacted]` in the `Debug`
implementation (`rust-service/src/persistence/models.rs`), and the three
declared-metadata columns travel in a `DeclaredAppMetadata` whose hand-written
`Debug`, in the same file, redacts all three on the same terms — the bundle
digest prints as `[local_identifier]`, the declared category as `[redacted]`,
and the document types as a count of how many there were.

Two of the declared-metadata columns are written and not yet read.
Classification reads the declared category and the document types off the event
as it arrives, never back out of the buffer; the only one of the three anything
reads again today is `app_bundle_stable_id`, used by the bundle-keyed override
lookup and by the triage query that lists the applications Velvt could not
classify. The other two are kept so a later reading of the same evidence does
not need the event back, and they are named here because they are on your disk
now — not because something is using them.

The triage list is the one place an application name leaves the Rust service
on purpose: `unclassified_triage` carries each unclassified application's
local name, as `display_name`, over the local socket to the Swift app so it can
ask you what the application is. The local dashboard carries the same names for
the Daily Activity chart. Both stay on the socket; neither is uploaded.

### The embedding sketch, and what can be read back out of it

`semantic_embedding_cache` is the one store on this list that is derived from a
window title. Tier 2 classification builds the string
`app name [SEP] window title`, turns it into a 256-number sketch, and keeps the
sketch — not the string — under the window's salted stable key. Nothing about
it is uploaded: `BatchEventPayload` has no field it could occupy.

The sketch is lossy and it is not the title. It is also not one-way. Each word
contributes at one hashed coordinate, and each character trigram of that word at
another, so a word leaves roughly a nine-coordinate signature and a dictionary
run over the same hash recovers a meaningful share of the words in a title.
Reimplemented from this repository's own source and run against a live database
on 2026-08-31, it recovered at least one dictionary word from 452 of 512 rows,
935 distinct words in total. On the synthetic title
`divorce attorney consultation booking` it returned exactly those four words out
of a 234,143-word dictionary and nothing else. `PRIVACY_AUDIT.md` Audit 7 is the
method and the numbers.

That measurement was taken against sketches computed with an unsalted hash, and
the shipped code has changed since. Since migration 0031 and commit `5b6ca1e`
(2026-09-14), `main.rs` builds the classifier with
`EmbeddingSimilarityPlugin::builtin_salted` and this install's `embedding_salt`,
a 32-byte random value generated on this Mac, and `add_hashed_feature`
(`rust-service/src/abstraction/plugin.rs`) hashes the salt ahead of every word
and trigram. Migration 0031 deleted every sketch computed before it. If the salt
cannot be read, the service turns Tier 2 off rather than fall back to the
unsalted hash. The tagged sources of 1.0.9 and 1.0.11 both include this.

What the salt changes: the dictionary run above can no longer be done from this
repository alone, once, for every install. It has to be redone per Mac, with that
Mac's salt in hand. What it does not change: the salt is stored in the same file
as the sketches, because the sketches must stay comparable across restarts, so
anyone holding the whole database file can run the same recovery against it.
Treat the two sketch columns as partially readable by whoever has the file.

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

### What salting does and does not protect

Two per-install salts live in the database, and they protect the same thing in
the same limited way. `stable_key_salt` (migration 0037) keys every window key,
application key, and bundle digest; `embedding_salt` (migration 0031) keys the
sketch. Each is 32 random bytes generated on this Mac. Because of them, a stored
key or sketch cannot be tested against guesses using only this repository, and
the same application or window produces different values on two Macs. Because
each salt is stored beside what it keys, neither protects anything from someone
who has the whole file. The only protection against that is who can read the
file: it is owner-only, inside an owner-only directory.

Which builds have which salt: `embedding_salt` is in the tagged sources of 1.0.9
and 1.0.11. `stable_key_salt` is not. Migration 0037 starts with the first
build after 1.0.11, and Velvt 1.0.11 has migrations 0001–0036, so on a 1.0.11
database every window key, application key, and bundle digest is a plain
SHA-256 over a fixed domain string and the raw input: the same on every Mac,
and testable against a list of guesses with this repository's source alone.
When the first build after 1.0.11 opens such a database, migration 0037
generates the salt and re-keys every digest already on disk under it.

### The rest of the database

The first table above is every store that holds something drawn from your Mac. It is
not every table in the file. A database with every migration in this source
tree applied holds 37 tables, plus SQLite's own `sqlite_sequence`. A Velvt
1.0.11 database (migrations 0001–0036) holds 34: it has no `stable_key_salt`,
`egress_ledger`, or `egress_ledger_checkpoint`, which migrations 0037 and 0038
add. Of the 37,
the 20 that are not in that table hold counters, settings, keys, feature state,
and the record of what was sent. They are listed here for the same reason the
three empty ones are — you will see them if you open the file.

| Table | What it holds | Retention |
|---|---|---|
| `classification_telemetry` | one counter per taxonomy version and classification tier: how many events that tier classified. No app identity and no label; the only time it holds is the counter's own `updated_at` | no sweep and no in-app removal; the counters persist until `~/.velvt/` is deleted |
| `classifier_artifact_telemetry` | the same shape for the classifier artifact: one counter per artifact version, such as `builtin-hash-v1` | no sweep and no in-app removal |
| `egress_ledger` | one row per HTTP request the helper made, written before it was sent: when, the method and URL, the body's byte count, the SHA-256 of the body, whether an account token was attached, and a hash chaining the row to the one before. No body is stored. For the sign-up, log-in, and token-refresh bodies the hash is taken with the password or token replaced by `[redacted]`. Described under "How to audit what is sent" below | 30 days or 100,000 rows, whichever is tighter, oldest first. No in-app removal |
| `egress_ledger_checkpoint` | the sequence number and hash of the last `egress_ledger` row retention removed, so the rows that remain still verify | the newest checkpoint only |
| `embedding_salt` | a 32-byte random value generated on this device by migration 0031: the per-install key the embedding feature hash is computed under, as described above. No activity data | singleton, and never rewritten: a second salt would invalidate every sketch stored under the first. No sweep |
| `stable_key_salt` | a second 32-byte random value generated on this device, by migration 0037: the per-install key every stable key, application key, and bundle digest in this file is an HMAC under. It stops a guess being checked offline from this repository alone and stops two Macs' keys matching; it does not stop someone who holds this whole file, because it is in it. No activity data. Not in a Velvt 1.0.11 database, which stops at migration 0036; it starts with the first build after 1.0.11 | singleton, and never rewritten while it exists: a second salt would orphan every correction keyed under the first, so if the row is deleted by hand Velvt mints a new one and removes the corrections and mappings it can no longer match. No sweep |
| `upload_host_backoff` | one row per upload host — the configured API hostname, its consecutive-failure count, and the earliest time a next attempt is allowed. No event content | removed for a host as soon as a batch upload to it succeeds; otherwise it persists |
| `work_block_intervention` | the drift offer a block received: broad anchor category, switch count, window length, salience, the fixed action id, when it was offered, when its in-app card was first on screen (`card_seen_at`, migration 0032), and the outcome you gave it and when. The block id is the primary key, so a block holds at most one. No label, app identity, window title, URL, or intention text | removed with its block, and by Clear Local Work Blocks |
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
| `schema_migration` | one row per applied migration: its version, its file name, a checksum of the migration's SQL (since migration 0039, the first build after 1.0.11; computed from the public source file, so nothing in it comes from your Mac), and when it ran. Created by the migration runner rather than by a migration file | no sweep; one row is added per migration and none is removed |
| `persistence_migration_probe` | nothing. Migration 0002 created it to prove that a new migration file is embedded and applied, and no code path, shipped or test, inserts a row | empty on every install; no sweep |

### What the app's destructive actions actually remove

Several rows above name an in-app action. Each one is a specific button, and
this is the whole of what it reaches:

- **Reset Corrections** deletes `personal_override`, `personal_app_override`,
  and `personal_semantic_prototype`, and sets `abstraction_map.display_name` to
  NULL on every row. The per-correction **Undo** does the same for one window:
  its own override row and prototype, the application-scoped override behind it,
  and the `display_name` on that window and on the other windows of the same
  application. Neither rewrites `raw_event_buffer`: a name you typed stays in
  the `local_display_label` of the events it was applied to until they expire.
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
  on acceptance clears the session it holds in memory and destroys the upload
  batches still waiting to be sent. It deletes nothing else from this database
  (`ClientMessage::DeleteAccount`, `rust-service/src/ipc/router.rs`).

A deleted row is also gone from the file itself. The service opens the database
with SQLite's `secure_delete` on, so SQLite overwrites a deleted row's bytes with
zeros instead of leaving them in free space, where a tool reading the raw file
could still find them. Audit 8 found the opposite before this was switched on on
2026-09-25: after Clear Local Work Blocks, the text of a cleared intention was
still readable in the raw file. This applies from the first build after 1.0.11.
Velvt 1.0.11 and earlier open the database without `secure_delete`, so on those
builds a deleted row's bytes can stay readable in the raw file, as Audit 8
found.

There is no in-app action that clears everything. Deleting `~/.velvt/` is the
only complete removal, and the procedure for it is at the end of this document.

### Stored outside the database

- **The macOS Keychain, service `com.velvt.mac`.** The Swift app keeps your
  session there, as one item (account `velvt.auth_snapshot`) holding the device
  and account access and refresh tokens and their expiry times, your user ID,
  the device ID, the email address you signed in with, and a pending-deletion
  marker. Signing out, and an accepted account deletion, remove every item
  Velvt keeps there. Your password is never stored anywhere. The Rust service has no Keychain item
  of its own and writes no token to disk: it holds the session Swift hands it
  in memory (`VolatileTokenStore`) for as long as it runs.
- **App preferences** (`~/Library/Preferences/com.velvt.mac.plist`). Onboarding
  state, the purpose and intensity you chose during onboarding, whether you
  were asked about Focus status, the menu window's size and whether it stays
  open, the offline-collection switch, two local counters (actions logged and
  interventions, cleared when you sign out), and up to 256 opaque notification
  IDs, kept so a notification is not scheduled twice. No activity, name, title,
  or notification text.
- **Notification Center.** Drift offers and daily insights you receive are
  held by macOS Notification Center, as any app's notifications are.
- **The socket.** `~/.velvt/velvt-service.sock` is the local connection between
  the app and the service. It holds no data.
- **An optional Claude Code log.** Velvt.app never writes
  `~/.velvt/agent-attention-log.jsonl`. It exists only if you installed the
  optional Claude Code notification hook from Velvt's workspace, which is not
  part of this repository or the app. That hook asks the running service
  whether you are in a work block and writes one line each time a Claude Code
  agent asks for your attention: the time, the notification type, Claude Code's
  own session ID, Velvt's verdict and the reason, and what it was based on: the
  work-block phase, the broad current and anchor categories, the classification
  status, and the block's remaining seconds. No command, path, file name, window
  title, or prompt text. The hook keeps the file owner-only
  (`0600`), rotates it at 256 KiB or when its oldest line is 14 days old, keeps
  one previous generation (`agent-attention-log.jsonl.1`), and deletes that one
  once it has gone 14 days without a write, so the two files stay under about
  512 KiB and a line lives at most about 28 days. Deleting `~/.velvt/` removes
  both.
- **Exports you save.** Export Velvt Data writes a JSON file wherever you choose
  in the save panel. Velvt does not track or remove it.
- **Backups.** Velvt does not exclude `~/.velvt/` from Time Machine or any
  other backup, so a backup of your home folder holds a copy of the database.
  Deleting `~/.velvt/` does not remove it from backups you already made.

### Every column

Every column of every table in the database, as the migrations create it. A
build test (`migrated_schema_holds_exactly_the_documented_columns` in
`rust-service/tests/published_claims.rs`) compares this list with the migrated
schema in both directions, so a migration that adds, renames or drops a column
fails until this list is updated in the same commit.

| Table | Columns |
|---|---|
| `abstraction_map` | `id`, `key_hash`, `stable_id`, `label`, `category`, `taxonomy_version`, `created_at`, `updated_at`, `classification_tier`, `display_name`, `classification_status`, `classification_confidence`, `classification_source` |
| `antecedent_finding` | `finding_id`, `candidate_id`, `candidate_registry_version`, `discovered_at`, `discovery_window_start`, `discovery_window_end`, `support_episodes`, `effect_size`, `q_value`, `confirmed_at`, `confirm_support_episodes`, `confirm_effect_size`, `state`, `surfaced_at`, `retracted_at`, `retraction_reason`, `user_disputed_at` |
| `batch_event` | `id`, `batch_id`, `event_id`, `stable_id`, `label`, `category`, `taxonomy_version`, `classification_tier`, `occurred_at`, `duration_seconds`, `created_at` |
| `block_antecedent` | `block_id`, `window_seconds`, `categories`, `switch_count`, `dominant_category`, `dominant_dwell_seconds`, `day_type`, `hour_bucket`, `is_first_block_of_day`, `antecedent_version` |
| `classification_telemetry` | `taxonomy_version`, `classification_tier`, `event_count`, `updated_at` |
| `classifier_artifact_telemetry` | `artifact_version`, `classification_count`, `updated_at` |
| `egress_ledger` | `seq`, `recorded_at`, `method`, `endpoint`, `body_bytes`, `body_sha256`, `body_redacted`, `bearer`, `prev_hash`, `entry_hash` |
| `egress_ledger_checkpoint` | `through_seq`, `through_hash`, `created_at` |
| `embedding_salt` | `id`, `salt`, `created_at` |
| `explain_probe_week` | `week_start_local_date`, `taps`, `updated_at` |
| `focus_observer_state` | `id`, `utc_offset_seconds`, `updated_at` |
| `focus_state_evidence` | `id`, `active`, `changed_at_bucket`, `local_hour`, `local_date`, `recorded_at` |
| `history_cache` | `id`, `date`, `payload`, `ttl`, `created_at` |
| `initiation_invitation` | `invitation_id`, `offered_at`, `local_date`, `action_id`, `policy_version`, `backoff_policy_version`, `outcome`, `outcome_at` |
| `initiation_settings` | `id`, `invitations_enabled`, `updated_at` |
| `insight_cache` | `id`, `date`, `payload`, `ttl`, `created_at`, `not_found` |
| `intervention_decision_log` | `decision_id`, `occurred_at`, `block_id`, `policy_version`, `anchor_category`, `switch_count`, `elapsed_seconds`, `remaining_seconds`, `gate_verdict`, `propensity`, `anchor_seen_within_600s`, `outcome_at` |
| `intervention_demotion_state` | `id`, `state`, `demoted_at`, `manual_reset_at`, `threshold_policy_version`, `repromotion_policy_version`, `updated_at` |
| `out_of_block_run` | `id`, `started_at_bucket`, `duration_seconds`, `category`, `classification_status`, `classification_confidence`, `local_hour`, `local_date` |
| `persistence_migration_probe` | `id`, `marker`, `created_at` |
| `personal_app_override` | `app_key_hash`, `category`, `activity_name`, `correction_count`, `created_at`, `updated_at`, `bundle_key_hash`, `app_only` |
| `personal_override` | `key_hash`, `category`, `created_at`, `updated_at`, `activity_name`, `app_key_hash` |
| `personal_semantic_prototype` | `key_hash`, `category`, `embedding`, `dimensions`, `correction_count`, `updated_at` |
| `quiet_hours_offer_state` | `id`, `rule_version`, `triggered_at`, `offered_at`, `response`, `responded_at` |
| `raw_event_buffer` | `id`, `event_id`, `stable_id`, `label`, `category`, `taxonomy_version`, `occurred_at`, `created_at`, `duration_seconds`, `local_display_label`, `classification_tier`, `classification_status`, `classification_confidence`, `classification_source`, `local_name_suggestion`, `upload_eligible`, `app_stable_id`, `app_scope_eligible`, `app_bundle_stable_id`, `declared_app_category`, `document_type_ids` |
| `schema_migration` | `id`, `version`, `name`, `created_at`, `checksum` |
| `semantic_embedding_cache` | `key_hash`, `embedding`, `dimensions`, `updated_at` |
| `stable_key_salt` | `id`, `salt`, `created_at` |
| `upload_batch` | `id`, `batch_id`, `status`, `sent_at`, `attempt_count`, `next_attempt_at`, `last_error_code`, `created_at` |
| `upload_host_backoff` | `id`, `host`, `attempt_count`, `next_attempt_at`, `created_at`, `updated_at` |
| `velvt_quiet_hours` | `id`, `start_local_minutes`, `end_local_minutes`, `rule_version`, `configured_at` |
| `weekly_digest` | `week_start_local_date`, `generated_at`, `blocks_declared`, `blocks_completed`, `recoveries`, `wrong_interventions`, `invitations_accepted`, `withheld`, `digest_version`, `delivered_at`, `acknowledged_at` |
| `work_block` | `block_id`, `state_version`, `phase`, `intention`, `purpose`, `intensity`, `planned_duration_seconds`, `started_at`, `paused_at`, `total_paused_seconds`, `ended_at`, `recovered_after_restart`, `recovery_of`, `intention_expires_at`, `created_at`, `updated_at`, `origin` |
| `work_block_category_correction` | `block_id`, `category`, `counts_as_category`, `corrected_at` |
| `work_block_intervention` | `block_id`, `offered_at`, `action_id`, `anchor_category`, `switch_count`, `window_seconds`, `outcome`, `outcome_at`, `salience`, `card_seen_at` |
| `work_block_observation` | `id`, `block_id`, `occurred_at`, `ended_at`, `category`, `classification_status`, `classification_confidence` |
| `work_block_result` | `block_id`, `payload`, `created_at` |

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
and `document_type_ids` — so the count and the list were both wrong. Corrected
here on 2026-09-24. That version then counted six columns in the table and five
in the prose, and described neither `app_stable_id` nor `label`. Both are
described above, the count is seven in both places, and the column list at the
end of this section is now checked by a build test. Corrected here on
2026-09-25.

This document and `PRIVACY_AUDIT.md` Audit 7 both said `scripts/prove_local.sh`
"reports the two embedding columns as UNINSPECTED with a byte count." The script
did not: it listed textual columns only, so a `BLOB` appeared nowhere but the
table inventory's "other" count. Rather than weaken the sentence, the script was
changed on 2026-09-24 to do what both documents say — it now names every `BLOB`
column with a byte count, the salts included. Recorded here because the claim was
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

A previous version said the embedding hash "is unsalted" and that a per-install
salt "is not in the shipped code today." Both were stale from 2026-09-14, when
`main.rs` began building the classifier with `builtin_salted` and this install's
`embedding_salt`; the document understated the protection. Corrected here on
2026-09-25.

A previous version said "all persistence lives in a SQLite database" and did not
mention the Keychain items, the app preferences, the optional Claude Code log,
or backups. It also named the personal-override, application-override, and
work-block-intervention rows without the columns migrations 0032, 0035, and
0036 added (`card_seen_at`, `app_only`, `app_key_hash`), and said the app
requests only the Accessibility and Notifications permissions, leaving out the
optional Focus status permission and the application metadata the client
reads. Corrected here on 2026-09-25.

## What is transmitted to the cloud, and in what form

Only the following ever leave the device. The Rust service sends every one of
them, over HTTPS to Velvt's API, and records each in `egress_ledger` before
sending it. This list is exactly `egress::ENDPOINTS`
(`rust-service/src/egress/mod.rs`), which a test compares with every API path in
the service's source, and a second test (`privacy_document_lists_exactly_the_endpoints_the_helper_can_reach`)
compares with this list.

- **`POST /v1/events/batches`** — abstracted event batches, every 60 seconds
  while there are events to send (50 events close a batch early), and on
  retry. Per batch: a batch ID, a schema version, the app version, the taxonomy
  version, and the abstraction types that appear in it. Per event: a random event ID, a
  timestamp, a duration, a broad category, a category-scoped abstraction type,
  and a classification tier — never a stable local ID, an on-device label, an
  application name or key, a bundle identifier or digest, declared metadata, a
  title, a URL, a path, or a filename.
- **`PATCH /v1/events/{event_id}/classification`** — when you correct a
  classification while signed in: the broad category you chose, as
  `{"category": ...}`. The activity name you typed is not sent.
- **`POST /v1/auth/signup`** and **`POST /v1/auth/login`** — when you create an
  account or sign in: your email address and password.
- **`POST /v1/devices`** — when you sign in on a Mac with no registered
  device: the app version.
- **`POST /v1/auth/devices/reissue`** — when you sign in on a Mac that is
  already registered, or the server says this device's token was revoked: the
  device ID.
- **`POST /v1/auth/refresh`** — when the access token is about to expire: the
  refresh token.
- **`GET /v1/auth/session`** — when the app restores a saved session. No body.
- **`POST /v1/auth/logout`** — when you sign out. No body.
- **`DELETE /v1/account`** — when you use Delete Account. No body.
- **`GET /v1/insights/poll`** — continuously while signed in: a long poll held
  open up to 70 seconds, then asked again. No body.
- **`GET /v1/history/daily?days=N`** and
  **`GET /v1/insights/daily?date=YYYY-MM-DD`** — every 10 minutes while signed
  in, and when the app asks for history or an insight that is not cached:
  read-only requests for already-abstracted, server-side-derived summaries.
- **`GET /v1/ready`** — when the menu asks whether the server is reachable, at
  most once a minute. No body and no account token.

Sign-up, log-in, and `/v1/ready` carry no token, and a token refresh carries
the refresh token in its body; every other request carries an access token.
Like any HTTPS request, each one also shows the server your IP address and the
time.

Two things outside the Rust service can reach the network, and neither is in the
ledger. The Swift app's own update check (Sparkle) requests an appcast from
Velvt's update feed when a build turns it on; both build configurations in this
repository turn it off, and Velvt 1.0.11 shipped with it off. And the Forgot
password link opens
`https://getvelvt.com/forgot-password/` in your web browser, which is your
browser's request rather than Velvt's. The Swift app makes no other network
request.

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
any Velvt database from this repository's source alone. Migration 0037 starts
with the first build after 1.0.11, so a Velvt 1.0.11 database still holds the
unsalted form. The salt ends that, and
ends two Macs sharing a key, but it is stored beside the keys, so anyone holding
the whole database file can still confirm a guessed title one hash at a time.
What limits that is retention: a mapping is swept 14 days after its window was
last observed, unless a correction or a buffered event still points at it.

**Does keep on disk, named here rather than left to be discovered:** the raw
application name, in `raw_event_buffer.local_name_suggestion`; on-device labels
that can name a service, in `raw_event_buffer.label`,
`raw_event_buffer.local_display_label`, `abstraction_map.label`, and
`batch_event.label`; salted digests of the application name and of its bundle
identifier, in `raw_event_buffer.app_stable_id`,
`raw_event_buffer.app_bundle_stable_id`, `personal_override.app_key_hash`,
`personal_app_override.app_key_hash`, and `personal_app_override.bundle_key_hash`,
which a holder of the whole file can reverse by hashing known names and bundle
identifiers under the salt stored beside them; the metadata an application
publishes about itself, in `raw_event_buffer.declared_app_category` and
`raw_event_buffer.document_type_ids`; the names you type when you correct a
classification, in `abstraction_map.display_name`, `personal_override`,
`personal_app_override`, and `raw_event_buffer.local_display_label`; and a
hashed sketch of `app name [SEP] window title`, in `semantic_embedding_cache`
and `personal_semantic_prototype`. The sketch is not the title and cannot be
turned back into one, but individual words are partially recoverable from it by
someone holding the whole file — the section above says how, and what the
per-install salt does and does not change.

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
It reports every `BLOB` column — the two embedding sketch columns and the two
32-byte salts — as UNINSPECTED with a byte count: it is bash and
sqlite3, it cannot decode a sketch, and a proof that silently omitted the column
would be worth less than one that names what it could not read. What can be read
back out of a sketch is the section above rather than anything the script
prints. The full abstraction and
upload code paths are open source in this repository; `PRIVACY_AUDIT.md` is the
line-by-line verification a security reviewer would otherwise have to redo from
scratch.

## How to audit what is sent

The helper (`velvt-service`) holds the only network client in Velvt's Rust code,
and it appends every request to `egress_ledger` before sending it. A request
the ledger cannot record is not sent. Velvt 1.0.11 and earlier do not have the
ledger; it starts with the first build after 1.0.11. Two tools read it:

- `scripts/prove_egress.sh` recomputes every row's hash with sqlite3 and perl,
  reports whether the chain is intact, and prints what was sent, by endpoint
  and row by row. An intact chain shows that no row was edited or removed from
  the middle. It cannot show that the end was not cut off, or that someone
  with write access to the file did not rebuild the whole chain. The head hash
  it prints is what you write down to catch that later.
- `/Applications/Velvt.app/Contents/Resources/velvt-service --dry-run-egress`
  opens the database read-only and prints each queued upload byte for byte,
  with the SHA-256 the ledger will record when it is sent. It also prints every
  endpoint the helper can reach, and sends nothing.

The ledger covers the helper's requests only. DNS lookups and TLS handshakes are
not rows. The app's own update check, when a build enables it, is not recorded.

## How to delete all local data

1. Quit Velvt.
2. Delete `~/.velvt/` (removes the SQLite database, the socket, and, if you
   installed the optional Claude Code hook, its log).
3. Remove the Keychain items: open Keychain Access and delete every entry
   whose name is `com.velvt.mac`, or run
   `security delete-generic-password -s com.velvt.mac` repeatedly until it
   reports that the item could not be found — each run removes one item. The
   Rust service keeps no Keychain item.
4. Remove the app preferences: `defaults delete com.velvt.mac`.
5. Backups made before this, such as Time Machine snapshots of your home
   folder, still hold copies; remove them with your backup tool if you need to.
6. To also delete your cloud account and any data associated with it, use
   the in-app "Delete Account" action before step 1, which sends
   `delete_account` over IPC and the Rust service relays it to the cloud's
   account-deletion endpoint.

## The open-source auditability guarantee

Every line of code that touches a raw event — from the Accessibility
callback in `swift-client/Sources/VelvtMac/Collection/` through the
abstraction engine in `rust-service/src/abstraction/` to the upload DTO in
`rust-service/src/upload/dto.rs` — is in this repository under
[`LICENSE`](LICENSE). There is no closed-source or server-side-only
component standing between your raw activity and the abstraction boundary;
anyone can read, build, and run this exact code to verify the claims in
this document.
