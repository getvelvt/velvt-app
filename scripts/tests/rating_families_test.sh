#!/usr/bin/env bash
# The SYNTHETIC rating families (scripts/generate_rating_families.py) and the
# analysis they check.
#
# These are contract tests of scripts/analyze_ratings.py, not evidence about
# anyone (FIX-1). They pin:
#
#   1. every spec in scripts/tests/fixtures/rating-families/ meets its
#      expectations: NULL (no spurious H1 beyond its nominal rate, held-out
#      AUROC near 0.5, and a leakage trap that shows the check can fail),
#      PLANTED (H1 passes at the planned n; slopes and tau recovered at a
#      large n; H2 passes there), MNAR (the bias is measured and the readout
#      shows the skew), SCALE (the person intercept absorbs scale use);
#   2. --require-held-out fails until a quarter of the specs are held out;
#   3. a spec with an unknown expectation, no expectation, or a held_out flag
#      that disagrees with its folder is refused;
#   4. the generator is deterministic;
#   5. the exports it writes run through the analysis CLI end to end, every
#      report line says SYNTHETIC, and the CLI refuses them without
#      --synthetic.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
generate="$repo_root/scripts/generate_rating_families.py"
analyze="$repo_root/scripts/analyze_ratings.py"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[[ -x "$generate" ]] || { echo "ERROR: $generate is not executable" >&2; exit 1; }

fail() { echo "FAIL: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1-2. The families, and the held-out rule.
# ---------------------------------------------------------------------------
held_out_met="$(python3 - "$repo_root/scripts" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
import generate_rating_families as gf
print("yes" if gf.held_out_status(gf.load_specs())[0] else "no")
PY
)"
if [[ "$held_out_met" == "yes" ]]; then
  "$generate" --check --require-held-out || fail "a family check failed"
else
  "$generate" --check || fail "a family check failed"
  set +e
  "$generate" --check --require-held-out > "$work/held-out.txt" 2>&1
  status=$?
  set -e
  [[ $status -eq 1 ]] || fail "--require-held-out passed without held-out fixtures (exit $status)"
  grep -q "NOT MET" "$work/held-out.txt" || fail "--require-held-out did not say why"
fi
echo "ok: every family meets its expectations"

# ---------------------------------------------------------------------------
# 3. Spec validation.
# ---------------------------------------------------------------------------
python3 - "$repo_root/scripts" "$work" <<'PY' || fail "a malformed spec was accepted"
import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import generate_rating_families as gf

good = json.loads((gf.FAMILY_DIR / "null.json").read_text())
cases = {
    "unknown-expectation.json": dict(good, expect=dict(good["expect"], h1_looks_fine=True)),
    "no-expectation.json": dict(good, expect={}),
    "unknown-key.json": dict(good, tuned=True),
    "claims-held-out.json": dict(good, held_out=True),
}
for name, spec in cases.items():
    path = Path(sys.argv[2]) / name
    path.write_text(json.dumps(spec))
    try:
        gf.load_spec(path)
    except gf.SpecError:
        continue
    raise AssertionError(f"{name} was accepted")
folder = Path(sys.argv[2]) / "held-out"
folder.mkdir()
path = folder / "not-flagged.json"
path.write_text(json.dumps(good))
try:
    gf.load_spec(path)
except gf.SpecError:
    pass
else:
    raise AssertionError("a spec in held-out/ without held_out: true was accepted")
PY
echo "ok: malformed specs are refused"

# ---------------------------------------------------------------------------
# 4-5. Determinism, and the CLI end to end.
# ---------------------------------------------------------------------------
"$generate" --out "$work/a" --family PLANTED > /dev/null
"$generate" --out "$work/b" --family PLANTED > /dev/null
diff -r "$work/a" "$work/b" > /dev/null || fail "the generator is not deterministic"
if find "$work/a" -type f | grep -v SYNTHETIC | grep -q .; then
  fail "a generated file name lacks SYNTHETIC"
fi

planted=("$work/a/planted"/*/)
"$analyze" --synthetic --final-look --ignore-stop-rule --bootstrap-replicates 199 \
  "${planted[@]}" > "$work/planted.txt" || fail "the analysis CLI failed on PLANTED"
if grep -v '^\[SYNTHETIC contract test, not evidence\] ' "$work/planted.txt" | grep -q .; then
  fail "a synthetic report line is not stamped"
fi
grep -q "H1 (primary): rho .*: PASSES" "$work/planted.txt" || fail "H1 did not pass on PLANTED"
grep -q "H2 (secondary)" "$work/planted.txt" || fail "H2 was not reported on PLANTED"
set +e
"$analyze" "${planted[@]}" > /dev/null 2>&1
status=$?
set -e
[[ $status -eq 2 ]] || fail "synthetic exports were read as real (exit $status)"
echo "ok: deterministic, stamped, and refused as real"

echo "rating_families_test.sh: all checks passed"
