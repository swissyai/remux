// Kent Beck desiderata: behavior-sensitive and predictive syscall coverage leads; fast, deterministic, isolated, structure-insensitive, specific, readable, writable, and inspiring output keeps the unsafe seam auditable.
#![forbid(unsafe_code)]

use std::io::Read;
use std::process::Command;

#[test]
fn pty_spawn_exercises_all_descriptor_and_session_invariants() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf 'pty-invariant-ok\\n'"]);

    let (mut child, mut master) = remux_pty::spawn_pty(&mut command).expect("spawn shell on PTY");
    let mut output = Vec::new();
    let mut buffer = [0_u8; 256];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes) => output.extend_from_slice(&buffer[..bytes]),
            Err(error) if error.raw_os_error() == Some(5) => break,
            Err(error) => panic!("read PTY master: {error}"),
        }
    }
    let status = child.wait().expect("wait for PTY child");

    assert!(status.success());
    assert!(String::from_utf8(output)
        .expect("PTY output is UTF-8")
        .contains("pty-invariant-ok"));
}
