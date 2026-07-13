# MVP Tester Onboarding And Troubleshooting

Audience: founder/internal testers using the macOS MVP build.

## Install

1. Quit any running Velvt build.
2. Move `velvt-mac.app` to `/Applications`.
3. Launch Velvt.
4. Grant Accessibility in System Settings > Privacy & Security > Accessibility.
5. Grant Notifications when prompted.

Use only the signed/notarized artifact for broad beta. Ad-hoc builds are for
internal smoke only and may require re-granting Accessibility after rebuilds.

## Permissions

Velvt requests:

- Accessibility, so it can observe active app/window changes.
- Notifications, so it can show ready insight notifications.

Velvt does not request Screen Recording, Input Monitoring, camera, microphone,
screenshot capture, keylogging, or filesystem monitoring permissions.

If Accessibility stops working after a rebuild:

1. Quit Velvt.
2. Remove Velvt from System Settings > Privacy & Security > Accessibility.
3. Reopen Velvt.
4. Add Velvt back to Accessibility.

Stable Apple Developer ID signing should make this unnecessary for real beta
builds.

## Diagnostics To Send

Ask testers to send:

- macOS version.
- Velvt app version.
- Whether Accessibility and Notifications show as granted.
- A screenshot of the Velvt menu popover if it is stuck or showing an error.
- The latest menu upload diagnostics: pending uploads, retry time, and last error code.

Do not ask testers to send raw local database files unless they explicitly
understand that those files may contain local-only raw activity state.

## Data Boundary

Raw app names, bundle IDs, window titles, URLs, paths, filenames, contacts, and
other identifying activity details stay on the Mac. The local service uploads
privacy-safe abstracted activity batches to the backend for summaries and
insights.

## Reset Or Uninstall

Quit Velvt, then remove the app:

```bash
rm -rf /Applications/velvt-mac.app
```

Reset local test data only when intentionally starting a clean smoke pass:

```bash
rm -rf ~/.velvt
```

Account deletion should be tested from the app UI so the backend erasure path
is exercised. Local reset is not a substitute for account deletion.
