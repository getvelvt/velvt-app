#!/usr/bin/env bash
set -euo pipefail

app_path="${1:-dist/velvt-mac.app}"
if [[ ! -d "$app_path" ]]; then
  echo "ERROR: Release app not found at '$app_path'." >&2
  exit 1
fi

app_path="$(cd "$(dirname "$app_path")" && pwd -P)/$(basename "$app_path")"
plist="$app_path/Contents/Info.plist"
resources="$app_path/Contents/Resources"
helper="$resources/velvt-service"

read_plist() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$plist"
}

configuration="$(read_plist VelvtBuildConfiguration)"
distributable="$(read_plist VelvtDistributable)"
api_url="$(read_plist VelvtAPIBaseURL)"
app_protocol="$(read_plist VelvtProtocolVersion)"

[[ "$configuration" == "Release" ]] || {
  echo "ERROR: artifact configuration is '$configuration', expected Release." >&2
  exit 1
}
[[ "$distributable" == "YES" ]] || {
  echo "ERROR: artifact is not marked distributable." >&2
  exit 1
}
"$(dirname "$0")/preflight_distribution.sh" "$api_url" >/dev/null
[[ -x "$helper" ]] || {
  echo "ERROR: embedded helper is missing or not executable." >&2
  exit 1
}
[[ -r "$resources/abstraction-taxonomy-mvp-1.json" ]] || {
  echo "ERROR: helper taxonomy resource is missing." >&2
  exit 1
}

helper_protocol="$($helper --protocol-version)"
[[ "$helper_protocol" == "$app_protocol" ]] || {
  echo "ERROR: app protocol $app_protocol does not match helper protocol $helper_protocol." >&2
  exit 1
}

if find "$app_path" -type f \( \
  -iname '*preview*.dylib' -o \
  -iname '*debug*.dylib' -o \
  -name '__preview.dylib' \
\) -print -quit | grep -q .; then
  echo "ERROR: Release artifact contains a preview/debug dylib." >&2
  exit 1
fi

codesign --verify --deep --strict --verbose=2 "$app_path"

echo "Release verification passed"
echo "  artifact: $app_path"
echo "  configuration: $configuration"
echo "  API URL: $api_url"
echo "  protocol: $app_protocol"
echo "  helper: embedded"
echo "  preview/debug dylibs: absent"
echo "  codesign: valid"
