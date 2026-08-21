#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
create_dmg="$repo_root/scripts/create_dmg.sh"
verify_helper_portability="$repo_root/scripts/verify_helper_portability.sh"
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

portable_helper="$fixture_dir/portable-helper"
stale_helper="$fixture_dir/stale-helper"
printf '#!/bin/sh\n# socket path embedded at compile time\n' > "$portable_helper"
printf '#!/bin/sh\n# runtime lookup: ../proto/ipc_socket_path\n' > "$stale_helper"
chmod +x "$portable_helper" "$stale_helper"
bash "$verify_helper_portability" "$portable_helper"
expect_failure bash "$verify_helper_portability" "$stale_helper"

expect_failure make -s -C "$repo_root" dmg VELVT_ALLOW_LOCAL_DMG=0

echo "DMG release policy tests passed"
