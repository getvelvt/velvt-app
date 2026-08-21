#!/usr/bin/env bash
# Points this clone's hooks at the tracked .githooks/ directory.
#
# Hooks are not shared by cloning, so this has to be run once per clone. It is
# a single `git config` on the local repository: it writes to .git/config, and
# touches no ref, no index, and no working-tree file.
#
#   ./scripts/install_git_hooks.sh
#
# To undo:  git config --unset core.hooksPath

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

[[ -d "$root/.git" ]] || {
  echo "ERROR: $root is not a git repository root." >&2
  exit 1
}
[[ -x "$root/.githooks/pre-push" ]] || {
  echo "ERROR: $root/.githooks/pre-push is missing or not executable." >&2
  exit 1
}

git -C "$root" config core.hooksPath .githooks

cat <<MSG
core.hooksPath -> .githooks

Installed:
  pre-push   every .swift under swift-client/Sources must have >= 4 references
             in project.pbxproj

Check it now without pushing:
  ./scripts/verify_pbxproj_membership.sh
MSG
