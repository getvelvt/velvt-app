# Independent Product and Startup Critic Handoff

Audit date: 2026-07-22
Scope: product promise, onboarding, privacy/permission exchange, core loop, insight usefulness, notification value, repeat engagement, and payment readiness across `velvt-app/` and `velvt-core/`.
Method: read-only repository and synthetic-UI inspection. Production code was not modified.

## 1 Verdict

**NO-SHIP for a public or paid release. Conditional GO only for a tightly facilitated, explicitly experimental alpha after the cross-functional P0/P1 blockers are resolved.**

Velvt has a credible product seed: an explicit work block, privacy-safe observation, inspectable evidence, and a bounded next action form a real loop rather than a passive chart. The implementation also makes unusually careful attempts to avoid moralizing or inventing intent. But the present product does not yet establish that this loop produces enough repeated value to justify Accessibility permission, an account, continuous background operation, cloud synchronization, notifications, and eventual payment.

The central product risk is not missing polish; it is a promise/evidence mismatch. First-run copy says Velvt shows **why** work became fragmented (`PermissionViews.swift:447-450`), while the actual model prompt expressly forbids causal explanation (`insight_prompt_service.py:6-10`) and the UI elsewhere correctly says category movement does not establish intent (`VelvtPopoverContentView.swift:550-556`). That inconsistency can make a technically careful product feel untrustworthy.

The repeated-value engine is also narrow. The backend deterministically chooses one of four observation families, the external model is allowed exactly one already-selected template ID, and nearly every state resolves to the same 20-minute “one lane” action (`insight_evidence_service.py:65-136,146-193`; `insight_prompt_service.py:13-21`). The novelty check compares only against the most recent approved insight (`insight_quality_service.py:125-149`). This is safe, but likely to become predictable before it becomes indispensable.

### Product/startup score

| Dimension | Weight | Score | Repository-grounded assessment |
|---|---:|---:|---|
| Problem clarity and positioning | 15% | 4/10 | “Private focus coach,” “know when focus broke,” “neutral observer,” and session-analysis language compete. The product cannot know whether focus “broke” or why. |
| Permission-to-value exchange | 15% | 4/10 | Accessibility, account creation, background operation, and optional notifications precede demonstrated personal value. The early local signal helps, but qualifying activity and setup still come first. |
| Time to first value | 10% | 6/10 | A deterministic local signal is designed to appear after qualifying activity and the alpha gate targets 90 seconds; a synthetic helper harness exists. No unaided participant result is recorded. |
| Repeated insight usefulness | 20% | 3/10 | Four deterministic observation families, one repeated action pattern, shallow novelty comparison, and no evidence of week-two return or user-rated usefulness. |
| Core-loop coherence | 15% | 6/10 | Work block -> timeline/result -> bounded next session is coherent and actionable; explicit sessions reduce intent overreach. |
| Trust, explainability, and privacy UX | 15% | 5/10 | Strong local boundary and evidence disclosure are real strengths; causal copy, account-deletion/Keychain documentation mismatches, and third-party model disclosure gaps damage trust. |
| Monetization and go-to-market readiness | 10% | 1/10 | Billing/subscriptions are explicitly out of scope; no paywall, pricing thesis, conversion path, or willingness-to-pay evidence exists. |
| **Weighted product score** | **100%** | **4.2/10** | Promising alpha concept, not a validated product. |

**Startup score: 3.5/10.** The wedge is differentiated by privacy and calm evidence, but there is no demonstrated retention, willingness to pay, scalable distribution, or defensible learning loop yet. The alpha plan itself requires 60% first-week usefulness and 40% week-two return (`closed-alpha-release-plan.md:84-98`); the repository contains targets, not results.

Evidence-state summary:

- **Packaged-app verified:** none in this product-critique track. The existing artifact was not treated as user-research evidence.
- **Dev-mode only:** synthetic screenshots and their historical snapshot/build claims; no current interactive user session was run.
- **Implemented-unverified:** onboarding, account gating, work blocks, local dashboard, cloud insights, evidence disclosure, notification handling, and settings/recovery source paths.
- **Proposed only:** public/paid positioning, monetization, validated retention, willingness to pay, adaptive notification policy, and claims of expansion-ready alpha outcomes.

### What is genuinely strong

1. **A real behavior loop exists.** The main workspace can show an active work block, latest observation/action, and explicit-session fragmentation evidence (`VelvtPopoverContentView.swift:432-479`). The action can initiate another session instead of ending at a chart.
2. **Intent boundaries are materially better than typical productivity software.** Session analysis requires an explicit block, calls switches observed movement, exposes coverage/confidence, and says it does not infer intent (`focus-activity-surfaces.md:12-31`; `VelvtPopoverContentView.swift:519-570`).
3. **Evidence is designed to be inspectable.** Insight cards include observation, baseline comparison, a next step, confidence, generation timing, and “Why am I seeing this?” (`InsightCardView.swift:66-151`).
4. **Notifications are optional and deduplicated per insight date in the live process** (`PermissionViews.swift:471-485`; `NotificationDeliveryCoordinator.swift:64-84`).
5. **The team has written falsifiable alpha gates.** Purpose comprehension, unaided permissions, first value, usefulness, return behavior, evidence, and integrity are all measurable (`closed-alpha-release-plan.md:84-98`). That is the right discipline; the missing piece is actual cohort evidence.

## 2 Evidence and commands run

All commands ran from `/Users/maximkudryashov/Projects/velvt-dev`.

- Read both instruction files in full: `sed -n ... velvt-app/AGENTS.md` and `velvt-core/AGENTS.md`.
- Inventoried product, UI, privacy, release, test, and architecture files with `rg --files`, `find`, and focused `rg -n` searches.
- Read the canonical/product-facing material: `velvt-app/README.md`, `PRIVACY.md`, `docs/private-beta-guide.md`, `docs/closed-alpha-release-plan.md`, `docs/architecture/focus-activity-surfaces.md`, `docs/architecture/work-block-loop.md`, `docs/scope3-minimal-dashboard-handoff.md`, `PERFORMANCE_REPORT.md`, and `DEFERRED.md`.
- Inspected first-run, permission, account, navigation, work-block, Today, Activity, insight-card, settings, and notification source in `swift-client/Sources/VelvtMac/`.
- Inspected insight evidence selection, deterministic copy, provider constraints, novelty gates, and notification delivery behavior in `velvt-core/app/services/` and corresponding tests.
- Inspected the two repository synthetic screenshots with the image viewer: `scope3-focus-fragmentation-synthetic.png` and `scope3-daily-activity-synthetic.png`. These are fixed-fixture renders, not packaged-app or user-research evidence.
- Reviewed `ARCHITECTURE_AUDIT_HANDOFF.md`, `CORE_QA_HANDOFF.md`, and `PRIVACY_SECURITY_HANDOFF.md` for cross-functional facts, then independently checked product-relevant source claims before relying on them.
- Checked worktrees with `git status --short`. A pre-existing edit exists at `velvt-app/swift-client/Sources/VelvtMac/UI/HistoryListView.swift`; this track did not touch it.
- Searched for pricing, subscription, billing, trial, paywall, purchase, retention, and activation surfaces. `velvt-core/README.md:50-56` explicitly places billing/subscription out of scope; no implemented monetization path was found.

No specialized product/UX critique skill was available. Native repository inspection and the existing synthetic visual artifacts were used; this limitation is recorded rather than invoking an unrelated skill.

## 3 Files changed

- Added `/Users/maximkudryashov/Projects/velvt-dev/PRODUCT_CRITIC_HANDOFF.md` (this file).
- No production code, tests, manifests, assets, lockfiles, or existing documentation were modified.

## 4 Tests added or executed

- Tests added: none; this role was explicitly read-only for production behavior.
- Automated suites executed: none. Core QA owns executable and packaged-app verification.
- Visual inspection executed: both synthetic Scope 3 screenshots. **Dev-mode only / synthetic fixture**, not real-user or installed-app validation.
- Research gates executed: none. No participant interviews, task-completion sessions, diary study, retention cohort, notification usefulness study, or willingness-to-pay experiment was available.

## 5 Findings P0-P3

### P0

No independent product-only P0 was established from source. This does **not** clear release: architecture, privacy/security, distribution, updater, and insight/notification tracks own independent P0s, and lack of product evidence cannot override them.

### P1 — release/expansion blockers

1. **The headline promise claims causal knowledge the product explicitly refuses and is unable to produce.** First-run UI promises Velvt shows when work fragmented, **why it happened**, and what to protect (`PermissionViews.swift:447-450`). The generation prompt forbids causal explanation (`insight_prompt_service.py:6-10`), and the session UI correctly says category movement is not proof of intent (`VelvtPopoverContentView.swift:550-556,625-688`). A buyer granting Accessibility permission will reasonably interpret “why” as content/context diagnosis; Velvt only has broad categories and timing. **Implemented-unverified.** Fix by making the acquisition/onboarding promise observational: “show where switching clustered, what evidence supports it, and one experiment for next time.” Do not use “focus broke” unless the user explicitly marks that outcome.

2. **Repeated value is too narrow to justify persistent access or payment, and usefulness gates have no results.** Four deterministic observation priorities resolve to a small copy library; the “LLM” may only return the one allowed ID that was already selected (`insight_evidence_service.py:65-136`; `insight_prompt_service.py:13-21`). Default actions repeatedly prescribe a 20-minute single lane (`insight_evidence_service.py:186-193`), while novelty compares against one prior approved insight (`insight_quality_service.py:125-149`). The alpha plan demands 60% useful first-week insight and 40% week-two return, but no outcomes are recorded (`closed-alpha-release-plan.md:84-98`). **Implemented-unverified plus missing evidence.** Do not expand beyond a facilitated alpha until both gates pass on the exact release build.

3. **Velvt asks for an account before local collection, weakening its strongest local-first story.** Onboarding states that an account is required and local collection starts only after authentication (`PermissionViews.swift:487-491`). Yet work-block intentions, session analysis, and early signals are expressly local, and the beta guide says local work-block/early-signal value can appear sooner (`private-beta-guide.md:38-41`). This creates a credibility and activation tax before users experience the privacy benefit. **Implemented-unverified.** Permit a local-only mode through first value, then ask for an account when the user chooses cloud history/insights; if business constraints forbid this, explain the necessity before permissions.

4. **Privacy trust claims and implementation-facing disclosures are not internally reliable enough for a permission-heavy product.** The independent privacy audit found that canonical docs claim a Rust Keychain store that is not implemented, account deletion copy is broader than retained audit identifiers, and configured third-party model processing is not disclosed in app privacy/onboarding. These are especially damaging because privacy is the product's differentiator, not a background compliance detail. **Implemented-unverified/documented mismatch.** Treat every such mismatch as a product blocker: a privacy-first product cannot ask users to trust caveated architecture documents over first-run copy.

5. **Notification value and timing are not proven, while immediate delivery is the default when no quiet-until value exists.** Swift schedules immediately if `doNotDisturbUntil` is absent (`NotificationScheduler.swift:31-33,53-72`); the deferred-seams document says Rust always sends it absent. Process-local handling deduplicates only by insight date during the debounce window (`NotificationDeliveryCoordinator.swift:64-84`). The beta explicitly does not promise adaptive notifications (`private-beta-guide.md:36-42`). **Implemented-unverified.** Keep notifications opt-in and off by default for expansion until the exact copy/timing ledger demonstrates useful, non-repetitive delivery; add user-set quiet hours and visible notification controls before paid release.

### P2 — high churn, comprehension, and positioning risks

1. **The positioning is internally split.** “Private focus coach,” “passive productivity intelligence,” “neutral observer,” “protect meaningful work,” and “Focus Fragmentation” imply different jobs. Coaching implies personalized interventions; passive intelligence implies recurring discovery; the current product mostly supplies a timer plus category-switch evidence. **Implemented-unverified copy.** Choose one primary job for alpha: “a private session review for Mac work blocks” is the most defensible current wedge.

2. **The product's vocabulary requires interpretation at the moment value should be obvious.** “Meaningful switches,” “fragmentation,” “coverage,” “cluster,” and broad category colors are analytical constructs. Explanations frequently live in hover/help or small muted text; the synthetic screenshot presents six metrics plus timeline marks before demonstrating a concrete payoff. **Dev-mode only synthetic visual plus implemented-unverified source.** Lead with the one observation and action; put the forensic timeline behind evidence disclosure.

3. **The local early signal risks being merely descriptive.** Focused seconds, switch count, and longest stretch are legible, but without a comparison or user-declared outcome they may tell users what they already know. The cloud layer adds comparison only after baseline maturity, increasing the time until non-obvious value. **Implemented-unverified.** Ask one lightweight end-of-block question (“Did this block feel protected?”) stored locally, then use it only to calibrate future session experiments—not to infer causality.

4. **The UI/documentation baseline is contradictory.** `scope3-minimal-dashboard-handoff.md:5-14` says the navigation rail was removed and replaced with one Focus/Activity control, but current source still renders Today/Your Week/Settings in a 132-point rail (`MenuBarPopoverView.swift:377-405,608-675`). The beta guide also refers to Today, Activity, and Your Week as separate surfaces (`private-beta-guide.md:7-12`). **Implemented-unverified/documentation mismatch.** Resolve the intended IA before usability testing so participant evidence applies to the build being evaluated.

5. **Recovery burden is too visible for a passive product.** Beta instructions include manual retry sync, restart local service, quit/reopen, Accessibility recovery, separate account deletion, and manual local-data removal (`private-beta-guide.md:20-30`). Users install passive tools specifically to avoid maintenance. **Implemented-unverified.** Automatic helper recovery and clear, single-step health recovery are product requirements, not only engineering polish.

6. **There is no monetization thesis in the product.** Billing/subscriptions are explicitly out of scope (`velvt-core/README.md:50-56`), and no pricing, trial, paywall, plan differentiation, buyer, or willingness-to-pay experiment exists. **Proposed only.** Do not add billing yet; first test whether users would pay for session review, recurring insight, team/coach reporting, or privacy assurance. Those are materially different businesses.

### P3 — polish and learning-loop gaps

1. **The first-run experience appears duplicated.** `FirstRunExperienceView` presents a multi-step intro, while `FirstRunOnboardingView` separately contains value proposition, permission, auth, service, and collection states. The sequence may be intentional, but it creates a long path before proof. **Implemented-unverified.** Measure unaided completion and remove any step that does not change informed consent or activation.

2. **Local diagnostic counters do not measure product success.** `AppMetricsStore` counts relayed events as “actions logged” and scheduled notifications as “interventions” (`AppMetricsStore.swift:34-88`; `EventRelay.swift:138`; `NotificationScheduler.swift:73-76`), and these are debug-only UI rows. Neither indicates useful insight, behavior change, or return intent. **Implemented-unverified.** Keep invasive telemetry out; use the planned opt-in interviews/surveys and add privacy-reviewed coarse activation events only if manual research becomes insufficient.

3. **Synthetic screenshots prove renderability, not comprehension.** They are useful engineering fixtures but cannot validate whether people understand colors, clusters, switching, or next-action relevance. **Dev-mode only.** Use them as facilitator prototypes, not market evidence.

### Prioritized product hardening plan

1. Rewrite the promise to match observable evidence; eliminate “why it happened” and unmarked “focus broke” claims.
2. Offer first local value before mandatory account creation, or document the unavoidable reason for the gate.
3. Run the existing two-week facilitated alpha on the exact signed build; do not reinterpret failed usefulness/return gates.
4. Grade every delivered insight/notification for novelty, specificity, action diversity, and user-rated usefulness; suppress safe-but-obvious repetition.
5. Collapse the value surface to observation -> evidence -> one experiment; make analytical detail secondary.
6. Resolve privacy/deletion/provider disclosures in first-run language before asking for Accessibility.
7. Only after retention evidence, test willingness to pay and buyer/job hypotheses; avoid building billing around an unvalidated loop.

## 6 Open questions or blockers

- No real participant evidence was available: purpose comprehension, unaided permission completion, first-value usefulness, week-two return, notification timing, uninstall reasons, and willingness to pay are unknown.
- No packaged, signed, installed release was exercised by this track, so all UI behavior remains source/synthetic rather than packaged-app verified.
- No production backend/provider configuration was available, so actual third-party processing and exact delivered-copy distribution are unknown.
- It is unclear whether the intended alpha IA is the current navigation rail or the “minimal dashboard” described in the Scope 3 handoff.
- It is unclear why authentication must precede entirely local work blocks and early signals; the repository offers no product/business rationale.
- External user research, payment experiments, credentials, and destructive actions were correctly treated as stop conditions.

## 7 Confidence level

**High (0.91) for source-grounded promise, onboarding, account-gate, insight-diversity, notification-policy, IA, and monetization findings.** These are directly evidenced in active source and canonical docs. **Medium (0.60) for predicted churn and willingness-to-pay outcomes** because no live participant cohort, packaged-app session, notification diary, or pricing study was available. The conservative conclusion is therefore “unvalidated,” not “users will definitely reject it.”
