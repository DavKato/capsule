use assert_cmd::Command;
use capsule::templates;
use std::path::Path;

fn cmd() -> Command {
    Command::cargo_bin("capsule").unwrap()
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/check")
        .join(name)
}

#[test]
fn check_valid_single_iter_passes() {
    cmd()
        .args(["check", "--capsule-dir"])
        .arg(fixture("valid-single-iter"))
        .assert()
        .success();
}

#[test]
fn check_valid_ralph_loop_passes() {
    cmd()
        .args(["check", "--capsule-dir"])
        .arg(fixture("valid-ralph-loop"))
        .assert()
        .success();
}

#[test]
fn check_all_embedded_templates_pass() {
    let entries = templates::list();
    assert!(
        !entries.is_empty(),
        "templates::list() returned no entries — nothing to check"
    );
    for entry in entries {
        let dir = tempfile::tempdir().unwrap();
        templates::copy_to(&entry.name, dir.path()).unwrap();
        let output = cmd()
            .args(["check", "--capsule-dir"])
            .arg(dir.path())
            .assert()
            .success();
        let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
        assert!(
            stdout.is_empty() || !stdout.contains("[ERROR]"),
            "template '{}' has check errors:\n{stdout}",
            entry.name
        );
    }
}

#[test]
fn check_invalid_route_target_fails() {
    let output = cmd()
        .args(["check", "--capsule-dir"])
        .arg(fixture("invalid-route-target"))
        .assert()
        .failure();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("implementors"),
        "expected unknown stage name in output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("implementer"),
        "expected typo suggestion 'implementer' in output, got:\n{stdout}"
    );
}

#[test]
fn check_missing_dockerfile_hints() {
    let output = cmd()
        .args(["check", "--capsule-dir"])
        .arg(fixture("invalid-missing-dockerfile"))
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Dockerfile"),
        "expected 'Dockerfile' hint in output, got:\n{stdout}"
    );
}

#[test]
fn check_missing_prompt_fails() {
    let output = cmd()
        .args(["check", "--capsule-dir"])
        .arg(fixture("invalid-missing-prompt"))
        .assert()
        .failure();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("prompt file not found"),
        "expected 'prompt file not found' in output, got:\n{stdout}"
    );
}
