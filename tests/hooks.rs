use capsule::hooks::run_before_all;
use std::fs;
use std::os::unix::fs::PermissionsExt;

// ── Unit tests: run_before_all ────────────────────────────────────────────────

/// No before-all.sh → Ok(()) with no side effects.
#[test]
fn before_all_absent_is_ok() {
    let dir = tempfile::tempdir().expect("temp dir");
    let result = run_before_all(dir.path());
    assert!(
        result.is_ok(),
        "absent before-all.sh must return Ok: {result:?}"
    );
}

/// before-all.sh present and exits 0 → Ok(()).
#[test]
fn before_all_success_is_ok() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("before-all.sh");
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let result = run_before_all(dir.path());
    assert!(
        result.is_ok(),
        "before-all.sh exit 0 must return Ok: {result:?}"
    );
}

/// before-all.sh present and exits non-zero → Err with message.
#[test]
fn before_all_failure_is_err() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("before-all.sh");
    fs::write(&script, "#!/bin/sh\nexit 42\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let result = run_before_all(dir.path());
    assert!(result.is_err(), "before-all.sh exit 42 must return Err");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("42") || msg.contains("before-all"),
        "error must mention exit code or script name, got: {msg}"
    );
}

/// before-all.sh side effects are observable (script writes a file).
#[test]
fn before_all_runs_on_host() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("before-all.sh");
    let sentinel = dir.path().join("ran");
    let sentinel_str = sentinel.to_string_lossy();
    let script_body = format!("#!/bin/sh\ntouch {sentinel_str}\nexit 0\n");
    fs::write(&script, script_body).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    run_before_all(dir.path()).expect("should succeed");
    assert!(
        sentinel.exists(),
        "before-all.sh should have created sentinel file"
    );
}
