#!/usr/bin/env bash
set -euo pipefail

mode="local"
app_path="dist/Velvt.app"
dmg_path=""
required_archs="${VELVT_RELEASE_ARCHS:-$(uname -m)}"
script_dir="$(cd "$(dirname "$0")" && pwd -P)"
layout_python="$script_dir/../.build-tools/dmgbuild/bin/python"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --app) app_path="${2:-}"; shift 2 ;;
    --dmg) dmg_path="${2:-}"; shift 2 ;;
    *)
      # Preserve the historical `verify_release.sh path/to/App.app` interface.
      app_path="$1"
      shift
      ;;
  esac
done

[[ "$mode" == "local" || "$mode" == "production" ]] || {
  echo "ERROR: --mode must be local or production." >&2
  exit 2
}
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
bash "$script_dir/verify_helper_portability.sh" "$helper"

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

sparkle_framework="$app_path/Contents/Frameworks/Sparkle.framework"
nested_code=()
if [[ -d "$sparkle_framework" ]]; then
  nested_code+=("$sparkle_framework")
  while IFS= read -r nested_path; do nested_code+=("$nested_path"); done < <(
    find "$sparkle_framework" -type d \( -name '*.xpc' -o -name '*.app' \) -print
  )
  while IFS= read -r nested_path; do nested_code+=("$nested_path"); done < <(
    find "$sparkle_framework" -type f -name Autoupdate -perm -111 -print
  )
  for nested_path in "${nested_code[@]}"; do
    codesign --verify --strict --verbose=2 "$nested_path"
  done
fi

signature_details="$(codesign -dvvv "$app_path" 2>&1)"
if ! grep -q 'flags=.*runtime' <<<"$signature_details"; then
  echo "ERROR: app signature does not enable hardened runtime." >&2
  exit 1
fi

if codesign -d --entitlements :- "$app_path" 2>/dev/null | grep -q 'com.apple.security.get-task-allow'; then
  echo "ERROR: Release artifact contains the debug get-task-allow entitlement." >&2
  exit 1
fi

for executable in "$app_path/Contents/MacOS/Velvt" "$helper"; do
  actual_archs="$(lipo -archs "$executable")"
  for arch in $required_archs; do
    if [[ " $actual_archs " != *" $arch "* ]]; then
      echo "ERROR: $(basename "$executable") is missing required architecture '$arch' (has: $actual_archs)." >&2
      exit 1
    fi
  done
done

# Every load path must be relative to the bundle. A build once shipped with an
# absolute rpath into the build machine's checkout, so on any other Mac dyld
# looked for Sparkle at a path that did not exist and killed the process before
# main() — a crash on launch that reads to the recipient as "your app is
# broken", with no way to tell it from one.
#
# The failure is invisible on the machine that produced it, because there the
# path resolves. It can only be caught here.
for executable in "$app_path/Contents/MacOS/Velvt" "$helper"; do
  while read -r rpath; do
    [[ -n "$rpath" ]] || continue
    case "$rpath" in
      @executable_path/* | @loader_path/* | @rpath/*) ;;
      *)
        echo "ERROR: $(basename "$executable") carries a non-relative rpath: $rpath" >&2
        echo "It resolves only on the machine that built it and will crash on launch elsewhere." >&2
        exit 1
        ;;
    esac
  done < <(otool -l "$executable" | awk '/LC_RPATH/{found=1} found && /^ *path /{print $2; found=0}')
done

# Belt and braces: a build-machine home directory anywhere in the shipped
# binaries means something was baked in that cannot exist on a user's Mac.
for executable in "$app_path/Contents/MacOS/Velvt" "$helper"; do
  if strings -a "$executable" | grep -q "^/Users/[^/]*/.*velvt"; then
    echo "ERROR: $(basename "$executable") embeds an absolute build-machine path." >&2
    strings -a "$executable" | grep "^/Users/[^/]*/.*velvt" | head -3 >&2
    exit 1
  fi
done

if [[ "$mode" == "production" ]]; then
  grep -q '^Authority=Developer ID Application:' <<<"$signature_details" || {
    echo "ERROR: production app is not signed with a Developer ID Application identity." >&2
    exit 1
  }
  team_id="$(sed -n 's/^TeamIdentifier=//p' <<<"$signature_details")"
  [[ -n "$team_id" && "$team_id" != "not set" ]] || {
    echo "ERROR: production app signature has no TeamIdentifier." >&2
    exit 1
  }
  [[ -d "$sparkle_framework" ]] || {
    echo "ERROR: production app does not embed Sparkle.framework." >&2
    exit 1
  }
  for nested_path in "${nested_code[@]}" "$helper"; do
    nested_signature="$(codesign -dvvv "$nested_path" 2>&1)"
    nested_team_id="$(sed -n 's/^TeamIdentifier=//p' <<<"$nested_signature")"
    [[ "$nested_team_id" == "$team_id" ]] || {
      echo "ERROR: nested code has TeamIdentifier '$nested_team_id', expected '$team_id': $nested_path" >&2
      exit 1
    }
    grep -q '^Authority=Developer ID Application:' <<<"$nested_signature" || {
      echo "ERROR: nested code is not signed with a Developer ID Application identity: $nested_path" >&2
      exit 1
    }
    grep -q 'flags=.*runtime' <<<"$nested_signature" || {
      echo "ERROR: nested code does not enable hardened runtime: $nested_path" >&2
      exit 1
    }
  done
  downloader_xpc="$(find "$sparkle_framework" -type d -name Downloader.xpc -print -quit)"
  [[ -n "$downloader_xpc" ]] || {
    echo "ERROR: Sparkle Downloader.xpc is missing." >&2
    exit 1
  }
  downloader_entitlements="$(codesign -d --entitlements :- "$downloader_xpc" 2>/dev/null || true)"
  [[ -n "$downloader_entitlements" && "$downloader_entitlements" == *'<dict>'* ]] || {
    echo "ERROR: Sparkle Downloader.xpc entitlements were not preserved." >&2
    exit 1
  }
  spctl --assess --type execute --verbose=2 "$app_path"
fi

if [[ -n "$dmg_path" ]]; then
  [[ -f "$dmg_path" ]] || { echo "ERROR: DMG not found at '$dmg_path'." >&2; exit 1; }
  [[ -f "$dmg_path.sha256" ]] || { echo "ERROR: DMG checksum not found at '$dmg_path.sha256'." >&2; exit 1; }
  (
    cd "$(dirname "$dmg_path")"
    shasum -a 256 -c "$(basename "$dmg_path").sha256"
  )
  hdiutil verify "$dmg_path"
  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/velvt-verify.XXXXXX")"
  cleanup_mount() {
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
    rmdir "$mount_point" >/dev/null 2>&1 || true
  }
  trap cleanup_mount EXIT
  hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mount_point" "$dmg_path"
  [[ -d "$mount_point/Velvt.app" ]] || { echo "ERROR: DMG does not contain Velvt.app." >&2; exit 1; }
  [[ -L "$mount_point/Applications" && "$(readlink "$mount_point/Applications")" == "/Applications" ]] || {
    echo "ERROR: DMG does not contain the drag-to-Applications link." >&2
    exit 1
  }
  [[ -f "$mount_point/.VolumeIcon.icns" && ! -L "$mount_point/.VolumeIcon.icns" ]] || {
    echo "ERROR: DMG volume icon is missing or unsafe." >&2
    exit 1
  }
  [[ -f "$mount_point/.background.png" && ! -L "$mount_point/.background.png" ]] || {
    echo "ERROR: DMG background is missing or unsafe." >&2
    exit 1
  }
  while IFS= read -r root_item; do
    case "$(basename "$root_item")" in
      Velvt.app|Applications|.VolumeIcon.icns|.background.png|.DS_Store) ;;
      *) echo "ERROR: unexpected DMG root item: $(basename "$root_item")" >&2; exit 1 ;;
    esac
  done < <(find "$mount_point" -mindepth 1 -maxdepth 1 -print)
  [[ "$(GetFileInfo -a "$mount_point")" == *C* ]] || {
    echo "ERROR: DMG root does not have the custom volume-icon flag." >&2
    exit 1
  }
  background_width="$(sips -g pixelWidth "$mount_point/.background.png" | awk '/pixelWidth:/ {print $2}')"
  background_height="$(sips -g pixelHeight "$mount_point/.background.png" | awk '/pixelHeight:/ {print $2}')"
  [[ "$background_width" == "660" && "$background_height" == "420" ]] || {
    echo "ERROR: DMG background must be 660x420 pixels." >&2
    exit 1
  }
  icon_check_root="$(mktemp -d "${TMPDIR:-/tmp}/velvt-icon-check.XXXXXX")"
  icon_check_dir="$icon_check_root/VelvtVolume.iconset"
  iconutil -c iconset "$mount_point/.VolumeIcon.icns" -o "$icon_check_dir"
  [[ -f "$icon_check_dir/icon_512x512@2x.png" ]] || {
    echo "ERROR: DMG volume icon lacks its 1024-pixel representation." >&2
    rm -rf "$icon_check_root"
    exit 1
  }
  rm -rf "$icon_check_root"
  [[ -f "$mount_point/.DS_Store" ]] || { echo "ERROR: polished DMG layout is missing." >&2; exit 1; }
  [[ -x "$layout_python" ]] || {
    echo "ERROR: pinned DMG verifier is unavailable; run: make prepare-dmg-tool" >&2
    exit 1
  }
  "$layout_python" "$script_dir/verify_dmg_layout.py" "$mount_point/.DS_Store"
  if strings "$mount_point/.DS_Store" | grep -Eq '/Users/|https?://'; then
    echo "ERROR: DMG layout contains a host path or network URL." >&2
    exit 1
  fi
  if strings "$mount_point/.DS_Store" | grep 'file://' | grep -Fvx 'file:///Volumes/Velvt' >/dev/null; then
    echo "ERROR: DMG layout contains a noncanonical file URL." >&2
    exit 1
  fi
  codesign --verify --deep --strict --verbose=2 "$mount_point/Velvt.app"
  cmp -s "$app_path/Contents/MacOS/Velvt" "$mount_point/Velvt.app/Contents/MacOS/Velvt" || {
    echo "ERROR: DMG app executable does not match the verified app." >&2
    exit 1
  }
  cmp -s "$helper" "$mount_point/Velvt.app/Contents/Resources/velvt-service" || {
    echo "ERROR: DMG helper does not match the verified app." >&2
    exit 1
  }
  if [[ "$mode" == "production" ]]; then
    xcrun stapler validate "$dmg_path"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"
  fi
  cleanup_mount
  trap - EXIT
fi

echo "Release verification passed"
echo "  artifact: $app_path"
echo "  configuration: $configuration"
echo "  API URL: $api_url"
echo "  protocol: $app_protocol"
echo "  helper: embedded"
echo "  preview/debug dylibs: absent"
echo "  codesign: valid"
echo "  hardened runtime: enabled"
echo "  required architectures: $required_archs"
echo "  verification mode: $mode"
if [[ -n "$dmg_path" ]]; then
  echo "  DMG: valid with branded volume icon, background, and Applications link"
fi
