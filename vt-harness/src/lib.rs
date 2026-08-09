#![forbid(unsafe_code)]
//! Deterministic black-box characterization of libghostty-vt's published C ABI.
//!
//! The harness intentionally depends on the same safe Rust C-ABI wrapper as the
//! cockpit. Expected values are generated only by executing that interface.

pub mod abi;
pub mod cases;
pub mod corpus;
pub mod differential;
pub mod fuzz;
pub mod golden;
pub mod invariants;
pub mod mutation;
pub mod receipt;
pub mod snapshot;

use std::fmt;
use std::io;

/// Harness-wide error with enough context to reproduce the failing tier.
#[derive(Debug)]
pub struct HarnessError(String);

impl HarnessError {
    /// Creates a contextual harness failure.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HarnessError {}

impl From<io::Error> for HarnessError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<libghostty_vt::Error> for HarnessError {
    fn from(error: libghostty_vt::Error) -> Self {
        Self(error.to_string())
    }
}

/// Result type used by every harness tier.
pub type Result<T> = std::result::Result<T, HarnessError>;

/// Stable non-cryptographic digest used only to make large state traces legible.
/// Equality is always checked on the underlying bytes in differential runs.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

/// Encodes arbitrary bytes as deterministic lowercase hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
