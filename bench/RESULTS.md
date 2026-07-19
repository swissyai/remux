# W4 benchmark results

Measured on `macos 15.6.1` / `aarch64` with `rustc 1.94.1 (e408947bf 2026-03-25)` at Unix time `1784502492`. Machine-readable receipt: [`results/run-1784502492-55430.json`](results/run-1784502492-55430.json).

Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|
| fork_per_event | 20 | 120 | 120 | 120 | 232.38 | 18.79 | 457.480 | 527.525 | 586.299 | 3.600 | configured-by-construction (events × --fork-cpu-ms) | 6.386 | Distinct-PID sampling measured 120 event workers; p50 completion was 457.5ms versus the configured 360ms hold. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| scripted_socket_supervisor | 20 | 120 | 21 | 0 | 37.45 | 18.37 | 0.114 | 0.269 | 0.429 | 0.080 | sampled cumulative process-tree CPU via ps | 6.533 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 37.45MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| real_shell_socket_supervisor | 20 | 120 | 21 | 0 | 60.31 | 19.06 | 0.083 | 0.181 | 0.674 | 0.170 | sampled cumulative process-tree CPU via ps | 6.296 | 20 attached PTY sessions ingested 120 events through one socket; distinct-PID sampling found 21 processes and 0 event forks at 60.31MiB peak RSS. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## TUI render receipt

TUI-only RSS is the resident supervisor/TUI root process; child-agent RSS is the authorized child set; total RSS is the simultaneously sampled complete subject tree. Idle CPU is root-process CPU delta / 60-second blocked idle wall window × 100. Event→redraw latency ends after the ANSI frame flush.

| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | TUI-only RSS (MiB) | Child-agent RSS (MiB) | TUI-inclusive total RSS (MiB) | Idle window (s) | Idle (CPU s / % / frames) | redraw p50 (ms) | redraw p95 (ms) | redraw p99 (ms) | Frames | Wall (s) | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| real_shell_tui | 20 | 120 | 21 | 0 | 5.89 | 53.52 | 59.39 | 60.1 | 0.100 / 0.166% / 0 frames | 0.257 | 0.779 | 2.454 | 120 | 71.479 | One event-driven ANSI TUI rendered 20 live tabs over 20 authorized real shells; root and child RSS were sampled separately, with no frame bytes written during the 60s idle window. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |

## W4 real-trace and attestation receipt

The headline workload is a live working session captured first and replayed second. `unattested` and `hash-chain` replay byte-identical trace records with identical captured monotonic spacing. Event→redraw starts before attestation handoff and ends after ANSI flush, so its delta includes the per-observation allocation/copy.

| Subject / mode | Sessions | Events | Distinct processes measured | Event forks | Peak RSS (MiB) | Events/s | ingest p95 (ms) | redraw p95 (ms) | CPU (s) | Attestation records / file bytes | Wall (s) | Trace / availability anchor | Interpretation | Reproduce |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---|
| remux_real_trace_unattested | 20 | 280 | 21 | 0 | 36.69 | 51.55 | 0.299 | 0.402 | 0.050 | 0 / 0 | 5.431 | `bench/traces/w4-working-session.trace` / `36aa4f2db01ee05139a65c083995cc151390c1028817ccdd0566811486c7f2fd` | Replayed 14 records/session from a prior live project-working PTY capture across 20 sessions; 21 measured PIDs, 0 event forks; attestation off. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| remux_real_trace_hash_chain | 20 | 280 | 21 | 0 | 35.94 | 55.51 | 2.019 | 2.098 | 0.120 | 380 / 64716 | 5.044 | `bench/traces/w4-working-session.trace` / `36aa4f2db01ee05139a65c083995cc151390c1028817ccdd0566811486c7f2fd` | Replayed 14 records/session from a prior live project-working PTY capture across 20 sessions; 21 measured PIDs, 0 event forks; attestation hash-chain. | `scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18` |
| infinitty_v0.1.7_macos / measured | 20 | 19 | 21 | N/A | 147.44 | N/A | 35.313 | N/A | N/A | N/A | N/A | /Applications/Infinitty.app/Contents/MacOS/Infinitty; /Applications/Infinitty.app/Contents/MacOS/infinitty; /Users/dev/Applications/Infinitty.app/Contents/MacOS/Infinitty; /Users/dev/Applications/Infinitty.app/Contents/MacOS/infinitty; /private/tmp/build/-Users-dev-founder-mode-remux/b3217d96-b2b9-4c01-9cdc-87d2ba58058c/scratchpad/repo/.build/release/infinitty | Measured latency is app-socket new-tab request→pane-id for 19 additions to one initial pane. The distinct-PID union may include transient shell-startup helpers; steady snapshots enforce one app + 20 resident shells. Infinitty exposes no event→flushed-redraw receipt and has no persistence/restore or attestation, so those cells remain N/A. | INFINITTY_APP=/private/tmp/build/-Users-dev-founder-mode-remux/b3217d96-b2b9-4c01-9cdc-87d2ba58058c/scratchpad/repo/.build/release/infinitty scripts/with_scorer_lock.sh cargo run -p bench -- --sessions 20 --events-per-session 6 --rate 20 --fork-hold-ms 360 --fork-cpu-ms 30 --fork-rss-mib 18 |

Measured hash-chain delta over the same real trace: redraw p95 +1.696ms, RSS -0.75MiB, wall -0.387s; 380 synchronized attestation records / 64716 bytes.

## Reproduction

Each measured row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run from the repository root through the scorer lock. The Infinitty row is an observed local-availability result: absent metrics are N/A, never estimates.
