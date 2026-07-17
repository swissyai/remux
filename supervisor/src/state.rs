// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Bounded live state and passive persistence schema.
//!
//! Contract: persistence contains layout, event metadata, status, and bounded
//! scrollback only. Restore is data reconstruction: this module deliberately has no
//! process-spawn API, command field, callback, or executable extension point.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::json::{self, Value};
use crate::protocol::{Event, EventKind};

const SCHEMA_VERSION: u64 = 1;
const SCROLLBACK_TAIL_LINES: usize = 64;

#[derive(Clone, Debug)]
pub struct LiveState {
    session_order: Vec<String>,
    sessions: BTreeMap<String, LiveSession>,
}

#[derive(Clone, Debug)]
struct LiveSession {
    status: String,
    last_event: Option<PersistedEvent>,
    scrollback_tail: VecDeque<String>,
    next_scrollback_offset: u64,
}

impl LiveState {
    pub fn new<I, S>(session_ids: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut session_order = Vec::new();
        let mut sessions = BTreeMap::new();
        for session_id in session_ids {
            let session_id = session_id.into();
            validate_session_id(&session_id)?;
            let session = LiveSession {
                status: "starting".to_owned(),
                last_event: None,
                scrollback_tail: VecDeque::with_capacity(SCROLLBACK_TAIL_LINES),
                next_scrollback_offset: 0,
            };
            if sessions.insert(session_id.clone(), session).is_some() {
                return Err(invalid_data("duplicate session id"));
            }
            session_order.push(session_id);
        }
        Ok(Self {
            session_order,
            sessions,
        })
    }

    pub fn apply_batch(&mut self, events: &[Event]) -> io::Result<()> {
        for event in events {
            let session = self
                .sessions
                .get_mut(&event.session_id)
                .ok_or_else(|| invalid_data("event names unknown session"))?;
            if let Some(previous) = &session.last_event {
                if event.sequence <= previous.sequence {
                    return Err(invalid_data("event sequence is not increasing"));
                }
            }
            match event.kind {
                EventKind::Status => session.status.clone_from(&event.payload),
                EventKind::Output => {
                    if session.scrollback_tail.len() == SCROLLBACK_TAIL_LINES {
                        session.scrollback_tail.pop_front();
                    }
                    session.scrollback_tail.push_back(event.payload.clone());
                    session.next_scrollback_offset = session
                        .next_scrollback_offset
                        .checked_add(1)
                        .ok_or_else(|| invalid_data("scrollback offset overflow"))?;
                }
                EventKind::Tool => {}
            }
            session.last_event = Some(PersistedEvent {
                sequence: event.sequence,
                sent_unix_micros: event.sent_unix_micros,
                kind: event.kind.as_str().to_owned(),
            });
        }
        Ok(())
    }

    pub fn persisted(&self) -> PersistedState {
        let sessions = self
            .session_order
            .iter()
            .filter_map(|session_id| {
                self.sessions
                    .get(session_id)
                    .map(|session| PersistedSession {
                        id: session_id.clone(),
                        status: session.status.clone(),
                        last_event: session.last_event.clone(),
                        scrollback: PersistedScrollback {
                            tail: session.scrollback_tail.iter().cloned().collect(),
                            next_offset: session.next_scrollback_offset,
                        },
                    })
            })
            .collect();
        PersistedState {
            schema_version: SCHEMA_VERSION,
            restore_policy: "passive".to_owned(),
            layout: PersistedLayout {
                kind: "tabs".to_owned(),
                session_ids: self.session_order.clone(),
            },
            sessions,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedState {
    pub schema_version: u64,
    pub restore_policy: String,
    pub layout: PersistedLayout,
    pub sessions: Vec<PersistedSession>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedLayout {
    pub kind: String,
    pub session_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedSession {
    pub id: String,
    pub status: String,
    pub last_event: Option<PersistedEvent>,
    pub scrollback: PersistedScrollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedEvent {
    pub sequence: u64,
    pub sent_unix_micros: u64,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedScrollback {
    pub tail: Vec<String>,
    pub next_offset: u64,
}

pub fn dump_atomic(path: &Path, state: &LiveState) -> io::Result<()> {
    let persisted = state.persisted();
    let encoded = encode_state(&persisted);
    let temporary = temporary_path(path)?;
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_parent(path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

pub fn restore_passive(path: &Path) -> io::Result<PersistedState> {
    let input = fs::read_to_string(path)?;
    let value = json::parse(&input).map_err(|error| invalid_data(error.to_string()))?;
    decode_state(&value)
}

fn encode_state(state: &PersistedState) -> String {
    let mut output = String::new();
    output.push_str("{\n");
    output.push_str(&format!(
        "  \"schema_version\": {},\n",
        state.schema_version
    ));
    output.push_str(&format!(
        "  \"restore_policy\": {},\n",
        json::quote(&state.restore_policy)
    ));
    output.push_str("  \"layout\": {\n");
    output.push_str(&format!(
        "    \"kind\": {},\n",
        json::quote(&state.layout.kind)
    ));
    output.push_str("    \"session_ids\": [");
    for (index, session_id) in state.layout.session_ids.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&json::quote(session_id));
    }
    output.push_str("]\n  },\n");
    output.push_str("  \"sessions\": [\n");
    for (index, session) in state.sessions.iter().enumerate() {
        output.push_str("    {\n");
        output.push_str(&format!("      \"id\": {},\n", json::quote(&session.id)));
        output.push_str(&format!(
            "      \"status\": {},\n",
            json::quote(&session.status)
        ));
        output.push_str("      \"last_event\": ");
        match &session.last_event {
            Some(event) => output.push_str(&format!(
                "{{\"sequence\": {}, \"sent_unix_micros\": {}, \"kind\": {}}}",
                event.sequence,
                event.sent_unix_micros,
                json::quote(&event.kind)
            )),
            None => output.push_str("null"),
        }
        output.push_str(",\n      \"scrollback\": {\"tail\": [");
        for (tail_index, line) in session.scrollback.tail.iter().enumerate() {
            if tail_index > 0 {
                output.push_str(", ");
            }
            output.push_str(&json::quote(line));
        }
        output.push_str(&format!(
            "], \"next_offset\": {}}}\n    }}",
            session.scrollback.next_offset
        ));
        if index + 1 != state.sessions.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push_str("  ]\n}\n");
    output
}

fn decode_state(value: &Value) -> io::Result<PersistedState> {
    let object = object(value, "state")?;
    exact_fields(
        object,
        &["schema_version", "restore_policy", "layout", "sessions"],
        "state",
    )?;
    let schema_version = number(field(object, "schema_version")?, "schema_version")?;
    if schema_version != SCHEMA_VERSION {
        return Err(invalid_data("unsupported state schema version"));
    }
    let restore_policy = string(field(object, "restore_policy")?, "restore_policy")?;
    if restore_policy != "passive" {
        return Err(invalid_data("restore policy must be passive"));
    }
    let layout = decode_layout(field(object, "layout")?)?;
    let sessions_value = array(field(object, "sessions")?, "sessions")?;
    let sessions = sessions_value
        .iter()
        .map(decode_session)
        .collect::<io::Result<Vec<_>>>()?;
    validate_restored(&layout, &sessions)?;
    Ok(PersistedState {
        schema_version,
        restore_policy: restore_policy.to_owned(),
        layout,
        sessions,
    })
}

fn decode_layout(value: &Value) -> io::Result<PersistedLayout> {
    let object = object(value, "layout")?;
    exact_fields(object, &["kind", "session_ids"], "layout")?;
    let kind = string(field(object, "kind")?, "layout.kind")?;
    if kind != "tabs" {
        return Err(invalid_data("unsupported layout kind"));
    }
    let session_ids = array(field(object, "session_ids")?, "layout.session_ids")?
        .iter()
        .map(|value| string(value, "layout session id").map(str::to_owned))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(PersistedLayout {
        kind: kind.to_owned(),
        session_ids,
    })
}

fn decode_session(value: &Value) -> io::Result<PersistedSession> {
    let object = object(value, "session")?;
    exact_fields(
        object,
        &["id", "status", "last_event", "scrollback"],
        "session",
    )?;
    let id = string(field(object, "id")?, "session.id")?.to_owned();
    let status = string(field(object, "status")?, "session.status")?.to_owned();
    let last_event = match field(object, "last_event")? {
        Value::Null => None,
        value => Some(decode_event(value)?),
    };
    let scrollback = decode_scrollback(field(object, "scrollback")?)?;
    Ok(PersistedSession {
        id,
        status,
        last_event,
        scrollback,
    })
}

fn decode_event(value: &Value) -> io::Result<PersistedEvent> {
    let object = object(value, "last_event")?;
    exact_fields(
        object,
        &["sequence", "sent_unix_micros", "kind"],
        "last_event",
    )?;
    let kind = string(field(object, "kind")?, "last_event.kind")?;
    if !matches!(kind, "status" | "tool" | "output") {
        return Err(invalid_data("unknown persisted event kind"));
    }
    Ok(PersistedEvent {
        sequence: number(field(object, "sequence")?, "last_event.sequence")?,
        sent_unix_micros: number(
            field(object, "sent_unix_micros")?,
            "last_event.sent_unix_micros",
        )?,
        kind: kind.to_owned(),
    })
}

fn decode_scrollback(value: &Value) -> io::Result<PersistedScrollback> {
    let object = object(value, "scrollback")?;
    exact_fields(object, &["tail", "next_offset"], "scrollback")?;
    let tail = array(field(object, "tail")?, "scrollback.tail")?
        .iter()
        .map(|value| string(value, "scrollback line").map(str::to_owned))
        .collect::<io::Result<Vec<_>>>()?;
    if tail.len() > SCROLLBACK_TAIL_LINES {
        return Err(invalid_data("persisted scrollback tail exceeds limit"));
    }
    Ok(PersistedScrollback {
        tail,
        next_offset: number(field(object, "next_offset")?, "scrollback.next_offset")?,
    })
}

fn validate_restored(layout: &PersistedLayout, sessions: &[PersistedSession]) -> io::Result<()> {
    if layout.session_ids.len() != sessions.len() {
        return Err(invalid_data("layout and session counts differ"));
    }
    let mut ids = BTreeSet::new();
    for (layout_id, session) in layout.session_ids.iter().zip(sessions) {
        validate_session_id(layout_id)?;
        if layout_id != &session.id {
            return Err(invalid_data("layout and session order differ"));
        }
        if session.status.is_empty() || !ids.insert(layout_id) {
            return Err(invalid_data("invalid restored session"));
        }
    }
    Ok(())
}

fn exact_fields(
    object: &BTreeMap<String, Value>,
    expected: &[&str],
    context: &str,
) -> io::Result<()> {
    for key in object.keys() {
        if !expected.contains(&key.as_str()) {
            return Err(invalid_data(format!("unknown field {context}.{key}")));
        }
    }
    for key in expected {
        if !object.contains_key(*key) {
            return Err(invalid_data(format!("missing field {context}.{key}")));
        }
    }
    Ok(())
}

fn field<'a>(object: &'a BTreeMap<String, Value>, name: &str) -> io::Result<&'a Value> {
    object
        .get(name)
        .ok_or_else(|| invalid_data(format!("missing field {name}")))
}

fn object<'a>(value: &'a Value, name: &str) -> io::Result<&'a BTreeMap<String, Value>> {
    match value {
        Value::Object(value) => Ok(value),
        _ => Err(invalid_data(format!("{name} must be an object"))),
    }
}

fn array<'a>(value: &'a Value, name: &str) -> io::Result<&'a [Value]> {
    match value {
        Value::Array(value) => Ok(value),
        _ => Err(invalid_data(format!("{name} must be an array"))),
    }
}

fn string<'a>(value: &'a Value, name: &str) -> io::Result<&'a str> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(invalid_data(format!("{name} must be a string"))),
    }
}

fn number(value: &Value, name: &str) -> io::Result<u64> {
    match value {
        Value::Number(value) => Ok(*value),
        _ => Err(invalid_data(format!("{name} must be an unsigned integer"))),
    }
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_data("invalid session id"));
    }
    Ok(())
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid_data("state path needs a UTF-8 file name"))?;
    Ok(parent.join(format!(".{file_name}.tmp-{}", std::process::id())))
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
