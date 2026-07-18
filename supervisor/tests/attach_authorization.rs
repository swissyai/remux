// Kent Beck desiderata: behavior-sensitive, specific, and predictive lead; fast, deterministic, isolated, structure-insensitive, readable, writable, and inspiring keep the safety test durable.
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use supervisor::attach::{consume_authorization, record_authorization, AttachScope};

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-auth-{name}-{}.log", std::process::id()))
}

#[test]
fn relaunch_consumes_one_explicit_logged_authorization() {
    let path = test_path("relaunch");
    let _ = fs::remove_file(&path);

    let refused = consume_authorization(&path, AttachScope::Relaunch, None)
        .expect_err("missing token must refuse relaunch");
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);

    record_authorization(&path, AttachScope::Relaunch, "test-token")
        .expect("record explicit authorization");
    let permit = consume_authorization(&path, AttachScope::Relaunch, Some("test-token"))
        .expect("consume explicit authorization");
    assert_eq!(permit.scope(), AttachScope::Relaunch);

    let reused = consume_authorization(&path, AttachScope::Relaunch, Some("test-token"))
        .expect_err("authorization token must be single-use");
    assert_eq!(reused.kind(), std::io::ErrorKind::PermissionDenied);

    let log = fs::read_to_string(&path).expect("read authorization audit log");
    assert!(log.contains("refused\trelaunch\tmissing-token"));
    assert!(log.contains("authorized\trelaunch\ttest-token"));
    assert!(log.contains("attached\trelaunch\ttest-token"));
    fs::remove_file(path).expect("remove authorization log fixture");
}
