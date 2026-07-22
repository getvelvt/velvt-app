#!/usr/bin/env bash
set -euo pipefail

dmg_path="${1:-dist/Velvt.dmg}"
profile="${VELVT_NOTARY_PROFILE:-}"
result_path="${VELVT_NOTARY_RESULT_PATH:-dist/notarization-result.plist}"

[[ -f "$dmg_path" ]] || { echo "ERROR: DMG not found: $dmg_path" >&2; exit 1; }
[[ -n "$profile" ]] || {
  echo "ERROR: notarization requires VELVT_NOTARY_PROFILE naming a notarytool Keychain profile." >&2
  echo "Create it with: xcrun notarytool store-credentials PROFILE_NAME" >&2
  exit 1
}

xcrun notarytool submit "$dmg_path" \
  --keychain-profile "$profile" \
  --wait \
  --output-format plist > "$result_path"

status="$(/usr/libexec/PlistBuddy -c 'Print :status' "$result_path")"
submission_id="$(/usr/libexec/PlistBuddy -c 'Print :id' "$result_path")"
if [[ "$status" != "Accepted" ]]; then
  echo "ERROR: Apple notarization status is '$status' (submission $submission_id)." >&2
  echo "Inspect: xcrun notarytool log $submission_id --keychain-profile '$profile'" >&2
  exit 1
fi

xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"

echo "Notarization accepted and ticket stapled"
echo "  submission: $submission_id"
echo "  result: $result_path"
