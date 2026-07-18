// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Short-lived process used to model one hook process per event.

use std::env;
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("fork-worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse(env::args().skip(1))?;
    let started = Instant::now();
    let bytes = config
        .rss_mib
        .checked_mul(1024 * 1024)
        .ok_or("RSS allocation overflow")?;
    let mut resident = vec![0_u8; usize::try_from(bytes)?];
    for page in resident.chunks_mut(4_096) {
        page[0] = 1;
    }
    std::hint::black_box((&resident, &config.kind));
    let mut checksum = 0_u64;
    while started.elapsed() < Duration::from_millis(config.cpu_ms) {
        checksum =
            std::hint::black_box(checksum.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    }
    std::hint::black_box(checksum);
    let deadline = started
        .checked_add(Duration::from_millis(config.hold_ms))
        .ok_or("fork hold deadline overflow")?;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    Ok(())
}

struct Config {
    hold_ms: u64,
    cpu_ms: u64,
    rss_mib: u64,
    kind: String,
}

impl Config {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut hold_ms = None;
        let mut cpu_ms = None;
        let mut rss_mib = None;
        let mut kind = None;
        let mut arguments = arguments;
        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--hold-ms" => hold_ms = Some(parse_positive(&value, "hold-ms")?),
                "--cpu-ms" => cpu_ms = Some(parse_positive(&value, "cpu-ms")?),
                "--rss-mib" => rss_mib = Some(parse_positive(&value, "rss-mib")?),
                "--kind" if matches!(value.as_str(), "status" | "tool" | "output") => {
                    kind = Some(value);
                }
                "--kind" => return Err("invalid event kind".into()),
                _ => return Err(format!("unknown flag {flag}").into()),
            }
        }
        let config = Self {
            hold_ms: hold_ms.ok_or("missing --hold-ms")?,
            cpu_ms: cpu_ms.ok_or("missing --cpu-ms")?,
            rss_mib: rss_mib.ok_or("missing --rss-mib")?,
            kind: kind.ok_or("missing --kind")?,
        };
        if config.cpu_ms > config.hold_ms {
            return Err("cpu-ms cannot exceed hold-ms".into());
        }
        Ok(config)
    }
}

fn parse_positive(value: &str, name: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let value = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {name}"))?;
    if value == 0 {
        Err(format!("{name} must be positive").into())
    } else {
        Ok(value)
    }
}
