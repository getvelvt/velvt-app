#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
create_dmg="$repo_root/scripts/create_dmg.sh"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

expect_failure() {
  if "$@" >/dev/null 2>&1; then
    echo "ERROR: command unexpectedly succeeded: $*" >&2
    exit 1
  fi
}

mkdir -p "$fixture_dir/Velvt.app"
ln -s "$fixture_dir/Velvt.app" "$fixture_dir/Symlink.app"
touch "$fixture_dir/existing.dmg"

expect_failure env VELVT_DMGBUILD_BIN="$fixture_dir/missing-dmgbuild" \
  "$create_dmg" "$fixture_dir/Velvt.app" "$fixture_dir/new.dmg" local
expect_failure env VELVT_DMGBUILD_BIN=/usr/bin/true \
  "$create_dmg" "$fixture_dir/Symlink.app" "$fixture_dir/new.dmg" local
expect_failure env VELVT_DMGBUILD_BIN=/usr/bin/true \
  "$create_dmg" "$fixture_dir/Velvt.app" "$fixture_dir/existing.dmg" production
expect_failure env VELVT_DMGBUILD_BIN=/usr/bin/true VELVT_DMG_VOLUME_NAME=Unexpected \
  "$create_dmg" "$fixture_dir/Velvt.app" "$fixture_dir/new.dmg" local

echo "DMG release policy tests passed"
