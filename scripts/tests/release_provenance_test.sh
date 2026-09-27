#!/usr/bin/env bash
# Guards what ties a build to its source: one version file, a release ledger it
# cannot fall behind, the dirty-tree refusal, and the commit stamp.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
provenance="$repo_root/scripts/release_provenance.sh"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

expect_failure() {
  if "$@" >/dev/null 2>&1; then
    fail "command unexpectedly succeeded: $*"
  fi
}

xcconfig_value() {
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$repo_root/swift-client/Configs/Version.xcconfig" | head -n 1
}

# --- One version, and it does not lag the ledger ------------------------------

version="$(xcconfig_value MARKETING_VERSION)"
build="$(xcconfig_value CURRENT_PROJECT_VERSION)"
[[ "$version" =~ ^[0-9]+(\.[0-9]+){2}$ && "$build" =~ ^[0-9]+$ ]] ||
  fail "Version.xcconfig must declare MARKETING_VERSION x.y.z and a numeric CURRENT_PROJECT_VERSION."

for config in Debug Release; do
  file="$repo_root/swift-client/Configs/$config.xcconfig"
  grep -qx '#include "Version.xcconfig"' "$file" || fail "$config.xcconfig does not include Version.xcconfig."
  if grep -Eq '^[[:space:]]*(MARKETING_VERSION|CURRENT_PROJECT_VERSION)[[:space:]]*=' "$file"; then
    fail "$config.xcconfig sets its own version; it lives only in Version.xcconfig."
  fi
done

newest_version=""
newest_build=0
while read -r shipped_version shipped_build; do
  if (( shipped_build > newest_build )); then
    newest_version="$shipped_version"
    newest_build="$shipped_build"
  fi
done < <("$provenance" ledger)
(( newest_build > 0 )) || fail "docs/RELEASES.md lists no releases."
(( build >= newest_build )) ||
  fail "Version.xcconfig build $build lags the newest shipped build $newest_build ($newest_version)."
if (( build == newest_build )) && [[ "$version" != "$newest_version" ]]; then
  fail "Version.xcconfig says $version ($build), but build $build shipped as $newest_version."
fi

# The Makefile's defaults are that file's values, and the release targets check
# provenance before they build anything.
make_plan="$(make -n -C "$repo_root" package-release VELVT_ALLOW_DIRTY_TREE=1 2>/dev/null)"
grep -Fq "MARKETING_VERSION=\"$version\"" <<<"$make_plan" || fail "package-release does not build $version."
grep -Fq "CURRENT_PROJECT_VERSION=\"$build\"" <<<"$make_plan" || fail "package-release does not build $build."
grep -Eq 'VELVT_SOURCE_COMMIT="[0-9a-f]{40}(-dirty)?"' <<<"$make_plan" ||
  fail "package-release does not stamp the source commit."
line_of() {
  grep -n -m1 -F -e "$1" <<<"$make_plan" | cut -d: -f1
}
guard_line="$(line_of 'release_provenance.sh require-clean')"
[[ -n "$guard_line" && "$guard_line" -lt "$(line_of preflight_distribution.sh)" && "$guard_line" -lt "$(line_of '-project swift-client/VelvtMac.xcodeproj')" ]] ||
  fail "package-release must check the tree before it builds anything."
for target in alpha-dmg release; do
  target_plan="$(make -n -C "$repo_root" "$target" VELVT_RELEASE_VERSION=0.0.0 VELVT_RELEASE_BUILD=0 2>/dev/null || true)"
  grep -Fq 'release_provenance.sh check-release' <<<"$target_plan" ||
    fail "$target does not run check-release."
done

# --- The script, against a throwaway repository -------------------------------

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
repo="$fixture_dir/repo"
mkdir -p "$repo/scripts" "$repo/swift-client/Configs" "$repo/docs"
cp "$provenance" "$repo/scripts/"
script="$repo/scripts/release_provenance.sh"
cat >"$repo/docs/RELEASES.md" <<'LEDGER'
| Version | Build | Notes |
|---|---:|---|
| 1.0.11 | 17 | shipped |
| 1.0.10 | 16 | shipped |

| Something else | 99 | not a release row |
LEDGER
set_version() {
  printf 'MARKETING_VERSION = %s\nCURRENT_PROJECT_VERSION = %s\n' "$1" "$2" >"$repo/swift-client/Configs/Version.xcconfig"
  git -C "$repo" add -A
  git -C "$repo" commit -qm "version $1 ($2)"
}
git -C "$repo" init -q
set_version 1.0.12 18

[[ "$("$script" ledger)" == $'1.0.11 17\n1.0.10 16' ]] || fail "ledger parsing read: $("$script" ledger)"

head_commit="$(git -C "$repo" rev-parse HEAD)"
[[ "$("$script" source-commit)" == "$head_commit" ]] || fail "source-commit on a clean tree"
"$script" require-clean >/dev/null
"$script" check-release 1.0.12 18 >/dev/null

touch "$repo/untracked.txt"
[[ "$("$script" source-commit)" == "$head_commit-dirty" ]] || fail "an untracked file must mark the stamp dirty"
expect_failure "$script" require-clean
VELVT_ALLOW_DIRTY_TREE=1 "$script" require-clean 2>/dev/null
expect_failure env VELVT_ALLOW_DIRTY_TREE=1 "$script" check-release 1.0.12 18
rm "$repo/untracked.txt"

# A version only on the command line is refused.
expect_failure "$script" check-release 1.0.13 18
expect_failure "$script" check-release 1.0.12 19

# A version or build that already shipped, without a tag at HEAD, is refused.
set_version 1.0.11 18
expect_failure "$script" check-release 1.0.11 18
set_version 1.0.13 17
expect_failure "$script" check-release 1.0.13 17

# A tag elsewhere refuses a second, different build of that version; a tag at
# HEAD is a rebuild of that release.
set_version 1.0.12 18
git -C "$repo" tag -a v1.0.12 -m test HEAD~1
expect_failure "$script" check-release 1.0.12 18
git -C "$repo" tag -d v1.0.12 >/dev/null
printf 'dmg' >"$fixture_dir/Velvt-1.0.12.dmg"
"$script" tag 1.0.12 18 "$fixture_dir/Velvt-1.0.12.dmg" >/dev/null
[[ "$(git -C "$repo" rev-parse 'v1.0.12^{commit}')" == "$(git -C "$repo" rev-parse HEAD)" ]] || fail "tag must point at HEAD"
git -C "$repo" tag -l --format='%(contents)' v1.0.12 | grep -Fq "$(shasum -a 256 "$fixture_dir/Velvt-1.0.12.dmg" | awk '{print $1}')" ||
  fail "tag must record the DMG sha256"
"$script" tag 1.0.12 18 "$fixture_dir/Velvt-1.0.12.dmg" >/dev/null
"$script" check-release 1.0.12 18 >/dev/null

# Outside a checkout nothing can be recorded, and the stamp says so.
outside="$fixture_dir/outside"
mkdir -p "$outside/scripts"
cp "$provenance" "$outside/scripts/"
[[ "$(GIT_CEILING_DIRECTORIES="$fixture_dir" "$outside/scripts/release_provenance.sh" source-commit)" == unknown ]] ||
  fail "source-commit outside a checkout must print unknown"

# --- The font license ships with the fonts ------------------------------------

fonts="$repo_root/swift-client/Sources/VelvtMac/Resources/Fonts"
grep -q '^SIL OPEN FONT LICENSE Version 1.1' "$fonts/OFL.txt" || fail "Fonts/OFL.txt is not the OFL 1.1 text."
grep -q '^Copyright .* The Manrope Project Authors' "$fonts/OFL.txt" || fail "Fonts/OFL.txt lacks the copyright line."
grep -Fq 'Copyright 2019 The Manrope Project Authors' "$fonts/NOTICE.md" ||
  fail "NOTICE.md must carry the copyright notice embedded in the vendored faces."
grep -Fq '.copy("Resources/Fonts")' "$repo_root/swift-client/Package.swift" ||
  fail "Package.swift no longer copies the whole Fonts directory, license included."

echo "release provenance tests passed"
