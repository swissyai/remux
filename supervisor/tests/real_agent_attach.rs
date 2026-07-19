// Kent Beck desiderata: behavior-sensitive and predictive end-to-end evidence leads; fast, deterministic, isolated, structure-insensitive, specific, readable, writable, and inspiring process checks keep the tracer trustworthy.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use supervisor::attestation::{attestation_path, verify_attestation, LogIntegrity};
use supervisor::scrollback::{read_segments, segment_path};
use supervisor::state::restore_passive;

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-real-{name}-{}", std::process::id()))
}

fn text(path: &Path) -> &str {
    path.to_str().expect("test path is UTF-8")
}

fn supervisor_binary() -> &'static str {
    env!("CARGO_BIN_EXE_remux-supervisor")
}

fn metric_value(metrics: &str, name: &str) -> u64 {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}\t")))
        .expect("metric is present")
        .parse()
        .expect("metric is numeric")
}

fn authorize(log: &Path, token: &str, scope: &str) {
    let output = Command::new(supervisor_binary())
        .args([
            "authorize",
            "--auth-log",
            text(log),
            "--token",
            token,
            "--scope",
            scope,
        ])
        .output()
        .expect("run explicit authorization step");
    assert!(
        output.status.success(),
        "authorization failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn real_shell_events_cross_one_socket_with_measured_zero_event_forks() {
    let root = test_root("end-to-end");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create real-agent fixture root");
    let auth = root.join("attach.log");
    let socket = root.join("socket");
    let state = root.join("state.json");
    let scrollback = root.join("scrollback");
    let attestations = root.join("attestations");
    let metrics = root.join("metrics.tsv");
    let ready = root.join("ready.tsv");
    authorize(&auth, "real-e2e", "launch");
    authorize(&auth, "real-e2e-drive", "drive");

    let mut child = Command::new(supervisor_binary())
        .args([
            "run",
            "--sessions",
            "1",
            "--events-per-session",
            "8",
            "--rate",
            "20",
            "--agent-kind",
            "real-shell",
            "--socket",
            text(&socket),
            "--state",
            text(&state),
            "--scrollback-dir",
            text(&scrollback),
            "--attestation-dir",
            text(&attestations),
            "--metrics",
            text(&metrics),
            "--ready",
            text(&ready),
            "--auth-log",
            text(&auth),
            "--attach-token",
            "real-e2e",
            "--attach-scope",
            "launch",
            "--drive-token",
            "real-e2e-drive",
            "--timeout-seconds",
            "5",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn authorized real-agent supervisor");
    let root_pid = child.id();
    let observed = observe_until_exit(&mut child, root_pid);

    let ready_text = fs::read_to_string(&ready).expect("read startup PID receipt");
    let child_pid = ready_text
        .lines()
        .find_map(|line| line.strip_prefix("child_pids\t"))
        .expect("ready receipt contains child PID")
        .parse::<u32>()
        .expect("child PID is numeric");
    let authorized = BTreeSet::from([root_pid, child_pid]);
    assert_eq!(
        observed, authorized,
        "distinct descendant PID union must contain only supervisor plus authorized shell"
    );

    let metrics_text = fs::read_to_string(&metrics).expect("read supervisor metrics");
    assert!(metrics_text.contains("schema\t3\n"));
    assert!(metrics_text.contains("agent_kind\treal-shell\n"));
    assert!(metrics_text.contains("events_ingested\t8\n"));
    assert!(
        !metrics_text.contains("per_event_forks"),
        "fork count must come from process observation, not a hardcoded subject metric"
    );
    let restored = restore_passive(&state).expect("restore passive in-memory state receipt");
    assert_eq!(restored.sessions[0].scrollback.persisted_through, 8);
    assert_eq!(restored.sessions[0].scrollback.tail.len(), 8);
    assert_eq!(
        read_segments(&segment_path(&scrollback, "session-000"), 8)
            .expect("read appended real-agent output"),
        (0..8)
            .map(|sequence| format!("shell-output-{sequence:03}"))
            .collect::<Vec<_>>()
    );
    let audit = fs::read_to_string(&auth).expect("read attach audit");
    assert!(audit.contains("authorized\tlaunch\treal-e2e"));
    assert!(audit.contains("attached\tlaunch\treal-e2e"));
    assert!(audit.contains("authorized\tdrive\treal-e2e-drive"));
    assert!(audit.contains("driving\tdrive\treal-e2e-drive"));
    let verified = verify_attestation(&attestation_path(&attestations, "session-000"))
        .expect("verify supervisor-owned real-agent attestation");
    assert_eq!(verified.integrity, LogIntegrity::Complete);
    assert_eq!(verified.input_bytes, 8 * 17 + 15);
    assert_eq!(
        verified.output_bytes,
        metric_value(&metrics_text, "pty_bytes")
    );
    fs::remove_dir_all(root).expect("remove real-agent fixture");
}

#[test]
fn relaunch_without_token_is_logged_and_refused_before_agent_spawn() {
    let root = test_root("refused-relaunch");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create relaunch fixture root");
    let auth = root.join("attach.log");
    let ready = root.join("ready.tsv");
    let frame = root.join("frame.ansi");

    let output = Command::new(supervisor_binary())
        .current_dir(&root)
        .args([
            "run",
            "--sessions",
            "1",
            "--events-per-session",
            "1",
            "--rate",
            "100",
            "--ready",
            text(&ready),
            "--auth-log",
            text(&auth),
            "--attach-scope",
            "relaunch",
            "--tui-output",
            text(&frame),
        ])
        .output()
        .expect("request unauthorized relaunch");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("relaunch requires an explicit authorization token"));
    assert!(
        !ready.exists(),
        "refusal must happen before any child is ready"
    );
    assert!(
        !frame.exists(),
        "refusal must happen before the TUI creates output"
    );
    assert_eq!(
        fs::read_to_string(&auth).expect("read refusal audit"),
        "refused\trelaunch\tmissing-token\n"
    );
    assert_eq!(
        fs::read_dir(&root).expect("list relaunch fixture").count(),
        1,
        "authorization log must be the only side effect"
    );
    fs::remove_dir_all(root).expect("remove relaunch fixture");
}

fn observe_until_exit(child: &mut Child, root_pid: u32) -> BTreeSet<u32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = BTreeSet::new();
    loop {
        observed.extend(descendants(&process_snapshot(), root_pid));
        if let Some(status) = child.try_wait().expect("poll supervisor") {
            assert!(status.success(), "real-agent supervisor exited {status}");
            return observed;
        }
        assert!(Instant::now() < deadline, "real-agent supervisor timed out");
        std::thread::yield_now();
    }
}

fn process_snapshot() -> Vec<(u32, u32)> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .expect("sample process tree");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("ps output is UTF-8")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            Some((pid, parent))
        })
        .collect()
}

fn descendants(entries: &[(u32, u32)], root_pid: u32) -> BTreeSet<u32> {
    let mut selected = BTreeSet::from([root_pid]);
    loop {
        let before = selected.len();
        for (pid, parent) in entries {
            if selected.contains(parent) {
                selected.insert(*pid);
            }
        }
        if selected.len() == before {
            return selected;
        }
    }
}
