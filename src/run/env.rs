use anyhow::{bail, Context, Result};
use capsule::config::{Config, GithubScope};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn parse_dotenv(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw_val = line[eq_pos + 1..].trim();
            let value = strip_quotes(raw_val).to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

fn strip_quotes(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Loads `.env` from capsule_dir into process env. Process env takes precedence (no overwrite).
pub(super) fn load_dotenv(capsule_dir: &Path) -> Result<()> {
    let path = capsule_dir.join(".env");
    if !path.exists() {
        return Ok(());
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for (key, value) in parse_dotenv(&content) {
        if std::env::var(&key).is_err() {
            // SAFETY: called before any threads are spawned (ctrlc handler registered later in main).
            unsafe { std::env::set_var(&key, &value) };
        }
    }
    Ok(())
}

/// Resolve GH_TOKEN when --github is set and write it to a temp env-file so
/// the token never appears in `docker run` args.
pub(super) fn setup_gh_token(
    cfg: &Config,
    pre_dotenv_env: &HashMap<String, String>,
    dotenv_map: &HashMap<String, String>,
) -> Result<Option<tempfile::NamedTempFile>> {
    let scope = match &cfg.github {
        None => return Ok(None),
        Some(s) => s,
    };

    let token = resolve_gh_token(scope, pre_dotenv_env, dotenv_map)?;

    match scope {
        GithubScope::Local => {
            capsule::display::info("GH_TOKEN: local (.capsule/.env)");
        }
        GithubScope::Global => {
            if pre_dotenv_env.contains_key("GH_TOKEN") {
                capsule::display::info("GH_TOKEN: global (process environment)");
            } else {
                capsule::display::warning(
                    "GH_TOKEN not found in process environment — falling back to gh auth token",
                );
                let _ = Command::new("gh").args(["auth", "status"]).status();
                eprint!("Inject into container? [y/N] ");
                let _ = std::io::stderr().flush();
                let mut answer = String::new();
                std::io::stdin()
                    .read_line(&mut answer)
                    .context("failed to read confirmation")?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    anyhow::bail!(
                        "Aborted. To avoid this prompt use 'local' mode: \
                         add GH_TOKEN to .capsule/.env and pass --github local"
                    );
                }
            }
        }
    }

    let mut tmp = tempfile::Builder::new()
        .prefix("capsule-gh-token-")
        .suffix(".env")
        .tempfile()
        .context("failed to create GH_TOKEN temp file")?;
    writeln!(tmp, "GH_TOKEN={token}").context("failed to write GH_TOKEN temp file")?;
    Ok(Some(tmp))
}

/// Resolves GH_TOKEN: local reads from dotenv only; global checks process env then `gh auth token`.
fn resolve_gh_token(
    scope: &GithubScope,
    pre_dotenv_env: &HashMap<String, String>,
    dotenv_map: &HashMap<String, String>,
) -> Result<String> {
    match scope {
        GithubScope::Local => dotenv_map
            .get("GH_TOKEN")
            .filter(|t| !t.is_empty())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "github is set to 'local' but GH_TOKEN not found in .capsule/.env \
                     — add GH_TOKEN=<token> to .capsule/.env"
                )
            }),
        GithubScope::Global => {
            if let Some(token) = pre_dotenv_env.get("GH_TOKEN").filter(|t| !t.is_empty()) {
                return Ok(token.clone());
            }
            let output = Command::new("gh").args(["auth", "token"]).output();
            if let Ok(out) = output {
                if out.status.success() {
                    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !token.is_empty() {
                        return Ok(token);
                    }
                }
            }
            bail!(
                "github is set to 'global' but GH_TOKEN not found in process environment \
                 — in CI, ensure GH_TOKEN is set by your platform \
                 — locally, consider using 'local' instead: add GH_TOKEN to .capsule/.env"
            )
        }
    }
}

/// Write `--env` pairs to a `NamedTempFile` in dotenv format.
/// Returns `None` when `pairs` is empty so no temp file is created.
pub(super) fn build_extra_env_tempfile(
    pairs: &[(String, String)],
) -> Result<Option<tempfile::NamedTempFile>> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let mut tmp = tempfile::Builder::new()
        .prefix("capsule-env-")
        .suffix(".env")
        .tempfile()
        .context("failed to create --env temp file")?;
    for (k, v) in pairs {
        writeln!(tmp, "{k}={v}").context("failed to write --env temp file")?;
    }
    Ok(Some(tmp))
}

/// Merge persisted run-environment pairs with CLI overrides. Persisted pairs
/// form the base; any same-key CLI pair overwrites; new CLI keys are appended.
pub(super) fn merge_env(
    persisted: &[(String, String)],
    cli_overrides: &[(String, String)],
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = persisted.to_vec();
    for (k, v) in cli_overrides {
        if let Some(existing) = result.iter_mut().find(|(ek, _)| ek == k) {
            existing.1 = v.clone();
        } else {
            result.push((k.clone(), v.clone()));
        }
    }
    result
}

pub(super) fn token_lifetime_warning(
    remaining_minutes: Option<u64>,
    threshold_minutes: Option<u32>,
) -> Option<String> {
    let threshold = threshold_minutes?;
    let remaining = remaining_minutes?;
    if remaining >= threshold as u64 {
        return None;
    }
    Some(format!(
        "Warning: OAuth token expires in {remaining} minutes (threshold: {threshold} min).\n\
         Run `claude auth login` to refresh before starting."
    ))
}

pub(super) fn env_gitignore_warning(capsule_dir: &Path) -> Option<String> {
    let env_path = capsule_dir.join(".env");
    if !env_path.exists() {
        return None;
    }

    // Canonicalize env_path so git resolves it correctly regardless of current_dir.
    // Without this, if capsule_dir is relative (e.g. ".capsule"), git would interpret
    // ".capsule/.env" relative to its new CWD and look for ".capsule/.capsule/.env".
    let abs_env_path = env_path.canonicalize().unwrap_or_else(|_| env_path.clone());
    let result = Command::new("git")
        .args(["check-ignore", "-q"])
        .arg(&abs_env_path)
        .current_dir(capsule_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match result {
        Ok(s) if s.success() => None,
        _ => Some(format!(
            "warning: {} is not gitignored — add it to .gitignore to avoid committing secrets",
            env_path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_extra_env_tempfile, env_gitignore_warning, load_dotenv, merge_env, parse_dotenv,
        resolve_gh_token, strip_quotes, token_lifetime_warning,
    };
    use capsule::config::GithubScope;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_capsule_dir(env_content: Option<&str>) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(content) = env_content {
            std::fs::write(dir.path().join(".env"), content).unwrap();
        }
        dir
    }

    #[test]
    fn parse_dotenv_basic_key_value() {
        let env = parse_dotenv("FOO=bar\nBAZ=qux\n");
        assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
        assert_eq!(env.get("BAZ").map(|s| s.as_str()), Some("qux"));
    }

    #[test]
    fn parse_dotenv_empty_content_is_empty_map() {
        let env = parse_dotenv("");
        assert!(env.is_empty());
    }

    #[test]
    fn parse_dotenv_ignores_comments_and_blank_lines() {
        let content = "# this is a comment\n\nFOO=hello\n\n# another comment\nBAR=world\n";
        let env = parse_dotenv(content);
        assert_eq!(env.len(), 2);
        assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("hello"));
        assert_eq!(env.get("BAR").map(|s| s.as_str()), Some("world"));
    }

    #[test]
    fn parse_dotenv_strips_double_quotes() {
        let env = parse_dotenv("SECRET=\"my secret value\"\n");
        assert_eq!(
            env.get("SECRET").map(|s| s.as_str()),
            Some("my secret value")
        );
    }

    #[test]
    fn parse_dotenv_strips_single_quotes() {
        let env = parse_dotenv("TOKEN='abc123'\n");
        assert_eq!(env.get("TOKEN").map(|s| s.as_str()), Some("abc123"));
    }

    #[test]
    fn parse_dotenv_value_with_equals() {
        let env = parse_dotenv("URL=https://example.com/path?a=1&b=2\n");
        assert_eq!(
            env.get("URL").map(|s| s.as_str()),
            Some("https://example.com/path?a=1&b=2")
        );
    }

    #[test]
    fn strip_quotes_no_quotes_unchanged() {
        assert_eq!(strip_quotes("hello"), "hello");
    }

    #[test]
    fn strip_quotes_double_quotes_stripped() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
    }

    #[test]
    fn strip_quotes_single_quotes_stripped() {
        assert_eq!(strip_quotes("'hello'"), "hello");
    }

    #[test]
    fn load_dotenv_absent_file_is_ok() {
        let dir = make_capsule_dir(None);
        assert!(load_dotenv(dir.path()).is_ok());
    }

    #[test]
    fn resolve_gh_token_local_reads_from_dotenv_map() {
        let pre_env: HashMap<String, String> = HashMap::new();
        let mut dotenv: HashMap<String, String> = HashMap::new();
        dotenv.insert("GH_TOKEN".to_string(), "ghs_localtoken".to_string());
        let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
        assert_eq!(token, "ghs_localtoken");
    }

    #[test]
    fn resolve_gh_token_local_ignores_process_env() {
        let mut pre_env: HashMap<String, String> = HashMap::new();
        pre_env.insert("GH_TOKEN".to_string(), "ghs_processtoken".to_string());
        let mut dotenv: HashMap<String, String> = HashMap::new();
        dotenv.insert("GH_TOKEN".to_string(), "ghs_dotenvtoken".to_string());
        let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
        assert_eq!(token, "ghs_dotenvtoken");
    }

    #[test]
    fn resolve_gh_token_local_missing_returns_error() {
        let pre_env: HashMap<String, String> = HashMap::new();
        let dotenv: HashMap<String, String> = HashMap::new();
        let result = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("local"), "error should mention 'local': {msg}");
        assert!(
            msg.contains(".capsule/.env"),
            "error should name the file: {msg}"
        );
    }

    #[test]
    fn resolve_gh_token_global_reads_from_pre_dotenv_env() {
        let mut pre_env: HashMap<String, String> = HashMap::new();
        pre_env.insert("GH_TOKEN".to_string(), "ghs_globaltoken".to_string());
        let dotenv: HashMap<String, String> = HashMap::new();
        let token = resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv).unwrap();
        assert_eq!(token, "ghs_globaltoken");
    }

    #[test]
    fn resolve_gh_token_global_missing_returns_error_or_token() {
        let pre_env: HashMap<String, String> = HashMap::new();
        let dotenv: HashMap<String, String> = HashMap::new();
        match resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv) {
            Ok(_token) => {}
            Err(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("global"),
                    "error should mention 'global': {msg}"
                );
            }
        }
    }

    #[test]
    fn token_warning_when_below_threshold() {
        let msg = token_lifetime_warning(Some(10), Some(15));
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(text.contains("10"), "msg: {text}");
        assert!(text.contains("15"), "msg: {text}");
    }

    #[test]
    fn no_token_warning_when_above_threshold() {
        assert!(token_lifetime_warning(Some(30), Some(15)).is_none());
    }

    #[test]
    fn no_token_warning_when_no_threshold() {
        assert!(token_lifetime_warning(Some(10), None).is_none());
    }

    #[test]
    fn no_token_warning_when_no_credentials() {
        assert!(token_lifetime_warning(None, Some(15)).is_none());
    }

    #[test]
    fn token_warning_when_expired() {
        let msg = token_lifetime_warning(Some(0), Some(15));
        assert!(msg.is_some());
    }

    #[test]
    fn extra_env_tempfile_empty_pairs_returns_none() {
        let result = build_extra_env_tempfile(&[]).unwrap();
        assert!(result.is_none(), "empty pairs must produce no tempfile");
    }

    #[test]
    fn extra_env_tempfile_content_is_dotenv_format() {
        let pairs = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "qux".to_string()),
        ];
        let tmp = build_extra_env_tempfile(&pairs).unwrap().unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "FOO=bar\nBAZ=qux\n");
    }

    #[test]
    fn extra_env_tempfile_single_pair() {
        let pairs = vec![("KEY".to_string(), "value".to_string())];
        let tmp = build_extra_env_tempfile(&pairs).unwrap().unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "KEY=value\n");
    }

    #[test]
    fn merge_env_both_empty_returns_empty() {
        assert_eq!(merge_env(&[], &[]), vec![]);
    }

    #[test]
    fn merge_env_no_persisted_returns_cli_pairs() {
        let cli = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        assert_eq!(merge_env(&[], &cli), cli);
    }

    #[test]
    fn merge_env_no_cli_returns_persisted_unchanged() {
        let persisted = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        assert_eq!(merge_env(&persisted, &[]), persisted);
    }

    #[test]
    fn merge_env_cli_overrides_persisted_per_key() {
        let persisted = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        let cli = vec![
            ("B".to_string(), "3".to_string()),
            ("C".to_string(), "4".to_string()),
        ];
        let merged = merge_env(&persisted, &cli);
        assert_eq!(
            merged,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "3".to_string()),
                ("C".to_string(), "4".to_string()),
            ]
        );
    }

    // ── env_gitignore_warning tests ───────────────────────────────────────────

    fn git_init(dir: &TempDir) {
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .expect("git init failed");
    }

    #[test]
    fn env_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(env_gitignore_warning(dir.path()).is_none());
    }

    #[test]
    fn env_present_not_gitignored_returns_warning_with_path() {
        let dir = TempDir::new().unwrap();
        git_init(&dir);
        std::fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
        let warning = env_gitignore_warning(dir.path());
        assert!(warning.is_some(), "expected a warning but got None");
        let msg = warning.unwrap();
        assert!(
            msg.contains(".env"),
            "warning should mention the .env path; got: {msg}"
        );
    }

    #[test]
    fn env_present_and_gitignored_returns_none() {
        let dir = TempDir::new().unwrap();
        git_init(&dir);
        std::fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
        std::fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        assert!(env_gitignore_warning(dir.path()).is_none());
    }

    #[test]
    #[serial_test::serial]
    fn env_relative_capsule_dir_gitignored_returns_none() {
        let root = TempDir::new().unwrap();
        git_init(&root);
        let capsule_dir = root.path().join(".capsule");
        std::fs::create_dir(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();
        std::fs::write(root.path().join(".gitignore"), ".capsule/.env\n").unwrap();

        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.path()).unwrap();
        let result = env_gitignore_warning(std::path::Path::new(".capsule"));
        std::env::set_current_dir(original).unwrap();

        assert!(result.is_none(), "expected no warning but got: {result:?}");
    }

    #[test]
    #[serial_test::serial]
    fn env_relative_capsule_dir_not_gitignored_returns_warning() {
        let root = TempDir::new().unwrap();
        git_init(&root);
        let capsule_dir = root.path().join(".capsule");
        std::fs::create_dir(&capsule_dir).unwrap();
        std::fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();

        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.path()).unwrap();
        let result = env_gitignore_warning(std::path::Path::new(".capsule"));
        std::env::set_current_dir(original).unwrap();

        assert!(result.is_some(), "expected a warning but got None");
    }
}
