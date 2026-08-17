#!/usr/bin/env bash
set -euo pipefail

app_path="${1:-dist/Velvt.app}"
dmg_path="${2:-dist/Velvt.dmg}"
volume_name="${VELVT_DMG_VOLUME_NAME:-Velvt}"
mode="${3:-local}"
dmgbuild_bin="${VELVT_DMGBUILD_BIN:-$(dirname "$0")/../.build-tools/dmgbuild/bin/dmgbuild}"

[[ -d "$app_path" ]] || { echo "ERROR: app not found: $app_path" >&2; exit 1; }
[[ ! -L "$app_path" ]] || { echo "ERROR: app input must not be a symlink" >&2; exit 1; }
[[ "$dmg_path" == *.dmg ]] || { echo "ERROR: DMG output must end in .dmg" >&2; exit 1; }
[[ "$mode" == "local" || "$mode" == "production" ]] || { echo "ERROR: mode must be local or production" >&2; exit 2; }
[[ "$volume_name" == "Velvt" ]] || { echo "ERROR: release DMG volume name must be Velvt" >&2; exit 1; }
[[ -x "$dmgbuild_bin" ]] || { echo "ERROR: pinned dmgbuild is unavailable; run: make prepare-dmg-tool" >&2; exit 1; }

app_path="$(cd "$(dirname "$app_path")" && pwd -P)/$(basename "$app_path")"
dmg_dir="$(cd "$(dirname "$dmg_path")" && pwd -P)"
dmg_path="$dmg_dir/$(basename "$dmg_path")"

if [[ "$mode" == "production" ]]; then
  [[ ! -e "$dmg_path" && ! -e "$dmg_path.sha256" ]] || {
    echo "ERROR: production DMG outputs are immutable; choose a new versioned path" >&2
    exit 1
  }
else
  rm -f "$dmg_path" "$dmg_path.sha256"
fi

staging="$(mktemp -d "${TMPDIR:-/tmp}/velvt-dmg.XXXXXX")"
output_tmp="$dmg_dir/.$(basename "$dmg_path" .dmg).tmp.$$.dmg"
cleanup() {
  rm -rf "$staging"
  rm -f "$output_tmp"
}
trap cleanup EXIT

icon_source="$(dirname "$0")/../swift-client/Resources/icons"
iconset="$staging/VelvtVolume.iconset"
mkdir -p "$iconset"
cp "$icon_source/16.png" "$iconset/icon_16x16.png"
cp "$icon_source/32.png" "$iconset/icon_16x16@2x.png"
cp "$icon_source/32.png" "$iconset/icon_32x32.png"
cp "$icon_source/64.png" "$iconset/icon_32x32@2x.png"
cp "$icon_source/128.png" "$iconset/icon_128x128.png"
cp "$icon_source/256.png" "$iconset/icon_128x128@2x.png"
cp "$icon_source/256.png" "$iconset/icon_256x256.png"
cp "$icon_source/512.png" "$iconset/icon_256x256@2x.png"
cp "$icon_source/512.png" "$iconset/icon_512x512.png"
cp "$icon_source/1024.png" "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$staging/VelvtVolume.icns"
rm -rf "$iconset"

xcrun swift "$(dirname "$0")/render_dmg_background.swift" \
  "$icon_source/1024.png" "$staging/background.png"

"$dmgbuild_bin" \
  -s "$(dirname "$0")/dmg_settings.py" \
  -D "app=$app_path" \
  -D "background=$staging/background.png" \
  -D "volume_icon=$staging/VelvtVolume.icns" \
  "$volume_name" "$output_tmp"
mv "$output_tmp" "$dmg_path"

# dmgbuild exits 0 even when its internal `ditto` fails, so a DMG can be
# produced containing the window layout and nothing else. That happened twice:
# macOS App Management blocks writing a bundle named `.app` into /Volumes
# unless the calling process holds that permission, and the resulting empty
# image then signed, notarized, stapled, and checksummed without complaint.
# Only the final content check caught it, eight minutes and two notarization
# round-trips later.
#
# Assert the app is present before anything downstream spends time on it.
verify_mount="$(mktemp -d "${TMPDIR:-/tmp}/velvt-dmg-assert.XXXXXX")"
assert_cleanup() {
  hdiutil detach "$verify_mount" -quiet >/dev/null 2>&1 || true
  rmdir "$verify_mount" >/dev/null 2>&1 || true
}
trap 'assert_cleanup; cleanup' EXIT
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$verify_mount" "$dmg_path"
if [[ ! -d "$verify_mount/$(basename "$app_path")" ]]; then
  echo "ERROR: the DMG was created without $(basename "$app_path")." >&2
  echo "dmgbuild reports success even when its copy fails. The usual cause is" >&2
  echo "macOS App Management: grant the terminal running this build App" >&2
  echo "Management in System Settings > Privacy & Security, then retry." >&2
  rm -f "$dmg_path"
  exit 1
fi
assert_cleanup
trap cleanup EXIT

hdiutil verify "$dmg_path"
if [[ "$mode" == "local" ]]; then
  (
    cd "$dmg_dir"
    shasum -a 256 "$(basename "$dmg_path")" > "$(basename "$dmg_path").sha256"
  )
fi

echo "Created $dmg_path"
if [[ "$mode" == "local" ]]; then
  echo "Created $dmg_path.sha256"
else
  echo "Production checksum deferred until signing, notarization, and stapling complete"
fi
