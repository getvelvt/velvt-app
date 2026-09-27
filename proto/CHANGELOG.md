# IPC Protocol Changelog

## Version 33 - 2026-09-27

- `unclassified_triage` now lists browser sites beside applications, as one
  list. Each entry is `{kind, stable_id, display_name, seconds_observed,
  event_count}`: `kind` is `application` or `site`, and `stable_id` (64
  lowercase hex, the salted application or site key) replaces
  `app_stable_id`. The list is ranked by `seconds_observed`, longest first,
  then applications ahead of sites, then by `stable_id`, and still holds at
  most 8 entries of at least 300 seconds over the requested 1 to 14 days. A
  site is one a browser tab was on that Velvt could not confidently
  categorize and holds no rule for; its `display_name` is its hostname, read
  from `local_site_name` (migration 0040).
- `unclassified_triage.entries[].display_name` is required and nullable. For
  an application Velvt holds no local name for it is `null`; until now Rust
  sent the literal `Unnamed application`, the client sent it back as the
  rule's `activity_name`, and every later window of the application was
  labelled with it. The client words the row itself and sends no name.
- Added `set_site_category {site_stable_id, category, activity_name?}` (Swift
  to Rust), the site-list sibling of `set_application_category`: the rule
  covers every page of the site in every browser. Validated the same way (64
  lowercase hex, else `invalid_site_stable_id`; a taxonomy category, else
  `invalid_classification_category`; a name of 1 to 48 characters with no
  control characters, else `invalid_local_activity_name`;
  `classification_correction_unavailable` and
  `classification_correction_persistence_failed` as for applications).
  Answered with `menu_status` carrying a Rust-authored
  `correction_acknowledgment` ("Got it — every page of <the typed name, or
  this site>, in every browser, counts as <category> from now on."), which
  never quotes the hostname. Nothing about the site is sent anywhere (the
  `menu_status` reply may refresh cloud readiness, as any status poll does),
  and the site's stored hostname is deleted when it is taught.
- `correction_history_page.items[].scope` and
  `menu_status.correction_history[].scope` gain `site`. A site rule's
  `stable_id` is its site key and its `local_label` is only a name the user
  typed, never the hostname. `remove_classification_override` and
  `update_classification_override` take a site rule's key as they take a
  window or app rule's. `remove_classification_override` tries the window
  rule, then the app rule, then the site rule. `update_classification_override`
  edits the app rule if one exists for the key, else the site rule, else
  treats the key as a window rule; an app or site rule that cannot be read
  counts as absent. An edit of a site rule writes `local_activity_name` as
  sent, so an edit with none clears the name the rule had.
- `set_application_category` resolves the application's bundle key by looking
  it up for that application key, not by searching a 14-day top-8 list, which
  missed an application the client had been shown on a 7-day list and keyed
  its rule on the name alone.
- Added the needs-a-category card and reminder, decided and worded in Rust
  (`category_prompt`, `CATEGORY_PROMPT_POLICY_VERSION` 1):
  - `request_category_prompt {utc_offset_seconds}` (Swift to Rust, -64800 to
    64800) is always answered with `category_prompt {prompt_id?, card?,
    notification?}` (Rust to Swift). An empty payload means no card. `card`
    is `{title, body, primary_action, secondary_action, entry_count}` and
    `notification` is `{title, body}`; `prompt_id` (64 lowercase hex: 32
    random bytes, kept while the entries the card counts stay the same and
    drawn again when they change, so it says nothing about any entry) is
    present exactly when `card` is.
  - The card counts the first eight entries of the last seven days' list,
    shows while any of them is unanswered, and never shows while a work block
    is active or paused. The notification additionally needs: not Velvt's
    quiet hours at the request's `utc_offset_seconds` (not an offset stored
    with an earlier Focus transition, which Clear Local Work Blocks removes),
    macOS Focus not known to be on, no reminder yet on the
    client's local date, an entry among those eight that no earlier reminder
    or answer has reached, and no backoff pause (three reminders in a row,
    each posted within the last 30 days and with no `opened` answer before
    the next, pause reminders for seven days after the latest). "New" is
    judged against every entry above the list's floor, not only the eight: a
    reminder and an answer reach the entries listed below the eight too, so
    one that moves up into the eight is not new. A notification is claimed
    in the same transaction that records it and is never handed over twice,
    whether or not the client posts it.
  - `acknowledge_category_prompt {prompt_id, response}` (Swift to Rust),
    `response` `opened` or `not_now`. Either answers every entry that card
    covered, even when it arrives after the list has moved on, so the card
    stays away until an entry no answer has reached is among the eight;
    `opened` also ends a run of unopened reminders. An id Rust holds no card
    for answers no entry. Answered with `category_prompt` as the card now
    stands, never with a notification; a malformed `prompt_id` is refused
    with `invalid_category_prompt_id`.
  - Copy is counts only, never a name, a hostname or a time, because macOS
    Notification Center keeps a notification's text. This is the third
    notification kind, beside the drift offer and the daily insight.
- Daily summaries built on this Mac, for the Patterns card:
  - `request_latest_history` gains a required `utc_offset_seconds` (-64800 to
    64800, clamped in Rust as the other offsets are).
  - `history_payload` gains a required `source`, `cloud` or `this_mac`. Rust
    answers cloud-first: signed in, the cloud's history is sent whenever it
    can be read. Signed out, or when the read fails for any reason (a
    timeout, a non-200, an unparseable body, no rows) or the cloud's week has
    no ready day while this Mac has one (uploads can stall for days while
    collection goes on, and the cloud then answers with empty days; such a
    week is also never pushed by the fetch scheduler), Rust builds the
    summaries from this Mac's own retained events (`dashboard.rs`
    `local_daily_history`) and sends them with `source: this_mac` instead of
    `cache_empty(backend_unavailable)`. They cover up to 14 local calendar
    days at the request's offset, oldest first, the days the Daily Activity
    chart draws; each field mirrors velvt-core's daily summary as closely as
    local evidence allows, and the doc comment on `local_daily_history` lists
    every place it differs. `focus_score` and `fragmentation_score` are null,
    `baseline_status` is `unavailable`, `baseline_comparison` is
    `{"status": "unavailable"}`, `type_proportions` is empty,
    `confidence_level` is `low` on a ready day and `none` on a `no_data` one,
    and a day is `ready` from a minute of active time.
  - `history_payload.days` is the number of rows the payload carries. It was
    the number requested, and the cloud answers at most 7 whatever it is
    asked, so a 7-row answer went out labelled 14 and Swift padded seven empty
    days in front of it. The shaper now refuses a `days` that differs from the
    row count.
  - `cache_empty` gains the reason `local_history_unavailable`: the only way a
    history request is now answered with `cache_empty`, when the summaries
    could not be built on this Mac either.
  - The client asks for history signed in or not, and only once the stored
    session has been handed to the service on the connection, so a request
    can no longer reach the service ahead of `auth_session`; and before the
    insight, so the history does not wait on the insight's cloud read. It
    asks again when the account settles into a different state and, while
    the last answer was not the cloud's (a `this_mac` history or
    `cache_empty`), when Patterns appears and on the menu status's 60-second
    cadence at most every 10 minutes. A `cloud` history is not asked for
    again: the service pushes one each time its fetch scheduler fetches one.
  - Signed in, the service asks the cloud for at most 7 days, the most it
    answers, so its cache can serve a request for 14. After a failed cloud
    read it stops waiting on the cloud for history: requests are answered
    from the cache once the fetch scheduler has refilled it, and otherwise
    built on this Mac at once, until then or a session change. The
    connection reads one message at a time, and each read that ran to the
    10-second HTTP timeout held every other message back.
- Compatibility: a v32 triage entry does not decode as a v33 one and a v32
  service rejects `set_site_category` and the two prompt requests, so the
  handshake requires 33 on both sides. `request_unclassified_triage` is
  unchanged. A v32 `request_latest_history` has no offset and a v32
  `history_payload` no source; neither decodes on the other side.
- Privacy: nothing new leaves the Mac. The hostname of a site that needs a
  category crosses the local socket as display text, like an application's
  local name; neither is uploaded, logged, or kept by the client. The prompt's
  record (migration 0041) holds salted keys, random card ids, dates, times
  and counts. A history built on this Mac is computed on request from
  `raw_event_buffer`, crosses only the local socket, and is stored nowhere;
  a signed-out Mac's history request makes no network request.

## Schema correction: menu_status sources - 2026-09-27 (no wire change; the protocol stays 32)

- `menu_status.queued_events[].classification_source` listed `seed`,
  `heuristic`, `embedding`, `user_rule` and `fallback`. Rust has emitted
  `declared_document_types` and `declared_app_category` there since protocol
  30, when the two tiers that read an application's own declarations
  arrived, and the Swift decoder has accepted both since then. The schema now
  lists them. No Rust or Swift type changed and the bytes on the socket are
  the same.
- `rust-service/tests/emitted_payload_schema.rs` now validates a real
  `menu_status`, with one queued event per stored source and an app rule in
  `correction_history`, against the schema, so the enum cannot drift again
  unnoticed.

## Browser tabs classified by their site - 2026-09-27 (no wire change; the protocol stays 32)

- Rust now classifies a browser tab by the site its `focused_document_url`
  names: a host in a compiled-in site table decides the tab's category at high
  confidence whatever its title says, and a site the table does not name is
  classified by its own hostname's labels when they agree. The title-keyword
  rules (browser-context and purpose) no longer decide a tab whose site can be
  read. They still decide a tab with no readable site (no URL, `localhost`, an
  address), and the purpose rules still read every window that is not a
  browser's. A rule about a site
  (migration 0040) applies in every browser; no command writes one yet.
- Every field, bound and value on the socket and in the upload is unchanged:
  the upload's `classification_tier` is still one of `exact_match`,
  `local_purpose_heuristic`, `embedding_similarity` and `fallback`, and no new
  status or source reaches `menu_status`.
- Drift policy version 5 (`DRIFT_POLICY_VERSION` 4 → 5). The gate's code,
  constants, branches and timing are version 4's. A browser tab that was the
  ambiguous browser prior, or UNLOGGED because two keyword rules disagreed, is
  now confident evidence when its site is seeded or read (unless it is filed
  as SYSTEM, which the gate never counts). The other way, a tab that title
  keywords made confident under version 4 is, short of a rare Tier 2 match,
  the ambiguous prior under version 5 when its site can be read but is neither
  seeded nor inferred (a Jira ticket on `*.atlassian.net`, the LinkedIn
  feed). Browser time moves both ways, mostly from unclear to a category, so
  the anchor, the switch counts and the decision points differ from version
  4, and the two are never pooled.
- Privacy: nothing new is sent. On disk, the hostname of a site Velvt could not
  categorize is kept in `local_site_name` (migration 0040) so it can be named
  when Velvt asks about it; `PRIVACY.md` describes the table and its 14-day
  horizon.

## Application-level dwells - 2026-09-26 (no wire change; the protocol stays 32)

- Swift now reports a dwell for an application it cannot observe at window
  level, most often one with no focused or main window when it is activated
  (AX error -25212). The `raw_event` carries the application's `app_name`,
  `bundle_id` and declared metadata, an empty `window_title` and no
  `focused_document_url`, in progress when the dwell begins and closed when it
  ends, like any other. Before, Swift sent nothing for such an application, and
  the dwell before it stayed open and absorbed its time.
- Every field and bound is unchanged: `window_title` was always a string that
  could be empty (an untitled window), and Rust has always classified an empty
  title through the application-level rungs. The schema now says so.
- Drift policy version 4 (`DRIFT_POLICY_VERSION` 3 → 4). The gate's constants,
  branches and timing are version 3's. It now receives a departure to such an
  application as an observation, so the decision points, switch counts and
  anchor differ from version 3, and the two are never pooled.
- Privacy: nothing new is sent or stored. The raw values are ones `raw_event`
  already carries, over the local socket only, and what Rust stores and
  uploads for the dwell is what it stores and uploads for any other.

## Version 32 - 2026-09-26

- Added optional `in_progress` (boolean) to `raw_event` (Swift to Rust). True
  when the dwell has only just begun: the activity became frontmost at
  `occurred_at` and nothing has been measured yet, so `duration_seconds` is 0
  and means nothing. The client sends each dwell twice: in progress when it
  begins, and closed, with its measured duration, when the next one begins.
  The closed report of one dwell always precedes the in-progress report of the
  next. An in-progress report is sent live or not at all: it is never buffered
  while the socket is down and never replayed.
- Why it exists: a dwell was reported only when it ended, so the in-block drift
  gate learned about a departure at the moment the person came back, and the
  offer it pushed was withdrawn as `returned` by the very next report. On the
  founder's Mac on 2026-09-25 the offer existed for about one second of wall
  time, and the notification was never posted. With the in-progress report the
  gate sees the departure while it is happening.
- Rust: an in-progress report is fed to the work-block gate and nothing else.
  It is never written to `raw_event_buffer` and never enqueued for upload, and
  outside an active block it is not even classified. Its classification is
  kept in memory until the closed report of the same dwell arrives and is
  reused there, so each dwell is still classified exactly once. The gate
  evaluates at the dwell's `occurred_at`, which the closed report carried too,
  and the closed report then lands on the observation the in-progress one
  opened and is a no-op there. When a pause, a sleep or a service restart
  closed that observation first, the closed report re-opens the ledger at the
  resume and is not evaluated again, so one dwell is one decision.
- Drift policy version 3 (`DRIFT_POLICY_VERSION` 2 → 3). The gate's constants
  and branches are unchanged, and on dwells that close inside the block with
  no boundary between their two reports the decisions and their timestamps
  are too; only the wall-clock moment moves. Elsewhere the decision points
  differ: a dwell still in progress when the block ends or the Mac sleeps is
  now decided on (version 2 never saw it), and a dwell interrupted by a pause
  or a restart is decided on when it began rather than at the resume. An
  offer now reaches the person while they are away. Decisions under the two
  versions are never pooled.
- Acknowledged like any `raw_event` (`raw_event_ack`, `accepted`).
- Compatibility: a closed dwell carries no `in_progress` key, byte for byte
  what a protocol-31 client sent. A pre-32 service rejects the key
  (`deny_unknown_fields`), which is why this is a protocol bump.
- Privacy: no new field carries raw data. The raw values are the ones
  `raw_event` already carries, sent at the start of the dwell as well as at its
  end, over the local socket only.
- Transport, no wire change: the service's frame reader is now cancel-safe.
  It ran inside a `select!` against pushes, and a frame that was half read
  when a push arrived was dropped, so the rest of it was answered with
  `malformed_message`. Two frames sent back to back lost the second whenever
  the first queued a push, which is the shape the in-progress report creates at
  every activity switch.

## Schema corrections: what the service emits - 2026-09-25 (no wire change; the protocol stays 31)

- `local_dashboard.daily_activity` declared `minItems`/`maxItems` 7, the count protocol 20
  introduced. Rust has sent `DAILY_ACTIVITY_DAYS` = 14 rows since 2026-08-27
  (`rust-service/src/dashboard.rs`; shipped in 1.0.9 and 1.0.11 at protocols
  28 and 30), and the outbound shaper rejects any other count. The schema now
  says 14. No Rust or Swift type changed and the bytes on the socket are the
  same as before; the Swift client renders whatever count it receives.
- A `daily_activity` segment listed `representative_event_id`, `stable_id` and
  `suggested_name` as required. Rust omits each of them when it has no value
  (`skip_serializing_if` on `LocalDailyActivitySegment`), and a segment for a
  seed-matched application has no `suggested_name`, so a real payload broke
  the schema on the first day with focus work in it. They are now optional.
- `work_block_state` listed `active_intervention` as required, while its own
  `$comment` said it is present only while an offer is unanswered, which is
  what Rust does (`skip_serializing_if`). It is now optional.
- The Swift decoders already read all four as optionals, so nothing on either
  side changes.
- Why the conformance test below missed these: it validates instances
  generated from the schema, so a 7-row instance agreed with a 7-row schema
  and every generated instance filled in every optional field.
  `rust-service/tests/emitted_payload_schema.rs` now drives the real router
  (real events, an active block), validates the `local_dashboard` and
  `work_block_state` payloads it emits against the schemas, and fails if the
  schema's row bounds and `DAILY_ACTIVITY_DAYS` differ. The validator it uses
  is the one `schema_conformance.rs` uses, moved to
  `shared-types/tests/support/json_schema.rs` so both share it.

## Schema corrections - 2026-09-25 (no wire change; the protocol stays 31)

`rust-service/shared-types/tests/schema_conformance.rs` now builds a maximal and
a minimal instance of every file in `proto/schema/`, parses each as a Rust
message, and validates what Rust serializes back. Its first run found four
schemas that described a wire no Rust build ever produced. Each is corrected to
match what Rust sends; no Rust type changed, so the bytes on the socket are the
same as before.

- `unclassified_triage`: removed the optional `entries[].bundle_id`. The Rust
  entry deliberately carries `app_stable_id` as its only identifier and never
  sent a bundle key hash; Rust looks the bundle identity up itself when
  `set_application_category` comes back. The Swift type no longer declares it.
- `history_payload`: the longest-stretch field on the socket is
  `longest_uninterrupted_seconds`, the name Rust has sent since the field was
  added on 2026-07-18. The schema said `focus_seconds` (the cloud API's name,
  which Rust renames when it parses the API response), and the Swift decoder
  read `focus_seconds` too, so History showed a longest stretch of 0. The
  Swift decoder now reads `longest_uninterrupted_seconds`.
- `menu_status`: declared `correction_history[].scope`, which Rust has sent
  since protocol 30 alongside `correction_history_page`'s.
- `acknowledged`: the payload is `null`, which is how serde encodes Rust's
  unit struct. The schema said an empty object.

## Version 31 - 2026-09-25

- Added `anchor_category` to `work_block_state` (Rust to Swift), top level,
  required and nullable like `current_category`. It is the broad category the
  drift gate treats as the block's anchor --- the one holding the most
  confidently observed, closed time --- computed by the same function the gate
  calls, so it is the value the gate measures departures from and the value an
  offer records. Null outside an active or paused block, and until a confident
  observation has closed; a finished block's category stays
  `result.safe_evidence_category`, which is gated on coverage.
- Why it exists: the rule "defer while `current_category` is the anchor" could
  not be applied by any local IPC client, because the anchor was on the wire
  only inside `active_intervention`, which exists only while an offer is
  pending. The workspace's Claude Code hook approximated it with "confidently
  in focus work".
- Compatibility: a pre-31 payload has no key at all, and both DTOs decode that
  as `None`/`nil`. A client that reads the payload directly can tell "no anchor
  yet" (null) from "a service older than 31" (absent).
- A category label only, and local IPC only. It carries nothing
  `current_category` does not already carry --- no app identity, window title,
  URL, or intention --- and no upload DTO has a field it could occupy.

## Version 30 - 2026-09-23

- Added `bundle_id` handling, `declared_app_category` and `document_type_ids`
  to `raw_event` (Swift to Rust): what the application itself declares about
  what it is, read from its own `Info.plist`. Facts, never conclusions --- Swift
  reports the strings and Rust decides whether any of them mean anything.
  `document_type_ids` is bounded to 256 identifiers of at most 64 characters,
  deduplicated and sorted by the client, and an oversized list is sent empty
  rather than truncated: a truncated list is a set the application never
  declared, and classifying on it would be worse than classifying on nothing.
  256 is above every application measured --- Xcode declares 152, Preview 49.
  An over-bound list that arrives anyway costs the declaration and nothing
  else: the event is stored with its duration intact.
  Absent metadata --- a missing key, an unreadable plist, an older client ---
  must classify exactly as it did before these fields existed.
- Why it exists: on a real machine with 107 installed applications, 63% of them
  classify as UNLOGGED, and `is_confident_evidence` excludes UNLOGGED, so that
  time reaches neither the drift gate nor the anchor. The measured cause was not
  weak inference but a weak key. `app_stable_key` hashes the name macOS reports,
  and that name is localized, changes between releases, and is often not the one
  anyone would recognise: `NSRunningApplication.localizedName` for Visual Studio
  Code is literally `Code`, which matched no taxonomy entry at all. A bundle
  identifier is none of those things. Stored as a hash under its own domain
  separator (`raw_event_buffer.app_bundle_stable_id` and
  `personal_app_override.bundle_key_hash`, migrations 0033 and 0034), so the
  identifier itself is never persisted, and added beside the name key rather
  than instead of it --- every name-keyed correction already on disk keeps
  working untouched.
- Added `request_unclassified_triage` (Swift to Rust) and `unclassified_triage`
  (Rust to Swift): the bounded list of applications Velvt observed but could not
  read, so teaching it becomes per-application and once rather than per-event
  and reactive. Each entry carries only facts --- the local name Velvt already
  holds, seconds observed, and event count. (Corrected 2026-09-25: this entry
  and the schema also listed an optional bundle key hash, which the Rust type
  never had and never sent.) Ranked by observed time, capped at 8, and floored at five minutes in the
  window: a list of thirty one-second curiosities is not a task anyone will do.
  An application the user has already taught leaves the list, and an empty list
  is the good state. No category, no guess, and no total presented as a score.
- Added `set_application_category` (Swift to Rust): the one-tap answer from that
  list. It carries no event id on purpose --- the user is telling Velvt what an
  application is, not correcting one moment of it --- and saving the same answer
  twice is the same as saving it once.
- Added `scope` (`window` or `app`) to each `correction_history_page` item.
  Until now the history listed window rules only, so an app-scoped rule could be
  neither seen nor removed: removing the window rule left the engine falling
  through into the surviving app rule and returning the same category and the
  same typed name on the next event. `stable_id` means an abstraction stable id
  for a window rule and the application's own key hash for an app rule, so a
  client cannot act on the id without reading the scope. Absent on an older or
  stored payload, where it defaults to `window` --- which is what every rule
  listed before this version was.
- Local IPC surface only. None of it is uploadable, structurally rather than by
  filtering: `upload/dto.rs` implements `Serialize` for `BatchEventPayload` by
  hand and emits exactly event_id, occurred_at, abstraction_type,
  abstraction_type_version, classification_tier and a payload of
  duration_seconds and category. There is no field a bundle identifier, a
  declared category, a document type or a triage entry could occupy.

## Version 29 - 2026-09-22

- Added `intervention_card_seen` (Swift to Rust): the in-app drift card was
  actually rendered on screen. A delivery fact and never a response --- the
  payload carries one field and has no place a user answer could sit, so a
  sighting cannot be widened into an outcome nobody gave. Silence is still
  recorded only when a block ends unanswered, and is still never inferred
  from a card or notification disappearing.
- Why it exists: `outcome = 'no_response'` conflated a person who saw the
  offer and said nothing with a person the offer never reached. Those are
  evidence about the action and evidence about delivery respectively, they
  call for opposite fixes, and the pre-registered denominator counted them
  identically. Stored as `work_block_intervention.card_seen_at` (migration
  0032), nullable and orthogonal to `outcome`, so no existing definition
  changes. NULL means "never observed on screen", which includes every row
  written before the migration --- those are unknown, not unseen.
- Local IPC surface only. The sighting is never uploaded; no DTO has a field
  it could occupy.

## Version 28 - 2026-08-07

- Added `request_demotion_state` (Swift to Rust) and `demotion_state` (Rust
  to Swift): the deterministic, versioned auto-demotion policy over the
  rolling wrong-intervention counter, disclosed as a feature and inspectable
  on demand. The payload carries only the two bounded counts the rate is
  computed from, the versioned policy constants (threshold percent, minimum
  sample, window days, threshold and re-promotion policy versions), the
  current state, the demotion instant when demoted, and Rust-authored
  disclosure copy. No transition history or timeline is representable, and
  the payload exists on the local IPC surface only — it is never uploaded.
- Added `reset_intervention_demotion` (Swift to Rust): the user's explicit
  one-tap resume from the demoted state. Resetting restarts the demotion
  evaluation window from the reset instant; it never edits or discards the
  underlying outcome record, and the wrong-intervention counter itself is
  untouched.
- Added `request_weekly_digest` / `acknowledge_weekly_digest` (Swift to
  Rust) and `weekly_digest` (Rust to Swift): the weekly receipts digest for
  the most recent completed local week, pull-delivered like invitations and
  held during quiet hours and Focus/DND. Every count is read from the same
  stored aggregates the local metrics use; recoveries and completions lead,
  the wrong-intervention count appears exactly once, and no streak, chain,
  or failure tally is representable. Local IPC surface only — never
  uploaded.
- Added `request_intervention_explanation` (Swift to Rust) and
  `intervention_explanation` (Rust to Swift): the one-tap "explain this
  nudge" affordance. Deterministic code selects the claim, evidence, and
  tone from the stored intervention record; the response is exactly one
  grounded sentence that cannot exceed that evidence. The request accepts no
  user text, and no reply, follow-up, or thread exists anywhere on this
  surface (D7). The tap is counted locally as a coarse, content-free weekly
  bucket only.

## Version 27 - 2026-08-07

- Added `request_initiation_invitation` (Swift to Rust): asks the
  deterministic, versioned initiation policy whether one invitation is
  pending. The request carries only the client's UTC offset; every gate
  (good hours, minimum samples, daily cap, quiet hours, Focus/DND, active
  block, opt-out, backoff) is owned and enforced in Rust, and below any
  minimum-sample gate the answer is silence — never a default window.
- Added `initiation_invitation` (Rust to Swift): at most one daily
  invitation to a 25-minute soft start. The payload is schedule-free by
  construction — no good-hours window, weekday, hour bucket, or timing
  evidence is representable — and carries only an opaque invitation id, the
  registered `soft_start_25` action, registered copy, the block duration,
  and the policy version.
- Added `dismiss_initiation_invitation` (Swift to Rust): the one-tap
  dismissal. Dismissal only ever reduces future invitations under the
  versioned backoff policy; repeated dismissal silences invitations
  entirely for a versioned interval.
- Added optional `invitation_id` to `start_work_block`: one tap on an
  invitation starts a declared block through the existing declaration path.
  Rust validates the id and records a content-free origin marker locally;
  the marker never appears in any IPC, cloud, log, or telemetry payload.
- Added `set_initiation_settings` / `request_initiation_settings` (Swift to
  Rust) and `initiation_settings` (Rust to Swift): the single Rust-owned
  opt-out for invitations. Opting out silences invitations and changes
  nothing else.
- Registered `soft_restart_10` as the second closed-registry action:
  `accept_work_block_recovery.action_id` and the work-block result's
  `next_action.action_id` now admit `protect_next_10` or `soft_restart_10`.
  The in-block drift offer (`active_intervention`) still only ever carries
  `protect_next_10`.

## Version 26 - 2026-08-07

- Added `focus_state_changed` (Swift to Rust): a coarse system Focus/DND
  transition report carrying only `active`, the transition time, and the
  client's UTC offset. Swift observes; Rust owns the Focus/DND evidence
  record and every decision derived from it. The Focus mode's name,
  configuration, and schedule are structurally unrepresentable in this
  message and never cross IPC.
- Added the Focus/DND outcome enum values `completed_under_dnd` and
  `delivery_suppressed_dnd`. They appear in the new optional
  `dnd_outcomes` array of the work-block result: a block completed while
  DND was active records `completed_under_dnd` (a success everywhere a
  completed block counts), and each mid-block nudge held because DND was
  active records `delivery_suppressed_dnd`. Held nudges are never delivered
  by another channel, never retried mid-block, and reconcile after the
  block as counts only.
- Added optional `reconciliation` to the work-block result: at most one
  Rust-authored calm post-block line noting what was held. Analyst voice;
  no reference to what the user missed.
- Added `quiet_hours_offer` (Rust to Swift): a deterministic, versioned
  next-morning offer produced by the late-night DND pattern rule. It is an
  offer, never a workaround, and carries only the rule version, a distinct
  day count, the proposed local window, and Rust-authored copy.
- Added `respond_quiet_hours_offer` (Swift to Rust): one-tap acceptance
  configures Velvt's own quiet hours; a decline is remembered locally and
  the offer is not re-asked for a versioned interval.

## Version 25 - 2026-08-08

- Added `was_focused` to `ReportInterventionOutcome.response`. The existing
  vocabulary could not express a false positive: `dismissed` means "not now",
  `not_helpful` concedes the drift happened, and `wrong_classification`
  disputes a label rather than the judgment. Only `was_focused` says the offer
  should never have fired, which makes it the ground-truth input to the
  wrong-intervention rate.
- Added `salience` (`normal` | `quiet`) to `active_intervention`. A quiet offer
  renders the in-app card without sending a notification. Salience only ever
  decreases: it drops after the user pushes an offer away and is restored by an
  offer that lands, never by continued drift.
- Added optional `correction_acknowledgment` to `MenuStatus`. It is set only on
  the status returned by a correction command, never on a polled one, so a
  correction is visibly believed without the confirmation reappearing every
  refresh.
- Documented `active_intervention` in `schema/work_block_state.json` and added
  the missing `schema/report_intervention_outcome.json`; both shipped in 24
  without a schema entry.

## Version 24 - 2026-07-31

- Added `ReportInterventionOutcome` so the user's explicit response to an
  in-session drift offer reaches the service. Silence is deliberately not
  representable: it is recorded when the block ends, never inferred from a
  notification disappearing.
- Added optional `active_intervention` to the work-block snapshot so an offer
  renders in-app. The in-app surface is the primary path; an OS notification
  depends on authorization and is suppressed by Focus.
- Aligned the intervention action registry with 0.1.5 Scope 4:
  `return_to_anchor` is now `protect_next_10`.

## Version 23 - 2026-07-29

- Added optional `focused_document_url` to the sole raw-event IPC message so
  event-driven browser tab changes can be distinguished when Accessibility
  exposes the focused document.
- The URL is local-only raw input. Rust reduces it to a validated hostname for
  classification and stable local identity, then discards it before SQLite,
  upload, telemetry, logging, dashboard, or delivery DTO construction.
- Events collected before authentication are retained for local first value
  with `upload_eligible = false` and never enter the cloud upload queue.

## Version 22 - 2026-07-26

- Added searchable, offset-paginated device-local correction history with a
  maximum page size of 20 so the popover never loads the full local rule set.
- Added local-only editing of persisted activity aliases and categories after
  the associated upload event is no longer present.
- Added bounded local activity-row context for inline corrections. Stable local
  identifiers, suggested names, and bundle identifiers remain local IPC only.

## Version 21 - 2026-07-25

- Added a bounded device-local correction history to `menu_status`, backed by
  persisted personal overrides rather than the transient upload queue.
- `correct_event_classification` can carry an optional local activity name.
  The name stays in the local override/mapping path and is excluded from cloud
  correction requests, upload DTOs, telemetry, logs, and crash diagnostics.

## Version 20 - 2026-07-20

- Replaced the local dashboard contract with exactly two analytical DTO branches:
  explicit-work-block Focus Fragmentation and seven-row Daily Activity.
- Rust now owns 60-minute clipping, deduplicated category transitions, version-1
  five-minute switching clusters, recoveries, coverage, like-for-like comparison,
  local day boundaries, local display-label aggregation, `Other`, and grounded
  segment evidence. Swift only renders these bounded aggregates.
- Local display labels remain local-IPC-only and are absent from cloud, upload,
  telemetry, logging, and crash-diagnostic contracts.

## Version 19 - 2026-07-18

- The device-local dashboard now includes a Rust-authored early Today signal
  with finite evidence progress, actual observation bounds, broad-category
  aggregates, and one modest local action.
- The early signal contains no raw app names, titles, URLs, filenames, paths,
  contacts, local labels, or inferred intentions.

## Version 18 - 2026-07-18

- Daily insights now include the exact privacy-safe evidence layers, approved
  emotional stage, baseline comparison, and suggested action used to render
  the insight. Raw local activity remains forbidden.
- Daily history now carries aggregate focused seconds, meaningful switch
  count, and longest uninterrupted seconds for the Today surface.
- Early-baseline insight payloads remain inspectable but do not trigger a
  user notification.

## Version 17 - 2026-07-17

- Raw activity events now include a bounded, locally measured `duration_seconds`
  dwell interval. This remains local-only until the Rust privacy boundary
  abstracts and uploads the event.
- Both the Swift collector and Rust privacy boundary enforce the 1,800-second
  maximum so unattended time cannot be counted as active use.

## Version 16 - 2026-07-17

- Added the bounded, local-only recent-activity dashboard request and snapshot.
- Dashboard rows contain safe categories and aggregate metrics only; raw app
  names and window titles never cross the Rust privacy boundary.

## Version 15 - 2026-07-16

- Added the device-local meaningful-work loop: start, pause, resume, end,
  lifecycle, recovery, clear-data, and state request/response messages.
- `work_block_state` is Rust-authored and versioned independently at state
  version 1. Free-form intention is permitted only on local IPC and is absent
  from cloud/upload/cache/notification contracts.
- Added a singular bounded `next_action` to the safe local session result;
  exactly one 10-minute recovery action is representable.

## Version 14 - 2026-07-16

- Queued-event summaries now distinguish classification status, confidence,
  and provenance while retaining the legacy tier for compatibility.
- Added device-local removal and reset operations for personal classification
  rules. Neither operation transmits a raw target or local mapping key.

## Version 13 - 2026-07-15

- Added `correct_event_classification` for device-local personal overrides and historical sync.
- Queued-event summaries now carry event/stable identifiers and classification provenance.

## Version 12 - 2026-07-08

- Added required upload diagnostics to `menu_status`: `upload_status`,
  `last_upload_error_code`, `next_upload_attempt_at`,
  `pending_upload_batch_count`, `failed_upload_batch_count`, and
  `rejected_upload_batch_count`.
- This is a coordinated protocol bump because `menu_status` uses a closed
  schema and the new diagnostics fields are required by Rust and Swift DTOs.

## Version 11 - 2026-06-25

- Added `auth_session` client message: the host client supplies a locally
  persisted device auth session to Rust after connection. Rust keeps it in
  memory only.
- Added `auth_session_updated` server message: Rust tells the host client to
  persist refreshed or reissued auth credentials in platform credential storage.
- Added `device_id` to `auth_success`; signup/login now returns a device-bound
  session for host-side persistence.
- Added optional `user_access_token`, `user_refresh_token`, and
  `user_expires_at` to `auth_success`, `auth_session`, and
  `auth_session_updated`. Device tokens remain the default credentials for API
  calls; user tokens are replayed only so Rust can refresh user auth and reissue
  device-bound credentials after relaunch.

## Version 10 - 2026-06-21

- Added optional `local_label` to `menu_status.queued_events`. It is a
  device-local display field for the queue inspector and must never be copied
  into cloud upload payloads or logs.

## Version 9 - 2026-06-21

- Added `flush_upload_queue`, an empty client request for an explicit upload
  queue flush. This version defines the wire contract only; service routing and
  upload behavior are introduced separately.

## Version 8 - 2026-06-20

- Added `request_menu_status` / `menu_status` for privacy-safe menu settings.

## Version 7 - 2026-06-16

- Added `notification_payload` server message: a ready-to-schedule
  notification pushed after a fresh (non-cached) daily insight fetch.
  `notification_id`, `title`, and `body` are Rust-authored display copy —
  Swift schedules exactly this content and never generates notification text
  itself. `insight_date` is the calendar date the insight covers.
  `do_not_disturb_until`, when present, is a future timestamp before which
  Swift must not deliver the notification.
- This message type and its Swift DTO (`NotificationPayload`,
  `ServerMessage.notificationPayload`) existed in `swift-client/` prior to
  this version but had no `proto/schema/` entry, no Rust counterpart, and no
  version bump — a partial protocol update that the version-bump process
  below exists to prevent. This entry retroactively closes that gap.

## Version 6 - 2026-06-15

- Added `sign_up` client message: Swift sends email/password credentials; Rust
  performs the HTTP signup and responds with `auth_success` or `auth_failure`.
- Added `log_in` client message: Swift sends email/password credentials; Rust
  performs the HTTP login and responds with `auth_success` or `auth_failure`.
- Added `log_out` client message: fire-and-forget notification to Rust that the
  client has cleared its local session. Rust revokes the server session.
- Added `delete_account` client message: Swift requests permanent account
  deletion. Rust responds with `account_deletion_accepted`.
- Added `auth_success` server message: carries `user_id`, `access_token`,
  `refresh_token`, and `expires_at`. Swift stores tokens in Keychain.
- Added `auth_failure` server message: carries `code` and `message`. Codes:
  `invalid_credentials`, `network_error`, `server_error`.
- Added `account_deletion_accepted` server message: confirms that the Rust
  service accepted and processed the account deletion request.
- Added `needs_reauth` server message: pushed when the session expires or the
  access token cannot be refreshed. Swift must clear Keychain and show login.
- Added `device_revoked` server message: pushed when the device registration is
  permanently revoked. Swift clears Keychain and shows the Device Revoked screen.

## Version 5 - 2026-06-15

- Added `shutting_down` server message: sent to all connected clients immediately
  before a graceful service shutdown. The `reason` field is `"sigterm"` or
  `"sigint"`. Clients should disconnect and reconnect after the service restarts.

## Version 4 - 2026-06-14

- Added `request_latest_insight` client message: Swift requests the insight for a
  specific date; Rust responds with `insight_payload` or `cache_empty`.
- Added `request_latest_history` client message: Swift requests history for the
  last N days; Rust responds with `history_payload`.
- Added `cache_empty` server message: returned when the requested payload has no
  cached entry yet. `payload_type` identifies which payload was requested.
- `insight_payload` and `history_payload` are now also pushed proactively after a
  successful cloud fetch and after `privacy_violation_alert` events, without a
  corresponding client request.

## Version 3 - 2026-06-14

- Added `privacy_violation_alert` from Rust to Swift for terminal cloud privacy
  rejection notifications.

## Version 2 - 2026-06-13

- Changed version negotiation to server-first `server_hello`, `client_hello`,
  and `acknowledged` or `version_mismatch` messages.

The integer in `version` identifies the IPC protocol version implemented by
both local workspaces. Every connection begins with a version handshake.

## Versioning Policy

### Non-Breaking Changes

Backward-compatible documentation clarifications do not require a version
bump. Additive optional fields may remain within the current version only when
both workspaces can safely ignore them. Because schemas are closed, even an
optional-field addition requires coordinated schema and DTO updates.

### Breaking Changes

Removing or renaming fields, changing field meaning or type, making optional
fields required, changing enum values, changing message direction, or adding
or removing message types is breaking and requires a protocol version bump.

### Version-Bump Process

1. Update the integer in `proto/version`.
2. Update every affected schema in `proto/schema/`.
3. Add a dated changelog entry describing compatibility impact.
4. Update Rust DTOs, dispatch, and contract tests.
5. Update Swift DTOs, dispatch, and contract tests.
6. Verify both workspaces negotiate and reject versions as documented.
7. Land `proto/`, `rust-service/`, and `swift-client/` changes atomically in
   the same commit.

Partial protocol updates are prohibited and must not be merged.

## Version 2

- Changed negotiation to `server_hello` followed by `client_hello`.
- Added typed `acknowledged`, `version_mismatch`, and `malformed_message` responses.
- Wrapped every message body in a tagged `payload` object.

## Version 1

- Initial newline-delimited JSON contract.
