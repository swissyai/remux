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

## Provenance

The W1–W3 clean-room records state:

From `research/W1-notes.md`:

> - Clean-room attestation: W1 was implemented from this repository's goal and standard
>   operating-system/Rust interfaces. `../cmux-upstream` and all GPL source were never
>   opened, copied, or paraphrased.
> - The only cmux comparison facts used are the existing receipt in
>   `research/2026-07-15-cmux-investigation.md`: observed hook lifetime 0.37–0.38s,
>   transient RSS 18–21MB, and a Unix-socket message path. Those facts were already in
>   remux before W1; behavior observed, no code copied.

From `research/W2-notes.md`:

> - Clean-room attestation: W2 was implemented from this repository's W2 goal, W1
>   interfaces, and standard operating-system/Rust interfaces. `../cmux-upstream` and
>   all GPL source were never opened, copied, paraphrased, or consulted.
> - No new upstream behavioral fact was used in this wave. The socket and incident facts
>   inherited from W1 were already recorded in `research/W1-notes.md`; behavior
>   observed, no code copied.
> - W2 added no dependency, telemetry, network operation, paid model invocation, or
>   publish/deploy action. The real command is the machine's `/bin/sh` running a
>   long-lived built-in read/print loop.

From `research/W3-notes.md`:

> - W3 is being implemented only from this repository's goals, prior-wave
>   interfaces, Rust/std documentation knowledge, and the permitted Infinitty
>   leverage-plan lanes 2–3. `../cmux-upstream` and all GPL source are not opened,
>   copied, paraphrased, or consulted.
> - No Infinitty source file has been opened. The only inherited observations are
>   from `research/2026-07-18-infinitty-leverage-plan.md`: visible agent control and
>   event-driven idle-zero rendering. No code or line-level expression was copied.
> - No dependency, telemetry, network operation, paid invocation, publish, or
>   deploy action is planned. Cargo remains offline.

The workspace has zero third-party crates: it is std-only, enforced by the offline
Cargo configuration. cmux source has never been present in this repository; only
behavioral facts from published receipts were used.
