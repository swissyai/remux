#![forbid(unsafe_code)]
//! One-process coordinator for all VT1 harness tiers.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use libghostty_vt::TerminalOptions;
use vt_harness::abi::{LinkedAbi, Operation};
use vt_harness::cases;
use vt_harness::receipt::{Durations, Receipt};
use vt_harness::{
    corpus, differential, fuzz, golden, invariants, mutation, receipt, HarnessError, Result,
};

const AUTHORED_FLOOR: usize = 300;
const CORPORA_FLOOR: usize = 20;
const STREAM_BYTES_FLOOR: usize = 5_000_000;
const ADVERSARIAL_FLOOR: usize = 100;
const INVARIANT_FLOOR: usize = 12;
const MUTATION_FLOOR: usize = 14;
const FUZZ_FLOOR: u64 = 100_000;

fn main() {
    if let Err(error) = real_main() {
        eprintln!("vt-harness: FAIL: {error}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let configuration = Configuration::parse()?;
    let root = env::current_dir()?.canonicalize()?;
    require_file(&root.join("Cargo.toml"))?;
    let abi = LinkedAbi;
    let authored_cases = cases::authored();
    let adversarial_cases = cases::adversarial();

    let authored_start = Instant::now();
    let authored = golden::run(
        &abi,
        &authored_cases,
        &root.join("vt-harness/golden/authored"),
        configuration.bless,
    )?;
    let authored_duration = authored_start.elapsed();

    let streams_start = Instant::now();
    let streams = corpus::replay(
        &abi,
        &abi,
        &root.join("vt-harness/corpus"),
        &root.join("vt-harness/golden/streams"),
        configuration.bless,
    )?;
    let streams_duration = streams_start.elapsed();

    let adversarial_start = Instant::now();
    let adversarial = golden::run(
        &abi,
        &adversarial_cases,
        &root.join("vt-harness/golden/adversarial"),
        configuration.bless,
    )?;
    let adversarial_duration = adversarial_start.elapsed();

    if configuration.bless {
        println!(
            "vt-harness: blessed authored={} snapshots={} corpora={} stream-steps={} adversarial={} snapshots={}",
            authored.cases,
            authored.snapshots,
            streams.corpora,
            streams.steps,
            adversarial.cases,
            adversarial.snapshots,
        );
        return Ok(());
    }

    enforce_corpus_floors(&authored, &streams, &adversarial)?;
    let total_start = Instant::now();

    let invariants_start = Instant::now();
    let properties = invariants::run(&abi)?;
    let invariants_duration = invariants_start.elapsed();
    if properties.len() < INVARIANT_FLOOR {
        return Err(HarnessError::new(format!(
            "only {} invariant properties, floor is {INVARIANT_FLOOR}",
            properties.len()
        )));
    }

    let differential_start = Instant::now();
    let differential_steps = authored_self_check(&abi, &authored_cases)?;
    let differential_duration = differential_start.elapsed();

    let mutations_start = Instant::now();
    let ghostty_source = env::var_os("GHOSTTY_SOURCE_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| HarnessError::new("GHOSTTY_SOURCE_DIR must name the pinned C2 source"))?;
    let kills = mutation::run(&root, &ghostty_source)?;
    let mutations_duration = mutations_start.elapsed();
    if kills.len() < MUTATION_FLOOR {
        return Err(HarnessError::new(format!(
            "only {} planted mutations, floor is {MUTATION_FLOOR}",
            kills.len()
        )));
    }

    let fuzz_start = Instant::now();
    let fuzz = fuzz::run(
        &abi,
        &abi,
        configuration.fuzz_executions,
        fuzz::DEFAULT_SEED,
    )?;
    let fuzz_duration = fuzz_start.elapsed();
    if fuzz.executions < FUZZ_FLOOR || fuzz.divergences != 0 {
        return Err(HarnessError::new(format!(
            "fuzz result executions={} divergences={} violates floor",
            fuzz.executions, fuzz.divergences
        )));
    }

    let git_sha = command_stdout(
        Command::new("git").args(["rev-parse", "HEAD"]),
        "read Git SHA",
    )?;
    let timestamp = command_stdout(
        Command::new("git").args(["show", "-s", "--format=%cI", "HEAD"]),
        "read Git commit timestamp",
    )?;
    let durations = Durations {
        authored: seconds(authored_duration),
        streams: seconds(streams_duration),
        adversarial: seconds(adversarial_duration),
        invariants: seconds(invariants_duration),
        differential: seconds(differential_duration),
        mutations: seconds(mutations_duration),
        fuzz: seconds(fuzz_duration),
        total: seconds(total_start.elapsed())
            + seconds(authored_duration)
            + seconds(streams_duration)
            + seconds(adversarial_duration),
    };
    receipt::write(
        &root.join("bench/results/vt-harness/receipt.json"),
        Receipt {
            schema_version: 1,
            git_sha: git_sha.trim(),
            timestamp: timestamp.trim(),
            authored_cases: authored.cases,
            stream_corpora: streams.corpora,
            stream_bytes: streams.bytes,
            adversarial_cases: adversarial.cases,
            invariant_properties: properties.len(),
            planted_mutations: kills.len(),
            mutations_killed: kills.len(),
            mutation_kills: &kills,
            fuzz_executions: fuzz.executions,
            fuzz_divergences: fuzz.divergences,
            durations,
            pass: true,
        },
    )?;
    println!(
        "vt-harness: PASS authored={} corpora={}/{}B adversarial={} properties={} differential-steps={} mutations={}/{} fuzz={}/{}",
        authored.cases,
        streams.corpora,
        streams.bytes,
        adversarial.cases,
        properties.len(),
        differential_steps,
        kills.len(),
        kills.len(),
        fuzz.executions,
        fuzz.divergences,
    );
    Ok(())
}

fn authored_self_check(abi: &LinkedAbi, cases: &[cases::Case]) -> Result<usize> {
    let mut steps = 0_usize;
    for case in cases {
        steps += differential::run(
            abi,
            abi,
            TerminalOptions {
                cols: case.cols,
                rows: case.rows,
                max_scrollback: case.max_scrollback,
            },
            &case.operations,
            &format!("authored-self-check:{}", case.id),
        )?;
    }
    differential::run(
        abi,
        abi,
        TerminalOptions {
            cols: 16,
            rows: 8,
            max_scrollback: 32,
        },
        &[
            Operation::write(b"self-check".to_vec()),
            Operation::Resize {
                cols: 9,
                rows: 5,
                cell_width_px: 8,
                cell_height_px: 16,
            },
        ],
        "abi-generic-resize-self-check",
    )?;
    Ok(steps + 2)
}

fn enforce_corpus_floors(
    authored: &golden::GoldenSummary,
    streams: &corpus::CorpusSummary,
    adversarial: &golden::GoldenSummary,
) -> Result<()> {
    if authored.cases < AUTHORED_FLOOR
        || streams.corpora < CORPORA_FLOOR
        || streams.bytes < STREAM_BYTES_FLOOR
        || adversarial.cases < ADVERSARIAL_FLOOR
    {
        return Err(HarnessError::new(format!(
            "tier floors failed: authored={}/{} corpora={}/{} streamBytes={}/{} adversarial={}/{}",
            authored.cases,
            AUTHORED_FLOOR,
            streams.corpora,
            CORPORA_FLOOR,
            streams.bytes,
            STREAM_BYTES_FLOOR,
            adversarial.cases,
            ADVERSARIAL_FLOOR,
        )));
    }
    Ok(())
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn require_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(HarnessError::new(format!(
            "required file missing: {}",
            path.display()
        )))
    }
}

fn command_stdout(command: &mut Command, context: &str) -> Result<String> {
    let output = command
        .output()
        .map_err(|error| HarnessError::new(format!("{context}: failed to execute: {error}")))?;
    if !output.status.success() {
        return Err(HarnessError::new(format!(
            "{context}: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| HarnessError::new(format!("{context}: stdout is not UTF-8")))
}

struct Configuration {
    bless: bool,
    fuzz_executions: u64,
}

impl Configuration {
    fn parse() -> Result<Self> {
        let mut bless = false;
        let mut fuzz_executions = FUZZ_FLOOR;
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--bless" => bless = true,
                "--fuzz-executions" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| HarnessError::new("--fuzz-executions needs a value"))?;
                    fuzz_executions = value
                        .parse::<u64>()
                        .map_err(|_| HarnessError::new("--fuzz-executions must be an integer"))?;
                    if !bless && fuzz_executions < FUZZ_FLOOR {
                        return Err(HarnessError::new(format!(
                            "fuzz smoke floor is {FUZZ_FLOOR}"
                        )));
                    }
                }
                other => {
                    return Err(HarnessError::new(format!(
                        "unknown argument {other}; use --bless or --fuzz-executions N"
                    )));
                }
            }
        }
        Ok(Self {
            bless,
            fuzz_executions,
        })
    }
}
