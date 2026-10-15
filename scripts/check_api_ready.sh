#!/usr/bin/env bash
# Refuses unless the API a build is about to be compiled against answers
# GET /v1/ready with 200 and {"status": "ready"}, the same answer the helper
# itself treats as ready (rust-service/src/ipc/router.rs). `make alpha-dmg` and
# `make release` run it before they build anything.
#
# The URL is compiled into the helper, and the app removes any
# VELVT_API_BASE_URL override before it starts the helper, so nobody who
# installs the build can point it anywhere else. preflight_distribution.sh
# checks only the URL's shape: in October 2026 it passed
# https://dev-api.getvelvt.com while that host answered Cloudflare 522, and
# every sign-up from a build cut then would have failed after ten seconds with
# "Check your connection". The body is checked too, because a 200 from
# something that is not velvt-core (a misrouted tunnel, a parked domain, a
# catch-all page) would pass a status check alone.
#
# VELVT_ALLOW_UNREACHABLE_API=1 skips the request, for a pipeline test while
# the API is down only. The probe is one unauthenticated GET from the build
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
  echo "WARNING: this is for a pipeline test while the API is down only. Before this build goes to anyone, check that the URL answers 200 and {\"status\": \"ready\"}." >&2
  exit 0
fi

refuse() { # <reason>
  echo "ERROR: $1" >&2
  echo "This build would compile that URL into the helper, and no one who installs it can change it," >&2
  echo "so every sign-in and upload from it would fail. Restore the backend, or build against one that answers." >&2
  echo "For a pipeline test while the API is down only, VELVT_ALLOW_UNREACHABLE_API=1 skips this check." >&2
  exit 1
}

body_file="$(mktemp "${TMPDIR:-/tmp}/velvt-ready.XXXXXX")"
trap 'rm -f "$body_file"' EXIT

set +e
status="$(curl -fsS --proto '=https' --connect-timeout 10 --max-time 30 \
  -o "$body_file" -w '%{http_code}' "$ready_url")"
curl_exit=$?
set -e

if [[ -z "$status" || "$status" == "000" ]]; then
  refuse "$ready_url did not answer (curl exit $curl_exit)."
elif [[ "$status" != "200" ]]; then
  refuse "$ready_url answered HTTP $status, not 200."
elif [[ $curl_exit -ne 0 ]]; then
  refuse "$ready_url answered 200, but the transfer failed (curl exit $curl_exit)."
fi

# Exit 0 for ready, 3 for anything else; any other status means python3 itself
# could not run. -I keeps PYTHON* variables and the user's site-packages out.
set +e
/usr/bin/python3 -I -c '
import json, sys
try:
    with open(sys.argv[1], "rb") as body_file:
        body = json.load(body_file)
except (OSError, ValueError):
    sys.exit(3)
sys.exit(0 if isinstance(body, dict) and body.get("status") == "ready" else 3)
' "$body_file" 2>/dev/null
body_check=$?
set -e
if [[ $body_check -eq 3 ]]; then
  refuse "$ready_url answered 200, but not with {\"status\": \"ready\"}, so it is not a Velvt API that is ready."
elif [[ $body_check -ne 0 ]]; then
  echo "ERROR: /usr/bin/python3 could not run (exit $body_check) to read what $ready_url answered." >&2
  echo "Install the Xcode command line tools (xcode-select --install) and run the target again." >&2
  exit 1
fi

echo "API readiness: $ready_url answered 200, ready"
