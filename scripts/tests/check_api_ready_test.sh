#!/usr/bin/env bash
# The release targets refuse to build against an API that does not answer
# /v1/ready with 200 and {"status": "ready"}. The defect this pins:
# preflight_distribution.sh checked
# only the URL's shape, so `make alpha-dmg` accepted dev-api.getvelvt.com while
# it answered Cloudflare 522, and the URL is compiled into the helper where no
# tester can change it.
#
# curl is a shim for every case but one, so this needs no network. The one real
# request is to a .invalid host, which never resolves.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
check="$repo_root/scripts/check_api_ready.sh"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
unset VELVT_ALLOW_UNREACHABLE_API

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[[ -x "$check" ]] || fail "scripts/check_api_ready.sh is missing or not executable."

# A curl that answers FIXTURE_HTTP_CODE with the body FIXTURE_BODY (written
# where -o says), exits FIXTURE_CURL_EXIT, and logs the URL it was asked for.
shim_bin="$fixture_dir/bin"
mkdir -p "$shim_bin"
cat >"$shim_bin/curl" <<'SHIM'
#!/usr/bin/env bash
output=/dev/null
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) output="$2"; shift 2 ;;
    https://*|http://*) echo "$1" >>"$FIXTURE_CURL_LOG"; shift ;;
    *) shift ;;
  esac
done
printf '%s' "$FIXTURE_BODY" >"$output"
printf '%s' "$FIXTURE_HTTP_CODE"
exit "$FIXTURE_CURL_EXIT"
SHIM
chmod +x "$shim_bin/curl"
export FIXTURE_CURL_LOG="$fixture_dir/curl.log"
# What velvt-core's /v1/ready returns when its database answers.
ready_body='{"status":"ready","database":"ok"}'
# The script's temporary body file goes here, so the test can see it removed.
export TMPDIR="$fixture_dir/tmp"
mkdir -p "$TMPDIR"

with_curl() { # <http code> <curl exit> <command...>, body FIXTURE_BODY or ready
  local code="$1" exit_status="$2"
  shift 2
  FIXTURE_BODY="${FIXTURE_BODY-$ready_body}" FIXTURE_HTTP_CODE="$code" FIXTURE_CURL_EXIT="$exit_status" \
    PATH="$shim_bin:$PATH" "$@"
}

# --- 200 and ready passes, and the request is GET <base>/v1/ready ---------------

: >"$FIXTURE_CURL_LOG"
output="$(with_curl 200 0 "$check" https://api.example.com 2>&1)" || fail "a ready 200 was refused: $output"
grep -q 'answered 200, ready' <<<"$output" || fail "a pass does not say what answered: $output"
[[ -z "$(ls -A "$TMPDIR")" ]] || fail "the response body was left in $TMPDIR: $(ls -A "$TMPDIR")"
[[ "$(cat "$FIXTURE_CURL_LOG")" == "https://api.example.com/v1/ready" ]] ||
  fail "requested $(cat "$FIXTURE_CURL_LOG"), not https://api.example.com/v1/ready"

: >"$FIXTURE_CURL_LOG"
with_curl 200 0 "$check" https://api.example.com/ >/dev/null 2>&1 || fail "a base URL with a trailing slash was refused."
[[ "$(cat "$FIXTURE_CURL_LOG")" == "https://api.example.com/v1/ready" ]] ||
  fail "a trailing slash produced $(cat "$FIXTURE_CURL_LOG")"

# --- Anything else is refused, with the reason and the override named -----------

expect_refusal() { # <description> <http code> <curl exit> <message fragment>, body FIXTURE_BODY
  local output
  if output="$(with_curl "$2" "$3" "$check" https://dev-api.example.com 2>&1)"; then
    fail "$1 was accepted."
  fi
  grep -qF "$4" <<<"$output" || fail "$1: the error does not say '$4': $output"
  grep -q 'VELVT_ALLOW_UNREACHABLE_API=1' <<<"$output" || fail "$1: the error does not name the override."
  [[ -z "$(ls -A "$TMPDIR")" ]] || fail "$1: the response body was left in $TMPDIR."
}

expect_refusal "Cloudflare 522" 522 22 'answered HTTP 522, not 200'
expect_refusal "a 503" 503 22 'answered HTTP 503, not 200'
expect_refusal "a redirect" 301 0 'answered HTTP 301, not 200'
expect_refusal "a host that does not answer" 000 7 'did not answer (curl exit 7)'
expect_refusal "a timeout" 000 28 'did not answer (curl exit 28)'
expect_refusal "a 200 whose body was cut off" 200 28 'answered 200, but the transfer failed (curl exit 28)'

# A 200 from something that is not a ready velvt-core: the helper itself would
# treat it as not ready, so the build must not be compiled against it.
not_ready='answered 200, but not with {"status": "ready"}'
FIXTURE_BODY='<html><body>Domain parked</body></html>' expect_refusal "a catch-all page" 200 0 "$not_ready"
FIXTURE_BODY='{"status":"ok"}' expect_refusal "another service's health check" 200 0 "$not_ready"
FIXTURE_BODY='["ready"]' expect_refusal "a JSON body that is not an object" 200 0 "$not_ready"
FIXTURE_BODY='' expect_refusal "an empty body" 200 0 "$not_ready"

# --- Only a URL a distributable build may carry is requested --------------------

: >"$FIXTURE_CURL_LOG"
if with_curl 200 0 "$check" http://localhost:8000 >/dev/null 2>&1; then
  fail "a local http URL was accepted."
fi
if with_curl 200 0 "$check" "" >/dev/null 2>&1; then
  fail "an empty URL was accepted."
fi
[[ ! -s "$FIXTURE_CURL_LOG" ]] || fail "a URL the shape preflight refuses was requested anyway."

# --- The override skips the request and says so ----------------------------------

: >"$FIXTURE_CURL_LOG"
output="$(VELVT_ALLOW_UNREACHABLE_API=1 with_curl 522 22 "$check" https://dev-api.example.com 2>&1)" ||
  fail "VELVT_ALLOW_UNREACHABLE_API=1 did not let the build go on."
grep -q 'WARNING: .*pipeline test while the API is down' <<<"$output" || fail "the override does not warn: $output"
[[ ! -s "$FIXTURE_CURL_LOG" ]] || fail "the override still made the request."
if VELVT_ALLOW_UNREACHABLE_API=yes with_curl 522 22 "$check" https://dev-api.example.com >/dev/null 2>&1; then
  fail "an override value other than 1 was honoured."
fi

# --- The real curl, against a host that cannot resolve ----------------------------

if command -v curl >/dev/null 2>&1; then
  if output="$("$check" https://velvt-ready-check.invalid 2>&1)"; then
    fail "an unresolvable host was accepted."
  fi
  grep -q 'did not answer' <<<"$output" || fail "an unresolvable host was refused for another reason: $output"
fi

# --- alpha-dmg and release run it, on the URL they bake, before building ----------

line_of() { # <fixed string> <text>
  { grep -n -m1 -F -e "$1" <<<"$2" || true; } | cut -d: -f1
}
expect_checked_before_build() { # <target> <url> [make args...]
  local target="$1" url="$2" plan check_line build_line
  shift 2
  plan="$(make -n -C "$repo_root" "$target" "$@" 2>/dev/null || true)"
  check_line="$(line_of "check_api_ready.sh \"$url\"" "$plan")"
  build_line="$(line_of '-project swift-client/VelvtMac.xcodeproj' "$plan")"
  [[ -n "$check_line" ]] || fail "make $target does not check that $url answers."
  [[ -n "$build_line" ]] || fail "make -n $target printed no build step to compare against."
  ((check_line < build_line)) || fail "make $target builds before it checks that the API answers."
  grep -Fq 'VELVT_ALLOW_UNREACHABLE_API="0"' <<<"$plan" || fail "make $target does not default the override to 0."
}

expect_checked_before_build alpha-dmg https://dev-api.getvelvt.com
expect_checked_before_build alpha-dmg https://api.example.com VELVT_API_BASE_URL=https://api.example.com
expect_checked_before_build release https://api.example.com \
  VELVT_RELEASE_VERSION=0.0.0 VELVT_RELEASE_BUILD=0 \
  VELVT_PRODUCTION_API_BASE_URL=https://api.example.com VELVT_APPROVED_PRODUCTION_API_HOST=api.example.com

# --- With the override, both targets repeat the warning when they finish --------
# The first warning scrolls away under the build and notarization output, and
# a finished build otherwise reads the same whether its API was checked or not.

expect_final_warning() { # <target> <url> [make args...]
  local target="$1" url="$2" override plan warning_line tag_line warning output
  shift 2
  for override in 1 0; do
    plan="$(make -n -C "$repo_root" "$target" VELVT_ALLOW_UNREACHABLE_API="$override" "$@" 2>/dev/null || true)"
    warning_line="$(line_of 'was never checked' "$plan")"
    tag_line="$(line_of 'release_provenance.sh tag' "$plan")"
    [[ -n "$warning_line" ]] || fail "make $target does not repeat the override warning at the end."
    [[ -n "$tag_line" ]] || fail "make -n $target printed no tag step to compare against."
    ((warning_line > tag_line)) || fail "make $target repeats the override warning before it finishes."
    warning="$(grep -m1 -F 'was never checked' <<<"$plan")"
    output="$(sh -c "$warning" 2>&1)" || fail "make $target's closing warning step fails: $warning"
    if [[ "$override" == "1" ]]; then
      grep -Fq "WARNING: VELVT_ALLOW_UNREACHABLE_API=1: $url/v1/ready was never checked" <<<"$output" ||
        fail "make $target does not end with the override warning: $output"
    else
      [[ -z "$output" ]] || fail "make $target warns about an override that was not set: $output"
    fi
  done
}

expect_final_warning alpha-dmg https://dev-api.getvelvt.com
expect_final_warning release https://api.example.com \
  VELVT_RELEASE_VERSION=0.0.0 VELVT_RELEASE_BUILD=0 \
  VELVT_PRODUCTION_API_BASE_URL=https://api.example.com VELVT_APPROVED_PRODUCTION_API_HOST=api.example.com

echo "check_api_ready tests passed"
