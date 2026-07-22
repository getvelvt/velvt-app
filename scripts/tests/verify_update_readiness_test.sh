#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_dir="$(cd "$script_dir/../.." && pwd -P)"
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/velvt-update-readiness.XXXXXX")"
trap 'rm -rf "$fixture_dir"' EXIT

app="$fixture_dir/Velvt.app"
plist="$app/Contents/Info.plist"
appcast="$fixture_dir/appcast.xml"
mkdir -p "$app/Contents/Frameworks/Sparkle.framework"
mkdir -p "$app/Contents/Frameworks/Sparkle.framework/Resources"

cat > "$app/Contents/Frameworks/Sparkle.framework/Resources/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleShortVersionString</key><string>2.9.4</string>
</dict></plist>
PLIST

cat > "$plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleVersion</key><string>42</string>
  <key>VelvtUpdaterEnabled</key><true/>
  <key>SUFeedURL</key><string>https://updates.getvelvt.com/appcast.xml</string>
  <key>SUPublicEDKey</key><string>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=</string>
  <key>SURequireSignedFeed</key><true/>
  <key>SUVerifyUpdateBeforeExtraction</key><true/>
  <key>SUSignedFeedFailureExpirationInterval</key><integer>0</integer>
  <key>SUEnableSystemProfiling</key><false/>
</dict>
</plist>
PLIST

cat > "$appcast" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <item>
      <sparkle:version>42</sparkle:version>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <enclosure url="https://updates.getvelvt.com/Velvt-0.2.0.zip"
                 length="1024"
                 type="application/octet-stream"
                 sparkle:edSignature="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="/>
    </item>
  </channel>
XML
printf '</rss>' >> "$appcast"
signed_length="$(stat -f '%z' "$appcast")"
cat >> "$appcast" <<XML
<!-- sparkle-signatures:
edSignature: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==
length: $signed_length
-->
XML

"$repo_dir/scripts/verify_update_readiness.sh" \
  --app "$app" \
  --appcast "$appcast" \
  --previous-build 41 \
  --expected-update-version 42 >/dev/null

expect_failure() {
  local description="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "ERROR: $description was accepted" >&2
    exit 1
  fi
}

expect_failure "mismatched expected version" "$repo_dir/scripts/verify_update_readiness.sh" \
  --app "$app" \
  --appcast "$appcast" \
  --previous-build 41 \
  --expected-update-version 43

/usr/libexec/PlistBuddy -c "Set :VelvtUpdaterEnabled false" "$plist"
expect_failure "disabled production updater" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 41
/usr/libexec/PlistBuddy -c "Set :VelvtUpdaterEnabled true" "$plist"

expect_failure "equal previous build" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 42
expect_failure "lower candidate than previous" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 43
"$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 41.9 >/dev/null
expect_failure "component-equivalent previous build" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 42.0
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion 41" "$plist"
expect_failure "candidate does not match packaged app" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 40
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion 42" "$plist"

archive="$fixture_dir/Velvt-0.2.0.zip"
dd if=/dev/zero of="$archive" bs=1024 count=1 2>/dev/null
(
  cd "$fixture_dir"
  shasum -a 256 "$(basename "$archive")" > "$(basename "$archive").sha256"
)
"$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --archive "$archive" --previous-build 41 >/dev/null
printf 'tamper' >> "$archive"
expect_failure "tampered archive" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --archive "$archive" --previous-build 41

/usr/libexec/PlistBuddy -c "Set :SUFeedURL http://updates.getvelvt.com/appcast.xml" "$plist"
expect_failure "insecure feed URL" "$repo_dir/scripts/verify_update_readiness.sh" --app "$app" --appcast "$appcast" --previous-build 41

echo "verify_update_readiness tests passed"
