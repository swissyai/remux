# remux

Rust terminal supervisor for running fleets of coding agents in one resident process.

Std-only, zero third-party crates. Agent events arrive over a Unix socket the supervisor
already owns — no per-event process forks. Most agent terminal workspaces fork a helper CLI
on every tool call (measured at ~0.36s and 18MB per fork); at fleet scale that pattern turns
into gigabytes of burst load. remux is built against that failure mode, with the benchmarks
kept in-repo.

## Why sessions live in the supervisor

Agent runs are getting longer — hours instead of seconds — and long runs get
handed between people. A session that exists only inside one terminal window
can't be handed off, audited, or trusted after the fact. remux treats the
session as the durable object and the window as a view:

- Sessions outlive windows. The supervisor owns PTYs, layout, and scrollback;
  a crash or detach loses nothing, and restore never re-executes anything.
- Driving is granted, not ambient. Launch and relaunch consume single-use,
  logged authorizations, so every handoff leaves an audit trail.
- Runs can be verified without being watched. `--attest` emits an externally
  verifiable chain receipt for the command a session actually ran.

Live shared attach — several people viewing and driving one session — is not
built yet. The pieces above are the substrate it needs: session state that no
single window owns, and an authorization log that says who did what.

## Design

- One resident process; no per-event forks. Session status is tracked in-process.
- PTY/session supervisor: workspace persistence and crash-safe restore.
- Restore is passive — layout and scrollback only. Agent sessions never auto-execute on
  restore; relaunch goes through an explicit gate.
- Attested runs: `remux-supervisor run --cwd DIR --attest -- COMMAND...` preserves command
  stdout and emits an externally verifiable chain receipt on stderr, behind a prior
  single-use lifecycle grant.
- Notifications and auto-naming exist but are off by default. No embedded browser, no feed,
  no cloud presence, no analytics, no NODE_OPTIONS preload injection.
- Memory budget: under 200MB resident for a 20-session fleet.

## Benchmarks

Reproducible and in-repo; Cargo is forced offline. Each measured row carries its full
reproduction command — run them through `scripts/with_scorer_lock.sh`.

- `bench/GLASS_RESULTS.md` — frozen 20-session terminal-layer sweep. remux measured at
  ~40MiB resident with 64ms fleet spawn including attestation; other terminals measured in
  the same sweep (Infinitty ~115MiB / 21 processes; resident cmux and Ghostty arms are
  honest N/A rather than estimates).
- `bench/RESULTS.md` — RSS, process count, latency, idle/render rails, real-trace
  attestation overhead. The fork-per-event baseline measured 2.1GB resident for the same
  fleet size remux holds under 40MiB.

## Build

```sh
scripts/with_scorer_lock.sh cargo build --offline
```

Workspace crates: `supervisor` (socket protocol, capability grants, attestation chain),
`pty`, `bench`. No third-party dependencies; the offline Cargo configuration enforces it.

## Status

Early and moving. The supervisor core, attestation receipts, and the benchmark harness are
real and measured; the full terminal workspace UI is in progress.

## License

Apache-2.0
