//! Public arbitrary-command supervision behind exact lifecycle and observe proofs.
//!
//! Contract: callers must supply a prior single-use launch grant. The command runs
//! non-interactively on one PTY in the requested directory; the supervisor relays and
//! attests output but never grants itself drive authority. Every successful spawn is
//! finalized with an observed exit and externally reverified hash-chain head.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::attach::{
    consume_lifecycle_authorization, deny_attestation_writes, spawn_authorized_pty, AttachScope,
};
use crate::attestation::{
    attestation_path, verify_attestation, AttestationWriter, ExitOutcome, LifecyclePhase,
    LogIntegrity, SessionAttestationSummary,
};
use crate::capability::observe_sessions;

/// Environment variable carrying the authorization audit-log path.
pub const AUTH_LOG_ENV: &str = "REMUX_AUTH_LOG";
/// Environment variable carrying the already-recorded single-use launch token.
pub const ATTACH_TOKEN_ENV: &str = "REMUX_ATTACH_TOKEN";
/// Optional environment variable selecting the durable receipt directory.
pub const ATTESTATION_DIR_ENV: &str = "REMUX_ATTESTATION_DIR";

const SESSION_ID: &str = "run";

/// Validated inputs for one public supervised run.
#[derive(Debug)]
pub struct SupervisedRunConfig {
    /// Existing working directory inherited by the child only.
    pub cwd: PathBuf,
    /// Exact executable and argv; no shell interpretation is added.
    pub command: Vec<OsString>,
    /// Existing authorization audit log containing `attach_token`'s launch grant.
    pub auth_log: PathBuf,
    /// Atomic single-use lifecycle token, removed from the child environment.
    pub attach_token: String,
    /// New directory in which `run.attest` is durably created.
    pub attestation_dir: PathBuf,
}

/// Durable outcome from a completed child, after external chain verification.
#[derive(Debug)]
pub struct SupervisedRunReceipt {
    /// Child process outcome observed by the supervisor.
    pub status: ExitStatus,
    /// Canonical attestation path for the one supervised session.
    pub attestation_path: PathBuf,
    /// Verified summary of the synchronized chain.
    pub attestation: SessionAttestationSummary,
}

/// Runs one command through the sole authorized PTY process route.
///
/// Output bytes are observed before being relayed to `output`. A relay failure kills
/// and waits for the child before returning the write error. Spawn, read, wait,
/// attestation-write, and external-verification failures are returned; a non-zero
/// child status is a valid receipt and is left to the CLI to propagate.
pub fn run_attested_command(
    config: SupervisedRunConfig,
    output: &mut impl Write,
) -> io::Result<SupervisedRunReceipt> {
    let cwd = canonical_directory(&config.cwd)?;
    if config.command.is_empty() || config.command[0].is_empty() {
        return Err(invalid_input("supervised command cannot be empty"));
    }

    let lifecycle = consume_lifecycle_authorization(
        &config.auth_log,
        AttachScope::Launch,
        Some(&config.attach_token),
        [SESSION_ID],
    )?;
    let observe = observe_sessions([SESSION_ID])?;
    let writer = AttestationWriter::start(&config.attestation_dir, [SESSION_ID], &observe)?;
    let observer = writer.observer()?;
    observer.lifecycle(SESSION_ID, LifecyclePhase::Created)?;

    let mut command = Command::new(&config.command[0]);
    command.args(&config.command[1..]).current_dir(cwd);
    command
        .env_remove(AUTH_LOG_ENV)
        .env_remove(ATTACH_TOKEN_ENV)
        .env_remove(ATTESTATION_DIR_ENV);
    let mut command = deny_attestation_writes(&command, &config.attestation_dir)?;
    let (mut child, mut master) = match spawn_authorized_pty(&lifecycle, SESSION_ID, &mut command) {
        Ok(spawned) => spawned,
        Err(error) => {
            observer.lifecycle(SESSION_ID, LifecyclePhase::Ended)?;
            drop(observer);
            let summary = writer.finish()?;
            verify_summary(&config.attestation_dir, &summary[SESSION_ID])?;
            return Err(error);
        }
    };
    observer.spawn(SESSION_ID, child.id())?;
    observer.lifecycle(SESSION_ID, LifecyclePhase::Running)?;

    let mut buffer = [0_u8; 8_192];
    let relay_result = loop {
        match master.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(bytes) => {
                if let Err(error) = observer
                    .output(SESSION_ID, &buffer[..bytes])
                    .and_then(|()| output.write_all(&buffer[..bytes]))
                    .and_then(|()| output.flush())
                {
                    break Err(error);
                }
            }
            Err(error) if error.raw_os_error() == Some(5) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    if relay_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    observer.exit(SESSION_ID, exit_outcome(status)?)?;
    observer.lifecycle(SESSION_ID, LifecyclePhase::Ended)?;
    drop(observer);
    let summary = writer.finish()?;
    let attestation = summary
        .get(SESSION_ID)
        .ok_or_else(|| io::Error::other("supervised run emitted no attestation summary"))?
        .clone();
    verify_summary(&config.attestation_dir, &attestation)?;
    relay_result?;

    Ok(SupervisedRunReceipt {
        status,
        attestation_path: attestation_path(&config.attestation_dir, SESSION_ID),
        attestation,
    })
}

fn canonical_directory(path: &Path) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    if canonical.is_dir() {
        Ok(canonical)
    } else {
        Err(invalid_input("supervised cwd is not a directory"))
    }
}

fn verify_summary(directory: &Path, expected: &SessionAttestationSummary) -> io::Result<()> {
    let verified = verify_attestation(&attestation_path(directory, SESSION_ID))?;
    if verified.integrity != LogIntegrity::Complete
        || verified.records != expected.records
        || verified.input_bytes != expected.input_bytes
        || verified.output_bytes != expected.output_bytes
        || verified.head != expected.head
    {
        return Err(io::Error::other(
            "supervised run summary differs from external verification",
        ));
    }
    Ok(())
}

fn exit_outcome(status: ExitStatus) -> io::Result<ExitOutcome> {
    status
        .code()
        .map(ExitOutcome::Code)
        .or_else(|| status.signal().map(ExitOutcome::Signal))
        .ok_or_else(|| io::Error::other("child exit has neither code nor signal"))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
