//! Scratch-built planted mutation gate over the public C ABI.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::{HarnessError, Result};

const GHOSTTY_COMMIT: &str = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
const ZIG_VERSION: &str = "0.15.2";
const MUTATIONS: &[(&str, &str, &str)] = &[
    (
        "M01-cols-off-by-one",
        "01-cols-off-by-one.patch",
        "mutation-probe:geometry-cols",
    ),
    (
        "M02-rows-off-by-one",
        "02-rows-off-by-one.patch",
        "mutation-probe:geometry-rows",
    ),
    (
        "M03-cursor-x-shift",
        "03-cursor-x-shift.patch",
        "mutation-probe:cursor-x",
    ),
    (
        "M04-cursor-y-shift",
        "04-cursor-y-shift.patch",
        "mutation-probe:cursor-y",
    ),
    (
        "M05-pending-wrap-inverted",
        "05-pending-wrap-inverted.patch",
        "mutation-probe:pending-wrap",
    ),
    (
        "M06-active-screen-swapped",
        "06-active-screen-swapped.patch",
        "mutation-probe:active-screen",
    ),
    (
        "M07-cursor-visible-inverted",
        "07-cursor-visible-inverted.patch",
        "mutation-probe:cursor-visible",
    ),
    (
        "M08-mouse-tracking-inverted",
        "08-mouse-tracking-inverted.patch",
        "mutation-probe:mouse-tracking",
    ),
    (
        "M09-total-rows-off-by-one",
        "09-total-rows-off-by-one.patch",
        "mutation-probe:total-rows",
    ),
    (
        "M10-scrollback-off-by-one",
        "10-scrollback-rows-off-by-one.patch",
        "mutation-probe:scrollback-rows",
    ),
    (
        "M11-width-pixels-off-by-one",
        "11-width-pixels-off-by-one.patch",
        "mutation-probe:width-pixels",
    ),
    (
        "M12-mode-query-inverted",
        "12-mode-query-inverted.patch",
        "mutation-probe:mode-query",
    ),
    (
        "M13-erase-complete-short",
        "13-erase-complete-short.patch",
        "mutation-probe:content-erase-complete",
    ),
    (
        "M14-erase-right-short",
        "14-erase-right-short.patch",
        "mutation-probe:content-erase-right",
    ),
];

/// One mutation rejected by a named harness tier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationKill {
    /// Stable planted mutation ID.
    pub id: String,
    /// Tier/probe that observed the divergence.
    pub detected_by: String,
}

/// Applies every committed patch to a temporary source copy, builds each
/// variant with pinned Zig, and rejects it against executed baseline output.
pub fn run(repository_root: &Path, ghostty_source: &Path) -> Result<Vec<MutationKill>> {
    validate_pins(repository_root, ghostty_source)?;
    let temp = Scratch::new()?;
    let scratch_source = temp.path.join("ghostty");
    run_command(
        Command::new("cp")
            .arg("-R")
            .arg(ghostty_source)
            .arg(&scratch_source),
        "copy pinned Ghostty to mutation scratch",
    )?;

    let baseline_prefix = temp.path.join("baseline");
    let baseline = build_probe(
        repository_root,
        &scratch_source,
        &baseline_prefix,
        &temp.path,
    )?;
    if baseline.is_empty() {
        return Err(HarnessError::new(
            "unmodified mutation probe emitted no output",
        ));
    }

    let mut kills = Vec::with_capacity(MUTATIONS.len());
    for (id, patch_file, detected_by) in MUTATIONS {
        let patch = repository_root
            .join("vt-harness/mutations")
            .join(patch_file);
        apply_patch(&scratch_source, &patch, false)?;
        let prefix = temp.path.join(id);
        let mutant = build_probe(repository_root, &scratch_source, &prefix, &temp.path)?;
        apply_patch(&scratch_source, &patch, true)?;
        if mutant == baseline {
            return Err(HarnessError::new(format!(
                "mutation survived id={id} patch={} probe={baseline:?}",
                patch.display()
            )));
        }
        kills.push(MutationKill {
            id: (*id).to_owned(),
            detected_by: (*detected_by).to_owned(),
        });
    }
    Ok(kills)
}

fn validate_pins(repository_root: &Path, ghostty_source: &Path) -> Result<()> {
    if !ghostty_source.join("build.zig").is_file() {
        return Err(HarnessError::new(format!(
            "GHOSTTY_SOURCE_DIR is not a source tree: {}",
            ghostty_source.display()
        )));
    }
    let commit = command_stdout(
        Command::new("git")
            .arg("-C")
            .arg(ghostty_source)
            .args(["rev-parse", "HEAD"]),
        "read Ghostty commit",
    )?;
    if commit.trim() != GHOSTTY_COMMIT {
        return Err(HarnessError::new(format!(
            "Ghostty commit {} does not match pin {GHOSTTY_COMMIT}",
            commit.trim()
        )));
    }
    let zig = command_stdout(Command::new("zig").arg("version"), "read Zig version")?;
    if zig.trim() != ZIG_VERSION {
        return Err(HarnessError::new(format!(
            "Zig version {} does not match pin {ZIG_VERSION}",
            zig.trim()
        )));
    }
    let mutation_dir = repository_root.join("vt-harness/mutations");
    let patch_count = fs::read_dir(&mutation_dir)?
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "patch")
        })
        .count();
    if patch_count != MUTATIONS.len() {
        return Err(HarnessError::new(format!(
            "mutation patch inventory has {patch_count} files, expected {}",
            MUTATIONS.len()
        )));
    }
    Ok(())
}

fn apply_patch(source: &Path, patch: &Path, reverse: bool) -> Result<()> {
    let input = fs::File::open(patch)?;
    let mut command = Command::new("patch");
    command
        .current_dir(source)
        .args(["--batch", "--silent", "-p1"]);
    if reverse {
        command.arg("--reverse");
    }
    command.stdin(Stdio::from(input));
    run_command(&mut command, &format!("apply patch {}", patch.display()))
}

fn build_probe(
    repository_root: &Path,
    source: &Path,
    prefix: &Path,
    temp_root: &Path,
) -> Result<String> {
    let cache = temp_root.join("zig-cache");
    let global_cache = temp_root.join("zig-global-cache");
    let system = env::var_os("GHOSTTY_ZIG_SYSTEM_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            HarnessError::new("GHOSTTY_ZIG_SYSTEM_DIR must name the offline package store")
        })?;
    if !system.is_dir() {
        return Err(HarnessError::new(format!(
            "offline Zig package store missing: {}",
            system.display()
        )));
    }
    run_command(
        Command::new("zig")
            .current_dir(source)
            .args([
                "build",
                "-Demit-lib-vt=true",
                "-Doptimize=ReleaseFast",
                "-Demit-xcframework=false",
                "-Dapp-runtime=none",
                "--prefix",
            ])
            .arg(prefix)
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--global-cache-dir")
            .arg(&global_cache)
            .arg("--system")
            .arg(&system),
        "build scratch libghostty-vt mutation",
    )?;

    let library = prefix.join("lib");
    let probe = prefix.join("mutation-probe");
    let rpath = format!("-Wl,-rpath,{}", library.display());
    run_command(
        Command::new("cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(prefix.join("include"))
            .arg(repository_root.join("vt-harness/src/mutation_probe.c"))
            .arg("-L")
            .arg(&library)
            .arg("-lghostty-vt")
            .arg(rpath)
            .arg("-o")
            .arg(&probe),
        "compile public C-ABI mutation probe",
    )?;
    command_stdout(&mut Command::new(&probe), "run public C-ABI mutation probe")
}

fn command_stdout(command: &mut Command, context: &str) -> Result<String> {
    let output = command
        .output()
        .map_err(|error| HarnessError::new(format!("{context}: failed to execute: {error}")))?;
    check_output(output, context).and_then(|bytes| {
        String::from_utf8(bytes)
            .map_err(|_| HarnessError::new(format!("{context}: stdout is not UTF-8")))
    })
}

fn run_command(command: &mut Command, context: &str) -> Result<()> {
    let output = command
        .output()
        .map_err(|error| HarnessError::new(format!("{context}: failed to execute: {error}")))?;
    check_output(output, context).map(|_| ())
}

fn check_output(output: Output, context: &str) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail = stderr
        .lines()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(HarnessError::new(format!(
        "{context}: status={} stderr-tail={tail}",
        output.status
    )))
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self> {
        let path = env::temp_dir().join(format!("remux-vt1-mutations-{}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_mutation_inventory_meets_floor_and_is_unique() {
        assert!(MUTATIONS.len() >= 14);
        let mut ids = MUTATIONS.iter().map(|row| row.0).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MUTATIONS.len());
    }

    #[test]
    fn failed_command_reports_stderr_tail() {
        let output = Command::new("sh")
            .args(["-c", "echo mutation-error >&2; exit 7"])
            .output()
            .expect("shell");
        let error = check_output(output, "unit command").expect_err("must fail");
        assert!(error.to_string().contains("mutation-error"));
    }
}
