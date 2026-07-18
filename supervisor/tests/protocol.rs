// Kent Beck desiderata: specific and structure-insensitive wire behavior leads; fast, deterministic, isolated, behavior-sensitive, readable, writable, predictive, and inspiring examples keep the protocol approachable.
#![forbid(unsafe_code)]

use supervisor::protocol::{parse_message, Control, Event, EventKind, Message};

#[test]
fn event_wire_format_round_trips() {
    let event = Event {
        session_id: "session-019".into(),
        sequence: 7,
        sent_unix_micros: 42,
        kind: EventKind::Tool,
        payload: "cargo-check".into(),
    };

    let encoded = event.encode().expect("encodable event");

    assert_eq!(parse_message(&encoded), Ok(Message::Event(event)));
    assert_eq!(
        parse_message("control\tdump\n"),
        Ok(Message::Control(Control::Dump))
    );
}

#[test]
fn framing_rejects_multiline_payloads() {
    let event = Event {
        session_id: "session-000".into(),
        sequence: 0,
        sent_unix_micros: 1,
        kind: EventKind::Output,
        payload: "line one\nline two".into(),
    };

    assert!(event.encode().is_err());
}
