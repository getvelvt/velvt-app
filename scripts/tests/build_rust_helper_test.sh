#!/usr/bin/env bash
# build_rust_helper.sh is the Xcode "Bundle Rust Service" phase. It copies the
# helper cargo just built into the app. The defect this pins: it copied from a
# hard-coded rust-service/target, so with CARGO_TARGET_DIR set the build either
# failed (no ./target) or bundled whatever older helper ./target still held, and
# a helper left in the app by an earlier build survived a failed one.
#
# cargo and rustup are shims, so this needs no Rust toolchain. The universal
# (YES) path needs clang and lipo, and is skipped without them.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script="$repo_root/scripts/build_rust_helper.sh"
# Each case sets its own. One inherited from the caller's shell would send the
# shim's fake helpers into a real target directory.
unset CARGO_TARGET_DIR
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# --- Shims -------------------------------------------------------------------

# A cargo that, like the real one, writes where CARGO_TARGET_DIR says (relative
# to the directory it runs in), and otherwise into ./target. It "builds" by
# copying $FIXTURE_FRESH_<triple> (or $FIXTURE_FRESH for the host build), and
# fails when FIXTURE_CARGO_FAILS=1.
shim_bin="$fixture_dir/bin"
mkdir -p "$shim_bin"
cat >"$shim_bin/cargo" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
[[ "${FIXTURE_CARGO_FAILS:-0}" != "1" ]] || { echo "error: could not compile velvt-service" >&2; exit 101; }
target_dir="${CARGO_TARGET_DIR:-target}"
triple=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) triple="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [[ -z "$triple" ]]; then
  mkdir -p "$target_dir/release"
  cp "$FIXTURE_FRESH" "$target_dir/release/velvt-service"
else
  source_var="FIXTURE_FRESH_${triple//-/_}"
  mkdir -p "$target_dir/$triple/release"
  cp "${!source_var}" "$target_dir/$triple/release/velvt-service"
fi
SHIM
cat >"$shim_bin/rustup" <<'SHIM'
#!/usr/bin/env bash
printf '%s\n' aarch64-apple-darwin x86_64-apple-darwin
SHIM
chmod +x "$shim_bin/cargo" "$shim_bin/rustup"

new_rust_dir() {
  local dir="$fixture_dir/$1"
  mkdir -p "$dir/resources"
  touch "$dir/Cargo.toml"
  printf '%s' "$dir"
}

run_script() {
  PATH="$shim_bin:$PATH" bash "$script" "$@"
}

printf '#!/bin/sh\necho FRESH\n' >"$fixture_dir/fresh-helper"
chmod +x "$fixture_dir/fresh-helper"
export FIXTURE_FRESH="$fixture_dir/fresh-helper"

# --- Debug (NO): CARGO_TARGET_DIR is where the helper comes from --------------

rust_dir="$(new_rust_dir stale-target)"
mkdir -p "$rust_dir/target/release"
printf '#!/bin/sh\necho STALE\n' >"$rust_dir/target/release/velvt-service"
chmod +x "$rust_dir/target/release/velvt-service"
out="$fixture_dir/stale-target-out/velvt-service"
CARGO_TARGET_DIR="$fixture_dir/shared-target" run_script "$rust_dir" "$out" NO >/dev/null ||
  fail "NO mode failed with CARGO_TARGET_DIR set."
[[ "$("$out")" == "FRESH" ]] ||
  fail "NO mode bundled the helper in ./target, not the one cargo built in CARGO_TARGET_DIR."

rust_dir="$(new_rust_dir no-local-target)"
out="$fixture_dir/no-local-target-out/velvt-service"
CARGO_TARGET_DIR="$fixture_dir/shared-target-2" run_script "$rust_dir" "$out" NO >/dev/null 2>&1 ||
  fail "NO mode failed with CARGO_TARGET_DIR set and no ./target."
[[ "$("$out")" == "FRESH" ]] || fail "NO mode did not bundle the helper cargo built."

# A relative CARGO_TARGET_DIR is relative to the crate, where cargo runs.
rust_dir="$(new_rust_dir relative-target)"
out="$fixture_dir/relative-target-out/velvt-service"
CARGO_TARGET_DIR=build-elsewhere run_script "$rust_dir" "$out" NO >/dev/null ||
  fail "NO mode failed with a relative CARGO_TARGET_DIR."
[[ "$("$out")" == "FRESH" && -x "$rust_dir/build-elsewhere/release/velvt-service" ]] ||
  fail "NO mode did not resolve a relative CARGO_TARGET_DIR from the crate."

# Unset, it is ./target, as before.
rust_dir="$(new_rust_dir default-target)"
out="$fixture_dir/default-target-out/velvt-service"
(unset CARGO_TARGET_DIR; run_script "$rust_dir" "$out" NO >/dev/null) ||
  fail "NO mode failed without CARGO_TARGET_DIR."
[[ "$("$out")" == "FRESH" ]] || fail "NO mode did not bundle ./target/release/velvt-service."

# --- A failed build leaves nothing behind for Xcode to bundle ------------------

rust_dir="$(new_rust_dir failed-build)"
out_dir="$fixture_dir/failed-build-out"
mkdir -p "$out_dir"
printf '#!/bin/sh\necho STALE\n' >"$out_dir/velvt-service"
chmod +x "$out_dir/velvt-service"
echo stale-tokenizer >"$out_dir/tokenizer.json"
if FIXTURE_CARGO_FAILS=1 run_script "$rust_dir" "$out_dir/velvt-service" NO >/dev/null 2>&1; then
  fail "a failed cargo build was reported as success."
fi
[[ ! -e "$out_dir/velvt-service" ]] || fail "a failed build left the previous helper in place."
[[ ! -e "$out_dir/tokenizer.json" ]] || fail "a failed build left a previous model artifact in place."

# A model artifact the crate no longer has does not ride along from a previous
# build; one it has is copied.
rust_dir="$(new_rust_dir artifacts)"
echo current-prototypes >"$rust_dir/resources/abstraction-prototypes.bin"
out_dir="$fixture_dir/artifacts-out"
mkdir -p "$out_dir"
echo stale-model >"$out_dir/abstraction-model.onnx"
run_script "$rust_dir" "$out_dir/velvt-service" NO >/dev/null || fail "NO mode failed with a model artifact."
[[ ! -e "$out_dir/abstraction-model.onnx" ]] || fail "a model artifact from a previous build survived."
[[ "$(cat "$out_dir/abstraction-prototypes.bin")" == "current-prototypes" ]] ||
  fail "the crate's model artifact was not copied."

# --- Distributable (YES): both slices come from CARGO_TARGET_DIR --------------

if ! xcrun --find clang >/dev/null 2>&1 || ! command -v lipo >/dev/null 2>&1; then
  echo "SKIP: no clang or lipo, so the universal-helper cases did not run"
  echo "build_rust_helper tests passed"
  exit 0
fi

stub_source="$fixture_dir/stub.c"
printf '#include <stdio.h>\nint main(void) { puts(TAG); return 0; }\n' >"$stub_source"
compile_stub() { # <arch> <tag> <output>
  xcrun clang -arch "$1" -mmacosx-version-min=11.0 -DTAG="\"$2\"" -o "$3" "$stub_source"
}
compile_stub arm64 FRESH-ARM64-SLICE "$fixture_dir/fresh-arm64"
compile_stub x86_64 FRESH-X86-SLICE "$fixture_dir/fresh-x86_64"
export FIXTURE_FRESH_aarch64_apple_darwin="$fixture_dir/fresh-arm64"
export FIXTURE_FRESH_x86_64_apple_darwin="$fixture_dir/fresh-x86_64"

rust_dir="$(new_rust_dir universal)"
mkdir -p "$rust_dir/target/aarch64-apple-darwin/release" "$rust_dir/target/x86_64-apple-darwin/release"
compile_stub arm64 STALE-ARM64-SLICE "$rust_dir/target/aarch64-apple-darwin/release/velvt-service"
compile_stub x86_64 STALE-X86-SLICE "$rust_dir/target/x86_64-apple-darwin/release/velvt-service"
out="$fixture_dir/universal-out/velvt-service"
CARGO_TARGET_DIR="$fixture_dir/shared-target-universal" run_script "$rust_dir" "$out" YES "arm64 x86_64" >/dev/null ||
  fail "YES mode failed with CARGO_TARGET_DIR set."
for slice in arm64 x86_64; do
  lipo -thin "$slice" -output "$fixture_dir/slice-$slice" "$out"
done
if ! grep -q FRESH-ARM64-SLICE "$fixture_dir/slice-arm64" || ! grep -q FRESH-X86-SLICE "$fixture_dir/slice-x86_64"; then
  fail "YES mode lipo'd the slices in ./target, not the ones cargo built in CARGO_TARGET_DIR."
fi
if grep -q STALE "$out"; then
  fail "YES mode bundled a stale slice."
fi

echo "build_rust_helper tests passed"
