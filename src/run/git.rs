use capsule::config::GitIdentity;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn resolve_git_identity(identity: &GitIdentity) -> (String, String) {
    match identity {
        GitIdentity::Capsule => ("Capsule".to_string(), "capsule@localhost".to_string()),
        GitIdentity::User => {
            let home = std::env::var("HOME").unwrap_or_default();
            let gitconfig = PathBuf::from(&home).join(".gitconfig");
            let xdg_config = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(&home).join(".config"))
                .join("git/config");
            let (mut name, mut email) = resolve_user_identity(&gitconfig, &xdg_config);
            if name.is_empty() {
                name = git_config_get("user.name");
            }
            if email.is_empty() {
                email = git_config_get("user.email");
            }
            (name, email)
        }
    }
}

fn resolve_user_identity(gitconfig: &Path, xdg_config: &Path) -> (String, String) {
    let (primary_name, primary_email) = parse_user_identity(gitconfig);
    if !primary_name.is_empty() && !primary_email.is_empty() {
        return (primary_name, primary_email);
    }
    let (xdg_name, xdg_email) = parse_user_identity(xdg_config);
    let name = if primary_name.is_empty() {
        xdg_name
    } else {
        primary_name
    };
    let email = if primary_email.is_empty() {
        xdg_email
    } else {
        primary_email
    };
    (name, email)
}

fn git_config_get(key: &str) -> String {
    Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn parse_user_identity(path: &Path) -> (String, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new()),
    };

    let mut in_user_section = false;
    let mut name = String::new();
    let mut email = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_user_section = trimmed.eq_ignore_ascii_case("[user]");
            continue;
        }
        if !in_user_section {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            match key.trim() {
                "name" => name = value.trim().to_string(),
                "email" => email = value.trim().to_string(),
                _ => {}
            }
        }
    }

    (name, email)
}

#[cfg(test)]
mod tests {
    use super::{git_config_get, parse_user_identity, resolve_git_identity, resolve_user_identity};
    use capsule::config::GitIdentity;

    #[test]
    fn capsule_identity_returns_fixed_name_and_email() {
        let (name, email) = resolve_git_identity(&GitIdentity::Capsule);
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

        let (name, email) = parse_user_identity(&config_path);
        assert_eq!(name, "Alice Dev");
        assert_eq!(email, "alice@example.com");
    }

    #[test]
    fn user_identity_returns_empty_strings_when_git_config_missing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let nonexistent = dir.path().join("does_not_exist");

        let (name, email) = parse_user_identity(&nonexistent);
        assert_eq!(name, "");
        assert_eq!(email, "");
    }

    #[test]
    fn user_identity_returns_empty_when_user_section_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_path = dir.path().join("gitconfig");
        std::fs::write(&config_path, "[core]\n\tpager = \n").unwrap();

        let (name, email) = parse_user_identity(&config_path);
        assert_eq!(name, "");
        assert_eq!(email, "");
    }

    #[test]
    fn xdg_config_used_as_fallback_when_gitconfig_has_no_user_section() {
        let dir = tempfile::tempdir().expect("temp dir");

        let gitconfig = dir.path().join(".gitconfig");
        std::fs::write(&gitconfig, "[core]\n\tpager = \n").unwrap();

        let xdg_git_dir = dir.path().join("xdg/git");
        std::fs::create_dir_all(&xdg_git_dir).unwrap();
        let xdg_config = xdg_git_dir.join("config");
        std::fs::write(
            &xdg_config,
            "[user]\n\tname = XDG User\n\temail = xdg@example.com\n",
        )
        .unwrap();

        let (name, email) = resolve_user_identity(&gitconfig, &xdg_config);
        assert_eq!(name, "XDG User");
        assert_eq!(email, "xdg@example.com");
    }

    #[test]
    fn git_config_get_falls_back_to_subprocess() {
        let name = git_config_get("user.name");
        let email = git_config_get("user.email");
        assert!(
            !name.is_empty(),
            "git config --get user.name should return a value on dev machines"
        );
        assert!(
            !email.is_empty(),
            "git config --get user.email should return a value on dev machines"
        );
    }

    #[test]
    fn identity_fields_resolved_independently_across_configs() {
        let dir = tempfile::tempdir().expect("temp dir");

        let gitconfig = dir.path().join(".gitconfig");
        std::fs::write(&gitconfig, "[user]\n\tname = Primary User\n").unwrap();

        let xdg_git_dir = dir.path().join("xdg/git");
        std::fs::create_dir_all(&xdg_git_dir).unwrap();
        let xdg_config = xdg_git_dir.join("config");
        std::fs::write(&xdg_config, "[user]\n\temail = xdg@example.com\n").unwrap();

        let (name, email) = resolve_user_identity(&gitconfig, &xdg_config);
        assert_eq!(name, "Primary User");
        assert_eq!(email, "xdg@example.com");
    }
}
