// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
#![forbid(unsafe_code)]
//! Supervisor library boundary.
//!
//! Contract: callers provide validated event messages; this crate updates bounded
//! in-memory state and persists a passive schema that has no executable fields.

pub mod attach;
mod json;
pub mod protocol;
pub mod restore;
pub mod scrollback;
pub mod state;
