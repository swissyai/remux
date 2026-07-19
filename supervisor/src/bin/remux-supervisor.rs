#![forbid(unsafe_code)]
//! PTY supervisor: authorized startup, one listener socket, batched state, and an
//! asynchronous scrollback persistence path.

use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use supervisor::attach::{
    consume_drive_authorization, consume_lifecycle_authorization, record_authorization,
    record_drive_authorization, spawn_authorized_pty, AttachScope,
};
use supervisor::attestation::{
    attestation_path, verify_attestation, AttestationObserver, AttestationSummary,
    AttestationWriter, ExitOutcome, LifecyclePhase, LogIntegrity,
};
use supervisor::capability::{
    observe_sessions, write_authorized_input, DriveCapability, DrivePresence,
};
use supervisor::protocol::{parse_message, unix_micros_now, Control, Event, EventKind, Message};
use supervisor::restore::inspect_passive;
use supervisor::scrollback::ScrollbackWriter;
use supervisor::state::{dump_atomic, restore_passive, LiveState};
use supervisor::tui::{TracerRenderer, TracerTabView};

const MAX_MESSAGE_BYTES: usize = 4_096;
const MAX_BATCH_MESSAGES: usize = 256;
const REAL_EVENT_PREFIX: &str = "remux-event:";
const REAL_AGENT_SCRIPT: &str = "while IFS= read -r line; do [ \"$line\" = \"__remux_done__\" ] && break; printf 'remux-event:%s\\n' \"$line\"; done";

fn main() {
    if let Err(error) = entrypoint() {
        eprintln!("remux-supervisor: {error}");
        std::process::exit(1);
    }
}

fn entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("authorize") => authorize(AuthorizeConfig::parse(arguments)?),
        Some("run") => run_supervisor(RunConfig::parse(arguments)?),
        Some("dump") => request_dump(DumpConfig::parse(arguments)?),
        Some("restore") => inspect_restore(RestoreConfig::parse(arguments)?),
        Some("tui") => render_restore(TuiConfig::parse(arguments)?),
        Some("verify-attestation") => {
            verify_attestation_command(VerifyAttestationConfig::parse(arguments)?)
        }
        _ => Err(usage().into()),
    }
}

fn authorize(config: AuthorizeConfig) -> Result<(), Box<dyn std::error::Error>> {
    match config.scope {
        AuthorizationScope::Drive => {
            record_drive_authorization(&config.auth_log, &config.token)?;
            println!("authorized drive capability");
        }
        AuthorizationScope::Lifecycle(scope) => {
            record_authorization(&config.auth_log, scope, &config.token)?;
            println!("authorized {scope} lifecycle capability");
        }
    }
    Ok(())
}

fn run_supervisor(config: RunConfig) -> Result<(), Box<dyn std::error::Error>> {
    let session_ids = (0..config.sessions)
        .map(|index| format!("session-{index:03}"))
        .collect::<Vec<_>>();
    let lifecycle = consume_lifecycle_authorization(
        &config.auth_log,
        config.attach_scope,
        config.attach_token.as_deref(),
        session_ids.iter().cloned(),
    )?;
    let drive = match config.drive_token.as_deref() {
        Some(token) => Some(Arc::new(consume_drive_authorization(
            &config.auth_log,
            Some(token),
            session_ids.iter().cloned(),
        )?)),
        None => None,
    };
    let drive_presence = drive
        .as_deref()
        .map_or_else(DrivePresence::none, DriveCapability::presence);
    prepare_output(&config.socket)?;
    prepare_output(&config.ready_file)?;
    prepare_output(&config.metrics_file)?;
    let listener = UnixListener::bind(&config.socket)?;
    let _socket_guard = SocketGuard(config.socket.clone());
    let stopping = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    let listener_thread = spawn_listener(listener, Arc::clone(&stopping), sender);

    let mut state = LiveState::new(session_ids.iter().cloned())?;
    let mut renderer = match &config.tui_target {
        Some(target) => Some(TracerRenderer::new(
            target.open()?,
            TracerTabView::live(session_ids.iter(), &drive_presence)?,
        )),
        None => None,
    };
    let (attestation_writer, attestation_observer) = match config.attestation_mode {
        AttestationMode::Off => (None, None),
        AttestationMode::HashChain => {
            let observe = observe_sessions(session_ids.iter().cloned())?;
            let writer = AttestationWriter::start(
                &config.attestation_dir,
                session_ids.iter().cloned(),
                &observe,
            )?;
            let observer = writer.observer()?;
            (Some(writer), Some(observer))
        }
    };
    let scrollback = ScrollbackWriter::start(&config.scrollback_dir, session_ids.iter().cloned())?;
    let interval_us = u64::from(config.sessions)
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_div(config.rate))
        .ok_or("invalid event interval")?;
    let stagger_us = 1_000_000_u64
        .checked_div(config.rate)
        .ok_or("invalid event rate")?;
    let mut children = Vec::with_capacity(config.sessions as usize);
    let mut pty_readers = Vec::with_capacity(config.sessions as usize);
    let mut pty_writers = Vec::with_capacity(config.sessions as usize);
    for (index, session_id) in session_ids.iter().enumerate() {
        let start_delay_us = config
            .initial_idle_ms
            .checked_mul(1_000)
            .and_then(|idle| {
                stagger_us
                    .checked_mul(u64::try_from(index).ok()?)
                    .and_then(|stagger| idle.checked_add(stagger))
            })
            .ok_or("start delay overflow")?;
        if let Some(observer) = &attestation_observer {
            observer.lifecycle(session_id, LifecyclePhase::Created)?;
        }
        match config.agent_kind {
            AgentKind::Scripted => {
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
                    &start_delay_us.to_string(),
                ]);
                let (child, master) = spawn_authorized_pty(&lifecycle, session_id, &mut command)?;
                record_attested_spawn(&attestation_observer, session_id, child.id())?;
                children.push(child);
                pty_readers.push(spawn_pty_drain(
                    master,
                    session_id.clone(),
                    attestation_observer.clone(),
                ));
            }
            AgentKind::RealShell => {
                let mut command = Command::new(&config.agent_shell);
                command.args(["-c", REAL_AGENT_SCRIPT]);
                let (child, master) = spawn_authorized_pty(&lifecycle, session_id, &mut command)?;
                record_attested_spawn(&attestation_observer, session_id, child.id())?;
                let input = master.try_clone()?;
                let event_socket = UnixStream::connect(&config.socket)?;
                let drive = Arc::clone(
                    drive
                        .as_ref()
                        .ok_or("real-shell input requires an explicit drive capability")?,
                );
                pty_writers.push(spawn_real_agent_input(
                    input,
                    session_id.clone(),
                    drive,
                    attestation_observer.clone(),
                    config.events_per_session,
                    interval_us,
                    start_delay_us,
                ));
                pty_readers.push(spawn_real_agent_events(
                    master,
                    event_socket,
                    session_id.clone(),
                    attestation_observer.clone(),
                ));
                children.push(child);
            }
        }
    }
    if let Some(renderer) = &mut renderer {
        renderer.redraw()?;
    }
    write_ready(&config.ready_file, &config.socket, &children)?;

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
    let mut redraw_latencies_us = Vec::with_capacity(usize::try_from(expected_events)?);
    while events_ingested < expected_events {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                format!("timed out after ingesting {events_ingested}/{expected_events} events")
            })?;
        let first = match receiver.recv_timeout(remaining) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                fail_if_child_exited(&mut children)?;
                return Err(format!(
                    "timed out after ingesting {events_ingested}/{expected_events} events"
                )
                .into());
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
            let pending = state.apply_batch(&events)?;
            scrollback.enqueue(pending)?;
            if let Some(renderer) = &mut renderer {
                renderer.view_mut().apply_batch(&events, &drive_presence)?;
                renderer.redraw()?;
                let redrawn = unix_micros_now()?;
                redraw_latencies_us.extend(
                    events
                        .iter()
                        .map(|event| redrawn.saturating_sub(event.sent_unix_micros)),
                );
            }
        }
        if dump_requested {
            let offsets = scrollback.flush()?;
            state.mark_scrollback_persisted(&offsets)?;
            dump_atomic(&config.state_file, &state)?;
            on_demand_dumps = on_demand_dumps
                .checked_add(1)
                .ok_or("dump count overflow")?;
        }
        batches = batches.checked_add(1).ok_or("batch count overflow")?;
    }

    for writer in pty_writers {
        writer
            .join()
            .map_err(|_| "real agent input writer panicked")??;
    }
    for (session_id, child) in session_ids.iter().zip(&mut children) {
        let status = child.wait()?;
        if let Some(observer) = &attestation_observer {
            observer.exit(session_id, exit_outcome(status)?)?;
        }
        if !status.success() {
            return Err(format!("agent exited with {status}").into());
        }
    }
    let mut pty_bytes = 0_u64;
    for reader in pty_readers {
        pty_bytes = pty_bytes
            .checked_add(reader.join().map_err(|_| "PTY reader panicked")??)
            .ok_or("PTY byte count overflow")?;
    }
    if let Some(observer) = &attestation_observer {
        for session_id in &session_ids {
            observer.lifecycle(session_id, LifecyclePhase::Ended)?;
        }
    }
    drop(attestation_observer);
    let attestation_summary = match attestation_writer {
        Some(writer) => {
            let summary = writer.finish()?;
            verify_attestation_summary(&config.attestation_dir, &summary)?;
            Some(summary)
        }
        None => None,
    };
    let offsets = scrollback.finish()?;
    state.mark_scrollback_persisted(&offsets)?;
    dump_atomic(&config.state_file, &state)?;
    write_metrics(
        &config.metrics_file,
        &RunMetrics {
            agent_kind: config.agent_kind,
            events_ingested,
            batches,
            children_spawned: u64::try_from(children.len())?,
            pty_bytes,
            on_demand_dumps,
            latencies_us,
            frames_rendered: renderer.as_ref().map_or(0, TracerRenderer::frames_rendered),
            redraw_latencies_us,
            attestation_summary,
        },
    )?;
    stopping.store(true, Ordering::Release);
    let wake = UnixStream::connect(&config.socket)?;
    drop(wake);
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
    println!("{}", inspect_passive(&config.state_file)?);
    Ok(())
}

fn render_restore(config: TuiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let state = restore_passive(&config.state_file)?;
    let view = TracerTabView::from_passive(&state)?;
    let mut renderer = TracerRenderer::new(config.target.open()?, view);
    renderer.redraw()?;
    Ok(())
}

fn verify_attestation_command(
    config: VerifyAttestationConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let verification = verify_attestation(&config.file)?;
    let integrity = match verification.integrity {
        LogIntegrity::Complete => "complete",
        LogIntegrity::TornTail => "torn-tail",
    };
    println!(
        "integrity\t{integrity}\nrecords\t{}\ninput_bytes\t{}\noutput_bytes\t{}\nhead\t{}",
        verification.records,
        verification.input_bytes,
        verification.output_bytes,
        verification.head_hex()
    );
    if verification.integrity == LogIntegrity::Complete {
        Ok(())
    } else {
        Err("attestation has a torn final frame".into())
    }
}

fn record_attested_spawn(
    observer: &Option<AttestationObserver>,
    session_id: &str,
    pid: u32,
) -> io::Result<()> {
    if let Some(observer) = observer {
        observer.spawn(session_id, pid)?;
        observer.lifecycle(session_id, LifecyclePhase::Running)?;
    }
    Ok(())
}

fn exit_outcome(status: std::process::ExitStatus) -> io::Result<ExitOutcome> {
    status
        .code()
        .map(ExitOutcome::Code)
        .or_else(|| status.signal().map(ExitOutcome::Signal))
        .ok_or_else(|| io::Error::other("child exit has neither code nor signal"))
}

fn verify_attestation_summary(directory: &Path, summary: &AttestationSummary) -> io::Result<()> {
    for (session_id, expected) in summary {
        let verified = verify_attestation(&attestation_path(directory, session_id))?;
        if verified.integrity != LogIntegrity::Complete
            || verified.records != expected.records
            || verified.input_bytes != expected.input_bytes
            || verified.output_bytes != expected.output_bytes
            || verified.head != expected.head
        {
            return Err(io::Error::other(
                "finished attestation differs from external verification",
            ));
        }
    }
    Ok(())
}

fn spawn_listener(
    listener: UnixListener,
    stopping: Arc<AtomicBool>,
    sender: mpsc::Sender<io::Result<Message>>,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut readers = Vec::new();
        loop {
            let (stream, _) = listener.accept()?;
            if stopping.load(Ordering::Acquire) {
                break;
            }
            let sender = sender.clone();
            readers.push(thread::spawn(move || read_stream(stream, sender)));
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

fn spawn_pty_drain(
    mut master: File,
    session_id: String,
    attestation: Option<AttestationObserver>,
) -> JoinHandle<io::Result<u64>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4_096];
        let mut total = 0_u64;
        loop {
            match master.read(&mut buffer) {
                Ok(0) => return Ok(total),
                Ok(bytes) => {
                    if let Some(observer) = &attestation {
                        observer.output(&session_id, &buffer[..bytes])?;
                    }
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

fn write_attested_input(
    drive: &DriveCapability,
    attestation: &Option<AttestationObserver>,
    session_id: &str,
    master: &mut File,
    bytes: &[u8],
) -> io::Result<()> {
    write_authorized_input(drive, session_id, master, bytes)?;
    if let Some(observer) = attestation {
        observer.input(session_id, bytes)?;
    }
    Ok(())
}

fn spawn_real_agent_input(
    mut master: File,
    session_id: String,
    drive: Arc<DriveCapability>,
    attestation: Option<AttestationObserver>,
    events: u64,
    interval_us: u64,
    start_delay_us: u64,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let origin = Instant::now()
            .checked_add(Duration::from_micros(start_delay_us))
            .ok_or_else(|| io::Error::other("real agent start delay overflow"))?;
        for sequence in 0..events {
            let scheduled = origin
                .checked_add(Duration::from_micros(
                    interval_us
                        .checked_mul(sequence)
                        .ok_or_else(|| io::Error::other("real agent schedule overflow"))?,
                ))
                .ok_or_else(|| io::Error::other("real agent schedule overflow"))?;
            sleep_until(scheduled);
            let input = format!("shell-output-{sequence:03}\n");
            write_attested_input(
                &drive,
                &attestation,
                &session_id,
                &mut master,
                input.as_bytes(),
            )?;
        }
        write_attested_input(
            &drive,
            &attestation,
            &session_id,
            &mut master,
            b"__remux_done__\n",
        )
    })
}

fn spawn_real_agent_events(
    master: File,
    mut socket: UnixStream,
    session_id: String,
    attestation: Option<AttestationObserver>,
) -> JoinHandle<io::Result<u64>> {
    thread::spawn(move || {
        let mut reader = BufReader::new(master);
        let mut total = 0_u64;
        let mut sequence = 0_u64;
        loop {
            let mut line = String::new();
            let bytes = match reader.read_line(&mut line) {
                Ok(bytes) => bytes,
                Err(error) if error.raw_os_error() == Some(5) => return Ok(total),
                Err(error) => return Err(error),
            };
            if bytes == 0 {
                return Ok(total);
            }
            let observed_at = unix_micros_now().map_err(io::Error::other)?;
            if let Some(observer) = &attestation {
                observer.output(&session_id, line.as_bytes())?;
            }
            total = total
                .checked_add(u64::try_from(bytes).map_err(io::Error::other)?)
                .ok_or_else(|| io::Error::other("PTY byte count overflow"))?;
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(payload) = line.strip_prefix(REAL_EVENT_PREFIX) else {
                continue;
            };
            let event = Event {
                session_id: session_id.clone(),
                sequence,
                sent_unix_micros: observed_at,
                kind: EventKind::Output,
                payload: payload.to_owned(),
            };
            socket.write_all(event.encode().map_err(io::Error::other)?.as_bytes())?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| io::Error::other("real agent event sequence overflow"))?;
        }
    })
}

fn sleep_until(deadline: Instant) {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

fn fail_if_child_exited(children: &mut [Child]) -> Result<(), Box<dyn std::error::Error>> {
    for child in children {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!("agent exited early with {status}").into());
            }
        }
    }
    Ok(())
}

fn write_ready(path: &Path, socket: &Path, children: &[Child]) -> io::Result<()> {
    let child_pids = children
        .iter()
        .map(|child| child.id().to_string())
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        path,
        format!(
            "pid\t{}\nsocket\t{}\nchildren\t{}\nchild_pids\t{}\n",
            std::process::id(),
            socket.display(),
            children.len(),
            child_pids
        ),
    )
}

struct RunMetrics {
    agent_kind: AgentKind,
    events_ingested: u64,
    batches: u64,
    children_spawned: u64,
    pty_bytes: u64,
    on_demand_dumps: u64,
    latencies_us: Vec<u64>,
    frames_rendered: u64,
    redraw_latencies_us: Vec<u64>,
    attestation_summary: Option<AttestationSummary>,
}

fn write_metrics(path: &Path, metrics: &RunMetrics) -> io::Result<()> {
    let latencies = join_numbers(&metrics.latencies_us);
    let redraw_latencies = join_numbers(&metrics.redraw_latencies_us);
    let (attestation_enabled, attestation_records, attestation_file_bytes, heads) =
        attestation_metrics(metrics.attestation_summary.as_ref())?;
    fs::write(
        path,
        format!(
            "schema\t3\nagent_kind\t{}\nevents_ingested\t{}\nbatches\t{}\nchildren_spawned\t{}\npty_bytes\t{}\non_demand_dumps\t{}\nlatencies_us\t{}\nframes_rendered\t{}\nredraw_latencies_us\t{}\nattestation_enabled\t{}\nattestation_records\t{}\nattestation_file_bytes\t{}\nattestation_heads\t{}\n",
            metrics.agent_kind.as_str(),
            metrics.events_ingested,
            metrics.batches,
            metrics.children_spawned,
            metrics.pty_bytes,
            metrics.on_demand_dumps,
            latencies,
            metrics.frames_rendered,
            redraw_latencies,
            attestation_enabled,
            attestation_records,
            attestation_file_bytes,
            heads
        ),
    )
}

fn attestation_metrics(summary: Option<&AttestationSummary>) -> io::Result<(u8, u64, u64, String)> {
    let Some(summary) = summary else {
        return Ok((0, 0, 0, String::new()));
    };
    let mut records = 0_u64;
    let mut file_bytes = 0_u64;
    let mut heads = Vec::with_capacity(summary.len());
    for (session_id, session) in summary {
        records = records
            .checked_add(session.records)
            .ok_or_else(|| io::Error::other("attestation record metric overflow"))?;
        file_bytes = file_bytes
            .checked_add(session.file_bytes)
            .ok_or_else(|| io::Error::other("attestation byte metric overflow"))?;
        heads.push(format!("{session_id}:{}", session.head_hex()));
    }
    Ok((1, records, file_bytes, heads.join(",")))
}

fn join_numbers(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttestationMode {
    Off,
    HashChain,
}

impl AttestationMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "off" => Ok(Self::Off),
            "hash-chain" => Ok(Self::HashChain),
            _ => Err("attestation must be off or hash-chain".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentKind {
    Scripted,
    RealShell,
}

impl AgentKind {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "scripted" => Ok(Self::Scripted),
            "real-shell" => Ok(Self::RealShell),
            _ => Err("agent-kind must be scripted or real-shell".to_owned()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Scripted => "scripted",
            Self::RealShell => "real-shell",
        }
    }
}

enum TuiTarget {
    StandardOutput,
    File(PathBuf),
}

impl TuiTarget {
    fn open(&self) -> io::Result<Box<dyn Write>> {
        match self {
            Self::StandardOutput => Ok(Box::new(io::stdout())),
            Self::File(path) => Ok(Box::new(File::create(path)?)),
        }
    }
}

struct RunConfig {
    sessions: u32,
    events_per_session: u64,
    rate: u64,
    socket: PathBuf,
    state_file: PathBuf,
    scrollback_dir: PathBuf,
    attestation_dir: PathBuf,
    attestation_mode: AttestationMode,
    metrics_file: PathBuf,
    ready_file: PathBuf,
    fake_agent: PathBuf,
    agent_kind: AgentKind,
    agent_shell: PathBuf,
    timeout_seconds: u64,
    auth_log: PathBuf,
    attach_token: Option<String>,
    attach_scope: AttachScope,
    drive_token: Option<String>,
    initial_idle_ms: u64,
    tui_target: Option<TuiTarget>,
}

impl RunConfig {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut sessions = 20;
        let mut events_per_session = 6;
        let mut rate = 20;
        let mut socket = PathBuf::from("remux.sock");
        let mut state_file = PathBuf::from("remux-state.json");
        let mut scrollback_dir = PathBuf::from("remux-scrollback");
        let mut attestation_dir = PathBuf::from("remux-attestations");
        let mut attestation_mode = AttestationMode::HashChain;
        let mut metrics_file = PathBuf::from("remux-metrics.tsv");
        let mut ready_file = PathBuf::from("remux-ready.tsv");
        let mut fake_agent = sibling_binary("fake-agent")?;
        let mut agent_kind = AgentKind::Scripted;
        let mut agent_shell = PathBuf::from("/bin/sh");
        let mut timeout_seconds = 60;
        let mut auth_log = PathBuf::from("remux-attach.log");
        let mut attach_token = None;
        let mut attach_scope = AttachScope::Launch;
        let mut drive_token = None;
        let mut initial_idle_ms = 0;
        let mut tui_target = None;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            if flag == "--tui" {
                tui_target = Some(TuiTarget::StandardOutput);
                continue;
            }
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--sessions" => sessions = parse_positive(&value, "sessions")?,
                "--events-per-session" => {
                    events_per_session = parse_positive(&value, "events-per-session")?;
                }
                "--rate" => rate = parse_positive(&value, "rate")?,
                "--socket" => socket = PathBuf::from(value),
                "--state" => state_file = PathBuf::from(value),
                "--scrollback-dir" => scrollback_dir = PathBuf::from(value),
                "--attestation-dir" => attestation_dir = PathBuf::from(value),
                "--attestation" => attestation_mode = AttestationMode::parse(&value)?,
                "--metrics" => metrics_file = PathBuf::from(value),
                "--ready" => ready_file = PathBuf::from(value),
                "--fake-agent" => fake_agent = PathBuf::from(value),
                "--agent-kind" => agent_kind = AgentKind::parse(&value)?,
                "--agent-shell" => agent_shell = PathBuf::from(value),
                "--timeout-seconds" => timeout_seconds = parse_positive(&value, "timeout-seconds")?,
                "--auth-log" => auth_log = PathBuf::from(value),
                "--attach-token" => attach_token = Some(value),
                "--attach-scope" => attach_scope = AttachScope::parse(&value)?,
                "--drive-token" => drive_token = Some(value),
                "--initial-idle-ms" => {
                    initial_idle_ms = value.parse().map_err(|_| "invalid initial-idle-ms")?;
                }
                "--tui-output" => tui_target = Some(TuiTarget::File(PathBuf::from(value))),
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(Self {
            sessions,
            events_per_session,
            rate,
            socket,
            state_file,
            scrollback_dir,
            attestation_dir,
            attestation_mode,
            metrics_file,
            ready_file,
            fake_agent,
            agent_kind,
            agent_shell,
            timeout_seconds,
            auth_log,
            attach_token,
            attach_scope,
            drive_token,
            initial_idle_ms,
            tui_target,
        })
    }
}

#[derive(Clone, Copy)]
enum AuthorizationScope {
    Drive,
    Lifecycle(AttachScope),
}

impl AuthorizationScope {
    fn parse(value: &str) -> io::Result<Self> {
        if value == "drive" {
            Ok(Self::Drive)
        } else {
            AttachScope::parse(value).map(Self::Lifecycle)
        }
    }
}

struct AuthorizeConfig {
    auth_log: PathBuf,
    token: String,
    scope: AuthorizationScope,
}

impl AuthorizeConfig {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut auth_log = None;
        let mut token = None;
        let mut scope = None;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--auth-log" => auth_log = Some(PathBuf::from(value)),
                "--token" => token = Some(value),
                "--scope" => scope = Some(AuthorizationScope::parse(&value)?),
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(Self {
            auth_log: auth_log.ok_or("missing --auth-log")?,
            token: token.ok_or("missing --token")?,
            scope: scope.ok_or("missing --scope")?,
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

struct TuiConfig {
    state_file: PathBuf,
    target: TuiTarget,
}

impl TuiConfig {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut state_file = None;
        let mut target = TuiTarget::StandardOutput;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments.next().ok_or_else(|| usage().to_owned())?;
            match flag.as_str() {
                "--state" => state_file = Some(PathBuf::from(value)),
                "--output" => target = TuiTarget::File(PathBuf::from(value)),
                _ => return Err(usage().to_owned()),
            }
        }
        Ok(Self {
            state_file: state_file.ok_or_else(|| usage().to_owned())?,
            target,
        })
    }
}

struct VerifyAttestationConfig {
    file: PathBuf,
}

impl VerifyAttestationConfig {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        match (
            arguments.next().as_deref(),
            arguments.next(),
            arguments.next(),
        ) {
            (Some("--file"), Some(file), None) => Ok(Self {
                file: PathBuf::from(file),
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
    "usage: remux-supervisor authorize --auth-log PATH --token TOKEN --scope drive|launch|relaunch\n       remux-supervisor run [--sessions N] [--events-per-session N] [--rate N] [--agent-kind scripted|real-shell] [--agent-shell PATH] [--socket PATH] [--state PATH] [--scrollback-dir PATH] [--attestation-dir PATH] [--attestation off|hash-chain] [--metrics PATH] [--ready PATH] [--fake-agent PATH] [--timeout-seconds N] [--auth-log PATH] [--attach-token TOKEN] [--attach-scope launch|relaunch] [--drive-token TOKEN] [--initial-idle-ms N] [--tui | --tui-output PATH]\n       remux-supervisor dump --socket PATH\n       remux-supervisor restore --state PATH\n       remux-supervisor tui --state PATH [--output PATH]\n       remux-supervisor verify-attestation --file PATH"
}
