//! ABI-generic terminal factory and operation seam.

use libghostty_vt::screen::Screen;
use libghostty_vt::terminal::{Mode, ScrollViewport, Terminal};
use libghostty_vt::TerminalOptions;

use crate::{snapshot, Result};

/// One deterministic action accepted by every ABI implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Feed exact PTY bytes through the VT stream processor.
    Write(Vec<u8>),
    /// Resize the terminal with explicit cell pixel dimensions.
    Resize {
        /// New cell columns.
        cols: u16,
        /// New cell rows.
        rows: u16,
        /// Cell width in pixels.
        cell_width_px: u32,
        /// Cell height in pixels.
        cell_height_px: u32,
    },
    /// Scroll the viewport to the top of history.
    ScrollTop,
    /// Scroll the viewport to the live bottom.
    ScrollBottom,
    /// Perform a full terminal reset.
    Reset,
}

impl Operation {
    /// Convenience constructor for an exact write.
    #[must_use]
    pub fn write(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Write(bytes.into())
    }
}

/// Small scalar state used by machine-checked invariants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalFacts {
    /// Current columns.
    pub cols: u16,
    /// Current rows.
    pub rows: u16,
    /// Cursor column.
    pub cursor_x: u16,
    /// Cursor row.
    pub cursor_y: u16,
    /// Total active-screen rows including history.
    pub total_rows: usize,
    /// Retained history rows.
    pub scrollback_rows: usize,
    /// Current screen buffer.
    pub active_screen: Screen,
}

/// Terminal instance exposed only through implementation-neutral operations
/// and state snapshots.
pub trait AbiTerminal {
    /// Applies one operation. Failure is fatal; malformed writes themselves
    /// are infallible at the C ABI.
    fn apply(&mut self, operation: &Operation) -> Result<()>;

    /// Returns the deterministic complete state used by goldens.
    fn full_state(&self) -> Result<String>;

    /// Returns the bounded complete state used by high-volume differential runs.
    fn fast_state(&self) -> Result<Vec<u8>>;

    /// Returns scalar geometry/history facts for invariant checks.
    fn facts(&self) -> Result<TerminalFacts>;

    /// Queries one public terminal mode.
    fn mode(&self, mode: Mode) -> Result<bool>;
}

/// Factory for one implementation of the published libghostty-vt C ABI.
///
/// A future Rust port only needs another factory/terminal adapter; authored
/// cases, corpora, properties, fuzzing, and divergence reporting stay shared.
pub trait AbiImplementation {
    /// Human-legible implementation label included in divergences.
    fn name(&self) -> &str;

    /// Creates a fresh terminal with the requested geometry and scrollback.
    fn create(&self, options: TerminalOptions) -> Result<Box<dyn AbiTerminal>>;
}

/// The linked Zig libghostty-vt reached through the same wrapper as cockpit.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkedAbi;

impl AbiImplementation for LinkedAbi {
    fn name(&self) -> &str {
        "linked-libghostty-vt"
    }

    fn create(&self, options: TerminalOptions) -> Result<Box<dyn AbiTerminal>> {
        Ok(Box::new(LinkedTerminal {
            terminal: Terminal::new(options)?,
        }))
    }
}

struct LinkedTerminal {
    terminal: Terminal<'static, 'static>,
}

impl AbiTerminal for LinkedTerminal {
    fn apply(&mut self, operation: &Operation) -> Result<()> {
        match operation {
            Operation::Write(bytes) => self.terminal.vt_write(bytes),
            Operation::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => self
                .terminal
                .resize(*cols, *rows, *cell_width_px, *cell_height_px)?,
            Operation::ScrollTop => self.terminal.scroll_viewport(ScrollViewport::Top),
            Operation::ScrollBottom => self.terminal.scroll_viewport(ScrollViewport::Bottom),
            Operation::Reset => self.terminal.reset(),
        }
        Ok(())
    }

    fn full_state(&self) -> Result<String> {
        snapshot::full(&self.terminal)
    }

    fn fast_state(&self) -> Result<Vec<u8>> {
        snapshot::fast(&self.terminal)
    }

    fn facts(&self) -> Result<TerminalFacts> {
        Ok(TerminalFacts {
            cols: self.terminal.cols()?,
            rows: self.terminal.rows()?,
            cursor_x: self.terminal.cursor_x()?,
            cursor_y: self.terminal.cursor_y()?,
            total_rows: self.terminal.total_rows()?,
            scrollback_rows: self.terminal.scrollback_rows()?,
            active_screen: self.terminal.active_screen()?,
        })
    }

    fn mode(&self, mode: Mode) -> Result<bool> {
        Ok(self.terminal.mode(mode)?)
    }
}
