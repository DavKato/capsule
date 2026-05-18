use assert_cmd::Command;
use capsule::templates;

fn cmd() -> Command {
    Command::cargo_bin("capsule").unwrap()
}

#[test]
fn templates_list_shows_all_templates() {
    let output = cmd().args(["templates", "list"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("ralph-loop"), "missing ralph-loop template");
    assert!(
        stdout.contains("single-stage"),
        "missing single-stage template"
    );
}

#[test]
fn templates_list_shows_descriptions() {
    let output = cmd().args(["templates", "list"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Implement"),
        "missing ralph-loop description"
    );
    assert!(
        stdout.contains("Single Claude invocation"),
        "missing single-stage description"
    );
}

#[test]
fn templates_list_shows_footer() {
    let output = cmd().args(["templates", "list"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("capsule explain pipeline-shapes"),
        "missing footer guidance"
    );
}

#[test]
fn single_stage_prompt_points_to_explain_without_issue_workflow() {
    let dir = tempfile::tempdir().unwrap();
    templates::copy_to("single-stage", dir.path()).unwrap();

    let prompt = std::fs::read_to_string(dir.path().join("prompt.md")).unwrap();

    assert!(
        prompt.contains("capsule explain"),
        "prompt should point users to capsule explain"
    );
    assert!(
        !prompt.contains("GitHub issues are provided"),
        "prompt should not assume GitHub issues are injected"
    );
    assert!(
        !prompt.contains("Working branch"),
        "prompt should not prescribe branch handling"
    );
}
