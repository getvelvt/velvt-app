#!/usr/bin/env bash
# Every .swift file under swift-client/Sources must be referenced at least four
# times in project.pbxproj: the PBXBuildFile line, the PBXFileReference line,
# the group child line, and the Sources build-phase line.
#
# Why this needs a guard at all: SwiftPM compiles the whole tree by directory,
# so `swift test` is green whether or not a file is in the Xcode target. The
# Xcode target lists its files explicitly. A new file added without the pbxproj
# entries therefore passes both test suites and fails inside `make alpha-dmg`,
# minutes into a build, with a link error that names a symbol rather than a
# file. That exact failure has cost this project a build once already.
#
# It runs on every pull request, in the swift job of .github/workflows/ci.yml.
# Run it directly, or install it as a pre-push hook via
# scripts/install_git_hooks.sh, to see the failure before the push rather than
# after it.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# Overridable so the guard can be tested against a synthetic tree without
# writing a throwaway .swift file into the real Sources directory.
sources="${VELVT_PBXPROJ_SOURCES:-$root/swift-client/Sources}"
pbxproj="${VELVT_PBXPROJ_FILE:-$root/swift-client/VelvtMac.xcodeproj/project.pbxproj}"
minimum="${VELVT_PBXPROJ_MIN_REFS:-4}"

if [[ ! -d "$sources" ]]; then
  echo "no $sources — nothing to check." >&2
  exit 0
fi

# `swift-client/VelvtMac.xcodeproj/project.pbxproj` is tracked, so every
# complete checkout has one. This used to exit 0 when the file was absent, back
# when .gitignore claimed to exclude it; a guard that passes when it cannot see
# the thing it guards is indistinguishable from no guard, and the only reason
# that was survivable is that the file was tracked anyway.
if [[ ! -f "$pbxproj" ]]; then
  cat >&2 <<MSG
No $pbxproj, so target membership cannot be checked.

That file is tracked, so a complete checkout has one. Either the checkout is
incomplete or the file was deleted. Restore it and run this again; this check
fails rather than skips, because skipping is how an unregistered file reaches
a release build.
MSG
  exit 1
fi

checked=0
bad=0
while IFS= read -r file; do
  base="$(basename "$file")"
  # Anchored on a non-identifier character so Foo.swift cannot be credited with
  # BarFoo.swift's references. Counts matching LINES, which is exactly four for
  # a correctly-registered file.
  count="$(grep -cE "[^A-Za-z0-9_]${base//./\\.}" "$pbxproj" || true)"
  checked=$((checked + 1))
  if (( count < minimum )); then
    printf 'MISSING  %-52s %d/%d references\n' "${file#"$root/"}" "$count" "$minimum"
    bad=$((bad + 1))
  fi
done < <(find "$sources" -name '*.swift' | sort)

if (( bad > 0 )); then
  cat >&2 <<MSG

$bad of $checked Swift file(s) are not fully registered in the Xcode target.
SwiftPM will still compile them and both test suites will still pass; the DMG
build is where this surfaces. Open VelvtMac.xcodeproj, confirm each file's
Target Membership, and commit the resulting project.pbxproj.
MSG
  exit 1
fi

echo "pbxproj membership: $checked Swift file(s), all with >= $minimum references."
