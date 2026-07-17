# W2 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784303768`. Machine-readable receipt: [`results/run-1784303768-18050.json`](results/run-1784303768-18050.json).

Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 212.78 | 18.62 | 447.816 | 525.151 | 540.647 | 3.600 | configured-by-construction (events × --fork-cpu-ms) | 6.444 | Distinct-PID sampling measured 120 event workers; p50 completion was 447.8ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| scripted_socket_supervisor | 20 | 120 | 21 | 0 | 37.41 | 18.96 | 0.082 | 0.225 | 0.350 | 0.040 | sampled cumulative process-tree CPU via ps | 6.329 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 37.41MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| real_shell_socket_supervisor | 20 | 120 | 21 | 0 | 58.47 | 18.95 | 0.050 | 0.184 | 3.954 | 0.070 | sampled cumulative process-tree CPU via ps | 6.333 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 58.47MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## Reproduction

Each table row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run the command from the repository root. The sweep refuses configurations whose estimated runtime exceeds five minutes or whose fork baseline could exceed the harness memory rail.
