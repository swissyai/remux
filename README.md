# remux

Rust terminal workspace for running fleets of coding agents without the overhead.

Continuous-improvement project. Reference implementation: cmux (github.com/manaflow-ai/cmux,
cloned at ../cmux-upstream). Born from a measured incident: cmux's per-tool-call hook
architecture (one 38MB CLI fork per tool call per session, 0.36s/18MB each) amplified a
fleet overload into a kernel panic on 2026-07-15. See research/2026-07-15-cmux-investigation.md.

Direction (founder, 2026-07-16): remux is cmux-to-Rust, optimized for the failure modes
we keep hitting in fleet work — fork storms, resident bloat, blind session auto-resume —
with benchmarks built in throughout. The same treatment extends to the agent runner
layer (pi): measure first, optimize what the receipts justify. End state: a working
environment that improves against its own benchmarks, usable by us and anyone else.
The full principles and their source studies (Devin Fusion economics, Replit
self-driving company, AIDE2 RSI, memory/loop/locality protocol, Modal 1M-sandbox
architecture) live in DOCTRINE.md — read it before designing any wave.

## Design goals

- One resident process; no per-event forks. Agent events arrive over a Unix socket the
  supervisor already owns.
- PTY/session supervisor: tabs, splits, workspace persistence, crash-safe restore.
- Restore is passive: layout and scrollback only. Agent sessions never auto-execute on
  restore; relaunch goes through an explicit gate (cmux's autoResumeAgentSessions
  defaulting to true respawned a crashed fleet on 2026-07-16 — remux must not
  reproduce that).
- Direct in-process session status tracking (what cmux does with hook storms).
- Optional notifications and auto-naming, off by default.
- No embedded browser, no feed, no cloud presence, no analytics, no NODE_OPTIONS preload
  injection.
- Memory budget: <200MB resident under a 20-session fleet (cmux measured 2.1GB).

## Status

W5 glass receipts: `remux-supervisor run --cwd DIR --attest -- COMMAND...` is the
public arbitrary-command route behind a prior single-use lifecycle grant; it preserves
command stdout and emits an externally verified chain receipt on stderr. The frozen
20-session terminal-layer sweep and founder-owned glass recommendation live in
`bench/GLASS_RESULTS.md`: remux and fresh Infinitty are numeric; correction 01 keeps
resident cmux/Ghostty state off-limits, so failed isolated-instance arms are honest
N/A. Current cmux hook cost is refreshed separately from five live hook CLI processes.

The inherited W4 table remains in `bench/RESULTS.md`: RSS/process/latency,
idle/render rails, real-trace attestation overhead, and the attributed Infinitty
baseline.

Run all build/test/benchmark commands through `scripts/with_scorer_lock.sh`; Cargo is
forced offline. See `CONTEXT.md` for stable domain terms and `AGENTS.md` for the
clean-room and engineering rules.
