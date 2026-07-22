#!/usr/bin/env bash
set -euo pipefail

mode="${1:-}"
app_path="${2:-dist/Velvt.app}"
entitlements="${VELVT_RELEASE_ENTITLEMENTS:-swift-client/Configs/Release.entitlements}"

case "$mode" in
  local)
    identity="-"
    timestamp_args=(--timestamp=none)
    ;;
  production)
    identity="${VELVT_CODESIGN_IDENTITY:-}"
    if [[ -z "$identity" || "$identity" == "-" ]]; then
      echo "ERROR: production signing requires VELVT_CODESIGN_IDENTITY='Developer ID Application: …'." >&2
      exit 1
    fi
    if ! security find-identity -v -p codesigning | grep -Fq "$identity"; then
      echo "ERROR: configured signing identity is not available in the keychain: $identity" >&2
      exit 1
    fi
    timestamp_args=(--timestamp)
    ;;
  *)
    echo "Usage: $0 local|production [path/to/Velvt.app]" >&2
    exit 2
    ;;
esac

[[ -d "$app_path" ]] || { echo "ERROR: app not found: $app_path" >&2; exit 1; }
[[ -f "$entitlements" ]] || { echo "ERROR: entitlements not found: $entitlements" >&2; exit 1; }

helper="$app_path/Contents/Resources/velvt-service"
[[ -x "$helper" ]] || { echo "ERROR: embedded helper not found: $helper" >&2; exit 1; }

# Sign Sparkle's nested code inside-out. Do not use --deep for signing: it can
# conceal an omitted nested component and produce a bundle Sparkle cannot use.
sparkle_framework="$app_path/Contents/Frameworks/Sparkle.framework"
if [[ -d "$sparkle_framework" ]]; then
  sign_sparkle_component() {
    local component="$1"
    if [[ "$(basename "$component")" == "Downloader.xpc" ]]; then
      local metadata_dir before_entitlements after_entitlements
      metadata_dir="$(mktemp -d "${TMPDIR:-/tmp}/velvt-sparkle-sign.XXXXXX")"
      before_entitlements="$metadata_dir/entitlements.before"
      after_entitlements="$metadata_dir/entitlements.after"
      codesign -d --entitlements :- "$component" >"$before_entitlements" 2>/dev/null
      [[ -s "$before_entitlements" ]] || {
        echo "ERROR: Sparkle Downloader.xpc has no entitlements to preserve." >&2
        rm -rf "$metadata_dir"
        exit 1
      }
      codesign --force --sign "$identity" --options runtime "${timestamp_args[@]}" \
        --preserve-metadata=entitlements "$component"
      codesign -d --entitlements :- "$component" >"$after_entitlements" 2>/dev/null
      cmp -s "$before_entitlements" "$after_entitlements" || {
        echo "ERROR: re-signing changed Sparkle Downloader.xpc entitlements." >&2
        rm -rf "$metadata_dir"
        exit 1
      }
      rm -rf "$metadata_dir"
    else
      codesign --force --sign "$identity" --options runtime "${timestamp_args[@]}" "$component"
    fi
  }
  while IFS= read -r nested_service; do
    sign_sparkle_component "$nested_service"
  done < <(find "$sparkle_framework" -type d -name '*.xpc' -print | sort -r)
  while IFS= read -r nested_app; do
    sign_sparkle_component "$nested_app"
  done < <(find "$sparkle_framework" -type d -name '*.app' -print | sort -r)
  while IFS= read -r autoupdate; do
    sign_sparkle_component "$autoupdate"
  done < <(find "$sparkle_framework" -type f -name Autoupdate -perm -111 -print)
  sign_sparkle_component "$sparkle_framework"
fi

codesign --force --sign "$identity" --options runtime "${timestamp_args[@]}" "$helper"
codesign --force --sign "$identity" --options runtime "${timestamp_args[@]}" \
  --entitlements "$entitlements" "$app_path"
codesign --verify --deep --strict --verbose=2 "$app_path"

echo "Signed $app_path ($mode, hardened runtime enabled)"
