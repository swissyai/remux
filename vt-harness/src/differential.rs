//! ABI-generic A/B execution with first-divergence receipts.

use libghostty_vt::TerminalOptions;

use crate::abi::{AbiImplementation, AbiTerminal, Operation};
use crate::{fnv1a64, hex, HarnessError, Result};

/// Two implementation-neutral ABI instances driven in exact lockstep.
pub struct Pair {
    left_name: String,
    right_name: String,
    left: Box<dyn AbiTerminal>,
    right: Box<dyn AbiTerminal>,
    step: u64,
}

impl Pair {
    /// Creates a pair with identical terminal options.
    pub fn new(
        left: &dyn AbiImplementation,
        right: &dyn AbiImplementation,
        options: TerminalOptions,
    ) -> Result<Self> {
        Ok(Self {
            left_name: left.name().to_owned(),
            right_name: right.name().to_owned(),
            left: left.create(options)?,
            right: right.create(options)?,
            step: 0,
        })
    }

    /// Applies one exact action and compares ABI-produced state immediately.
    /// Returns the common fast state for per-step corpus tracing.
    pub fn apply(&mut self, operation: &Operation, replay: &str) -> Result<Vec<u8>> {
        self.left.apply(operation).map_err(|error| {
            HarnessError::new(format!(
                "differential left={} replay={replay} step={} apply: {error}",
                self.left_name, self.step
            ))
        })?;
        self.right.apply(operation).map_err(|error| {
            HarnessError::new(format!(
                "differential right={} replay={replay} step={} apply: {error}",
                self.right_name, self.step
            ))
        })?;
        let left_state = self.left.fast_state()?;
        let right_state = self.right.fast_state()?;
        if left_state != right_state {
            let left_full = self.left.full_state()?;
            let right_full = self.right.full_state()?;
            let operation_bytes = match operation {
                Operation::Write(bytes) => hex(bytes),
                _ => format!("{operation:?}"),
            };
            return Err(HarnessError::new(format!(
                "VT DIVERGENCE replay={replay} step={} left={} right={} input={} leftFast={:016x} rightFast={:016x} {}",
                self.step,
                self.left_name,
                self.right_name,
                operation_bytes,
                fnv1a64(&left_state),
                fnv1a64(&right_state),
                first_difference(&left_full, &right_full),
            )));
        }
        self.step = self.step.saturating_add(1);
        Ok(left_state)
    }
}

/// Runs an operation list through two implementations and returns compared
/// step count. This is the cheap seam future Zig-vs-Rust waves reuse.
pub fn run(
    left: &dyn AbiImplementation,
    right: &dyn AbiImplementation,
    options: TerminalOptions,
    operations: &[Operation],
    replay: &str,
) -> Result<usize> {
    let mut pair = Pair::new(left, right, options)?;
    for operation in operations {
        let _state = pair.apply(operation, replay)?;
    }
    Ok(operations.len())
}

fn first_difference(left: &str, right: &str) -> String {
    for (index, (left_line, right_line)) in left.lines().zip(right.lines()).enumerate() {
        if left_line != right_line {
            return format!(
                "fullLine={} left={left_line:?} right={right_line:?}",
                index + 1
            );
        }
    }
    format!(
        "fullLineCount left={} right={}",
        left.lines().count(),
        right.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::LinkedAbi;

    #[test]
    fn linked_self_check_compares_every_step() {
        let abi = LinkedAbi;
        let count = run(
            &abi,
            &abi,
            TerminalOptions {
                cols: 8,
                rows: 4,
                max_scrollback: 16,
            },
            &[
                Operation::write(b"hello".to_vec()),
                Operation::write(b"\x1b[31mred".to_vec()),
            ],
            "unit-self-check",
        )
        .expect("self-check");
        assert_eq!(count, 2);
    }
}
