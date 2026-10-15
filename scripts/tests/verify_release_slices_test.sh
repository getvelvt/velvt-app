#!/usr/bin/env bash
# verify_release.sh asks every slice of the embedded helper for its protocol,
# version and source commit and compares them with the app. The defect this
# pins: it asked only the slice the build machine ran natively, so on Apple
# Silicon a universal helper whose x86_64 slice was from another build passed,
# and every Intel tester would have hit a version-mismatch alert on each launch.
# Which slice ran natively also depended on the perl and arch(1) first on PATH.
#
# The fixture is an ad-hoc signed Velvt.app whose helper is lipo'd from two
# small C programs, so this needs clang, lipo and codesign, and no Rust. It
# passes on Apple Silicon (with or without Rosetta 2, natively or from a Rosetta
# shell) and on Intel; each case says what it needs.
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

good_arm64="$(slice arm64 "$app_protocol" "$app_version" "$app_commit")"
good_x86_64="$(slice x86_64 "$app_protocol" "$app_version" "$app_commit")"

# Which slices this Mac can run: on Apple Silicon arm64, and x86_64 under
# Rosetta 2; on Intel x86_64 alone. Asked of arch(1) itself, not uname, which
# says x86_64 in a Rosetta shell on Apple Silicon.
can_run() { /usr/bin/arch "-$1" /usr/bin/true >/dev/null 2>&1; }
runnable=""
for candidate in arm64 x86_64; do
  if can_run "$candidate"; then runnable+="${runnable:+ }$candidate"; fi
done
[[ -n "$runnable" ]] || fail "this Mac can run neither slice, so nothing here can be checked."

# A stand-in for arch(1) that cannot run the slices named in FIXTURE_NO_SLICES,
# as on a Mac without Rosetta 2 (x86_64) or an Intel Mac (arm64). Only the
# tests point VELVT_TEST_ARCH_BIN at it.
limited_arch="$fixture_dir/limited-arch"
cat >"$limited_arch" <<'SHIM'
#!/bin/sh
for slice in $FIXTURE_NO_SLICES; do
  if [ "$1" = "-$slice" ]; then
    echo "arch: posix_spawnp: $2: Bad CPU type in executable" >&2
    exit 1
  fi
done
exec /usr/bin/arch "$@"
SHIM
chmod +x "$limited_arch"

run_verify() { # <app> [mode] -> output on stdout, status as the function's
  VELVT_RELEASE_ARCHS="arm64 x86_64" bash "$verify" --mode "${2:-local}" --app "$1" 2>&1
}

run_verify_without() { # <slices this Mac cannot run> <app> [mode]
  FIXTURE_NO_SLICES="$1" VELVT_TEST_ARCH_BIN="$limited_arch" run_verify "$2" "${3:-local}"
}

# --- Matching slices pass, and the summary names each slice that was run --------

app="$(make_app matching "$good_arm64" "$good_x86_64")"
output="$(run_verify "$app")" || fail "a helper whose slices both match the app was refused: $output"
grep -q "helper slices run: $runnable\$" <<<"$output" ||
  fail "the summary does not say the slices this Mac can run ($runnable) were run: $output"

# --- A mismatched slice is refused, whatever the build machine ------------------

expect_slice_refusal() { # <case> <slice> <arm64 slice> <x86_64 slice> <message fragment>
  local app output
  app="$(make_app "$1" "$3" "$4")"
  if output="$(run_verify "$app")"; then
    if can_run "$2"; then
      fail "$1: a helper whose $2 slice does not match the app passed verification."
    fi
    # A slice this Mac cannot run. Local mode says so rather than passing
    # silently.
    grep -q "WARNING: this Mac cannot run the helper's $2 slice" <<<"$output" ||
      fail "$1: a $2 slice that could not be run was not reported."
    return 0
  fi
  grep -q "$5" <<<"$output" || fail "$1: refused, but not with the expected reason: $output"
}

expect_slice_refusal x86_64-protocol x86_64 "$good_arm64" "$(slice x86_64 31 "$app_version" "$app_commit")" \
  'x86_64 slice speaks protocol 31'
expect_slice_refusal x86_64-version x86_64 "$good_arm64" "$(slice x86_64 "$app_protocol" 1.0.13 "$app_commit")" \
  'x86_64 slice reports version 1.0.13'
expect_slice_refusal x86_64-commit x86_64 "$good_arm64" "$(slice x86_64 "$app_protocol" "$app_version" "$other_commit")" \
  "x86_64 slice was built from $other_commit"
expect_slice_refusal arm64-protocol arm64 "$(slice arm64 31 "$app_version" "$app_commit")" "$good_x86_64" \
  'arm64 slice speaks protocol 31'
expect_slice_refusal arm64-commit arm64 "$(slice arm64 "$app_protocol" "$app_version" "$other_commit")" "$good_x86_64" \
  "arm64 slice was built from $other_commit"

# --- What is first on PATH does not decide which slice is checked ---------------
# Needs a Mac that runs both slices: Apple Silicon with Rosetta 2.

if [[ "$runnable" == "arm64 x86_64" ]]; then
  arm64_mismatch="$(make_app arm64-mismatch "$(slice arm64 31 "$app_version" "$app_commit")" "$good_x86_64")"

  # An x86_64 perl first on PATH (an Intel Homebrew carried over to Apple
  # Silicon). A helper exec'd from it ran its x86_64 slice, so the arm64 one
  # went unchecked and this mismatch passed.
  x86_64_perl_bin="$fixture_dir/x86_64-perl-bin"
  mkdir -p "$x86_64_perl_bin"
  printf '#!/bin/sh
exec /usr/bin/arch -x86_64 /usr/bin/perl "$@"
' >"$x86_64_perl_bin/perl"
  chmod +x "$x86_64_perl_bin/perl"
  if output="$(PATH="$x86_64_perl_bin:$PATH" run_verify "$arm64_mismatch")"; then
    fail "with an x86_64 perl first on PATH, a helper whose arm64 slice does not match the app passed: $output"
  fi
  grep -q 'arm64 slice speaks protocol 31' <<<"$output" ||
    fail "with an x86_64 perl first on PATH, the arm64 mismatch was refused for another reason: $output"

  # The verifier itself running under Rosetta, as from a Rosetta terminal.
  if output="$(VELVT_RELEASE_ARCHS="arm64 x86_64" /usr/bin/arch -x86_64 /bin/bash "$verify" --mode local --app "$arm64_mismatch" 2>&1)"; then
    fail "run from an x86_64 shell, a helper whose arm64 slice does not match the app passed: $output"
  fi
  grep -q 'arm64 slice speaks protocol 31' <<<"$output" ||
    fail "run from an x86_64 shell, the arm64 mismatch was refused for another reason: $output"

  # A GNU arch (coreutils) first on PATH, which rejects -x86_64 and so made the
  # x86_64 slice look unrunnable: a production build refused, blaming Rosetta.
  gnu_arch_bin="$fixture_dir/gnu-arch-bin"
  mkdir -p "$gnu_arch_bin"
  printf '#!/bin/sh
echo "arch: extra operand" >&2
exit 1
' >"$gnu_arch_bin/arch"
  chmod +x "$gnu_arch_bin/arch"
  output="$(PATH="$gnu_arch_bin:$PATH" run_verify "$app")" ||
    fail "with a GNU arch first on PATH, a matching helper was refused: $output"
  grep -q 'helper slices run: arm64 x86_64$' <<<"$output" ||
    fail "with a GNU arch first on PATH, not every slice was run: $output"
fi

# --- A Mac that cannot run a slice ----------------------------------------------
# A local build says what it could not check; a production build, which goes to
# testers, refuses. The mismatched slice is the one this Mac cannot run, so
# only skipping it lets the local verification pass.

# Apple Silicon without Rosetta 2. Needs a Mac that runs arm64.
if can_run arm64; then
  app="$(make_app no-rosetta "$good_arm64" "$(slice x86_64 31 "$app_version" "$app_commit")")"
  output="$(run_verify_without x86_64 "$app")" ||
    fail "without Rosetta, a local verification failed instead of warning: $output"
  grep -q "WARNING: this Mac cannot run the helper's x86_64 slice" <<<"$output" ||
    fail "without Rosetta, a local verification did not say the x86_64 slice went unchecked: $output"
  grep -q 'helper slices run: arm64$' <<<"$output" ||
    fail "without Rosetta, the summary claims slices that were not run: $output"
  if output="$(run_verify_without x86_64 "$app" production)"; then
    fail "without Rosetta, a production verification passed with the x86_64 slice unchecked."
  fi
  grep -q 'Rosetta 2 is not installed' <<<"$output" ||
    fail "without Rosetta, a production verification failed for another reason: $output"
fi

# An Intel Mac, which cannot run the arm64 slice most testers use. Needs a Mac
# that runs x86_64.
if can_run x86_64; then
  app="$(make_app intel "$(slice arm64 31 "$app_version" "$app_commit")" "$good_x86_64")"
  output="$(run_verify_without arm64 "$app")" ||
    fail "on Intel, a local verification failed instead of warning: $output"
  grep -q "WARNING: this Mac cannot run the helper's arm64 slice" <<<"$output" ||
    fail "on Intel, a local verification did not say the arm64 slice went unchecked: $output"
  grep -q 'helper slices run: x86_64$' <<<"$output" ||
    fail "on Intel, the summary claims slices that were not run: $output"
  if output="$(run_verify_without arm64 "$app" production)"; then
    fail "on Intel, a production verification passed with the arm64 slice unchecked."
  fi
  grep -q "cannot run the helper's arm64 slice" <<<"$output" && grep -q 'Apple Silicon' <<<"$output" ||
    fail "on Intel, a production verification failed for another reason: $output"
fi

# A Mac that can run no slice at all has checked nothing, even locally.
app="$(make_app none-runnable "$good_arm64" "$good_x86_64")"
if output="$(run_verify_without "arm64 x86_64" "$app")"; then
  fail "a verification that could run no slice passed: $output"
fi
grep -q 'can run none of the helper' <<<"$output" ||
  fail "a verification that could run no slice failed for another reason: $output"

echo "verify_release slice tests passed"
