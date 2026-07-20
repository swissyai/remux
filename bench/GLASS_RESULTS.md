# W5 terminal-layer glass receipts

Same-machine serial run `run-1784563710-30555` at Unix time `1784563710` on `macos 15.6.1` / `aarch64`. Frozen workload `bench/workloads/w5-terminal-fleet.tsv` SHA-256 `ef7890d4cd5e04360a1a952d3aefb7c0378f1c1f929ce0e2484c25eb727f66c5`; machine gate and negative controls: `bench/results/w5/run-1784563710-30555/preflight.tsv`. Machine-readable receipt: [`results/w5/run-1784563710-30555/report.json`](results/w5/run-1784563710-30555/report.json).

All RSS values are externally sampled complete subject trees. Peak covers launch + trace replay; steady is the median post-trace resident sample. Distinct PID is the observed union, not an asserted spawn count. N/A means the subject exposes no equivalent receipt or was absent; it is never an estimate.

Machine-state axis: this machine's interactive zsh startup includes a custom agent picker. Binding correction 01 excludes the founder's resident cmux/Ghostty instances; every measured foreign session uses a direct non-interactive workload and `NTAP_DONE=1`. Pre-correction resident rows are retained anomalies, never promoted here.

| Terminal layer | Availability | Workload | Sessions | Pre-existing | Peak RSS (MiB) | Steady RSS (MiB) | Distinct / steady PIDs | Fleet spawn (ms) | Per-session ack p95 (ms) | Capability gaps | Raw |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|---|
| remux std-only TUI (attested) `0.1.0@6d00755c18fb` | measured | W4 real-trace replay + ANSI TUI + hash chain | 20 | 0 | 40.09 | 37.12 | 21 / 21 | 63.835 | N/A | Per-session spawn acknowledgements are not exposed (fleet launch→ready is measured); 380 attestation records are externally verified. | `bench/results/w5/run-1784563710-30555/remux.tsv` |
| cmux Ghostty-embedded incumbent `0.64.19@1c22c5564` | incompatible-isolated-instance | N/A | N/A | N/A | N/A | N/A | N/A / N/A | N/A | N/A | Binding correction 01 forbids measuring the founder's resident app; locked direct/open -na probes exposed no independent socket. Peak/steady RSS, process tree, fleet spawn, and terminal acknowledgement are N/A, never borrowed from the retained anomaly. | `bench/results/w5/run-1784563710-30555/cmux.tsv` |
| ghostty vanilla Metal baseline `1.3.1@332b2aefc` | incompatible-isolated-instance | N/A | N/A | N/A | N/A | N/A | N/A / N/A | N/A | N/A | Binding correction 01 forbids the founder's resident app. The installed public AppleScript dictionary cannot target one secondary app PID, so a clean 20-session process tree is unavailable and every metric is N/A. | `bench/results/w5/run-1784563710-30555/ghostty.tsv` |
| Infinitty v0.1.7 narrow Metal `0.1.7@09b3e8b2aa3cec72a3c9a1db604c9c3c5235ee1c` | measured-fresh-process | 20 app-socket panes replaying W4 trace | 20 | 0 | 114.83 | 114.69 | 21 / 21 | 821.882 | 41.582 | App-socket pane acknowledgement is measured; persistence/passive restore, capability authorization, supervisor attestation, and event→flushed-redraw receipts are N/A. | `bench/results/w5/run-1784563710-30555/infinitty.tsv` |

## cmux hook architecture refresh

**Behavior observed, no code copied.** Installed cmux `0.64.19@1c22c5564` accepted `5` live `hooks pi event` invocations against its running Unix socket. External sampling observed `5` distinct hook CLI PIDs (`1.0` fork/event), `13.33`MiB peak per-hook RSS, and 163.986/177.388/177.388ms wall p50/p95/p99. Each accepted event launched a separate current cmux CLI process; the 2026-07-15 0.37–0.38s/18–21MB figure is refreshed, not copied forward. Raw: `bench/results/w5/run-1784563710-30555/cmux-hooks.tsv`.

## Glass verdict (recommendation; founder decides)

Build-vs-adopt threshold: build a renderer only for a required capability an adoptable path cannot supply, or for a measured roughly **10x** real-consumer outcome advantage. A self-authored fleet microbenchmark certifies machinery and cost; it does not by itself prove a 10x product outcome.

| Candidate path | What receipts justify now | Recommendation | Re-open gate |
|---|---|---|---|
| std-only TUI as-is | Already owns passive restore, exact capabilities, attestation, idle/redraw gates, and the measured remux arm without a renderer dependency. | **Stay TUI now.** Preserve the lowest-complexity substrate while real consumers exercise the public run route. | A production workflow shows a glass limitation with accepted-outcome, latency, or retention harm. |
| embed libghostty | The installed Ghostty manifest confirms the nearest mature Metal path, but correction 01 makes its resident app off-limits and its public control surface cannot target a clean secondary PID; terminal-layer cost is N/A. W5 does not measure libghostty ABI/integration cost. | **Shadow only; do not adopt this wave.** The missing isolated baseline weakens, not strengthens, an adoption case. | A hermetic isolated prototype measures end-to-end RSS/redraw/build surface and beats TUI on a named consumer enough to pay its dependency/FFI tax. |
| narrow Metal renderer | Infinitty is an existence proof for a small resident Metal terminal, not evidence that rebuilding terminal correctness creates a 10x consumer gain. It adds the largest bespoke maintenance surface. | **Park.** Build-vs-adopt law is not met. | libghostty cannot satisfy a required measured capability, and a narrow prototype demonstrates roughly 10x outcome advantage under a fixed budget. |

No renderer dependency, FFI, telemetry, or display code was adopted in W5.
