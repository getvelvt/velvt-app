#!/usr/bin/env bash
set -euo pipefail

app_path=""
appcast_path=""
expected_update_version=""
archive_path=""
previous_build=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app_path="${2:-}"; shift 2 ;;
    --appcast) appcast_path="${2:-}"; shift 2 ;;
    --expected-update-version) expected_update_version="${2:-}"; shift 2 ;;
    --archive) archive_path="${2:-}"; shift 2 ;;
    --previous-build) previous_build="${2:-}"; shift 2 ;;
    *)
      echo "Usage: $0 --app Velvt.app --appcast appcast.xml --previous-build BUILD [--archive update.zip] [--expected-update-version BUILD]" >&2
      exit 2
      ;;
  esac
done

[[ -d "$app_path" ]] || { echo "ERROR: app bundle not found: $app_path" >&2; exit 1; }
[[ -f "$appcast_path" ]] || { echo "ERROR: appcast not found: $appcast_path" >&2; exit 1; }

plist="$app_path/Contents/Info.plist"
framework="$app_path/Contents/Frameworks/Sparkle.framework"
[[ -f "$plist" ]] || { echo "ERROR: Info.plist not found: $plist" >&2; exit 1; }
[[ -d "$framework" ]] || {
  echo "ERROR: Sparkle.framework is not embedded; Velvt has no active app updater." >&2
  exit 1
}
sparkle_plist="$framework/Resources/Info.plist"
[[ -f "$sparkle_plist" ]] || { echo "ERROR: embedded Sparkle Info.plist not found." >&2; exit 1; }
sparkle_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$sparkle_plist" 2>/dev/null || true)"
[[ "$sparkle_version" == "2.9.4" ]] || {
  echo "ERROR: packaged Sparkle version must be exactly 2.9.4; got '$sparkle_version'." >&2
  exit 1
}

read_plist() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null
}

feed_url="$(read_plist SUFeedURL || true)"
public_key="$(read_plist SUPublicEDKey || true)"
updater_enabled="$(read_plist VelvtUpdaterEnabled || true)"
require_signed_feed="$(read_plist SURequireSignedFeed || true)"
verify_before_extraction="$(read_plist SUVerifyUpdateBeforeExtraction || true)"
signed_feed_failure_expiration="$(read_plist SUSignedFeedFailureExpirationInterval || true)"
system_profiling="$(read_plist SUEnableSystemProfiling || true)"
bundle_version="$(read_plist CFBundleVersion || true)"

[[ "$updater_enabled" == "true" ]] || {
  echo "ERROR: VelvtUpdaterEnabled must be true for production releases." >&2
  exit 1
}
[[ "$feed_url" == https://* ]] || {
  echo "ERROR: SUFeedURL must be a non-empty HTTPS URL." >&2
  exit 1
}
case "${feed_url#https://}" in
  *'@'*|*'?'*|*'#'*)
    echo "ERROR: SUFeedURL must not contain credentials, query parameters, or fragments." >&2
    exit 1
    ;;
esac
[[ "$public_key" =~ ^[A-Za-z0-9+/]{43}=$ ]] || {
  echo "ERROR: SUPublicEDKey must be a base64-encoded 32-byte Ed25519 public key." >&2
  exit 1
}
decoded_key_bytes="$(printf '%s' "$public_key" | openssl base64 -d -A 2>/dev/null | wc -c | tr -d ' ')"
[[ "$decoded_key_bytes" == "32" ]] || {
  echo "ERROR: SUPublicEDKey does not decode to 32 bytes." >&2
  exit 1
}
[[ "$require_signed_feed" == "true" ]] || {
  echo "ERROR: SURequireSignedFeed must be true for production releases." >&2
  exit 1
}
[[ "$verify_before_extraction" == "true" ]] || {
  echo "ERROR: SUVerifyUpdateBeforeExtraction must be true for signed feeds." >&2
  exit 1
}
[[ "$signed_feed_failure_expiration" == "0" ]] || {
  echo "ERROR: SUSignedFeedFailureExpirationInterval must be 0; signature failures must never expire." >&2
  exit 1
}
[[ "$system_profiling" == "false" ]] || {
  echo "ERROR: SUEnableSystemProfiling must be false for the privacy-preserving updater." >&2
  exit 1
}
[[ "$bundle_version" =~ ^[0-9]+([.][0-9]+)*$ ]] || {
  echo "ERROR: CFBundleVersion must be an increasing numeric version; got '$bundle_version'." >&2
  exit 1
}
[[ "$previous_build" =~ ^[0-9]+([.][0-9]+)*$ ]] || {
  echo "ERROR: --previous-build is required and must be the numeric installed N build." >&2
  exit 1
}

xmllint --noout "$appcast_path"

xpath_string() {
  xmllint --xpath "string($1)" "$appcast_path"
}

enclosure="(//*[local-name()='item'][1]/*[local-name()='enclosure'])[1]"
update_url="$(xpath_string "$enclosure/@url")"
update_version="$(xpath_string "(//*[local-name()='item'][1]/*[local-name()='version'])[1]")"
update_signature="$(xpath_string "$enclosure/@*[local-name()='edSignature']")"
update_length="$(xpath_string "$enclosure/@length")"
minimum_system_version="$(xpath_string "(//*[local-name()='item'][1]/*[local-name()='minimumSystemVersion'])[1]")"

[[ "$update_url" == https://* ]] || {
  echo "ERROR: latest appcast enclosure URL must use HTTPS." >&2
  exit 1
}
case "${update_url#https://}" in
  *'@'*|*'?'*|*'#'*)
    echo "ERROR: update archive URL must not contain credentials, query parameters, or fragments." >&2
    exit 1
    ;;
esac
feed_host="${feed_url#https://}"
feed_host="${feed_host%%/*}"
update_host="${update_url#https://}"
update_host="${update_host%%/*}"
[[ "$update_host" == "$feed_host" ]] || {
  echo "ERROR: update archive host '$update_host' does not match approved feed host '$feed_host'." >&2
  exit 1
}
[[ "$update_version" =~ ^[0-9]+([.][0-9]+)*$ ]] || {
  echo "ERROR: latest appcast item needs a numeric sparkle:version." >&2
  exit 1
}

version_greater_than() {
  local candidate="$1"
  local installed="$2"
  local old_ifs="$IFS"
  local index length
  local -a candidate_parts installed_parts
  IFS='.' read -r -a candidate_parts <<<"$candidate"
  IFS='.' read -r -a installed_parts <<<"$installed"
  IFS="$old_ifs"
  length="${#candidate_parts[@]}"
  if (( ${#installed_parts[@]} > length )); then length="${#installed_parts[@]}"; fi
  for ((index = 0; index < length; index++)); do
    local candidate_part="${candidate_parts[index]:-0}"
    local installed_part="${installed_parts[index]:-0}"
    if ((10#$candidate_part > 10#$installed_part)); then return 0; fi
    if ((10#$candidate_part < 10#$installed_part)); then return 1; fi
  done
  return 1
}

[[ "$update_version" == "$bundle_version" ]] || {
  echo "ERROR: appcast candidate '$update_version' must equal packaged app build '$bundle_version'." >&2
  exit 1
}
version_greater_than "$update_version" "$previous_build" || {
  echo "ERROR: candidate build '$update_version' must be newer than previous build '$previous_build'." >&2
  exit 1
}
[[ "$update_signature" =~ ^[A-Za-z0-9+/]{86}==$ ]] || {
  echo "ERROR: latest appcast enclosure needs a base64 Ed25519 sparkle:edSignature." >&2
  exit 1
}
decoded_signature_bytes="$(printf '%s' "$update_signature" | openssl base64 -d -A 2>/dev/null | wc -c | tr -d ' ')"
[[ "$decoded_signature_bytes" == "64" ]] || {
  echo "ERROR: sparkle:edSignature does not decode to 64 bytes." >&2
  exit 1
}
signature_marker_count="$(LC_ALL=C grep -ao '<!-- sparkle-signatures:' "$appcast_path" | wc -l | tr -d ' ')"
signature_block_start="$(tail -n 4 "$appcast_path" | head -n 1)"
[[ "$signature_marker_count" == "1" && "$signature_block_start" == *'<!-- sparkle-signatures:' && "$(tail -n 1 "$appcast_path")" == "-->" ]] || {
  echo "ERROR: appcast needs exactly one trailing Sparkle signed-feed block." >&2
  exit 1
}
feed_signature="$(sed -n 's/^edSignature: //p' "$appcast_path" | tail -n 1)"
feed_signed_length="$(sed -n 's/^length: //p' "$appcast_path" | tail -n 1)"
signed_content_length="$(LC_ALL=C grep -abo '<!-- sparkle-signatures:' "$appcast_path" | cut -d: -f1)"
[[ "$feed_signed_length" =~ ^[1-9][0-9]*$ && "$feed_signed_length" == "$signed_content_length" ]] || {
  echo "ERROR: signed-feed length does not match the bytes before its signature block." >&2
  exit 1
}
[[ "$feed_signature" =~ ^[A-Za-z0-9+/]{86}==$ ]] || {
  echo "ERROR: signed-feed block does not contain a base64 Ed25519 signature." >&2
  exit 1
}
decoded_feed_signature_bytes="$(printf '%s' "$feed_signature" | openssl base64 -d -A 2>/dev/null | wc -c | tr -d ' ')"
[[ "$decoded_feed_signature_bytes" == "64" ]] || {
  echo "ERROR: signed-feed signature does not decode to 64 bytes." >&2
  exit 1
}
[[ "$update_length" =~ ^[1-9][0-9]*$ ]] || {
  echo "ERROR: latest appcast enclosure needs a positive byte length." >&2
  exit 1
}
[[ -n "$minimum_system_version" ]] || {
  echo "ERROR: latest appcast item must declare sparkle:minimumSystemVersion." >&2
  exit 1
}
if [[ -n "$expected_update_version" && "$update_version" != "$expected_update_version" ]]; then
  echo "ERROR: appcast version '$update_version' does not match expected '$expected_update_version'." >&2
  exit 1
fi
if [[ -n "$archive_path" ]]; then
  [[ -f "$archive_path" ]] || { echo "ERROR: update archive not found: $archive_path" >&2; exit 1; }
  [[ -f "$archive_path.sha256" ]] || { echo "ERROR: update archive checksum not found: $archive_path.sha256" >&2; exit 1; }
  archive_dir="$(cd "$(dirname "$archive_path")" && pwd -P)"
  (
    cd "$archive_dir"
    shasum -a 256 -c "$(basename "$archive_path").sha256" >/dev/null
  )
  [[ "${update_url##*/}" == "$(basename "$archive_path")" ]] || {
    echo "ERROR: appcast URL does not reference the verified local archive." >&2
    exit 1
  }
  archive_length="$(stat -f '%z' "$archive_path")"
  [[ "$update_length" == "$archive_length" ]] || {
    echo "ERROR: appcast length '$update_length' does not match archive length '$archive_length'." >&2
    exit 1
  }
fi

echo "Update readiness verification passed"
echo "  app: $app_path"
echo "  Sparkle: $sparkle_version"
echo "  packaged candidate build: $bundle_version"
echo "  feed: $feed_url"
echo "  candidate build: $update_version"
echo "  archive URL: $update_url"
echo "  archive Ed25519 signature: present"
echo "  signed feed enforcement: enabled"
echo "  embedded signed-feed signature: present"
echo "  pre-extraction verification: enabled"
echo "  minimum macOS: $minimum_system_version"
if [[ -n "$archive_path" ]]; then
  echo "  immutable archive checksum: verified"
fi
