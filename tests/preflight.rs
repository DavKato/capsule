mod common;

use capsule::preflight::{check_docker, env_gitignore_warning};
use serial_test::serial;
use std::fs;
use std::path::Path;
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

// ── Test 4: relative capsule_dir, .env gitignored → no warning (regression #27) ─
// Uses set_current_dir; must be serial to avoid races with other tests.
#[test]
#[serial]
fn env_relative_capsule_dir_gitignored_returns_none() {
    let root = TempDir::new().unwrap();
    git_init(&root);
    let capsule_dir = root.path().join(".capsule");
    fs::create_dir(&capsule_dir).unwrap();
    fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();
    fs::write(root.path().join(".gitignore"), ".capsule/.env\n").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    let result = env_gitignore_warning(Path::new(".capsule"));
    std::env::set_current_dir(original).unwrap();

    assert!(result.is_none(), "expected no warning but got: {result:?}");
}

// ── Test 5: relative capsule_dir, .env not gitignored → warning ───────────────
#[test]
#[serial]
fn env_relative_capsule_dir_not_gitignored_returns_warning() {
    let root = TempDir::new().unwrap();
    git_init(&root);
    let capsule_dir = root.path().join(".capsule");
    fs::create_dir(&capsule_dir).unwrap();
    fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();
    // No .gitignore → .env is not ignored.

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    let result = env_gitignore_warning(Path::new(".capsule"));
    std::env::set_current_dir(original).unwrap();

    assert!(result.is_some(), "expected a warning but got None");
}

// ── Test 6 (integration): Docker available → check_docker succeeds ────────────
#[test]
fn docker_available_check_succeeds() {
    if !common::docker_available() { return; }
    check_docker().expect("docker check should succeed when Docker is running");
}
