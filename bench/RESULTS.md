# W1 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784254185`. Machine-readable receipt: [`results/run-1784254185-57038.json`](results/run-1784254185-57038.json).

Peak RSS is the sampled sum for each subject process tree. CPU seconds are sampled cumulative subject CPU; harness/sampler processes are excluded. Latency is event creation-to-ingest for the supervisor and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Processes spawned | Per-event forks | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 254.06 | 18.83 | 466.009 | 535.928 | 663.312 | 3.600 | 6.372 | One process per event produced 120 forks; p50 completion was 466.0ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| socket_supervisor | 20 | 120 | 21 | 0 | 35.62 | 18.23 | 0.068 | 0.253 | 0.491 | 0.030 | 6.584 | 20 PTY sessions ingested 120 events through one socket with zero per-event forks at 35.62MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## Reproduction

Each table row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run the command from the repository root. The sweep refuses configurations whose estimated runtime exceeds five minutes or whose fork baseline could exceed the harness memory rail.
