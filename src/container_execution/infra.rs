use std::process::Command;

/// Detect the Docker network of a Compose project running with `working_dir` equal to `pwd`.
///
/// Runs `docker ps` to find containers from a Compose project at `pwd`, then inspects
/// those containers to find the associated network name. Returns `None` if no Compose
/// project is running at `pwd` or if any Docker call fails (best-effort).
pub fn detect_compose_network(pwd: &std::path::Path) -> Option<String> {
    let pwd_str = pwd.to_string_lossy();

    let ps_out = Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("label=com.docker.compose.project.working_dir={pwd_str}"),
            "--format",
            "{{.ID}}",
        ])
        .output()
        .ok()?;

    if !ps_out.status.success() {
        return None;
    }

    let ids: Vec<&str> = std::str::from_utf8(&ps_out.stdout)
        .ok()?
        .lines()
        .filter(|l| !l.is_empty())
        .collect();

    let container_id = ids.first()?;

    let inspect_out = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range $k, $v := .NetworkSettings.Networks}}{{$k}}\n{{end}}",
            container_id,
        ])
        .output()
        .ok()?;

    if !inspect_out.status.success() {
        return None;
    }

    std::str::from_utf8(&inspect_out.stdout)
        .ok()?
        .lines()
        .find(|l| !l.is_empty())
        .map(|s| s.to_string())
}

/// Returns the JSON content for a per-run `.mcp.json` file that points
/// `capsule mcp-serve` at the given binary path inside the container.
pub fn make_mcp_config(capsule_container_bin: &std::path::Path) -> String {
    let bin = capsule_container_bin.to_string_lossy();
    serde_json::json!({
        "mcpServers": {"capsule": {"command": bin.as_ref(), "args": ["mcp-serve"]}}
    })
    .to_string()
}

fn read_expires_at(claude_dir: &std::path::Path) -> Option<u64> {
    let source = super::credentials_source::CredentialsSource::detect(claude_dir);
    let bytes = source.read().ok().flatten()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json.pointer("/claudeAiOauth/expiresAt")
        .and_then(serde_json::Value::as_u64)
}

pub fn token_remaining_minutes(claude_dir: &std::path::Path) -> Option<u64> {
    let expires_at = read_expires_at(claude_dir)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if expires_at <= now_ms {
        Some(0)
    } else {
        Some((expires_at - now_ms) / 60_000)
    }
}

pub fn host_token_is_expired(claude_dir: &std::path::Path) -> bool {
    match token_remaining_minutes(claude_dir) {
        None => true,
        Some(0) => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_mcp_config_contains_binary_path_and_mcp_serve() {
        let bin = std::path::Path::new("/usr/local/bin/capsule");
        let cfg = make_mcp_config(bin);
        let v: serde_json::Value = serde_json::from_str(&cfg).expect("valid JSON");
        assert_eq!(
            v["mcpServers"]["capsule"]["command"],
            "/usr/local/bin/capsule"
        );
        assert_eq!(v["mcpServers"]["capsule"]["args"][0], "mcp-serve");
    }

    #[test]
    fn host_token_expired_when_expires_at_in_past() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":1000}}"#).unwrap();
        assert!(host_token_is_expired(dir.path()));
    }

    #[test]
    fn host_token_not_expired_when_expires_at_in_future() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":2524608000000}}"#).unwrap();
        assert!(!host_token_is_expired(dir.path()));
    }

    // On macOS a missing file falls back to the login Keychain, so "no file" no
    // longer means "no credentials"; this case only holds without that fallback.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn host_token_expired_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(host_token_is_expired(dir.path()));
    }

    #[test]
    fn host_token_expired_when_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        std::fs::write(&creds, "not json").unwrap();
        assert!(host_token_is_expired(dir.path()));
    }

    // See note above: skipped on macOS due to Keychain fallback.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn token_remaining_none_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(token_remaining_minutes(dir.path()).is_none());
    }

    #[test]
    fn token_remaining_none_when_expired() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":1000}}"#).unwrap();
        assert_eq!(token_remaining_minutes(dir.path()), Some(0));
    }

    #[test]
    fn token_remaining_returns_minutes_when_valid() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let future_ms = now_ms + 30 * 60 * 1000; // 30 min from now
        let json = format!(r#"{{"claudeAiOauth":{{"expiresAt":{future_ms}}}}}"#);
        std::fs::write(&creds, json).unwrap();
        let remaining = token_remaining_minutes(dir.path()).unwrap();
        assert!((29..=31).contains(&remaining), "got {remaining}");
    }
}
