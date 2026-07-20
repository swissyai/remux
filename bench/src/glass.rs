// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring workload contracts.
//! W5 terminal-layer workload and receipt boundary.
//!
//! Contract: the frozen manifest has an exact closed schema; live adapters report
//! optional values as N/A rather than estimates. Renderers are deterministic and do
//! not inspect subject processes or know app-specific control protocols.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{bytes_to_mib, Percentiles};

const WORKLOAD_HEADER: &str = "REMUX_GLASS_WORKLOAD_V1";

/// Frozen W5 workload and machine-admission contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlassWorkload {
    pub sessions: u32,
    pub trace: PathBuf,
    pub trace_records_per_session: u64,
    pub trace_last_event_us: u64,
    pub hold_after_trace_ms: u64,
    pub steady_window_ms: u64,
    pub sample_interval_ms: u64,
    pub arm_timeout_seconds: u64,
    pub max_load_per_logical_cpu_milli: u64,
    pub min_free_memory_percent: u8,
    pub cmux_hook_events: u32,
}

impl GlassWorkload {
    /// Reads the exact versioned field set and rejects omissions, extras, duplicates,
    /// malformed numbers, unsafe bounds, and a trace shape other than W5's frozen
    /// 20-session W4 replay.
    pub fn read(path: &Path) -> io::Result<Self> {
        let input = fs::read_to_string(path)?;
        Self::parse(&input)
    }

    pub fn parse(input: &str) -> io::Result<Self> {
        if !input.ends_with('\n') || input.lines().next() != Some(WORKLOAD_HEADER) {
            return Err(invalid_data("invalid glass workload framing"));
        }
        let mut fields = BTreeMap::new();
        for line in input.lines().skip(1) {
            let (key, value) = line
                .split_once('\t')
                .ok_or_else(|| invalid_data("invalid glass workload field"))?;
            if key.is_empty()
                || value.is_empty()
                || value.contains('\t')
                || fields.insert(key, value).is_some()
            {
                return Err(invalid_data("invalid or duplicate glass workload field"));
            }
        }
        let exact = [
            "arm_timeout_seconds",
            "cmux_hook_events",
            "hold_after_trace_ms",
            "max_load_per_logical_cpu_milli",
            "min_free_memory_percent",
            "sample_interval_ms",
            "sessions",
            "steady_window_ms",
            "trace",
            "trace_last_event_us",
            "trace_records_per_session",
        ];
        let actual = fields.keys().copied().collect::<Vec<_>>();
        if actual != exact {
            return Err(invalid_data("glass workload exact field set differs"));
        }
        let workload = Self {
            sessions: parse_field(&fields, "sessions")?,
            trace: PathBuf::from(fields["trace"]),
            trace_records_per_session: parse_field(&fields, "trace_records_per_session")?,
            trace_last_event_us: parse_field(&fields, "trace_last_event_us")?,
            hold_after_trace_ms: parse_field(&fields, "hold_after_trace_ms")?,
            steady_window_ms: parse_field(&fields, "steady_window_ms")?,
            sample_interval_ms: parse_field(&fields, "sample_interval_ms")?,
            arm_timeout_seconds: parse_field(&fields, "arm_timeout_seconds")?,
            max_load_per_logical_cpu_milli: parse_field(&fields, "max_load_per_logical_cpu_milli")?,
            min_free_memory_percent: parse_field(&fields, "min_free_memory_percent")?,
            cmux_hook_events: parse_field(&fields, "cmux_hook_events")?,
        };
        workload.validate()?;
        Ok(workload)
    }

    fn validate(&self) -> io::Result<()> {
        if self.sessions != 20
            || self.trace_records_per_session != 14
            || self.trace_last_event_us == 0
            || self.hold_after_trace_ms < self.steady_window_ms
            || self.steady_window_ms < 500
            || self.sample_interval_ms == 0
            || self.sample_interval_ms > 100
            || self.arm_timeout_seconds < 10
            || self.arm_timeout_seconds > 120
            || self.max_load_per_logical_cpu_milli == 0
            || self.max_load_per_logical_cpu_milli > 2_000
            || self.min_free_memory_percent == 0
            || self.min_free_memory_percent > 100
            || self.cmux_hook_events < 3
            || self.cmux_hook_events > 20
            || self.trace != Path::new("bench/traces/w4-working-session.trace")
        {
            return Err(invalid_data("glass workload violates frozen safety bounds"));
        }
        Ok(())
    }
}

/// Machine state sampled immediately before one serial subject arm.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassMachineState {
    pub load_one: f64,
    pub logical_cpus: u32,
    pub free_memory_percent: u8,
}

/// Refuses contended or memory-constrained launches at the preregistered boundary.
pub fn admit_machine(state: &GlassMachineState, workload: &GlassWorkload) -> io::Result<()> {
    if !state.load_one.is_finite() || state.logical_cpus == 0 {
        return Err(invalid_data("machine admission sample is invalid"));
    }
    let maximum_load =
        f64::from(state.logical_cpus) * workload.max_load_per_logical_cpu_milli as f64 / 1_000.0;
    if state.load_one > maximum_load {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "one-minute load {:.2} exceeds preregistered {:.2}",
                state.load_one, maximum_load
            ),
        ));
    }
    if state.free_memory_percent < workload.min_free_memory_percent {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "free memory {}% is below preregistered {}%",
                state.free_memory_percent, workload.min_free_memory_percent
            ),
        ));
    }
    Ok(())
}

/// Enforces the exact resident terminal/session shape before measurement.
pub fn require_session_shape(actual: u32, workload: &GlassWorkload) -> io::Result<()> {
    if actual == workload.sessions {
        Ok(())
    } else {
        Err(invalid_data(format!(
            "subject exposed {actual}/{} W5 sessions",
            workload.sessions
        )))
    }
}

/// One same-machine terminal-layer observation.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassArmResult {
    pub subject: String,
    pub version: String,
    pub availability: String,
    pub workload_mode: String,
    pub sessions_requested: u32,
    pub sessions_observed: Option<u32>,
    pub preexisting_sessions: Option<u32>,
    pub events_per_session: Option<u64>,
    pub baseline_rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub steady_rss_bytes: Option<u64>,
    pub steady_rss_samples: Vec<u64>,
    pub distinct_pid_count: Option<u64>,
    pub steady_pid_count: Option<u64>,
    pub fleet_spawn_wall_us: Option<u64>,
    pub spawn_latency_us: Option<Percentiles>,
    pub reproduction: Option<String>,
    pub capability_gaps: String,
    pub raw_artifact: String,
}

/// Repeated live invocation of cmux's current hook entrypoint.
#[derive(Clone, Debug, PartialEq)]
pub struct CmuxHookResult {
    pub version: String,
    pub events: u32,
    pub distinct_hook_pids: u64,
    pub per_event_forks: f64,
    pub peak_hook_rss_bytes: u64,
    pub wall_latency_us: Percentiles,
    pub command: String,
    pub raw_artifact: String,
    pub observation: String,
}

/// Complete W5 report; every arm has a raw artifact even when unavailable.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassReport {
    pub run_id: String,
    pub generated_unix_seconds: u64,
    pub machine: crate::Machine,
    pub workload_sha256: String,
    pub workload_path: String,
    pub preflight_artifact: String,
    pub arms: Vec<GlassArmResult>,
    pub cmux_hook: CmuxHookResult,
}

/// Deterministic machine-readable report.
pub fn render_glass_json(report: &GlassReport) -> String {
    let mut output = format!(
        "{{\n  \"schema_version\": 1,\n  \"run_id\": {},\n  \"generated_unix_seconds\": {},\n  \"machine\": {{\"os\": {}, \"architecture\": {}, \"rustc\": {}}},\n  \"workload\": {{\"path\": {}, \"sha256\": {}, \"preflight_artifact\": {}}},\n  \"arms\": [\n",
        quote(&report.run_id),
        report.generated_unix_seconds,
        quote(&report.machine.os),
        quote(&report.machine.architecture),
        quote(&report.machine.rustc),
        quote(&report.workload_path),
        quote(&report.workload_sha256),
        quote(&report.preflight_artifact)
    );
    for (index, arm) in report.arms.iter().enumerate() {
        output.push_str(&format!(
            "    {{\"subject\": {}, \"version\": {}, \"availability\": {}, \"workload_mode\": {}, \"sessions_requested\": {}, \"sessions_observed\": {}, \"preexisting_sessions\": {}, \"events_per_session\": {}, \"baseline_rss_bytes\": {}, \"peak_rss_bytes\": {}, \"steady_rss_bytes\": {}, \"steady_rss_samples\": {}, \"distinct_pid_count\": {}, \"steady_pid_count\": {}, \"fleet_spawn_wall_us\": {}, \"spawn_latency_us\": {}, \"reproduce\": {}, \"capability_gaps\": {}, \"raw_artifact\": {}}}{}\n",
            quote(&arm.subject), quote(&arm.version), quote(&arm.availability),
            quote(&arm.workload_mode), arm.sessions_requested,
            optional_u32_json(arm.sessions_observed), optional_u32_json(arm.preexisting_sessions),
            optional_u64_json(arm.events_per_session), optional_u64_json(arm.baseline_rss_bytes),
            optional_u64_json(arm.peak_rss_bytes), optional_u64_json(arm.steady_rss_bytes),
            list_u64_json(&arm.steady_rss_samples), optional_u64_json(arm.distinct_pid_count),
            optional_u64_json(arm.steady_pid_count), optional_u64_json(arm.fleet_spawn_wall_us),
            optional_percentiles_json(arm.spawn_latency_us.as_ref()),
            arm.reproduction.as_deref().map_or_else(|| "null".to_owned(), quote),
            quote(&arm.capability_gaps), quote(&arm.raw_artifact),
            if index + 1 == report.arms.len() { "" } else { "," }
        ));
    }
    let hook = &report.cmux_hook;
    output.push_str(&format!(
        "  ],\n  \"cmux_hook\": {{\"version\": {}, \"events\": {}, \"distinct_hook_pids\": {}, \"per_event_forks\": {:.6}, \"peak_hook_rss_bytes\": {}, \"wall_latency_us\": {{\"p50\": {}, \"p95\": {}, \"p99\": {}}}, \"command\": {}, \"raw_artifact\": {}, \"observation\": {}}}\n}}\n",
        quote(&hook.version), hook.events, hook.distinct_hook_pids, hook.per_event_forks,
        hook.peak_hook_rss_bytes, hook.wall_latency_us.p50, hook.wall_latency_us.p95,
        hook.wall_latency_us.p99, quote(&hook.command), quote(&hook.raw_artifact),
        quote(&hook.observation)
    ));
    output
}

/// Publishable comparison plus the founder-owned glass recommendation.
pub fn render_glass_markdown(report: &GlassReport, json_path: &str) -> String {
    let mut output = format!(
        "# W5 terminal-layer glass receipts\n\nSame-machine serial run `{}` at Unix time `{}` on `{}` / `{}`. Frozen workload `{}` SHA-256 `{}`; machine gate and negative controls: `{}`. Machine-readable receipt: [`{}`]({}).\n\nAll RSS values are externally sampled complete subject trees. Peak covers launch + trace replay; steady is the median post-trace resident sample. Distinct PID is the observed union, not an asserted spawn count. N/A means the subject exposes no equivalent receipt or was absent; it is never an estimate.\n\n",
        report.run_id, report.generated_unix_seconds, report.machine.os,
        report.machine.architecture, report.workload_path, report.workload_sha256,
        report.preflight_artifact, json_path, json_path
    );
    output.push_str("| Terminal layer | Availability | Workload | Sessions | Pre-existing | Peak RSS (MiB) | Steady RSS (MiB) | Distinct / steady PIDs | Fleet spawn (ms) | Per-session ack p95 (ms) | Capability gaps | Raw |\n");
    output.push_str("|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|---|\n");
    for arm in &report.arms {
        output.push_str(&format!(
            "| {} `{}` | {} | {} | {} | {} | {} | {} | {} / {} | {} | {} | {} | `{}` |\n",
            arm.subject,
            arm.version,
            arm.availability,
            arm.workload_mode,
            optional_u32_markdown(arm.sessions_observed),
            optional_u32_markdown(arm.preexisting_sessions),
            optional_bytes_markdown(arm.peak_rss_bytes),
            optional_bytes_markdown(arm.steady_rss_bytes),
            optional_u64_markdown(arm.distinct_pid_count),
            optional_u64_markdown(arm.steady_pid_count),
            optional_millis_markdown(arm.fleet_spawn_wall_us),
            arm.spawn_latency_us.as_ref().map_or_else(
                || "N/A".to_owned(),
                |value| format!("{:.3}", value.p95 as f64 / 1_000.0)
            ),
            arm.capability_gaps,
            arm.raw_artifact
        ));
    }
    let hook = &report.cmux_hook;
    output.push_str(&format!(
        "\n## cmux hook architecture refresh\n\n**Behavior observed, no code copied.** Installed cmux `{}` accepted `{}` live `hooks pi event` invocations against its running Unix socket. External sampling observed `{}` distinct hook CLI PIDs (`{:.1}` fork/event), `{:.2}`MiB peak per-hook RSS, and {:.3}/{:.3}/{:.3}ms wall p50/p95/p99. {} Raw: `{}`.\n\n",
        hook.version, hook.events, hook.distinct_hook_pids, hook.per_event_forks,
        bytes_to_mib(hook.peak_hook_rss_bytes), hook.wall_latency_us.p50 as f64 / 1_000.0,
        hook.wall_latency_us.p95 as f64 / 1_000.0, hook.wall_latency_us.p99 as f64 / 1_000.0,
        hook.observation, hook.raw_artifact
    ));
    output.push_str("## Glass verdict (recommendation; founder decides)\n\nBuild-vs-adopt threshold: build a renderer only for a required capability an adoptable path cannot supply, or for a measured roughly **10x** real-consumer outcome advantage. A self-authored fleet microbenchmark certifies machinery and cost; it does not by itself prove a 10x product outcome.\n\n");
    output.push_str(
        "| Candidate path | What receipts justify now | Recommendation | Re-open gate |\n",
    );
    output.push_str("|---|---|---|---|\n");
    output.push_str("| std-only TUI as-is | Already owns passive restore, exact capabilities, attestation, idle/redraw gates, and the measured remux arm without a renderer dependency. | **Stay TUI now.** Preserve the lowest-complexity substrate while real consumers exercise the public run route. | A production workflow shows a glass limitation with accepted-outcome, latency, or retention harm. |\n");
    output.push_str("| embed libghostty | Ghostty vanilla supplies the nearest glass baseline and mature Metal terminal behavior, but W5 measures an app binary, not libghostty ABI/integration cost; persistence and remux attestation remain ours. | **Shadow as the first glass adoption candidate; do not adopt this wave.** | Offline prototype measures end-to-end RSS/redraw/build surface and beats TUI on a named consumer enough to pay its dependency/FFI tax. |\n");
    output.push_str("| narrow Metal renderer | Infinitty is an existence proof for a small resident Metal terminal, not evidence that rebuilding terminal correctness creates a 10x consumer gain. It adds the largest bespoke maintenance surface. | **Park.** Build-vs-adopt law is not met. | libghostty cannot satisfy a required measured capability, and a narrow prototype demonstrates roughly 10x outcome advantage under a fixed budget. |\n");
    output
        .push_str("\nNo renderer dependency, FFI, telemetry, or display code was adopted in W5.\n");
    output
}

fn parse_field<T>(fields: &BTreeMap<&str, &str>, key: &str) -> io::Result<T>
where
    T: std::str::FromStr,
{
    fields[key]
        .parse()
        .map_err(|_| invalid_data(format!("invalid glass workload {key}")))
}

fn optional_u32_json(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn optional_u64_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn list_u64_json(values: &[u64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn optional_percentiles_json(value: Option<&Percentiles>) -> String {
    value.map_or_else(
        || "null".to_owned(),
        |percentiles| {
            format!(
                "{{\"p50\": {}, \"p95\": {}, \"p99\": {}}}",
                percentiles.p50, percentiles.p95, percentiles.p99
            )
        },
    )
}

fn optional_u32_markdown(value: Option<u32>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |number| number.to_string())
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

fn optional_millis_markdown(value: Option<u64>) -> String {
    value.map_or_else(
        || "N/A".to_owned(),
        |micros| format!("{:.3}", micros as f64 / 1_000.0),
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

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::{admit_machine, require_session_shape, GlassMachineState, GlassWorkload};

    const VALID: &str = "REMUX_GLASS_WORKLOAD_V1\nsessions\t20\ntrace\tbench/traces/w4-working-session.trace\ntrace_records_per_session\t14\ntrace_last_event_us\t3736680\nhold_after_trace_ms\t10000\nsteady_window_ms\t1000\nsample_interval_ms\t20\narm_timeout_seconds\t45\nmax_load_per_logical_cpu_milli\t1000\nmin_free_memory_percent\t25\ncmux_hook_events\t5\n";

    #[test]
    fn frozen_workload_parses_and_every_truncation_fails() {
        let workload = GlassWorkload::parse(VALID).expect("parse frozen W5 workload");
        assert_eq!(workload.sessions, 20);
        assert_eq!(workload.trace_records_per_session, 14);
        for length in 0..VALID.len() {
            assert!(
                GlassWorkload::parse(&VALID[..length]).is_err(),
                "accepted truncated workload at byte {length}"
            );
        }
    }

    #[test]
    fn malformed_duplicate_extra_and_unsafe_workloads_fail_closed() {
        for malformed in [
            VALID.replacen("sessions\t20", "sessions\t19", 1),
            VALID.replacen("sessions\t20\n", "sessions\t20\nsessions\t20\n", 1),
            VALID.replacen("cmux_hook_events\t5", "unknown\t1", 1),
            VALID.replacen("sample_interval_ms\t20", "sample_interval_ms\t0", 1),
            VALID.replacen(
                "min_free_memory_percent\t25",
                "min_free_memory_percent\t101",
                1,
            ),
            VALID.trim_end().to_owned(),
        ] {
            assert!(GlassWorkload::parse(&malformed).is_err());
        }
    }

    #[test]
    fn admission_and_shape_negative_controls_reject_both_boundaries() {
        let workload = GlassWorkload::parse(VALID).expect("parse admission workload");
        admit_machine(
            &GlassMachineState {
                load_one: 10.0,
                logical_cpus: 10,
                free_memory_percent: 25,
            },
            &workload,
        )
        .expect("inclusive registered boundary passes");
        assert!(admit_machine(
            &GlassMachineState {
                load_one: 10.01,
                logical_cpus: 10,
                free_memory_percent: 25,
            },
            &workload,
        )
        .is_err());
        assert!(admit_machine(
            &GlassMachineState {
                load_one: 1.0,
                logical_cpus: 10,
                free_memory_percent: 24,
            },
            &workload,
        )
        .is_err());
        require_session_shape(20, &workload).expect("exact shape passes");
        assert!(require_session_shape(19, &workload).is_err());
        assert!(require_session_shape(21, &workload).is_err());
    }
}
