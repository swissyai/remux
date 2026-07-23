# W4 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784503926`. Machine-readable receipt: [`results/run-1784503926-64590.json`](results/run-1784503926-64590.json).

Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 232.20 | 18.59 | 488.456 | 532.079 | 607.846 | 3.600 | configured-by-construction (events × --fork-cpu-ms) | 6.455 | Distinct-PID sampling measured 120 event workers; p50 completion was 488.5ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| scripted_socket_supervisor | 20 | 120 | 21 | 0 | 38.78 | 18.82 | 0.086 | 0.231 | 0.807 | 0.270 | sampled cumulative process-tree CPU via ps | 6.377 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 38.78MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| real_shell_socket_supervisor | 20 | 120 | 21 | 0 | 62.56 | 18.59 | 0.080 | 0.193 | 0.493 | 0.430 | sampled cumulative process-tree CPU via ps | 6.456 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 62.56MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## TUI render receipt

TUI-only RSS is the resident supervisor/TUI root process; child-agent RSS is the authorized child set; total RSS is the simultaneously sampled complete subject tree. Idle CPU is root-process CPU delta / 60-second blocked idle wall window × 100. Event→redraw latency ends after the ANSI frame flush.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | TUI-only RSS (MiB) | Child-agent RSS (MiB) | TUI-inclusive total RSS (MiB) | Idle window (s) | Idle (CPU s / % / frames) | redraw p50 (ms) | redraw p95 (ms) | redraw p99 (ms) | Frames | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| real_shell_tui | 20 | 120 | 21 | 0 | 5.94 | 44.59 | 50.30 | 60.1 | 0.090 / 0.150% / 0 frames | 0.285 | 0.955 | 2.192 | 118 | 71.274 | One event-driven ANSI TUI rendered 20 live tabs over 20 authorized real shells; root and child RSS were sampled separately, with no frame bytes written during the 60s idle window. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## W4 real-trace and attestation receipt

The headline workload is a live working session captured first and replayed second. `unattested` and `hash-chain` replay byte-identical trace records with identical captured monotonic spacing. Event→redraw starts before attestation handoff and ends after ANSI flush, so its delta includes the per-observation allocation/copy.

| Subject / mode | Sessions | Events | Distinct processes measured | Event forks | Peak RSS (MiB) | Events/s | ingest p95 (ms) | redraw p95 (ms) | CPU (s) | Attestation records / file bytes | Wall (s) | Trace / availability anchor | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---|
| remux_real_trace_unattested | 20 | 280 | 21 | 0 | 35.75 | 52.47 | 0.353 | 0.506 | 0.060 | 0 / 0 | 5.337 | `bench/traces/w4-working-session.trace` / `36aa4f2db01ee05139a65c083995cc151390c1028817ccdd0566811486c7f2fd` | Replayed 14 records/session from a prior live project-working PTY capture across 20 sessions; 21 measured PIDs, 0 event forks; attestation off. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| remux_real_trace_hash_chain | 20 | 280 | 21 | 0 | 36.19 | 56.48 | 1.983 | 2.598 | 0.340 | 380 / 69294 | 4.958 | `bench/traces/w4-working-session.trace` / `36aa4f2db01ee05139a65c083995cc151390c1028817ccdd0566811486c7f2fd` | Replayed 14 records/session from a prior live project-working PTY capture across 20 sessions; 21 measured PIDs, 0 event forks; attestation hash-chain. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| infinitty_v0.1.7_macos / measured | 20 | 19 | 21 | N/A | 145.92 | N/A | 79.563 | N/A | N/A | N/A | N/A | /Applications/Infinitty.app/Contents/MacOS/Infinitty; /Applications/Infinitty.app/Contents/MacOS/infinitty; <local-path>/Applications/Infinitty.app/Contents/MacOS/Infinitty; <local-path>/Applications/Infinitty.app/Contents/MacOS/infinitty; <local-path>/infinitty | Measured latency is app-socket new-tab request→pane-id for 19 additions to one initial pane. The distinct-PID union may include transient shell-startup helpers; steady snapshots enforce one app + 20 resident shells. Infinitty exposes no event→flushed-redraw receipt and has no persistence/restore or attestation, so those cells remain N/A. | INFINITTY_APP=<local-path>/infinitty scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18 |

Measured hash-chain delta over the same real trace: redraw p95 +2.092ms, RSS +0.44MiB, wall -0.379s; 380 synchronized attestation records / 69294 bytes.

## Reproduction

Each measured row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run from the repository root through the scorer lock. The Infinitty row is an observed local-availability result: absent metrics are N/A, never estimates.
