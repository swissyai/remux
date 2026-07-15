# remux

Rust terminal workspace for running fleets of coding agents without the overhead.

Continuous-improvement project. Reference implementation: cmux (github.com/manaflow-ai/cmux,
cloned at ../cmux-upstream). Born from a measured incident: cmux's per-tool-call hook
architecture (one 38MB CLI fork per tool call per session, 0.36s/18MB each) amplified a
fleet overload into a kernel panic on 2026-07-15. See research/2026-07-15-cmux-investigation.md.

## Design goals

- One resident process; no per-event forks. Agent events arrive over a Unix socket the
  supervisor already owns.
- PTY/session supervisor: tabs, splits, workspace persistence, crash-safe restore.
- Direct in-process session status tracking (what cmux does with hook storms).
- Optional notifications and auto-naming, off by default.
- No embedded browser, no feed, no cloud presence, no analytics, no NODE_OPTIONS preload
  injection.
- Memory budget: <200MB resident under a 20-session fleet (cmux measured 2.1GB).

## Status

Charter stage. Nothing built. See AGENTS.md for the operating rules and the licensing
decision that must be made before any code is written.
