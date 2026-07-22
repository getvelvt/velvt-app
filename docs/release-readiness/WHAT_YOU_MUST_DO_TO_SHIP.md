# What You Must Do Before Velvt Can Ship

This list contains only actions that require your identity, authority, accounts,
hardware, consent, or business decisions. Build commands, signing automation,
release packaging, updater publication, test execution, SBOM/provenance work,
and code changes are intentionally not assigned to you.

## 1. Provide Apple release authority

- [ ] Install or make available a **Developer ID Application** certificate and
  tell Codex its full identity string (`VELVT_CODESIGN_IDENTITY`).
- [ ] Create a `notarytool` Keychain profile and tell Codex only the profile
  name (`VELVT_NOTARY_PROFILE`). Do not put Apple credentials or certificate
  private keys in chat.
- [ ] Confirm the Apple Team ID that is authorized to publish Velvt.

Codex will then perform signing, nested-signature validation, notarization,
stapling, Gatekeeper checks, and evidence capture.

## 2. Approve production service addresses

- [ ] Provide the exact production backend URL and separately approve its exact
  hostname:
  - `VELVT_PRODUCTION_API_BASE_URL`
  - `VELVT_APPROVED_PRODUCTION_API_HOST`
- [ ] Confirm you control that domain and that its production TLS certificate
  and health endpoint are live.
- [ ] Provide the final HTTPS update feed URL and archive base URL:
  - `VELVT_UPDATE_FEED_URL`
  - `VELVT_UPDATE_BASE_URL`
- [ ] Give Codex authorized access to publish immutable versioned archives and
  atomically replace the appcast. Do not send hosting passwords in chat.

## 3. Establish update-key custody

- [ ] Choose where the production Sparkle Ed25519 private key will live: an
  unlocked protected Keychain/secret store, or an external file with mode
  `0400` or `0600`.
- [ ] Give Codex the protected key reference/path and the public key
  (`VELVT_UPDATE_PUBLIC_ED_KEY`). Never paste the private-key contents in chat.
- [ ] Name the people responsible for key backup, recovery, rotation,
  revocation, and a compromised-key incident.

Codex will hash and verify the Sparkle tools, generate/sign the immutable
archive and appcast, publish archive-first/appcast-last, and run the adversarial
update matrix.

## 4. Approve release and support policy

- [ ] Approve the release version, increasing build number, and previous build:
  - `VELVT_RELEASE_VERSION`
  - `VELVT_RELEASE_BUILD`
  - `VELVT_PREVIOUS_RELEASE_BUILD`
- [ ] Approve the minimum supported macOS version.
- [ ] Confirm that production support remains universal **Apple Silicon and
  Intel** (`arm64 x86_64`), or explicitly approve a different policy.
- [ ] Name owners for feed withdrawal, archive retention, forward repair,
  customer communication, backend backup/restore, monitoring, and secret
  rotation.

## 5. Provide clean-machine and consent access

- [ ] Provide one clean Intel Mac and one clean Apple-Silicon Mac, or authorized
  remote access to both, with permission to install into `/Applications`.
- [ ] During the supervised test, manually grant, deny, revoke, and regrant
  Accessibility and Notifications when macOS prompts. Codex cannot click those
  privacy-consent prompts for you.
- [ ] Provide a production-like test account and permission to create a
  privacy-safe local fixture for the version-N to version-N+1 update test. Do
  not use real customer activity.

Codex will run the clean install, quarantine/Gatekeeper, sleep/wake,
helper-kill, offline recovery, state preservation, update/relaunch, and failure
recovery ledger around those manual consent steps.

## 6. Give final ship approval

- [ ] Review the final signed-artifact ledger, notary IDs, published hashes,
  clean-machine results, updater matrix, privacy evidence, operational-owner
  list, and remaining risk statement.
- [ ] Explicitly approve or reject public release. Only you can authorize the
  production publication decision.
