//! Regression coverage for the GUI's ownership of an auto-started daemon.
//!
//! This deliberately avoids GPUI and a display: constructing the backend is
//! the part of GUI startup that owns the child, and dropping that owner is the
//! shutdown boundary we need to verify.

#[cfg(unix)]
#[test]
fn auto_started_child_survives_window_backend_drop_then_stops_on_owner_drop() {
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
    let child = Command::new("sh")
        .args(["-c", "trap 'exit 0' TERM; while :; do sleep 1; done"])
        .spawn()
        .expect("spawn isolated daemon stand-in");

    // `from_parts` models the result of GUI auto-start and performs the same
    // ownership transfer that startup uses, without opening a window.
    let child_pid = child.id();
    assert!(
        process_exists(child_pid),
        "stand-in daemon exited during startup"
    );
    let backend = tau_gui::backend::Backend::from_parts(
        PathBuf::from("/tmp/tau-gui-daemon-lifetime-test.sock"),
        runtime.handle().clone(),
        Some(child),
        "/tmp".into(),
        "test/model".into(),
    );

    // GPUI startup gives the root and view their own Backend handles. Dropping
    // one of those handles must not terminate the process owned by the GUI.
    let window_backend = backend.clone();
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        process_exists(child_pid),
        "owned daemon did not survive startup"
    );
    drop(window_backend);
    assert!(
        process_exists(child_pid),
        "owned daemon was stopped when a window handle was dropped"
    );
    drop(backend);
    assert!(
        !process_exists(child_pid),
        "owned daemon survived Backend::drop"
    );
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}
