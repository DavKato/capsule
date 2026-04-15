use capsule::config::{resolve, CliOverrides, Config, GitIdentity, GithubScope};
use std::collections::HashMap;
use tempfile::TempDir;

fn empty_env() -> HashMap<String, String> {
    HashMap::new()
}

fn no_cli() -> CliOverrides {
    CliOverrides::default()
}

/// Helper: create a temp capsule dir with a config.yml containing the given YAML.
fn capsule_dir_with_config(yaml: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yml"), yaml).unwrap();
    dir
}

// ── Test 1 (tracer bullet): no config.yml, no env, CLI only ────────────────
#[test]
fn no_config_file_uses_defaults_and_cli() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(3),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli, &empty_env()).unwrap();
    assert_eq!(cfg.iterations, 3);
    assert!(!cfg.rebuild);
    assert!(!cfg.verbose);
    assert_eq!(cfg.git_identity, GitIdentity::User);
}

// ── Test 2: config.yml value used when no CLI flag given ───────────────────
#[test]
fn config_file_iterations_used_when_no_cli_flag() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let cfg: Config = resolve(dir.path(), no_cli(), &empty_env()).unwrap();
    assert_eq!(cfg.iterations, 5);
}

// ── Test 3: env var overrides config.yml ───────────────────────────────────
#[test]
fn env_var_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let mut env = empty_env();
    env.insert("CAPSULE_ITERATIONS".into(), "10".into());
    let cfg: Config = resolve(dir.path(), no_cli(), &env).unwrap();
    assert_eq!(cfg.iterations, 10);
}

// ── Test 4: CLI flag overrides env var and config.yml ─────────────────────
#[test]
fn cli_flag_overrides_env_and_config_file() {
    let dir = capsule_dir_with_config("iterations: 5\n");
    let mut env = empty_env();
    env.insert("CAPSULE_ITERATIONS".into(), "10".into());
    let cli = CliOverrides {
        iterations: Some(20),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli, &env).unwrap();
    assert_eq!(cfg.iterations, 20);
}

// ── Test 5: missing config.yml is not an error ────────────────────────────
#[test]
fn missing_config_file_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // No config.yml written — just provide iterations via CLI
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    assert!(resolve(dir.path(), cli, &empty_env()).is_ok());
}

// ── Test 6: malformed YAML is a clear error naming the file ───────────────
#[test]
fn malformed_yaml_produces_clear_error() {
    let dir = capsule_dir_with_config(": this is not valid yaml: {\n");
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let err = resolve(dir.path(), cli, &empty_env()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("config.yml"),
        "error message should name the file; got: {msg}"
    );
}

// ── Test 7: model and verbose from config.yml ─────────────────────────────
#[test]
fn config_file_model_and_verbose() {
    let dir = capsule_dir_with_config("iterations: 1\nmodel: claude-opus-4-6\nverbose: true\n");
    let cfg: Config = resolve(dir.path(), no_cli(), &empty_env()).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("claude-opus-4-6"));
    assert!(cfg.verbose);
}

// ── Test 8: git_identity from config.yml and env var ─────────────────────
#[test]
fn git_identity_capsule_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngit_identity: capsule\n");
    let cfg: Config = resolve(dir.path(), no_cli(), &empty_env()).unwrap();
    assert_eq!(cfg.git_identity, GitIdentity::Capsule);
}

#[test]
fn git_identity_env_var_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngit_identity: capsule\n");
    let mut env = empty_env();
    env.insert("CAPSULE_GIT_IDENTITY".into(), "user".into());
    let cfg: Config = resolve(dir.path(), no_cli(), &env).unwrap();
    assert_eq!(cfg.git_identity, GitIdentity::User);
}

// ── Test 9: model env var ─────────────────────────────────────────────────
#[test]
fn model_env_var_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\nmodel: old-model\n");
    let mut env = empty_env();
    env.insert("CAPSULE_MODEL".into(), "new-model".into());
    let cfg: Config = resolve(dir.path(), no_cli(), &env).unwrap();
    assert_eq!(cfg.model.as_deref(), Some("new-model"));
}

// ── Test 10: rebuild env var ──────────────────────────────────────────────
#[test]
fn rebuild_env_var_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\nrebuild: false\n");
    let mut env = empty_env();
    env.insert("CAPSULE_REBUILD".into(), "true".into());
    let cfg: Config = resolve(dir.path(), no_cli(), &env).unwrap();
    assert!(cfg.rebuild);
}

// ── Tests for github field ────────────────────────────────────────────────

#[test]
fn github_absent_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let cli = CliOverrides {
        iterations: Some(1),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli, &empty_env()).unwrap();
    assert!(cfg.github.is_none());
}

#[test]
fn github_local_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: local\n");
    let cfg: Config = resolve(dir.path(), no_cli(), &empty_env()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}

#[test]
fn github_global_from_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let cfg: Config = resolve(dir.path(), no_cli(), &empty_env()).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Global));
}

#[test]
fn github_env_var_overrides_config_file() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: local\n");
    let mut env = empty_env();
    env.insert("CAPSULE_GITHUB".into(), "global".into());
    let cfg: Config = resolve(dir.path(), no_cli(), &env).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Global));
}

#[test]
fn github_cli_overrides_env_and_config() {
    let dir = capsule_dir_with_config("iterations: 1\ngithub: global\n");
    let mut env = empty_env();
    env.insert("CAPSULE_GITHUB".into(), "global".into());
    let cli = CliOverrides {
        iterations: Some(1),
        github: Some(GithubScope::Local),
        ..Default::default()
    };
    let cfg: Config = resolve(dir.path(), cli, &env).unwrap();
    assert_eq!(cfg.github, Some(GithubScope::Local));
}
