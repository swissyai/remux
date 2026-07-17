//! Explicit authorization boundary for starting or restarting an attached command.
//!
//! Contract: an attach permit can only be obtained by consuming a prior audit-log
//! authorization. The opaque permit is the only public route to PTY process creation.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command};

use crate::pty;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachScope {
    Launch,
    Relaunch,
}

impl AttachScope {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "launch" => Ok(Self::Launch),
            "relaunch" => Ok(Self::Relaunch),
            _ => Err(invalid_input("attach scope must be launch or relaunch")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Relaunch => "relaunch",
        }
    }
}

impl fmt::Display for AttachScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct AttachAuthorization {
    scope: AttachScope,
}

impl AttachAuthorization {
    pub fn scope(&self) -> AttachScope {
        self.scope
    }
}

pub fn record_authorization(log: &Path, scope: AttachScope, token: &str) -> io::Result<()> {
    validate_token(token)?;
    append_record(log, "authorized", scope, token)
}

pub fn consume_authorization(
    log: &Path,
    scope: AttachScope,
    token: Option<&str>,
) -> io::Result<AttachAuthorization> {
    let Some(token) = token else {
        append_record(log, "refused", scope, "missing-token")?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{scope} requires an explicit authorization token"),
        ));
    };
    validate_token(token)?;
    let (grants, uses) = authorization_counts(log, scope, token)?;
    if grants <= uses {
        append_record(log, "refused", scope, token)?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("no unused {scope} authorization for token"),
        ));
    }
    append_record(log, "attached", scope, token)?;
    Ok(AttachAuthorization { scope })
}

pub fn spawn_authorized_pty(
    authorization: &AttachAuthorization,
    command: &mut Command,
) -> io::Result<(Child, File)> {
    let _scope = authorization.scope();
    pty::spawn_pty(command)
}

fn authorization_counts(log: &Path, scope: AttachScope, token: &str) -> io::Result<(u64, u64)> {
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach authorization log record",
            ));
        };
        AttachScope::parse(record_scope).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid attach scope in authorization log",
            )
        })?;
        validate_token(record_token).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid token in attach authorization log",
            )
        })?;
        if *record_scope == scope.as_str() && *record_token == token {
            match *action {
                "authorized" => grants = grants.saturating_add(1),
                "attached" => uses = uses.saturating_add(1),
                "refused" => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid action in attach authorization log",
                    ))
                }
            }
        } else if !matches!(*action, "authorized" | "attached" | "refused") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid action in attach authorization log",
            ));
        }
    }
    Ok((grants, uses))
}

fn append_record(log: &Path, action: &str, scope: AttachScope, token: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(file, "{action}\t{}\t{token}", scope.as_str())?;
    file.sync_data()
}

fn validate_token(token: &str) -> io::Result<()> {
    if token.is_empty()
        || token.len() > 128
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid_input("invalid attach authorization token"));
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
