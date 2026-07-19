# macOS signing and Accessibility permission stability

Velvt’s bundle identifier is `com.velvt.mac`. macOS Accessibility (TCC) can only retain a permission grant across rebuilt app binaries when that bundle keeps a stable code-signing identity.

## Before shipping or relying on persistent Accessibility access

1. Enrol in the Apple Developer Program and create/select an Apple Developer team in Xcode.
2. Open `swift-client/VelvtMac.xcodeproj`, select target **velvt-mac**, then open **Signing & Capabilities**.
3. Set Team to the new team, leave Bundle Identifier as `com.velvt.mac`, and enable **Automatically manage signing** for Debug development builds.
4. For distributed builds, create a **Developer ID Application** certificate in Xcode → Settings → Accounts → Manage Certificates, then archive and sign the release with that identity.
5. Remove `CODE_SIGNING_ALLOWED = NO` only after a team is configured. Never commit a certificate, private key, provisioning profile, or Apple ID credential.
6. Install one signed build in `/Applications`, grant Accessibility once in System Settings → Privacy & Security → Accessibility, then rebuild with the same team and bundle ID. The permission should remain recognized.

## Diagnosing a revoked/missing grant

- Velvt checks `AXIsProcessTrustedWithOptions` without prompting during normal monitoring.
- When access is absent, the menu-bar popover presents the explicit **Open Accessibility Settings** recovery action.
- Do not rely on ad-hoc or unsigned build products for persistence testing: they intentionally do not have a stable TCC code requirement.

## App and menu icons

- The status icon is assigned through `NSStatusItem.button.image`; use a monochrome template image so macOS adapts it to the menu-bar appearance.
- The Finder/Dock icon is selected by `ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon` and must be supplied in `Assets.xcassets/AppIcon.appiconset`.

## Private-beta release-readiness handoff

Local ad-hoc packaging is verification only. Before distributing 0.1.5, a human release owner must complete and record every item below:

- [ ] Select the production **Developer ID Application** identity without changing bundle identifier `com.velvt.mac`.
- [ ] Confirm the Release target uses Hardened Runtime and only the reviewed entitlements.
- [ ] Build the Release artifact against the non-localhost hosted beta API URL.
- [ ] Sign the app and every nested executable with the same Developer ID identity; verify with `codesign --verify --deep --strict --verbose=2`.
- [ ] Submit the exact artifact to Apple notarization and save the accepted submission identifier.
- [ ] Staple the notarization ticket, then validate it offline and with `spctl --assess --type execute --verbose=2`.
- [ ] Run clean-Mac acceptance on the oldest supported macOS (13) and a current macOS: checksum, install, first launch, explicit Accessibility and optional Notifications choices, first work block, early local signal, denial recovery, relaunch, guided-tour replay, sign out, delete-account confirmation, and uninstall.
- [ ] Run a hosted-backend smoke using a beta account: auth, privacy-safe queue/upload, retry, Today, Your Week, and account deletion. Do not use localhost or production participant data.
- [ ] Publish the SHA-256 checksum beside the private download instructions and have a second person verify it from the distributed file.
- [ ] Retain a checksum-addressed rollback artifact and its matching participant guide; document who can halt distribution and replace the download.

Do not tag, push, publish, or distribute from a local verification run. The canonical participant path is [Velvt 0.1.5 private-beta guide](private-beta-guide.md).
