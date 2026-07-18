// Kent Beck desiderata: predictive and behavior-sensitive safety dominate; fast, deterministic, isolated, structure-insensitive, specific, readable, writable, and inspiring fixtures make failures actionable.
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use supervisor::protocol::{Event, EventKind};
use supervisor::scrollback::{read_segments, segment_path, ScrollbackWriter};
use supervisor::state::{dump_atomic, restore_passive, LiveState};

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-{name}-{}", std::process::id()))
}

fn event(sequence: u64, kind: EventKind, payload: &str) -> Event {
    Event {
        session_id: "session-000".into(),
        sequence,
        sent_unix_micros: 1_000 + sequence,
        kind,
        payload: payload.into(),
    }
}

#[test]
fn passive_state_round_trips_append_only_segments_and_tail_pointers() {
    let root = test_path("scrollback-round-trip");
    let path = root.join("state.json");
    let segments = root.join("scrollback");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create state fixture root");
    let mut live = LiveState::new(["session-000", "session-001"]).expect("valid sessions");
    let writer = ScrollbackWriter::start(&segments, ["session-000", "session-001"])
        .expect("start off-path scrollback writer");

    let first = live
        .apply_batch(&[
            event(0, EventKind::Status, "busy"),
            event(1, EventKind::Output, "compiled target"),
        ])
        .expect("ordered first batch");
    writer.enqueue(first).expect("enqueue first segment");
    let first_offsets = writer.flush().expect("flush first segment");
    live.mark_scrollback_persisted(&first_offsets)
        .expect("record first persisted pointer");
    let segment_file = segment_path(&segments, "session-000");
    let first_bytes = fs::read(&segment_file).expect("read first append receipt");

    let second = live
        .apply_batch(&[event(2, EventKind::Output, "tests passed")])
        .expect("ordered second batch");
    writer.enqueue(second).expect("enqueue second segment");
    let final_offsets = writer.finish().expect("finish scrollback writer");
    live.mark_scrollback_persisted(&final_offsets)
        .expect("record final persisted pointer");
    let final_bytes = fs::read(&segment_file).expect("read final append receipt");
    assert!(
        final_bytes.starts_with(&first_bytes),
        "later segments must append without rewriting prior bytes"
    );

    let expected = live.persisted();
    dump_atomic(&path, &live).expect("atomic state dump");
    let restored = restore_passive(&path).expect("passive restore");

    assert_eq!(restored, expected);
    assert_eq!(restored.layout.session_ids, ["session-000", "session-001"]);
    assert_eq!(
        restored.sessions[0].scrollback.tail,
        ["compiled target", "tests passed"]
    );
    assert_eq!(restored.sessions[0].scrollback.persisted_through, 2);
    assert_eq!(
        restored.sessions[0].scrollback.segments_file,
        "session-000.segments"
    );
    assert_eq!(
        read_segments(
            &segment_file,
            restored.sessions[0].scrollback.persisted_through
        )
        .expect("reconstruct persisted scrollback data"),
        ["compiled target", "tests passed"]
    );
    assert!(
        !root
            .join(format!(".state.json.tmp-{}", std::process::id()))
            .exists(),
        "atomic rename must leave no temporary state file"
    );
    fs::remove_dir_all(root).expect("remove state fixture");
}

#[test]
fn restore_rejects_hostile_attach_fields_without_execution_capability() {
    let root = test_path("reject-command");
    let path = root.join("state.json");
    let sentinel = root.join("must-not-exist");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create hostile fixture root");
    let json = format!(
        "{{\"schema_version\":2,\"restore_policy\":\"passive\",\"layout\":{{\"kind\":\"tabs\",\"session_ids\":[]}},\"sessions\":[],\"relaunch\":{{\"command\":\"touch {}\",\"authorization_token\":\"hostile\"}}}}",
        sentinel.display()
    );
    fs::write(&path, json).expect("write hostile fixture");

    let error = restore_passive(&path).expect_err("attach field must be rejected");

    assert!(error.to_string().contains("unknown field"));
    assert!(
        !sentinel.exists(),
        "restore must never execute persisted text"
    );
    let restore_sources = [
        include_str!("../src/state.rs"),
        include_str!("../src/restore.rs"),
    ];
    for forbidden in [
        "std::process::Command",
        "Command::new",
        ".spawn(",
        ".exec(",
        "spawn_authorized_pty",
        "consume_authorization",
    ] {
        assert!(
            restore_sources
                .iter()
                .all(|source| !source.contains(forbidden)),
            "restore module must not hold process capability: {forbidden}"
        );
    }
    fs::remove_dir_all(root).expect("remove hostile fixture");
}
