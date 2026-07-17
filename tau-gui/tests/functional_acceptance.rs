//! Functional acceptance coverage for the GUI's public backend seam.
//!
//! The GUI view is intentionally private, so these tests exercise the same
//! typed boundary used by `TauView`: `Backend` and `tau_client` protocol
//! values.  A real daemon is not started here; the focused failure-path tests
//! keep this suite deterministic and suitable for CI without credentials.

use std::path::PathBuf;
use std::time::Duration;

use tau_gui::backend::{Backend, DaemonAction};

fn backend(socket: PathBuf) -> (Backend, tokio::runtime::Runtime) {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let backend = Backend::from_parts(
        socket,
        runtime.handle().clone(),
        None,
        "/tmp/gui-functional-acceptance".into(),
        "acceptance/model".into(),
    );
    (backend, runtime)
}

#[test]
fn backend_exposes_model_and_working_directory() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-no-daemon.sock"));
    assert_eq!(backend.model(), "acceptance/model");
    assert_eq!(backend.cwd(), "/tmp/gui-functional-acceptance");
    assert!(!backend.auto_started(), "a pre-existing daemon must not warn");
}

#[test]
fn stale_or_missing_daemon_is_reported_through_typed_turn_stream() {
    let (backend, runtime) = backend(PathBuf::from(format!(
        "/tmp/tau-functional-stale-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    )));
    let mut stream = backend.turn_with_options(
        "negotiate stale daemon".into(),
        None,
        Some("Code".into()),
        Some("acceptance/model".into()),
    );
    let result = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), stream.recv())
            .await
            .expect("backend should answer promptly")
            .expect("backend should return an error item")
    });
    assert!(result.is_err(), "unreachable daemon must not fabricate events");
}

#[test]
fn cancel_is_safe_before_turn_started() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-cancel.sock"));
    backend.cancel();
}

#[test]
fn replay_reports_unreachable_daemon_without_panicking() {
    let (backend, runtime) = backend(PathBuf::from("/tmp/tau-functional-replay.sock"));
    let result = runtime.block_on(async { backend.replay("session".into(), 0).await });
    assert!(result.is_err());
}

#[test]
fn ownership_warning_actions_are_public_and_idempotent() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-owner.sock"));
    for action in [
        DaemonAction::Okay,
        DaemonAction::Quit,
        DaemonAction::Disown,
        // Preference-writing actions are covered by the production seam's
        // unit tests; acceptance tests must not alter the user's config.
    ] {
        backend.daemon_action(action);
    }
}
