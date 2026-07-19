// Kent Beck desiderata: behavior-sensitive, predictive, and specific attestation evidence leads; fast, deterministic, isolated, structure-insensitive, readable, writable, and inspiring fixtures keep external verification honest.
#![forbid(unsafe_code)]

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use supervisor::attach::{
    consume_lifecycle_authorization, record_authorization, spawn_authorized_pty, AttachScope,
};
use supervisor::attestation::{
    attestation_path, repair_torn_tail, verify_attestation, AttestationWriter, ExitOutcome,
    LifecyclePhase, LogIntegrity,
};
use supervisor::capability::observe_sessions;

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-attest-{name}-{}", std::process::id()))
}

fn clean_root(root: &Path) {
    let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
    let _ = fs::remove_dir_all(root);
}

fn read_pty(master: &mut fs::File) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => return output,
            Ok(bytes) => output.extend_from_slice(&buffer[..bytes]),
            Err(error) if error.raw_os_error() == Some(5) => return output,
            Err(error) => panic!("read attested PTY: {error}"),
        }
    }
}

#[test]
fn attestation_chain_covers_lifecycle_bytes_spawn_timing_and_exit() {
    let root = test_root("complete");
    clean_root(&root);
    let logs = root.join("logs");
    let observe = observe_sessions(["session-000"]).expect("mint exact observe proof");
    let writer = AttestationWriter::start(&logs, ["session-000"], &observe)
        .expect("start protected attestation writer");
    let observer = writer.observer().expect("mint attestation observe route");

    observer
        .lifecycle("session-000", LifecyclePhase::Created)
        .expect("record created");
    observer
        .spawn("session-000", 42)
        .expect("record observed spawn");
    observer
        .lifecycle("session-000", LifecyclePhase::Running)
        .expect("record running");
    observer
        .input("session-000", b"abc")
        .expect("record observed input");
    observer
        .output("session-000", b"first")
        .expect("record first observed output");
    observer
        .output("session-000", b"-second")
        .expect("record rolling observed output");
    observer
        .exit("session-000", ExitOutcome::Code(0))
        .expect("record exit");
    observer
        .lifecycle("session-000", LifecyclePhase::Ended)
        .expect("record ended");
    drop(observer);
    let summary = writer.finish().expect("finish synchronized attestation");

    let path = attestation_path(&logs, "session-000");
    let verified = verify_attestation(&path).expect("externally verify attestation");
    let session = &summary["session-000"];
    assert_eq!(verified.integrity, LogIntegrity::Complete);
    assert_eq!(verified.records, 8);
    assert_eq!(verified.input_bytes, 3);
    assert_eq!(verified.output_bytes, 12);
    assert_eq!(verified.head, session.head);
    assert_eq!(verified.head_hex().len(), 64);
    assert_eq!(
        fs::metadata(&path)
            .expect("attestation metadata")
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
    let bytes = fs::read(&path).expect("read canonical attestation frames");
    assert!(bytes.windows(64).any(
        |window| window == b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    ));
    clean_root(&root);
}

#[test]
fn torn_final_attestation_frame_is_detected_and_only_that_suffix_is_repaired() {
    let root = test_root("torn");
    clean_root(&root);
    let logs = root.join("logs");
    let observe = observe_sessions(["session-000"]).expect("mint observe proof");
    let writer = AttestationWriter::start(&logs, ["session-000"], &observe)
        .expect("start attestation writer");
    let observer = writer.observer().expect("mint observer");
    observer
        .lifecycle("session-000", LifecyclePhase::Created)
        .expect("record complete frame");
    observer
        .output("session-000", b"torn-tail")
        .expect("record frame to tear");
    drop(observer);
    writer.finish().expect("finish complete source log");
    let path = attestation_path(&logs, "session-000");
    let complete = fs::read(&path).expect("read complete source log");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("open fault seam");
    fs::write(&path, &complete[..complete.len() - 2]).expect("inject torn commit marker");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("reprotect torn log");

    let torn = verify_attestation(&path).expect("verify valid prefix plus torn tail");
    assert_eq!(torn.integrity, LogIntegrity::TornTail);
    assert_eq!(torn.records, 1);
    let repaired = repair_torn_tail(&path).expect("discard only incomplete final frame");
    assert_eq!(repaired.integrity, LogIntegrity::Complete);
    assert_eq!(repaired.records, 1);
    assert_eq!(repaired.head, torn.head);
    assert!(fs::metadata(&path).expect("repaired metadata").len() < complete.len() as u64);
    clean_root(&root);
}

#[test]
fn committed_attestation_mutation_fails_hash_verification() {
    let root = test_root("tamper");
    clean_root(&root);
    let logs = root.join("logs");
    let observe = observe_sessions(["session-000"]).expect("mint observe proof");
    let writer = AttestationWriter::start(&logs, ["session-000"], &observe)
        .expect("start attestation writer");
    let observer = writer.observer().expect("mint observer");
    observer
        .lifecycle("session-000", LifecyclePhase::Running)
        .expect("record frame");
    drop(observer);
    writer.finish().expect("finish source log");
    let path = attestation_path(&logs, "session-000");
    let mut bytes = fs::read(&path).expect("read source frame");
    let index = bytes
        .windows(b"running".len())
        .position(|window| window == b"running")
        .expect("find canonical lifecycle value");
    bytes[index] = b'R';
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("open tamper seam");
    fs::write(&path, bytes).expect("inject committed mutation");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("reprotect mutation");

    let error = verify_attestation(&path).expect_err("mutated committed frame must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("hash differs"));
    clean_root(&root);
}

#[test]
fn agent_inside_authorized_pty_cannot_append_its_own_attestation() {
    let root = test_root("agent-write");
    clean_root(&root);
    fs::create_dir(&root).expect("create agent-write root");
    let logs = root.join("logs");
    let auth = root.join("auth.log");
    let observe = observe_sessions(["session-000"]).expect("mint observe proof");
    let writer = AttestationWriter::start(&logs, ["session-000"], &observe)
        .expect("start protected attestation writer");
    let observer = writer.observer().expect("mint observer");
    let path = attestation_path(&logs, "session-000");
    record_authorization(&auth, AttachScope::Launch, "agent-write")
        .expect("record lifecycle grant");
    let lifecycle = consume_lifecycle_authorization(
        &auth,
        AttachScope::Launch,
        Some("agent-write"),
        ["session-000"],
    )
    .expect("consume lifecycle grant");

    observer
        .lifecycle("session-000", LifecyclePhase::Created)
        .expect("record created");
    let mut command = Command::new("/bin/sh");
    command.env("REMUX_ATTESTATION_PATH", &path).args([
        "-c",
        "printf forged >> \"$REMUX_ATTESTATION_PATH\"; printf observed-output",
    ]);
    let (mut child, mut master) = spawn_authorized_pty(&lifecycle, "session-000", &mut command)
        .expect("spawn hostile agent only through lifecycle route");
    observer
        .spawn("session-000", child.id())
        .expect("record spawn");
    observer
        .lifecycle("session-000", LifecyclePhase::Running)
        .expect("record running");
    let output = read_pty(&mut master);
    observer
        .output("session-000", &output)
        .expect("record supervisor-observed PTY bytes");
    let status = child.wait().expect("wait hostile agent");
    let outcome = status
        .code()
        .map(ExitOutcome::Code)
        .or_else(|| status.signal().map(ExitOutcome::Signal))
        .expect("child has code or signal");
    observer
        .exit("session-000", outcome)
        .expect("record observed exit");
    observer
        .lifecycle("session-000", LifecyclePhase::Ended)
        .expect("record ended");
    drop(observer);
    let summary = writer.finish().expect("finish protected attestation");

    assert!(status.success());
    assert!(String::from_utf8_lossy(&output).contains("Permission denied"));
    let file = fs::read(&path).expect("read protected chain");
    assert!(!file
        .windows(b"forged".len())
        .any(|window| window == b"forged"));
    let verified = verify_attestation(&path).expect("agent attempt left valid chain");
    assert_eq!(verified.head, summary["session-000"].head);
    assert_eq!(verified.output_bytes, output.len() as u64);
    assert!(OpenOptions::new().append(true).open(&path).is_err());
    let mut append = OpenOptions::new();
    append.append(true);
    assert!(append.open(&path).is_err());
    clean_root(&root);
}
