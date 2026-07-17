// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.

use std::fs;
use std::path::PathBuf;

use supervisor::protocol::{Event, EventKind};
use supervisor::state::{dump_atomic, restore_passive, LiveState};

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-{name}-{}.json", std::process::id()))
}

#[test]
fn passive_state_round_trips_layout_and_scrollback() {
    let path = test_path("round-trip");
    let _ = fs::remove_file(&path);
    let mut live = LiveState::new(["session-000", "session-001"]).expect("valid sessions");
    live.apply_batch(&[
        Event {
            session_id: "session-000".into(),
            sequence: 0,
            sent_unix_micros: 1_000,
            kind: EventKind::Status,
            payload: "busy".into(),
        },
        Event {
            session_id: "session-000".into(),
            sequence: 1,
            sent_unix_micros: 1_001,
            kind: EventKind::Output,
            payload: "compiled target".into(),
        },
    ])
    .expect("ordered events");

    let expected = live.persisted();
    dump_atomic(&path, &live).expect("atomic state dump");
    let restored = restore_passive(&path).expect("passive restore");

    assert_eq!(restored, expected);
    assert_eq!(restored.layout.session_ids, ["session-000", "session-001"]);
    assert_eq!(restored.sessions[0].scrollback.tail, ["compiled target"]);
    fs::remove_file(path).expect("remove state fixture");
}

#[test]
fn restore_rejects_executable_fields_without_running_them() {
    let path = test_path("reject-command");
    let sentinel = test_path("must-not-exist");
    let _ = fs::remove_file(&sentinel);
    let json = format!(
        "{{\"schema_version\":1,\"restore_policy\":\"passive\",\"layout\":{{\"kind\":\"tabs\",\"session_ids\":[]}},\"sessions\":[],\"command\":\"touch {}\"}}",
        sentinel.display()
    );
    fs::write(&path, json).expect("write hostile fixture");

    let error = restore_passive(&path).expect_err("command field must be rejected");

    assert!(error.to_string().contains("unknown field"));
    assert!(
        !sentinel.exists(),
        "restore must never execute persisted text"
    );
    fs::remove_file(path).expect("remove hostile fixture");
}
