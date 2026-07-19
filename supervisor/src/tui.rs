//! Std-only ANSI tracer tabs over passive or live session state.
//!
//! Contract: this module owns presentation data and writes a frame only when its
//! caller explicitly requests `redraw`. It has no timer, process, attach, socket,
//! or authorization capability. Input text is bounded and sanitized before it can
//! reach a terminal. Output failures are returned immediately.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};

use crate::capability::DrivePresence;
use crate::protocol::{Event, EventKind};
use crate::state::PersistedState;

const MAX_TABS: usize = 64;
const MAX_LABEL_CHARS: usize = 32;
const MAX_STATUS_CHARS: usize = 32;
const MAX_TAIL_CHARS: usize = 120;
const TAIL_LINES: usize = 8;

/// A tab's observed connection state; detached state never implies a launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    /// Reconstructed from persisted data with no live execution capability.
    Detached,
    /// Updated by an already-running supervised session.
    Live,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Tab {
    session_id: String,
    label: String,
    status: String,
    tail: VecDeque<String>,
    connection: ConnectionState,
    agent_driving: bool,
}

/// Bounded presentation state for one tracer tab strip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TracerTabView {
    tabs: Vec<Tab>,
    index_by_session: BTreeMap<String, usize>,
    selected: usize,
}

impl TracerTabView {
    /// Reconstructs tabs from validated passive state.
    ///
    /// Every tab starts detached and not agent-driven. The function fails on an
    /// empty, oversized, or duplicate session set rather than rendering ambiguity.
    pub fn from_passive(state: &PersistedState) -> io::Result<Self> {
        Self::from_sessions(
            state.sessions.iter().map(|session| {
                (
                    session.id.as_str(),
                    session.status.as_str(),
                    session.scrollback.tail.as_slice(),
                )
            }),
            ConnectionState::Detached,
            &DrivePresence::none(),
        )
    }

    /// Creates live tabs for sessions that were already launched through the
    /// supervisor's authorization boundary.
    ///
    /// This function only records presentation state; it cannot launch or attach.
    /// It fails for an empty, oversized, invalid, or duplicate session set.
    pub fn live<I, S>(session_ids: I, drive: &DrivePresence) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::from_sessions(
            session_ids
                .into_iter()
                .map(|session_id| (session_id, "starting", &[] as &[String])),
            ConnectionState::Live,
            drive,
        )
    }

    fn from_sessions<'a, I, S, T>(
        sessions: I,
        connection: ConnectionState,
        drive: &DrivePresence,
    ) -> io::Result<Self>
    where
        I: IntoIterator<Item = (S, T, &'a [String])>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        let mut tabs = Vec::new();
        let mut index_by_session = BTreeMap::new();
        for (session_id, status, tail) in sessions {
            let session_id = session_id.as_ref();
            validate_session_id(session_id)?;
            if tabs.len() == MAX_TABS {
                return Err(invalid_data("tab count exceeds limit"));
            }
            let index = tabs.len();
            if index_by_session
                .insert(session_id.to_owned(), index)
                .is_some()
            {
                return Err(invalid_data("duplicate tracer tab"));
            }
            let tail = tail
                .iter()
                .rev()
                .take(TAIL_LINES)
                .map(|line| display_text(line, MAX_TAIL_CHARS))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            tabs.push(Tab {
                session_id: session_id.to_owned(),
                label: display_text(session_id, MAX_LABEL_CHARS),
                status: display_text(status.as_ref(), MAX_STATUS_CHARS),
                tail,
                connection,
                agent_driving: drive.is_driven(session_id),
            });
        }
        if tabs.is_empty() {
            return Err(invalid_data("tracer tab strip cannot be empty"));
        }
        Ok(Self {
            tabs,
            index_by_session,
            selected: 0,
        })
    }

    /// Applies already-validated live events to presentation state.
    ///
    /// The caller supplies a projection derived from held drive capabilities.
    /// Unknown sessions fail without a partial redraw. No I/O occurs here.
    pub fn apply_batch(&mut self, events: &[Event], drive: &DrivePresence) -> io::Result<()> {
        for event in events {
            if !self.index_by_session.contains_key(&event.session_id) {
                return Err(invalid_data("TUI event names unknown session"));
            }
        }
        for event in events {
            let index = *self
                .index_by_session
                .get(&event.session_id)
                .ok_or_else(|| invalid_data("TUI event names unknown session"))?;
            let tab = self
                .tabs
                .get_mut(index)
                .ok_or_else(|| invalid_data("TUI tab index is invalid"))?;
            tab.connection = ConnectionState::Live;
            tab.agent_driving = drive.is_driven(&event.session_id);
            match event.kind {
                EventKind::Status => {
                    tab.status = display_text(&event.payload, MAX_STATUS_CHARS);
                }
                EventKind::Output => {
                    if tab.tail.len() == TAIL_LINES {
                        tab.tail.pop_front();
                    }
                    tab.tail
                        .push_back(display_text(&event.payload, MAX_TAIL_CHARS));
                }
                EventKind::Tool => {}
            }
        }
        Ok(())
    }

    /// Returns a complete ANSI frame for the current tab strip.
    #[must_use]
    pub fn render_ansi(&self) -> String {
        let mut frame = String::with_capacity(self.tabs.len().saturating_mul(80));
        frame.push_str("\x1b[2J\x1b[HREMUX / TRACER / ");
        frame.push_str(&self.tabs.len().to_string());
        frame.push_str(" SESSIONS\nTABS ");
        for (index, tab) in self.tabs.iter().enumerate() {
            if index == self.selected {
                frame.push_str("\x1b[7m");
            }
            frame.push('[');
            frame.push_str(&format!(
                "{:02} {} | {} | {}",
                index + 1,
                tab.label,
                tab.status,
                indicator(tab)
            ));
            frame.push(']');
            if index == self.selected {
                frame.push_str("\x1b[0m");
            }
            if index + 1 != self.tabs.len() {
                frame.push(' ');
            }
        }
        frame.push('\n');
        let selected = &self.tabs[self.selected];
        frame.push_str("SESSION ");
        frame.push_str(&selected.label);
        frame.push_str("  STATUS ");
        frame.push_str(&selected.status);
        frame.push_str("  CONTROL ");
        frame.push_str(indicator(selected));
        frame.push_str("\nTAIL (last ");
        frame.push_str(&TAIL_LINES.to_string());
        frame.push_str(")\n");
        if selected.tail.is_empty() {
            frame.push_str("  (no output)\n");
        } else {
            for line in &selected.tail {
                frame.push_str("  ");
                frame.push_str(line);
                frame.push('\n');
            }
        }
        frame
    }

    /// Number of tabs represented by this bounded view.
    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }
}

/// Explicit frame writer. Construction and state updates do not write output;
/// only `redraw` does, which keeps redraw scheduling event-driven at the caller.
pub struct TracerRenderer<W> {
    output: W,
    view: TracerTabView,
    frames_rendered: u64,
}

impl<W: Write> TracerRenderer<W> {
    /// Creates a renderer with zero emitted frames.
    pub fn new(output: W, view: TracerTabView) -> Self {
        Self {
            output,
            view,
            frames_rendered: 0,
        }
    }

    /// Accesses presentation state without causing output.
    pub fn view_mut(&mut self) -> &mut TracerTabView {
        &mut self.view
    }

    /// Writes and flushes exactly one complete ANSI frame.
    ///
    /// Any write or flush error is returned; the frame counter advances only
    /// after the flush succeeds.
    pub fn redraw(&mut self) -> io::Result<()> {
        let frame = self.view.render_ansi();
        self.output.write_all(frame.as_bytes())?;
        self.output.flush()?;
        self.frames_rendered = self
            .frames_rendered
            .checked_add(1)
            .ok_or_else(|| io::Error::other("TUI frame counter overflow"))?;
        Ok(())
    }

    /// Number of successfully flushed frames.
    #[must_use]
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Returns the owned output after rendering.
    pub fn into_inner(self) -> W {
        self.output
    }
}

fn indicator(tab: &Tab) -> &'static str {
    match (tab.connection, tab.agent_driving) {
        (ConnectionState::Detached, _) => "DETACHED",
        (ConnectionState::Live, true) => "AGENT DRIVING",
        (ConnectionState::Live, false) => "LIVE",
    }
}

fn display_text(value: &str, maximum_chars: usize) -> String {
    let mut output = String::with_capacity(value.len().min(maximum_chars));
    let mut characters = value.chars();
    for character in characters.by_ref().take(maximum_chars) {
        if character.is_control() {
            output.push('�');
        } else {
            output.push(character);
        }
    }
    if characters.next().is_some() {
        output.push('…');
    }
    output
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_data("invalid TUI session id"));
    }
    Ok(())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
