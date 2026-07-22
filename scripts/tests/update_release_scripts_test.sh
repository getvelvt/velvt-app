#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_dir="$(cd "$script_dir/../.." && pwd -P)"
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/velvt-update-release.XXXXXX")"
trap 'rm -rf "$fixture_dir"' EXIT

grep -Fq -- '--preserve-metadata=entitlements' "$repo_dir/scripts/sign_release.sh" || {
  echo "ERROR: Sparkle Downloader signing does not preserve entitlements" >&2
  exit 1
}
if grep -Fq -- '--preserve-metadata=identifier,entitlements,requirements' "$repo_dir/scripts/sign_release.sh"; then
  echo "ERROR: Sparkle signing preserves the upstream designated requirement" >&2
  exit 1
fi

archive="$fixture_dir/Velvt-0.2.0-42.zip"
printf 'immutable update fixture' > "$archive"
(
  cd "$fixture_dir"
  shasum -a 256 "$(basename "$archive")" > "$(basename "$archive").sha256"
)
key="$fixture_dir/private-key"
printf 'fixture-key-never-used-cryptographically' > "$key"
chmod 600 "$key"
bin_dir="$fixture_dir/sparkle-bin"
mkdir -p "$bin_dir"

cat > "$bin_dir/generate_appcast" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
output=""
prefix=""
directory=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ed-key-file) shift 2 ;;
    --download-url-prefix) prefix="$2"; shift 2 ;;
    -o) output="$2"; shift 2 ;;
    --output) echo "unsupported option: --output" >&2; exit 64 ;;
    *) directory="$1"; shift ;;
  esac
done
archive="$(find "$directory" -maxdepth 1 -name '*.zip' -print -quit)"
length="$(stat -f '%z' "$archive")"
name="$(basename "$archive")"
cat > "$output" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>
<sparkle:version>42</sparkle:version>
<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
<enclosure url="${prefix}${name}" length="$length" sparkle:edSignature="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="/>
</item></channel>
XML
printf '</rss>' >> "$output"
signed_length="$(stat -f '%z' "$output")"
cat >> "$output" <<XML
<!-- sparkle-signatures:
edSignature: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==
length: $signed_length
-->
XML
FAKE
chmod +x "$bin_dir/generate_appcast"
if grep -Fq -- '--output' "$repo_dir/scripts/generate_update_appcast.sh"; then
  echo "ERROR: generate_update_appcast uses unsupported Sparkle --output option" >&2
  exit 1
fi
cat > "$bin_dir/sign_update" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "--verify" && "${2:-}" == "--ed-key-file" && -f "${3:-}" && -f "${4:-}" ]] || exit 64
grep -q '<!-- sparkle-signatures:' "$4"
FAKE
chmod +x "$bin_dir/sign_update"
generate_appcast_sha256="$(shasum -a 256 "$bin_dir/generate_appcast" | awk '{print $1}')"
sign_update_sha256="$(shasum -a 256 "$bin_dir/sign_update" | awk '{print $1}')"

grep -Fq 'VELVT_BUILD_MARKETING_VERSION="$(VELVT_RELEASE_VERSION)"' "$repo_dir/Makefile" || {
  echo "ERROR: release does not propagate MARKETING_VERSION" >&2
  exit 1
}
grep -Fq 'VELVT_BUILD_NUMBER="$(VELVT_RELEASE_BUILD)"' "$repo_dir/Makefile" || {
  echo "ERROR: release does not propagate CURRENT_PROJECT_VERSION" >&2
  exit 1
}

appcast="$fixture_dir/appcast.xml"
"$repo_dir/scripts/generate_update_appcast.sh" \
  --archive "$archive" \
  --appcast "$appcast" \
  --download-url-prefix https://updates.getvelvt.com/ \
  --private-key-file "$key" \
  --sparkle-bin-dir "$bin_dir" \
  --expected-build 42 \
  --expected-generate-appcast-sha256 "$generate_appcast_sha256" \
  --expected-sign-update-sha256 "$sign_update_sha256" >/dev/null
xmllint --noout "$appcast"

expect_failure() {
  local description="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "ERROR: $description was accepted" >&2
    exit 1
  fi
}

expect_failure "appcast overwrite" "$repo_dir/scripts/generate_update_appcast.sh" \
  --archive "$archive" --appcast "$appcast" --download-url-prefix https://updates.getvelvt.com/ \
  --private-key-file "$key" --sparkle-bin-dir "$bin_dir" --expected-build 42 \
  --expected-generate-appcast-sha256 "$generate_appcast_sha256" --expected-sign-update-sha256 "$sign_update_sha256"
expect_failure "HTTP download URL" "$repo_dir/scripts/generate_update_appcast.sh" \
  --archive "$archive" --appcast "$fixture_dir/http.xml" --download-url-prefix http://updates.getvelvt.com/ \
  --private-key-file "$key" --sparkle-bin-dir "$bin_dir" --expected-build 42 \
  --expected-generate-appcast-sha256 "$generate_appcast_sha256" --expected-sign-update-sha256 "$sign_update_sha256"
chmod 644 "$key"
expect_failure "world-readable private key" "$repo_dir/scripts/generate_update_appcast.sh" \
  --archive "$archive" --appcast "$fixture_dir/perms.xml" --download-url-prefix https://updates.getvelvt.com/ \
  --private-key-file "$key" --sparkle-bin-dir "$bin_dir" --expected-build 42 \
  --expected-generate-appcast-sha256 "$generate_appcast_sha256" --expected-sign-update-sha256 "$sign_update_sha256"

repo_key="$repo_dir/.fixture-private-key"
trap 'rm -f "$repo_key"; rm -rf "$fixture_dir"' EXIT
printf 'fixture' > "$repo_key"
chmod 600 "$repo_key"
expect_failure "repository-resident private key" "$repo_dir/scripts/generate_update_appcast.sh" \
  --archive "$archive" --appcast "$fixture_dir/repo-key.xml" --download-url-prefix https://updates.getvelvt.com/ \
  --private-key-file "$repo_key" --sparkle-bin-dir "$bin_dir" --expected-build 42 \
  --expected-generate-appcast-sha256 "$generate_appcast_sha256" --expected-sign-update-sha256 "$sign_update_sha256"
rm -f "$repo_key"

fake_app="$fixture_dir/Fake.app"
mkdir -p "$fake_app/Contents"
cat > "$fake_app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleVersion</key><string>42</string>
<key>CFBundleShortVersionString</key><string>0.2.0</string>
</dict></plist>
PLIST
expect_failure "archive version mismatch" "$repo_dir/scripts/create_update_archive.sh" \
  --app "$fake_app" --archive "$fixture_dir/mismatch.zip" --expected-build 43 --expected-version 0.2.0

echo "update release script tests passed"
"$repo_dir/scripts/tests/update_local_adversarial_harness_test.sh"
