# Insight and Notification Quality Audit

## 1 Verdict

**NO-SHIP for production notifications.** The deterministic insight pipeline is substantially safer than a free-form LLM pipeline, and six high-confidence quality defects were fixed in this audit. Development-mode suites now verify baseline gating, fragmentation/switching evidence, no-signal suppression, full-window novelty, provider fallback, backend claim deduplication, UTC day selection, IPC delivery, permission handling, and native request construction.

Two P1 release blockers remain:

1. There is no end-to-end delivery acknowledgement. `GET /v1/insights/poll` marks an insight delivered before Rust queues IPC and before Swift checks notification permission or macOS accepts the request. A Rust/app crash, disconnected IPC queue overflow, denied permission, or `UNUserNotificationCenter.add` failure can permanently consume the only notification attempt.
2. Quiet-hour/evening behavior is undefined. The backend runs the prior UTC-day workflow at 03:00 UTC, Rust always emits `do_not_disturb_until: None`, and Swift therefore schedules immediately. This can interrupt users late at night depending on local time. No device timezone or reviewed local delivery window exists.

Release status distinctions:

- **Verified in a packaged app:** none in this workstream.
- **Verified only in development mode:** deterministic insight generation/gates, Rust fetch/poll/push behavior, Swift IPC coordinator and native-notification request construction.
- **Implemented but unverified in a packaged app:** duplicate-path removal, generic-notification suppression, full-window novelty comparison, dominant-reference selection, UTC latest-insight date selection, corrected baseline copy.
- **Proposed only:** delivery acknowledgement/retry, local quiet-hours policy, true evening summary behavior, clean-install native notification verification.

## 2 Evidence and commands run

### Event-to-notification chain

`AbstractedEvent` -> UTC daily normalization/sessionization -> fragmentation score and switching clusters -> prior-summary baseline comparison -> reviewed evidence template -> provider selects only the allowed template ID (template fallback on provider failure) -> confidence/classification/coverage/sufficiency/novelty/evidence gates -> approved `DailyInsight` -> APNs silent wake or authenticated long poll -> backend atomic claim -> Rust IPC insight + optional notification payload -> Swift permission check -> `UNUserNotificationCenter.add`.

### Deterministic evidence ledger

| Fixture | Model/insight outcome | Notification outcome | Evidence | Verification |
|---|---|---|---|---|
| Baseline learning, days 1-14 | Low/early baseline; comparison makes no mature-baseline claim | Suppressed because tone is `early` | `test_modeling_baseline.py`, `early_baseline_delivers_insight_without_notification` | Development only |
| Mature baseline, day 15 | Compared against prior summaries | Eligible only if evidence is non-generic | `test_modeling_baseline.py`, emotional-stage tests | Development only |
| Fragmented switching loop | Highest-transition safe-category cluster; exact transition count and interval | Eligible after gates | `focus_communication_loop`, cluster/scoring tests | Development only |
| Stable uninterrupted work | Longest zero-switch work session rendered in minutes | Eligible after gates | `stable_focus_block` | Development only |
| Minority reference activity | Does not displace the stronger focus observation after this audit | Follows focus eligibility | `minor_reference_activity_does_not_displace_focus_signal` | Development only |
| Dominant reference activity | Neutral factual minute count; no distraction/intent inference | Eligible after gates | `reference_heavy_legitimate_work`, AI-assistant fixture | Development only |
| No events/inactivity | Summary activity skips; no insight is generated | No APNs/IPC notification | `test_summary_activity_lists_active_users_and_skips_no_event_users` | Development only |
| Low coverage/unclassified | Input contract or quality gate rejects | No approved insight; no notification | `low_coverage_unclassified`, input-contract tests | Development only |
| Generic recorded activity | Remains available as UI insight | Suppressed after this audit | `generic_recorded_activity_delivers_insight_without_notification` | Development only |
| Duplicate within 7-day novelty window | Rejected against the most similar prior insight, not merely yesterday | No approved insight; no notification | `test_novelty_gate_checks_the_full_window_not_only_the_latest_insight` | Development only |
| Duplicate poll response | Rust dedupe allows one insight push and one notification payload | At most one per repeated insight ID in-process | `deliver_once_for_duplicate_polled_insight` | Development only |
| Startup latest-fetch + poll | Latest fetch updates display only; claim-once poll exclusively owns notification creation | Duplicate startup notification removed | `latest_insight_fetch_updates_display_without_scheduling_notification` | Development only |
| UTC/local midnight boundary | Swift requests the backend's UTC calendar date | Prevents wrong-date latest-insight request | `testCurrentUTCDateStringDoesNotAdvanceAtLocalMidnightBeforeUTCMidnight` | Development only |
| Provider/LLM unavailable | Provider failures fall through to deterministic template | Eligible only if all quality gates pass | provider-chain and Temporal activity fallback tests | Development only |
| Offline/transient poll failure | Exponential capped backoff; cached display data remains available | Notification is delayed, but claimed-before-delivery failures are not recoverable | Rust poll/fetch tests | Development only |
| Permission denied/restricted/unknown | Insight can still be shown in UI | Swift discards notification without prompt/retry | Swift coordinator permission matrix | Development only |
| Notification cap | Backend claim and Rust dedupe cap the normal poll path; Swift burst debounce caps same-date bursts | No durable acknowledgement or retry ledger | polling service concurrency tests, Swift burst tests | Development only |
| Evening summary/quiet hours | No local-evening summary implementation found | `do_not_disturb_until` is always `None`; immediate delivery | code inspection of schedule, Rust push, Swift scheduler | Proposed only / blocker |

### Commands and results

```text
velvt-core/.venv/bin/pytest -q tests/test_scope2_work_loop_evaluation.py tests/test_insight_quality_service.py tests/test_insight_provider_service.py tests/test_insight_generation_service.py tests/test_insight_input_service.py tests/test_insight_emotional_stage.py tests/test_insight_polling_service.py tests/test_temporal_workflows.py tests/test_temporal_activities.py tests/test_apns_service.py
87 passed, 1 warning in 5.18s (final integrated rerun; the earlier pre-final run was 85 passed)

velvt-core/.venv/bin/pytest -q tests/test_insight_quality_service.py tests/test_insight_generation_service.py tests/test_scope2_work_loop_evaluation.py
37 passed, 1 warning in 1.97s

velvt-core/.venv/bin/pytest -q tests/test_scope2_work_loop_evaluation.py tests/test_insight_generation_service.py tests/test_insight_provider_service.py tests/test_insight_quality_service.py
46 passed, 1 warning in 1.89s

velvt-core/.venv/bin/ruff check ...
All checks passed

velvt-core/.venv/bin/ruff format --check ...
Formatted/checked successfully

velvt-app/rust-service: cargo test delivery::
74 passed, 0 failed

velvt-app/rust-service: cargo fmt --check
passed

velvt-app/rust-service: cargo clippy -- -D warnings
passed

velvt-app: swift test --package-path swift-client --filter 'NotificationDeliveryCoordinatorTests|UNNotificationSchedulerTests|DisplayDataCoordinatorTests'
51 passed, 0 failed
```

The Swift command initially failed inside the filesystem sandbox because SwiftPM could not write its module cache and nested `sandbox-exec` was denied. It passed when rerun with approved elevated execution. Existing Swift 6 actor-isolation warnings remain; they did not fail this suite.

### Copy-quality grading

Scale: 1 weak, 5 release-quality.

| Copy class | Specificity | Truthfulness | Non-obviousness | Calibration | Novelty | Usefulness | Neutral voice |
|---|---:|---:|---:|---:|---:|---:|---:|
| Switching loop | 5 | 5 | 4 | 5 | 4 | 4 | 5 |
| Stable focus period | 5 | 5 | 3 | 5 | 3 | 4 | 5 |
| Dominant reference activity | 4 | 5 | 2 | 5 | 2 | 3 | 5 |
| Mature baseline deviation/trend | 4 | 4 | 4 | 4 | 4 | 4 | 4 |
| Early baseline | 3 | 5 after copy fix | 2 | 5 | 2 | 2 | 5 |
| Generic recorded activity | 2 | 5 | 1 | 5 | 1 | 1 | 5 |
| Repeated `protect next 20 minutes` action | 2 | 4 | 1 | 4 | 1 | 2 | 4 |

The neutral observer voice is strong: providers cannot author free-form claims, and deterministic copy avoids causal, intent, app, task, and moralizing language. The weak point is usefulness/novelty: every template ends with essentially the same 20-minute recommendation, so the system can feel repetitive even when the observation changes.

## 3 Files changed

- `velvt-core/app/services/insight_evidence_service.py`
  - Reference activity must be the dominant category before it becomes the selected observation.
  - Removed the false fixed “seven-day” baseline claim; configured maturity is currently 14 prior summaries.
- `velvt-core/app/services/insight_quality_service.py`
  - Novelty now compares against every approved insight in the configured window and rejects the closest match.
- `velvt-core/tests/test_scope2_work_loop_evaluation.py`
  - Added minority-reference and baseline-window truthfulness fixtures.
- `velvt-core/tests/test_insight_quality_service.py`
  - Added full-window novelty regression.
- `velvt-app/rust-service/src/delivery/fetch.rs`
  - Latest-insight/display fetch no longer creates a second notification.
  - Added startup duplicate-path regression.
- `velvt-app/rust-service/src/delivery/poll.rs`
  - Generic `recorded_activity` insights remain UI-visible but no longer interrupt users.
  - Added generic-notification suppression regression.
- `velvt-app/rust-service/src/delivery/push.rs`
  - Corrected ownership comment: only an atomically claimed poll result should create a notification.
- `velvt-app/swift-client/Sources/VelvtMac/Delivery/DisplayDataCoordinator.swift`
  - Default latest-insight date now uses the backend's documented UTC calendar day.
- `velvt-app/swift-client/Tests/VelvtMacTests/DisplayDataCoordinatorTests.swift`
  - Added UTC/local-midnight regression.

User-owned `HistoryListView.swift` and packaging/updater files were not modified by this workstream.

## 4 Tests added or executed

Added:

- `test_novelty_gate_checks_the_full_window_not_only_the_latest_insight`
- `minor_reference_activity_does_not_displace_focus_signal`
- `test_early_baseline_copy_does_not_claim_a_different_learning_window`
- `latest_insight_fetch_updates_display_without_scheduling_notification`
- `generic_recorded_activity_delivers_insight_without_notification`
- `testCurrentUTCDateStringDoesNotAdvanceAtLocalMidnightBeforeUTCMidnight`

Executed:

- 87 targeted backend insight/model/workflow/APNs tests in the final integrated rerun.
- 74 Rust delivery tests.
- 51 Swift display/notification tests.
- Rust format and Clippy with warnings denied.
- Python Ruff lint and formatting checks.

Not executed:

- Real LLM/provider calls: intentionally excluded; no credentials or external side effects.
- Real APNs request: mocked only; no APNs credentials.
- Packaged `.app`/DMG launch, notification permission prompt, Notification Center appearance, sleep/wake delivery, or offline-to-online delivery on a clean machine.

## 5 Findings P0-P3

### P0

- None proven in this workstream.

### P1

- **Open — claim is not delivery:** backend marks `DailyInsight.delivered = true` before Rust IPC delivery, Swift permission evaluation, and macOS scheduling. There is no acknowledgement/retry state machine, so accepted-but-not-displayed notifications can be permanently lost.
- **Open — no quiet-hours policy:** prior-day generation occurs at 03:00 UTC and `do_not_disturb_until` is always absent. Immediate local delivery can be late-night/spammy. A timezone-aware product decision and implementation are required.
- **Open — packaged behavior unverified:** no evidence that the signed/notarized app receives IPC payloads and schedules/displays a native notification from `/Applications`, including sleep/wake and restart.
- **Open — evening summary scenario absent:** implementation provides one prior-UTC-day workflow, not a deterministic local-evening summary. If evening summaries are a launch promise, this is a product/implementation mismatch.
- **Fixed — duplicate startup notifications:** proactive/latest fetch and long poll both created notification payloads. Fetch is now display-only; poll claim owns notification creation.
- **Fixed — novelty checked only yesterday:** a duplicate from two to seven days ago could pass when yesterday differed. The gate now checks the full configured window.
- **Fixed — generic interruptions:** mature `recorded_activity` copy could generate a low-value notification. It is now UI-only.
- **Fixed — false baseline duration:** copy claimed a seven-day baseline while default maturity requires 14 prior summaries.
- **Fixed — minority reference false salience:** any positive reference duration could override a much stronger focus observation. Reference must now be the dominant category.
- **Fixed — UTC/local date mismatch:** Swift requested a local calendar date while backend summaries and poll dates use UTC calendar days.

### P2

- **Open — repetitive action copy:** every observation recommends protecting the next 20 minutes for one lane. Token novelty can reject exact repetition, but the product action remains monotonous and may not justify repeated attention.
- **Open — Jaccard novelty is lexical only:** semantically equivalent copy with different wording can pass. Current deterministic templates reduce, but do not eliminate, this risk.
- **Open — delivery is user-global, not device-aware:** backend `delivered` is a single insight flag. The first polling device claims it; other registered Macs do not independently receive a notification. Confirm intended multi-device behavior.
- **Open — no persisted client-side notification ledger:** Swift debounce only covers near-simultaneous same-date payloads in one process. Correctness currently relies on the backend claim.
- **Open — APNs only wakes/fetches:** this is privacy-correct, but real APNs-to-poll latency and wake reliability have not been measured.

### P3

- Existing Swift tests emit actor-isolation warnings that become errors under Swift 6 language mode.
- Test warning: Starlette's current `TestClient`/`httpx` integration is deprecated.

## 6 Open questions or blockers

1. What local delivery window is product-approved (for example 08:00-20:00), and where will device timezone be sourced without widening the behavioral/privacy boundary?
2. Should a notification be retried after permission changes or scheduling failure, and what explicit client acknowledgement should transition `claimed` to `delivered`?
3. Is one notification per user or one per registered device intended?
4. Is “evening summary” a launch requirement? If yes, define local-day semantics, DST behavior, incomplete-day cutoff, and notification cap.
5. Should the repeated 20-minute action rotate among a small reviewed action library based on evidence type, or should low-actionability insights remain UI-only?
6. Distribution owner must verify the final packaged artifact from `/Applications` with a real notification permission grant, denial/re-enable, sleep/wake, app restart, and offline recovery. The artifact must include these source fixes before that evidence is valid.

## 7 Confidence

**High (0.91)** for source-level findings and development-mode behavior: all conclusions are tied to inspected code and reproducible tests. **Low-to-medium (0.45)** for real-world notification delivery because no packaged app, APNs credentials, clean-machine install, sleep/wake cycle, or macOS Notification Center observation was available to this workstream.
