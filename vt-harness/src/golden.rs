//! Full-state golden snapshot execution.

use std::fs;
use std::path::Path;

use libghostty_vt::TerminalOptions;

use crate::abi::AbiImplementation;
use crate::cases::Case;
use crate::{fnv1a64, HarnessError, Result};

/// Result of running one fixed golden tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GoldenSummary {
    /// Number of table rows executed.
    pub cases: usize,
    /// Number of operation-step full states compared.
    pub snapshots: usize,
}

/// Executes every case and either blesses or compares per-step complete states.
///
/// Blessing is an explicit developer action and never used by the scorer.
pub fn run(
    implementation: &dyn AbiImplementation,
    cases: &[Case],
    directory: &Path,
    bless: bool,
) -> Result<GoldenSummary> {
    if bless {
        if directory.exists() {
            fs::remove_dir_all(directory).map_err(|error| {
                HarnessError::new(format!("remove {}: {error}", directory.display()))
            })?;
        }
        fs::create_dir_all(directory)?;
    } else if !directory.is_dir() {
        return Err(HarnessError::new(format!(
            "golden directory missing: {} (run with --bless intentionally)",
            directory.display()
        )));
    }

    let mut snapshots = 0_usize;
    for case in cases {
        let mut terminal = implementation.create(TerminalOptions {
            cols: case.cols,
            rows: case.rows,
            max_scrollback: case.max_scrollback,
        })?;
        let mut actual = String::new();
        for (step, operation) in case.operations.iter().enumerate() {
            terminal
                .apply(operation)
                .map_err(|error| HarnessError::new(format!("{} step {step}: {error}", case.id)))?;
            let state = terminal.full_state().map_err(|error| {
                HarnessError::new(format!("{} snapshot step {step}: {error}", case.id))
            })?;
            actual.push_str(&format!(
                "CASE {} STEP {} STATE {:016x}\n",
                case.id,
                step,
                fnv1a64(state.as_bytes())
            ));
            actual.push_str(&state);
            actual.push_str("END-STATE\n");
            snapshots += 1;
        }
        let path = directory.join(format!("{}.snap", case.id));
        if bless {
            fs::write(&path, actual)?;
            continue;
        }
        let expected = fs::read_to_string(&path).map_err(|error| {
            HarnessError::new(format!("read golden {}: {error}", path.display()))
        })?;
        if expected != actual {
            return Err(HarnessError::new(format!(
                "golden divergence implementation={} case={} file={} {}",
                implementation.name(),
                case.id,
                path.display(),
                first_difference(&expected, &actual)
            )));
        }
    }

    Ok(GoldenSummary {
        cases: cases.len(),
        snapshots,
    })
}

fn first_difference(expected: &str, actual: &str) -> String {
    for (index, (left, right)) in expected.lines().zip(actual.lines()).enumerate() {
        if left != right {
            return format!("line={} expected={left:?} actual={right:?}", index + 1);
        }
    }
    format!(
        "line-count expected={} actual={}",
        expected.lines().count(),
        actual.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_difference_reports_exact_line() {
        assert_eq!(
            first_difference("a\nb\n", "a\nc\n"),
            "line=2 expected=\"b\" actual=\"c\""
        );
    }

    #[test]
    fn first_difference_reports_truncation() {
        assert_eq!(
            first_difference("a\n", "a\nb\n"),
            "line-count expected=1 actual=2"
        );
    }
}
