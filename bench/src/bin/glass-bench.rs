// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring adapter receipts.
#![forbid(unsafe_code)]
//! W5 same-machine terminal-layer orchestrator.
//!
//! Contract: arms run serially after preregistered idle admission. The harness uses
//! installed public control surfaces only and writes each raw artifact before the
//! summary. It never fetches, installs, updates, or invokes an agent provider.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bench::glass::{
    admit_machine, render_glass_json, render_glass_markdown, require_session_shape, CmuxHookResult,
    GlassArmResult, GlassMachineState, GlassReport, GlassWorkload,
};
use bench::system::{self, ResourceTracker};
use bench::{percentiles, Machine};

const WORKLOAD_RELATIVE_PATH: &str = "bench/workloads/w5-terminal-fleet.tsv";
const CMUX_APP: &str = "/Applications/cmux.app/Contents/MacOS/cmux";
const CMUX_CLI: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";
const CMUX_INFO: &str = "/Applications/cmux.app/Contents/Info.plist";
const GHOSTTY_APP: &str = "/Applications/Ghostty.app/Contents/MacOS/ghostty";
const GHOSTTY_INFO: &str = "/Applications/Ghostty.app/Contents/Info.plist";
const INFINITTY_SOCKET: &str = "/tmp/infinitty-current.sock";
const SAMPLE_SLEEP_FLOOR: Duration = Duration::from_millis(1);

fn main() {
    if let Err(error) = run() {
        eprintln!("glass-bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse(env::args().skip(1))?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("bench crate has no workspace parent")?;
    let workload_path = workspace.join(&config.workload);
    let workload = GlassWorkload::read(&workload_path)?;
    let workload_sha256 = sha256_file(&workload_path)?;
    prepare_binaries(workspace)?;
    let binary_directory = env::current_exe()?
        .parent()
        .ok_or("glass-bench executable has no parent")?
        .to_path_buf();
    let supervisor = binary_directory.join("remux-supervisor");
    let trace_agent = binary_directory.join("trace-agent");
    if !supervisor.is_file() || !trace_agent.is_file() {
        return Err("prepared supervisor binaries are missing".into());
    }
    verify_trace_contract(workspace, &workload)?;

    let generated_unix_seconds = unix_seconds()?;
    let run_id = format!("run-{generated_unix_seconds}-{}", std::process::id());
    let results_root = workspace.join(&config.output_directory);
    let run_directory = results_root.join(&run_id);
    fs::create_dir_all(&run_directory)?;
    let preflight_relative = relative_artifact(&config.output_directory, &run_id, "preflight.tsv");
    let preflight = run_preflight(&workload, &workload_sha256)?;
    write_atomic(&run_directory.join("preflight.tsv"), &preflight)?;

    let reproduction = format!(
        "scripts/with_scorer_lock.sh cargo run --offline -p bench --bin glass-bench -- --workload {} --output-dir {}",
        config.workload.display(),
        config.output_directory.display()
    );
    let temporary = TemporaryDirectory::new()?;
    let workload_command = make_workload_command(
        &temporary.path,
        &trace_agent,
        &workspace.join(&workload.trace),
        workload.hold_after_trace_ms,
    )?;

    let mut arms = Vec::new();

    let remux_state = admitted_machine(&workload)?;
    let remux_artifact = relative_artifact(&config.output_directory, &run_id, "remux.tsv");
    let remux = run_remux_arm(
        &workload,
        &supervisor,
        &trace_agent,
        workspace,
        &temporary.path,
        reproduction.clone(),
        remux_artifact.clone(),
    )?;
    write_atomic(
        &run_directory.join("remux.tsv"),
        &render_arm_artifact(&remux, &remux_state, &workload_sha256)?,
    )?;
    arms.push(remux);

    let cmux_state = admitted_machine(&workload)?;
    let cmux_artifact = relative_artifact(&config.output_directory, &run_id, "cmux.tsv");
    let hook_artifact = relative_artifact(&config.output_directory, &run_id, "cmux-hooks.tsv");
    let (cmux, hook) = run_cmux_arm(
        &workload,
        &workload_command,
        reproduction.clone(),
        cmux_artifact.clone(),
        hook_artifact.clone(),
    )?;
    write_atomic(
        &run_directory.join("cmux.tsv"),
        &render_arm_artifact(&cmux, &cmux_state, &workload_sha256)?,
    )?;
    write_atomic(
        &run_directory.join("cmux-hooks.tsv"),
        &render_hook_artifact(&hook, &cmux_state, &workload_sha256)?,
    )?;
    arms.push(cmux);

    let ghostty_state = admitted_machine(&workload)?;
    let ghostty_artifact = relative_artifact(&config.output_directory, &run_id, "ghostty.tsv");
    let ghostty = run_ghostty_arm(
        &workload,
        &workload_command,
        &temporary.path,
        reproduction.clone(),
        ghostty_artifact.clone(),
    )?;
    write_atomic(
        &run_directory.join("ghostty.tsv"),
        &render_arm_artifact(&ghostty, &ghostty_state, &workload_sha256)?,
    )?;
    arms.push(ghostty);

    let infinitty_state = admitted_machine(&workload)?;
    let infinitty_artifact = relative_artifact(&config.output_directory, &run_id, "infinitty.tsv");
    let infinitty = run_infinitty_arm(
        &workload,
        &workload_command,
        &temporary.path,
        reproduction,
        infinitty_artifact.clone(),
    )?;
    write_atomic(
        &run_directory.join("infinitty.tsv"),
        &render_arm_artifact(&infinitty, &infinitty_state, &workload_sha256)?,
    )?;
    arms.push(infinitty);

    let report = GlassReport {
        run_id: run_id.clone(),
        generated_unix_seconds,
        machine: machine()?,
        workload_sha256,
        workload_path: config.workload.display().to_string(),
        preflight_artifact: preflight_relative,
        arms,
        cmux_hook: hook,
    };
    validate_complete_report(&report, &workload)?;
    let json = render_glass_json(&report);
    let run_json = run_directory.join("report.json");
    let latest_json = results_root.join("latest.json");
    write_atomic(&run_json, &json)?;
    write_atomic(&latest_json, &json)?;
    let report_json_relative = format!("results/w5/{run_id}/report.json");
    let markdown = render_glass_markdown(&report, &report_json_relative);
    write_atomic(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("GLASS_RESULTS.md"),
        &markdown,
    )?;

    println!("W5 glass run {run_id} PASS");
    for arm in &report.arms {
        println!(
            "{}: {} sessions, peak {}, steady {}, {} PIDs",
            arm.subject,
            display_optional(arm.sessions_observed.map(u64::from)),
            display_optional(arm.peak_rss_bytes),
            display_optional(arm.steady_rss_bytes),
            display_optional(arm.distinct_pid_count)
        );
    }
    println!(
        "cmux hooks: {} events / {} PIDs / {}us p95",
        report.cmux_hook.events,
        report.cmux_hook.distinct_hook_pids,
        report.cmux_hook.wall_latency_us.p95
    );
    println!("wrote {}", run_json.display());
    Ok(())
}

fn run_preflight(workload: &GlassWorkload, workload_sha256: &str) -> io::Result<String> {
    let current = machine_state()?;
    admit_machine(&current, workload)?;
    require_session_shape(workload.sessions, workload)?;
    let load_limit =
        f64::from(current.logical_cpus) * workload.max_load_per_logical_cpu_milli as f64 / 1_000.0;
    let load_negative = GlassMachineState {
        load_one: load_limit + 0.01,
        logical_cpus: current.logical_cpus,
        free_memory_percent: workload.min_free_memory_percent,
    };
    if admit_machine(&load_negative, workload).is_ok() {
        return Err(io::Error::other("load negative control did not reject"));
    }
    let memory_negative = GlassMachineState {
        load_one: 0.0,
        logical_cpus: current.logical_cpus,
        free_memory_percent: workload.min_free_memory_percent.saturating_sub(1),
    };
    if admit_machine(&memory_negative, workload).is_ok() {
        return Err(io::Error::other("memory negative control did not reject"));
    }
    if require_session_shape(workload.sessions - 1, workload).is_ok() {
        return Err(io::Error::other("shape negative control did not reject"));
    }
    if percentiles(&[]).is_ok() {
        return Err(io::Error::other(
            "empty-metric negative control did not reject",
        ));
    }
    Ok(format!(
        "schema\t1\nworkload_sha256\t{workload_sha256}\nmanifest_reference\tpass\nmanifest_truncation_negative\trejected-by-test\nidle_current\tpass\nidle_load_negative\trejected\nidle_memory_negative\trejected\nsession_shape_reference\tpass\nsession_shape_negative\trejected\nempty_metric_negative\trejected\nload_one\t{:.2}\nlogical_cpus\t{}\nfree_memory_percent\t{}\n",
        current.load_one, current.logical_cpus, current.free_memory_percent
    ))
}

fn run_remux_arm(
    workload: &GlassWorkload,
    supervisor: &Path,
    trace_agent: &Path,
    workspace: &Path,
    temporary_root: &Path,
    reproduction: String,
    raw_artifact: String,
) -> Result<GlassArmResult, Box<dyn std::error::Error>> {
    let root = temporary_root.join("remux");
    fs::create_dir_all(&root)?;
    let auth_log = root.join("auth.log");
    let token = format!("w5-remux-{}", std::process::id());
    authorize(supervisor, &auth_log, &token)?;
    let ready = root.join("ready.tsv");
    let metrics = root.join("metrics.tsv");
    let attestations = root.join("attestations");
    let tui = root.join("tui.ansi");
    let started = Instant::now();
    let mut child = Command::new(supervisor)
        .args([
            "run",
            "--sessions",
            &workload.sessions.to_string(),
            "--events-per-session",
            &workload.trace_records_per_session.to_string(),
            "--rate",
            "20",
            "--agent-kind",
            "trace-replay",
            "--trace-agent",
            path_text(trace_agent)?,
            "--trace",
            path_text(&workspace.join(&workload.trace))?,
            "--trace-hold-after-ms",
            &workload.hold_after_trace_ms.to_string(),
            "--socket",
            path_text(&root.join("socket"))?,
            "--state",
            path_text(&root.join("state.json"))?,
            "--scrollback-dir",
            path_text(&root.join("scrollback"))?,
            "--attestation-dir",
            path_text(&attestations)?,
            "--attestation",
            "hash-chain",
            "--metrics",
            path_text(&metrics)?,
            "--ready",
            path_text(&ready)?,
            "--timeout-seconds",
            &workload.arm_timeout_seconds.to_string(),
            "--auth-log",
            path_text(&auth_log)?,
            "--attach-token",
            &token,
            "--attach-scope",
            "launch",
            "--tui-output",
            path_text(&tui)?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let deadline = started
        .checked_add(Duration::from_secs(workload.arm_timeout_seconds))
        .ok_or("remux deadline overflow")?;
    let mut resources = ResourceTracker::default();
    let authorized = loop {
        observe_root(root_pid, &mut resources)?;
        if ready.exists() {
            break parse_ready(&ready, root_pid, workload.sessions)?;
        }
        if let Some(status) = child.try_wait()? {
            return Err(process_error("remux exited before ready", status, &mut child).into());
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            return Err("remux did not become ready".into());
        }
        thread::sleep(sample_interval(workload));
    };
    let fleet_spawn_wall_us = u64::try_from(started.elapsed().as_micros())?;
    wait_trace_completion(root_pid, workload, &mut resources, &mut child, deadline)?;
    let (steady_rss_samples, steady_pid_count) = sample_steady(
        root_pid,
        workload,
        &mut resources,
        Some(&mut child),
        deadline,
    )?;
    let status = loop {
        observe_root(root_pid, &mut resources)?;
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            return Err("remux exceeded arm timeout".into());
        }
        thread::sleep(sample_interval(workload));
    };
    let stderr = read_stderr(&mut child)?;
    if !status.success() {
        return Err(format!("remux exited with {status}: {}", stderr.trim()).into());
    }
    if !authorized.is_subset(resources.observed_pids()) {
        return Err("remux sampler missed an authorized PID".into());
    }
    let parsed = parse_tsv(&metrics)?;
    expect_field(&parsed, "schema", "3")?;
    expect_field(&parsed, "agent_kind", "trace-replay")?;
    expect_field(&parsed, "attestation_enabled", "1")?;
    expect_field(
        &parsed,
        "events_ingested",
        &u64::from(workload.sessions)
            .checked_mul(workload.trace_records_per_session)
            .ok_or("remux event total overflow")?
            .to_string(),
    )?;
    let records = parse_u64_field(&parsed, "attestation_records")?;
    if records == 0 || parse_u64_field(&parsed, "attestation_file_bytes")? == 0 {
        return Err("remux attested arm emitted an empty chain".into());
    }
    for index in 0..workload.sessions {
        let verification = Command::new(supervisor)
            .args([
                "verify-attestation",
                "--file",
                path_text(&attestations.join(format!("session-{index:03}.attest")))?,
            ])
            .output()?;
        if !verification.status.success()
            || !verification.stdout.starts_with(b"integrity\tcomplete\n")
        {
            return Err(format!("remux session-{index:03} attestation failed").into());
        }
    }
    if fs::metadata(&tui)?.len() == 0 {
        return Err("remux TUI arm emitted no frame".into());
    }
    let peak = resources.peak_rss_bytes();
    let steady = median(&steady_rss_samples)?;
    require_metrics(
        peak,
        steady,
        resources.distinct_pid_count(),
        steady_pid_count,
    )?;
    Ok(GlassArmResult {
        subject: "remux std-only TUI (attested)".to_owned(),
        version: format!("0.1.0@{}", git_short_revision(workspace)?),
        availability: "measured".to_owned(),
        workload_mode: "W4 real-trace replay + ANSI TUI + hash chain".to_owned(),
        sessions_requested: workload.sessions,
        sessions_observed: Some(workload.sessions),
        preexisting_sessions: Some(0),
        events_per_session: Some(workload.trace_records_per_session),
        baseline_rss_bytes: None,
        peak_rss_bytes: Some(peak),
        steady_rss_bytes: Some(steady),
        steady_rss_samples,
        distinct_pid_count: Some(resources.distinct_pid_count()),
        steady_pid_count: Some(steady_pid_count),
        fleet_spawn_wall_us: Some(fleet_spawn_wall_us),
        spawn_latency_us: None,
        reproduction: Some(reproduction),
        capability_gaps: format!(
            "Per-session spawn acknowledgements are not exposed (fleet launch→ready is measured); {records} attestation records are externally verified."
        ),
        raw_artifact,
    })
}

fn run_cmux_arm(
    workload: &GlassWorkload,
    workload_command: &Path,
    reproduction: String,
    raw_artifact: String,
    hook_artifact: String,
) -> Result<(GlassArmResult, CmuxHookResult), Box<dyn std::error::Error>> {
    if !Path::new(CMUX_APP).is_file()
        || !Path::new(CMUX_CLI).is_file()
        || !Path::new(CMUX_INFO).is_file()
    {
        return Err("installed cmux app/CLI/public manifest is absent".into());
    }
    let root_pid = exact_process_pid(CMUX_APP)?;
    let socket = cmux_socket()?;
    let ping = cmux_output(&socket, &["ping"], None)?;
    require_success("cmux ping", &ping)?;
    let version = format!(
        "{}@{}",
        plist_value(CMUX_INFO, "CFBundleShortVersionString")?,
        plist_value(CMUX_INFO, "CMUXCommit")?
    );
    let before_ids = cmux_workspace_ids(&socket)?;
    let preexisting = u32::try_from(before_ids.len())?;
    let baseline_snapshot = system::snapshot()?;
    let baseline_selected = system::descendants(&baseline_snapshot, root_pid);
    let baseline_rss = system::selected_rss_bytes(&baseline_snapshot, &baseline_selected);
    if baseline_rss == 0 {
        return Err("cmux baseline process tree is absent".into());
    }
    let mut resources = ResourceTracker::default();
    resources.observe(&baseline_snapshot, &baseline_selected);
    let mut owned = Vec::new();
    let mut spawn_latencies = Vec::new();
    let started = Instant::now();
    let result = (|| -> Result<(GlassArmResult, CmuxHookResult), Box<dyn std::error::Error>> {
        for index in 0..workload.sessions {
            let name = format!("W5-GLASS-{}-{index:02}", std::process::id());
            let launched = Instant::now();
            let output = cmux_output(
                &socket,
                &[
                    "--id-format",
                    "uuids",
                    "new-workspace",
                    "--name",
                    &name,
                    "--cwd",
                    "/tmp",
                    "--command",
                    path_text(workload_command)?,
                    "--no-focus",
                ],
                None,
            )?;
            require_success("cmux new-workspace", &output)?;
            spawn_latencies.push(u64::try_from(launched.elapsed().as_micros())?);
            let id = first_uuid(&String::from_utf8(output.stdout)?)
                .ok_or("cmux new-workspace returned no UUID")?;
            if !owned.insert_unique(id) {
                return Err("cmux returned a duplicate benchmark workspace".into());
            }
            observe_root(root_pid, &mut resources)?;
        }
        let fleet_spawn_wall_us = u64::try_from(started.elapsed().as_micros())?;
        let after_ids = cmux_workspace_ids(&socket)?;
        let owned_set = owned.iter().cloned().collect::<BTreeSet<_>>();
        if !owned_set.is_subset(&after_ids) || after_ids.len() != before_ids.len() + owned.len() {
            return Err("cmux W5 workspace ownership/shape differs".into());
        }
        require_session_shape(u32::try_from(owned.len())?, workload)?;
        wait_trace_completion_without_child(root_pid, workload, &mut resources)?;
        let (steady_rss_samples, steady_pid_count) = sample_steady(
            root_pid,
            workload,
            &mut resources,
            None,
            arm_deadline(workload)?,
        )?;
        let hook = run_cmux_hooks(&socket, &version, workload.cmux_hook_events, hook_artifact)?;
        let peak = resources.peak_rss_bytes();
        let steady = median(&steady_rss_samples)?;
        require_metrics(
            peak,
            steady,
            resources.distinct_pid_count(),
            steady_pid_count,
        )?;
        Ok((
            GlassArmResult {
                subject: "cmux Ghostty-embedded incumbent".to_owned(),
                version: version.clone(),
                availability: "measured-resident-app".to_owned(),
                workload_mode: "20 W5-owned workspaces replaying W4 trace".to_owned(),
                sessions_requested: workload.sessions,
                sessions_observed: Some(workload.sessions),
                preexisting_sessions: Some(preexisting),
                events_per_session: Some(workload.trace_records_per_session),
                baseline_rss_bytes: Some(baseline_rss),
                peak_rss_bytes: Some(peak),
                steady_rss_bytes: Some(steady),
                steady_rss_samples,
                distinct_pid_count: Some(resources.distinct_pid_count()),
                steady_pid_count: Some(steady_pid_count),
                fleet_spawn_wall_us: Some(fleet_spawn_wall_us),
                spawn_latency_us: Some(percentiles(&spawn_latencies)?),
                reproduction: Some(reproduction),
                capability_gaps: format!(
                    "Installed app refused isolated second-instance probes; full tree includes {preexisting} disclosed pre-existing workspaces. No remux-equivalent passive-restore or supervisor attestation receipt was measured."
                ),
                raw_artifact,
            },
            hook,
        ))
    })();
    for id in owned.iter().rev() {
        let _ = cmux_output(&socket, &["close-workspace", "--workspace", id], None);
    }
    let final_ids = cmux_workspace_ids(&socket)?;
    if final_ids != before_ids {
        return Err("cmux cleanup did not restore the pre-run workspace set".into());
    }
    result
}

trait InsertUnique<T> {
    fn insert_unique(&mut self, value: T) -> bool;
}

impl<T: Eq> InsertUnique<T> for Vec<T> {
    fn insert_unique(&mut self, value: T) -> bool {
        if self.contains(&value) {
            false
        } else {
            self.push(value);
            true
        }
    }
}

fn run_cmux_hooks(
    socket: &Path,
    version: &str,
    events: u32,
    raw_artifact: String,
) -> Result<CmuxHookResult, Box<dyn std::error::Error>> {
    let mut wall = Vec::with_capacity(usize::try_from(events)?);
    let mut pids = BTreeSet::new();
    let mut peak_rss = 0_u64;
    for _ in 0..events {
        let started = Instant::now();
        let mut child = Command::new(CMUX_CLI)
            .env("CMUX_SOCKET_PATH", socket)
            .env("CMUX_QUIET", "1")
            .args(["hooks", "pi", "event"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child.id();
        if !pids.insert(pid) {
            terminate(&mut child);
            return Err("cmux hook PID was unexpectedly reused".into());
        }
        child
            .stdin
            .take()
            .ok_or("cmux hook stdin missing")?
            .write_all(b"{}\n")?;
        let mut tracker = ResourceTracker::default();
        loop {
            let snapshot = system::snapshot()?;
            tracker.observe(&snapshot, &BTreeSet::from([pid]));
            if child.try_wait()?.is_some() {
                break;
            }
            thread::sleep(SAMPLE_SLEEP_FLOOR);
        }
        let output = child.wait_with_output()?;
        require_success("cmux hooks pi event", &output)?;
        if output.stdout != b"{}\n" {
            return Err("cmux hook did not return its accepted live payload".into());
        }
        wall.push(u64::try_from(started.elapsed().as_micros())?);
        peak_rss = peak_rss.max(tracker.peak_rss_bytes());
    }
    if pids.len() != usize::try_from(events)? || peak_rss == 0 {
        return Err("cmux hook sampler missed a required live process".into());
    }
    Ok(CmuxHookResult {
        version: version.to_owned(),
        events,
        distinct_hook_pids: u64::try_from(pids.len())?,
        per_event_forks: pids.len() as f64 / f64::from(events),
        peak_hook_rss_bytes: peak_rss,
        wall_latency_us: percentiles(&wall)?,
        command: "printf '{}\\n' | CMUX_SOCKET_PATH=... cmux hooks pi event".to_owned(),
        raw_artifact,
        observation: "Each accepted event launched a separate current cmux CLI process; the 2026-07-15 0.37–0.38s/18–21MB figure is refreshed, not copied forward.".to_owned(),
    })
}

fn run_ghostty_arm(
    workload: &GlassWorkload,
    workload_command: &Path,
    temporary_root: &Path,
    reproduction: String,
    raw_artifact: String,
) -> Result<GlassArmResult, Box<dyn std::error::Error>> {
    if !Path::new(GHOSTTY_APP).is_file() || !Path::new(GHOSTTY_INFO).is_file() {
        return Ok(unavailable_arm(
            "ghostty vanilla Metal baseline",
            "artifact-not-present",
            workload,
            reproduction,
            "Ghostty app/public manifest absent; all process and latency values are N/A.",
            raw_artifact,
        ));
    }
    let root_pid = exact_process_pid(GHOSTTY_APP)?;
    let version = format!(
        "{}@{}",
        plist_value(GHOSTTY_INFO, "CFBundleShortVersionString")?,
        plist_value(GHOSTTY_INFO, "GhosttyCommit")?
    );
    let script = temporary_root.join("ghostty-w5.applescript");
    fs::write(&script, ghostty_script())?;
    let before = ghostty_number(&script, &["total"])?;
    let baseline_snapshot = system::snapshot()?;
    let baseline_selected = system::descendants(&baseline_snapshot, root_pid);
    let baseline_rss = system::selected_rss_bytes(&baseline_snapshot, &baseline_selected);
    let mut resources = ResourceTracker::default();
    resources.observe(&baseline_snapshot, &baseline_selected);
    let mut window_id = None;
    let mut spawn_latencies = Vec::new();
    let started = Instant::now();
    let result = (|| -> Result<GlassArmResult, Box<dyn std::error::Error>> {
        let launched = Instant::now();
        let created = ghostty_output(&script, &["create", path_text(workload_command)?, "/tmp"])?;
        require_success("Ghostty new window", &created)?;
        spawn_latencies.push(u64::try_from(launched.elapsed().as_micros())?);
        let id = String::from_utf8(created.stdout)?.trim().to_owned();
        if id.is_empty() || id.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err("Ghostty returned an invalid W5 window id".into());
        }
        window_id = Some(id.clone());
        observe_root(root_pid, &mut resources)?;
        for _ in 1..workload.sessions {
            let launched = Instant::now();
            let output =
                ghostty_output(&script, &["add", &id, path_text(workload_command)?, "/tmp"])?;
            require_success("Ghostty new tab", &output)?;
            spawn_latencies.push(u64::try_from(launched.elapsed().as_micros())?);
            observe_root(root_pid, &mut resources)?;
        }
        let fleet_spawn_wall_us = u64::try_from(started.elapsed().as_micros())?;
        require_session_shape(ghostty_number(&script, &["window", &id])?, workload)?;
        let total = ghostty_number(&script, &["total"])?;
        if total != before + workload.sessions {
            return Err("Ghostty total session shape differs after W5 creation".into());
        }
        wait_trace_completion_without_child(root_pid, workload, &mut resources)?;
        let (steady_rss_samples, steady_pid_count) = sample_steady(
            root_pid,
            workload,
            &mut resources,
            None,
            arm_deadline(workload)?,
        )?;
        let peak = resources.peak_rss_bytes();
        let steady = median(&steady_rss_samples)?;
        require_metrics(
            peak,
            steady,
            resources.distinct_pid_count(),
            steady_pid_count,
        )?;
        Ok(GlassArmResult {
            subject: "ghostty vanilla Metal baseline".to_owned(),
            version,
            availability: "measured-resident-app".to_owned(),
            workload_mode: "20 AppleScript-manifest tabs replaying W4 trace".to_owned(),
            sessions_requested: workload.sessions,
            sessions_observed: Some(workload.sessions),
            preexisting_sessions: Some(before),
            events_per_session: Some(workload.trace_records_per_session),
            baseline_rss_bytes: Some(baseline_rss),
            peak_rss_bytes: Some(peak),
            steady_rss_bytes: Some(steady),
            steady_rss_samples,
            distinct_pid_count: Some(resources.distinct_pid_count()),
            steady_pid_count: Some(steady_pid_count),
            fleet_spawn_wall_us: Some(fleet_spawn_wall_us),
            spawn_latency_us: Some(percentiles(&spawn_latencies)?),
            reproduction: Some(reproduction),
            capability_gaps: "AppleScript create acknowledgement is measured, but Ghostty exposes no remux-equivalent supervisor attestation, passive restore receipt, or event→flushed-redraw boundary; those remain N/A.".to_owned(),
            raw_artifact,
        })
    })();
    if let Some(id) = &window_id {
        let _ = ghostty_output(&script, &["close", id]);
    }
    let after = ghostty_number(&script, &["total"])?;
    if after != before {
        return Err("Ghostty cleanup did not restore the pre-run terminal count".into());
    }
    result
}

fn run_infinitty_arm(
    workload: &GlassWorkload,
    workload_command: &Path,
    temporary_root: &Path,
    reproduction: String,
    raw_artifact: String,
) -> Result<GlassArmResult, Box<dyn std::error::Error>> {
    let candidates = infinitty_candidates();
    let Some(binary) = candidates.iter().find(|path| path.is_file()) else {
        return Ok(unavailable_arm(
            "Infinitty v0.1.7 narrow Metal",
            "artifact-not-present",
            workload,
            reproduction,
            &format!(
                "No local executable at {}; no-network rail forbids fetching, so all unavailable cells are N/A.",
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            raw_artifact,
        ));
    };
    let socket = Path::new(INFINITTY_SOCKET);
    if matches!(infinitty_request(socket, "ping"), Ok(response) if response == "pong") {
        return Err("refusing to benchmark over an existing Infinitty instance".into());
    }
    if fs::symlink_metadata(socket).is_ok() {
        fs::remove_file(socket)?;
    }
    let root = temporary_root.join("infinitty");
    fs::create_dir_all(&root)?;
    let config = root.join("infinitty.conf");
    fs::write(
        &config,
        "auto-update = off\nhints = false\nnotch = false\nmarkdown-render = off\n",
    )?;
    let started = Instant::now();
    let mut child = Command::new(binary)
        .env("INFINITTY_CONFIG", &config)
        .env("INFINITTY_NO_ACTIVATE", "1")
        .env("SHELL", workload_command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let deadline = started
        .checked_add(Duration::from_secs(workload.arm_timeout_seconds))
        .ok_or("Infinitty deadline overflow")?;
    let mut resources = ResourceTracker::default();
    let mut spawn_latencies = Vec::new();
    let result = (|| -> Result<GlassArmResult, Box<dyn std::error::Error>> {
        loop {
            observe_root(root_pid, &mut resources)?;
            if matches!(infinitty_request(socket, "ping"), Ok(response) if response == "pong") {
                break;
            }
            if let Some(status) = child.try_wait()? {
                return Err(format!("Infinitty exited during startup with {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("Infinitty app socket did not become ready".into());
            }
            thread::sleep(sample_interval(workload));
        }
        spawn_latencies.push(u64::try_from(started.elapsed().as_micros())?);
        let mut pane_ids = parse_infinitty_ids(&infinitty_request(socket, "list")?)?;
        if pane_ids.len() != 1 {
            return Err("Infinitty did not start with one W5 pane".into());
        }
        let baseline_snapshot = system::snapshot()?;
        let baseline_selected = system::descendants(&baseline_snapshot, root_pid);
        let baseline_rss = system::selected_rss_bytes(&baseline_snapshot, &baseline_selected);
        resources.observe(&baseline_snapshot, &baseline_selected);
        for _ in 1..workload.sessions {
            let launched = Instant::now();
            let response = infinitty_request(socket, "new-tab /tmp")?;
            spawn_latencies.push(u64::try_from(launched.elapsed().as_micros())?);
            let id = response
                .parse::<u64>()
                .map_err(|_| "Infinitty returned invalid pane id")?;
            if !pane_ids.insert(id) {
                return Err("Infinitty returned duplicate pane id".into());
            }
            observe_root(root_pid, &mut resources)?;
        }
        let fleet_spawn_wall_us = u64::try_from(started.elapsed().as_micros())?;
        let listed = parse_infinitty_ids(&infinitty_request(socket, "list")?)?;
        if listed != pane_ids {
            return Err("Infinitty list differs from W5-owned pane set".into());
        }
        require_session_shape(u32::try_from(listed.len())?, workload)?;
        wait_trace_completion(root_pid, workload, &mut resources, &mut child, deadline)?;
        let (steady_rss_samples, steady_pid_count) = sample_steady(
            root_pid,
            workload,
            &mut resources,
            Some(&mut child),
            deadline,
        )?;
        let peak = resources.peak_rss_bytes();
        let steady = median(&steady_rss_samples)?;
        require_metrics(
            peak,
            steady,
            resources.distinct_pid_count(),
            steady_pid_count,
        )?;
        Ok(GlassArmResult {
            subject: "Infinitty v0.1.7 narrow Metal".to_owned(),
            version: "0.1.7@09b3e8b2aa3cec72a3c9a1db604c9c3c5235ee1c".to_owned(),
            availability: "measured-fresh-process".to_owned(),
            workload_mode: "20 app-socket panes replaying W4 trace".to_owned(),
            sessions_requested: workload.sessions,
            sessions_observed: Some(workload.sessions),
            preexisting_sessions: Some(0),
            events_per_session: Some(workload.trace_records_per_session),
            baseline_rss_bytes: Some(baseline_rss),
            peak_rss_bytes: Some(peak),
            steady_rss_bytes: Some(steady),
            steady_rss_samples,
            distinct_pid_count: Some(resources.distinct_pid_count()),
            steady_pid_count: Some(steady_pid_count),
            fleet_spawn_wall_us: Some(fleet_spawn_wall_us),
            spawn_latency_us: Some(percentiles(&spawn_latencies)?),
            reproduction: Some(format!("INFINITTY_APP={} {reproduction}", binary.display())),
            capability_gaps: "App-socket pane acknowledgement is measured; persistence/passive restore, capability authorization, supervisor attestation, and event→flushed-redraw receipts are N/A.".to_owned(),
            raw_artifact,
        })
    })();
    if let Ok(list) = infinitty_request(socket, "list") {
        if let Ok(ids) = parse_infinitty_ids(&list) {
            for id in ids {
                let _ = infinitty_request(socket, &format!("close {id}"));
            }
        }
    }
    let shutdown = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("Infinitty shutdown overflow")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= shutdown {
            terminate(&mut child);
            break None;
        }
        thread::sleep(sample_interval(workload));
    };
    let stderr = read_stderr(&mut child)?;
    let expected_socket = PathBuf::from(format!("/tmp/infinitty-app-{root_pid}.sock"));
    if matches!(fs::read_link(socket), Ok(target) if target == expected_socket) {
        let _ = fs::remove_file(socket);
    }
    let _ = fs::remove_file(expected_socket);
    if let Some(status) = status {
        if !status.success() && result.is_ok() {
            return Err(format!("Infinitty exited with {status}: {}", stderr.trim()).into());
        }
    }
    result
}

fn unavailable_arm(
    subject: &str,
    availability: &str,
    workload: &GlassWorkload,
    reproduction: String,
    capability_gaps: &str,
    raw_artifact: String,
) -> GlassArmResult {
    GlassArmResult {
        subject: subject.to_owned(),
        version: "N/A".to_owned(),
        availability: availability.to_owned(),
        workload_mode: "N/A".to_owned(),
        sessions_requested: workload.sessions,
        sessions_observed: None,
        preexisting_sessions: None,
        events_per_session: None,
        baseline_rss_bytes: None,
        peak_rss_bytes: None,
        steady_rss_bytes: None,
        steady_rss_samples: Vec::new(),
        distinct_pid_count: None,
        steady_pid_count: None,
        fleet_spawn_wall_us: None,
        spawn_latency_us: None,
        reproduction: Some(reproduction),
        capability_gaps: capability_gaps.to_owned(),
        raw_artifact,
    }
}

fn validate_complete_report(
    report: &GlassReport,
    workload: &GlassWorkload,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = [
        "remux std-only TUI (attested)",
        "cmux Ghostty-embedded incumbent",
        "ghostty vanilla Metal baseline",
        "Infinitty v0.1.7 narrow Metal",
    ];
    if report.arms.len() != expected.len() {
        return Err("glass report arm count differs".into());
    }
    for (arm, expected_subject) in report.arms.iter().zip(expected) {
        if arm.subject != expected_subject || arm.sessions_requested != workload.sessions {
            return Err("glass report arm identity differs".into());
        }
        if arm.availability.starts_with("measured") {
            require_session_shape(
                arm.sessions_observed.ok_or("measured arm misses shape")?,
                workload,
            )?;
            require_metrics(
                arm.peak_rss_bytes.ok_or("measured arm misses peak RSS")?,
                arm.steady_rss_bytes
                    .ok_or("measured arm misses steady RSS")?,
                arm.distinct_pid_count
                    .ok_or("measured arm misses PID union")?,
                arm.steady_pid_count
                    .ok_or("measured arm misses steady PIDs")?,
            )?;
            if arm.fleet_spawn_wall_us.unwrap_or(0) == 0 {
                return Err("measured arm misses spawn cost".into());
            }
        } else if arm.peak_rss_bytes.is_some()
            || arm.steady_rss_bytes.is_some()
            || arm.distinct_pid_count.is_some()
            || arm.fleet_spawn_wall_us.is_some()
        {
            return Err("unavailable arm contains guessed measurements".into());
        }
    }
    if report.cmux_hook.events != workload.cmux_hook_events
        || report.cmux_hook.distinct_hook_pids != u64::from(workload.cmux_hook_events)
        || (report.cmux_hook.per_event_forks - 1.0).abs() > f64::EPSILON
        || report.cmux_hook.peak_hook_rss_bytes == 0
    {
        return Err("cmux hook receipt is incomplete".into());
    }
    Ok(())
}

fn render_arm_artifact(
    arm: &GlassArmResult,
    state: &GlassMachineState,
    workload_sha256: &str,
) -> io::Result<String> {
    for value in [
        arm.subject.as_str(),
        arm.version.as_str(),
        arm.availability.as_str(),
        arm.workload_mode.as_str(),
        arm.capability_gaps.as_str(),
        arm.raw_artifact.as_str(),
    ] {
        validate_tsv_value(value)?;
    }
    Ok(format!(
        "schema\t1\nworkload_sha256\t{}\nadmission\tpass\nload_one\t{:.2}\nlogical_cpus\t{}\nfree_memory_percent\t{}\nsubject\t{}\nversion\t{}\navailability\t{}\nworkload_mode\t{}\nsessions_requested\t{}\nsessions_observed\t{}\npreexisting_sessions\t{}\nevents_per_session\t{}\nbaseline_rss_bytes\t{}\npeak_rss_bytes\t{}\nsteady_rss_bytes\t{}\nsteady_rss_samples\t{}\ndistinct_pid_count\t{}\nsteady_pid_count\t{}\nfleet_spawn_wall_us\t{}\nspawn_latency_p50_us\t{}\nspawn_latency_p95_us\t{}\nspawn_latency_p99_us\t{}\ncapability_gaps\t{}\n",
        workload_sha256,
        state.load_one,
        state.logical_cpus,
        state.free_memory_percent,
        arm.subject,
        arm.version,
        arm.availability,
        arm.workload_mode,
        arm.sessions_requested,
        optional_u32(arm.sessions_observed),
        optional_u32(arm.preexisting_sessions),
        optional_u64(arm.events_per_session),
        optional_u64(arm.baseline_rss_bytes),
        optional_u64(arm.peak_rss_bytes),
        optional_u64(arm.steady_rss_bytes),
        if arm.steady_rss_samples.is_empty() {
            "N/A".to_owned()
        } else {
            arm.steady_rss_samples
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        },
        optional_u64(arm.distinct_pid_count),
        optional_u64(arm.steady_pid_count),
        optional_u64(arm.fleet_spawn_wall_us),
        arm.spawn_latency_us
            .as_ref()
            .map_or_else(|| "N/A".to_owned(), |value| value.p50.to_string()),
        arm.spawn_latency_us
            .as_ref()
            .map_or_else(|| "N/A".to_owned(), |value| value.p95.to_string()),
        arm.spawn_latency_us
            .as_ref()
            .map_or_else(|| "N/A".to_owned(), |value| value.p99.to_string()),
        arm.capability_gaps
    ))
}

fn render_hook_artifact(
    hook: &CmuxHookResult,
    state: &GlassMachineState,
    workload_sha256: &str,
) -> io::Result<String> {
    for value in [
        hook.version.as_str(),
        hook.command.as_str(),
        hook.observation.as_str(),
    ] {
        validate_tsv_value(value)?;
    }
    Ok(format!(
        "schema\t1\nworkload_sha256\t{workload_sha256}\nadmission\tpass\nload_one\t{:.2}\nlogical_cpus\t{}\nfree_memory_percent\t{}\nversion\t{}\nevents\t{}\ndistinct_hook_pids\t{}\nper_event_forks\t{:.6}\npeak_hook_rss_bytes\t{}\nwall_p50_us\t{}\nwall_p95_us\t{}\nwall_p99_us\t{}\ncommand\t{}\nobservation\t{}\n",
        state.load_one,
        state.logical_cpus,
        state.free_memory_percent,
        hook.version,
        hook.events,
        hook.distinct_hook_pids,
        hook.per_event_forks,
        hook.peak_hook_rss_bytes,
        hook.wall_latency_us.p50,
        hook.wall_latency_us.p95,
        hook.wall_latency_us.p99,
        hook.command,
        hook.observation
    ))
}

fn validate_tsv_value(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\t' | b'\r' | b'\n'))
    {
        Err(io::Error::other("raw artifact value contains a separator"))
    } else {
        Ok(())
    }
}

fn make_workload_command(
    root: &Path,
    trace_agent: &Path,
    trace: &Path,
    hold_ms: u64,
) -> io::Result<PathBuf> {
    let path = root.join("w5-agent-workload");
    let trace_agent = shell_quote(path_text_io(trace_agent)?);
    let trace = shell_quote(path_text_io(trace)?);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nexec {trace_agent} --trace {trace} --start-delay-us 0 --hold-after-trace-ms {hold_ms}\n"
        ),
    )?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ghostty_script() -> &'static str {
    r#"on run argv
set operation to item 1 of argv
using terms from application "Ghostty"
tell application id "com.mitchellh.ghostty"
if operation is "total" then
return (count of terminals) as text
else if operation is "create" then
set commandText to item 2 of argv
set workdir to item 3 of argv
set config to new surface configuration from {initial working directory:workdir, command:commandText, wait after command:true}
set targetWindow to new window with configuration config
return (id of targetWindow) as text
else
set targetId to item 2 of argv
set matches to every window whose id is targetId
if (count of matches) is not 1 then error "Ghostty W5 window not found"
set targetWindow to item 1 of matches
if operation is "add" then
set commandText to item 3 of argv
set workdir to item 4 of argv
set config to new surface configuration from {initial working directory:workdir, command:commandText, wait after command:true}
set createdTab to new tab in targetWindow with configuration config
return (id of createdTab) as text
else if operation is "window" then
return (count of terminals of targetWindow) as text
else if operation is "close" then
close window targetWindow
return "closed"
else
error "unknown Ghostty W5 operation"
end if
end if
end tell
end using terms from
end run
"#
}

fn ghostty_output(script: &Path, arguments: &[&str]) -> io::Result<Output> {
    Command::new("/usr/bin/osascript")
        .arg(script)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
}

fn ghostty_number(script: &Path, arguments: &[&str]) -> Result<u32, Box<dyn std::error::Error>> {
    let output = ghostty_output(script, arguments)?;
    require_success("Ghostty AppleScript count", &output)?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}

fn cmux_socket() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(explicit) = env::var_os("CMUX_SOCKET_PATH") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(path);
        }
        return Err("CMUX_SOCKET_PATH does not exist".into());
    }
    let home = env::var_os("HOME").ok_or("HOME is absent")?;
    let candidates = [
        PathBuf::from(&home).join(".local/state/cmux/cmux.sock"),
        PathBuf::from(home).join(".local/state/cmux/cmux-501.sock"),
    ];
    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| "no running cmux socket is present".into())
}

fn cmux_output(socket: &Path, arguments: &[&str], input: Option<&[u8]>) -> io::Result<Output> {
    let mut command = Command::new(CMUX_CLI);
    command
        .env("CMUX_SOCKET_PATH", socket)
        .env("CMUX_QUIET", "1")
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn()?;
    if let Some(bytes) = input {
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("cmux stdin missing"))?
            .write_all(bytes)?;
    }
    child.wait_with_output()
}

fn cmux_workspace_ids(socket: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let output = cmux_output(socket, &["--id-format", "uuids", "list-workspaces"], None)?;
    require_success("cmux list-workspaces", &output)?;
    let text = String::from_utf8(output.stdout)?;
    let ids = text.lines().filter_map(first_uuid).collect::<BTreeSet<_>>();
    if ids.is_empty() && !text.trim().is_empty() {
        return Err("cmux workspace list contains no parseable UUID".into());
    }
    Ok(ids)
}

fn first_uuid(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|field| is_uuid(field))
        .map(ToOwned::to_owned)
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn infinitty_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Infinitty.app/Contents/MacOS/Infinitty"),
        PathBuf::from("/Applications/Infinitty.app/Contents/MacOS/infinitty"),
    ];
    if let Some(home) = env::var_os("HOME") {
        candidates
            .push(PathBuf::from(&home).join("Applications/Infinitty.app/Contents/MacOS/Infinitty"));
        candidates
            .push(PathBuf::from(home).join("Applications/Infinitty.app/Contents/MacOS/infinitty"));
    }
    if let Some(explicit) = env::var_os("INFINITTY_APP") {
        candidates.push(PathBuf::from(explicit));
    }
    candidates
}

fn infinitty_request(path: &Path, request: &str) -> io::Result<String> {
    use std::os::unix::net::UnixStream;

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

fn parse_infinitty_ids(list: &str) -> io::Result<BTreeSet<u64>> {
    let mut ids = BTreeSet::new();
    let mut remaining = list;
    let marker = "\"id\":";
    while let Some(index) = remaining.find(marker) {
        remaining = &remaining[index + marker.len()..];
        let value = remaining.trim_start();
        let digit_count = value.bytes().take_while(u8::is_ascii_digit).count();
        let suffix = value[digit_count..].trim_start();
        if digit_count == 0 || !(suffix.starts_with(',') || suffix.starts_with('}')) {
            return Err(io::Error::other("Infinitty list has invalid pane id"));
        }
        let id = value[..digit_count]
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

fn wait_trace_completion(
    root_pid: u32,
    workload: &GlassWorkload,
    resources: &mut ResourceTracker,
    child: &mut Child,
    deadline: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let wait = Duration::from_micros(workload.trace_last_event_us.saturating_add(250_000));
    let until = Instant::now()
        .checked_add(wait)
        .ok_or("trace wait overflow")?;
    while Instant::now() < until {
        observe_root(root_pid, resources)?;
        if let Some(status) = child.try_wait()? {
            return Err(format!("subject exited before W4 trace completed: {status}").into());
        }
        if Instant::now() >= deadline {
            return Err("subject timed out before trace completion".into());
        }
        thread::sleep(sample_interval(workload));
    }
    Ok(())
}

fn wait_trace_completion_without_child(
    root_pid: u32,
    workload: &GlassWorkload,
    resources: &mut ResourceTracker,
) -> Result<(), Box<dyn std::error::Error>> {
    let wait = Duration::from_micros(workload.trace_last_event_us.saturating_add(250_000));
    let until = Instant::now()
        .checked_add(wait)
        .ok_or("trace wait overflow")?;
    while Instant::now() < until {
        observe_root(root_pid, resources)?;
        thread::sleep(sample_interval(workload));
    }
    Ok(())
}

fn sample_steady(
    root_pid: u32,
    workload: &GlassWorkload,
    resources: &mut ResourceTracker,
    mut child: Option<&mut Child>,
    deadline: Instant,
) -> Result<(Vec<u64>, u64), Box<dyn std::error::Error>> {
    let until = Instant::now()
        .checked_add(Duration::from_millis(workload.steady_window_ms))
        .ok_or("steady deadline overflow")?;
    let mut rss = Vec::new();
    let mut final_count = 0;
    while Instant::now() < until {
        let snapshot = system::snapshot()?;
        let selected = system::descendants(&snapshot, root_pid);
        resources.observe(&snapshot, &selected);
        let current = system::selected_rss_bytes(&snapshot, &selected);
        final_count = system::selected_process_count(&snapshot, &selected);
        if current == 0 || final_count == 0 {
            return Err("steady sampler lost the subject tree".into());
        }
        rss.push(current);
        if let Some(process) = child.as_deref_mut() {
            if let Some(status) = process.try_wait()? {
                return Err(format!("subject exited during steady window with {status}").into());
            }
        }
        if Instant::now() >= deadline {
            return Err("subject exceeded steady deadline".into());
        }
        thread::sleep(sample_interval(workload));
    }
    if rss.is_empty() {
        return Err("steady sampler emitted no samples".into());
    }
    Ok((rss, final_count))
}

fn observe_root(root_pid: u32, resources: &mut ResourceTracker) -> io::Result<()> {
    let snapshot = system::snapshot()?;
    let selected = system::descendants(&snapshot, root_pid);
    if !selected.contains(&root_pid) || !snapshot.iter().any(|entry| entry.pid == root_pid) {
        return Err(io::Error::other("subject root PID is absent"));
    }
    resources.observe(&snapshot, &selected);
    Ok(())
}

fn require_metrics(peak: u64, steady: u64, distinct: u64, steady_pids: u64) -> io::Result<()> {
    if peak == 0 || steady == 0 || distinct == 0 || steady_pids == 0 {
        Err(io::Error::other("required resource metric is empty"))
    } else {
        Ok(())
    }
}

fn admitted_machine(workload: &GlassWorkload) -> io::Result<GlassMachineState> {
    let state = machine_state()?;
    admit_machine(&state, workload)?;
    Ok(state)
}

fn machine_state() -> io::Result<GlassMachineState> {
    let load = command_output("/usr/sbin/sysctl", &["-n", "vm.loadavg"])?;
    let load_one = load
        .trim_matches(|character: char| matches!(character, '{' | '}' | ' '))
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::other("vm.loadavg is empty"))?
        .parse::<f64>()
        .map_err(|_| io::Error::other("vm.loadavg is invalid"))?;
    let logical_cpus = command_output("/usr/sbin/sysctl", &["-n", "hw.logicalcpu"])?
        .parse::<u32>()
        .map_err(|_| io::Error::other("logical CPU count is invalid"))?;
    let memory = command_output("/usr/bin/memory_pressure", &["-Q"])?;
    let prefix = "System-wide memory free percentage:";
    let free_memory_percent = memory
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(str::trim)
        .and_then(|value| value.strip_suffix('%'))
        .ok_or_else(|| io::Error::other("memory pressure free percentage is absent"))?
        .parse::<u8>()
        .map_err(|_| io::Error::other("memory free percentage is invalid"))?;
    Ok(GlassMachineState {
        load_one,
        logical_cpus,
        free_memory_percent,
    })
}

fn verify_trace_contract(workspace: &Path, workload: &GlassWorkload) -> io::Result<()> {
    let input = fs::read_to_string(workspace.join(&workload.trace))?;
    if !input.ends_with('\n') || input.lines().next() != Some("REMUX_TRACE_V1") {
        return Err(io::Error::other("W5 trace framing differs"));
    }
    let records = input.lines().skip(6).collect::<Vec<_>>();
    if records.len()
        != usize::try_from(workload.trace_records_per_session).map_err(io::Error::other)?
    {
        return Err(io::Error::other("W5 trace record count differs"));
    }
    let final_time = records
        .last()
        .and_then(|line| line.split_once('\t'))
        .and_then(|(time, _)| time.parse::<u64>().ok())
        .ok_or_else(|| io::Error::other("W5 trace final time is invalid"))?;
    if final_time != workload.trace_last_event_us {
        return Err(io::Error::other("W5 trace final time differs"));
    }
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
        Err(format!("offline workspace preparation failed with {status}").into())
    }
}

fn authorize(
    supervisor: &Path,
    auth_log: &Path,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(supervisor)
        .args([
            "authorize",
            "--auth-log",
            path_text(auth_log)?,
            "--token",
            token,
            "--scope",
            "launch",
        ])
        .output()?;
    require_success("remux lifecycle authorization", &output)
}

fn parse_ready(path: &Path, root_pid: u32, sessions: u32) -> io::Result<BTreeSet<u32>> {
    let fields = parse_tsv(path)?;
    if parse_u64_field(&fields, "pid")? != u64::from(root_pid)
        || parse_u64_field(&fields, "children")? != u64::from(sessions)
    {
        return Err(io::Error::other("remux ready shape differs"));
    }
    let children = fields
        .get("child_pids")
        .ok_or_else(|| io::Error::other("remux ready child PIDs absent"))?
        .split(',')
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| io::Error::other("remux ready child PID invalid"))
        })
        .collect::<io::Result<BTreeSet<_>>>()?;
    if children.len() != usize::try_from(sessions).map_err(io::Error::other)? {
        return Err(io::Error::other("remux ready child PID count differs"));
    }
    let mut all = children;
    all.insert(root_pid);
    Ok(all)
}

fn parse_tsv(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let input = fs::read_to_string(path)?;
    let mut fields = BTreeMap::new();
    for line in input.lines() {
        let (key, value) = line
            .split_once('\t')
            .ok_or_else(|| io::Error::other("invalid TSV field"))?;
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(io::Error::other("duplicate TSV field"));
        }
    }
    Ok(fields)
}

fn expect_field(fields: &BTreeMap<String, String>, key: &str, expected: &str) -> io::Result<()> {
    if fields.get(key).map(String::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(io::Error::other(format!("field {key} differs")))
    }
}

fn parse_u64_field(fields: &BTreeMap<String, String>, key: &str) -> io::Result<u64> {
    fields
        .get(key)
        .ok_or_else(|| io::Error::other(format!("field {key} absent")))?
        .parse::<u64>()
        .map_err(|_| io::Error::other(format!("field {key} invalid")))
}

fn exact_process_pid(command_path: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let output = command_output("/bin/ps", &["-axo", "pid=,comm="])?;
    let pids = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let command = fields.next()?;
            (command == command_path).then_some(pid)
        })
        .collect::<Vec<_>>();
    match pids.as_slice() {
        [pid] => Ok(*pid),
        [] => Err(format!("subject process is not running: {command_path}").into()),
        _ => Err(format!("multiple subject processes are running: {command_path}").into()),
    }
}

fn plist_value(path: &str, key: &str) -> io::Result<String> {
    command_output("/usr/bin/plutil", &["-extract", key, "raw", path])
}

fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()?;
    require_success("workload SHA-256", &output)?;
    let text = String::from_utf8(output.stdout)?;
    let digest = text
        .split_whitespace()
        .next()
        .ok_or("SHA-256 output empty")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("workload SHA-256 is invalid".into());
    }
    Ok(digest.to_owned())
}

fn git_short_revision(workspace: &Path) -> io::Result<String> {
    let output = Command::new("/usr/bin/git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("git revision query failed"));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| io::Error::other("git revision is not UTF-8"))
}

fn machine() -> Result<Machine, Box<dyn std::error::Error>> {
    let version = command_output("/usr/bin/sw_vers", &["-productVersion"])?;
    let rustc = command_output(
        env::var("RUSTC").as_deref().unwrap_or("rustc"),
        &["--version"],
    )?;
    Ok(Machine {
        os: format!("{} {version}", env::consts::OS),
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

fn require_success(label: &str, output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{label} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}

fn process_error(label: &str, status: std::process::ExitStatus, child: &mut Child) -> String {
    let stderr = read_stderr(child).unwrap_or_else(|error| format!("stderr read failed: {error}"));
    format!("{label} with {status}: {}", stderr.trim())
}

fn read_stderr(child: &mut Child) -> io::Result<String> {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    Ok(stderr)
}

fn median(samples: &[u64]) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(percentiles(samples)?.p50)
}

fn arm_deadline(workload: &GlassWorkload) -> Result<Instant, Box<dyn std::error::Error>> {
    Instant::now()
        .checked_add(Duration::from_secs(workload.arm_timeout_seconds))
        .ok_or_else(|| "arm deadline overflow".into())
}

fn sample_interval(workload: &GlassWorkload) -> Duration {
    Duration::from_millis(workload.sample_interval_ms)
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |number| number.to_string())
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |number| number.to_string())
}

fn display_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |number| number.to_string())
}

fn relative_artifact(output_directory: &Path, run_id: &str, name: &str) -> String {
    output_directory
        .join(run_id)
        .join(name)
        .display()
        .to_string()
}

fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("atomic output name is not UTF-8"))?;
    let temporary = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn path_text(path: &Path) -> Result<&str, Box<dyn std::error::Error>> {
    path.to_str().ok_or_else(|| "path is not UTF-8".into())
}

fn path_text_io(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("path is not UTF-8"))
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn unix_seconds() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let path = PathBuf::from(format!(
            "/tmp/rmx-glass-{}-{}",
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
    workload: PathBuf,
    output_directory: PathBuf,
}

impl Config {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut workload = PathBuf::from(WORKLOAD_RELATIVE_PATH);
        let mut output_directory = PathBuf::from("bench/results/w5");
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--workload" => workload = PathBuf::from(value),
                "--output-dir" => output_directory = PathBuf::from(value),
                _ => return Err(format!("unknown glass-bench flag {flag}").into()),
            }
        }
        if workload.is_absolute() || output_directory.is_absolute() {
            return Err("glass-bench paths must be workspace-relative".into());
        }
        Ok(Self {
            workload,
            output_directory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{first_uuid, is_uuid, parse_infinitty_ids, Config, WORKLOAD_RELATIVE_PATH};
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn public_subject_identifiers_fail_closed() {
        let id = "5E044570-9C7E-445F-8775-5C1472ABC515";
        assert!(is_uuid(id));
        assert_eq!(
            first_uuid(&format!("workspace:1 {id} title")).as_deref(),
            Some(id)
        );
        for invalid in [
            "5E0445709C7E445F87755C1472ABC515",
            "5E044570-9C7E-445F-8775-5C1472ABC51Z",
            "../../owned",
            "",
        ] {
            assert!(!is_uuid(invalid));
        }
        assert_eq!(
            parse_infinitty_ids(r#"[{"id":1},{"id": 20}]"#).expect("parse pane ids"),
            BTreeSet::from([1, 20])
        );
        for invalid in ["[]", r#"[{"id":}]"#, r#"[{"id":1},{"id":1}]"#] {
            assert!(parse_infinitty_ids(invalid).is_err());
        }
    }

    #[test]
    fn config_is_bounded_to_workspace_relative_paths() {
        let default = Config::parse(std::iter::empty()).expect("parse default glass config");
        assert_eq!(default.workload, Path::new(WORKLOAD_RELATIVE_PATH));
        assert!(Config::parse(["--workload".to_owned()].into_iter()).is_err());
        assert!(
            Config::parse(["--output-dir".to_owned(), "/tmp/out".to_owned()].into_iter()).is_err()
        );
        assert!(Config::parse(["--unknown".to_owned(), "x".to_owned()].into_iter()).is_err());
    }
}
