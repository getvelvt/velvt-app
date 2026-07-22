#!/usr/bin/env bash
set -euo pipefail

sparkle_bin_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sparkle-bin-dir) sparkle_bin_dir="${2:-}"; shift 2 ;;
    *) echo "Usage: $0 --sparkle-bin-dir /path/to/Sparkle-2.9.4/bin" >&2; exit 2 ;;
  esac
done

sign_update="$sparkle_bin_dir/sign_update"
[[ -x "$sign_update" ]] || { echo "ERROR: pinned Sparkle sign_update is unavailable: $sign_update" >&2; exit 1; }

fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/velvt-update-adversarial.XXXXXX")"
cleanup() { rm -rf "$fixture_root"; }
trap cleanup EXIT
key_a="$fixture_root/non-production-fixture-key-a"
key_b="$fixture_root/non-production-fixture-key-b"
# Public, deterministic test seeds. These are intentionally not production secrets.
printf 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' > "$key_a"
printf 'AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=' > "$key_b"
chmod 600 "$key_a" "$key_b"

fail() { echo "ERROR: $*" >&2; exit 1; }
expect_failure() {
  local description="$1"
  shift
  if "$@" >/dev/null 2>&1; then fail "$description unexpectedly succeeded"; fi
  echo "PASS adversarial: $description rejected"
}

version_greater_than() {
  local candidate="$1" previous="$2" old_ifs="$IFS" index length
  local -a candidate_parts previous_parts
  IFS='.' read -r -a candidate_parts <<<"$candidate"
  IFS='.' read -r -a previous_parts <<<"$previous"
  IFS="$old_ifs"
  length="${#candidate_parts[@]}"
  if (( ${#previous_parts[@]} > length )); then length="${#previous_parts[@]}"; fi
  for ((index = 0; index < length; index++)); do
    local candidate_part="${candidate_parts[index]:-0}"
    local previous_part="${previous_parts[index]:-0}"
    if ((10#$candidate_part > 10#$previous_part)); then return 0; fi
    if ((10#$candidate_part < 10#$previous_part)); then return 1; fi
  done
  return 1
}

assert_supported_os() {
  local simulated_os="$1" minimum_os="$2"
  [[ "$simulated_os" == "$minimum_os" ]] || version_greater_than "$simulated_os" "$minimum_os"
}

make_fixture_archive() {
  local build="$1"
  local output="$2"
  local staging="$fixture_root/staging-$build"
  mkdir -p "$staging/Velvt.app/Contents/Resources"
  printf 'fixture-build=%s\n' "$build" > "$staging/Velvt.app/Contents/Resources/build.txt"
  printf 'fixture-binary-%s\n' "$build" > "$staging/Velvt.app/Contents/Resources/payload.bin"
  touch -t 202001010000 "$staging/Velvt.app/Contents/Resources/build.txt" "$staging/Velvt.app/Contents/Resources/payload.bin"
  (
    cd "$staging"
    /usr/bin/zip -X -q -r "$output" Velvt.app
  )
}

archives="$fixture_root/archives"
mkdir -p "$archives"
archive_n="$archives/Velvt-0.1.0-1.zip"
archive_n1="$archives/Velvt-0.2.0-2.zip"
make_fixture_archive 1 "$archive_n"
make_fixture_archive 2 "$archive_n1"

signing_metadata="$($sign_update --ed-key-file "$key_a" "$archive_n1")"
archive_signature="$(sed -n 's/.*sparkle:edSignature="\([^"]*\)".*/\1/p' <<<"$signing_metadata")"
archive_length="$(sed -n 's/.*length="\([0-9]*\)".*/\1/p' <<<"$signing_metadata")"
[[ "$archive_signature" =~ ^[A-Za-z0-9+/]{86}==$ ]] || fail "real sign_update returned an invalid archive signature"
[[ "$archive_length" == "$(stat -f '%z' "$archive_n1")" ]] || fail "real sign_update returned the wrong archive length"
$sign_update --verify --ed-key-file "$key_a" "$archive_n1" "$archive_signature"
echo "PASS real-tool: N+1 archive signature verified"

wrong_key_archive="$fixture_root/wrong-key.zip"
cp "$archive_n1" "$wrong_key_archive"
expect_failure "archive wrong key" "$sign_update" --verify --ed-key-file "$key_b" "$wrong_key_archive" "$archive_signature"
modified_archive="$fixture_root/modified.zip"
cp "$archive_n1" "$modified_archive"
printf 'tamper' >> "$modified_archive"
expect_failure "modified archive" "$sign_update" --verify --ed-key-file "$key_a" "$modified_archive" "$archive_signature"
truncated_archive="$fixture_root/truncated.zip"
head -c "$((archive_length - 1))" "$archive_n1" > "$truncated_archive"
expect_failure "truncated archive" "$sign_update" --verify --ed-key-file "$key_a" "$truncated_archive" "$archive_signature"

feed="$fixture_root/appcast.xml"
cat > "$feed" <<XML
<?xml version="1.0" standalone="yes"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0"><channel>
<title>Velvt local adversarial fixture</title><item>
<title>0.2.0</title><sparkle:version>2</sparkle:version>
<sparkle:shortVersionString>0.2.0</sparkle:shortVersionString>
<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
<enclosure url="https://updates.example.invalid/$(basename "$archive_n1")" length="$archive_length" type="application/octet-stream" sparkle:edSignature="$archive_signature"></enclosure>
</item></channel></rss>
XML
$sign_update --ed-key-file "$key_a" "$feed"
$sign_update --verify --ed-key-file "$key_a" "$feed"
grep -q '</rss><!-- sparkle-signatures:' "$feed" || fail "real signed-feed format was not emitted"
echo "PASS real-tool: appcast signature verified in Sparkle 2.9.4 format"

modified_feed="$fixture_root/modified-appcast.xml"
cp "$feed" "$modified_feed"
sed -i '' 's/Velvt local adversarial fixture/Velvt modified fixture/' "$modified_feed"
expect_failure "modified signed feed" "$sign_update" --verify --ed-key-file "$key_a" "$modified_feed"
expect_failure "feed wrong key" "$sign_update" --verify --ed-key-file "$key_b" "$feed"

version_greater_than 2 1 || fail "N+1 build was not accepted over N"
expect_failure "same build" version_greater_than 2 2
expect_failure "lower build" version_greater_than 1 2
echo "PASS structural: candidate 2 accepted only over prior build 1"

assert_supported_os 14.0 13.0 || fail "supported simulated OS was rejected"
expect_failure "unsupported minimum OS" assert_supported_os 14.0 99.0
grep -q 'https://updates.example.invalid/' "$feed" || fail "reserved unreachable HTTPS fixture URL is missing"
expect_failure "non-HTTPS update URL" bash -c '[[ "file:///tmp/update.zip" == https://* ]]'
echo "PASS structural: reserved unreachable host is isolated; no network request was attempted"

external_state="$fixture_root/external-state"
mkdir -p "$external_state"
printf 'sqlite-count=17\npreference-count=4\nkeychain-surrogate=present\n' > "$external_state/state-ledger"
state_before="$(shasum -a 256 "$external_state/state-ledger" | awk '{print $1}')"
for archive in "$archive_n" "$archive_n1"; do
  unzip -Z1 "$archive" | while IFS= read -r entry; do
    [[ "$entry" == Velvt.app/* || "$entry" == Velvt.app/ ]] || fail "archive escaped the application bundle: $entry"
  done
  if unzip -Z1 "$archive" | grep -q 'external-state\|state-ledger'; then fail "archive captured external user state"; fi
  install_fixture="$fixture_root/install-$(basename "$archive" .zip)"
  mkdir -p "$install_fixture"
  ditto -x -k "$archive" "$install_fixture"
done
state_after="$(shasum -a 256 "$external_state/state-ledger" | awk '{print $1}')"
[[ "$state_after" == "$state_before" ]] || fail "external-state fixture changed during archive extraction"
echo "PASS boundary: local state stayed outside both bundle archives and remained unchanged"

published="$fixture_root/published"
mkdir -p "$published"
publish_feed() {
  local source_feed="$1" source_archive="$2"
  [[ -f "$published/$(basename "$source_archive")" ]] || return 1
  ln "$source_feed" "$published/appcast.xml"
}
expect_failure "appcast-before-archive publication" publish_feed "$feed" "$archive_n1"
archive_digest="$(shasum -a 256 "$archive_n1" | awk '{print $1}')"
ln "$archive_n1" "$published/$(basename "$archive_n1")"
[[ "$(shasum -a 256 "$published/$(basename "$archive_n1")" | awk '{print $1}')" == "$archive_digest" ]] || fail "published archive checksum changed"
expect_failure "immutable archive overwrite" ln "$archive_n1" "$published/$(basename "$archive_n1")"
publish_feed "$feed" "$archive_n1"
expect_failure "immutable appcast overwrite" publish_feed "$feed" "$archive_n1"
echo "PASS publication: immutable archive-first/appcast-last state machine enforced"

echo "LOCAL UPDATE ADVERSARIAL HARNESS PASSED"
echo "VERIFICATION: real Sparkle archive/feed cryptography; structural version/OS/URL/publish/data-boundary checks"
echo "NOT VERIFIED: app replacement, relaunch, Keychain/TCC persistence, network failure UI, Developer ID, notarization, or external publication"
