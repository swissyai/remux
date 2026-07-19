// Kent Beck desiderata: behavior-sensitive, specific, and predictive lead; fast, deterministic, isolated, structure-insensitive, readable, writable, and inspiring keep the safety test durable.
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use supervisor::attach::{
    consume_drive_authorization, consume_lifecycle_authorization, record_authorization,
    record_drive_authorization, AttachScope,
};
use supervisor::capability::{CapabilityTier, DrivePresence, ROUTE_DECLARATIONS};

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("remux-auth-{name}-{}", std::process::id()))
}

#[test]
fn drive_and_lifecycle_each_consume_one_explicit_logged_authorization() {
    let root = test_root("exact-tiers");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create authorization fixture");
    let path = root.join("capabilities.log");

    let refused =
        consume_lifecycle_authorization(&path, AttachScope::Relaunch, None, ["session-000"])
            .expect_err("missing token must refuse relaunch");
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);

    record_authorization(&path, AttachScope::Relaunch, "lifecycle-token")
        .expect("record explicit lifecycle authorization");
    let lifecycle = consume_lifecycle_authorization(
        &path,
        AttachScope::Relaunch,
        Some("lifecycle-token"),
        ["session-000"],
    )
    .expect("consume explicit lifecycle authorization");
    assert_eq!(
        lifecycle.action(),
        supervisor::capability::LifecycleAction::Relaunch
    );
    let reused = consume_lifecycle_authorization(
        &path,
        AttachScope::Relaunch,
        Some("lifecycle-token"),
        ["session-000"],
    )
    .expect_err("lifecycle authorization token must be single-use");
    assert_eq!(reused.kind(), std::io::ErrorKind::PermissionDenied);

    record_drive_authorization(&path, "drive-token").expect("record explicit drive authorization");
    let drive = consume_drive_authorization(&path, Some("drive-token"), ["session-000"])
        .expect("consume explicit drive authorization");
    assert!(drive.presence().is_driven("session-000"));
    let reused = consume_drive_authorization(&path, Some("drive-token"), ["session-000"])
        .expect_err("drive authorization token must be single-use");
    assert_eq!(reused.kind(), std::io::ErrorKind::PermissionDenied);

    let log = fs::read_to_string(&path).expect("read capability authorization audit log");
    assert!(log.contains("refused\trelaunch\tmissing-token"));
    assert!(log.contains("authorized\trelaunch\tlifecycle-token"));
    assert!(log.contains("attached\trelaunch\tlifecycle-token"));
    assert!(log.contains("authorized\tdrive\tdrive-token"));
    assert!(log.contains("driving\tdrive\tdrive-token"));
    fs::remove_dir_all(root).expect("remove authorization fixture");
}

#[test]
fn concurrent_consumers_cannot_both_claim_one_drive_token() {
    let root = test_root("concurrent-claim");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).expect("create concurrent authorization fixture");
    let path = root.join("capabilities.log");
    record_drive_authorization(&path, "one-use").expect("record one drive grant");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            consume_drive_authorization(&path, Some("one-use"), ["session-000"])
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("authorization worker did not panic"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let log = fs::read_to_string(&path).expect("read concurrent authorization log");
    assert_eq!(log.matches("driving\tdrive\tone-use").count(), 1);
    assert_eq!(log.matches("refused\tdrive\tone-use").count(), 1);
    fs::remove_dir_all(root).expect("remove concurrent authorization fixture");
}

#[test]
fn route_inventory_declares_one_exact_tier_and_no_implicit_broadening() {
    let names = ROUTE_DECLARATIONS
        .iter()
        .map(|route| route.name)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names.len(), ROUTE_DECLARATIONS.len());
    assert_eq!(
        ROUTE_DECLARATIONS
            .iter()
            .filter(|route| route.tier == CapabilityTier::Observe)
            .count(),
        2
    );
    assert_eq!(
        ROUTE_DECLARATIONS
            .iter()
            .filter(|route| route.tier == CapabilityTier::Drive)
            .count(),
        2
    );
    assert_eq!(
        ROUTE_DECLARATIONS
            .iter()
            .filter(|route| route.tier == CapabilityTier::Lifecycle)
            .count(),
        1
    );
    assert!(
        !DrivePresence::none().is_driven("session-000"),
        "positive drive state must require a drive proof"
    );
}
