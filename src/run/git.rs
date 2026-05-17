use capsule::config::GitIdentity;
use std::path::{Path, PathBuf};

pub(super) fn resolve_git_identity(identity: &GitIdentity) -> (String, String) {
    match identity {
        GitIdentity::Capsule => ("Capsule".to_string(), "capsule@localhost".to_string()),
        GitIdentity::User => {
            let path = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".gitconfig"))
                .unwrap_or_default();
            parse_user_identity(&path)
        }
    }
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
    use super::{parse_user_identity, resolve_git_identity};
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
}
