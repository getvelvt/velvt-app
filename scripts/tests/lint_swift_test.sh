#!/usr/bin/env bash
# The Swift lint gate has to fail on a new finding, not just pass on today's
# tree. Before it existed, `make lint-swift` printed 26,958 warnings and exited
# 0, which is what an untested lint gate looks like.
#
# Runs the gate against a synthetic package and a fake `swift` that replays
# canned swift-format output, so it needs no toolchain and never touches the
# real swift-client/.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
gate="$repo_root/scripts/lint_swift.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$gate" ]] || { echo "ERROR: $gate is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

package="$work/package"
mkdir -p "$package/Sources" "$package/Tests"
package="$(cd "$package" && pwd -P)"
echo '{ "version": 1, "indentation": { "spaces": 4 } }' > "$package/.swift-format"
cat > "$package/.swift-format-baseline" <<'BASELINE'
# comment lines and blank lines are ignored

Sources/Old.swift Indentation
Sources/Old.swift DoNotUseSemicolons
Tests/Fixed.swift LineLength
BASELINE

# The fake driver prints whatever is in $work/lint.out and exits with the
# status in $work/lint.status, the way `swift format lint` reports on stderr.
cat > "$work/swift" <<'SWIFT'
#!/usr/bin/env bash
[[ "$1 $2" == "format lint" ]] || { echo "unexpected: $*" >&2; exit 64; }
cat "$(dirname "$0")/lint.out" >&2
exit "$(cat "$(dirname "$0")/lint.status")"
SWIFT
chmod +x "$work/swift"

run_gate() {
  VELVT_SWIFT_PACKAGE_DIR="$package" VELVT_SWIFT_BIN="$work/swift" "$gate" > "$work/result.txt" 2>&1
}
lint_output() { printf '%s\n' "$@" > "$work/lint.out"; echo "${status:-0}" > "$work/lint.status"; }

# 1. Findings whose (file, rule) pair is in the baseline pass, whether swift
#    format names the file relative to the package (pretty-printer findings)
#    or by absolute path (lint-rule findings).
lint_output \
  "Sources/Old.swift:3:1: warning: [Indentation] indent by 2 spaces" \
  "$package/Sources/Old.swift:9:12: warning: [DoNotUseSemicolons] remove ';' and move the next statement to a new line" \
  "$package/Sources/Old.swift:9:12: note: a note line that belongs to the warning above"
run_gate || { cat "$work/result.txt" >&2; fail "baselined findings were rejected"; }
grep -qF "2 finding(s) tolerated by the baseline, 0 not" "$work/result.txt" \
  || { cat "$work/result.txt" >&2; fail "the summary did not count the tolerated findings"; }

# 2. A baselined pair with nothing left to tolerate is reported, not fatal.
grep -qF "Tests/Fixed.swift LineLength" "$work/result.txt" \
  || { cat "$work/result.txt" >&2; fail "a stale baseline pair was not reported"; }

# 3. A new rule in a baselined file fails, and the finding is printed.
lint_output \
  "Sources/Old.swift:3:1: warning: [Indentation] indent by 2 spaces" \
  "$package/Sources/Old.swift:5:1: warning: [NeverForceUnwrap] do not force unwrap"
if run_gate; then cat "$work/result.txt" >&2; fail "a new rule in a baselined file passed"; fi
grep -qF "Sources/Old.swift:5:1: warning: [NeverForceUnwrap]" "$work/result.txt" \
  || { cat "$work/result.txt" >&2; fail "the gate did not print the new finding"; }

# 4. A baselined rule in a file that is clean on it fails.
lint_output "Sources/New.swift:1:1: warning: [Indentation] indent by 2 spaces"
if run_gate; then cat "$work/result.txt" >&2; fail "a baselined rule passed in a file not baselined for it"; fi

# 5. swift format failing outright is fatal, even with no warnings.
status=1 lint_output "Sources/Broken.swift: error: file contains invalid Swift syntax"
if run_gate; then cat "$work/result.txt" >&2; fail "a swift format failure passed"; fi

# 6. An error line is fatal even when swift format exits 0.
lint_output "Sources/Broken.swift:1:1: error: something went wrong"
if run_gate; then cat "$work/result.txt" >&2; fail "an error finding passed"; fi

# 7. Without a .swift-format the gate refuses to run, rather than linting
#    against swift format's 2-space defaults.
lint_output
rm "$package/.swift-format"
if run_gate; then cat "$work/result.txt" >&2; fail "the gate ran without a .swift-format"; fi
grep -qF ".swift-format is missing" "$work/result.txt" || fail "the missing configuration was not named"

echo "lint_swift_test: all checks passed"
