#!/usr/bin/env bash
# The socket peer check (docs/architecture/personal-vault-and-devices.md §9.3,
# PR A0): in a Release build the app talks only to a helper signed as
# com.velvt.service, and the helper serves only an app signed as com.velvt.mac,
# each by the same team as itself. Neither half can tell, at run time, that the
# build put the wrong name on the other, so the release scripts carry it:
# sign_release.sh names the helper, and verify_release.sh refuses a build whose
# halves would refuse each other, or whose helper was built for a Debug app and
# checks nothing.
#
# The fixture is a Velvt.app whose app and helper are small C programs, signed
# by the real sign_release.sh in local (ad-hoc) mode, so this needs clang, lipo
# and codesign, and no Rust. An ad-hoc build has no team, so each requirement
# is the identifier alone; the team-pinned form is covered by the Rust and
# Swift unit tests and by verify_release.sh --mode production.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
sign="$repo_root/scripts/sign_release.sh"
verify="$repo_root/scripts/verify_release.sh"

if [[ "$(uname -s)" != "Darwin" ]] || ! xcrun --find clang >/dev/null 2>&1; then
  echo "SKIP: socket peer release tests need macOS with clang"
  exit 0
fi

fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

app_protocol=35
app_version=1.0.14
app_commit=0123456789abcdef0123456789abcdef01234567
ad_hoc_app_requirement='identifier "com.velvt.mac"'

cat >"$fixture_dir/helper.c" <<'C'
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  if (strcmp(argv[1], "--protocol-version") == 0) { puts(PROTOCOL); return 0; }
  if (strcmp(argv[1], "--version") == 0) { puts(VERSION); return 0; }
  if (strcmp(argv[1], "--source-commit") == 0) { puts(COMMIT); return 0; }
  if (strcmp(argv[1], "--peer-requirement") == 0) { puts(PEER_REQUIREMENT); return 0; }
  return 2;
}
C
printf 'int main(void) { return 0; }\n' >"$fixture_dir/app.c"

# helper_answering <name> <peer requirement> -> path of a universal helper
helper_answering() {
  local output="$fixture_dir/helper-$1"
  local arch
  for arch in arm64 x86_64; do
    xcrun clang -arch "$arch" -mmacosx-version-min=11.0 \
      -DPROTOCOL="\"$app_protocol\"" -DVERSION="\"$app_version\"" -DCOMMIT="\"$app_commit\"" \
      -DPEER_REQUIREMENT="$(printf '"%s"' "${2//\"/\\\"}")" \
      -o "$output-$arch" "$fixture_dir/helper.c"
  done
  lipo -create "$output-arm64" "$output-x86_64" -output "$output"
  printf '%s' "$output"
}

for arch in arm64 x86_64; do
  xcrun clang -arch "$arch" -mmacosx-version-min=11.0 -o "$fixture_dir/app-$arch" "$fixture_dir/app.c"
done

# make_app <name> <helper> -> path of an unsigned Velvt.app
make_app() {
  local app="$fixture_dir/$1/Velvt.app"
  local contents="$app/Contents"
  mkdir -p "$contents/MacOS" "$contents/Resources/Fonts"
  lipo -create "$fixture_dir/app-arm64" "$fixture_dir/app-x86_64" -output "$contents/MacOS/Velvt"
  cp "$2" "$contents/Resources/velvt-service"
  echo '{}' >"$contents/Resources/abstraction-taxonomy-mvp-1.json"
  echo "$app_version" >"$contents/Resources/velvt-service.version"
  echo 'SIL Open Font License 1.1' >"$contents/Resources/Fonts/OFL.txt"
  cat >"$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Velvt</string>
  <key>CFBundleIdentifier</key><string>com.velvt.mac</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$app_version</string>
  <key>CFBundleVersion</key><string>20</string>
  <key>VelvtBuildConfiguration</key><string>Release</string>
  <key>VelvtDistributable</key><string>YES</string>
  <key>VelvtAPIBaseURL</key><string>https://api.example.com</string>
  <key>VelvtProtocolVersion</key><string>$app_protocol</string>
  <key>VelvtSourceCommit</key><string>$app_commit</string>
</dict>
</plist>
PLIST
  printf '%s' "$app"
}

sign_local() { # <app>
  bash "$sign" local "$1" >/dev/null 2>&1 || fail "sign_release.sh local failed on $1"
}

# Re-signs the helper as <identifier> and then the app around it, as a build
# that skipped sign_release.sh's naming would.
resign_helper_as() { # <app> <identifier>
  codesign --force --sign - --options runtime --identifier "$2" \
    "$1/Contents/Resources/velvt-service" >/dev/null 2>&1
  codesign --force --sign - --options runtime "$1" >/dev/null 2>&1
}

run_verify() { # <app>
  VELVT_RELEASE_ARCHS="arm64 x86_64" bash "$verify" --mode local --app "$1" 2>&1
}

identifier_of() {
  codesign -dv "$1" 2>&1 | sed -n 's/^Identifier=//p'
}

expect_refusal() { # <case> <app> <message fragment>
  local output
  if output="$(run_verify "$2")"; then
    fail "$1: verify_release.sh passed: $output"
  fi
  grep -qF "$3" <<<"$output" || fail "$1: refused, but not because of the peer check: $output"
}

# --- sign_release.sh names both halves, and the pair verifies -------------------

app="$(make_app signed "$(helper_answering ad-hoc "$ad_hoc_app_requirement")")"
sign_local "$app"
[[ "$(identifier_of "$app/Contents/Resources/velvt-service")" == "com.velvt.service" ]] ||
  fail "sign_release.sh signed the helper as '$(identifier_of "$app/Contents/Resources/velvt-service")', not com.velvt.service."
[[ "$(identifier_of "$app")" == "com.velvt.mac" ]] ||
  fail "sign_release.sh signed the app as '$(identifier_of "$app")', not com.velvt.mac."
output="$(run_verify "$app")" || fail "a correctly signed pair was refused: $output"
grep -qF "socket peer check: the helper requires $ad_hoc_app_requirement" <<<"$output" ||
  fail "the summary does not state what the helper requires: $output"
grep -qF 'socket peer check: the app requires identifier "com.velvt.service"' <<<"$output" ||
  fail "the summary does not state what the app requires: $output"

# --- A helper under codesign's default name: the app would refuse it ------------

app="$(make_app default-name "$(helper_answering default-name "$ad_hoc_app_requirement")")"
sign_local "$app"
resign_helper_as "$app" velvt-service
expect_refusal default-name "$app" "the helper is signed as 'velvt-service'"

# --- An app under another name: the helper would refuse it ----------------------

app="$(make_app other-app "$(helper_answering other-app "$ad_hoc_app_requirement")")"
sign_local "$app"
codesign --force --sign - --options runtime --identifier com.velvt.other "$app" >/dev/null 2>&1
expect_refusal other-app "$app" "the app is signed as 'com.velvt.other'"

# --- A helper built for a Debug app checks nothing ------------------------------

app="$(make_app debug-helper "$(helper_answering debug-helper none)")"
sign_local "$app"
expect_refusal debug-helper "$app" "does not check the app's code signature on the socket"

# --- A helper holding the app to something its signature does not call for -----

app="$(make_app wrong-requirement "$(helper_answering wrong-requirement 'identifier "com.velvt.other"')")"
sign_local "$app"
expect_refusal wrong-requirement "$app" "the helper holds the app to 'identifier \"com.velvt.other\"'"

echo "socket peer release tests passed"
