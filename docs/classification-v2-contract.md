# Classification v2 — implementation contract

Written 2026-09-23. The single agreed design for five changes. Every agent
working on this reads this file first and implements against it exactly.

## Why

Measured on a real machine (107 installed apps, 15 running in the foreground):

- **63% of installed apps are UNLOGGED.** `is_confident_evidence`
  (`work_block/mod.rs:1464`) excludes UNLOGGED, so those apps are invisible to
  the drift gate, the anchor, and the frozen feature contract.
- **The user's editor is one of them.** `NSRunningApplication.localizedName`
  for VS Code is literally `Code`. The taxonomy has `Visual Studio Code`,
  `Code - OSS` and `VSCodium`. Matching is strict full-string equality after
  normalisation (`plugin.rs:190`), so `Code` matches nothing.
- **Accented app names can never match.** `normalize_classifier_text`
  (`normalize.rs:6`) uses `to_ascii_lowercase` then replaces every non-ASCII
  alphanumeric with a space, so `é` is destroyed rather than folded.
- **8 of 205 seeds are dead.** `SeedDictionaryPlugin` refuses any seed whose
  pattern is a browser name (`plugin.rs:176`), so the eight browser entries are
  structurally unreachable.

The fix is not better inference. It is **keying on something stable** and
**making the teaching cheap and visible**.

## Invariants — these outrank every item below

1. **Nothing new crosses the wire.** `upload/dto.rs`'s hand-written `Serialize`
   emits exactly `event_id, occurred_at, abstraction_type,
   abstraction_type_version, classification_tier, payload{duration_seconds,
   category}`. Bundle identifiers, declared categories and document types are
   **device-local only**. Add them to no DTO. There must be a test asserting a
   bundle id cannot appear in an upload payload.
2. **Rust owns all judgement.** Swift reports facts, never conclusions. Swift
   must not decide a category, a purpose, or a confidence.
3. **No new user-visible colour, no gamification, no guilt.** Triage copy says
   what Velvt could not read. It never says the user wasted time.
4. **Additive migrations only.** No table rebuild, no CHECK widening.

## 1. Key on bundle identifier

`app_stable_key` (`key.rs:52`) hashes the app *name*. Names are localised,
change between versions, and differ from the marketing name (`Code`).

**Design — dual key, no destructive re-key.**

- `RawEventMessage` gains `bundle_id: Option<String>` (protocol v30).
- New `app_bundle_key = SHA256("velvt:abstraction-app-bundle-key:v1" ‖ len ‖ bundle_id)`,
  computed only when a bundle id is present. A **separate domain string** from
  `app_stable_key`'s so the two can never collide (`key.rs:32` sets the
  precedent).

  This domain string was drafted here as `velvt.app.bundle.v1` and reconciled to
  the implemented one, which follows the house pattern its two siblings already
  set (`velvt:abstraction-key:v1`, `velvt:abstraction-app-key:v1`). The pattern
  is what makes "one domain per fact" legible and is quoted in
  `persistence/sqlite.rs`'s collision argument; a domain string is also a
  persisted format, so the code is the side that could not move.
- `raw_event_buffer` gains `app_bundle_stable_id TEXT` (migration, additive).
- Override lookup order becomes: window override → **bundle override** → app
  (name) override → plugins. Existing name-keyed rows keep working untouched;
  new corrections write both where a bundle id exists.
- `personal_app_override` gains `bundle_key_hash TEXT` (additive, nullable,
  with an index). Do NOT change its primary key.

**Taxonomy.** `abstraction-taxonomy-mvp-1.json` gains an optional
`bundle_identifier` on a seed entry, and a new `seed_bundles` array for
bundle-only entries. Bump `category_taxonomy_version` to `mvp-2`. At minimum
add: `com.microsoft.VSCode`, `com.apple.dt.Xcode`, `com.todesktop.230313mzl4w4u92`
(Cursor), `com.apple.MobileSMS` (Messages — **not** `com.apple.iChat`, which is the retired iChat id), `com.apple.Notes`, `com.apple.TextEdit`,
`com.apple.freeform`, `com.apple.AddressBook`, `com.figma.Desktop`,
`com.tinyspeck.slackmacgap`, `com.hnc.Discord`, `notion.id`, `com.spotify.client`,
`md.obsidian`, `com.apple.Terminal`, `com.googlecode.iterm2`.

Also **delete the eight unreachable browser seed entries** and say so in a
comment — dead configuration that looks live is worse than none.

## 2. Fix the three correction bugs

All in `ipc/router.rs` and `persistence/sqlite.rs`.

- **`UpdateClassificationOverride` (router.rs:804) must also update the
  app-scope rule.** Today it writes only `save_personal_override`, so editing a
  saved rule leaves `personal_app_override` holding the old category for every
  other window of that app. Write both, exactly as `CorrectEventClassification`
  does, and only when the event is `app_scope_eligible`.
- **App-scope rules need a read path.** `search_personal_overrides`
  (`sqlite.rs:694`) joins `personal_override` to `abstraction_map` only, so a
  user can neither see nor delete what they taught at app scope. Add app rules
  to the correction-history result, marked with their scope so the UI can say
  "this app" vs "this window". `RemoveClassificationOverride` must be able to
  remove an app rule.
- **Remove and Reset must acknowledge.** Both return
  `correction_acknowledgment: None` (router.rs:846, :865). Give them the same
  acknowledgement treatment `Correct` and `Update` get (`router.rs:1281`), in
  the same voice: plain, factual, no praise.

## 3. Document types (`CFBundleDocumentTypes`)

50% of installed apps declare the file types they open. An app that opens
`public.source-code` is doing focus work; one that opens `public.movie` is not.
Higher precision than an App Store category, same zero cost, no TCC permission.

- `RawEventMessage` gains `document_type_ids: Vec<String>` — the declared UTI
  identifiers, each ≤ 64 chars, sorted and deduplicated by Swift, **bounded to
  256 entries**.

  An earlier draft said 24 with hard rejection. **A census killed that:** Xcode
  declares 152 document types and Preview 49, so a 24-cap would have rejected
  the list outright and silenced the signal precisely for the richest apps. A
  lexicographic prefix is worse than rejection — Xcode's first 24 sorted UTIs
  are all `com.apple.*` and would skew the majority rule.
  
  256 is above every app measured. If an app somehow exceeds it, send an **empty
  list**, not a prefix: abstaining is honest, a biased sample is not. The list
  is cached per bundle id and sent once per app per process, so size is not a
  hot-path concern.
- New Tier 1.5 `DocumentTypePlugin`, running **after** the name/bundle seeds
  and **before** the declared-category plugin. Conformance mapping:
  - `public.source-code`, `public.shell-script`, `public.c-*`, `public.objective-c-*`,
    `public.swift-source`, `public.python-script` → `FOCUS_WORK`
  - `public.plain-text`, `public.rtf`, `net.daringfireball.markdown`,
    `com.adobe.pdf`, `public.composite-content` → `REFERENCE`
  - `public.movie`, `public.audio`, `public.audiovisual-content` → `PASSIVE_CONSUMPTION`
  - `public.image` alone → no verdict (too ambiguous; a design tool and a photo
    viewer both claim it)
- Confidence `Medium`, tier `local_purpose_heuristic`, source a new
  `ClassificationSource::DeclaredDocumentTypes`.
- **A verdict requires an unambiguous majority.** If declared types map to more
  than one category and no category holds ≥ 70% of the mapped types, return
  `None`. Guessing here is worse than falling through.

## 4. Declared category — whitelist only

`LSApplicationCategoryType` has high recall (87% of live foreground apps) and
**poor precision**: `productivity` maps to four Velvt categories (Pages→focus,
Mail→communication, Reminders→task, Safari→reference) and `utilities` is mostly
SYSTEM except Terminal, which is focus work.

- `RawEventMessage` gains `declared_app_category: Option<String>`.
- New `DeclaredCategoryPlugin`, the **last** classifier before the embedding
  tier. Whitelist ONLY:
  - `public.app-category.developer-tools` → `FOCUS_WORK`
  - `public.app-category.video`, `.music`, `.entertainment` → `PASSIVE_CONSUMPTION`
  - `public.app-category.news`, `.books`, `.reference`, `.education` → `REFERENCE`
- Everything else — **explicitly including `utilities`, `productivity`,
  `business`, `graphics-design`, `photography`, every `*-games`, and
  `social-networking`** — returns `None`.

  `social-networking` was in an earlier draft of this contract mapped to
  `SOCIAL_FEED`. **That was wrong and a census proved it:** on this machine
  Messages (`com.apple.MobileSMS`) and WhatsApp (`net.whatsapp.WhatsApp`) both
  declare `social-networking`, and both are `COMMUNICATION`, not a feed. The
  behavioural difference matters — a feed is drift, a message often is not —
  so the value is polysemous exactly where it would do damage. Both apps are in
  the bundle seed list, which is where they should be classified. Put the excluded list in the source with the reason, so a later reader
  does not "complete" the mapping and destroy its precision.
- Confidence `Medium`, source `ClassificationSource::DeclaredAppCategory`.

## 5. Triage surface

Converts teaching from per-event and reactive to per-app and once.

**Backend.** New IPC pair (protocol v30):
- `RequestUnclassifiedTriage { lookback_days: u32 }` (Swift→Rust)
- `UnclassifiedTriage { entries: [...], window_days }` (Rust→Swift)

Each entry: `app_stable_id`, `display_name` (the local activity name Velvt
already holds), `seconds_observed`, `event_count`, `bundle_id` when known.
Rank by `seconds_observed` descending, **cap at 8**, and only include apps with
at least 5 minutes observed in the window — a list of thirty one-second
curiosities is not a task anyone will do.

Query over `raw_event_buffer` where the resolved category is `UNLOGGED`,
grouped by the app key, bounded to the retention window (14 days).

- `SetApplicationCategory { app_stable_id, category, activity_name }`
  (Swift→Rust) writes an app-scope override directly, with no event id — the
  whole point is that the user is teaching Velvt about an *app*, not correcting
  one moment. Must be idempotent and must emit an acknowledgement.

**UI.** A section in Settings → "Teach Velvt Your Apps" (the destination
already exists). Copy, exactly:

> Velvt could not read {n} apps you used this week.
> Tell it what they are and it will know from now on.

Each row: the app name, the time observed, and a category picker. No guilt
framing, no "wasted", no totals presented as a score. If the list is empty, say
so plainly and positively — that is the good state.

## Client responsibilities (Swift)

Where the event is assembled, collect from `NSRunningApplication`:
- `bundleIdentifier`
- `bundleURL` → `Contents/Info.plist` → `LSApplicationCategoryType`
- the same plist → `CFBundleDocumentTypes[].LSItemContentTypes`, flattened,
  deduplicated, sorted, bounded to 256 — and **empty, not trimmed**, for an
  application that exceeds it (§3 says why; this line said 24 in the draft §3
  corrected)

Reading an app's own `Info.plist` requires **no TCC permission** — it is
world-readable metadata shipped by the developer. Cache per bundle id for the
process lifetime; do not re-read the plist on every event. On any failure
(missing key, unreadable plist, sandbox denial) send `nil`/empty and carry on:
absent metadata must degrade to today's behaviour exactly.

## Protocol

Bump `PROTOCOL_VERSION` 29 → **30**. Update `proto/version`, both xcconfigs,
`proto/CHANGELOG.md`, and add schema files for the new messages. The
`testProtocolVersionSourcesMatch` guard will catch a missed xcconfig.

## Definition of done

- `cargo test --all-targets` and `swift test` both green.
- A test asserting no new field can reach an upload payload.
- A test asserting `Code` (bundle `com.microsoft.VSCode`) classifies as
  `FOCUS_WORK` — the case that motivated all of this.
- A test asserting `utilities` and `productivity` return `None`.
- A test asserting an app with no metadata behaves exactly as it does today.


## Appendix — measured bundle identifiers (this machine, 2026-09-23)

Verified by reading each app's own `Info.plist`. Use these verbatim; a wrong
bundle id is a silent classification failure with no error anywhere.

| App | Bundle id | Declared category | #UTIs |
|---|---|---|---:|
| Visual Studio Code | `com.microsoft.VSCode` | developer-tools | 1 |
| Xcode | `com.apple.dt.Xcode` | developer-tools | 152 |
| Terminal | `com.apple.Terminal` | utilities | 7 |
| Messages | `com.apple.MobileSMS` | social-networking | 3 |
| WhatsApp | `net.whatsapp.WhatsApp` | social-networking | 10 |
| Slack | `com.tinyspeck.slackmacgap` | business | 0 |
| Notes | `com.apple.Notes` | productivity | 11 |
| TextEdit | `com.apple.TextEdit` | productivity | 15 |
| Freeform | `com.apple.freeform` | productivity | 0 |
| Contacts | `com.apple.AddressBook` | productivity | 1 |
| Obsidian | `md.obsidian` | productivity | 2 |
| Preview | `com.apple.Preview` | productivity | 49 |
| Safari | `com.apple.Safari` | productivity | 0 |
| Google Chrome | `com.google.Chrome` | *(none)* | 17 |
| Spotify | `com.spotify.client` | music | 0 |
| zoom.us | `us.zoom.xos` | video | 0 |
| Photos | `com.apple.Photos` | photography | 7 |
| News | `com.apple.news` | news | 0 |

Three things this table is evidence for: **VS Code declares only
`public.folder`**, so document types alone would not classify it — the bundle
seed and `developer-tools` both must; **Terminal declares `utilities`**, which
is why that value is excluded rather than mapped to SYSTEM; and **zoom.us
declares `video`**, which the whitelist maps to PASSIVE_CONSUMPTION even though
a Zoom call is COMMUNICATION. Zoom is covered by an existing name seed, which
runs first — but it is a live example of why the whitelist must stay small.
