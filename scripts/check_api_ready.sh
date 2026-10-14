#!/usr/bin/env bash
# Refuses unless the API a build is about to be compiled against answers
# GET /v1/ready with 200. `make alpha-dmg` and `make release` run it before they
# build anything.
#
# The URL is compiled into the helper, and the app removes any
# VELVT_API_BASE_URL override before it starts the helper, so nobody who
# installs the build can point it anywhere else. preflight_distribution.sh
# checks only the URL's shape: in October 2026 it passed
# https://dev-api.getvelvt.com while that host answered Cloudflare 522, and
# every sign-up from a build cut then would have failed after ten seconds with
# "Check your connection".
#
# VELVT_ALLOW_UNREACHABLE_API=1 skips the request, for an offline test of the
# build pipeline only. The probe is one unauthenticated GET from the build
# machine; it carries nothing from anyone's Mac.
set -euo pipefail

api_url="${1:-}"
script_dir="$(cd "$(dirname "$0")" && pwd -P)"

# Only a URL a distributable build may carry is ever requested. This prints
# its own error.
"$script_dir/preflight_distribution.sh" "$api_url" >/dev/null

ready_url="${api_url%/}/v1/ready"

if [[ "${VELVT_ALLOW_UNREACHABLE_API:-0}" == "1" ]]; then
  echo "WARNING: VELVT_ALLOW_UNREACHABLE_API=1, so $ready_url was not checked." >&2
  echo "WARNING: this is for offline testing only. Before this build goes to anyone, check that the URL answers 200." >&2
  exit 0
fi

set +e
status="$(curl -fsS --proto '=https' --connect-timeout 10 --max-time 30 \
  -o /dev/null -w '%{http_code}' "$ready_url")"
curl_exit=$?
set -e

if [[ $curl_exit -ne 0 || "$status" != "200" ]]; then
  if [[ -z "$status" || "$status" == "000" ]]; then
    echo "ERROR: $ready_url did not answer (curl exit $curl_exit)." >&2
  else
    echo "ERROR: $ready_url answered HTTP $status, not 200." >&2
  fi
  echo "This build would compile that URL into the helper, and no one who installs it can change it," >&2
  echo "so every sign-in and upload from it would fail. Restore the backend, or build against one that answers." >&2
  echo "For an offline test of the build pipeline only, VELVT_ALLOW_UNREACHABLE_API=1 skips this check." >&2
  exit 1
fi

echo "API readiness: $ready_url answered 200"
