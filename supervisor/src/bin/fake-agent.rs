// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Scripted PTY child used by the synthetic fleet.

use std::env;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use supervisor::protocol::{unix_micros_now, Event, EventKind};

fn main() {
    if let Err(error) = run() {
        eprintln!("fake-agent: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse(env::args().skip(1))?;
    let mut socket = UnixStream::connect(&config.socket)?;
    let origin = Instant::now()
        .checked_add(Duration::from_micros(config.start_delay_us))
        .ok_or("start delay overflow")?;
    for sequence in 0..config.events {
        let scheduled = origin
            .checked_add(Duration::from_micros(
                config
                    .interval_us
                    .checked_mul(sequence)
                    .ok_or("event schedule overflow")?,
            ))
            .ok_or("event schedule overflow")?;
        sleep_until(scheduled);
        let (kind, payload) = scripted_event(sequence);
        let event = Event {
            session_id: config.session_id.clone(),
            sequence,
            sent_unix_micros: unix_micros_now()?,
            kind,
            payload,
        };
        socket.write_all(event.encode()?.as_bytes())?;
        writeln!(io::stdout(), "{} event {sequence}", config.session_id)?;
        io::stdout().flush()?;
    }
    Ok(())
}

fn scripted_event(sequence: u64) -> (EventKind, String) {
    match sequence % 6 {
        0 => (EventKind::Status, "busy".to_owned()),
        1 | 4 => (EventKind::Tool, format!("tool-{sequence:03}")),
        2 | 5 => (EventKind::Output, format!("output-{sequence:03}")),
        _ => (EventKind::Status, "idle".to_owned()),
    }
}

fn sleep_until(deadline: Instant) {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

struct Config {
    socket: PathBuf,
    session_id: String,
    events: u64,
    interval_us: u64,
    start_delay_us: u64,
}

impl Config {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut socket = None;
        let mut session_id = None;
        let mut events = None;
        let mut interval_us = None;
        let mut start_delay_us = None;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--socket" => socket = Some(PathBuf::from(value)),
                "--session-id" => session_id = Some(value),
                "--events" => events = Some(parse_positive(&value, "events")?),
                "--interval-us" => interval_us = Some(parse_positive(&value, "interval-us")?),
                "--start-delay-us" => {
                    start_delay_us =
                        Some(value.parse::<u64>().map_err(|_| "invalid start-delay-us")?);
                }
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(Self {
            socket: socket.ok_or("missing --socket")?,
            session_id: session_id.ok_or("missing --session-id")?,
            events: events.ok_or("missing --events")?,
            interval_us: interval_us.ok_or("missing --interval-us")?,
            start_delay_us: start_delay_us.ok_or("missing --start-delay-us")?,
        })
    }
}

fn parse_positive(value: &str, name: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {name}"))?;
    if parsed == 0 {
        Err(format!("{name} must be positive").into())
    } else {
        Ok(parsed)
    }
}
