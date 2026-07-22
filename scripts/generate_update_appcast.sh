#!/usr/bin/env bash
set -euo pipefail

archive_path=""
appcast_path=""
download_url_prefix=""
private_key_file=""
sparkle_bin_dir=""
expected_build=""
expected_generate_appcast_sha256=""
expected_sign_update_sha256=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive) archive_path="${2:-}"; shift 2 ;;
    --appcast) appcast_path="${2:-}"; shift 2 ;;
    --download-url-prefix) download_url_prefix="${2:-}"; shift 2 ;;
    --private-key-file) private_key_file="${2:-}"; shift 2 ;;
    --sparkle-bin-dir) sparkle_bin_dir="${2:-}"; shift 2 ;;
    --expected-build) expected_build="${2:-}"; shift 2 ;;
    --expected-generate-appcast-sha256) expected_generate_appcast_sha256="${2:-}"; shift 2 ;;
    --expected-sign-update-sha256) expected_sign_update_sha256="${2:-}"; shift 2 ;;
    *)
      echo "Usage: $0 --archive FILE.zip --appcast appcast.xml --download-url-prefix https://host/ --private-key-file FILE --sparkle-bin-dir DIR --expected-build BUILD" >&2
      exit 2
      ;;
  esac
done

[[ -f "$archive_path" ]] || { echo "ERROR: archive not found: $archive_path" >&2; exit 1; }
[[ -f "$archive_path.sha256" ]] || { echo "ERROR: archive checksum not found: $archive_path.sha256" >&2; exit 1; }
[[ "$appcast_path" == *.xml ]] || { echo "ERROR: appcast output must end in .xml." >&2; exit 1; }
[[ ! -e "$appcast_path" ]] || { echo "ERROR: immutable appcast output already exists: $appcast_path" >&2; exit 1; }
[[ "$download_url_prefix" == https://* ]] || { echo "ERROR: download URL prefix must use HTTPS." >&2; exit 1; }
[[ "$expected_build" =~ ^[0-9]+([.][0-9]+)*$ ]] || { echo "ERROR: --expected-build must be numeric." >&2; exit 1; }
[[ "$expected_generate_appcast_sha256" =~ ^[a-fA-F0-9]{64}$ ]] || { echo "ERROR: expected generate_appcast SHA-256 is required." >&2; exit 1; }
[[ "$expected_sign_update_sha256" =~ ^[a-fA-F0-9]{64}$ ]] || { echo "ERROR: expected sign_update SHA-256 is required." >&2; exit 1; }
[[ -f "$private_key_file" && ! -L "$private_key_file" ]] || {
  echo "ERROR: Sparkle private key must be a regular, non-symlink file." >&2
  exit 1
}
[[ -x "$sparkle_bin_dir/generate_appcast" ]] || {
  echo "ERROR: Sparkle generate_appcast not found at $sparkle_bin_dir/generate_appcast" >&2
  exit 1
}
[[ -x "$sparkle_bin_dir/sign_update" ]] || {
  echo "ERROR: Sparkle sign_update not found at $sparkle_bin_dir/sign_update" >&2
  exit 1
}
actual_generate_appcast_sha256="$(shasum -a 256 "$sparkle_bin_dir/generate_appcast" | awk '{print $1}')"
actual_sign_update_sha256="$(shasum -a 256 "$sparkle_bin_dir/sign_update" | awk '{print $1}')"
[[ "$actual_generate_appcast_sha256" == "$expected_generate_appcast_sha256" ]] || {
  echo "ERROR: generate_appcast does not match the approved SHA-256." >&2
  exit 1
}
[[ "$actual_sign_update_sha256" == "$expected_sign_update_sha256" ]] || {
  echo "ERROR: sign_update does not match the approved SHA-256." >&2
  exit 1
}

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
key_dir="$(cd "$(dirname "$private_key_file")" && pwd -P)"
key_path="$key_dir/$(basename "$private_key_file")"
case "$key_path" in
  "$repo_dir"|"$repo_dir"/*)
    echo "ERROR: Sparkle private key must live outside the repository." >&2
    exit 1
    ;;
esac
key_mode="$(stat -f '%Lp' "$key_path")"
[[ "$key_mode" == "400" || "$key_mode" == "600" ]] || {
  echo "ERROR: Sparkle private key permissions must be 0400 or 0600; got $key_mode." >&2
  exit 1
}

archive_dir="$(cd "$(dirname "$archive_path")" && pwd -P)"
(
  cd "$archive_dir"
  shasum -a 256 -c "$(basename "$archive_path").sha256"
)

output_dir="$(dirname "$appcast_path")"
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd -P)"
appcast_path="$output_dir/$(basename "$appcast_path")"
staging="$(mktemp -d "$output_dir/.velvt-appcast.XXXXXX")"
cleanup() { rm -rf "$staging"; }
trap cleanup EXIT
cp "$archive_path" "$staging/"

"$sparkle_bin_dir/generate_appcast" \
  --ed-key-file "$key_path" \
  --download-url-prefix "${download_url_prefix%/}/" \
  -o "$staging/appcast.xml" \
  "$staging"

xmllint --noout "$staging/appcast.xml"
enclosure="(//*[local-name()='item'][1]/*[local-name()='enclosure'])[1]"
generated_build="$(xmllint --xpath "string((//*[local-name()='item'][1]/*[local-name()='version'])[1])" "$staging/appcast.xml")"
generated_url="$(xmllint --xpath "string($enclosure/@url)" "$staging/appcast.xml")"
generated_length="$(xmllint --xpath "string($enclosure/@length)" "$staging/appcast.xml")"
actual_length="$(stat -f '%z' "$archive_path")"
[[ "$generated_build" == "$expected_build" ]] || {
  echo "ERROR: generated appcast build '$generated_build' does not match '$expected_build'." >&2
  exit 1
}
[[ "${generated_url##*/}" == "$(basename "$archive_path")" ]] || {
  echo "ERROR: generated appcast URL does not reference the immutable archive." >&2
  exit 1
}
[[ "$generated_length" == "$actual_length" ]] || {
  echo "ERROR: generated appcast length '$generated_length' does not match archive '$actual_length'." >&2
  exit 1
}
signature_marker_count="$(LC_ALL=C grep -ao '<!-- sparkle-signatures:' "$staging/appcast.xml" | wc -l | tr -d ' ')"
signature_block_start="$(tail -n 4 "$staging/appcast.xml" | head -n 1)"
[[ "$signature_marker_count" == "1" && "$signature_block_start" == *'<!-- sparkle-signatures:' && "$(tail -n 1 "$staging/appcast.xml")" == "-->" ]] || {
  echo "ERROR: generate_appcast did not emit exactly one trailing signed-feed block." >&2
  exit 1
}
feed_signature="$(sed -n 's/^edSignature: //p' "$staging/appcast.xml" | tail -n 1)"
feed_signed_length="$(sed -n 's/^length: //p' "$staging/appcast.xml" | tail -n 1)"
signed_content_length="$(LC_ALL=C grep -abo '<!-- sparkle-signatures:' "$staging/appcast.xml" | cut -d: -f1)"
[[ "$feed_signed_length" =~ ^[1-9][0-9]*$ && "$feed_signed_length" == "$signed_content_length" ]] || {
  echo "ERROR: signed-feed length does not match the bytes before the signature block." >&2
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
"$sparkle_bin_dir/sign_update" --verify --ed-key-file "$key_path" "$staging/appcast.xml"

ln "$staging/appcast.xml" "$appcast_path"
echo "Generated signed Sparkle appcast"
echo "  appcast: $appcast_path"
echo "  archive: $archive_path"
echo "  build: $generated_build"
echo "  embedded signed-feed signature: present"
echo "  signed-feed cryptographic verification: passed"
echo "Publish the immutable archive first; publish this appcast atomically only after the archive is reachable."
