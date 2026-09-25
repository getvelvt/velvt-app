#!/usr/bin/env bash
# Ties a build to the commit it came from.
#
# 1.0.8 shipped from an unreviewed commit, and 1.0.10 and 1.0.11 from a working
# tree nothing had committed; 1.0.10's source is lost. Every version before
# 1.0.9 took its number from the make command line, so no commit records it.
# The release targets call this script so that cannot happen quietly again.
#
#   source-commit            print HEAD's commit id, suffixed -dirty when the
#                            tree has uncommitted or untracked changes, or
#                            "unknown" outside a git checkout
#   require-clean            fail on a dirty tree; VELVT_ALLOW_DIRTY_TREE=1
#                            downgrades that to a warning, for local
#                            experiments only (the build is stamped -dirty)
#   check-release VER BUILD  everything a distributable build needs: a clean
#                            tree (no override), VER and BUILD equal to
#                            swift-client/Configs/Version.xcconfig, and either
#                            HEAD is already tagged vVER (a rebuild of that
#                            release) or VER is new and BUILD is higher than
#                            every build in docs/RELEASES.md
#   tag VER BUILD DMG        create the annotated tag vVER at HEAD, recording
#                            the DMG's sha256; a no-op if vVER is already HEAD
#   ledger                   print "version build" for each row of
#                            docs/RELEASES.md's release table
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
version_file="$repo_root/swift-client/Configs/Version.xcconfig"
ledger_file="$repo_root/docs/RELEASES.md"

die() {
  echo "ERROR: $*" >&2
  exit 1
}

in_git_checkout() {
  git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1
}

tree_status() {
  git -C "$repo_root" status --porcelain --untracked-files=normal
}

source_commit() {
  if ! in_git_checkout; then
    echo unknown
    return
  fi
  local head
  head="$(git -C "$repo_root" rev-parse HEAD)"
  if [[ -n "$(tree_status)" ]]; then
    echo "$head-dirty"
  else
    echo "$head"
  fi
}

require_clean() {
  local allow_dirty="${1:-0}"
  if ! in_git_checkout; then
    if [[ "$allow_dirty" == "1" ]]; then
      echo "WARNING: not a git checkout; the build is stamped 'unknown' and must not be distributed." >&2
      return 0
    fi
    die "not a git checkout, so no commit can record what this build contains."
  fi
  local status
  status="$(tree_status)"
  [[ -n "$status" ]] || return 0
  if [[ "$allow_dirty" == "1" ]]; then
    echo "WARNING: VELVT_ALLOW_DIRTY_TREE=1: building from uncommitted changes." >&2
    echo "The build is stamped $(source_commit) and must not be given to anyone." >&2
    return 0
  fi
  {
    echo "ERROR: the working tree has uncommitted or untracked changes:"
    head -n 20 <<<"$status" | sed 's/^/  /'
    [[ "$(wc -l <<<"$status")" -le 20 ]] || echo "  ..."
    echo "A build must come from a commit, or nothing can reproduce it."
    echo "Commit or stash the changes. For a local experiment that will never"
    echo "leave this Mac, set VELVT_ALLOW_DIRTY_TREE=1 (package-release and dmg"
    echo "only; alpha-dmg and release never accept it)."
  } >&2
  exit 1
}

xcconfig_value() {
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\\([^[:space:]]*\\)[[:space:]]*\$/\\1/p" "$version_file" | head -n 1
}

# Rows of the release table look like "| 1.0.11 | 17 | 30 | ...". Rows in any
# other table of the ledger do not start with a dotted version and a number.
ledger_rows() {
  [[ -f "$ledger_file" ]] || die "release ledger not found at docs/RELEASES.md."
  awk -F'|' '
    $2 ~ /^ *[0-9]+\.[0-9]+\.[0-9]+ *$/ && $3 ~ /^ *[0-9]+ *$/ {
      gsub(/ /, "", $2); gsub(/ /, "", $3); print $2, $3
    }' "$ledger_file"
}

check_release() {
  local version="${1:-}" build="${2:-}"
  [[ -n "$version" && "$build" =~ ^[0-9]+$ ]] || die "usage: $0 check-release VERSION BUILD (BUILD is a number)"
  require_clean 0

  local file_version file_build
  file_version="$(xcconfig_value MARKETING_VERSION)"
  file_build="$(xcconfig_value CURRENT_PROJECT_VERSION)"
  [[ "$version" == "$file_version" && "$build" == "$file_build" ]] || die \
    "asked to build $version ($build), but swift-client/Configs/Version.xcconfig says $file_version ($file_build). Change the version there and commit it; do not override it on the command line."

  local head tagged
  head="$(git -C "$repo_root" rev-parse HEAD)"
  if tagged="$(git -C "$repo_root" rev-parse -q --verify "refs/tags/v$version^{commit}")"; then
    [[ "$tagged" == "$head" ]] || die \
      "v$version is already tagged at ${tagged:0:12}, and HEAD is ${head:0:12}. A second, different $version would be indistinguishable from the first: raise the version and build in Version.xcconfig."
    echo "Rebuilding the tagged release v$version from ${head:0:12}."
    return 0
  fi

  local rows shipped_version shipped_build
  rows="$(ledger_rows)"
  [[ -n "$rows" ]] || die "docs/RELEASES.md lists no releases, so nothing can say whether $version ($build) is new."
  while read -r shipped_version shipped_build; do
    [[ "$shipped_version" != "$version" ]] || die \
      "$version already shipped as build $shipped_build (docs/RELEASES.md) and has no tag at HEAD. Raise the version in Version.xcconfig."
    (( build > shipped_build )) || die \
      "build $build is not higher than the shipped build $shipped_build ($shipped_version). Raise CURRENT_PROJECT_VERSION in Version.xcconfig."
  done <<<"$rows"
}

tag_release() {
  local version="${1:-}" build="${2:-}" dmg="${3:-}"
  [[ -n "$version" && -n "$build" && -f "$dmg" ]] || die "usage: $0 tag VERSION BUILD DMG"
  local head tagged digest
  head="$(git -C "$repo_root" rev-parse HEAD)"
  digest="$(shasum -a 256 "$dmg" | awk '{print $1}')"
  if tagged="$(git -C "$repo_root" rev-parse -q --verify "refs/tags/v$version^{commit}")"; then
    [[ "$tagged" == "$head" ]] || die "v$version already points at ${tagged:0:12}, not HEAD ${head:0:12}."
    echo "v$version already tags ${head:0:12}."
  else
    git -C "$repo_root" tag -a "v$version" "$head" -F - <<EOF
Velvt $version (build $build) — source of $(basename "$dmg")

$(basename "$dmg") sha256 $digest
EOF
    echo "Tagged v$version at ${head:0:12}."
  fi
  echo "Next: git push origin v$version, and add the $version row to docs/RELEASES.md."
}

case "${1:-}" in
  source-commit) source_commit ;;
  require-clean) require_clean "${VELVT_ALLOW_DIRTY_TREE:-0}" ;;
  check-release) shift; check_release "$@" ;;
  tag) shift; tag_release "$@" ;;
  ledger) ledger_rows ;;
  *) die "usage: $0 source-commit | require-clean | check-release VERSION BUILD | tag VERSION BUILD DMG | ledger" ;;
esac
