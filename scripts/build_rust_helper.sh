#!/usr/bin/env bash
set -euo pipefail

rust_dir="${1:-}"
output_path="${2:-}"
distributable="${3:-NO}"
requested_archs="${4:-$(uname -m)}"

[[ -f "$rust_dir/Cargo.toml" ]] || { echo "ERROR: Rust service not found: $rust_dir" >&2; exit 1; }
[[ -n "$output_path" ]] || { echo "ERROR: helper output path is required." >&2; exit 1; }

case "$distributable" in YES|NO) ;; *) echo "ERROR: distributable must be YES or NO." >&2; exit 2 ;; esac

if [[ "$distributable" == "YES" ]]; then
  for required_arch in arm64 x86_64; do
    [[ " $requested_archs " == *" $required_arch "* ]] || {
      echo "ERROR: distributable helper requires arm64 and x86_64; ARCHS='$requested_archs'." >&2
      exit 1
    }
  done
fi

mkdir -p "$(dirname "$output_path")"
cd "$rust_dir"

if [[ "$distributable" == "NO" ]]; then
  if [[ "$(uname -m)" == "arm64" ]]; then
    cargo build --release --features onnx
  else
    cargo build --release
  fi
  cp target/release/velvt-service "$output_path"
  for artifact in abstraction-model.onnx tokenizer.json abstraction-prototypes.bin; do
    [[ ! -f "resources/$artifact" ]] || cp "resources/$artifact" "$(dirname "$output_path")/$artifact"
  done
  exit 0
fi

# A shipped helper must not carry the build machine's paths. Without this, the
# 1.0.11 helper held 867 strings naming the builder's home directory and
# checkout (panic locations and debug info from every crate). Later flags win
# when several prefixes match, so the most specific directory comes last.
# CARGO_ENCODED_RUSTFLAGS rather than RUSTFLAGS so a path with a space survives;
# flags the caller already set in either variable are carried over, since the
# encoded form replaces both.
remap_rustflags() {
  local flags=()
  local flag
  if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
    IFS=$'\x1f' read -r -a flags <<<"$CARGO_ENCODED_RUSTFLAGS"
  else
    for flag in ${RUSTFLAGS:-}; do flags+=("$flag"); done
  fi
  flags+=("--remap-path-prefix=$HOME=/build-home")
  flags+=("--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo")
  flags+=("--remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup")
  if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    flags+=("--remap-path-prefix=$(cd "$CARGO_TARGET_DIR" 2>/dev/null && pwd -P || echo "$CARGO_TARGET_DIR")=/velvt/target")
  fi
  flags+=("--remap-path-prefix=$(pwd -P)=/velvt/rust-service")
  local IFS=$'\x1f'
  printf '%s' "${flags[*]}"
}
export CARGO_ENCODED_RUSTFLAGS="$(remap_rustflags)"

for arch in arm64 x86_64; do
  case "$arch" in
    arm64) target="aarch64-apple-darwin" ;;
    x86_64) target="x86_64-apple-darwin" ;;
  esac
  rustup target list --installed | grep -Fxq "$target" || {
    echo "ERROR: missing Rust target '$target'; run: rustup target add $target" >&2
    exit 1
  }
  cargo build --release --target "$target"
done

lipo -create \
  target/aarch64-apple-darwin/release/velvt-service \
  target/x86_64-apple-darwin/release/velvt-service \
  -output "$output_path"

for artifact in abstraction-model.onnx tokenizer.json abstraction-prototypes.bin; do
  [[ ! -f "resources/$artifact" ]] || cp "resources/$artifact" "$(dirname "$output_path")/$artifact"
done

actual_archs="$(lipo -archs "$output_path")"
for required_arch in arm64 x86_64; do
  [[ " $actual_archs " == *" $required_arch "* ]] || {
    echo "ERROR: universal helper is missing '$required_arch' (has: $actual_archs)." >&2
    exit 1
  }
done
