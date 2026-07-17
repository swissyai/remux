#!/usr/bin/env bash
# Global heavy-op lock: at most ONE scorer/install run on this machine at a
# time, fleet-wide. Agents run every scorer through this wrapper:
#
#   scripts/with_scorer_lock.sh bash -c 'cd api && npm run typecheck && node --test test/*.test.mjs'
#
# Why: six concurrent npm/tsc/test runs wedged the machine hard enough to
# trip the hardware watchdog (2026-07-15 and 2026-07-16 resets). Serializing
# scorers costs seconds; the alternative costs a reboot.
set -euo pipefail

LOCK_DIR="${LH_SCORER_LOCK_DIR:-/tmp/lh-scorer.lock}"
WAIT_SECS="${LH_SCORER_LOCK_WAIT:-1800}"
POLL=5

[ $# -gt 0 ] || { echo "usage: with_scorer_lock.sh <command> [args...]" >&2; exit 2; }

acquire() {
  local waited=0 owner
  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    owner="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
    if [ -n "$owner" ] && ! kill -0 "$owner" 2>/dev/null; then
      rm -rf "$LOCK_DIR"   # stale: owner is gone
      continue
    fi
    if [ "$waited" -ge "$WAIT_SECS" ]; then
      echo "with_scorer_lock: gave up after ${WAIT_SECS}s (held by pid ${owner:-unknown})" >&2
      exit 75
    fi
    [ "$waited" -eq 0 ] && echo "with_scorer_lock: waiting for the machine-wide scorer slot (held by pid ${owner:-unknown})..." >&2
    sleep "$POLL"; waited=$((waited + POLL))
  done
  echo $$ > "$LOCK_DIR/pid"
}

release() { rm -rf "$LOCK_DIR"; }

acquire
trap release EXIT INT TERM
"$@"
