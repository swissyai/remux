#!/usr/bin/env bash
# One deterministic entry point for every VT1 tier. Run through scorer lock.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export VT1_REAL_ZIG="${VT1_REAL_ZIG:-$HOME/.local/bin/zig}"
export PATH="$ROOT/scripts/vt1-tools:$HOME/.local/bin:$PATH"
export GHOSTTY_SOURCE_DIR="${GHOSTTY_SOURCE_DIR:?set GHOSTTY_SOURCE_DIR to a pinned Ghostty source checkout}"
export GHOSTTY_ZIG_SYSTEM_DIR="${GHOSTTY_ZIG_SYSTEM_DIR:-$HOME/.cache/zig/p}"

[ "$(zig version)" = "0.15.2" ] || {
  echo "VT harness requires Zig 0.15.2" >&2
  exit 1
}
[ -f "$GHOSTTY_SOURCE_DIR/build.zig" ] || {
  echo "GHOSTTY_SOURCE_DIR is not the pinned Ghostty source" >&2
  exit 1
}

exec cargo run -p vt-harness --release --offline -- "$@"
