#!/usr/bin/env bash
# Every test for the measurement and evidence scripts, in one command.
#
# These cover `analyze_cohort.py`, `export_cohort_evidence.sh`,
# `prove_local.sh`, `antecedent_probe.py`, `generate_traces.py`, and the
# pbxproj target-membership guard. They need only python3 and the sqlite3 that
# ships with macOS — no cargo, no Xcode — so they run in seconds and there is
# no excuse for skipping them.
#
# The Rust half of the trace harness lives in
# `rust-service/tests/trace_replay.rs` and runs under `cargo test`.

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

tests=(
  analyze_cohort_test.sh
  export_cohort_evidence_test.sh
  prove_local_test.sh
  antecedent_probe_test.sh
  generate_traces_test.sh
  verify_pbxproj_membership_test.sh
)

failed=0
for test in "${tests[@]}"; do
  printf '\n=== %s\n' "$test"
  if ! "$here/$test"; then
    failed=$((failed + 1))
  fi
done

printf '\n'
if (( failed > 0 )); then
  echo "$failed of ${#tests[@]} measurement test(s) FAILED" >&2
  exit 1
fi
echo "all ${#tests[@]} measurement tests passed"
