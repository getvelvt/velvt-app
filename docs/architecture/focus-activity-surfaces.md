# Focus Fragmentation and Daily Activity

Protocol 20 replaces the score-led local dashboard with exactly two analytical
surfaces behind one persisted `Focus` / `Activity` segmented control. The latest
grounded observation and next action remain above that control.

## Ownership and privacy

- Rust performs all bounded history queries and derives timeline blocks,
  deduplicated category transitions, clusters, longest stretches, recoveries,
  coverage, comparison eligibility, seven local day rows, label buckets,
  `Other`, percentages, and grounded detail evidence.
- Swift renders the DTO and stores only the selected segment in `UserDefaults`.
  It never scans events or derives behavioral metrics.
- Daily display labels may appear only in the `local_dashboard.daily_activity`
  local IPC branch. The upload structs cannot represent them. Rust and Swift
  log only message types/counts, and the DTO's Rust `Debug` output redacts labels.
- Raw app identity, titles, URLs, paths, files, contacts, stable mapping hashes,
  and intention text cannot appear in either analytical DTO.

## Focus Fragmentation rule

Focus exists only when Rust has an explicit current or most-recent work block.
It clips blocks longer than one hour to their most recent 60 minutes and preserves
the actual duration for shorter blocks. Terminal blocks use their persisted wall
clock end so later activity is never attached to the block.

`SWITCHING_CLUSTER_RULE_VERSION = 1` means at least three deduplicated,
classified category transitions in an inclusive five-minute window. `system`,
`idle`, `unlogged`, unclassified, and same-category duplicates are excluded.
Overlapping qualifying windows merge deterministically. A recovery is a return
to the dominant covered work-block category after moving away, matching the
documented Scope 2 session rule.

An earlier-today comparison is emitted only when both adjacent windows are a
full 60 minutes and each has at least 75% classified coverage. The UI otherwise
says `Not enough comparable activity`. Seven-day comparison remains optional
and is not emitted by this implementation; no single day is called a baseline.

## Daily Activity rule

Rust uses seven indexed local-calendar queries, each capped at 2,048 events.
High/medium-confidence rows use their curated local display label, then fall
back to the safe category. Weak rows become `Unclassified`. Buckets under one
minute or five percent and buckets after the five largest labels merge into one
`Other`, producing at most five labels plus `Other`. Dwell durations clip at day
boundaries. Each segment has at most one deterministic cluster or sustained-block
explanation with its evidence window and confidence.

Rows explicitly encode `no_data`, `low_confidence`, `still_building`, or `ready`.
Color is paired with text, `.help`, focusable controls, accessible labels, and
confidence wording.

## Runtime behavior

The dashboard is requested on connection, popover appearance, and work-block
state changes. It is not recomputed inside the event collection callback and
adds no polling loop. Raw-event retention and the existing seven-day cache/data
clear paths are unchanged.
