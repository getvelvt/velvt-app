#!/usr/bin/env bash
# Every script under scripts/ that starts with a shebang is run directly, by
# the Makefile, by CI, by another test, or by a person following the docs, so
# git must store it as executable (100755).
#
# The defect this pins: the cohort export, the analysis harness, prove_local.sh
# and most of the measurement tests were committed as 100644. With
# core.fileMode=false a checkout hides that, and `make test-measurement` then
# failed with "Permission denied" on a fresh clone. The mode that matters is
# the one in the index, because it is the one a clone gets, so that is what
# this reads. Fix a failure with `git update-index --chmod=+x <path>`.
#
# A copy that is pasted or downloaded never has the bit, whatever git says,
# which is why the tester's instruction is `bash export_cohort_evidence.sh`.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

if ! git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "script_modes_test.sh: SKIPPED (not a git checkout, no index to read)"
  exit 0
fi

bad=()
while IFS= read -r line; do
  mode="${line%% *}"
  path="${line#*$'\t'}"
  [[ -f "$repo_root/$path" ]] || continue
  if [[ "$(head -c 2 "$repo_root/$path")" == "#!" && "$mode" != "100755" ]]; then
    bad+=("$mode $path")
  fi
done < <(git -C "$repo_root" ls-files -s -- scripts)

if (( ${#bad[@]} > 0 )); then
  echo "FAIL: scripts with a shebang that git does not store as executable:" >&2
  printf '  %s\n' "${bad[@]}" >&2
  echo "Fix: git update-index --chmod=+x <path>" >&2
  exit 1
fi

echo "script_modes_test.sh: OK"
