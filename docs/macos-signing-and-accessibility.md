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
- The Finder/Dock icon is selected by `ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon` and must be supplied in `Resources/Assets.xcassets/AppIcon.appiconset`.
