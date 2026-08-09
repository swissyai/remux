#!/usr/bin/env bash
# VT1 scorer: the invariant harness exists, runs green, and can reject.
# Run through scripts/with_scorer_lock.sh. Floors match GOAL.md exactly.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export VT1_REAL_ZIG="${VT1_REAL_ZIG:-$HOME/.local/bin/zig}"
export PATH="$ROOT/scripts/vt1-tools:$HOME/.local/bin:$PATH"
export GHOSTTY_SOURCE_DIR="${GHOSTTY_SOURCE_DIR:?set GHOSTTY_SOURCE_DIR to a pinned Ghostty source checkout}"
export GHOSTTY_ZIG_SYSTEM_DIR="${GHOSTTY_ZIG_SYSTEM_DIR:-$HOME/.cache/zig/p}"
fail() { echo "VT1 FAIL: $*" >&2; exit 1; }

[ -x scripts/run_vt_harness.sh ] || fail "scripts/run_vt_harness.sh missing or not executable"
[ -d vt-harness ] || fail "vt-harness crate missing"

cargo fmt --check -p vt-harness || fail "cargo fmt"
cargo clippy -p vt-harness --all-targets --offline -- -D warnings || fail "clippy"
cargo test -p vt-harness --offline --quiet || fail "tests"

grep -Rn "forbid(unsafe_code)" vt-harness/src >/dev/null \
  || fail "vt-harness must carry forbid(unsafe_code) outside the FFI seam"

# The one entry command: runs every tier, writes the receipt.
scripts/run_vt_harness.sh || fail "harness run failed"

RECEIPT="bench/results/vt-harness/receipt.json"
[ -s "$RECEIPT" ] || fail "receipt missing: $RECEIPT"

python3 - "$RECEIPT" <<'EOF' || exit 1
import json, sys
r = json.load(open(sys.argv[1]))
def need(k):
    if k not in r:
        print(f"VT1 FAIL: receipt missing field {k}", file=sys.stderr); sys.exit(1)
    return r[k]
floors = {
    "authoredCases": 300,
    "streamCorpora": 20,
    "streamBytes": 5_000_000,
    "adversarialCases": 100,
    "invariantProperties": 12,
    "plantedMutations": 14,
    "fuzzExecutions": 100_000,
}
for k, floor in floors.items():
    v = need(k)
    if v < floor:
        print(f"VT1 FAIL: {k}={v} below floor {floor}", file=sys.stderr); sys.exit(1)
if need("mutationsKilled") != r["plantedMutations"]:
    print(f"VT1 FAIL: mutation kill rate below 100% "
          f"({r['mutationsKilled']}/{r['plantedMutations']})", file=sys.stderr); sys.exit(1)
kills = need("mutationKills")
if len(kills) != r["plantedMutations"] or any(not k.get("detectedBy") for k in kills):
    print("VT1 FAIL: every planted mutation must record its detecting tier", file=sys.stderr); sys.exit(1)
if need("fuzzDivergences") != 0:
    print(f"VT1 FAIL: fuzz divergences={r['fuzzDivergences']} (must be 0)", file=sys.stderr); sys.exit(1)
if need("pass") is not True:
    print("VT1 FAIL: receipt pass flag is not true", file=sys.stderr); sys.exit(1)
if not need("gitSha") or not need("timestamp"):
    print("VT1 FAIL: receipt must carry gitSha and timestamp", file=sys.stderr); sys.exit(1)
print("VT1-01 scorer: PASS "
      f"(cases={r['authoredCases']}, corpora={r['streamCorpora']}, "
      f"adversarial={r['adversarialCases']}, properties={r['invariantProperties']}, "
      f"mutations {r['mutationsKilled']}/{r['plantedMutations']} killed, "
      f"fuzz={r['fuzzExecutions']} execs / {r['fuzzDivergences']} div)")
EOF
