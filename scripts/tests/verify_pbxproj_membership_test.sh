#!/usr/bin/env bash
# The guard has to fail on a real omission, not just pass on a clean tree. A
# guard nobody has watched fail is a guard nobody knows works.
#
# The negative cases run against a synthetic Sources directory and a synthetic
# pbxproj, so no throwaway .swift file is ever written into the real tree.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
guard="$repo_root/scripts/verify_pbxproj_membership.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$guard" ]] || { echo "ERROR: $guard is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

mkdir -p "$work/Sources/UI"
touch "$work/Sources/Registered.swift"
touch "$work/Sources/UI/AlsoRegistered.swift"

pbx="$work/project.pbxproj"
cat > "$pbx" <<'PBX'
// !$*UTF8*$!
{
	objects = {
		AAAA1 /* Registered.swift in Sources */ = {isa = PBXBuildFile; fileRef = AAAA2 /* Registered.swift */; };
		AAAA2 /* Registered.swift */ = {isa = PBXFileReference; name = Registered.swift; path = Registered.swift; };
		BBBB1 /* AlsoRegistered.swift in Sources */ = {isa = PBXBuildFile; fileRef = BBBB2 /* AlsoRegistered.swift */; };
		BBBB2 /* AlsoRegistered.swift */ = {isa = PBXFileReference; name = AlsoRegistered.swift; path = UI/AlsoRegistered.swift; };
		CCCC /* group */ = {
			children = (
				AAAA2 /* Registered.swift */,
				BBBB2 /* AlsoRegistered.swift */,
			);
		};
		DDDD /* Sources */ = {
			files = (
				AAAA1 /* Registered.swift in Sources */,
				BBBB1 /* AlsoRegistered.swift in Sources */,
			);
		};
	};
}
PBX

run_guard() {
  VELVT_PBXPROJ_SOURCES="$work/Sources" VELVT_PBXPROJ_FILE="$pbx" "$guard" "$@"
}

# 1. A fully registered tree passes.
run_guard > "$work/clean.txt" 2>&1 || {
  cat "$work/clean.txt" >&2
  fail "a fully registered tree was rejected"
}
grep -qF "2 Swift file(s), all with >= 4 references" "$work/clean.txt" \
  || fail "the pass message did not report the file count"

# 2. A file with zero references fails, and is named.
touch "$work/Sources/Orphan.swift"
if run_guard > "$work/orphan.txt" 2>&1; then
  cat "$work/orphan.txt" >&2
  fail "an unregistered Swift file passed the guard"
fi
grep -qF "Orphan.swift" "$work/orphan.txt" || fail "the guard did not name the offending file"
grep -qF "0/4 references" "$work/orphan.txt" || fail "the guard did not report the shortfall"
rm "$work/Sources/Orphan.swift"

# 3. A file with three of four references fails. This is the realistic
#    half-registered case — Xcode dropped it from the Sources build phase and
#    nothing else notices.
touch "$work/Sources/HalfRegistered.swift"
cat >> "$pbx" <<'PBX'
// EEEE1 /* HalfRegistered.swift */ = {isa = PBXFileReference; };
// EEEE2 /* HalfRegistered.swift */,
// EEEE3 /* HalfRegistered.swift */,
PBX
if run_guard > "$work/half.txt" 2>&1; then
  cat "$work/half.txt" >&2
  fail "a file with 3 of 4 references passed"
fi
grep -qF "3/4 references" "$work/half.txt" || fail "the guard miscounted a 3-reference file"
rm "$work/Sources/HalfRegistered.swift"

# 4. A shorter name must not be credited with a longer name's references.
#    Without an anchor, grep for "Registered.swift" would also match every
#    "AlsoRegistered.swift" line and the guard would pass a file that has none.
touch "$work/Sources/View.swift"
cat >> "$pbx" <<'PBX'
// FFFF1 /* MenuBarPopoverView.swift */ = {isa = PBXFileReference; };
// FFFF2 /* MenuBarPopoverView.swift */,
// FFFF3 /* MenuBarPopoverView.swift in Sources */,
// FFFF4 /* MenuBarPopoverView.swift */,
// FFFF5 /* MenuBarPopoverView.swift */,
PBX
if run_guard > "$work/substring.txt" 2>&1; then
  cat "$work/substring.txt" >&2
  fail "View.swift was credited with MenuBarPopoverView.swift's references"
fi
grep -qF "View.swift" "$work/substring.txt" || fail "the substring case named the wrong file"
rm "$work/Sources/View.swift"

# 5. Clean again, to prove the failures above were caused by what was added.
run_guard >/dev/null 2>&1 || fail "the tree did not return to passing"

# 6. A missing pbxproj is not a failure. The file is in .gitignore, so a clone
#    that only touches Rust must still be able to push.
if ! VELVT_PBXPROJ_SOURCES="$work/Sources" VELVT_PBXPROJ_FILE="$work/absent.pbxproj" \
     "$guard" > "$work/absent.txt" 2>&1; then
  cat "$work/absent.txt" >&2
  fail "a missing project.pbxproj blocked the guard instead of skipping it"
fi
grep -qF ".gitignore" "$work/absent.txt" || fail "the skip message does not explain itself"

# 7. And the real tree, which is the point of the guard existing.
"$guard" > "$work/real.txt" 2>&1 || {
  cat "$work/real.txt" >&2
  fail "the real swift-client tree is NOT clean"
}
cat "$work/real.txt"

echo "verify_pbxproj_membership_test.sh: OK"
