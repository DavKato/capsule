use assert_cmd::Command;

fn cmd() -> Command {
    Command::cargo_bin("capsule").unwrap()
}

#[test]
fn templates_list_shows_all_templates() {
    let output = cmd().args(["templates", "list"]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("ralph-loop"), "missing ralph-loop template");
    assert!(
        stdout.contains("single-iter"),
        "missing single-iter template"
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
        "missing single-iter description"
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
