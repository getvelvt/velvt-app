#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd -P)"
tool_root="${VELVT_DMGBUILD_TOOL_ROOT:-$repo_root/.build-tools/dmgbuild}"
python_bin="${VELVT_DMGBUILD_PYTHON:-python3}"
requirements="$repo_root/scripts/dmgbuild-requirements.txt"

if [[ -x "$tool_root/bin/dmgbuild" ]] && \
  "$tool_root/bin/python" -c 'import importlib.metadata; assert importlib.metadata.version("dmgbuild") == "1.6.2"' 2>/dev/null && \
  "$tool_root/bin/dmgbuild" --help >/dev/null 2>&1; then
  echo "Pinned dmgbuild 1.6.2 already prepared at $tool_root"
  exit 0
fi

mkdir -p "$(dirname "$tool_root")"
rm -rf "$tool_root"
"$python_bin" -m venv "$tool_root"
"$tool_root/bin/python" -m pip install \
  --disable-pip-version-check \
  --only-binary=:all: \
  --require-hashes \
  -r "$requirements"
echo "Prepared pinned dmgbuild 1.6.2 at $tool_root"
