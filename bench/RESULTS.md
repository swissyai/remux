# W3 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784406928`. Machine-readable receipt: [`results/run-1784406928-8701.json`](results/run-1784406928-8701.json).

Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 193.44 | 18.93 | 398.494 | 450.234 | 475.128 | 3.600 | configured-by-construction (events × --fork-cpu-ms) | 6.339 | Distinct-PID sampling measured 120 event workers; p50 completion was 398.5ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| scripted_socket_supervisor | 20 | 120 | 21 | 0 | 37.97 | 19.35 | 0.065 | 0.153 | 0.878 | 0.020 | sampled cumulative process-tree CPU via ps | 6.201 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 37.97MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| real_shell_socket_supervisor | 20 | 120 | 21 | 0 | 54.98 | 19.71 | 0.043 | 0.066 | 0.151 | 0.050 | sampled cumulative process-tree CPU via ps | 6.089 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 54.98MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## TUI render receipt

TUI-only RSS is the resident supervisor/TUI root process; child-agent RSS is the authorized child set; total RSS is the simultaneously sampled complete subject tree. Idle CPU is root-process CPU delta / 60-second blocked idle wall window × 100. Event→redraw latency ends after the ANSI frame flush.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | TUI-only RSS (MiB) | Child-agent RSS (MiB) | TUI-inclusive total RSS (MiB) | Idle window (s) | Idle (CPU s / % / frames) | redraw p50 (ms) | redraw p95 (ms) | redraw p99 (ms) | Frames | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| real_shell_tui | 20 | 120 | 21 | 0 | 5.27 | 49.02 | 54.09 | 60.1 | 0.110 / 0.183% / 0 frames | 0.083 | 0.154 | 0.205 | 119 | 71.179 | One event-driven ANSI TUI rendered 20 live tabs over 20 authorized real shells; root and child RSS were sampled separately, with no frame bytes written during the 60s idle window. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## Reproduction

Each table row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run the command from the repository root. The sweep refuses configurations whose estimated runtime exceeds five minutes or whose fork baseline could exceed the harness memory rail.
