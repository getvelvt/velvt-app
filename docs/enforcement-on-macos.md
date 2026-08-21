# Enforcement on macOS — what is available, what is not, and why it is cut

**Status: decided. Enforcement is cut from scope.**
**Written 2026-08-21.** Design record. No code changes accompany this document.

This file exists because "block the apps that break your focus" was proposed as a
flagship product direction, and the answer is no for a reason that is
compiler-verifiable in about thirty minutes. Recording the answer here means
nobody spends thirty minutes on it again, and — more importantly — nobody puts it
on a slide.

---

## 1. The finding

**`ManagedSettingsStore()` is unavailable on macOS.** Screen Time's
app-shielding store is an iOS/iPadOS API. It does not exist for a native macOS
target.

The trap is that it *looks* available. `import ManagedSettings` **compiles**
against `MacOSX15.4.sdk` — the module is present in the SDK because the framework
ships as part of a shared family — so a developer scanning for feasibility by
checking whether the import resolves gets a false positive. The failure appears
only when the store is instantiated:

```
'ManagedSettingsStore' is unavailable in macOS
```

That is a compile-time availability error, not a runtime entitlement problem. No
entitlement, provisioning profile, or Apple approval changes it, because the type
is not offered on the platform.

**Consequence for the product claim.** The sentence *"…then block you from
opening them"* is an unbacked claim that a technically literate investor can
falsify in one compile, in front of the founder. It is cut from the product, from
the deck, and from the "Next" slide — not deferred, not marked "Next," **cut**.
See `pivot-engineering/00-GROUND-TRUTH.md` § 6b.3 and
`pivot-engineering/02-MOAT.md` § "OPEN QUESTION — Enforcement".

---

## 2. What is actually available on macOS

Ranked by how much it restricts, weakest first.

| Mechanism | What it can do | What it cannot do | Verdict |
|---|---|---|---|
| **Notification suppression / Focus** | Stop Velvt and other apps from interrupting; already shipped and reconciled after a block | Stop the *user* from opening anything | **Shipped.** This is the honest actuator |
| **A dismissible always-on-top window** ("Commitment Shield") | Put visible friction in front of a switch, with a countdown and an override that always works | Prevent anything | **Designed, not planned.** See § 4 |
| **App Management (`NSWorkspace` / user-approved control)** | Manage updates and, with user approval, control other apps in narrow ways | Deny a launch the user initiates | Not an enforcement path |
| **Endpoint Security `AUTH_EXEC`** | Genuinely deny process execution | — | **The only true block. Rejected. See § 3** |

Nothing between the always-on-top window and Endpoint Security exists. There is
no partial, low-privilege blocking API on macOS. That gap is the whole decision.

---

## 3. Endpoint Security `AUTH_EXEC` — the only real block, and why it is rejected

`AUTH_EXEC` is the Endpoint Security authorization event for process execution. A
client that subscribes to it is asked to allow or deny **every** `exec` on the
machine and must answer within a deadline. It is the mechanism the real blockers
use, and it would genuinely work.

**Cost, in four parts, all mandatory:**

1. **The `com.apple.developer.endpoint-security.client` entitlement**, which is
   granted by Apple on request, case by case, on an **unknown timeline**. It is
   not something a team schedules around.
2. **A System Extension.** The blocking logic moves out of the app into a
   separate installed extension with its own approval flow, its own update path,
   and its own failure modes.
3. **Full Disk Access**, granted by the user in System Settings, on top of the
   Accessibility permission Velvt already asks for.
4. **A latency budget with teeth.** An ES client that answers `AUTH_EXEC` slowly
   degrades the whole machine; one that answers wrongly can prevent the user from
   launching software. The failure mode of a bug is not "the feature does not
   work," it is "your Mac does not start applications."

**And the reason it is rejected is not the cost.** It is this:

> An Endpoint Security client sees **every process execution on the machine** —
> every binary, every launch, everything the user runs, including things far
> outside the apps Velvt observes today.

That is **strictly more raw data than Velvt collects now**, acquired at a
strictly higher privilege level, in a product whose single differentiating claim
is that it takes less than the alternatives and can prove it. Shipping it would
mean standing in front of a customer and saying *"raw work context never leaves
your Mac"* while holding a veto over every program they run.

**It inverts the thesis.** That is the argument. The entitlement timeline and the
System Extension are merely why it would also be slow.

---

## 4. What survives: friction, not enforcement — and it is not planned either

If customer interviews say restriction is what people actually pay for (see § 5),
the honest ceiling is the **Commitment Shield**:

- a dismissible always-on-top window with a countdown;
- **an override that always works**, one interaction away, never hidden;
- **the override is recorded as the outcome measurement.**

That last property is what makes it interesting rather than merely annoying. It
is the highest-rate label generator in any design considered, because every
instance produces a decision with a clear outcome attached, and it requires no
Apple entitlement — it is friction, not enforcement.

**It is still not planned**, for one reason stated plainly: it contradicts
product invariant 2 of `plan/05-unified-roadmap.md` — *backoff, never
escalation* — as that invariant is currently written. A window the user must
dismiss is more intrusive than the nudge that preceded it, which is escalation by
any reading.

So the Commitment Shield is a **designed option contingent on an invariant
change**, and an invariant change is a founder decision informed by customer
evidence, not an engineering decision. It is written down here so that if that
evidence arrives, the design does not have to be reinvented.

---

## 5. What replaces the enforcement question

The question was never really *"can we block apps?"* It was **"do people pay to
be stopped?"** — and that one is still open and still worth answering.

Freedom, Cold Turkey, Opal, and Brick all monetize restriction. RescueTime, the
pure-insight product, is the one that got commoditized against a free,
preinstalled Screen Time. That is a real asymmetry and it is a reason to ask the
question rather than to assume the answer.

**So it moves to the customer interviews as a pricing probe, not to the roadmap
as a build item.** `pivot-engineering/09-RISKS-AND-KILLS.md` § 2.1 records why:
enforcement was originally eliminated using an internal invariant the founder
wrote and a stress test whose personas were synthetic — that is, eliminated with
no user evidence, in a company that refuses to build anything else without user
evidence.

**Do not put restriction on a slide.** Ask about it in interviews, record what
people say in their own words, and let that decide whether invariant 2 is
revisited.

---

## 6. What must not happen

- **Do not re-open this by checking whether `import ManagedSettings` compiles.**
  It does. That is the trap. Instantiate the store.
- **Do not describe Focus/DND activation as blocking, app unavailability, or
  restriction**, in copy, in analytics, or in a deck. It is notification
  protection. Name the capability actually delivered — the same rule
  `ACT-1` in `pivot-engineering/10-BUNDLE-ABSORPTION.md` § 2 arrives at
  independently.
- **Do not list enforcement as "Next."** "Next" means specified but not built.
  This is *not possible without inverting the product's core claim*, which is a
  different category and must not be presented as a roadmap item.
- **Do not treat this as reversible if a future macOS SDK adds a shielding API.**
  If that happens, the § 3 argument still has to be re-run: the question is not
  whether an API exists, it is what privilege and what visibility it costs.

---

## 7. Provenance

| Claim | How it was established |
|---|---|
| `ManagedSettingsStore()` is unavailable in macOS | Compile against `MacOSX15.4.sdk` for a native macOS target; the error is an availability error, not a link error |
| `import ManagedSettings` compiles anyway | Same compile — the import resolves and only instantiation fails |
| `AUTH_EXEC` requires an Apple-granted entitlement, a System Extension, and Full Disk Access | Apple's Endpoint Security documentation and entitlement request process |
| Enforcement is cut from scope, deck, and "Next" | `pivot-engineering/00-GROUND-TRUTH.md` § 6b.3; `06-SEVEN-DAY-PLAN.md` § 0 Fact 3 |
| The willingness-to-pay question is reinstated as an interview probe | `pivot-engineering/09-RISKS-AND-KILLS.md` § 2.1 |

**Nothing in this document was measured on a user.** The compile result is a
fact; every statement about what people will pay for is an open question, and is
labelled as one.
