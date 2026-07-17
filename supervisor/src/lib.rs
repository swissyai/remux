// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Supervisor library boundary.
//!
//! Contract: callers provide validated event messages; this crate updates bounded
//! in-memory state and persists a passive schema that has no executable fields.

pub mod attach;
mod json;
pub mod protocol;
mod pty;
pub mod scrollback;
pub mod state;
