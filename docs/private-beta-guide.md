# Velvt 0.1.5 private-beta guide

Velvt supports macOS 13 Ventura and later. Use only the build and SHA-256 checksum supplied by the beta coordinator; do not redistribute either one.

## Install and begin

1. Confirm the downloaded app matches the supplied checksum, move **velvt-mac.app** to `/Applications`, and open it from Finder.
2. Read the optional intro, or choose **Skip intro** for the 30-second summary. Established launches continue from the intro into the live guided tour. Both also remain available under **Settings → Onboarding & Tour**.
3. On the privacy step, choose **Allow Accessibility** only when ready. macOS opens its permission flow only after that action. If access is denied, use **Open Accessibility Settings** in Velvt to recover.
4. Velvt begins local collection after Accessibility is granted. Open **Today**, enter an optional local intention, and choose **Start Work Block**; no account or notification permission is required for this first value. End the block from the same control.
5. Sign in later with the private-beta account supplied for the cohort when you want synchronized history and cloud-delivered insights. Pre-sign-in observations stay local and are not uploaded retroactively.
6. Notifications are optional and can be enabled from Settings. Declining does not prevent local collection, work blocks, Today, Activity, or Your Week.
7. Keep Velvt running in the menu bar. Today shows observation progress immediately and progressively strengthens its signal as qualifying evidence accumulates. Its maturity label, observation window, and freshness explain how reliable it is.
8. **Your Week** starts with **Today so far** after one observed day, advances to **This week so far** for a partial window, and uses **Week-over-week coaching** only when both weeks have sufficient coverage. Each card states its observed-day coverage and confidence.

## Correct local activity

Select an activity segment in **Your Week → Daily Activity** to rename or categorize it without leaving the row. A local suggestion may appear when Velvt has recognizable on-device application context; choose **Use suggestion** to confirm it, or edit it before saving. Low-confidence suggestions are never silently applied.

Saved names and category rules affect future matching local activity. Search, edit, recategorize, page through, or undo all saved rules under **Settings → Activity & Corrections**. This history is independent of the upload queue and remains available after synchronization and relaunch.

## Privacy

Raw app names, bundle identifiers, window titles, browser tabs, URLs, filenames, paths, contacts, icons, favicons, work-block intentions, local suggestions, and user-created activity aliases stay on this Mac. Approved broad categories, coarse durations, timestamps, and safe summaries may synchronize for beta insights.

Velvt does not add product analytics or telemetry. Diagnostics copied from **Settings → App Info** contain status codes and coarse counts, not raw activity. Before sharing a screenshot, redact unrelated personal content visible elsewhere on the Mac.

## Everyday recovery and account controls

- **Working offline:** continue normally. Privacy-safe batches can queue locally; the header and App Info show synchronization state. After connectivity returns, choose **Retry Backend Synchronization** if automatic retry has not recovered.
- **Local service unavailable:** wait through the short startup/reconnect grace period. If the state remains unavailable, use **Restart Local Service** in App Info, then quit and reopen Velvt if needed. Relaunching replays the short intro and guided tour without resetting account, permission, or Today state.
- **Accessibility denied or revoked:** collection pauses. Choose **Open Accessibility Settings**, enable Velvt, and return to the app. No permission is marked granted merely by skipping onboarding.
- **Sign out:** choose **Log Out** at the bottom of the main popover. This clears the local authenticated session; it does not pretend local data was deleted.
- **Delete the account:** while signed in, choose **Delete Account** and confirm the destructive request. If the service is unreachable, Velvt returns to the signed-in state so the request can be retried.

## Uninstall

First sign out or request account deletion as appropriate, then quit Velvt and move `/Applications/velvt-mac.app` to the Trash. Remove Velvt from **System Settings → Privacy & Security → Accessibility** and **Notifications**. App removal does not itself erase local service data or Keychain state. For a complete local reset, remove `~/.velvt` only after quitting Velvt and only if permanent deletion of local history is intended; contact beta support before doing so if evidence is needed for an open bug.

## Support and known limitations

Use the support channel named in the beta invitation. Include Velvt version, macOS version, the visible state, the preceding action, and whether the Mac had just slept, woken, restarted, signed out, or reconnected. Never send app names, window titles, URLs, filenames, paths, contacts, intentions, credentials, the SQLite database, or unredacted screenshots.

Known limitations for 0.1.5:

- It is a menu-bar app for macOS 13 or later; there is no dashboard or cross-platform client.
- Early local signals need qualifying observed activity and may remain in progress during a short or sparse session.
- A true week-over-week comparison requires at least four observed current-week days and four observed prior-week days. Earlier tiers remain explicitly partial.
- Notifications are local delivery of approved insight copy; remote APNs and adaptive notifications are not promised by this beta.
- Ad-hoc local verification builds do not provide the stable signing identity required for durable Accessibility permission across rebuilds.

Advanced development and service troubleshooting stays in [Quickstart](quickstart.md) and the architecture documents; it is intentionally outside this participant path.
