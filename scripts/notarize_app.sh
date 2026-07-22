#!/usr/bin/env bash
set -euo pipefail

app_path="${1:-dist/Velvt.app}"
profile="${VELVT_NOTARY_PROFILE:-}"
result_path="${VELVT_APP_NOTARY_RESULT_PATH:-dist/app-notarization-result.plist}"

[[ -d "$app_path" ]] || { echo "ERROR: app not found: $app_path" >&2; exit 1; }
[[ -n "$profile" ]] || {
  echo "ERROR: app notarization requires VELVT_NOTARY_PROFILE naming a notarytool Keychain profile." >&2
  exit 1
}

staging="$(mktemp -d "${TMPDIR:-/tmp}/velvt-app-notary.XXXXXX")"
cleanup() { rm -rf "$staging"; }
trap cleanup EXIT
archive="$staging/Velvt-notarization.zip"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$archive"
xcrun notarytool submit "$archive" \
  --keychain-profile "$profile" \
  --wait \
  --output-format plist > "$result_path"

status="$(/usr/libexec/PlistBuddy -c 'Print :status' "$result_path")"
submission_id="$(/usr/libexec/PlistBuddy -c 'Print :id' "$result_path")"
[[ "$status" == "Accepted" ]] || {
  echo "ERROR: Apple app notarization status is '$status' (submission $submission_id)." >&2
  exit 1
}
xcrun stapler staple "$app_path"
xcrun stapler validate "$app_path"
spctl --assess --type execute --verbose=2 "$app_path"

echo "Application notarization accepted and ticket stapled"
echo "  submission: $submission_id"
echo "  result: $result_path"
