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
