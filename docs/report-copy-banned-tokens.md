# `REPORT_COPY_BANNED_TOKENS` — narrowing the copy vocabulary ban by surface

**Status: design only. No Rust is changed by this document.**
**Written 2026-08-21.** Supersedes nothing until the amendment under invariant 7
of `plan/05-unified-roadmap.md` is accepted; the note there points here.

---

## 1. What exists today

`velvt-app/rust-service/src/work_block/mod.rs:47` holds one registry, applied to
every Rust-authored copy surface:

```rust
pub const BANNED_COPY_TOKENS: &[&str] = &[
    "still", "dismiss", "failed", "failure", "ignored", "last time", "again",
    "learned", "adaptive", "missed", "skipped", "declined", "you didn't",
    "you haven't", "you never", "last invitation", "last offer", "streak",
    "broken chain",
];
```

Matched **case-insensitively as substrings** against rendered copy. Copy is
authored in Rust beside the evidence that justifies it, and Swift renders it
verbatim, so the registry is a real mechanism rather than a style guide. That is
unusual discipline and it is an asset.

The nineteen tokens fall into four groups:

| Group | Tokens | Invariant |
|---|---|---|
| **Absence framing** | `you didn't`, `you haven't`, `you never`, `missed`, `skipped`, `declined`, `ignored` | 7 — describes the user by what they failed to do |
| **Failure tallies and history** | `failed`, `failure`, `last time`, `again`, `still`, `last offer`, `last invitation`, `dismiss` | 2, 7 — counselor voice; `still` is named in invariant 7 by example |
| **Streak language** | `streak`, `broken chain` | 6 |
| **Capability claims** | `learned`, `adaptive` | The honesty rule: the shipped detector is deterministic |

---

## 2. The problem, stated precisely

**The first three groups are about *voice* and should apply everywhere. The
fourth is about *capability* and should apply only where a capability is being
claimed.**

`learned` and `adaptive` are banned because through 0.1.6 the product is
deterministic and `GOAL.md` says: *"Do not write marketing language that claims
Velvt learns, adapts, or predicts."* That is correct for **intervention copy**,
where the user is being interrupted and the interruption implicitly asserts that
Velvt knows something.

It is wrong, and increasingly wrong, for **retrospective report copy**, where the
sentence describes evidence rather than claiming a capability. A substring match
is blunt enough that the ban currently blocks true and useful sentences:

| Sentence | Why it is blocked | Why it should be allowed |
|---|---|---|
| "You corrected this app once; Velvt has **learned** that correction and applies it to every window of that app." | contains `learned` | It is a factual statement about a stored correction, verifiable in `work_block_category_correction`. Nothing predictive is claimed |
| "This week's chart shows the categories Velvt **learned** from your corrections." | contains `learned` | Describes provenance, which is exactly what the honesty rule wants surfaced |
| "**Adaptive** quiet hours are off." | contains `adaptive` | Names a setting. Claims nothing |

The current registry forces such sentences to be paraphrased into vaguer prose —
which makes the copy **less** grounded, not more. **A ban that pushes copy away
from naming its own evidence is working against the invariant it serves.**

A second, quieter problem: substring matching over-catches. `dismiss` blocks
"dismiss" in a button label that is simply the correct word for the control;
`again` blocks "Try again" in an error state that has nothing to do with the
user's behavior. Those are in scope for the same split.

---

## 3. The proposal — two registries, not one relaxed registry

**Do not relax `BANNED_COPY_TOKENS`.** Add a second, narrower registry that
applies to report surfaces only, and leave the intervention registry exactly as
it is.

```rust
/// Applies to every intervention surface: drift offers, invitations,
/// soft-restart, demotion disclosure, and any string delivered while a
/// block is active. Unchanged. This is the strict registry.
pub const BANNED_COPY_TOKENS: &[&str] = &[ /* the existing nineteen */ ];

/// Applies to retrospective, non-interrupting surfaces only: session
/// results, weekly receipts, dashboard headlines, correction history,
/// activity charts, and settings labels.
///
/// Narrower than BANNED_COPY_TOKENS by exactly two tokens -- `learned`
/// and `adaptive` -- and by nothing else. Absence framing, failure
/// tallies, history references, and streak language remain banned here,
/// because they are voice rules and voice does not change by surface.
pub const REPORT_COPY_BANNED_TOKENS: &[&str] = &[ /* the same list, minus
    "learned" and "adaptive" */ ];
```

**The differences are exactly two tokens.** Not a family, not a policy, not a
judgment call at authoring time. Anyone can diff the two constants and see the
entire delta.

### The surface split

| Surface | Registry | Reason |
|---|---|---|
| Drift offer titles and bodies (`DRIFT_TITLES`, `drift_body`) | **strict** | Interrupts. **Frozen copy — do not touch** (`pivot-engineering/06-SEVEN-DAY-PLAN.md` § 1.2) |
| Initiation invitations | **strict** | Interrupts, outside a block |
| Soft-restart copy | **strict** | Interrupts |
| Demotion disclosure | **strict** | Delivered in the moment; must read as a feature, not an apology |
| Explain-this-nudge output, incl. any LLM-phrased variant | **strict** | Justifies an interruption |
| Session result / end-of-block card | report | Retrospective |
| Weekly receipts | report | Retrospective |
| Dashboard headlines and observations | report | Retrospective |
| Correction history and inline activity naming | report | Describes stored corrections — the case that motivates this |
| Settings and permission labels | report | Names controls |

**The test that decides an ambiguous surface:** *does this string arrive
unrequested while the user is trying to work?* If yes, strict. If the user
navigated to it, report.

### Three rules that travel with the proposal

1. **The two-token delta is the whole permission.** Any future proposal to remove
   a third token from `REPORT_COPY_BANNED_TOKENS` is a separate, dated amendment.
   Registries that drift apart token by token stop being checkable.
2. **`learned` in report copy must be adjacent to its evidence.** A sentence
   using it must also state the stored quantity that justifies it — the
   correction count, the number of days, the number of blocks. *"Velvt learned
   this from 3 corrections"* is permitted; *"Velvt has learned your patterns"* is
   not, and would fail the grounded axis of the voice rubric even though it
   passes the token filter. **The token filter is a floor, not the standard.**
3. **`adaptive` is permitted only as the name of a shipped setting**, never as a
   description of the engine's behaviour.

---

## 4. What this does *not* do

**It does not authorize `learned` for the behavioral engine's output.** The
per-surface lift procedure in `pivot-engineering/testing/04-PRODUCT-COPY-REVIEW.md`
§ 2 governs that, and it is stricter than this document:

- **T1** — every surface saying `learned` must name the engine layer that
  justifies it, and that layer must have passed its gate in
  `testing/03-BEHAVIORAL-ENGINE-VALIDATION.md`. A surface claiming learning
  backed by an ungated layer is an `HONESTY`/S0 finding.
- **T2** — the ban must be lifted **per surface**, never globally. *"A blanket
  removal of `learned` from `BANNED_COPY_TOKENS` is `HONESTY`/S1 on its own,
  because it removes the mechanism that keeps the claim tied to the capability."*
- **T3** — the enforcing test must still fail when a banned token is introduced.
  Prove it by introducing one and watching the test fail.

**This proposal is consistent with T2**, and that consistency is the point of the
two-registry shape: a second registry with a two-token delta and a fixed surface
map **is** a per-surface lift, expressed as a constant rather than as a
convention. A blanket relaxation is what T2 forbids, and it is what this
deliberately is not.

**It does not change any string.** No copy is rewritten by this document.

**It does not weaken invariants 2, 6, or 7.** Every token those invariants
motivate is present in both registries.

---

## 5. Implementation notes, for whoever picks this up later

Roughly, and none of this is scheduled:

- Both constants in `work_block/mod.rs`, adjacent, with the report registry
  defined as the strict one minus two tokens so the delta cannot silently widen.
- The compatibility test that enforces the registry gains a second case per
  surface class, plus **a test asserting the delta is exactly `["learned",
  "adaptive"]`**. That test is the mechanism; without it the two lists drift.
- Any LLM phrasing seam filters on its **output**, against the registry of the
  surface it is phrasing for (`testing/04` § 5, T5). An LLM produces "still" and
  "again" naturally.
- **No protocol change.** Copy is authored in Rust and rendered verbatim by
  Swift; no DTO carries a token list.
- **No new Swift file**, and therefore no `project.pbxproj` reconciliation.

**Cost estimate: not measured.** No one has implemented this. Do not quote a
number for it.

---

## 6. Provenance

| Statement | Source |
|---|---|
| The nineteen current tokens and their substring matching | `velvt-app/rust-service/src/work_block/mod.rs:47`, read 2026-08-21 |
| Which invariants motivate which group | `plan/05-unified-roadmap.md` invariants 2, 6, 7 |
| "Do not write marketing language that claims Velvt learns, adapts, or predicts" | `GOAL.md` |
| The per-surface lift procedure and its severities | `pivot-engineering/testing/04-PRODUCT-COPY-REVIEW.md` § 2 |
| Drift copy is frozen through the pitch week | `pivot-engineering/06-SEVEN-DAY-PLAN.md` § 1.2 |
