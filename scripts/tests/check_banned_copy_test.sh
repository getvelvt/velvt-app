#!/usr/bin/env bash
# The banned-copy guard has to fail on a real capability claim, not just pass
# on today's tree, and it has to ignore the comments and identifiers it is
# documented to ignore. Every negative case runs against a synthetic tree, so
# no throwaway source file is ever written into swift-client/ or rust-service/.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
guard="$repo_root/scripts/check_banned_copy.py"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$guard" ]] || { echo "ERROR: $guard is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# 1. The real tree is clean, with its real allowlist.
"$guard" > "$work/real.txt" 2>&1 || { cat "$work/real.txt" >&2; fail "the repository itself has a banned capability claim"; }

tree="$work/tree"
swift="$tree/swift-client/Sources/VelvtMac"
rust="$tree/rust-service/src"
mkdir -p "$swift" "$rust" "$tree/rust-service/shared-types/src"
echo '[]' > "$work/empty-allowlist.json"

run_guard() {
  local allowlist="${1:-$work/empty-allowlist.json}"
  "$repo_root/scripts/check_banned_copy.py" --root "$tree" --allowlist "$allowlist" > "$work/result.txt" 2>&1
}
expect_pass() {
  run_guard "$@" || { cat "$work/result.txt" >&2; fail "clean synthetic tree was rejected"; }
}
expect_fail_naming() {
  local needle="$1"; shift
  if run_guard "$@"; then
    cat "$work/result.txt" >&2
    fail "the guard passed but should have reported: $needle"
  fi
  grep -qF -- "$needle" "$work/result.txt" || { cat "$work/result.txt" >&2; fail "the report did not name: $needle"; }
}

# 2. What the guard must not see: comments, doc comments, nested block
#    comments, identifiers, interpolated code, and Rust test-only items.
cat > "$swift/Clean.swift" <<'SWIFT'
// "Velvt learns how your attention breaks" was the onboarding line once.
/* A block comment that says the model adapts. /* nested: it predicts */ */
/// Doc comment: nothing here gets smarter.
struct Clean {
    let learning = 0.0
    func resetClassificationLearning() {}
    var label: String { "Compared with \(learning) recent days" }
    var raw: String { #"A raw "quoted" string with \(nothing) interpolated"# }
    var nested: String { "Days: \(learning > 0 ? "some" : "none")" }
    var block: String {
        """
        Multi-line copy that only "counts" things.
        """
    }
}
SWIFT
cat > "$rust/clean.rs" <<'RUST'
//! Velvt learns nothing; this is a module doc comment.
/// The model adapts? No, this is a doc comment.
pub fn learning_rate<'a>(label: &'a str) -> &'a str {
    let quote = '"';
    let escaped = '\'';
    let _ = (quote, escaped, b'"');
    /* a block comment: it predicts */
    label
}
pub fn copy() -> &'static str {
    "Velvt compared this block with your last three."
}
#[cfg(test)]
mod tests {
    #[test]
    fn copy_never_claims_learning() {
        assert!(!super::copy().contains("learns"), "the copy says Velvt learns");
    }
}
#[cfg(any(test, feature = "test-helpers"))]
pub fn fixture() -> &'static str { "a fixture that gets smarter" }
RUST
expect_pass

# 3. A banned claim in an ordinary Swift literal is reported with its line.
cat > "$swift/Bad.swift" <<'SWIFT'
struct Bad {
    let title = "Velvt learns your rhythm"
}
SWIFT
expect_fail_naming 'swift-client/Sources/VelvtMac/Bad.swift:2: learn*'
rm "$swift/Bad.swift"

# 4. Swift literal forms: the text around an interpolation, a literal nested
#    in an interpolation, multi-line, and raw strings are all copy.
for literal in \
  '"It \(count) adapts to you"' \
  '"\(flag ? "It predicts drift" : "")"' \
  '#"Velvt gets smarter"#' \
  $'"""\n        Learning from your recent sessions\n        """'; do
  printf 'struct Forms {\n    let count = 1\n    let flag = true\n    var text: String {\n        %s\n    }\n}\n' "$literal" > "$swift/Forms.swift"
  expect_fail_naming 'swift-client/Sources/VelvtMac/Forms.swift:5:'
done
rm "$swift/Forms.swift"

# 5. A Rust literal in production code is reported, raw strings included,
#    and `cfg(not(test))` is production code.
printf 'pub fn a() -> &%sstatic str {\n    "Velvt adapts to your day."\n}\n' "'" > "$rust/bad.rs"
expect_fail_naming 'rust-service/src/bad.rs:2: adapt*'
printf 'pub fn b() -> &%sstatic str {\n    r#"The model "predicts" drift."#\n}\n' "'" > "$rust/bad.rs"
expect_fail_naming 'rust-service/src/bad.rs:2: predict*'
printf '#[cfg(not(test))]\npub fn c() -> &%sstatic str { "behavioral modeling takes over" }\n' "'" > "$rust/bad.rs"
expect_fail_naming 'rust-service/src/bad.rs:2: behavioral model'
rm "$rust/bad.rs"

# 6. An allowlist entry covers exactly one (path, literal) pair: the listed
#    literal passes, a different sentence in the same file still fails, and an
#    entry that matches nothing is reported as stale.
printf 'pub const TOKENS: &[&str] = &["learns"];\n' > "$rust/tokens.rs"
cat > "$work/allowlist.json" <<'JSON'
[{"path": "rust-service/src/tokens.rs", "literal": "learns", "why": "names the banned word in a registry"}]
JSON
expect_pass "$work/allowlist.json"
printf 'pub const TOKENS: &[&str] = &["learns"];\npub fn d() -> &%sstatic str { "Velvt learns." }\n' "'" > "$rust/tokens.rs"
expect_fail_naming 'rust-service/src/tokens.rs:2: learn*' "$work/allowlist.json"
printf 'pub const TOKENS: &[&str] = &[];\n' > "$rust/tokens.rs"
expect_fail_naming 'STALE   allowlist entry matches nothing: rust-service/src/tokens.rs: "learns"' "$work/allowlist.json"

# 7. An allowlist entry without a reason is refused outright.
echo '[{"path": "rust-service/src/tokens.rs", "literal": "learns", "why": " "}]' > "$work/no-reason.json"
if run_guard "$work/no-reason.json"; then fail "an allowlist entry with no reason was accepted"; fi
grep -qF "non-empty why" "$work/result.txt" || { cat "$work/result.txt" >&2; fail "the refusal did not say why"; }

echo "check_banned_copy_test.sh: all cases passed"
