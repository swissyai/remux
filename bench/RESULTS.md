# W3 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784407224`. Machine-readable receipt: [`results/run-1784407224-19529.json`](results/run-1784407224-19529.json).

Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 213.02 | 18.78 | 408.008 | 462.470 | 584.316 | 3.600 | configured-by-construction (events × --fork-cpu-ms) | 6.388 | Distinct-PID sampling measured 120 event workers; p50 completion was 408.0ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| scripted_socket_supervisor | 20 | 120 | 21 | 0 | 37.48 | 18.83 | 0.062 | 0.100 | 0.124 | 0.020 | sampled cumulative process-tree CPU via ps | 6.374 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 37.48MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| real_shell_socket_supervisor | 20 | 120 | 21 | 0 | 55.70 | 19.71 | 0.038 | 0.058 | 0.074 | 0.040 | sampled cumulative process-tree CPU via ps | 6.090 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 55.70MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## TUI render receipt

TUI-only RSS is the resident supervisor/TUI root process; child-agent RSS is the authorized child set; total RSS is the simultaneously sampled complete subject tree. Idle CPU is root-process CPU delta / 60-second blocked idle wall window × 100. Event→redraw latency ends after the ANSI frame flush.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | TUI-only RSS (MiB) | Child-agent RSS (MiB) | TUI-inclusive total RSS (MiB) | Idle window (s) | Idle (CPU s / % / frames) | redraw p50 (ms) | redraw p95 (ms) | redraw p99 (ms) | Frames | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| real_shell_tui | 20 | 120 | 21 | 0 | 5.11 | 50.11 | 55.20 | 60.0 | 0.110 / 0.183% / 0 frames | 0.077 | 0.134 | 0.199 | 121 | 71.091 | One event-driven ANSI TUI rendered 20 live tabs over 20 authorized real shells; root and child RSS were sampled separately, with no frame bytes written during the 60s idle window. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## Reproduction

Each table row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run the command from the repository root. The sweep refuses configurations whose estimated runtime exceeds five minutes or whose fork baseline could exceed the harness memory rail.
