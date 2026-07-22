# macOS distribution

Velvt has two deliberately separate distribution modes. A local build proves
the Release bundle and DMG mechanics on the current machine. It is ad-hoc
signed and must never be published. A production release requires a Developer
ID Application identity and accepted Apple notarization; there is no fallback.

## Local packaging verification

```sh
make dmg
make verify-release
```

Outputs:

- `dist/Velvt.app`
- `dist/Velvt.dmg`
- `dist/Velvt.dmg.sha256`

The DMG has a high-resolution Velvt volume icon and a deterministic 660x420
branded install window containing `Velvt.app` and an `/Applications` symlink
for the standard drag-to-Applications flow. Its icon positions, background,
Finder preferences, and root contents are generated without Finder automation
by the hash-locked `dmgbuild` environment prepared by `make prepare-dmg-tool`.
This keeps local and CI output reproducible and avoids capturing host paths in
`.DS_Store`.

Local verification checks Release configuration,
the hosted HTTPS API, the bundled service and taxonomy, protocol agreement,
debug-library exclusion, strict code-sign validity, hardened runtime, declared
architectures, DMG integrity, the volume icon's 1024-pixel representation,
background dimensions, exact root allowlist, icon coordinates, window chrome,
safe layout metadata, and mounted app/helper bytes. `VELVT_RELEASE_ARCHS`
defaults to the
universal `arm64 x86_64` policy; a production release must not weaken that
policy without an explicit support decision.

## Production release

Provision a Developer ID Application certificate and store notarization
credentials in the Keychain once:

```sh
xcrun notarytool store-credentials velvt-notary
```

Then build the production artifact:

```sh
make release \
  VELVT_CODESIGN_IDENTITY="Developer ID Application: Example (TEAMID)" \
  VELVT_NOTARY_PROFILE=velvt-notary \
  VELVT_PRODUCTION_API_BASE_URL=https://api.example.com \
  VELVT_APPROVED_PRODUCTION_API_HOST=api.example.com \
  VELVT_RELEASE_ARCHS="arm64 x86_64"
```

`make release` signs the embedded Rust executable and app inside-out with
hardened runtime, creates and signs the DMG, waits for Apple's notary service,
staples and validates the ticket, performs Gatekeeper assessments, and writes
the checksum after all artifact mutations. The notary response is retained at
`dist/notarization-result.plist`.

Production DMG output paths are immutable: the release fails if either the DMG
or its checksum already exists. Use a new versioned path for every candidate.

The reviewed release entitlements are in
`swift-client/Configs/Release.entitlements`. They are intentionally empty:
Velvt's current Accessibility and local-notification behavior does not need a
private entitlement or App Sandbox exception. Any future capability must be
reviewed before adding an entitlement.

## Clean-machine gate

Before publication, test the exact checksum-addressed DMG on Intel and Apple
Silicon Macs matching the declared architecture policy:

1. Confirm no existing `/Applications/Velvt.app` is present; never overwrite a
   participant's installed copy during a clean-install test.
2. Mount the DMG, drag Velvt to Applications, and launch from Applications.
3. Confirm Gatekeeper opens it without override, then exercise Accessibility
   denial/grant/revocation, Notifications denial/grant, helper startup, quit,
   and relaunch.
4. Confirm local data and permissions behave as documented and record the OS,
   architecture, app version, DMG checksum, signing team, and notary submission
   ID.

An ad-hoc local build cannot satisfy this gate because it has no stable team
identity and no Apple notarization ticket.
