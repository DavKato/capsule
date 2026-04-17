use crate::config::GitIdentity;
use std::collections::HashMap;
use std::process::Command;

/// Resolve the git author name and email for the given identity mode.
///
/// `env` is the environment map for the process (pass `&std::env::vars().collect()`
/// in production, or a controlled map in tests). It is forwarded to the `git`
/// subprocess, which honours `GIT_CONFIG_GLOBAL`, `HOME`, etc.
///
/// - `Capsule`: returns fixed `("Capsule", "capsule@localhost")`.
/// - `User`: queries `git config user.name` and `git config user.email` using
///   the provided environment. Returns empty strings for any missing value.
pub fn resolve_git_identity(
    identity: &GitIdentity,
    env: &HashMap<String, String>,
) -> (String, String) {
    match identity {
        GitIdentity::Capsule => ("Capsule".to_string(), "capsule@localhost".to_string()),
        GitIdentity::User => {
            let name = git_config_get("user.name", env);
            let email = git_config_get("user.email", env);
            (name, email)
        }
    }
}

fn git_config_get(key: &str, env: &HashMap<String, String>) -> String {
    Command::new("git")
        .args(["config", key])
        .env_clear()
        .envs(env)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
