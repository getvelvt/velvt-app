#!/usr/bin/env bash
# The Swift lint gate behind `make lint-swift`.
#
# `swift format lint` exits 0 on warnings, so running it bare enforced nothing:
# with no `.swift-format` it linted against its 2-space defaults, printed about
# 27,000 warnings on a codebase written mostly in 4 spaces, and CI stayed green.
# `swift-client/.swift-format` now states the house style, but about 8,000
# findings remain, most of them in files still indented with 2 spaces.
# Reformatting those at once would conflict with every open Swift branch, so it
# is a change of its own.
#
# Until then this is a ratchet. `swift-client/.swift-format-baseline` lists the
# (file, rule) pairs that had findings when the gate was introduced. A finding
# is tolerated only when its file and its rule are listed together. Everything
# else fails, so:
#
# - a rule a file is clean on stays clean in that file;
# - a file that is clean on every rule stays clean on every rule;
# - a new file is held to every rule.
#
# Baseline pairs that no longer match anything are reported, not fatal, so
# fixing old findings never turns someone else's pull request red. Delete them
# when you see them. Never add a pair to get a change through.
#
# Overrides, for scripts/tests/lint_swift_test.sh:
#   VELVT_SWIFT_PACKAGE_DIR  the Swift package to lint (default: swift-client)
#   VELVT_SWIFT_BIN          the swift driver (default: swift)

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
package="$(cd "${VELVT_SWIFT_PACKAGE_DIR:-$repo_root/swift-client}" && pwd -P)"
swift_bin="${VELVT_SWIFT_BIN:-swift}"
baseline="$package/.swift-format-baseline"

[[ -f "$package/.swift-format" ]] || {
  echo "ERROR: $package/.swift-format is missing; swift format would lint against its 2-space defaults." >&2
  exit 1
}
[[ -f "$baseline" ]] || { echo "ERROR: $baseline is missing." >&2; exit 1; }

output="$(mktemp)"
trap 'rm -f "$output"' EXIT

# Not --strict: sorting warnings is this script's job. A non-zero exit is
# swift format itself failing -- an unparsable file or a bad configuration --
# and that is always fatal.
if ! (cd "$package" && "$swift_bin" format lint --recursive Sources Tests) >"$output" 2>&1; then
  cat "$output" >&2
  echo "ERROR: swift format lint did not complete." >&2
  exit 1
fi

# Lint-rule findings name their file by absolute path, pretty-printer findings
# by a path relative to the package. Both are compared relative to it.
awk -v package="$package/" '
  FNR == NR {
    if (NF == 2 && $1 !~ /^#/) { allowed[$1 " " $2] = 1; hits[$1 " " $2] = 0 }
    next
  }
  /: error: / { print > "/dev/stderr"; failed++; next }
  /: warning: \[[A-Za-z]+\]/ {
    line = $0
    if (index(line, package) == 1) line = substr(line, length(package) + 1)
    file = substr(line, 1, index(line, ":") - 1)
    match(line, /\[[A-Za-z]+\]/)
    key = file " " substr(line, RSTART + 1, RLENGTH - 2)
    if (key in allowed) {
      tolerated++
      hits[key]++
    } else {
      print line > "/dev/stderr"
      failed++
    }
  }
  END {
    for (key in hits) if (hits[key] == 0) stale++
    if (stale > 0) {
      printf "note: %d baseline pair(s) no longer have findings; delete them from .swift-format-baseline:\n", stale
      for (key in hits) if (hits[key] == 0) printf "  %s\n", key
    }
    printf "swift format: %d finding(s) tolerated by the baseline, %d not\n", tolerated, failed
    if (failed > 0) {
      print "ERROR: the findings above are not in swift-client/.swift-format-baseline." > "/dev/stderr"
      print "Fix them rather than adding to the baseline; swift format format --in-place <file> does most of it." > "/dev/stderr"
      exit 1
    }
  }
' "$baseline" "$output"
