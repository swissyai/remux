// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
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
    Machine, ScenarioResult,
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);
const SUPERVISOR_RSS_LIMIT_BYTES: u64 = 200 * 1024 * 1024;
const BENCHMARK_BUDGET_SECONDS: f64 = 300.0;
const FORK_MEMORY_RAIL_MIB: u64 = 512;

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
    let supervisor_result = run_supervisor_baseline(
        &config,
        &binary_directory.join("remux-supervisor"),
        &binary_directory.join("fake-agent"),
        command,
    )?;
    if supervisor_result.peak_rss_bytes >= SUPERVISOR_RSS_LIMIT_BYTES {
        return Err(format!(
            "supervisor peak RSS {:.2}MiB exceeds 200MiB contract",
            bytes_to_mib(supervisor_result.peak_rss_bytes)
        )
        .into());
    }
    if supervisor_result.per_event_forks != 0 {
        return Err("supervisor reported per-event forks".into());
    }

    let generated_unix_seconds = unix_seconds()?;
    let run_id = format!("run-{generated_unix_seconds}-{}", std::process::id());
    let report = BenchmarkReport {
        run_id: run_id.clone(),
        generated_unix_seconds,
        machine: machine()?,
        config: config.report_config(),
        results: vec![fork_result, supervisor_result],
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
    for result in report.results {
        println!(
            "{}: {} events, {} spawns, {:.2}MiB peak RSS, {:.2} events/s",
            result.model,
            result.events,
            result.processes_spawned,
            bytes_to_mib(result.peak_rss_bytes),
            result.events_per_second
        );
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
    Ok(ScenarioResult {
        model: "fork_per_event".to_owned(),
        sessions: config.sessions,
        events: total_events,
        processes_spawned: spawned,
        per_event_forks: spawned,
        peak_rss_bytes,
        events_per_second: total_events as f64 / wall_seconds,
        latency_us: latency_us.clone(),
        cpu_seconds: resources.cpu_seconds(),
        wall_seconds,
        command,
        interpretation: format!(
            "One process per event produced {spawned} forks; p50 completion was {:.1}ms versus the configured {}ms hold.",
            latency_us.p50 as f64 / 1_000.0,
            config.fork_hold_ms
        ),
    })
}

fn run_supervisor_baseline(
    config: &Config,
    supervisor: &Path,
    fake_agent: &Path,
    command: String,
) -> Result<ScenarioResult, Box<dyn std::error::Error>> {
    let temporary = TemporaryDirectory::new()?;
    let socket = temporary.path.join("s");
    let state = temporary.path.join("state.json");
    let metrics = temporary.path.join("metrics.tsv");
    let ready = temporary.path.join("ready.tsv");
    let mut child = Command::new(supervisor)
        .args([
            "run",
            "--sessions",
            &config.sessions.to_string(),
            "--events-per-session",
            &config.events_per_session.to_string(),
            "--rate",
            &config.rate.to_string(),
            "--socket",
            path_text(&socket)?,
            "--state",
            path_text(&state)?,
            "--metrics",
            path_text(&metrics)?,
            "--ready",
            path_text(&ready)?,
            "--fake-agent",
            path_text(fake_agent)?,
            "--timeout-seconds",
            &config.supervisor_timeout_seconds().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let root_pid = child.id();
    let started = Instant::now();
    let timeout = Duration::from_secs_f64(config.estimated_supervisor_seconds() + 20.0);
    let mut resources = ResourceTracker::default();
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
        if ready.exists() && !dump_sent {
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

    let parsed = parse_metrics(&metrics)?;
    let total_events = config.total_events()?;
    expect_metric(&parsed, "schema", 1)?;
    expect_metric(&parsed, "events_ingested", total_events)?;
    expect_metric(&parsed, "children_spawned", u64::from(config.sessions))?;
    expect_metric(&parsed, "per_event_forks", 0)?;
    if metric(&parsed, "on_demand_dumps")? == 0 {
        return Err("on-demand state dump was not observed".into());
    }
    if metric(&parsed, "pty_bytes")? == 0 {
        return Err("PTY sessions produced no output".into());
    }
    let latencies = parsed
        .get("latencies_us")
        .ok_or("metrics missing latencies_us")?
        .split(',')
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| io::Error::other("invalid latency metric"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if latencies.len() != usize::try_from(total_events)? {
        return Err("latency sample count differs from event count".into());
    }
    verify_passive_restore(supervisor, &state)?;

    let latency_us = percentiles(&latencies)?;
    let peak_rss_bytes = resources.peak_rss_bytes();
    Ok(ScenarioResult {
        model: "socket_supervisor".to_owned(),
        sessions: config.sessions,
        events: total_events,
        processes_spawned: u64::from(config.sessions) + 1,
        per_event_forks: 0,
        peak_rss_bytes,
        events_per_second: total_events as f64 / wall_seconds,
        latency_us,
        cpu_seconds: resources.cpu_seconds(),
        wall_seconds,
        command,
        interpretation: format!(
            "{} PTY sessions ingested {total_events} events through one socket with zero per-event forks at {:.2}MiB peak RSS.",
            config.sessions,
            bytes_to_mib(peak_rss_bytes)
        ),
    })
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
                    config.events_per_session = parse_positive(&value, "events-per-session")?
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
        if self.estimated_fork_seconds() >= BENCHMARK_BUDGET_SECONDS
            || self.estimated_supervisor_seconds() >= BENCHMARK_BUDGET_SECONDS
        {
            return Err("configuration exceeds five-minute benchmark budget".into());
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
        self.estimated_supervisor_seconds().ceil() as u64 + 10
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
