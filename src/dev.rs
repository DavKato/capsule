use std::path::Path;
use std::sync::OnceLock;

static DEV_BUILD: OnceLock<bool> = OnceLock::new();

pub fn is_dev_build() -> bool {
    *DEV_BUILD.get_or_init(|| {
        std::env::args()
            .next()
            .map(|a| a.contains("capsule-dev"))
            .unwrap_or(false)
    })
}

pub fn log_docker_env(
    env_file: Option<&Path>,
    extra_env_file: Option<&Path>,
    container_name: &str,
) {
    if !is_dev_build() {
        return;
    }
    eprintln!("[dev] container: {container_name}");
    dump_file("env_file (.capsule/.env)", env_file);
    dump_file("extra_env_file (--env)", extra_env_file);
}

fn is_secret_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    upper.ends_with("_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_PASSWORD")
}

fn redact_line(line: &str) -> String {
    if let Some((key, _)) = line.split_once('=') {
        if is_secret_key(key) {
            return format!("{key}=[REDACTED]");
        }
    }
    line.to_string()
}

fn dump_file(label: &str, path: Option<&Path>) {
    match path {
        None => eprintln!("[dev] {label}: <none>"),
        Some(p) => match std::fs::read_to_string(p) {
            Ok(content) => {
                eprintln!("[dev] {label} ({}):", p.display());
                for line in content.lines() {
                    eprintln!("[dev]   {}", redact_line(line));
                }
            }
            Err(e) => eprintln!("[dev] {label} ({}): read error: {e}", p.display()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_docker_env_no_files() {
        log_docker_env(None, None, "test-container");
    }

    #[test]
    fn log_docker_env_with_files() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        let extra = dir.path().join("extra.env");
        std::fs::write(&env, "FOO=bar\n").unwrap();
        std::fs::write(&extra, "BAZ=qux\n").unwrap();
        log_docker_env(Some(&env), Some(&extra), "test-container");
    }

    #[test]
    fn log_docker_env_missing_file() {
        let missing = Path::new("/tmp/nonexistent-capsule-test.env");
        log_docker_env(Some(missing), None, "test-container");
    }

    #[test]
    fn redact_line_redacts_secrets_and_keeps_safe_vars() {
        assert_eq!(
            redact_line("ANTHROPIC_API_KEY=sk-ant-xxx"),
            "ANTHROPIC_API_KEY=[REDACTED]"
        );
        assert_eq!(redact_line("MY_TOKEN=abc123"), "MY_TOKEN=[REDACTED]");
        assert_eq!(redact_line("DB_SECRET=hunter2"), "DB_SECRET=[REDACTED]");
        assert_eq!(redact_line("DB_PASSWORD=s3cr3t"), "DB_PASSWORD=[REDACTED]");
        assert_eq!(redact_line("DEBUG=true"), "DEBUG=true");
        assert_eq!(redact_line("FOO=bar"), "FOO=bar");
        assert_eq!(redact_line("NO_EQUALS_HERE"), "NO_EQUALS_HERE");
    }
}
