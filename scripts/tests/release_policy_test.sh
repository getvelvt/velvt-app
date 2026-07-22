#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
preflight="$repo_root/scripts/preflight_distribution.sh"

expect_failure() {
  if "$@" >/dev/null 2>&1; then
    echo "ERROR: command unexpectedly succeeded: $*" >&2
    exit 1
  fi
}

expect_failure "$preflight" "" production api.getvelvt.com
expect_failure "$preflight" http://api.getvelvt.com production api.getvelvt.com
expect_failure "$preflight" https://dev-api.getvelvt.com production api.getvelvt.com
expect_failure "$preflight" https://staging-api.getvelvt.com production api.getvelvt.com
expect_failure "$preflight" https://10.0.0.1 production 10.0.0.1
expect_failure "$preflight" https://172.16.0.1 production 172.16.0.1
expect_failure "$preflight" https://192.168.1.1 production 192.168.1.1
expect_failure "$preflight" https://169.254.1.1 production 169.254.1.1
expect_failure "$preflight" https://user@api.getvelvt.com production api.getvelvt.com
expect_failure "$preflight" 'https://api.getvelvt.com/v1?redirect=other' production api.getvelvt.com
expect_failure "$preflight" 'https://api.getvelvt.com/v1#fragment' production api.getvelvt.com
expect_failure "$preflight" https://api.getvelvt.com:8443 production api.getvelvt.com
expect_failure "$preflight" https://other.getvelvt.com production api.getvelvt.com
expect_failure "$preflight" https://api.getvelvt.com production
"$preflight" https://api.getvelvt.com production api.getvelvt.com >/dev/null
"$preflight" https://api.getvelvt.com:443/v1 production api.getvelvt.com >/dev/null
expect_failure make -C "$repo_root" release
expect_failure make -C "$repo_root" release \
  VELVT_PRODUCTION_API_BASE_URL=https://api.getvelvt.com

fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
touch "$fixture_dir/Cargo.toml"
expect_failure "$repo_root/scripts/build_rust_helper.sh" \
  "$fixture_dir" "$fixture_dir/output" YES arm64

echo "release policy tests passed"
