# Velvt closed-alpha release plan

## Cohort and duration

Recruit 10–20 macOS knowledge workers for two weeks. Include a mix of document-heavy,
communication-heavy, technical, and creative work. This is a learning cohort, not a source of
statistically representative product claims. Do not publish or fabricate outcomes.

## Facilitated setup script

1. Ask the participant to share their screen and think aloud. Do not explain Velvt yet.
2. Provide the signed build and ask them to open it from Finder.
3. Start a one-minute timer. Ask: “What do you think Velvt will help you understand?” Record the
   answer without coaching.
4. Ask them to continue setup. Observe whether the Accessibility explanation is sufficient before
   macOS System Settings opens.
5. Let them accept or decline Notifications. Confirm verbally that either choice permits collection.
6. Ask them to sign in. Do not supply UI instructions unless they are blocked for two minutes.
7. Confirm the app says local collection has started and shows real baseline progress.
8. Ask them to open Today, Your Week, Activity, and all four Settings destinations using only the
   keyboard once.
9. Leave Velvt running through one sleep/wake cycle and confirm that activity continues without a
   connection-state failure or duplicate interval.
10. Ask the participant to file one practice bug report using the instructions below.

## Interview questions

- After one minute, what do you believe Velvt does?
- What did you expect Accessibility access to reveal or transmit?
- What is a “meaningful switch” in your own words?
- What does the fragmentation comparison tell you? What remains unclear?
- Is the suggested action realistic for your next work period? Why or why not?
- Did “Why am I seeing this?” provide enough evidence to trust the observation?
- Which part felt judgmental, vague, or overly technical?
- Did a notification arrive at a useful time? Was its wording proportionate to the evidence?
- When did you choose to reopen Velvt during the week?
- What would make you remove Velvt from your Mac?

## Bug reports

Participants should include:

- Velvt version and macOS version;
- the visible state and the action immediately before the problem;
- expected and actual behavior;
- a screenshot with personal content redacted;
- whether the Mac had just slept, woken, restarted, logged out, or reconnected.

Never request app names, window titles, URLs, filenames, raw activity labels, database files,
credentials, or copied logs containing personal content. A team member assigns severity (P0 data or
privacy loss, P1 blocked core flow, P2 degraded flow, P3 polish), reproduces with synthetic data,
and links the report to the daily review log.

## Participant privacy explanation

Velvt observes activity locally through macOS Accessibility permission, converts it into broad work
categories at the on-device privacy boundary, and sends only approved aggregate categories,
timestamps, durations, summaries, and evidence metadata to the backend. Raw app names, bundle IDs,
window titles, URLs, filenames, paths, contacts, and local intention text do not leave the Mac.
Notifications contain reviewed evidence-based copy; early baseline collection is not notified. A
participant may decline Notifications, pause collection by removing Accessibility access, log out,
or request account deletion. Alpha participation is voluntary and can end at any time.

## Daily review

- Review new P0–P3 reports and privacy-boundary test results.
- Check setup notes for unaided permission completion and one-minute purpose comprehension.
- Reproduce sleep, wake, restart, logout, and reconnect reports with synthetic activity.
- Audit each observed insight: observation, baseline comparison, suggested action, evidence
  disclosure, emotional stage, and notification eligibility.
- Record qualitative usefulness feedback without raw activity or personal content.
- Do not change the build or participant instructions mid-cohort without noting the version boundary.

## Weekly review

- At the end of week one, aggregate setup completion, time to first useful observation, inspectable
  evidence, and participant usefulness ratings.
- At the end of week two, calculate return participation from scheduled interviews or voluntary
  check-ins, review retention qualitatively, and decide whether each release gate passed, failed, or
  lacks evidence.
- Review all open P0/P1 defects and every possible lost/duplicated-activity report before expanding
  the cohort.

## Release gates

- 80% can explain Velvt’s purpose after one minute.
- 90% complete permissions without assistance.
- First useful observation within five minutes.
- No connection-state flickering during normal sleep/wake.
- Every insight has inspectable evidence.
- At least 60% rate one first-week insight as useful.
- At least 40% return during week two.
- No lost or duplicated activity across sleep, restart, logout, or reconnect.
- No open P0/P1 defects.

Expansion requires every privacy, integrity, flicker, evidence, and P0/P1 gate to pass. Cohort
comprehension, usefulness, and return gates may be iterated only with a documented product change
and a new alpha round; they must not be waived by reinterpretation.

## Measurement boundary

Use facilitator timestamps, scheduled interviews, opt-in surveys, and de-identified issue counts for
this alpha. Do not add invasive analytics. If later measurement needs product telemetry, first seek
explicit approval for a privacy-safe event containing only an event identifier (for example,
`onboarding_collection_confirmed` or `evidence_disclosure_opened`), coarse app version, and timestamp.
The proposal must document retention, aggregation, user control, and why existing research methods
are insufficient before implementation.
