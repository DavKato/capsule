mod common;

use capsule::preflight::{check_docker, env_gitignore_warning};
use common::requires_docker;
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

#[test]
fn env_absent_returns_none() {
    let dir = TempDir::new().unwrap();
    assert!(env_gitignore_warning(dir.path()).is_none());
}

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

#[test]
fn env_present_and_gitignored_returns_none() {
    let dir = TempDir::new().unwrap();
    git_init(&dir);
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
    assert!(env_gitignore_warning(dir.path()).is_none());
}

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

#[test]
#[serial]
fn env_relative_capsule_dir_not_gitignored_returns_warning() {
    let root = TempDir::new().unwrap();
    git_init(&root);
    let capsule_dir = root.path().join(".capsule");
    fs::create_dir(&capsule_dir).unwrap();
    fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    let result = env_gitignore_warning(Path::new(".capsule"));
    std::env::set_current_dir(original).unwrap();

    assert!(result.is_some(), "expected a warning but got None");
}

#[test]
#[requires_docker]
fn docker_available_check_succeeds() {
    check_docker().expect("docker check should succeed when Docker is running");
}
