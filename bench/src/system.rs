// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Read-only process-tree sampler.
//!
//! Contract: `/bin/ps` is measurement instrumentation, not part of either subject.
//! Callers identify subject PIDs explicitly; peak RSS and cumulative CPU retain the
//! maximum observed values without adding sampler resource use.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub rss_kib: u64,
    pub cpu_seconds: f64,
}

pub fn snapshot() -> io::Result<Vec<ProcessEntry>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,rss=,time="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("ps process snapshot failed"));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ps output is not UTF-8"))?;
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 {
            continue;
        }
        let entry = ProcessEntry {
            pid: parse_field(fields[0], "pid")?,
            parent_pid: parse_field(fields[1], "parent pid")?,
            rss_kib: parse_field(fields[2], "RSS")?,
            cpu_seconds: parse_cpu_time(fields[3])?,
        };
        entries.push(entry);
    }
    Ok(entries)
}

pub fn descendants(entries: &[ProcessEntry], root_pid: u32) -> BTreeSet<u32> {
    let mut selected = BTreeSet::from([root_pid]);
    loop {
        let before = selected.len();
        for entry in entries {
            if selected.contains(&entry.parent_pid) {
                selected.insert(entry.pid);
            }
        }
        if selected.len() == before {
            return selected;
        }
    }
}

/// Returns cumulative CPU seconds for selected processes present in one snapshot.
pub fn selected_cpu_seconds(entries: &[ProcessEntry], selected: &BTreeSet<u32>) -> f64 {
    entries
        .iter()
        .filter(|entry| selected.contains(&entry.pid))
        .map(|entry| entry.cpu_seconds)
        .sum()
}

#[derive(Default)]
pub struct ResourceTracker {
    peak_rss_kib: u64,
    cpu_by_pid: BTreeMap<u32, f64>,
    observed_pids: BTreeSet<u32>,
}

impl ResourceTracker {
    pub fn observe(&mut self, entries: &[ProcessEntry], selected: &BTreeSet<u32>) {
        let observed = entries
            .iter()
            .filter(|entry| selected.contains(&entry.pid))
            .collect::<Vec<_>>();
        let rss_kib = observed.iter().map(|entry| entry.rss_kib).sum();
        self.peak_rss_kib = self.peak_rss_kib.max(rss_kib);
        for entry in observed {
            self.observed_pids.insert(entry.pid);
            self.cpu_by_pid
                .entry(entry.pid)
                .and_modify(|seconds| *seconds = seconds.max(entry.cpu_seconds))
                .or_insert(entry.cpu_seconds);
        }
    }

    pub fn peak_rss_bytes(&self) -> u64 {
        self.peak_rss_kib.saturating_mul(1_024)
    }

    pub fn cpu_seconds(&self) -> f64 {
        self.cpu_by_pid.values().sum()
    }

    pub fn observed_pids(&self) -> &BTreeSet<u32> {
        &self.observed_pids
    }

    pub fn distinct_pid_count(&self) -> u64 {
        u64::try_from(self.observed_pids.len()).unwrap_or(u64::MAX)
    }
}

fn parse_cpu_time(value: &str) -> io::Result<f64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (parse_field::<u64>(days, "CPU days")?, clock),
        None => (0, value),
    };
    let parts = clock.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0.0, parse_float(minutes)?, parse_float(seconds)?),
        [hours, minutes, seconds] => (
            parse_float(hours)?,
            parse_float(minutes)?,
            parse_float(seconds)?,
        ),
        _ => return Err(io::Error::other("unrecognized ps CPU time")),
    };
    Ok(days as f64 * 86_400.0 + hours * 3_600.0 + minutes * 60.0 + seconds)
}

fn parse_float(value: &str) -> io::Result<f64> {
    value
        .parse::<f64>()
        .map_err(|_| io::Error::other("invalid ps CPU time"))
}

fn parse_field<T>(value: &str, name: &str) -> io::Result<T>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| io::Error::other(format!("invalid ps {name}")))
}
