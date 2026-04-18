mod common;

use assert_cmd::Command;
use common::requires_docker;
use predicates::prelude::*;
use tempfile::TempDir;

fn cmd() -> Command {
    Command::cargo_bin("capsule").unwrap()
}

/// Create a minimal capsule dir with a prompt.md so the binary can pass
/// preflight and reach the iteration loop.
fn make_capsule_dir(prompt: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prompt.md"), prompt).unwrap();
    dir
}

#[test]
#[requires_docker]
fn iterations_prints_headers() {
    let dir = make_capsule_dir("test prompt");
    cmd()
        .args([
            "run",
            "--iterations",
            "3",
            "--capsule-dir",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("── Iteration 1 / 3 ──"))
        .stdout(predicate::str::contains("── Iteration 2 / 3 ──"))
        .stdout(predicate::str::contains("── Iteration 3 / 3 ──"));
}

#[test]
fn help_lists_all_flags() {
    let output = cmd().args(["run", "--help"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();

    assert!(
        stdout.contains("--iterations") || stdout.contains("-i"),
        "missing --iterations"
    );
    assert!(
        stdout.contains("--prompt") || stdout.contains("-p"),
        "missing --prompt"
    );
    assert!(stdout.contains("--capsule-dir"), "missing --capsule-dir");
    assert!(stdout.contains("--rebuild"), "missing --rebuild");
    assert!(
        stdout.contains("--model") || stdout.contains("-m"),
        "missing --model"
    );
    assert!(
        stdout.contains("--verbose") || stdout.contains("-v"),
        "missing --verbose"
    );
    assert!(stdout.contains("--git-identity"), "missing --git-identity");
}

#[test]
fn completion_bash_is_nonempty_and_references_capsule() {
    let output = cmd().args(["completion", "bash"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(!stdout.is_empty(), "bash completion is empty");
    assert!(
        stdout.contains("capsule"),
        "bash completion doesn't reference capsule"
    );
}

#[test]
fn completion_zsh_is_nonempty() {
    let output = cmd().args(["completion", "zsh"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(!stdout.is_empty(), "zsh completion is empty");
}

#[test]
fn completion_fish_is_nonempty() {
    let output = cmd().args(["completion", "fish"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(!stdout.is_empty(), "fish completion is empty");
}

#[test]
fn version_flag_short_prints_version() {
    let output = cmd().arg("-v").assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("capsule"),
        "version output missing binary name"
    );
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "version output missing version"
    );
}

#[test]
fn version_flag_long_prints_version() {
    let output = cmd().arg("--version").assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "version output missing version"
    );
}

#[test]
fn bare_capsule_prints_help() {
    let output = cmd().assert().failure();
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("run"),
        "help should mention 'run' subcommand"
    );
    assert!(
        stderr.contains("completion"),
        "help should mention 'completion' subcommand"
    );
    assert!(
        stderr.contains("update"),
        "help should mention 'update' subcommand"
    );
}
