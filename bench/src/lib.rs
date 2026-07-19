// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Public report boundary for the benchmark harness.
//!
//! Contract: scenario runners return measurements in common units; renderers produce
//! deterministic Markdown and JSON without knowing supervisor implementation details.

pub mod system;

#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkReport {
    pub run_id: String,
    pub generated_unix_seconds: u64,
    pub machine: Machine,
    pub config: BenchmarkConfig,
    pub results: Vec<ScenarioResult>,
    pub tui_result: Option<TuiScenarioResult>,
    pub trace_results: Vec<TraceScenarioResult>,
    pub infinitty_result: InfinittyScenarioResult,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Machine {
    pub os: String,
    pub architecture: String,
    pub rustc: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkConfig {
    pub sessions: u32,
    pub events_per_session: u64,
    pub rate: u64,
    pub fork_hold_ms: u64,
    pub fork_cpu_ms: u64,
    pub fork_rss_mib: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioResult {
    pub model: String,
    pub sessions: u32,
    pub events: u64,
    pub processes_spawned: u64,
    pub per_event_forks: u64,
    pub peak_rss_bytes: u64,
    pub events_per_second: f64,
    pub latency_us: Percentiles,
    pub cpu_seconds: f64,
    pub cpu_source: String,
    pub wall_seconds: f64,
    pub command: String,
    pub interpretation: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Percentiles {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}

/// Measured 20-tab render scenario with root/child resource separation.
#[derive(Clone, Debug, PartialEq)]
pub struct TuiScenarioResult {
    pub model: String,
    pub sessions: u32,
    pub events: u64,
    pub processes_spawned: u64,
    pub per_event_forks: u64,
    pub tui_peak_rss_bytes: u64,
    pub child_agent_peak_rss_bytes: u64,
    pub total_peak_rss_bytes: u64,
    pub idle_window_seconds: f64,
    pub idle_cpu_seconds: f64,
    pub idle_cpu_percent: f64,
    pub idle_frames_rendered: u64,
    pub redraw_latency_us: Percentiles,
    pub frames_rendered: u64,
    pub wall_seconds: f64,
    pub command: String,
    pub interpretation: String,
}

/// Before/after replay of one immutable real working-session trace.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceScenarioResult {
    pub model: String,
    pub attestation_enabled: bool,
    pub trace_path: String,
    pub trace_command_sha256: String,
    pub sessions: u32,
    pub events: u64,
    pub processes_spawned: u64,
    pub per_event_forks: u64,
    pub peak_rss_bytes: u64,
    pub events_per_second: f64,
    pub ingest_latency_us: Percentiles,
    pub redraw_latency_us: Percentiles,
    pub cpu_seconds: f64,
    pub attestation_records: u64,
    pub attestation_file_bytes: u64,
    pub wall_seconds: f64,
    pub command: String,
    pub interpretation: String,
}

/// Same-machine competitor observation. Metrics are optional only when the local
/// subject or a requested feature is absent; unavailable values render as N/A/null.
#[derive(Clone, Debug, PartialEq)]
pub struct InfinittyScenarioResult {
    pub model: String,
    pub availability: String,
    pub probe_paths: Vec<String>,
    pub sessions: u32,
    pub events: Option<u64>,
    pub processes_spawned: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub latency_us: Option<Percentiles>,
    pub command: Option<String>,
    pub feature_gap: String,
}

pub fn percentiles(samples: &[u64]) -> Result<Percentiles, &'static str> {
    if samples.is_empty() {
        return Err("latency sample set is empty");
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    Ok(Percentiles {
        p50: nearest_rank(&sorted, 50),
        p95: nearest_rank(&sorted, 95),
        p99: nearest_rank(&sorted, 99),
    })
}

fn nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percentile).saturating_add(99) / 100;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

pub fn render_markdown(report: &BenchmarkReport, json_path: &str) -> String {
    let mut output = format!(
        "# W4 benchmark results\n\nMeasured on `{}` / `{}` with `{}` at Unix time `{}`. Machine-readable receipt: [`{}`]({}).\n\n",
        report.machine.os,
        report.machine.architecture,
        report.machine.rustc,
        report.generated_unix_seconds,
        json_path,
        json_path
    );
    output.push_str("Peak RSS and process counts come from repeated snapshots of each subject process tree; the count is the union of distinct observed PIDs. Harness/sampler processes are excluded. Supervisor CPU is sampled cumulative subject CPU. Fork-model CPU is configured-by-construction as events × `--fork-cpu-ms`, and is labeled separately below. Latency is event creation-to-ingest for socket scenarios and spawn-to-exit for fork-per-event.\n\n");
    output.push_str("| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | CPU provenance | Wall (s) | Interpretation | Reproduce |\n");
    output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|---|\n");
    for result in &report.results {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {:.3} | {} | `{}` |\n",
            result.model,
            result.sessions,
            result.events,
            result.processes_spawned,
            result.per_event_forks,
            bytes_to_mib(result.peak_rss_bytes),
            result.events_per_second,
            micros_to_millis(result.latency_us.p50),
            micros_to_millis(result.latency_us.p95),
            micros_to_millis(result.latency_us.p99),
            result.cpu_seconds,
            result.cpu_source,
            result.wall_seconds,
            result.interpretation,
            result.command
        ));
    }
    output.push_str("\n## TUI render receipt\n\nTUI-only RSS is the resident supervisor/TUI root process; child-agent RSS is the authorized child set; total RSS is the simultaneously sampled complete subject tree. Idle CPU is root-process CPU delta / 60-second blocked idle wall window × 100. Event→redraw latency ends after the ANSI frame flush.\n\n");
    output.push_str("| Model | Sessions | Events | Distinct processes measured | Per-event forks measured | TUI-only RSS (MiB) | Child-agent RSS (MiB) | TUI-inclusive total RSS (MiB) | Idle window (s) | Idle (CPU s / % / frames) | redraw p50 (ms) | redraw p95 (ms) | redraw p99 (ms) | Frames | Wall (s) | Interpretation | Reproduce |\n");
    output.push_str(
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|\n",
    );
    if let Some(result) = &report.tui_result {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.1} | {:.3} / {:.3}% / {} frames | {:.3} | {:.3} | {:.3} | {} | {:.3} | {} | `{}` |\n",
            result.model,
            result.sessions,
            result.events,
            result.processes_spawned,
            result.per_event_forks,
            bytes_to_mib(result.tui_peak_rss_bytes),
            bytes_to_mib(result.child_agent_peak_rss_bytes),
            bytes_to_mib(result.total_peak_rss_bytes),
            result.idle_window_seconds,
            result.idle_cpu_seconds,
            result.idle_cpu_percent,
            result.idle_frames_rendered,
            micros_to_millis(result.redraw_latency_us.p50),
            micros_to_millis(result.redraw_latency_us.p95),
            micros_to_millis(result.redraw_latency_us.p99),
            result.frames_rendered,
            result.wall_seconds,
            result.interpretation,
            result.command
        ));
    }
    output.push_str("\n## W4 real-trace and attestation receipt\n\nThe headline workload is a live working session captured first and replayed second. `unattested` and `hash-chain` replay byte-identical trace records with identical captured monotonic spacing. Event→redraw starts before attestation handoff and ends after ANSI flush, so its delta includes the per-observation allocation/copy.\n\n");
    output.push_str("| Subject / mode | Sessions | Events | Distinct processes measured | Event forks | Peak RSS (MiB) | Events/s | ingest p95 (ms) | redraw p95 (ms) | CPU (s) | Attestation records / file bytes | Wall (s) | Trace / availability anchor | Interpretation | Reproduce |\n");
    output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---|\n");
    for result in &report.trace_results {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.3} | {:.3} | {:.3} | {} / {} | {:.3} | `{}` / `{}` | {} | `{}` |\n",
            result.model,
            result.sessions,
            result.events,
            result.processes_spawned,
            result.per_event_forks,
            bytes_to_mib(result.peak_rss_bytes),
            result.events_per_second,
            micros_to_millis(result.ingest_latency_us.p95),
            micros_to_millis(result.redraw_latency_us.p95),
            result.cpu_seconds,
            result.attestation_records,
            result.attestation_file_bytes,
            result.wall_seconds,
            result.trace_path,
            result.trace_command_sha256,
            result.interpretation,
            result.command
        ));
    }
    let infinitty = &report.infinitty_result;
    output.push_str(&format!(
        "| {} / {} | {} | {} | {} | N/A | {} | N/A | {} | N/A | N/A | N/A | N/A | {} | {} | {} |\n",
        infinitty.model,
        infinitty.availability,
        infinitty.sessions,
        optional_u64_markdown(infinitty.events),
        optional_u64_markdown(infinitty.processes_spawned),
        optional_bytes_markdown(infinitty.peak_rss_bytes),
        optional_latency_markdown(infinitty.latency_us.as_ref()),
        infinitty.probe_paths.join("; "),
        infinitty.feature_gap,
        infinitty.command.as_deref().unwrap_or("N/A")
    ));
    if let [before, after] = report.trace_results.as_slice() {
        output.push_str(&format!(
            "\nMeasured hash-chain delta over the same real trace: redraw p95 {:+.3}ms, RSS {:+.2}MiB, wall {:+.3}s; {} synchronized attestation records / {} bytes.\n",
            micros_to_millis(after.redraw_latency_us.p95) - micros_to_millis(before.redraw_latency_us.p95),
            bytes_to_mib(after.peak_rss_bytes) - bytes_to_mib(before.peak_rss_bytes),
            after.wall_seconds - before.wall_seconds,
            after.attestation_records,
            after.attestation_file_bytes
        ));
    }
    output.push_str("\n## Reproduction\n\nEach measured row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run from the repository root through the scorer lock. The Infinitty row is an observed local-availability result: absent metrics are N/A, never estimates.\n");
    output
}

pub fn render_json(report: &BenchmarkReport) -> String {
    let mut output = format!(
        "{{\n  \"schema_version\": 4,\n  \"run_id\": {},\n  \"generated_unix_seconds\": {},\n  \"machine\": {{\"os\": {}, \"architecture\": {}, \"rustc\": {}}},\n  \"config\": {{\"sessions\": {}, \"events_per_session\": {}, \"rate\": {}, \"fork_hold_ms\": {}, \"fork_cpu_ms\": {}, \"fork_rss_mib\": {}}},\n  \"results\": [\n",
        quote(&report.run_id),
        report.generated_unix_seconds,
        quote(&report.machine.os),
        quote(&report.machine.architecture),
        quote(&report.machine.rustc),
        report.config.sessions,
        report.config.events_per_session,
        report.config.rate,
        report.config.fork_hold_ms,
        report.config.fork_cpu_ms,
        report.config.fork_rss_mib
    );
    for (index, result) in report.results.iter().enumerate() {
        output.push_str(&format!(
            "    {{\n      \"model\": {},\n      \"sessions\": {},\n      \"events\": {},\n      \"processes_spawned\": {},\n      \"per_event_forks\": {},\n      \"peak_rss_bytes\": {},\n      \"events_per_second\": {:.6},\n      \"latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}},\n      \"cpu_seconds\": {:.6},\n      \"cpu_source\": {},\n      \"wall_seconds\": {:.6},\n      \"reproduce\": {},\n      \"interpretation\": {}\n    }}{}\n",
            quote(&result.model), result.sessions, result.events, result.processes_spawned,
            result.per_event_forks, result.peak_rss_bytes, result.events_per_second,
            result.latency_us.p50, result.latency_us.p95, result.latency_us.p99,
            result.cpu_seconds, quote(&result.cpu_source), result.wall_seconds,
            quote(&result.command), quote(&result.interpretation),
            if index + 1 == report.results.len() { "" } else { "," }
        ));
    }
    output.push_str("  ],\n  \"tui_result\": ");
    match &report.tui_result {
        Some(result) => output.push_str(&format!(
            "{{\n    \"model\": {},\n    \"sessions\": {},\n    \"events\": {},\n    \"processes_spawned\": {},\n    \"per_event_forks\": {},\n    \"tui_peak_rss_bytes\": {},\n    \"child_agent_peak_rss_bytes\": {},\n    \"total_peak_rss_bytes\": {},\n    \"idle_window_seconds\": {:.6},\n    \"idle_cpu_seconds\": {:.6},\n    \"idle_cpu_percent\": {:.6},\n    \"idle_frames_rendered\": {},\n    \"redraw_latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}},\n    \"frames_rendered\": {},\n    \"wall_seconds\": {:.6},\n    \"reproduce\": {},\n    \"interpretation\": {}\n  }}",
            quote(&result.model), result.sessions, result.events, result.processes_spawned,
            result.per_event_forks, result.tui_peak_rss_bytes, result.child_agent_peak_rss_bytes,
            result.total_peak_rss_bytes, result.idle_window_seconds, result.idle_cpu_seconds,
            result.idle_cpu_percent, result.idle_frames_rendered, result.redraw_latency_us.p50,
            result.redraw_latency_us.p95, result.redraw_latency_us.p99, result.frames_rendered,
            result.wall_seconds, quote(&result.command), quote(&result.interpretation)
        )),
        None => output.push_str("null"),
    }
    output.push_str(",\n  \"trace_results\": [\n");
    for (index, result) in report.trace_results.iter().enumerate() {
        output.push_str(&format!(
            "    {{\n      \"model\": {},\n      \"attestation_enabled\": {},\n      \"trace_path\": {},\n      \"trace_command_sha256\": {},\n      \"sessions\": {},\n      \"events\": {},\n      \"processes_spawned\": {},\n      \"per_event_forks\": {},\n      \"peak_rss_bytes\": {},\n      \"events_per_second\": {:.6},\n      \"ingest_latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}},\n      \"redraw_latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}},\n      \"cpu_seconds\": {:.6},\n      \"attestation_records\": {},\n      \"attestation_file_bytes\": {},\n      \"wall_seconds\": {:.6},\n      \"reproduce\": {},\n      \"interpretation\": {}\n    }}{}\n",
            quote(&result.model), result.attestation_enabled, quote(&result.trace_path),
            quote(&result.trace_command_sha256), result.sessions, result.events,
            result.processes_spawned, result.per_event_forks, result.peak_rss_bytes,
            result.events_per_second, result.ingest_latency_us.p50, result.ingest_latency_us.p95,
            result.ingest_latency_us.p99, result.redraw_latency_us.p50,
            result.redraw_latency_us.p95, result.redraw_latency_us.p99, result.cpu_seconds,
            result.attestation_records, result.attestation_file_bytes, result.wall_seconds,
            quote(&result.command), quote(&result.interpretation),
            if index + 1 == report.trace_results.len() { "" } else { "," }
        ));
    }
    let infinitty = &report.infinitty_result;
    output.push_str("  ],\n  \"infinitty_result\": {\n");
    output.push_str(&format!(
        "    \"model\": {},\n    \"availability\": {},\n    \"probe_paths\": [{}],\n    \"sessions\": {},\n    \"events\": {},\n    \"processes_spawned\": {},\n    \"peak_rss_bytes\": {},\n    \"latency_us\": {},\n    \"reproduce\": {},\n    \"feature_gap\": {}\n  }}\n}}\n",
        quote(&infinitty.model),
        quote(&infinitty.availability),
        infinitty.probe_paths.iter().map(|path| quote(path)).collect::<Vec<_>>().join(", "),
        infinitty.sessions,
        optional_u64_json(infinitty.events),
        optional_u64_json(infinitty.processes_spawned),
        optional_u64_json(infinitty.peak_rss_bytes),
        optional_percentiles_json(infinitty.latency_us.as_ref()),
        infinitty.command.as_deref().map_or_else(|| "null".to_owned(), quote),
        quote(&infinitty.feature_gap)
    ));
    output
}

pub fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn micros_to_millis(micros: u64) -> f64 {
    micros as f64 / 1_000.0
}

fn optional_u64_markdown(value: Option<u64>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |number| number.to_string())
}

fn optional_bytes_markdown(value: Option<u64>) -> String {
    value.map_or_else(
        || "N/A".to_owned(),
        |bytes| format!("{:.2}", bytes_to_mib(bytes)),
    )
}

fn optional_latency_markdown(value: Option<&Percentiles>) -> String {
    value.map_or_else(
        || "N/A".to_owned(),
        |latency| format!("{:.3}", micros_to_millis(latency.p95)),
    )
}

fn optional_u64_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn optional_percentiles_json(value: Option<&Percentiles>) -> String {
    value.map_or_else(
        || "null".to_owned(),
        |latency| {
            format!(
                "{{\"p50\": {}, \"p95\": {}, \"p99\": {}}}",
                latency.p50, latency.p95, latency.p99
            )
        },
    )
}

fn quote(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < '\u{20}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}
