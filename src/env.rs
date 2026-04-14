use anyhow::{Context, Result};
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

/// Resolve `GH_TOKEN` from the provided environment map.
///
/// Falls back to running `gh auth token` on the host if the key is absent.
/// Returns `None` if both sources fail (no token available).
pub fn resolve_gh_token(env: &HashMap<String, String>) -> Option<String> {
    if let Some(token) = env.get("GH_TOKEN") {
        if !token.is_empty() {
            return Some(token.clone());
        }
    }
    // Fallback: ask the gh CLI.
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }
    None
}
