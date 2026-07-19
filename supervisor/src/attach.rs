//! Logged authorization boundary for drive and lifecycle capabilities.
//!
//! Contract: a drive or lifecycle proof can only be obtained by consuming one
//! prior audit-log authorization. The exact lifecycle proof is the only public
//! route to PTY process creation; drive proofs cannot spawn processes.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::capability::{DriveCapability, LifecycleAction, LifecycleCapability};

/// Lifecycle authorization scope accepted by the CLI and audit log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachScope {
    /// First launch of supervised sessions.
    Launch,
    /// Explicit relaunch after passive restore or exit.
    Relaunch,
}

impl AttachScope {
    /// Parses the stable CLI/log spelling.
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "launch" => Ok(Self::Launch),
            "relaunch" => Ok(Self::Relaunch),
            _ => Err(invalid_input("attach scope must be launch or relaunch")),
        }
    }

    /// Stable CLI/log spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Relaunch => "relaunch",
        }
    }

    fn action(self) -> LifecycleAction {
        match self {
            Self::Launch => LifecycleAction::Launch,
            Self::Relaunch => LifecycleAction::Relaunch,
        }
    }
}

impl fmt::Display for AttachScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Records one single-use lifecycle authorization.
pub fn record_authorization(log: &Path, scope: AttachScope, token: &str) -> io::Result<()> {
    validate_token(token)?;
    append_record(log, "authorized", scope.as_str(), token)
}

/// Records one single-use drive authorization.
pub fn record_drive_authorization(log: &Path, token: &str) -> io::Result<()> {
    validate_token(token)?;
    append_record(log, "authorized", "drive", token)
}

/// Consumes one lifecycle grant and binds its exact proof to `session_ids`.
pub fn consume_lifecycle_authorization<I, S>(
    log: &Path,
    scope: AttachScope,
    token: Option<&str>,
    session_ids: I,
) -> io::Result<LifecycleCapability>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    consume(log, scope.as_str(), "attached", token)?;
    LifecycleCapability::granted(scope.action(), session_ids)
}

/// Consumes one drive grant and binds its exact proof to `session_ids`.
pub fn consume_drive_authorization<I, S>(
    log: &Path,
    token: Option<&str>,
    session_ids: I,
) -> io::Result<DriveCapability>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    consume(log, "drive", "driving", token)?;
    DriveCapability::granted(session_ids)
}

/// Spawns `command` on a PTY under the exact lifecycle proof.
///
/// The function refuses an out-of-scope session before process creation and
/// returns every PTY/spawn failure. This remains the sole supervisor route to
/// [`remux_pty::spawn_pty`].
///
/// An observe proof cannot call this route:
///
/// ```compile_fail
/// use std::process::Command;
/// use supervisor::attach::spawn_authorized_pty;
/// use supervisor::capability::observe_sessions;
/// let observe = observe_sessions(["session-000"]).unwrap();
/// let mut command = Command::new("/usr/bin/true");
/// spawn_authorized_pty(&observe, "session-000", &mut command).unwrap();
/// ```
pub fn spawn_authorized_pty(
    authorization: &LifecycleCapability,
    session_id: &str,
    command: &mut Command,
) -> io::Result<(Child, File)> {
    if !authorization.permits(session_id) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "lifecycle capability does not cover session",
        ));
    }
    remux_pty::spawn_pty(command)
}

/// Rebuilds a command under the macOS sandbox with writes to `directory` denied.
///
/// Program, arguments, explicit environment changes, and current directory are
/// preserved. Standard-I/O configuration is intentionally not accepted; callers
/// attach the returned command through [`spawn_authorized_pty`]. The function fails
/// closed when the platform sandbox executable or a canonical directory is absent.
pub fn deny_attestation_writes(command: &Command, directory: &Path) -> io::Result<Command> {
    let sandbox = Path::new("/usr/bin/sandbox-exec");
    if !sandbox.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "attested sessions require /usr/bin/sandbox-exec",
        ));
    }
    let protected = fs::canonicalize(directory)?;
    let protected = protected
        .to_str()
        .ok_or_else(|| invalid_input("attestation directory path is not UTF-8"))?;
    let escaped = protected.replace('\\', "\\\\").replace('"', "\\\"");
    let profile = format!("(version 1) (allow default) (deny file-write* (subpath \"{escaped}\"))");
    let mut wrapped = Command::new(sandbox);
    wrapped.arg("-p").arg(profile).arg(command.get_program());
    wrapped.args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                wrapped.env(key, value);
            }
            None => {
                wrapped.env_remove(key);
            }
        }
    }
    if let Some(directory) = command.get_current_dir() {
        wrapped.current_dir(directory);
    }
    Ok(wrapped)
}

fn consume(log: &Path, scope: &str, action: &str, token: Option<&str>) -> io::Result<()> {
    let Some(token) = token else {
        append_record(log, "refused", scope, "missing-token")?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{scope} requires an explicit authorization token"),
        ));
    };
    validate_token(token)?;
    let (grants, uses) = authorization_counts(log, scope, token, action)?;
    if grants <= uses {
        append_record(log, "refused", scope, token)?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("no unused {scope} authorization for token"),
        ));
    }
    if let Err(error) = claim_single_use(log, scope, token) {
        if error.kind() == io::ErrorKind::AlreadyExists {
            append_record(log, "refused", scope, token)?;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{scope} authorization token was already claimed"),
            ));
        }
        return Err(error);
    }
    append_record(log, action, scope, token)
}

fn authorization_counts(
    log: &Path,
    scope: &str,
    token: &str,
    consume_action: &str,
) -> io::Result<(u64, u64)> {
    let file = match File::open(log) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error),
    };
    let mut grants = 0_u64;
    let mut uses = 0_u64;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields = line.split('\t').collect::<Vec<_>>();
        let [action, record_scope, record_token] = fields.as_slice() else {
            return Err(invalid_data("invalid capability authorization log record"));
        };
        validate_scope(record_scope)?;
        validate_token(record_token)
            .map_err(|_| invalid_data("invalid token in capability authorization log"))?;
        if *record_scope == scope && *record_token == token {
            match *action {
                "authorized" => grants = grants.saturating_add(1),
                value if value == consume_action => uses = uses.saturating_add(1),
                "attached" | "driving" | "refused" => {}
                _ => return Err(invalid_data("invalid capability authorization action")),
            }
        } else if !matches!(*action, "authorized" | "attached" | "driving" | "refused") {
            return Err(invalid_data("invalid capability authorization action"));
        }
    }
    Ok((grants, uses))
}

fn claim_single_use(log: &Path, scope: &str, token: &str) -> io::Result<()> {
    let path = claim_path(log, scope, token)?;
    let mut claim = OpenOptions::new().create_new(true).write(true).open(path)?;
    writeln!(claim, "claimed\t{scope}\t{token}")?;
    claim.sync_all()?;
    File::open(log.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
}

fn claim_path(log: &Path, scope: &str, token: &str) -> io::Result<PathBuf> {
    let file_name = log
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_input("authorization log needs a UTF-8 file name"))?;
    Ok(log.with_file_name(format!(".{file_name}.claim-{scope}-{token}")))
}

fn append_record(log: &Path, action: &str, scope: &str, token: &str) -> io::Result<()> {
    validate_scope(scope)?;
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(file, "{action}\t{scope}\t{token}")?;
    file.sync_data()
}

fn validate_scope(scope: &str) -> io::Result<()> {
    if matches!(scope, "drive" | "launch" | "relaunch") {
        Ok(())
    } else {
        Err(invalid_data(
            "invalid capability scope in authorization log",
        ))
    }
}

fn validate_token(token: &str) -> io::Result<()> {
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid_input("invalid capability authorization token"));
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
