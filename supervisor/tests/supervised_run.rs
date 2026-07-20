// Kent Beck desiderata: behavior-sensitive, predictive, and specific route evidence leads; fast, deterministic, isolated, structure-insensitive, readable, writable, and inspiring faults keep the public contract honest.
#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use supervisor::attestation::{verify_attestation, LogIntegrity};
use supervisor::supervised_run::{ATTACH_TOKEN_ENV, ATTESTATION_DIR_ENV, AUTH_LOG_ENV};

fn supervisor() -> &'static str {
    env!("CARGO_BIN_EXE_remux-supervisor")
}

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-public-run-{name}-{}", std::process::id()))
}

fn clean_root(root: &Path) {
    let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
    let _ = fs::remove_dir_all(root);
}

fn authorize(root: &Path, token: &str) {
    let output = Command::new(supervisor())
        .args([
            "authorize",
            "--auth-log",
            root.join("auth.log").to_str().expect("UTF-8 auth path"),
            "--token",
            token,
            "--scope",
            "launch",
        ])
        .output()
        .expect("run authorization command");
    assert!(
        output.status.success(),
        "authorization failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run(root: &Path, cwd: &Path, attestations: &Path, token: &str, command: &[&str]) -> Output {
    let mut invocation = Command::new(supervisor());
    invocation
        .env(AUTH_LOG_ENV, root.join("auth.log"))
        .env(ATTACH_TOKEN_ENV, token)
        .env(ATTESTATION_DIR_ENV, attestations)
        .args([
            "run",
            "--cwd",
            cwd.to_str().expect("UTF-8 cwd"),
            "--attest",
            "--",
        ])
        .args(command)
        .output()
        .expect("run public supervised command")
}

#[test]
fn exact_public_surface_consumes_lifecycle_and_emits_observe_owned_receipt() {
    let root = test_root("success");
    clean_root(&root);
    fs::create_dir_all(root.join("cwd")).expect("create working directory");
    authorize(&root, "public-success");
    let attestations = root.join("attestations");

    let output = run(
        &root,
        &root.join("cwd"),
        &attestations,
        "public-success",
        &[
            "/bin/sh",
            "-c",
            "test -z \"$REMUX_ATTACH_TOKEN\"; printf 'public-payload\\n'; pwd",
        ],
    );
    assert!(
        output.status.success(),
        "public run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("public-payload"));
    assert!(stdout.contains(root.join("cwd").to_str().expect("UTF-8 cwd")));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("remux_run\t1\n"));
    assert!(stderr.contains(&format!(
        "attestation\t{}\n",
        attestations.join("run.attest").display()
    )));
    assert!(stderr.contains("exit\tcode:0\n"));

    let verified = verify_attestation(&attestations.join("run.attest"))
        .expect("verify public-run attestation externally");
    assert_eq!(verified.integrity, LogIntegrity::Complete);
    assert!(verified.records >= 6);
    assert_eq!(verified.output_bytes, output.stdout.len() as u64);
    let log = fs::read_to_string(root.join("auth.log")).expect("read lifecycle log");
    assert_eq!(log.matches("attached\tlaunch\tpublic-success").count(), 1);

    let reused = run(
        &root,
        &root.join("cwd"),
        &root.join("reuse-attestations"),
        "public-success",
        &["/usr/bin/true"],
    );
    assert!(!reused.status.success());
    assert!(String::from_utf8_lossy(&reused.stderr).contains("no unused launch authorization"));
    clean_root(&root);
}

#[test]
fn exact_s1_arm_argv_runs_attested_without_a_provider_or_network() {
    let root = test_root("s1-arm");
    clean_root(&root);
    let cwd = root.join("cell");
    let bin = root.join("bin");
    let session = root.join("session");
    fs::create_dir_all(&cwd).expect("create S1 cell");
    fs::create_dir_all(&bin).expect("create S1 fixture bin");
    fs::create_dir_all(&session).expect("create S1 session fixture");
    let pi = bin.join("pi");
    fs::write(
        &pi,
        "#!/bin/sh\nset -eu\n[ -z \"${NODE_OPTIONS:-}\" ]\n[ \"$CMUX_CLAUDE_HOOKS_DISABLED\" = 1 ]\n[ \"$CMUX_CODEX_HOOKS_DISABLED\" = 1 ]\n[ \"$#\" -eq 13 ]\nprintf 's1-fixture-argv'\nprintf ' <%s>' \"$@\"\nprintf '\\ncwd=%s\\n' \"$PWD\"\n",
    )
    .expect("write no-provider Pi fixture");
    fs::set_permissions(&pi, fs::Permissions::from_mode(0o700))
        .expect("make Pi fixture executable");
    authorize(&root, "s1-token");
    let attestations = root.join("attestations");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = Command::new(supervisor())
        .env(AUTH_LOG_ENV, root.join("auth.log"))
        .env(ATTACH_TOKEN_ENV, "s1-token")
        .env(ATTESTATION_DIR_ENV, &attestations)
        .env("PATH", path)
        .env("NODE_OPTIONS", "hostile-preload")
        .args([
            "run",
            "--cwd",
            cwd.to_str().expect("UTF-8 S1 cwd"),
            "--attest",
            "--",
            "env",
            "-u",
            "NODE_OPTIONS",
            "CMUX_CLAUDE_HOOKS_DISABLED=1",
            "CMUX_CODEX_HOOKS_DISABLED=1",
            "pi",
            "--mode",
            "json",
            "--provider",
            "openai-codex",
            "--model",
            "gpt-5.6-sol",
            "--thinking",
            "low",
            "--approve",
            "--session-dir",
            session.to_str().expect("UTF-8 S1 session path"),
            "--print",
            "PROMPT",
        ])
        .output()
        .expect("run exact S1 arm argv through public route");
    assert!(
        output.status.success(),
        "S1 fixture arm failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("s1-fixture-argv <--mode> <json>"));
    assert!(stdout.contains("<--provider> <openai-codex>"));
    assert!(stdout.contains("<--model> <gpt-5.6-sol>"));
    assert!(stdout.contains("<--thinking> <low> <--approve>"));
    assert!(stdout.contains("<--print> <PROMPT>"));
    assert!(stdout.contains(&format!(
        "cwd={}",
        fs::canonicalize(&cwd).expect("canonical S1 cwd").display()
    )));
    assert_eq!(
        verify_attestation(&attestations.join("run.attest"))
            .expect("verify exact S1 arm chain")
            .integrity,
        LogIntegrity::Complete
    );
    clean_root(&root);
}

#[test]
fn hostile_child_cannot_modify_its_public_run_attestation() {
    let root = test_root("hostile");
    clean_root(&root);
    fs::create_dir_all(root.join("cwd")).expect("create hostile cwd");
    authorize(&root, "public-hostile");
    let attestations = root.join("attestations");
    let path = attestations.join("run.attest");
    let script = format!(
        "chmod 600 '{}'; printf forged >> '{}'; printf safe-output",
        path.display(),
        path.display()
    );

    let output = run(
        &root,
        &root.join("cwd"),
        &attestations,
        "public-hostile",
        &["/bin/sh", "-c", &script],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Operation not permitted"));
    let bytes = fs::read(&path).expect("read hostile public-run chain");
    assert!(!bytes.windows(6).any(|window| window == b"forged"));
    assert_eq!(
        fs::metadata(&path)
            .expect("hostile attestation metadata")
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
    assert_eq!(
        verify_attestation(&path)
            .expect("verify hostile chain")
            .integrity,
        LogIntegrity::Complete
    );
    clean_root(&root);
}

#[test]
fn invalid_cwd_does_not_spend_token_but_exec_failure_does_and_is_attested() {
    let root = test_root("faults");
    clean_root(&root);
    fs::create_dir_all(root.join("cwd")).expect("create fault cwd");
    authorize(&root, "cwd-token");

    let invalid = run(
        &root,
        &root.join("missing"),
        &root.join("invalid-attestations"),
        "cwd-token",
        &["/usr/bin/true"],
    );
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("No such file"));
    let recovered = run(
        &root,
        &root.join("cwd"),
        &root.join("recovered-attestations"),
        "cwd-token",
        &["/usr/bin/true"],
    );
    assert!(recovered.status.success());

    authorize(&root, "spawn-token");
    let failed_attestations = root.join("spawn-attestations");
    let failed = run(
        &root,
        &root.join("cwd"),
        &failed_attestations,
        "spawn-token",
        &["/definitely/not/a/remux-command"],
    );
    assert!(!failed.status.success());
    let verified = verify_attestation(&failed_attestations.join("run.attest"))
        .expect("spawn failure retains complete attempt chain");
    assert_eq!(verified.integrity, LogIntegrity::Complete);
    assert!(verified.records >= 5);
    let reused = run(
        &root,
        &root.join("cwd"),
        &root.join("spawn-reuse-attestations"),
        "spawn-token",
        &["/usr/bin/true"],
    );
    assert!(!reused.status.success());
    assert!(String::from_utf8_lossy(&reused.stderr).contains("no unused launch authorization"));
    clean_root(&root);
}

#[test]
fn child_exit_code_is_propagated_after_chain_finalization() {
    let root = test_root("exit");
    clean_root(&root);
    fs::create_dir_all(root.join("cwd")).expect("create exit cwd");
    authorize(&root, "exit-token");
    let attestations = root.join("attestations");

    let output = run(
        &root,
        &root.join("cwd"),
        &attestations,
        "exit-token",
        &["/bin/sh", "-c", "printf before-exit; exit 23"],
    );
    assert_eq!(output.status.code(), Some(23));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exit\tcode:23"));
    assert_eq!(
        verify_attestation(&attestations.join("run.attest"))
            .expect("verify nonzero chain")
            .integrity,
        LogIntegrity::Complete
    );
    clean_root(&root);
}

#[test]
fn malformed_public_surface_and_missing_token_fail_closed() {
    let root = test_root("parse");
    clean_root(&root);
    fs::create_dir_all(root.join("cwd")).expect("create parser cwd");
    authorize(&root, "parse-token");
    let malformed = Command::new(supervisor())
        .args([
            "run",
            "--cwd",
            root.join("cwd").to_str().expect("UTF-8 cwd"),
            "--attest",
            "/usr/bin/true",
        ])
        .output()
        .expect("run malformed public surface");
    assert!(!malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("COMMAND..."));

    let missing = Command::new(supervisor())
        .env(AUTH_LOG_ENV, root.join("auth.log"))
        .env(ATTESTATION_DIR_ENV, root.join("missing-token-attestations"))
        .env_remove(ATTACH_TOKEN_ENV)
        .args([
            "run",
            "--cwd",
            root.join("cwd").to_str().expect("UTF-8 cwd"),
            "--attest",
            "--",
            "/usr/bin/true",
        ])
        .output()
        .expect("run missing-token public surface");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains(ATTACH_TOKEN_ENV));

    let recovered = run(
        &root,
        &root.join("cwd"),
        &root.join("recovered-attestations"),
        "parse-token",
        &["/usr/bin/true"],
    );
    assert!(recovered.status.success());
    clean_root(&root);
}
