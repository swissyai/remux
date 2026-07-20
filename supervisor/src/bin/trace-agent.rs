// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Replays one previously captured working-session trace through stdout.

use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use supervisor::trace::RecordedTrace;

fn main() {
    if let Err(error) = run() {
        eprintln!("trace-agent: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse(env::args().skip(1))?;
    let trace = RecordedTrace::read(&config.trace)?;
    let origin = Instant::now()
        .checked_add(Duration::from_micros(config.start_delay_us))
        .ok_or("trace replay start delay overflow")?;
    let mut output = io::stdout().lock();
    for record in trace.records() {
        let scheduled = origin
            .checked_add(Duration::from_micros(record.at_micros()))
            .ok_or("trace replay schedule overflow")?;
        sleep_until(scheduled);
        output.write_all(record.bytes())?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    drop(output);
    thread::sleep(Duration::from_millis(config.hold_after_trace_ms));
    Ok(())
}

fn sleep_until(deadline: Instant) {
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

struct Config {
    trace: PathBuf,
    start_delay_us: u64,
    hold_after_trace_ms: u64,
}

impl Config {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut trace = None;
        let mut start_delay_us = None;
        let mut hold_after_trace_ms = 0;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--trace" => trace = Some(PathBuf::from(value)),
                "--start-delay-us" => {
                    start_delay_us = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| "invalid trace replay start delay")?,
                    );
                }
                "--hold-after-trace-ms" => {
                    hold_after_trace_ms = value
                        .parse::<u64>()
                        .map_err(|_| "invalid trace replay hold")?;
                }
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        Ok(Self {
            trace: trace.ok_or("missing --trace")?,
            start_delay_us: start_delay_us.ok_or("missing --start-delay-us")?,
            hold_after_trace_ms,
        })
    }
}
