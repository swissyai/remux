//! Capability-free passive restore presentation boundary.
//!
//! Contract: this module can read and format persisted data only. It receives no
//! authorization, command, callback, socket, or process handle.

use std::io;
use std::path::Path;

use crate::state::restore_passive;

pub fn inspect_passive(path: &Path) -> io::Result<String> {
    let state = restore_passive(path)?;
    Ok(format!(
        "restored passive layout: {} sessions, policy {}",
        state.sessions.len(),
        state.restore_policy
    ))
}
