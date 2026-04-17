use capsule::config::GitIdentity;
use capsule::git::resolve_git_identity;
use std::collections::HashMap;

fn env_with_git_config(config_path: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("GIT_CONFIG_GLOBAL".to_string(), config_path.to_string());
    // Suppress system-level git config so only our file is consulted.
    env.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
    env
}

#[test]
fn capsule_identity_returns_fixed_name_and_email() {
    let env = HashMap::new();
    let (name, email) = resolve_git_identity(&GitIdentity::Capsule, &env);
    assert_eq!(name, "Capsule");
    assert_eq!(email, "capsule@localhost");
}

#[test]
fn user_identity_reads_from_git_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("gitconfig");
    std::fs::write(
        &config_path,
        "[user]\n\tname = Alice Dev\n\temail = alice@example.com\n",
    )
    .unwrap();

    let env = env_with_git_config(config_path.to_str().unwrap());
    let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

    assert_eq!(name, "Alice Dev");
    assert_eq!(email, "alice@example.com");
}

#[test]
fn user_identity_returns_empty_strings_when_git_config_missing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let nonexistent = dir.path().join("does_not_exist");

    let env = env_with_git_config(nonexistent.to_str().unwrap());
    let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

    assert_eq!(name, "");
    assert_eq!(email, "");
}

#[test]
fn user_identity_returns_empty_when_user_section_absent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("gitconfig");
    std::fs::write(&config_path, "[core]\n\tpager = \n").unwrap();

    let env = env_with_git_config(config_path.to_str().unwrap());
    let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

    assert_eq!(name, "");
    assert_eq!(email, "");
}
