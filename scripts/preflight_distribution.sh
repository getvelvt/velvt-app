#!/usr/bin/env bash
set -euo pipefail

api_url="${1:-}"

if [[ -z "$api_url" ]]; then
  echo "ERROR: distributable builds require VELVT_API_BASE_URL." >&2
  exit 1
fi

if [[ "$api_url" != https://* ]]; then
  echo "ERROR: distributable builds require an https API URL; got '$api_url'." >&2
  exit 1
fi

case "$api_url" in
  https://localhost|https://localhost/*|https://localhost:*|\
  https://127.*|https://0.0.0.0|https://0.0.0.0/*|https://0.0.0.0:*|\
  https://\[::1\]|https://\[::1\]/*|https://\[::1\]:*)
    echo "ERROR: distributable builds cannot use a localhost API URL ('$api_url')." >&2
    exit 1
    ;;
esac

echo "Distribution preflight: hosted API URL accepted ($api_url)"
