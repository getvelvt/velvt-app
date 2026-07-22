#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_dir="$(cd "$script_dir/../.." && pwd -P)"
sparkle_bin_dir="${VELVT_SPARKLE_BIN_DIR:-$repo_dir/swift-client/.build/artifacts/sparkle/Sparkle/bin}"

if [[ ! -x "$sparkle_bin_dir/sign_update" ]]; then
  if [[ "${VELVT_REQUIRE_REAL_SPARKLE_TOOLS:-0}" == "1" ]]; then
    echo "ERROR: real Sparkle tools are required but unavailable at $sparkle_bin_dir" >&2
    exit 1
  fi
  echo "SKIP: resolve Swift packages to run the real Sparkle adversarial harness"
  exit 0
fi

output="$($repo_dir/scripts/update_local_adversarial_harness.sh --sparkle-bin-dir "$sparkle_bin_dir")"
printf '%s\n' "$output"
grep -q 'LOCAL UPDATE ADVERSARIAL HARNESS PASSED' <<<"$output"
grep -q 'NOT VERIFIED: app replacement' <<<"$output"

echo "update local adversarial harness test passed"
