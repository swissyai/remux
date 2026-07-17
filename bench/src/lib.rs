// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Public report boundary for the W1 benchmark harness.
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
        "# W1 benchmark results\n\nMeasured on `{}` / `{}` with `{}` at Unix time `{}`. Machine-readable receipt: [`{}`]({}).\n\n",
        report.machine.os,
        report.machine.architecture,
        report.machine.rustc,
        report.generated_unix_seconds,
        json_path,
        json_path
    );
    output.push_str("Peak RSS is the sampled sum for each subject process tree. CPU seconds are sampled cumulative subject CPU; harness/sampler processes are excluded. Latency is event creation-to-ingest for the supervisor and spawn-to-exit for fork-per-event.\n\n");
    output.push_str("| Model | Sessions | Events | Processes spawned | Per-event forks | Peak RSS (MiB) | Events/s | p50 (ms) | p95 (ms) | p99 (ms) | CPU (s) | Wall (s) | Interpretation | Reproduce |\n");
    output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|\n");
    for result in &report.results {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.2} | {:.2} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {} | `{}` |\n",
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
            result.wall_seconds,
            result.interpretation,
            result.command
        ));
    }
    output.push_str("\n## Reproduction\n\nEach table row carries its complete reproduction command. Cargo is forced offline by `.cargo/config.toml`; run the command from the repository root. The sweep refuses configurations whose estimated runtime exceeds five minutes or whose fork baseline could exceed the harness memory rail.\n");
    output
}

pub fn render_json(report: &BenchmarkReport) -> String {
    let mut output = format!(
        "{{\n  \"schema_version\": 1,\n  \"run_id\": {},\n  \"generated_unix_seconds\": {},\n  \"machine\": {{\"os\": {}, \"architecture\": {}, \"rustc\": {}}},\n  \"config\": {{\"sessions\": {}, \"events_per_session\": {}, \"rate\": {}, \"fork_hold_ms\": {}, \"fork_rss_mib\": {}}},\n  \"results\": [\n",
        quote(&report.run_id),
        report.generated_unix_seconds,
        quote(&report.machine.os),
        quote(&report.machine.architecture),
        quote(&report.machine.rustc),
        report.config.sessions,
        report.config.events_per_session,
        report.config.rate,
        report.config.fork_hold_ms,
        report.config.fork_rss_mib
    );
    for (index, result) in report.results.iter().enumerate() {
        output.push_str(&format!(
            "    {{\n      \"model\": {},\n      \"sessions\": {},\n      \"events\": {},\n      \"processes_spawned\": {},\n      \"per_event_forks\": {},\n      \"peak_rss_bytes\": {},\n      \"events_per_second\": {:.6},\n      \"latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}},\n      \"cpu_seconds\": {:.6},\n      \"wall_seconds\": {:.6},\n      \"reproduce\": {},\n      \"interpretation\": {}\n    }}{}\n",
            quote(&result.model),
            result.sessions,
            result.events,
            result.processes_spawned,
            result.per_event_forks,
            result.peak_rss_bytes,
            result.events_per_second,
            result.latency_us.p50,
            result.latency_us.p95,
            result.latency_us.p99,
            result.cpu_seconds,
            result.wall_seconds,
            quote(&result.command),
            quote(&result.interpretation),
            if index + 1 == report.results.len() { "" } else { "," }
        ));
    }
    output.push_str("  ]\n}\n");
    output
}

pub fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn micros_to_millis(micros: u64) -> f64 {
    micros as f64 / 1_000.0
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
