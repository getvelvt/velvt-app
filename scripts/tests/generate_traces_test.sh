#!/usr/bin/env bash
# The generator is the ground truth for `rust-service/tests/trace_replay.rs`,
# so two things have to hold: the fixtures on disk are exactly what it produces
# today, and the clock rule is enforced in code rather than described in a
# comment. A generator that silently emitted a backdated trace would produce a
# suite that passes while exercising nothing.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
generator="$repo_root/scripts/generate_traces.py"
traces="$repo_root/scripts/traces"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$generator" ]] || { echo "ERROR: $generator is not executable" >&2; exit 1; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. The committed fixtures are not stale.
# ---------------------------------------------------------------------------
"$generator" --check > "$work/check.txt" 2>&1 || {
  cat "$work/check.txt" >&2
  fail "the committed fixtures do not match a fresh generation"
}

# ---------------------------------------------------------------------------
# 2. Generation is deterministic from the seed. Same seed, byte-identical.
# ---------------------------------------------------------------------------
"$generator" --out "$work/first" >/dev/null
"$generator" --out "$work/second" >/dev/null
for name in SYNTHETIC-suite-a-recovery.jsonl SYNTHETIC-suite-b-null.jsonl \
            SYNTHETIC-suite-b-null-compressed.jsonl SYNTHETIC-manifest.json; do
  cmp -s "$work/first/$name" "$work/second/$name" \
    || fail "$name is not reproducible from the seed"
done

# A different seed must actually change the noise, or the seed is decorative.
"$generator" --out "$work/other" --seed 999 >/dev/null
if cmp -s "$work/first/SYNTHETIC-suite-b-null.jsonl" \
          "$work/other/SYNTHETIC-suite-b-null.jsonl"; then
  fail "changing the seed did not change the null traces"
fi
# Suite A is hand-authored, so it must NOT move with the seed.
cmp -s "$work/first/SYNTHETIC-suite-a-recovery.jsonl" \
       "$work/other/SYNTHETIC-suite-a-recovery.jsonl" \
  || fail "the hand-authored recovery suite drifted with the seed"

# ---------------------------------------------------------------------------
# 3. The clock rule is enforced, not merely documented.
# ---------------------------------------------------------------------------
python3 - "$generator" <<'PY' || fail "the monotonicity guard does not fire"
import importlib.util, sys
spec = importlib.util.spec_from_file_location("gen", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

backwards = [module.observation(400, "FOCUS_WORK"), module.observation(100, "COMMUNICATION")]
try:
    module.block(backwards, where="test")
except module.NonMonotonicTrace:
    pass
else:
    raise SystemExit("a backdated trace was accepted")

repeated = [module.observation(100, "FOCUS_WORK"), module.observation(100, "COMMUNICATION")]
try:
    module.block(repeated, where="test")
except module.NonMonotonicTrace:
    pass
else:
    raise SystemExit("two observations at the same instant were accepted")

before_start = [module.observation(0, "FOCUS_WORK"), module.observation(100, "COMMUNICATION")]
try:
    module.block(before_start, where="test")
except module.NonMonotonicTrace:
    pass
else:
    raise SystemExit("an observation at the block start instant was accepted")

# An invented category must be refused: a fixture exercising a category the
# product cannot produce validates nothing.
try:
    module.block([module.observation(10, "DEEP_WORK")], where="test")
except ValueError:
    pass
else:
    raise SystemExit("a category outside the shipped taxonomy was accepted")

# And a legal trace is still accepted.
module.block([module.observation(10, "FOCUS_WORK"), module.observation(20, "COMMUNICATION")],
             where="test")
PY

# ---------------------------------------------------------------------------
# 4. Every fixture is labelled SYNTHETIC — in the filename and in the header
#    record of the file. An unlabelled synthetic number reaching a deck is the
#    failure this project's whole thesis cannot survive.
# ---------------------------------------------------------------------------
for path in "$traces"/*.jsonl; do
  base="$(basename "$path")"
  case "$base" in
    SYNTHETIC-*) : ;;
    *) fail "$base is not labelled SYNTHETIC in its filename" ;;
  esac
  # Parsed rather than grepped: the label contains an em dash, which json.dumps
  # escapes to —, so a byte-level grep would pass or fail for reasons that
  # have nothing to do with the label being present.
  python3 - "$path" <<'PY' || fail "$base: header record is not labelled SYNTHETIC"
import json, sys
with open(sys.argv[1]) as handle:
    header = json.loads(handle.readline())
assert header.get("kind") == "header", header.get("kind")
assert header.get("synthetic") is True, header
assert header.get("label", "").startswith("SYNTHETIC"), header.get("label")
assert "No real user" in header.get("label", ""), header.get("label")
PY
done
[[ -f "$traces/ASSUMPTIONS.md" ]] || fail "no ASSUMPTIONS.md beside the generator output"
grep -qF "n = 1" "$traces/ASSUMPTIONS.md" || fail "ASSUMPTIONS.md does not state the sample size"

# ---------------------------------------------------------------------------
# 5. The null family really is structureless in the way it claims: no trace
#    may carry an expect_offer, and every trace must be monotone.
# ---------------------------------------------------------------------------
python3 - "$traces/SYNTHETIC-suite-b-null.jsonl" <<'PY' || fail "null suite integrity"
import json, sys
lines = open(sys.argv[1]).read().splitlines()
header = json.loads(lines[0])
assert header["kind"] == "header", header
assert header["expect_offers"] == 0, header
traces = [json.loads(line) for line in lines[1:]]
assert len(traces) == header["traces"], (len(traces), header["traces"])
for trace in traces:
    assert trace["family"] == "NULL", trace["trace_id"]
    assert trace["expect_offer"] is False, trace["trace_id"]
    assert trace["blocks"], trace["trace_id"]
    for block in trace["blocks"]:
        offsets = [o["t"] for o in block["observations"]]
        assert offsets == sorted(offsets), f"{trace['trace_id']}: not monotone"
        assert len(set(offsets)) == len(offsets), f"{trace['trace_id']}: duplicate instants"
        assert offsets[0] >= 1, f"{trace['trace_id']}: starts at the block start"
        assert 300 <= block["planned_duration_seconds"] <= 10800, block
PY

# ---------------------------------------------------------------------------
# 6. Suite A's negatives must outnumber its positives, or a gate that always
#    fires would pass the recovery suite.
# ---------------------------------------------------------------------------
python3 - "$traces/SYNTHETIC-suite-a-recovery.jsonl" <<'PY' || fail "recovery suite balance"
import json, sys
lines = open(sys.argv[1]).read().splitlines()
traces = [json.loads(line) for line in lines[1:]]
positives = [t for t in traces if t["expect_offer"]]
negatives = [t for t in traces if not t["expect_offer"]]
assert len(positives) >= 5, len(positives)
assert len(negatives) > len(positives), (len(negatives), len(positives))
assert any(t["family"] == "TRAP" for t in traces), "no backdated trap trace"
for trace in traces:
    assert trace["expect_reason"].strip(), f"{trace['trace_id']} has no stated reason"
PY

echo "generate_traces_test.sh: OK"
