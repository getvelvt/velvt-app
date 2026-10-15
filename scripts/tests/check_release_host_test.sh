#!/usr/bin/env bash
# alpha-dmg and release refuse, before they build, on a Mac that cannot run
# every slice the release carries. The defect this pins: the production
# verification that refuses an unrunnable slice ran only after the build,
# signing, two notarizations and the DMG, so a Mac without Rosetta 2 found out
# fifteen minutes and a spent DMG path later.
#
# arch(1) is a stand-in through VELVT_TEST_ARCH_BIN, so every case runs on any
# Mac; one case uses the real arch(1) and expects what this Mac can do.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
check="$repo_root/scripts/check_release_host.sh"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
unset VELVT_TEST_ARCH_BIN

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[[ -x "$check" ]] || fail "scripts/check_release_host.sh is missing or not executable."

# An arch(1) that runs every slice except those in FIXTURE_NO_SLICES.
fake_arch="$fixture_dir/arch"
cat >"$fake_arch" <<'SHIM'
#!/bin/sh
for slice in $FIXTURE_NO_SLICES; do
  if [ "$1" = "-$slice" ]; then
    echo "arch: posix_spawnp: $2: Bad CPU type in executable" >&2
    exit 1
  fi
done
exit 0
SHIM
chmod +x "$fake_arch"

run_without() { # <slices this Mac cannot run> -> output, status as the function's
  FIXTURE_NO_SLICES="$1" VELVT_TEST_ARCH_BIN="$fake_arch" "$check" "arm64 x86_64" 2>&1
}

output="$(run_without "")" || fail "a Mac that runs both slices was refused: $output"
grep -q 'runs every release slice (arm64 x86_64)' <<<"$output" || fail "a pass does not say what was checked: $output"

if output="$(run_without x86_64)"; then
  fail "Apple Silicon without Rosetta 2 was accepted."
fi
grep -q 'cannot run x86_64 code' <<<"$output" && grep -q 'softwareupdate --install-rosetta' <<<"$output" ||
  fail "without Rosetta, the refusal does not say how to install it: $output"

if output="$(run_without arm64)"; then
  fail "an Intel Mac, which cannot run the arm64 slice, was accepted."
fi
grep -q 'cannot run arm64 code' <<<"$output" && grep -q 'Apple Silicon' <<<"$output" ||
  fail "on Intel, the refusal does not say where to build: $output"

if "$check" "" >/dev/null 2>&1; then
  fail "an empty architecture list was accepted."
fi

# The real arch(1), named by path: what is first on PATH does not matter. A GNU
# arch (coreutils) rejects -x86_64 and would blame a missing Rosetta.
gnu_arch_bin="$fixture_dir/gnu-arch-bin"
mkdir -p "$gnu_arch_bin"
printf '#!/bin/sh\necho "arch: extra operand" >&2\nexit 1\n' >"$gnu_arch_bin/arch"
chmod +x "$gnu_arch_bin/arch"
expected_pass=1
for slice in arm64 x86_64; do
  /usr/bin/arch "-$slice" /usr/bin/true >/dev/null 2>&1 || expected_pass=0
done
if output="$(PATH="$gnu_arch_bin:$PATH" "$check" "arm64 x86_64" 2>&1)"; then
  [[ "$expected_pass" == "1" ]] || fail "this Mac cannot run both slices, but the check passed: $output"
else
  [[ "$expected_pass" == "0" ]] || fail "this Mac runs both slices, but the check refused: $output"
fi

# --- alpha-dmg and release run it before they build -------------------------------

line_of() { # <fixed string> <text>
  { grep -n -m1 -F -e "$1" <<<"$2" || true; } | cut -d: -f1
}
expect_checked_before_build() { # <target> [make args...]
  local target="$1" plan check_line build_line
  shift
  plan="$(make -n -C "$repo_root" "$target" "$@" 2>/dev/null || true)"
  check_line="$(line_of 'check_release_host.sh "arm64 x86_64"' "$plan")"
  build_line="$(line_of '-project swift-client/VelvtMac.xcodeproj' "$plan")"
  [[ -n "$check_line" ]] || fail "make $target does not check that this Mac runs every release slice."
  [[ -n "$build_line" ]] || fail "make -n $target printed no build step to compare against."
  ((check_line < build_line)) || fail "make $target builds before it checks this Mac runs every slice."
}

expect_checked_before_build alpha-dmg
expect_checked_before_build release \
  VELVT_RELEASE_VERSION=0.0.0 VELVT_RELEASE_BUILD=0 \
  VELVT_PRODUCTION_API_BASE_URL=https://api.example.com VELVT_APPROVED_PRODUCTION_API_HOST=api.example.com

echo "check_release_host tests passed"
