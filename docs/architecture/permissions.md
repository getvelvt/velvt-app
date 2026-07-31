# macOS Permission Handling

## Scope

Velvt requests exactly two macOS permissions:

1. **Accessibility** allows the client to detect which application and window
   are focused. It does not grant Velvt screen recording, microphone, camera,
   contacts, location, or filesystem access.
2. **Notifications** allow the client to deliver daily insights supplied by the
   Rust service. Denying notifications does not disable collection or prevent
   insights from appearing in the menu bar popover.

`PermissionType` is the exhaustive compile-time permission allowlist. Adding a
case requires explicit PR review. Swift extensions cannot add enum cases, so a
test-only third `PermissionType` cannot bypass this boundary.

## Ownership

`PermissionManagerProtocol` is the only permission interface consumed by app,
UI, and collection-coordination code. Tests use `FakePermissionManager` and do
not trigger system dialogs.

All system permission API calls live in
`swift-client/Sources/VelvtMac/Permissions/PermissionManager.swift`.
The permission layer has no IPC, SQLite, upload, or cloud responsibilities.

Permission state is published reactively as:

```swift
AnyPublisher<[PermissionType: PermissionStatus], Never>
```

`PermissionStatus` has four states: `unknown`, `granted`, `denied`, and
`restricted`.

## Accessibility Monitoring And Recovery

macOS does not publish an Accessibility-revocation callback. While Velvt is
foregrounded, `PermissionManager` performs a lightweight check every five
seconds. The monitor timer pauses when the app resigns active and resumes when
the app becomes active; no background timer cycle calls the Accessibility API.

When Accessibility becomes denied or restricted:

- `PermissionCollectionCoordinator` stops the collection agent.
- The menu bar icon changes to a warning indicator.
- The popover explains that collection is paused and offers an
  **Open Accessibility Settings** button.

To recover:

1. Open the Velvt menu bar popover.
2. Select **Open Accessibility Settings**.
3. Enable Velvt in **System Settings > Privacy & Security > Accessibility**.
4. Return to Velvt. The next foreground re-check restarts collection.

## Notification Denial

Notification status checks use `getNotificationSettings()`. First-run
onboarding does not request notification authorization; the optional request is
deferred until the user enables notifications from the live product flow.
Once a request returns denied, `PermissionManager` guards against calling
`requestAuthorization` again. The app remains functional and displays insights
in the menu bar.

## Adding A Permission

Adding a permission is intentionally review-gated:

1. Add one case to `PermissionType`.
2. Add its system check and request adapter inside `PermissionManager.swift`.
3. Add onboarding rationale and recovery behavior where required.
4. Add fake-manager transition, denial, recovery, and API-audit tests.
5. Update this document and verify the permission API audit.

Do not add permission API calls to collection, delivery, app, or UI modules.

## Verification

```sh
swift test --package-path swift-client
xcodebuild -quiet \
  -project swift-client/VelvtMac.xcodeproj \
  -scheme velvt-mac \
  -destination 'generic/platform=macOS' \
  build

rg -n "requestAlwaysAuthorization|requestWhenInUseAuthorization|AVCaptureDevice|requestAccess|CNContactStore|CLLocationManager|CGRequestScreenCaptureAccess|CGPreflightScreenCaptureAccess|EKEventStore|SFSpeechRecognizer|AXIsProcessTrusted|UNUserNotificationCenter|requestAuthorization" \
  swift-client/Sources \
  --glob '*.swift'
```

The audit may report approved API calls only in
`Permissions/PermissionManager.swift`.
