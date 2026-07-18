// Kent Beck desiderata: behavior-sensitive, predictive, and readable tracer receipts lead; fast, deterministic, isolated, structure-insensitive, specific, writable, and inspiring checks keep render safety visible.
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use supervisor::protocol::{Event, EventKind};
use supervisor::state::{dump_atomic, LiveState};
use supervisor::tui::{TracerRenderer, TracerTabView};

fn event(sequence: u64, kind: EventKind, payload: &str) -> Event {
    Event {
        session_id: "session-000".to_owned(),
        sequence,
        sent_unix_micros: 1_000 + sequence,
        kind,
        payload: payload.to_owned(),
    }
}

#[test]
fn twenty_restored_tabs_render_visibly_detached_in_one_strip() {
    let session_ids = (0..20)
        .map(|index| format!("session-{index:03}"))
        .collect::<Vec<_>>();
    let passive = LiveState::new(session_ids)
        .expect("create passive fleet")
        .persisted();

    let view = TracerTabView::from_passive(&passive).expect("restore tracer tabs");
    let frame = view.render_ansi();

    assert_eq!(view.tab_count(), 20);
    let strip = frame.lines().nth(1).expect("frame has one tab strip");
    assert!(strip.starts_with("TABS "));
    for index in 0..20 {
        assert!(
            strip.contains(&format!("session-{index:03}")),
            "strip misses restored session {index}"
        );
    }
    assert_eq!(strip.matches("DETACHED").count(), 20);
    assert!(!frame.contains("AGENT DRIVING"));
}

#[test]
fn live_batches_show_agent_control_and_keep_a_sanitized_bounded_tail() {
    let passive = LiveState::new(["session-000"])
        .expect("create one passive session")
        .persisted();
    let mut view = TracerTabView::from_passive(&passive).expect("restore detached tab");
    let mut events = vec![event(0, EventKind::Status, "busy\u{001b}]hostile")];
    events.extend(
        (0..12).map(|index| event(index + 1, EventKind::Output, &format!("tail-{index:03}"))),
    );

    view.apply_batch(&events, true)
        .expect("apply live display batch");
    let frame = view.render_ansi();

    assert!(frame.contains("AGENT DRIVING"));
    assert!(frame.contains("busy�]hostile"));
    assert!(!frame.contains("\u{001b}]hostile"));
    for index in 0..4 {
        assert!(!frame.contains(&format!("tail-{index:03}")));
    }
    for index in 4..12 {
        assert!(frame.contains(&format!("tail-{index:03}")));
    }
}

#[test]
fn passive_tui_command_renders_without_launching_sessions() {
    let root = PathBuf::from(format!("/tmp/remux-tui-passive-{}", std::process::id()));
    let state_path = root.join("state.json");
    let output_path = root.join("frame.ansi");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create passive TUI fixture");
    let state = LiveState::new(["session-000"]).expect("create passive state");
    dump_atomic(&state_path, &state).expect("persist passive state");

    let output = Command::new(env!("CARGO_BIN_EXE_remux-supervisor"))
        .args([
            "tui",
            "--state",
            state_path.to_str().expect("state path is UTF-8"),
            "--output",
            output_path.to_str().expect("output path is UTF-8"),
        ])
        .output()
        .expect("run passive TUI command");

    assert!(output.status.success());
    let frame = fs::read_to_string(&output_path).expect("read passive ANSI frame");
    assert!(frame.contains("DETACHED"));
    assert!(!frame.contains("AGENT DRIVING"));
    assert_eq!(
        fs::read_dir(&root).expect("list passive fixture").count(),
        2,
        "passive render may create only its requested output"
    );
    fs::remove_dir_all(root).expect("remove passive TUI fixture");
}

#[test]
fn renderer_writes_only_on_explicit_redraw() {
    let view = TracerTabView::live(["session-000"], true).expect("create live tab");
    let mut renderer = TracerRenderer::new(Vec::new(), view);

    assert_eq!(renderer.frames_rendered(), 0);
    renderer.redraw().expect("flush explicit frame");
    assert_eq!(renderer.frames_rendered(), 1);
    let output = String::from_utf8(renderer.into_inner()).expect("ANSI frame is UTF-8");
    assert!(output.starts_with("\u{001b}[2J\u{001b}[HREMUX / TRACER"));
}

#[test]
fn render_module_has_no_process_attach_or_timer_capability() {
    let source = include_str!("../src/tui.rs");
    for forbidden in [
        "std::process",
        "Command::new",
        ".spawn(",
        "spawn_authorized_pty",
        "consume_authorization",
        "record_authorization",
        concat!("thread::", "sleep"),
        "recv_timeout",
        "poll(",
    ] {
        assert!(
            !source.contains(forbidden),
            "render module holds forbidden capability: {forbidden}"
        );
    }
}
