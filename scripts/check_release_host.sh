#!/usr/bin/env bash
# Refuses unless this Mac can run every slice the release will carry
# (VELVT_RELEASE_ARCHS). verify_release.sh --mode production asks each slice of
# the helper for its protocol, version and source commit, and refuses a slice it
# cannot run. That step comes after the build, signing, two notarizations and
# the DMG, so `make alpha-dmg` and `make release` run this first: a Mac without
# Rosetta 2, or an Intel Mac, finds out in a second instead of after fifteen
# minutes and a spent DMG path.
#
# VELVT_TEST_ARCH_BIN replaces arch(1) for the tests only, as in
# verify_release.sh.
set -euo pipefail

archs="${1:-}"
[[ -n "${archs// /}" ]] || { echo "usage: $0 'ARCH...'" >&2; exit 2; }
arch_bin="${VELVT_TEST_ARCH_BIN:-/usr/bin/arch}"

for slice in $archs; do
  "$arch_bin" "-$slice" /usr/bin/true >/dev/null 2>&1 && continue
  echo "ERROR: this Mac cannot run $slice code, so the release verification could not check the helper's $slice slice." >&2
  if [[ "$slice" == "x86_64" ]]; then
    echo "Rosetta 2 is not installed. Install it with: softwareupdate --install-rosetta --agree-to-license" >&2
  else
    echo "Cut production builds on an Apple Silicon Mac with Rosetta 2, which runs every slice." >&2
  fi
  exit 1
done

echo "Release host: this Mac runs every release slice ($archs)"
