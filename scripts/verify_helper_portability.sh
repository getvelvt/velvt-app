#!/usr/bin/env bash
set -euo pipefail

helper="${1:-}"

[[ -x "$helper" ]] || {
  echo "ERROR: embedded helper is missing or not executable: $helper" >&2
  exit 1
}

# Older helpers opened this source-relative path at runtime. It exists on the
# build machine and nowhere else, so those binaries entered a silent crash loop
# after installation from a DMG. The portable implementation embeds the file's
# contents at compile time and does not retain this lookup string.
if strings -a "$helper" | grep -F '../proto/ipc_socket_path' >/dev/null; then
  echo "ERROR: embedded helper still resolves proto/ipc_socket_path from the build checkout." >&2
  echo "Rebuild the Rust helper from the current source before packaging." >&2
  exit 1
fi
