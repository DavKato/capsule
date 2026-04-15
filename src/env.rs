use crate::config::GithubScope;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Parse a `.env` file's content into a key→value map.
///
/// - Lines beginning with `#` are comments and ignored.
/// - Blank lines are ignored.
/// - Values may optionally be wrapped in double or single quotes (stripped).
/// - The first `=` separates key from value; `=` signs in the value are preserved.
pub fn parse_dotenv(content: &str) -> HashMap<String, String> {
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

/// Load `${capsule_dir}/.env` into the current process environment.
///
/// No-op if the file does not exist. Variables already set in the environment
/// are **not** overwritten (the file values only fill gaps).
pub fn load_dotenv(capsule_dir: &Path) -> Result<()> {
    let path = capsule_dir.join(".env");
    if !path.exists() {
        return Ok(());
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for (key, value) in parse_dotenv(&content) {
        // Only set if not already set — process env takes precedence.
        if std::env::var(&key).is_err() {
            std::env::set_var(&key, &value);
        }
    }
    Ok(())
}

/// Resolve `GH_TOKEN` based on the requested scope.
///
/// - `Local`: reads `GH_TOKEN` exclusively from `dotenv_map` (the parsed `.capsule/.env`).
///   Never reads from the process environment, so a process-env token cannot shadow
///   the project token. Returns an error if the key is absent.
/// - `Global`: reads `GH_TOKEN` from `pre_dotenv_env` (the process environment captured
///   *before* `.env` is sourced). Falls back to `gh auth token` on the host.
///   Returns an error if neither source has a token.
///
/// The two-map design prevents precedence bugs: `local` explicitly ignores
/// whatever is in the process environment.
pub fn resolve_gh_token(
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
            // 1. Process env (pre-dotenv so .env cannot interfere with global scope).
            if let Some(token) = pre_dotenv_env.get("GH_TOKEN").filter(|t| !t.is_empty()) {
                return Ok(token.clone());
            }
            // 2. gh auth token fallback.
            let output = std::process::Command::new("gh")
                .args(["auth", "token"])
                .output();
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
