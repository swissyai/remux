//! Machine-checked VT state invariants independent of A/B equivalence.

use libghostty_vt::screen::Screen;
use libghostty_vt::terminal::Mode;
use libghostty_vt::TerminalOptions;

use crate::abi::{AbiImplementation, AbiTerminal, Operation};
use crate::{HarnessError, Result};

/// Names of all independently executed properties.
pub const PROPERTY_NAMES: &[&str] = &[
    "cursor-bounds-after-print",
    "cursor-bounds-after-extreme-cup",
    "cursor-bounds-after-shrink",
    "resize-dimensions-exact",
    "total-rows-cover-viewport",
    "scrollback-accounting-consistent",
    "reset-preserves-dimensions",
    "reset-clears-scrollback",
    "split-escape-safety",
    "split-utf8-safety",
    "attributes-survive-resize",
    "reflow-preserves-glyph-count",
    "alternate-screen-isolation",
    "mode-roundtrip",
    "sgr-reset-is-non-retroactive",
];

/// Executes every property against one implementation and returns the names
/// that passed. The first violation fails the harness with its property name.
pub fn run(implementation: &dyn AbiImplementation) -> Result<Vec<&'static str>> {
    let mut passed = Vec::with_capacity(PROPERTY_NAMES.len());

    property(implementation, PROPERTY_NAMES[0], |terminal| {
        terminal.apply(&Operation::write(vec![b'x'; 20_000]))?;
        cursor_in_bounds(terminal)
    })?;
    passed.push(PROPERTY_NAMES[0]);

    property(implementation, PROPERTY_NAMES[1], |terminal| {
        terminal.apply(&Operation::write(b"\x1b[99999;99999H".to_vec()))?;
        cursor_in_bounds(terminal)
    })?;
    passed.push(PROPERTY_NAMES[1]);

    property(implementation, PROPERTY_NAMES[2], |terminal| {
        terminal.apply(&Operation::write(b"\x1b[8;16H".to_vec()))?;
        terminal.apply(&Operation::Resize {
            cols: 3,
            rows: 2,
            cell_width_px: 9,
            cell_height_px: 18,
        })?;
        cursor_in_bounds(terminal)
    })?;
    passed.push(PROPERTY_NAMES[2]);

    property(implementation, PROPERTY_NAMES[3], |terminal| {
        terminal.apply(&Operation::Resize {
            cols: 23,
            rows: 11,
            cell_width_px: 7,
            cell_height_px: 14,
        })?;
        let facts = terminal.facts()?;
        require(
            facts.cols == 23 && facts.rows == 11,
            "resize dimensions differ",
        )
    })?;
    passed.push(PROPERTY_NAMES[3]);

    property(implementation, PROPERTY_NAMES[4], |terminal| {
        terminal.apply(&Operation::write(
            b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\n".to_vec(),
        ))?;
        let facts = terminal.facts()?;
        require(
            facts.total_rows >= usize::from(facts.rows),
            "total rows smaller than viewport",
        )
    })?;
    passed.push(PROPERTY_NAMES[4]);

    let mut bounded = implementation.create(TerminalOptions {
        cols: 8,
        rows: 3,
        max_scrollback: 5,
    })?;
    bounded.apply(&Operation::write(
        (0..200)
            .map(|n| format!("{n}\r\n"))
            .collect::<String>()
            .into_bytes(),
    ))?;
    let bounded_facts = bounded.facts()?;
    require(
        bounded_facts.total_rows == bounded_facts.scrollback_rows + usize::from(bounded_facts.rows),
        PROPERTY_NAMES[5],
    )?;
    passed.push(PROPERTY_NAMES[5]);

    property(implementation, PROPERTY_NAMES[6], |terminal| {
        terminal.apply(&Operation::Resize {
            cols: 13,
            rows: 7,
            cell_width_px: 8,
            cell_height_px: 16,
        })?;
        terminal.apply(&Operation::Reset)?;
        let facts = terminal.facts()?;
        require(
            facts.cols == 13 && facts.rows == 7,
            "reset changed geometry",
        )
    })?;
    passed.push(PROPERTY_NAMES[6]);

    property(implementation, PROPERTY_NAMES[7], |terminal| {
        terminal.apply(&Operation::write(
            (0..40)
                .map(|n| format!("line{n}\r\n"))
                .collect::<String>()
                .into_bytes(),
        ))?;
        terminal.apply(&Operation::Reset)?;
        require(
            terminal.facts()?.scrollback_rows == 0,
            "reset retained scrollback",
        )
    })?;
    passed.push(PROPERTY_NAMES[7]);

    split_property(
        implementation,
        PROPERTY_NAMES[8],
        b"before\x1b[1;38;2;1;2;3;48;5;7mstyled\x1b[0m\x1b[4;5Hafter",
        &[1, 2, 7, 13, 21, 34],
    )?;
    passed.push(PROPERTY_NAMES[8]);

    split_property(
        implementation,
        PROPERTY_NAMES[9],
        "Aé界🙂e\u{301}Z".as_bytes(),
        &[1, 2, 3, 5, 8, 10],
    )?;
    passed.push(PROPERTY_NAMES[9]);

    property(implementation, PROPERTY_NAMES[10], |terminal| {
        terminal.apply(&Operation::write(b"\x1b[1;3;4;38;2;9;8;7mX".to_vec()))?;
        terminal.apply(&Operation::Resize {
            cols: 9,
            rows: 5,
            cell_width_px: 8,
            cell_height_px: 16,
        })?;
        let state = terminal.full_state()?;
        require(
            state.contains("glyph=58 ")
                && state.contains("bold=true italic=true")
                && state.contains("underline=Single"),
            "styled cell changed across resize",
        )
    })?;
    passed.push(PROPERTY_NAMES[10]);

    property(implementation, PROPERTY_NAMES[11], |terminal| {
        terminal.apply(&Operation::write(vec![b'a'; 70]))?;
        let before = terminal.full_state()?.matches("glyph=61 ").count();
        terminal.apply(&Operation::Resize {
            cols: 7,
            rows: 12,
            cell_width_px: 8,
            cell_height_px: 16,
        })?;
        let after = terminal.full_state()?.matches("glyph=61 ").count();
        require(
            before == 70 && after == before,
            "reflow lost or duplicated glyphs",
        )
    })?;
    passed.push(PROPERTY_NAMES[11]);

    property(implementation, PROPERTY_NAMES[12], |terminal| {
        terminal.apply(&Operation::write(b"primary".to_vec()))?;
        let primary = terminal.full_state()?;
        terminal.apply(&Operation::write(b"\x1b[?1049halternate".to_vec()))?;
        require(
            terminal.facts()?.active_screen == Screen::Alternate,
            "alternate screen did not activate",
        )?;
        terminal.apply(&Operation::write(b"\x1b[?1049l".to_vec()))?;
        require(
            terminal.facts()?.active_screen == Screen::Primary,
            "primary screen did not restore",
        )?;
        require(
            terminal.full_state()? == primary,
            "alternate screen modified primary state",
        )
    })?;
    passed.push(PROPERTY_NAMES[12]);

    property(implementation, PROPERTY_NAMES[13], |terminal| {
        terminal.apply(&Operation::write(b"\x1b[?2004h".to_vec()))?;
        require(terminal.mode(Mode::BRACKETED_PASTE)?, "mode did not set")?;
        terminal.apply(&Operation::write(b"\x1b[?2004l".to_vec()))?;
        require(!terminal.mode(Mode::BRACKETED_PASTE)?, "mode did not reset")
    })?;
    passed.push(PROPERTY_NAMES[13]);

    property(implementation, PROPERTY_NAMES[14], |terminal| {
        terminal.apply(&Operation::write(b"\x1b[1mX\x1b[0mY".to_vec()))?;
        let state = terminal.full_state()?;
        let bold = state
            .lines()
            .find(|line| line.contains("glyph=58 "))
            .is_some_and(|line| line.contains("bold=true"));
        let reset = state
            .lines()
            .find(|line| line.contains("glyph=59 "))
            .is_some_and(|line| line.contains("bold=false"));
        require(bold && reset, "SGR reset retroactively changed cells")
    })?;
    passed.push(PROPERTY_NAMES[14]);

    Ok(passed)
}

fn property(
    implementation: &dyn AbiImplementation,
    name: &str,
    check: impl FnOnce(&mut dyn AbiTerminal) -> Result<()>,
) -> Result<()> {
    let mut terminal = implementation.create(TerminalOptions {
        cols: 16,
        rows: 8,
        max_scrollback: 64,
    })?;
    check(terminal.as_mut())
        .map_err(|error| HarnessError::new(format!("invariant {name} failed: {error}")))
}

fn cursor_in_bounds(terminal: &dyn AbiTerminal) -> Result<()> {
    let facts = terminal.facts()?;
    require(
        facts.cursor_x < facts.cols && facts.cursor_y < facts.rows,
        "cursor outside terminal geometry",
    )
}

fn split_property(
    implementation: &dyn AbiImplementation,
    name: &str,
    bytes: &[u8],
    split_points: &[usize],
) -> Result<()> {
    let options = TerminalOptions {
        cols: 16,
        rows: 8,
        max_scrollback: 64,
    };
    let mut whole = implementation.create(options)?;
    whole.apply(&Operation::write(bytes.to_vec()))?;
    let expected = whole.full_state()?;

    let mut split = implementation.create(options)?;
    let mut start = 0_usize;
    for end in split_points
        .iter()
        .copied()
        .filter(|end| *end > 0 && *end < bytes.len())
        .chain(std::iter::once(bytes.len()))
    {
        if end > start {
            split.apply(&Operation::write(bytes[start..end].to_vec()))?;
            start = end;
        }
    }
    require(split.full_state()? == expected, name)
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(HarnessError::new(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::LinkedAbi;

    #[test]
    fn property_inventory_has_no_duplicates_and_exceeds_floor() {
        assert!(PROPERTY_NAMES.len() >= 12);
        let mut names = PROPERTY_NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROPERTY_NAMES.len());
    }

    #[test]
    fn split_boundary_property_is_live() {
        split_property(
            &LinkedAbi,
            "unit-split",
            "A\u{1b}[31mBé".as_bytes(),
            &[1, 2, 4, 7, 9],
        )
        .expect("split property");
    }
}
