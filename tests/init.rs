use assert_cmd::Command;

fn cmd() -> Command {
    Command::cargo_bin("capsule").unwrap()
}

#[test]
fn init_with_template_creates_capsule_dir() {
    let dir = tempfile::tempdir().unwrap();
    cmd()
        .args(["init", "--template", "single-stage"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert!(dir.path().join(".capsule").exists());
    assert!(dir.path().join(".capsule/config.yml").exists());
}

#[test]
fn init_refuses_existing_capsule_without_force() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".capsule")).unwrap();
    let output = cmd()
        .args(["init", "--template", "single-stage"])
        .current_dir(dir.path())
        .assert()
        .failure();
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("already exists") || stderr.contains("--force"),
        "error should mention existing dir or --force: {stderr}"
    );
}

#[test]
fn init_force_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let capsule_dir = dir.path().join(".capsule");
    std::fs::create_dir(&capsule_dir).unwrap();
    std::fs::write(capsule_dir.join("sentinel.txt"), b"old").unwrap();
    cmd()
        .args(["init", "--template", "single-stage", "--force"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert!(
        capsule_dir.join("config.yml").exists(),
        "config.yml missing after --force copy"
    );
    assert!(
        !capsule_dir.join("sentinel.txt").exists(),
        "sentinel should be gone after --force"
    );
}

#[test]
fn init_non_tty_without_template_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    // assert_cmd does not attach a TTY, so stdin.is_terminal() returns false
    let output = cmd().arg("init").current_dir(dir.path()).assert().failure();
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("interactive mode requires a TTY"),
        "expected TTY message: {stderr}"
    );
    assert!(
        stderr.contains("capsule templates list"),
        "expected redirect hint: {stderr}"
    );
    assert!(
        stderr.contains("capsule init --template <name>"),
        "expected template hint: {stderr}"
    );
}
