#!/usr/bin/env bash
# verify_release.sh asks the embedded helper for its protocol, version and
# source commit and compares them with the app. The defect this pins: it asked
# only the slice the build machine runs natively, so on Apple Silicon a universal
# helper whose x86_64 slice was from another build passed, and every Intel tester
# would have hit a version-mismatch alert on each launch.
#
# The fixture is an ad-hoc signed Velvt.app whose helper is lipo'd from two
# small C programs, so this needs clang, lipo and codesign, and no Rust.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
verify="$repo_root/scripts/verify_release.sh"

if [[ "$(uname -s)" != "Darwin" ]] || ! xcrun --find clang >/dev/null 2>&1; then
  echo "SKIP: verify_release slice tests need macOS with clang"
  exit 0
fi

fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

app_protocol=33
app_version=1.0.14
app_commit=0123456789abcdef0123456789abcdef01234567
other_commit=fedcba9876543210fedcba9876543210fedcba98

cat >"$fixture_dir/helper.c" <<'C'
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  if (strcmp(argv[1], "--protocol-version") == 0) { puts(PROTOCOL); return 0; }
  if (strcmp(argv[1], "--version") == 0) { puts(VERSION); return 0; }
  if (strcmp(argv[1], "--source-commit") == 0) { puts(COMMIT); return 0; }
  return 2;
}
C
printf 'int main(void) { return 0; }\n' >"$fixture_dir/app.c"

compile() { # <arch> <output> <source> [defines...]
  local arch="$1" output="$2" source="$3"
  shift 3
  xcrun clang -arch "$arch" -mmacosx-version-min=11.0 "$@" -o "$output" "$source"
}

slice() { # <arch> <protocol> <version> <commit> -> path of the compiled slice
  local output="$fixture_dir/helper-$1-$2-$3-$4"
  [[ -x "$output" ]] ||
    compile "$1" "$output" "$fixture_dir/helper.c" \
      -DPROTOCOL="\"$2\"" -DVERSION="\"$3\"" -DCOMMIT="\"$4\""
  printf '%s' "$output"
}

compile arm64 "$fixture_dir/app-arm64" "$fixture_dir/app.c"
compile x86_64 "$fixture_dir/app-x86_64" "$fixture_dir/app.c"

# make_app <name> <arm64 slice> <x86_64 slice> -> path of a signed Velvt.app
make_app() {
  local app="$fixture_dir/$1/Velvt.app"
  local contents="$app/Contents"
  mkdir -p "$contents/MacOS" "$contents/Resources/Fonts"
  lipo -create "$fixture_dir/app-arm64" "$fixture_dir/app-x86_64" -output "$contents/MacOS/Velvt"
  lipo -create "$2" "$3" -output "$contents/Resources/velvt-service"
  echo '{}' >"$contents/Resources/abstraction-taxonomy-mvp-1.json"
  echo "$app_version" >"$contents/Resources/velvt-service.version"
  echo 'SIL Open Font License 1.1' >"$contents/Resources/Fonts/OFL.txt"
  cat >"$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Velvt</string>
  <key>CFBundleIdentifier</key><string>com.velvt.mac.verify-release-fixture</string>
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
  codesign --force --sign - --options runtime "$contents/Resources/velvt-service" >/dev/null 2>&1
  codesign --force --sign - --options runtime "$app" >/dev/null 2>&1
  printf '%s' "$app"
}

run_verify() { # <app> -> output on stdout, status as the function's
  VELVT_RELEASE_ARCHS="arm64 x86_64" bash "$verify" --mode local --app "$1" 2>&1
}

good_arm64="$(slice arm64 "$app_protocol" "$app_version" "$app_commit")"
good_x86_64="$(slice x86_64 "$app_protocol" "$app_version" "$app_commit")"

# Whether this Mac can run the x86_64 slice at all: natively on Intel, under
# Rosetta 2 on Apple Silicon.
can_run_x86_64=0
if [[ "$(uname -m)" == "x86_64" ]] || arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
  can_run_x86_64=1
fi

# --- Matching slices pass -------------------------------------------------------

app="$(make_app matching "$good_arm64" "$good_x86_64")"
output="$(run_verify "$app")" || fail "a helper whose slices both match the app was refused: $output"
if [[ "$(uname -m)" == "arm64" && "$can_run_x86_64" == "1" ]]; then
  grep -q 'helper slices run: arm64 x86_64' <<<"$output" ||
    fail "the summary does not say the x86_64 slice was run: $output"
fi

# --- A mismatched x86_64 slice is refused, whatever the build machine -----------

expect_x86_64_refusal() { # <case> <x86_64 slice> <message fragment>
  local app output
  app="$(make_app "$1" "$good_arm64" "$2")"
  if output="$(run_verify "$app")"; then
    if [[ "$can_run_x86_64" == "1" ]]; then
      fail "$1: a helper whose x86_64 slice does not match the app passed verification."
    fi
    # Apple Silicon without Rosetta cannot run the slice. Local mode says so
    # rather than passing silently.
    grep -q 'WARNING: .*x86_64 slice' <<<"$output" ||
      fail "$1: an x86_64 slice that could not be run was not reported."
    return 0
  fi
  grep -q "$3" <<<"$output" || fail "$1: refused, but not with the expected reason: $output"
}

expect_x86_64_refusal protocol "$(slice x86_64 31 "$app_version" "$app_commit")" \
  'x86_64 slice.* protocol 31'
expect_x86_64_refusal version "$(slice x86_64 "$app_protocol" 1.0.13 "$app_commit")" \
  'x86_64 slice.* version 1.0.13'
expect_x86_64_refusal commit "$(slice x86_64 "$app_protocol" "$app_version" "$other_commit")" \
  "x86_64 slice.*$other_commit"

# --- Apple Silicon without Rosetta 2 ---------------------------------------------
# An arch(1) that cannot run x86_64, as on a Mac where Rosetta was never
# installed. A local build says what it could not check; a production build,
# which goes to testers, refuses.

if [[ "$(uname -m)" == "arm64" ]]; then
  shim_bin="$fixture_dir/no-rosetta-bin"
  mkdir -p "$shim_bin"
  cat >"$shim_bin/arch" <<'SHIM'
#!/bin/sh
if [ "$1" = "-x86_64" ]; then
  echo "arch: posix_spawnp: $2: Bad CPU type in executable" >&2
  exit 1
fi
exec /usr/bin/arch "$@"
SHIM
  chmod +x "$shim_bin/arch"
  app="$(make_app no-rosetta "$good_arm64" "$(slice x86_64 31 "$app_version" "$app_commit")")"
  output="$(PATH="$shim_bin:$PATH" run_verify "$app")" ||
    fail "without Rosetta, a local verification failed instead of warning: $output"
  grep -q "WARNING: this Mac cannot run the helper's x86_64 slice" <<<"$output" ||
    fail "without Rosetta, a local verification did not say the x86_64 slice went unchecked: $output"
  grep -q 'helper slices run: arm64$' <<<"$output" ||
    fail "without Rosetta, the summary claims slices that were not run: $output"
  if output="$(PATH="$shim_bin:$PATH" VELVT_RELEASE_ARCHS="arm64 x86_64" bash "$verify" --mode production --app "$app" 2>&1)"; then
    fail "without Rosetta, a production verification passed with the x86_64 slice unchecked."
  fi
  grep -q 'Rosetta 2 is not installed' <<<"$output" ||
    fail "without Rosetta, a production verification failed for another reason: $output"
fi

# --- The native arm64 check still holds (control) -------------------------------

if [[ "$(uname -m)" == "arm64" ]]; then
  app="$(make_app arm64-mismatch "$(slice arm64 31 "$app_version" "$app_commit")" "$good_x86_64")"
  if output="$(run_verify "$app")"; then
    fail "a helper whose arm64 slice does not match the app passed verification."
  fi
  grep -q 'does not match helper protocol 31' <<<"$output" ||
    fail "the arm64 mismatch was refused for the wrong reason: $output"
fi

echo "verify_release slice tests passed"
