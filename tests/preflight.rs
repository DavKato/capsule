use capsule::preflight::{check_docker, env_gitignore_warning};
use std::fs;
use tempfile::TempDir;

fn git_init(dir: &TempDir) {
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("git init failed");
}

// ── Test 1 (tracer bullet): .env absent → no warning ──────────────────────────
#[test]
fn env_absent_returns_none() {
    let dir = TempDir::new().unwrap();
    assert!(env_gitignore_warning(dir.path()).is_none());
}

// ── Test 2: .env present, not gitignored → warning containing the path ────────
#[test]
fn env_present_not_gitignored_returns_warning_with_path() {
    let dir = TempDir::new().unwrap();
    git_init(&dir);
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    // No .gitignore → .env is not ignored.
    let warning = env_gitignore_warning(dir.path());
    assert!(warning.is_some(), "expected a warning but got None");
    let msg = warning.unwrap();
    assert!(
        msg.contains(".env"),
        "warning should mention the .env path; got: {msg}"
    );
}

// ── Test 3: .env present and gitignored → no warning ─────────────────────────
#[test]
fn env_present_and_gitignored_returns_none() {
    let dir = TempDir::new().unwrap();
    git_init(&dir);
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
    assert!(env_gitignore_warning(dir.path()).is_none());
}

// ── Test 4 (integration): Docker available → check_docker succeeds ────────────
// Requires a running Docker daemon; skipped in environments without Docker.
#[test]
#[ignore]
fn docker_available_check_succeeds() {
    check_docker().expect("docker check should succeed when Docker is running");
}

// ── Test 5 (integration): Docker unavailable → check_docker returns error ─────
// Run manually: stop Docker daemon, then `cargo test docker_unavailable -- --ignored`
#[test]
#[ignore]
fn docker_unavailable_check_returns_error_naming_docker() {
    // This test must be run with Docker stopped.
    let err = check_docker().expect_err("expected an error when Docker is unavailable");
    let msg = err.to_string();
    assert!(
        msg.to_ascii_lowercase().contains("docker"),
        "error should mention Docker; got: {msg}"
    );
}
