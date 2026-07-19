// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Synthetic fleet orchestrator.
//!
//! Contract: the harness imports no supervisor internals. It drives compiled binaries
//! and the public Unix-socket control protocol, then writes publishable receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bench::system::{self, ResourceTracker};
use bench::{
    bytes_to_mib, percentiles, render_json, render_markdown, BenchmarkConfig, BenchmarkReport,
    InfinittyScenarioResult, Machine, ScenarioResult, TraceScenarioResult, TuiScenarioResult,
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);
const SUPERVISOR_RSS_LIMIT_BYTES: u64 = 200 * 1024 * 1024;
const BENCHMARK_BUDGET_SECONDS: f64 = 300.0;
const FORK_MEMORY_RAIL_MIB: u64 = 512;
const TUI_IDLE_WINDOW: Duration = Duration::from_secs(60);
const TUI_INITIAL_IDLE_MS: u64 = 65_000;
const TUI_IDLE_CPU_LIMIT_PERCENT: f64 = 0.5;
const TUI_REDRAW_P95_LIMIT_US: u64 = 50_000;
const ATTESTATION_REDRAW_OVERHEAD_LIMIT_US: u64 = 5_000;
const REAL_TRACE_RELATIVE_PATH: &str = "bench/traces/w4-working-session.trace";

fn main() {
    if let Err(error) = run() {
        eprintln!("bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse(env::args().skip(1))?;
    config.validate()?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("bench crate has no workspace parent")?;
    prepare_binaries(workspace)?;
    let binary_directory = env::current_exe()?
        .parent()
        .ok_or("bench executable has no parent")?
        .to_path_buf();
    let command = config.reproduction_command();

    let fork_result = run_fork_baseline(
        &config,
        &binary_directory.join("fork-worker"),
        command.clone(),
    )?;
    let scripted_result = run_supervisor_baseline(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("fake-agent"),
        SupervisorAgent::Scripted,
        command.clone(),
    )?;
    let real_agent_result = run_supervisor_baseline(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("fake-agent"),
        SupervisorAgent::RealShell,
        command.clone(),
    )?;
    let tui_result = run_tui_baseline(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("fake-agent"),
        command.clone(),
    )?;
    let trace_path = workspace.join(REAL_TRACE_RELATIVE_PATH);
    let trace_metadata = parse_trace_metadata(&trace_path)?;
    let trace_before = run_trace_replay(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("trace-agent"),
        &trace_path,
        &trace_metadata,
        TraceAttestation::Off,
        command.clone(),
    )?;
    let trace_after = run_trace_replay(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("trace-agent"),
        &trace_path,
        &trace_metadata,
        TraceAttestation::HashChain,
        command.clone(),
    )?;
    let infinitty_result = run_infinitty_baseline(config.sessions, command)?;
    for result in [&scripted_result, &real_agent_result] {
        if result.peak_rss_bytes >= SUPERVISOR_RSS_LIMIT_BYTES {
            return Err(format!(
                "{} peak RSS {:.2}MiB exceeds 200MiB contract",
                result.model,
                bytes_to_mib(result.peak_rss_bytes)
            )
            .into());
        }
        if result.per_event_forks != 0 {
            return Err(format!("{} measured per-event forks", result.model).into());
        }
    }
    if tui_result.total_peak_rss_bytes >= SUPERVISOR_RSS_LIMIT_BYTES {
        return Err(format!(
            "{} TUI-inclusive RSS {:.2}MiB exceeds 200MiB contract",
            tui_result.model,
            bytes_to_mib(tui_result.total_peak_rss_bytes)
        )
        .into());
    }
    if tui_result.per_event_forks != 0 {
        return Err("TUI scenario measured per-event forks".into());
    }
    if tui_result.idle_cpu_percent > TUI_IDLE_CPU_LIMIT_PERCENT {
        return Err(format!(
            "TUI idle CPU {:.3}% exceeds {:.3}% gate",
            tui_result.idle_cpu_percent, TUI_IDLE_CPU_LIMIT_PERCENT
        )
        .into());
    }
    if tui_result.redraw_latency_us.p95 >= TUI_REDRAW_P95_LIMIT_US {
        return Err(format!(
            "TUI redraw p95 {}us exceeds {}us gate",
            tui_result.redraw_latency_us.p95, TUI_REDRAW_P95_LIMIT_US
        )
        .into());
    }
    for trace in [&trace_before, &trace_after] {
        if trace.peak_rss_bytes >= SUPERVISOR_RSS_LIMIT_BYTES {
            return Err(format!("{} exceeds 200MiB memory rail", trace.model).into());
        }
        if trace.per_event_forks != 0 {
            return Err(format!("{} measured per-event forks", trace.model).into());
        }
        if trace.redraw_latency_us.p95 >= TUI_REDRAW_P95_LIMIT_US {
            return Err(format!("{} redraw p95 exceeds 50ms", trace.model).into());
        }
    }
    if trace_before.attestation_records != 0 || trace_before.attestation_file_bytes != 0 {
        return Err("unattested real-trace baseline emitted attestation data".into());
    }
    if trace_after.attestation_records == 0 || trace_after.attestation_file_bytes == 0 {
        return Err("attested real-trace replay emitted no attestation data".into());
    }
    let redraw_overhead = trace_after
        .redraw_latency_us
        .p95
        .saturating_sub(trace_before.redraw_latency_us.p95);
    if redraw_overhead > ATTESTATION_REDRAW_OVERHEAD_LIMIT_US {
        return Err(format!(
            "attestation redraw p95 overhead {redraw_overhead}us exceeds {ATTESTATION_REDRAW_OVERHEAD_LIMIT_US}us gate"
        )
        .into());
    }

    let generated_unix_seconds = unix_seconds()?;
    let run_id = format!("run-{generated_unix_seconds}-{}", std::process::id());
    let report = BenchmarkReport {
        run_id: run_id.clone(),
        generated_unix_seconds,
        machine: machine()?,
        config: config.report_config(),
        results: vec![fork_result, scripted_result, real_agent_result],
        tui_result: Some(tui_result),
        trace_results: vec![trace_before, trace_after],
        infinitty_result,
    };
    let results_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("results");
    fs::create_dir_all(&results_directory)?;
    let timestamped_name = format!("{run_id}.json");
    let timestamped_path = results_directory.join(&timestamped_name);
    let latest_path = results_directory.join("latest.json");
    let json = render_json(&report);
    write_atomic(&timestamped_path, &json)?;
    write_atomic(&latest_path, &json)?;
    let markdown = render_markdown(&report, &format!("results/{timestamped_name}"));
    let markdown_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("RESULTS.md");
    write_atomic(&markdown_path, &markdown)?;

    println!("wrote {}", markdown_path.display());
    println!("wrote {}", timestamped_path.display());
    for result in &report.results {
        println!(
            "{}: {} events, {} spawns, {:.2}MiB peak RSS, {:.2} events/s",
            result.model,
            result.events,
            result.processes_spawned,
            bytes_to_mib(result.peak_rss_bytes),
            result.events_per_second
        );
    }
    if let Some(result) = &report.tui_result {
        println!(
            "{}: {:.2}MiB TUI + {:.2}MiB children, {:.3}% idle CPU, {}us redraw p95",
            result.model,
            bytes_to_mib(result.tui_peak_rss_bytes),
            bytes_to_mib(result.child_agent_peak_rss_bytes),
            result.idle_cpu_percent,
            result.redraw_latency_us.p95
        );
    }
    for result in &report.trace_results {
        println!(
            "{}: {} real-trace events, {:.2}MiB, {}us redraw p95, {} attestation records",
            result.model,
            result.events,
            bytes_to_mib(result.peak_rss_bytes),
            result.redraw_latency_us.p95,
            result.attestation_records
        );
    }
    println!(
        "{}: {}",
        report.infinitty_result.model, report.infinitty_result.availability
    );
    Ok(())
}

fn prepare_binaries(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--workspace", "--offline", "--quiet"])
        .current_dir(workspace)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("workspace preparation failed with {status}").into())
    }
}

fn run_fork_baseline(
    config: &Config,
    worker: &Path,
    command: String,
) -> Result<ScenarioResult, Box<dyn std::error::Error>> {
    let total_events = config.total_events()?;
    let started = Instant::now();
    let timeout = Duration::from_secs_f64(config.estimated_fork_seconds() + 20.0);
    let mut active = Vec::<TrackedChild>::new();
    let mut spawned = 0_u64;
    let mut completed = 0_u64;
    let mut latencies = Vec::with_capacity(usize::try_from(total_events)?);
    let mut resources = ResourceTracker::default();
    let mut next_sample = Instant::now();
    while completed < total_events {
        if started.elapsed() > timeout {
            terminate_children(&mut active);
            return Err("fork-per-event baseline timed out".into());
        }
        while spawned < total_events
            && started.elapsed().as_secs_f64() >= spawned as f64 / config.rate as f64
        {
            let kind = match spawned % 3 {
                0 => "status",
                1 => "tool",
                _ => "output",
            };
            let child = Command::new(worker)
                .args([
                    "--hold-ms",
                    &config.fork_hold_ms.to_string(),
                    "--cpu-ms",
                    &config.fork_cpu_ms.to_string(),
                    "--rss-mib",
                    &config.fork_rss_mib.to_string(),
                    "--kind",
                    kind,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            active.push(TrackedChild {
                child,
                started: Instant::now(),
            });
            spawned += 1;
        }
        if Instant::now() >= next_sample {
            let snapshot = system::snapshot()?;
            let selected = active
                .iter()
                .map(|tracked| tracked.child.id())
                .collect::<BTreeSet<_>>();
            resources.observe(&snapshot, &selected);
            next_sample = Instant::now()
                .checked_add(SAMPLE_INTERVAL)
                .ok_or("sample deadline overflow")?;
        }
        for index in (0..active.len()).rev() {
            if let Some(status) = active[index].child.try_wait()? {
                if !status.success() {
                    terminate_children(&mut active);
                    return Err(format!("fork worker exited with {status}").into());
                }
                let latency = u64::try_from(active[index].started.elapsed().as_micros())?;
                latencies.push(latency);
                active.swap_remove(index);
                completed += 1;
            }
        }
        if completed < total_events {
            thread::sleep(Duration::from_millis(2));
        }
    }
    let wall_seconds = started.elapsed().as_secs_f64();
    let latency_us = percentiles(&latencies)?;
    let peak_rss_bytes = resources.peak_rss_bytes();
    let processes_spawned = resources.distinct_pid_count();
    if processes_spawned != spawned {
        return Err(
            format!("process sampler observed {processes_spawned}/{spawned} fork workers").into(),
        );
    }
    Ok(ScenarioResult {
        model: "fork_per_event".to_owned(),
        sessions: config.sessions,
        events: total_events,
        processes_spawned,
        per_event_forks: processes_spawned,
        peak_rss_bytes,
        events_per_second: total_events as f64 / wall_seconds,
        latency_us: latency_us.clone(),
        cpu_seconds: total_events as f64 * config.fork_cpu_ms as f64 / 1_000.0,
        cpu_source: "configured-by-construction (events × --fork-cpu-ms)".to_owned(),
        wall_seconds,
        command,
        interpretation: format!(
            "Distinct-PID sampling measured {processes_spawned} event workers; p50 completion was {:.1}ms versus the configured {}ms hold.",
            latency_us.p50 as f64 / 1_000.0,
            config.fork_hold_ms
        ),
    })
}

#[derive(Clone, Copy)]
enum SupervisorAgent {
    Scripted,
    RealShell,
}

impl SupervisorAgent {
    fn argument(self) -> &'static str {
        match self {
            Self::Scripted => "scripted",
            Self::RealShell => "real-shell",
        }
    }

    fn model(self) -> &'static str {
        match self {
            Self::Scripted => "scripted_socket_supervisor",
            Self::RealShell => "real_shell_socket_supervisor",
        }
    }
}

fn run_supervisor_baseline(
    config: &Config,
    supervisor: &Path,
    fake_agent: &Path,
    agent: SupervisorAgent,
    command: String,
) -> Result<ScenarioResult, Box<dyn std::error::Error>> {
    let temporary = TemporaryDirectory::new()?;
    let socket = temporary.path.join("s");
    let state = temporary.path.join("state.json");
    let scrollback = temporary.path.join("scrollback");
    let attestations = temporary.path.join("attestations");
    let metrics = temporary.path.join("metrics.tsv");
    let ready = temporary.path.join("ready.tsv");
    let auth_log = temporary.path.join("attach.log");
    let auth_token = format!("bench-{}-{}", agent.argument(), std::process::id());
    let drive_token = format!("bench-drive-{}-{}", agent.argument(), std::process::id());
    authorize(supervisor, &auth_log, &auth_token, "launch")?;
    authorize(supervisor, &auth_log, &drive_token, "drive")?;
    let mut child = Command::new(supervisor)
        .args([
            "run",
            "--sessions",
            &config.sessions.to_string(),
            "--events-per-session",
            &config.events_per_session.to_string(),
            "--rate",
            &config.rate.to_string(),
            "--agent-kind",
            agent.argument(),
            "--socket",
            path_text(&socket)?,
            "--state",
            path_text(&state)?,
            "--scrollback-dir",
            path_text(&scrollback)?,
            "--attestation-dir",
            path_text(&attestations)?,
            "--metrics",
            path_text(&metrics)?,
            "--ready",
            path_text(&ready)?,
            "--fake-agent",
            path_text(fake_agent)?,
            "--timeout-seconds",
            &config.supervisor_timeout_seconds().to_string(),
            "--auth-log",
            path_text(&auth_log)?,
            "--attach-token",
            &auth_token,
            "--attach-scope",
            "launch",
            "--drive-token",
            &drive_token,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let started = Instant::now();
    let timeout = Duration::from_secs_f64(config.estimated_supervisor_seconds() + 20.0);
    let mut resources = ResourceTracker::default();
    let mut authorized_pids = None;
    let mut dump_sent = false;
    let status = loop {
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err("socket supervisor baseline timed out".into());
        }
        let snapshot = system::snapshot()?;
        let selected = system::descendants(&snapshot, root_pid);
        resources.observe(&snapshot, &selected);
        if ready.exists() && authorized_pids.is_none() {
            authorized_pids = Some(parse_ready_pids(&ready, root_pid, config.sessions)?);
        }
        if authorized_pids.is_some() && !dump_sent {
            let mut stream = UnixStream::connect(&socket)?;
            stream.write_all(b"control\tdump\n")?;
            dump_sent = true;
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(SAMPLE_INTERVAL);
    };
    let wall_seconds = started.elapsed().as_secs_f64();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    if !status.success() {
        return Err(format!("supervisor exited with {status}: {}", stderr.trim()).into());
    }
    if !dump_sent {
        return Err("supervisor exited before accepting on-demand dump".into());
    }
    let authorized_pids = authorized_pids.ok_or("supervisor produced no ready PID receipt")?;
    let missed = authorized_pids
        .difference(resources.observed_pids())
        .copied()
        .collect::<Vec<_>>();
    if !missed.is_empty() {
        return Err(format!("process sampler missed authorized startup PIDs {missed:?}").into());
    }
    let per_event_forks = u64::try_from(
        resources
            .observed_pids()
            .difference(&authorized_pids)
            .count(),
    )?;

    let parsed = parse_metrics(&metrics)?;
    let total_events = config.total_events()?;
    expect_metric(&parsed, "schema", 3)?;
    expect_metric(&parsed, "events_ingested", total_events)?;
    expect_metric(&parsed, "children_spawned", u64::from(config.sessions))?;
    if parsed.get("agent_kind").map(String::as_str) != Some(agent.argument()) {
        return Err("supervisor agent-kind receipt differs from requested scenario".into());
    }
    if metric(&parsed, "on_demand_dumps")? == 0 {
        return Err("on-demand state dump was not observed".into());
    }
    if metric(&parsed, "pty_bytes")? == 0 {
        return Err("PTY sessions produced no output".into());
    }
    let latencies = parse_metric_samples(&parsed, "latencies_us")?;
    if latencies.len() != usize::try_from(total_events)? {
        return Err("latency sample count differs from event count".into());
    }
    verify_passive_restore(supervisor, &state)?;

    let latency_us = percentiles(&latencies)?;
    let peak_rss_bytes = resources.peak_rss_bytes();
    let processes_spawned = resources.distinct_pid_count();
    Ok(ScenarioResult {
        model: agent.model().to_owned(),
        sessions: config.sessions,
        events: total_events,
        processes_spawned,
        per_event_forks,
        peak_rss_bytes,
        events_per_second: total_events as f64 / wall_seconds,
        latency_us,
        cpu_seconds: resources.cpu_seconds(),
        cpu_source: "sampled cumulative process-tree CPU via ps".to_owned(),
        wall_seconds,
        command,
        interpretation: format!(
            "{} attached PTY sessions ingested {total_events} events through one socket; distinct-PID sampling found {processes_spawned} processes and {per_event_forks} event forks at {:.2}MiB peak RSS.",
            config.sessions,
            bytes_to_mib(peak_rss_bytes)
        ),
    })
}

fn run_tui_baseline(
    config: &Config,
    supervisor: &Path,
    fake_agent: &Path,
    command: String,
) -> Result<TuiScenarioResult, Box<dyn std::error::Error>> {
    let temporary = TemporaryDirectory::new()?;
    let socket = temporary.path.join("s");
    let state = temporary.path.join("state.json");
    let scrollback = temporary.path.join("scrollback");
    let attestations = temporary.path.join("attestations");
    let metrics = temporary.path.join("metrics.tsv");
    let ready = temporary.path.join("ready.tsv");
    let auth_log = temporary.path.join("attach.log");
    let tui_output = temporary.path.join("tui.ansi");
    let auth_token = format!("bench-tui-{}", std::process::id());
    let drive_token = format!("bench-tui-drive-{}", std::process::id());
    authorize(supervisor, &auth_log, &auth_token, "launch")?;
    authorize(supervisor, &auth_log, &drive_token, "drive")?;
    let timeout_seconds = TUI_INITIAL_IDLE_MS
        .div_ceil(1_000)
        .saturating_add(config.supervisor_timeout_seconds())
        .saturating_add(20);
    let mut child = Command::new(supervisor)
        .args([
            "run",
            "--sessions",
            &config.sessions.to_string(),
            "--events-per-session",
            &config.events_per_session.to_string(),
            "--rate",
            &config.rate.to_string(),
            "--agent-kind",
            "real-shell",
            "--socket",
            path_text(&socket)?,
            "--state",
            path_text(&state)?,
            "--scrollback-dir",
            path_text(&scrollback)?,
            "--attestation-dir",
            path_text(&attestations)?,
            "--metrics",
            path_text(&metrics)?,
            "--ready",
            path_text(&ready)?,
            "--fake-agent",
            path_text(fake_agent)?,
            "--timeout-seconds",
            &timeout_seconds.to_string(),
            "--auth-log",
            path_text(&auth_log)?,
            "--attach-token",
            &auth_token,
            "--attach-scope",
            "launch",
            "--drive-token",
            &drive_token,
            "--initial-idle-ms",
            &TUI_INITIAL_IDLE_MS.to_string(),
            "--tui-output",
            path_text(&tui_output)?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let started = Instant::now();
    let result = (|| -> Result<TuiScenarioResult, Box<dyn std::error::Error>> {
        let root_set = BTreeSet::from([root_pid]);
        let startup_deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("TUI startup deadline overflow")?;
        let mut total_resources = ResourceTracker::default();
        let mut tui_resources = ResourceTracker::default();
        let authorized_pids = loop {
            let snapshot = system::snapshot()?;
            let selected = system::descendants(&snapshot, root_pid);
            total_resources.observe(&snapshot, &selected);
            tui_resources.observe(&snapshot, &root_set);
            if ready.exists() {
                break parse_ready_pids(&ready, root_pid, config.sessions)?;
            }
            if let Some(status) = child.try_wait()? {
                return Err(format!("TUI supervisor exited during startup with {status}").into());
            }
            if Instant::now() >= startup_deadline {
                return Err("TUI supervisor did not become ready".into());
            }
            thread::sleep(SAMPLE_INTERVAL);
        };
        let child_pids = authorized_pids
            .iter()
            .copied()
            .filter(|pid| *pid != root_pid)
            .collect::<BTreeSet<_>>();
        let mut child_resources = ResourceTracker::default();
        let initial_snapshot = system::snapshot()?;
        observe_tui_resources(
            &initial_snapshot,
            root_pid,
            &child_pids,
            &mut total_resources,
            &mut tui_resources,
            &mut child_resources,
        );
        let idle_cpu_start = system::selected_cpu_seconds(&initial_snapshot, &root_set);
        let initial_frame_bytes = fs::metadata(&tui_output)?.len();
        if initial_frame_bytes == 0 {
            return Err("TUI produced no initial frame before ready".into());
        }

        let idle_started = Instant::now();
        let idle_deadline = idle_started
            .checked_add(TUI_IDLE_WINDOW)
            .ok_or("TUI idle deadline overflow")?;
        let (idle_final_snapshot, idle_window_seconds) = loop {
            let snapshot = system::snapshot()?;
            observe_tui_resources(
                &snapshot,
                root_pid,
                &child_pids,
                &mut total_resources,
                &mut tui_resources,
                &mut child_resources,
            );
            if let Some(status) = child.try_wait()? {
                return Err(format!("TUI supervisor exited during idle with {status}").into());
            }
            let now = Instant::now();
            if now >= idle_deadline {
                break (snapshot, idle_started.elapsed().as_secs_f64());
            }
            let remaining = idle_deadline.saturating_duration_since(now);
            thread::sleep(remaining.min(Duration::from_millis(100)));
        };
        let idle_cpu_end = system::selected_cpu_seconds(&idle_final_snapshot, &root_set);
        let idle_cpu_seconds = (idle_cpu_end - idle_cpu_start).max(0.0);
        let idle_cpu_percent = idle_cpu_seconds / idle_window_seconds * 100.0;
        if fs::metadata(&tui_output)?.len() != initial_frame_bytes {
            return Err("TUI emitted a frame without a state event during idle".into());
        }

        let process_deadline = started
            .checked_add(Duration::from_secs(timeout_seconds.saturating_add(10)))
            .ok_or("TUI process deadline overflow")?;
        let status = loop {
            let snapshot = system::snapshot()?;
            observe_tui_resources(
                &snapshot,
                root_pid,
                &child_pids,
                &mut total_resources,
                &mut tui_resources,
                &mut child_resources,
            );
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= process_deadline {
                return Err("TUI supervisor timed out after idle window".into());
            }
            thread::sleep(SAMPLE_INTERVAL);
        };
        let wall_seconds = started.elapsed().as_secs_f64();
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_string(&mut stderr)?;
        }
        if !status.success() {
            return Err(format!("TUI supervisor exited with {status}: {}", stderr.trim()).into());
        }

        let missed = authorized_pids
            .difference(total_resources.observed_pids())
            .copied()
            .collect::<Vec<_>>();
        if !missed.is_empty() {
            return Err(format!("TUI process sampler missed authorized PIDs {missed:?}").into());
        }
        let per_event_forks = u64::try_from(
            total_resources
                .observed_pids()
                .difference(&authorized_pids)
                .count(),
        )?;
        let parsed = parse_metrics(&metrics)?;
        let total_events = config.total_events()?;
        expect_metric(&parsed, "schema", 3)?;
        expect_metric(&parsed, "events_ingested", total_events)?;
        expect_metric(&parsed, "children_spawned", u64::from(config.sessions))?;
        if parsed.get("agent_kind").map(String::as_str) != Some("real-shell") {
            return Err("TUI receipt did not use real-shell agents".into());
        }
        let redraw_latencies = parse_metric_samples(&parsed, "redraw_latencies_us")?;
        if redraw_latencies.len() != usize::try_from(total_events)? {
            return Err("TUI redraw latency count differs from event count".into());
        }
        let frames_rendered = metric(&parsed, "frames_rendered")?;
        if frames_rendered < 2 || frames_rendered > total_events.saturating_add(1) {
            return Err("TUI frame count is outside event-driven bounds".into());
        }
        let output = fs::read_to_string(&tui_output)?;
        for index in 0..config.sessions {
            if !output.contains(&format!("session-{index:03}")) {
                return Err(format!("TUI output misses session-{index:03}").into());
            }
        }
        if !output.contains("AGENT DRIVING") {
            return Err("TUI output misses agent-driving indicator".into());
        }
        verify_passive_restore(supervisor, &state)?;

        let redraw_latency_us = percentiles(&redraw_latencies)?;
        let tui_peak_rss_bytes = tui_resources.peak_rss_bytes();
        let child_agent_peak_rss_bytes = child_resources.peak_rss_bytes();
        let total_peak_rss_bytes = total_resources.peak_rss_bytes();
        let processes_spawned = total_resources.distinct_pid_count();
        Ok(TuiScenarioResult {
            model: "real_shell_tui".to_owned(),
            sessions: config.sessions,
            events: total_events,
            processes_spawned,
            per_event_forks,
            tui_peak_rss_bytes,
            child_agent_peak_rss_bytes,
            total_peak_rss_bytes,
            idle_window_seconds,
            idle_cpu_seconds,
            idle_cpu_percent,
            idle_frames_rendered: 0,
            redraw_latency_us,
            frames_rendered,
            wall_seconds,
            command,
            interpretation: format!(
                "One event-driven ANSI TUI rendered {} live tabs over {} authorized real shells; root and child RSS were sampled separately, with no frame bytes written during the 60s idle window.",
                config.sessions, config.sessions
            ),
        })
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[derive(Clone, Copy)]
enum TraceAttestation {
    Off,
    HashChain,
}

impl TraceAttestation {
    fn argument(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::HashChain => "hash-chain",
        }
    }

    fn model(self) -> &'static str {
        match self {
            Self::Off => "remux_real_trace_unattested",
            Self::HashChain => "remux_real_trace_hash_chain",
        }
    }

    fn enabled(self) -> bool {
        matches!(self, Self::HashChain)
    }
}

struct TraceMetadata {
    records_per_session: u64,
    command_sha256: String,
}

fn run_trace_replay(
    config: &Config,
    supervisor: &Path,
    trace_agent: &Path,
    trace_path: &Path,
    trace: &TraceMetadata,
    attestation: TraceAttestation,
    command: String,
) -> Result<TraceScenarioResult, Box<dyn std::error::Error>> {
    let temporary = TemporaryDirectory::new()?;
    let socket = temporary.path.join("s");
    let state = temporary.path.join("state.json");
    let scrollback = temporary.path.join("scrollback");
    let attestations = temporary.path.join("attestations");
    let metrics = temporary.path.join("metrics.tsv");
    let ready = temporary.path.join("ready.tsv");
    let auth_log = temporary.path.join("attach.log");
    let tui_output = temporary.path.join("trace.ansi");
    let suffix = attestation.argument();
    let auth_token = format!("bench-trace-{suffix}-{}", std::process::id());
    let drive_token = format!("bench-trace-drive-{suffix}-{}", std::process::id());
    authorize(supervisor, &auth_log, &auth_token, "launch")?;
    authorize(supervisor, &auth_log, &drive_token, "drive")?;
    let timeout_seconds = 30_u64;
    let mut child = Command::new(supervisor)
        .args([
            "run",
            "--sessions",
            &config.sessions.to_string(),
            "--events-per-session",
            &trace.records_per_session.to_string(),
            "--rate",
            &config.rate.to_string(),
            "--agent-kind",
            "trace-replay",
            "--trace-agent",
            path_text(trace_agent)?,
            "--trace",
            path_text(trace_path)?,
            "--socket",
            path_text(&socket)?,
            "--state",
            path_text(&state)?,
            "--scrollback-dir",
            path_text(&scrollback)?,
            "--attestation-dir",
            path_text(&attestations)?,
            "--attestation",
            attestation.argument(),
            "--metrics",
            path_text(&metrics)?,
            "--ready",
            path_text(&ready)?,
            "--timeout-seconds",
            &timeout_seconds.to_string(),
            "--auth-log",
            path_text(&auth_log)?,
            "--attach-token",
            &auth_token,
            "--attach-scope",
            "launch",
            "--drive-token",
            &drive_token,
            "--tui-output",
            path_text(&tui_output)?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let started = Instant::now();
    let deadline = started
        .checked_add(Duration::from_secs(timeout_seconds.saturating_add(10)))
        .ok_or("trace replay deadline overflow")?;
    let mut resources = ResourceTracker::default();
    let mut authorized_pids = None;
    let status = loop {
        let snapshot = system::snapshot()?;
        let selected = system::descendants(&snapshot, root_pid);
        resources.observe(&snapshot, &selected);
        if ready.exists() && authorized_pids.is_none() {
            authorized_pids = Some(parse_ready_pids(&ready, root_pid, config.sessions)?);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} timed out", attestation.model()).into());
        }
        thread::sleep(SAMPLE_INTERVAL);
    };
    let wall_seconds = started.elapsed().as_secs_f64();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    if !status.success() {
        return Err(format!(
            "{} exited with {status}: {}",
            attestation.model(),
            stderr.trim()
        )
        .into());
    }
    let authorized_pids = authorized_pids.ok_or("trace replay produced no ready PID receipt")?;
    let missed = authorized_pids
        .difference(resources.observed_pids())
        .copied()
        .collect::<Vec<_>>();
    if !missed.is_empty() {
        return Err(format!("trace sampler missed authorized PIDs {missed:?}").into());
    }
    let per_event_forks = u64::try_from(
        resources
            .observed_pids()
            .difference(&authorized_pids)
            .count(),
    )?;
    let parsed = parse_metrics(&metrics)?;
    let total_events = u64::from(config.sessions)
        .checked_mul(trace.records_per_session)
        .ok_or("real-trace event count overflow")?;
    expect_metric(&parsed, "schema", 3)?;
    expect_metric(&parsed, "events_ingested", total_events)?;
    expect_metric(&parsed, "children_spawned", u64::from(config.sessions))?;
    expect_metric(
        &parsed,
        "attestation_enabled",
        u64::from(attestation.enabled()),
    )?;
    if parsed.get("agent_kind").map(String::as_str) != Some("trace-replay") {
        return Err("real-trace receipt has wrong agent kind".into());
    }
    let ingest_samples = parse_metric_samples(&parsed, "latencies_us")?;
    let redraw_samples = parse_metric_samples(&parsed, "redraw_latencies_us")?;
    if ingest_samples.len() != usize::try_from(total_events)?
        || redraw_samples.len() != usize::try_from(total_events)?
    {
        return Err("real-trace latency count differs from event count".into());
    }
    let attestation_records = metric(&parsed, "attestation_records")?;
    let attestation_file_bytes = metric(&parsed, "attestation_file_bytes")?;
    if attestation.enabled() {
        verify_attestation_files(supervisor, &attestations, config.sessions)?;
        let heads = parsed
            .get("attestation_heads")
            .ok_or("attested replay misses chain heads")?;
        if heads.split(',').count() != usize::try_from(config.sessions)? {
            return Err("attested replay chain-head count differs from sessions".into());
        }
    }
    let output = fs::read_to_string(&tui_output)?;
    if !output.contains("AGENT DRIVING") {
        return Err("real-trace TUI misses explicit drive indicator".into());
    }
    verify_passive_restore(supervisor, &state)?;
    let processes_spawned = resources.distinct_pid_count();
    let peak_rss_bytes = resources.peak_rss_bytes();
    Ok(TraceScenarioResult {
        model: attestation.model().to_owned(),
        attestation_enabled: attestation.enabled(),
        trace_path: REAL_TRACE_RELATIVE_PATH.to_owned(),
        trace_command_sha256: trace.command_sha256.clone(),
        sessions: config.sessions,
        events: total_events,
        processes_spawned,
        per_event_forks,
        peak_rss_bytes,
        events_per_second: total_events as f64 / wall_seconds,
        ingest_latency_us: percentiles(&ingest_samples)?,
        redraw_latency_us: percentiles(&redraw_samples)?,
        cpu_seconds: resources.cpu_seconds(),
        attestation_records,
        attestation_file_bytes,
        wall_seconds,
        command,
        interpretation: format!(
            "Replayed {} records/session from a prior live project-working PTY capture across {} sessions; {} measured PIDs, {} event forks; attestation {}.",
            trace.records_per_session,
            config.sessions,
            processes_spawned,
            per_event_forks,
            attestation.argument()
        ),
    })
}

fn verify_attestation_files(
    supervisor: &Path,
    directory: &Path,
    sessions: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    for index in 0..sessions {
        let path = directory.join(format!("session-{index:03}.attest"));
        let output = Command::new(supervisor)
            .args(["verify-attestation", "--file", path_text(&path)?])
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() || !output.stdout.starts_with(b"integrity\tcomplete\n") {
            return Err(format!(
                "external attestation verification failed for session-{index:03}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
    }
    Ok(())
}

fn parse_trace_metadata(path: &Path) -> io::Result<TraceMetadata> {
    let input = fs::read_to_string(path)?;
    if !input.ends_with('\n') || input.lines().next() != Some("REMUX_TRACE_V1") {
        return Err(io::Error::other("real trace has invalid framing"));
    }
    let fields = parse_metrics_lines(input.lines().skip(1).take(5))?;
    if fields.get("source").map(String::as_str) != Some("live-working-session") {
        return Err(io::Error::other(
            "real trace source is not live-working-session",
        ));
    }
    if fields.get("exit_code").map(String::as_str) != Some("0") {
        return Err(io::Error::other("real trace capture did not exit zero"));
    }
    let command_sha256 = fields
        .get("command_sha256")
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
        .ok_or_else(|| io::Error::other("real trace command digest is invalid"))?
        .clone();
    let records_per_session = metric(&fields, "record_count")?;
    if records_per_session == 0 {
        return Err(io::Error::other("real trace is empty"));
    }
    let actual_records = u64::try_from(input.lines().skip(6).count()).map_err(io::Error::other)?;
    if actual_records != records_per_session {
        return Err(io::Error::other("real trace record count differs"));
    }
    Ok(TraceMetadata {
        records_per_session,
        command_sha256,
    })
}

fn run_infinitty_baseline(
    sessions: u32,
    command: String,
) -> Result<InfinittyScenarioResult, Box<dyn std::error::Error>> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Infinitty.app/Contents/MacOS/Infinitty"),
        PathBuf::from("/Applications/Infinitty.app/Contents/MacOS/infinitty"),
    ];
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join("Applications/Infinitty.app/Contents/MacOS/Infinitty"));
        candidates.push(home.join("Applications/Infinitty.app/Contents/MacOS/infinitty"));
    }
    if let Some(explicit) = env::var_os("INFINITTY_APP") {
        candidates.push(PathBuf::from(explicit));
    }
    let probe_paths = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let Some(binary) = candidates.iter().find(|path| path.is_file()) else {
        return Ok(InfinittyScenarioResult {
            model: "infinitty_v0.1.7_macos".to_owned(),
            availability: "artifact-not-present".to_owned(),
            probe_paths,
            sessions,
            events: None,
            processes_spawned: None,
            peak_rss_bytes: None,
            latency_us: None,
            command: None,
            feature_gap: "Same-machine probes found no local Infinitty checkout, app bundle, or executable; the no-network rail forbids fetching one, so RSS/process/latency are measured-unavailable N/A rather than inferred.".to_owned(),
        });
    };
    if sessions < 2 {
        return Err("Infinitty fleet baseline requires at least two sessions".into());
    }
    let app_socket = Path::new("/tmp/infinitty-current.sock");
    if matches!(infinitty_request(app_socket, "ping"), Ok(response) if response == "pong") {
        return Err("refusing to benchmark over an existing Infinitty instance".into());
    }
    if fs::symlink_metadata(app_socket).is_ok() {
        fs::remove_file(app_socket)?;
    }
    let temporary = TemporaryDirectory::new()?;
    let config_path = temporary.path.join("infinitty.conf");
    fs::write(
        &config_path,
        "auto-update = off\nhints = false\nnotch = false\nmarkdown-render = off\n",
    )?;
    let mut child = Command::new(binary)
        .env("INFINITTY_CONFIG", &config_path)
        .env("INFINITTY_NO_ACTIVATE", "1")
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let result = (|| -> Result<InfinittyScenarioResult, Box<dyn std::error::Error>> {
        let startup_deadline = Instant::now()
            .checked_add(Duration::from_secs(15))
            .ok_or("Infinitty startup deadline overflow")?;
        let mut resources = ResourceTracker::default();
        loop {
            let snapshot = system::snapshot()?;
            resources.observe(&snapshot, &system::descendants(&snapshot, root_pid));
            if matches!(infinitty_request(app_socket, "ping"), Ok(response) if response == "pong") {
                break;
            }
            if let Some(status) = child.try_wait()? {
                return Err(format!("Infinitty exited during startup with {status}").into());
            }
            if Instant::now() >= startup_deadline {
                return Err("Infinitty app socket did not become ready".into());
            }
            thread::sleep(SAMPLE_INTERVAL);
        }
        let initial_list = infinitty_request(app_socket, "list")?;
        let mut pane_ids = parse_infinitty_pane_ids(&initial_list)?;
        if pane_ids.len() != 1 {
            return Err("Infinitty did not start with exactly one pane".into());
        }
        let mut latencies = Vec::with_capacity(usize::try_from(sessions - 1)?);
        for _ in 1..sessions {
            let started = Instant::now();
            let response = infinitty_request(app_socket, "new-tab /tmp")?;
            latencies.push(u64::try_from(started.elapsed().as_micros())?);
            let pane_id = response
                .parse::<u64>()
                .map_err(|_| format!("invalid Infinitty new-tab response: {response}"))?;
            if !pane_ids.insert(pane_id) {
                return Err("Infinitty returned a duplicate pane id".into());
            }
            let snapshot = system::snapshot()?;
            resources.observe(&snapshot, &system::descendants(&snapshot, root_pid));
        }
        let listed = parse_infinitty_pane_ids(&infinitty_request(app_socket, "list")?)?;
        if listed != pane_ids || listed.len() != usize::try_from(sessions)? {
            return Err("Infinitty list does not show the requested 20-pane shape".into());
        }
        let steady_deadline = Instant::now()
            .checked_add(Duration::from_secs(1))
            .ok_or("Infinitty steady deadline overflow")?;
        loop {
            let snapshot = system::snapshot()?;
            let descendants = system::descendants(&snapshot, root_pid);
            resources.observe(&snapshot, &descendants);
            if descendants.len() != usize::try_from(sessions)?.saturating_add(1) {
                return Err(format!(
                    "Infinitty process tree has {} processes for {sessions} panes",
                    descendants.len()
                )
                .into());
            }
            if let Some(status) = child.try_wait()? {
                return Err(
                    format!("Infinitty exited during steady sampling with {status}").into(),
                );
            }
            if Instant::now() >= steady_deadline {
                break;
            }
            thread::sleep(SAMPLE_INTERVAL);
        }
        let process_count = resources.distinct_pid_count();
        let resident_shape = u64::from(sessions).saturating_add(1);
        if process_count < resident_shape {
            return Err(format!(
                "Infinitty sampler observed only {process_count}/{resident_shape} resident processes"
            )
            .into());
        }
        let peak_rss_bytes = resources.peak_rss_bytes();
        let latency_us = percentiles(&latencies)?;
        for pane_id in &pane_ids {
            let response = infinitty_request(app_socket, &format!("close {pane_id}"))?;
            if response != "ok" {
                return Err(format!("Infinitty close failed: {response}").into());
            }
        }
        Ok(InfinittyScenarioResult {
            model: "infinitty_v0.1.7_macos".to_owned(),
            availability: "measured".to_owned(),
            probe_paths,
            sessions,
            events: Some(u64::from(sessions - 1)),
            processes_spawned: Some(process_count),
            peak_rss_bytes: Some(peak_rss_bytes),
            latency_us: Some(latency_us),
            command: Some(format!("INFINITTY_APP={} {command}", binary.display())),
            feature_gap: "Measured latency is app-socket new-tab request→pane-id for 19 additions to one initial pane. The distinct-PID union may include transient shell-startup helpers; steady snapshots enforce one app + 20 resident shells. Infinitty exposes no event→flushed-redraw receipt and has no persistence/restore or attestation, so those cells remain N/A.".to_owned(),
        })
    })();

    if result.is_err() {
        if let Ok(list) = infinitty_request(app_socket, "list") {
            if let Ok(ids) = parse_infinitty_pane_ids(&list) {
                for pane_id in ids {
                    let _ = infinitty_request(app_socket, &format!("close {pane_id}"));
                }
            }
        }
    }
    let shutdown_deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("Infinitty shutdown deadline overflow")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= shutdown_deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(SAMPLE_INTERVAL);
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    let expected_socket = PathBuf::from(format!("/tmp/infinitty-app-{root_pid}.sock"));
    if matches!(fs::read_link(app_socket), Ok(target) if target == expected_socket) {
        let _ = fs::remove_file(app_socket);
    }
    let _ = fs::remove_file(&expected_socket);
    if let Some(status) = status {
        if !status.success() {
            return Err(format!("Infinitty exited with {status}: {}", stderr.trim()).into());
        }
    }
    result
}

fn infinitty_request(path: &Path, request: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut response = Vec::new();
    let mut buffer = [0_u8; 4_096];
    while response.len() <= 256 * 1_024 {
        let bytes = stream.read(&mut buffer)?;
        if bytes == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..bytes]);
        if response.contains(&b'\n') {
            break;
        }
    }
    let line = response
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or_else(|| io::Error::other("Infinitty returned no response"))?;
    String::from_utf8(line.to_vec())
        .map(|value| value.trim().to_owned())
        .map_err(|_| io::Error::other("Infinitty response is not UTF-8"))
}

fn parse_infinitty_pane_ids(list: &str) -> io::Result<BTreeSet<u64>> {
    let mut ids = BTreeSet::new();
    let mut remaining = list;
    let marker = "\"id\":";
    while let Some(index) = remaining.find(marker) {
        remaining = &remaining[index + marker.len()..];
        let digits = remaining
            .trim_start()
            .bytes()
            .take_while(u8::is_ascii_digit)
            .collect::<Vec<_>>();
        if digits.is_empty() {
            return Err(io::Error::other("Infinitty list has invalid pane id"));
        }
        let id = std::str::from_utf8(&digits)
            .map_err(io::Error::other)?
            .parse::<u64>()
            .map_err(|_| io::Error::other("Infinitty pane id overflow"))?;
        if !ids.insert(id) {
            return Err(io::Error::other("Infinitty list has duplicate pane id"));
        }
    }
    if ids.is_empty() {
        return Err(io::Error::other("Infinitty list has no pane ids"));
    }
    Ok(ids)
}

fn parse_metrics_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
) -> io::Result<BTreeMap<String, String>> {
    let mut metrics = BTreeMap::new();
    for line in lines {
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| io::Error::other("invalid key/value line"))?;
        if metrics.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(io::Error::other("duplicate key/value field"));
        }
    }
    Ok(metrics)
}

fn observe_tui_resources(
    snapshot: &[system::ProcessEntry],
    root_pid: u32,
    child_pids: &BTreeSet<u32>,
    total: &mut ResourceTracker,
    tui: &mut ResourceTracker,
    children: &mut ResourceTracker,
) {
    let all = system::descendants(snapshot, root_pid);
    let root = BTreeSet::from([root_pid]);
    total.observe(snapshot, &all);
    tui.observe(snapshot, &root);
    children.observe(snapshot, child_pids);
}

fn authorize(
    supervisor: &Path,
    auth_log: &Path,
    auth_token: &str,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let authorization = Command::new(supervisor)
        .args([
            "authorize",
            "--auth-log",
            path_text(auth_log)?,
            "--token",
            auth_token,
            "--scope",
            scope,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if authorization.status.success() {
        Ok(())
    } else {
        Err(format!(
            "attach authorization failed: {}",
            String::from_utf8_lossy(&authorization.stderr).trim()
        )
        .into())
    }
}

fn parse_ready_pids(path: &Path, root_pid: u32, sessions: u32) -> io::Result<BTreeSet<u32>> {
    let ready = parse_metrics(path)?;
    let recorded_root = metric(&ready, "pid")?;
    if recorded_root != u64::from(root_pid) {
        return Err(io::Error::other("ready receipt root PID differs"));
    }
    expect_metric(&ready, "children", u64::from(sessions))?;
    let child_pids = ready
        .get("child_pids")
        .ok_or_else(|| io::Error::other("ready receipt missing child_pids"))?
        .split(',')
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| io::Error::other("invalid child PID in ready receipt"))
        })
        .collect::<io::Result<BTreeSet<_>>>()?;
    if child_pids.len() != usize::try_from(sessions).map_err(io::Error::other)? {
        return Err(io::Error::other(
            "distinct child PID count differs from session count",
        ));
    }
    let mut authorized = child_pids;
    authorized.insert(root_pid);
    Ok(authorized)
}

fn parse_metrics(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let input = fs::read_to_string(path)?;
    let mut metrics = BTreeMap::new();
    for line in input.lines() {
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| io::Error::other("invalid supervisor metric line"))?;
        if metrics.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(io::Error::other("duplicate supervisor metric"));
        }
    }
    Ok(metrics)
}

fn parse_metric_samples(metrics: &BTreeMap<String, String>, name: &str) -> io::Result<Vec<u64>> {
    let values = metrics
        .get(name)
        .ok_or_else(|| io::Error::other(format!("metrics missing {name}")))?;
    if values.is_empty() {
        return Ok(Vec::new());
    }
    values
        .split(',')
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| io::Error::other(format!("invalid metric sample {name}")))
        })
        .collect()
}

fn metric(metrics: &BTreeMap<String, String>, name: &str) -> io::Result<u64> {
    metrics
        .get(name)
        .ok_or_else(|| io::Error::other(format!("metrics missing {name}")))?
        .parse::<u64>()
        .map_err(|_| io::Error::other(format!("invalid metric {name}")))
}

fn expect_metric(metrics: &BTreeMap<String, String>, name: &str, expected: u64) -> io::Result<()> {
    let actual = metric(metrics, name)?;
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "metric {name}: expected {expected}, got {actual}"
        )))
    }
}

fn verify_passive_restore(
    supervisor: &Path,
    state: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(supervisor)
        .args(["restore", "--state", path_text(state)?])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "passive restore verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("restored passive layout") {
        return Err("passive restore verification returned unexpected output".into());
    }
    Ok(())
}

fn terminate_children(children: &mut [TrackedChild]) {
    for tracked in children {
        let _ = tracked.child.kill();
        let _ = tracked.child.wait();
    }
}

struct TrackedChild {
    child: Child,
    started: Instant,
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let path = PathBuf::from(format!(
            "/tmp/rmx-{}-{}",
            std::process::id(),
            unix_seconds()?
        ));
        match fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Debug)]
struct Config {
    sessions: u32,
    events_per_session: u64,
    rate: u64,
    fork_hold_ms: u64,
    fork_cpu_ms: u64,
    fork_rss_mib: u64,
}

impl Config {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut config = Self {
            sessions: 20,
            events_per_session: 6,
            rate: 20,
            fork_hold_ms: 360,
            fork_cpu_ms: 30,
            fork_rss_mib: 18,
        };
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--sessions" => config.sessions = parse_positive(&value, "sessions")?,
                "--events-per-session" => {
                    config.events_per_session = parse_positive(&value, "events-per-session")?;
                }
                "--rate" => config.rate = parse_positive(&value, "rate")?,
                "--fork-hold-ms" => config.fork_hold_ms = parse_positive(&value, "fork-hold-ms")?,
                "--fork-cpu-ms" => config.fork_cpu_ms = parse_positive(&value, "fork-cpu-ms")?,
                "--fork-rss-mib" => config.fork_rss_mib = parse_positive(&value, "fork-rss-mib")?,
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(config)
    }

    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.sessions > 64 {
            return Err("sessions exceeds safety limit 64".into());
        }
        if self.fork_cpu_ms > self.fork_hold_ms {
            return Err("fork-cpu-ms cannot exceed fork-hold-ms".into());
        }
        let concurrent_forks = self
            .rate
            .checked_mul(self.fork_hold_ms)
            .and_then(|value| value.checked_add(999))
            .and_then(|value| value.checked_div(1_000))
            .and_then(|value| value.checked_add(1))
            .ok_or("fork concurrency estimate overflow")?;
        let resident_mib = concurrent_forks
            .checked_mul(self.fork_rss_mib)
            .ok_or("fork memory estimate overflow")?;
        if resident_mib > FORK_MEMORY_RAIL_MIB {
            return Err(format!(
                "fork baseline estimates {resident_mib}MiB, over {FORK_MEMORY_RAIL_MIB}MiB rail"
            )
            .into());
        }
        let sweep_seconds = self.estimated_fork_seconds()
            + self.estimated_supervisor_seconds() * 3.0
            + TUI_INITIAL_IDLE_MS as f64 / 1_000.0;
        if sweep_seconds >= BENCHMARK_BUDGET_SECONDS {
            return Err("configuration exceeds five-minute full-sweep budget".into());
        }
        Ok(())
    }

    fn total_events(&self) -> Result<u64, Box<dyn std::error::Error>> {
        u64::from(self.sessions)
            .checked_mul(self.events_per_session)
            .ok_or_else(|| "total event count overflow".into())
    }

    fn estimated_fork_seconds(&self) -> f64 {
        u64::from(self.sessions) as f64 * self.events_per_session as f64 / self.rate as f64
            + self.fork_hold_ms as f64 / 1_000.0
    }

    fn estimated_supervisor_seconds(&self) -> f64 {
        u64::from(self.sessions) as f64 * self.events_per_session as f64 / self.rate as f64
    }

    fn supervisor_timeout_seconds(&self) -> u64 {
        u64::from(self.sessions)
            .saturating_mul(self.events_per_session)
            .div_ceil(self.rate)
            .saturating_add(10)
    }

    fn reproduction_command(&self) -> String {
        format!(
            "scripts/with_scorer_lock.sh cargo run -p bench -- --sessions {} --events-per-session {} --rate {} --fork-hold-ms {} --fork-cpu-ms {} --fork-rss-mib {}",
            self.sessions,
            self.events_per_session,
            self.rate,
            self.fork_hold_ms,
            self.fork_cpu_ms,
            self.fork_rss_mib
        )
    }

    fn report_config(&self) -> BenchmarkConfig {
        BenchmarkConfig {
            sessions: self.sessions,
            events_per_session: self.events_per_session,
            rate: self.rate,
            fork_hold_ms: self.fork_hold_ms,
            fork_cpu_ms: self.fork_cpu_ms,
            fork_rss_mib: self.fork_rss_mib,
        }
    }
}

fn parse_positive<T>(value: &str, name: &str) -> Result<T, Box<dyn std::error::Error>>
where
    T: std::str::FromStr + PartialEq + From<u8>,
{
    let parsed = value.parse::<T>().map_err(|_| format!("invalid {name}"))?;
    if parsed == T::from(0) {
        Err(format!("{name} must be positive").into())
    } else {
        Ok(parsed)
    }
}

fn machine() -> Result<Machine, Box<dyn std::error::Error>> {
    let os_version = command_output("/usr/bin/sw_vers", &["-productVersion"])
        .unwrap_or_else(|_| env::consts::OS.to_owned());
    let rustc = command_output(
        env::var("RUSTC").as_deref().unwrap_or("rustc"),
        &["--version"],
    )?;
    Ok(Machine {
        os: format!("{} {}", env::consts::OS, os_version),
        architecture: env::consts::ARCH.to_owned(),
        rustc,
    })
}

fn command_output(program: &str, arguments: &[&str]) -> io::Result<String> {
    let output = Command::new(program).args(arguments).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("{program} failed")));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| io::Error::other(format!("{program} output is not UTF-8")))
}

fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("output file name is not UTF-8"))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let write_result = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    write_result
}

fn path_text(path: &Path) -> Result<&str, Box<dyn std::error::Error>> {
    path.to_str().ok_or_else(|| "path is not UTF-8".into())
}

fn unix_seconds() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}
