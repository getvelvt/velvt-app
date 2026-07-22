#!/usr/bin/env bash
set -euo pipefail

api_url="${1:-}"
mode="${2:-distribution}"
approved_production_host="${3:-}"

[[ "$mode" == "distribution" || "$mode" == "production" ]] || {
  echo "ERROR: preflight mode must be distribution or production." >&2
  exit 2
}

if [[ -z "$api_url" ]]; then
  echo "ERROR: distributable builds require VELVT_API_BASE_URL." >&2
  exit 1
fi

if [[ "$api_url" != https://* ]]; then
  echo "ERROR: distributable builds require an https API URL; got '$api_url'." >&2
  exit 1
fi

location="${api_url#https://}"
authority="${location%%/*}"
host="${authority%%:*}"
case "$location" in
  *\?*|*\#*|*@*)
    echo "ERROR: API URL cannot contain credentials, a query, or a fragment." >&2
    exit 1
    ;;
esac
case "$authority" in
  *:443) host="${authority%:443}" ;;
  *:*)
    echo "ERROR: API URL must use the default HTTPS port." >&2
    exit 1
    ;;
esac
[[ -n "$host" ]] || { echo "ERROR: API URL hostname is empty." >&2; exit 1; }

case "$api_url" in
  https://localhost|https://localhost/*|https://localhost:*|\
  https://127.*|https://0.0.0.0|https://0.0.0.0/*|https://0.0.0.0:*|\
  https://10.*|https://192.168.*|https://169.254.*|\
  https://172.1[6-9].*|https://172.2[0-9].*|https://172.3[01].*|\
  https://\[::1\]|https://\[::1\]/*|https://\[::1\]:*)
    echo "ERROR: distributable builds cannot use a local or private API URL ('$api_url')." >&2
    exit 1
    ;;
esac

if [[ "$mode" == "production" ]]; then
  [[ -n "$approved_production_host" ]] || {
    echo "ERROR: production preflight requires an approved exact API hostname." >&2
    exit 1
  }
  case "$host" in
    dev|dev.*|dev-*|*.dev|*.dev.*|staging|staging.*|staging-*|*.staging|*.staging.*|\
    test|test.*|test-*|*.test|*.test.*|*.invalid)
      echo "ERROR: production releases cannot use a development, staging, test, or invalid API host ('$host')." >&2
      exit 1
      ;;
  esac
  if [[ "$host" != "$approved_production_host" ]]; then
    echo "ERROR: production API host '$host' does not match approved host '$approved_production_host'." >&2
    exit 1
  fi
fi

echo "Distribution preflight: $mode API URL accepted ($api_url)"
