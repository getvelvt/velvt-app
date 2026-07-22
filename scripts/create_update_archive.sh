#!/usr/bin/env bash
set -euo pipefail

app_path=""
archive_path=""
expected_build=""
expected_version=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app_path="${2:-}"; shift 2 ;;
    --archive) archive_path="${2:-}"; shift 2 ;;
    --expected-build) expected_build="${2:-}"; shift 2 ;;
    --expected-version) expected_version="${2:-}"; shift 2 ;;
    *)
      echo "Usage: $0 --app Velvt.app --archive Velvt-VERSION-BUILD.zip --expected-build BUILD --expected-version VERSION" >&2
      exit 2
      ;;
  esac
done

[[ -d "$app_path" ]] || { echo "ERROR: app bundle not found: $app_path" >&2; exit 1; }
[[ "$archive_path" == *.zip ]] || { echo "ERROR: update archive must end in .zip." >&2; exit 1; }
[[ "$expected_build" =~ ^[0-9]+([.][0-9]+)*$ ]] || { echo "ERROR: --expected-build must be numeric." >&2; exit 1; }
[[ "$expected_version" =~ ^[0-9]+([.][0-9]+)*$ ]] || { echo "ERROR: --expected-version must be numeric." >&2; exit 1; }
[[ ! -e "$archive_path" && ! -e "$archive_path.sha256" ]] || {
  echo "ERROR: immutable update output already exists: $archive_path" >&2
  exit 1
}

plist="$app_path/Contents/Info.plist"
[[ -f "$plist" ]] || { echo "ERROR: Info.plist not found: $plist" >&2; exit 1; }
actual_build="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist")"
actual_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
[[ "$actual_build" == "$expected_build" ]] || {
  echo "ERROR: app build '$actual_build' does not match expected '$expected_build'." >&2
  exit 1
}
[[ "$actual_version" == "$expected_version" ]] || {
  echo "ERROR: app version '$actual_version' does not match expected '$expected_version'." >&2
  exit 1
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
"$script_dir/verify_release.sh" --mode production --app "$app_path"
xcrun stapler validate "$app_path"

archive_dir="$(dirname "$archive_path")"
mkdir -p "$archive_dir"
archive_dir="$(cd "$archive_dir" && pwd -P)"
archive_path="$archive_dir/$(basename "$archive_path")"
temporary_archive="$(mktemp "$archive_dir/.Velvt-update.XXXXXX.zip")"
temporary_checksum="$(mktemp "$archive_dir/.Velvt-update-checksum.XXXXXX")"
cleanup() { rm -f "$temporary_archive" "$temporary_checksum"; }
trap cleanup EXIT

ditto -c -k --sequesterRsrc --keepParent "$app_path" "$temporary_archive"
unzip -tq "$temporary_archive" >/dev/null
archive_digest="$(shasum -a 256 "$temporary_archive" | awk '{print $1}')"
printf '%s  %s\n' "$archive_digest" "$(basename "$archive_path")" > "$temporary_checksum"
ln "$temporary_archive" "$archive_path"
ln "$temporary_checksum" "$archive_path.sha256"
rm -f "$temporary_archive" "$temporary_checksum"
trap - EXIT

echo "Created immutable Sparkle full-update archive"
echo "  archive: $archive_path"
echo "  checksum: $archive_path.sha256"
echo "  version: $actual_version ($actual_build)"
