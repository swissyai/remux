// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! PTY supervisor tracer bullet: one process, one listener socket, batched state.

use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use supervisor::protocol::{parse_message, unix_micros_now, Control, Message};
use supervisor::pty::spawn_pty;
use supervisor::state::{dump_atomic, restore_passive, LiveState};

const MAX_MESSAGE_BYTES: usize = 4_096;
const MAX_BATCH_MESSAGES: usize = 256;

fn main() {
    if let Err(error) = entrypoint() {
        eprintln!("remux-supervisor: {error}");
        std::process::exit(1);
    }
}

fn entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("run") => run_supervisor(RunConfig::parse(arguments)?),
        Some("dump") => request_dump(DumpConfig::parse(arguments)?),
        Some("restore") => inspect_restore(RestoreConfig::parse(arguments)?),
        _ => Err(usage().into()),
    }
}

fn run_supervisor(config: RunConfig) -> Result<(), Box<dyn std::error::Error>> {
    prepare_output(&config.socket)?;
    prepare_output(&config.ready_file)?;
    prepare_output(&config.metrics_file)?;
    let listener = UnixListener::bind(&config.socket)?;
    listener.set_nonblocking(true)?;
    let _socket_guard = SocketGuard(config.socket.clone());
    let stopping = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    let listener_thread = spawn_listener(listener, Arc::clone(&stopping), sender);

    let session_ids = (0..config.sessions)
        .map(|index| format!("session-{index:03}"))
        .collect::<Vec<_>>();
    let mut state = LiveState::new(session_ids.iter().cloned())?;
    let interval_us = u64::from(config.sessions)
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_div(config.rate))
        .ok_or("invalid event interval")?;
    let stagger_us = 1_000_000_u64
        .checked_div(config.rate)
        .ok_or("invalid event rate")?;
    let mut children = Vec::with_capacity(config.sessions as usize);
    let mut pty_readers = Vec::with_capacity(config.sessions as usize);
    for (index, session_id) in session_ids.iter().enumerate() {
        let mut command = Command::new(&config.fake_agent);
        command.args([
            "--socket",
            path_text(&config.socket)?,
            "--session-id",
            session_id,
            "--events",
            &config.events_per_session.to_string(),
            "--interval-us",
            &interval_us.to_string(),
            "--start-delay-us",
            &stagger_us
                .checked_mul(u64::try_from(index)?)
                .ok_or("start stagger overflow")?
                .to_string(),
        ]);
        let (child, master) = spawn_pty(&mut command)?;
        children.push(child);
        pty_readers.push(spawn_pty_drain(master));
    }
    write_ready(&config.ready_file, &config.socket, children.len())?;

    let expected_events = u64::from(config.sessions)
        .checked_mul(config.events_per_session)
        .ok_or("expected event count overflow")?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(config.timeout_seconds))
        .ok_or("supervisor timeout overflow")?;
    let mut events_ingested = 0_u64;
    let mut batches = 0_u64;
    let mut on_demand_dumps = 0_u64;
    let mut latencies_us = Vec::with_capacity(usize::try_from(expected_events)?);
    while events_ingested < expected_events {
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out after ingesting {events_ingested}/{expected_events} events"
            )
            .into());
        }
        let first = match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                fail_if_child_exited(&mut children)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("ingest channel disconnected".into())
            }
        };
        let mut messages = Vec::with_capacity(MAX_BATCH_MESSAGES);
        messages.push(first);
        while messages.len() < MAX_BATCH_MESSAGES {
            match receiver.try_recv() {
                Ok(message) => messages.push(message),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let mut events = Vec::with_capacity(messages.len());
        let mut dump_requested = false;
        for inbound in messages {
            match inbound? {
                Message::Event(event) => {
                    let received = unix_micros_now()?;
                    latencies_us.push(received.saturating_sub(event.sent_unix_micros));
                    events.push(event);
                }
                Message::Control(Control::Dump) => dump_requested = true,
            }
        }
        if !events.is_empty() {
            events_ingested = events_ingested
                .checked_add(u64::try_from(events.len())?)
                .ok_or("ingested event count overflow")?;
            state.apply_batch(&events)?;
        }
        if dump_requested {
            dump_atomic(&config.state_file, &state)?;
            on_demand_dumps += 1;
        }
        batches += 1;
    }

    for child in &mut children {
        let status = child.wait()?;
        if !status.success() {
            return Err(format!("fake agent exited with {status}").into());
        }
    }
    let mut pty_bytes = 0_u64;
    for reader in pty_readers {
        pty_bytes = pty_bytes
            .checked_add(reader.join().map_err(|_| "PTY reader panicked")??)
            .ok_or("PTY byte count overflow")?;
    }
    dump_atomic(&config.state_file, &state)?;
    write_metrics(
        &config.metrics_file,
        &RunMetrics {
            events_ingested,
            batches,
            children_spawned: u64::from(config.sessions),
            per_event_forks: 0,
            pty_bytes,
            on_demand_dumps,
            latencies_us,
        },
    )?;
    stopping.store(true, Ordering::Release);
    listener_thread
        .join()
        .map_err(|_| "socket listener panicked")??;
    Ok(())
}

fn request_dump(config: DumpConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(config.socket)?;
    stream.write_all(b"control\tdump\n")?;
    Ok(())
}

fn inspect_restore(config: RestoreConfig) -> Result<(), Box<dyn std::error::Error>> {
    let state = restore_passive(&config.state_file)?;
    println!(
        "restored passive layout: {} sessions, policy {}",
        state.sessions.len(),
        state.restore_policy
    );
    Ok(())
}

fn spawn_listener(
    listener: UnixListener,
    stopping: Arc<AtomicBool>,
    sender: mpsc::Sender<io::Result<Message>>,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut readers = Vec::new();
        while !stopping.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Accepted sockets inherit O_NONBLOCK on Darwin. Reader threads are
                    // intentionally blocking so a temporary lack of data is not EOF.
                    stream.set_nonblocking(false)?;
                    let sender = sender.clone();
                    readers.push(thread::spawn(move || read_stream(stream, sender)));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error),
            }
        }
        drop(sender);
        for reader in readers {
            reader
                .join()
                .map_err(|_| io::Error::other("socket reader panicked"))??;
        }
        Ok(())
    })
}

fn read_stream(stream: UnixStream, sender: mpsc::Sender<io::Result<Message>>) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(());
        }
        let parsed = if bytes > MAX_MESSAGE_BYTES {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "socket message exceeds limit",
            ))
        } else {
            parse_message(&line)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
        };
        let failed = parsed.is_err();
        if sender.send(parsed).is_err() {
            return Ok(());
        }
        if failed {
            return Ok(());
        }
    }
}

fn spawn_pty_drain(mut master: File) -> JoinHandle<io::Result<u64>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4_096];
        let mut total = 0_u64;
        loop {
            match master.read(&mut buffer) {
                Ok(0) => return Ok(total),
                Ok(bytes) => {
                    total = total
                        .checked_add(u64::try_from(bytes).map_err(io::Error::other)?)
                        .ok_or_else(|| io::Error::other("PTY byte count overflow"))?;
                }
                Err(error) if error.raw_os_error() == Some(5) => return Ok(total),
                Err(error) => return Err(error),
            }
        }
    })
}

fn fail_if_child_exited(children: &mut [Child]) -> Result<(), Box<dyn std::error::Error>> {
    for child in children {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!("fake agent exited early with {status}").into());
            }
        }
    }
    Ok(())
}

fn write_ready(path: &Path, socket: &Path, children: usize) -> io::Result<()> {
    fs::write(
        path,
        format!(
            "pid\t{}\nsocket\t{}\nchildren\t{}\n",
            std::process::id(),
            socket.display(),
            children
        ),
    )
}

struct RunMetrics {
    events_ingested: u64,
    batches: u64,
    children_spawned: u64,
    per_event_forks: u64,
    pty_bytes: u64,
    on_demand_dumps: u64,
    latencies_us: Vec<u64>,
}

fn write_metrics(path: &Path, metrics: &RunMetrics) -> io::Result<()> {
    let latencies = metrics
        .latencies_us
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        path,
        format!(
            "schema\t1\nevents_ingested\t{}\nbatches\t{}\nchildren_spawned\t{}\nper_event_forks\t{}\npty_bytes\t{}\non_demand_dumps\t{}\nlatencies_us\t{}\n",
            metrics.events_ingested,
            metrics.batches,
            metrics.children_spawned,
            metrics.per_event_forks,
            metrics.pty_bytes,
            metrics.on_demand_dumps,
            latencies
        ),
    )
}

fn prepare_output(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn path_text(path: &Path) -> Result<&str, Box<dyn std::error::Error>> {
    path.to_str().ok_or_else(|| "path is not UTF-8".into())
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct RunConfig {
    sessions: u32,
    events_per_session: u64,
    rate: u64,
    socket: PathBuf,
    state_file: PathBuf,
    metrics_file: PathBuf,
    ready_file: PathBuf,
    fake_agent: PathBuf,
    timeout_seconds: u64,
}

impl RunConfig {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut sessions = 20;
        let mut events_per_session = 6;
        let mut rate = 20;
        let mut socket = PathBuf::from("remux.sock");
        let mut state_file = PathBuf::from("remux-state.json");
        let mut metrics_file = PathBuf::from("remux-metrics.tsv");
        let mut ready_file = PathBuf::from("remux-ready.tsv");
        let mut fake_agent = sibling_binary("fake-agent")?;
        let mut timeout_seconds = 60;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--sessions" => sessions = parse_positive(&value, "sessions")?,
                "--events-per-session" => {
                    events_per_session = parse_positive(&value, "events-per-session")?
                }
                "--rate" => rate = parse_positive(&value, "rate")?,
                "--socket" => socket = PathBuf::from(value),
                "--state" => state_file = PathBuf::from(value),
                "--metrics" => metrics_file = PathBuf::from(value),
                "--ready" => ready_file = PathBuf::from(value),
                "--fake-agent" => fake_agent = PathBuf::from(value),
                "--timeout-seconds" => timeout_seconds = parse_positive(&value, "timeout-seconds")?,
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(Self {
            sessions,
            events_per_session,
            rate,
            socket,
            state_file,
            metrics_file,
            ready_file,
            fake_agent,
            timeout_seconds,
        })
    }
}

struct DumpConfig {
    socket: PathBuf,
}

impl DumpConfig {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        match (
            arguments.next().as_deref(),
            arguments.next(),
            arguments.next(),
        ) {
            (Some("--socket"), Some(socket), None) => Ok(Self {
                socket: PathBuf::from(socket),
            }),
            _ => Err(usage().to_owned()),
        }
    }
}

struct RestoreConfig {
    state_file: PathBuf,
}

impl RestoreConfig {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        match (
            arguments.next().as_deref(),
            arguments.next(),
            arguments.next(),
        ) {
            (Some("--state"), Some(state_file), None) => Ok(Self {
                state_file: PathBuf::from(state_file),
            }),
            _ => Err(usage().to_owned()),
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

fn sibling_binary(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let executable = env::current_exe()?;
    let directory = executable.parent().ok_or("executable has no parent")?;
    Ok(directory.join(name))
}

fn usage() -> &'static str {
    "usage: remux-supervisor run [--sessions N] [--events-per-session N] [--rate N] [--socket PATH] [--state PATH] [--metrics PATH] [--ready PATH] [--fake-agent PATH] [--timeout-seconds N]\n       remux-supervisor dump --socket PATH\n       remux-supervisor restore --state PATH"
}
