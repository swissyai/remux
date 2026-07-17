// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Line protocol for the single supervisor-owned Unix socket.
//!
//! Contract: one newline-delimited message carries either one agent event or one
//! control request. Fields cannot contain tabs/newlines, so framing stays bounded and
//! parsing never invokes another process.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventKind {
    Status,
    Tool,
    Output,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Tool => "tool",
            Self::Output => "output",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "status" => Ok(Self::Status),
            "tool" => Ok(Self::Tool),
            "output" => Ok(Self::Output),
            _ => Err(ProtocolError::new("unknown event kind")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub session_id: String,
    pub sequence: u64,
    pub sent_unix_micros: u64,
    pub kind: EventKind,
    pub payload: String,
}

impl Event {
    pub fn encode(&self) -> Result<String, ProtocolError> {
        validate_atom(&self.session_id, "session id")?;
        validate_atom(&self.payload, "payload")?;
        Ok(format!(
            "event\t{}\t{}\t{}\t{}\t{}\n",
            self.session_id,
            self.sequence,
            self.sent_unix_micros,
            self.kind.as_str(),
            self.payload
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Control {
    Dump,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Event(Event),
    Control(Control),
}

pub fn parse_message(line: &str) -> Result<Message, ProtocolError> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let fields: Vec<&str> = line.split('\t').collect();
    match fields.as_slice() {
        ["control", "dump"] => Ok(Message::Control(Control::Dump)),
        ["event", session_id, sequence, sent, kind, payload] => {
            validate_atom(session_id, "session id")?;
            validate_atom(payload, "payload")?;
            let sequence = sequence
                .parse::<u64>()
                .map_err(|_| ProtocolError::new("invalid event sequence"))?;
            let sent_unix_micros = sent
                .parse::<u64>()
                .map_err(|_| ProtocolError::new("invalid event timestamp"))?;
            Ok(Message::Event(Event {
                session_id: (*session_id).to_owned(),
                sequence,
                sent_unix_micros,
                kind: EventKind::parse(kind)?,
                payload: (*payload).to_owned(),
            }))
        }
        _ => Err(ProtocolError::new("invalid message shape")),
    }
}

pub fn unix_micros_now() -> Result<u64, ProtocolError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProtocolError::new("system clock predates Unix epoch"))?;
    u64::try_from(elapsed.as_micros()).map_err(|_| ProtocolError::new("timestamp overflow"))
}

fn validate_atom(value: &str, name: &str) -> Result<(), ProtocolError> {
    if value.is_empty() || value.contains(['\t', '\r', '\n']) {
        return Err(ProtocolError::new(format!("invalid {name}")));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    message: String,
}

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for ProtocolError {}
