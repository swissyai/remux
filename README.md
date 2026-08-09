# remux

Rust terminal supervisor for running fleets of coding agents in one resident process.

Product crates are std-only with zero third-party dependencies (the VT characterization
harness additionally uses the MIT `libghostty-vt` bindings). Agent events arrive over a Unix socket the supervisor
already owns — no per-event process forks. Most agent terminal workspaces fork a helper CLI
on every tool call (measured at ~0.36s and 18MB per fork); at fleet scale that pattern turns
into gigabytes of burst load. remux is built against that failure mode, with the benchmarks
kept in-repo.

## Design

- One resident process; no per-event forks. Session status is tracked in-process.
- PTY/session supervisor: workspace persistence and crash-safe restore.
- Restore is passive — layout and scrollback only. Agent sessions never auto-execute on
  restore; relaunch goes through an explicit gate.
- Attested runs: `remux-supervisor run --cwd DIR --attest -- COMMAND...` preserves command
  stdout and emits an externally verifiable chain receipt on stderr, behind a prior
  single-use lifecycle grant. The receipt is a hash chain, not a signature: it establishes
  that the recorded sequence is intact and in what order, not who ran it.
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

## VT characterization harness

`vt-harness/` pins the terminal-emulation core's behavior so any future change to the VT
layer is gated by receipts, not trust: 425 table-driven characterization cases with
full-state golden snapshots, 20 replayed byte-stream corpora (5.4MB, per-step state
diffs), 120 adversarial malformed-input cases, 15 machine-checked invariants, an
ABI-generic differential A/B runner with a seeded fuzzer (100k-execution smoke tier), and
a planted-mutation gate: 14 committed behavioral mutations against a scratch build of the
upstream source, all of which the harness must detect — a harness that cannot reject is
not a gate. One command runs every tier and writes a machine-readable receipt:

```sh
GHOSTTY_SOURCE_DIR=/path/to/ghostty scripts/with_scorer_lock.sh scripts/score_vt1_01.sh
```

Requires Zig 0.15.2 and a Ghostty source checkout (MIT, © Mitchell Hashimoto and
contributors) for the differential/mutation tiers; `cargo fetch` once for the harness's
crates.io dependencies.

## Build

```sh
scripts/with_scorer_lock.sh cargo build --offline
```

Workspace crates: `supervisor` (socket protocol, capability grants, attestation chain),
`pty`, `bench`, `vt-harness`. Product crates carry no third-party dependencies; the
harness declares its own (fetched once with `cargo fetch`).

## Status

Early and moving. The supervisor core, attestation receipts, and the benchmark harness are
real and measured; the full terminal workspace UI is in progress.

## License

Apache-2.0
