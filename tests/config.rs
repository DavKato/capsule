use capsule::config::{resolve, CliOverrides, Config, GitIdentity, GithubScope};
use tempfile::TempDir;

fn no_cli() -> CliOverrides {
    CliOverrides::default()
}

/// Helper: create a temp capsule dir with a config.yml containing the given YAML.
fn capsule_dir_with_config(yaml: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yml"), yaml).unwrap();
    dir
}

#[test]
fn no_config_file_uses_defaults_and_cli() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(3),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.iterations, 3);
    assert!(!cfg.rebuild);
    assert!(!cfg.verbose);
    assert_eq!(cfg.git_identity, GitIdentity::User);
}

#[test]
fn config_file_iterations_used_when_no_cli_flag() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.iterations, 5);
}

#[test]
fn cli_flag_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let cli = CliOverrides {
        iterations: Some(20),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.iterations, 20);
}

#[test]
fn missing_config_file_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    assert!(resolve(dir.path(), cli).is_ok());
}

#[test]
fn malformed_yaml_produces_clear_error() {
    let dir = capsule_dir_with_config(": this is not valid yaml: {\n");
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let err = resolve(dir.path(), cli).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("config.yml"),
        "error message should name the file; got: {msg}"
    );
}

#[test]
fn config_file_model_and_verbose() {
    let dir = capsule_dir_with_config("iterations: 1\nmodel: claude-opus-4-6\nverbose: true\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus-4-6"));
    assert!(cfg.verbose);
}

#[test]
fn git_identity_capsule_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngit_identity: capsule\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.git_identity, GitIdentity::Capsule);
}

#[test]
fn github_absent_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert!(cfg.github.is_none());
}

#[test]
fn github_local_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: local\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}

#[test]
fn github_global_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let cfg: Config = resolve(dir.path(), no_cli()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Global));
}

#[test]
fn github_cli_overrides_config() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let cli = CliOverrides {
        iterations: Some(1),
        github: Some(GithubScope::Local),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}
