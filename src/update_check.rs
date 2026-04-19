use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const RELEASES_URL: &str = "https://api.github.com/repos/DavKato/capsule/releases/latest";

pub struct UpdateNotice {
    pub current: String,
    pub latest: String,
}

/// Spawns a background thread to check for updates. Returns a receiver that
/// yields `Some(notice)` when a newer version is available, or `None` otherwise.
pub fn spawn_check() -> Receiver<Option<UpdateNotice>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = check_for_update();
        let _ = tx.send(result);
    });
    rx
}

/// Prints the update notification box if a newer version was found.
/// Waits up to 2 seconds for the background thread before giving up.
pub fn maybe_print_notice(rx: Receiver<Option<UpdateNotice>>) {
    let Ok(Some(notice)) = rx.recv_timeout(Duration::from_secs(2)) else {
        return;
    };
    eprintln!();
    eprintln!("╭─────────────────────────────────────────╮");
    eprintln!(
        "│  Update available: {} → {}{}│",
        notice.current,
        notice.latest,
        padding(notice.current.len() + notice.latest.len())
    );
    eprintln!("│  Run capsule update to install it.      │");
    eprintln!("╰─────────────────────────────────────────╯");
}

fn padding(used: usize) -> &'static str {
    // "  Update available: X.X.X → X.X.X" + padding + "│"
    // Box inner width = 41, "  Update available: " = 20, " → " = 3, trailing " │" = 2
    // available for versions: 41 - 20 - 3 - 2 = 16 chars; we pad the rest
    const INNER: usize = 41;
    const PREFIX: usize = 20; // "  Update available: "
    const ARROW: usize = 3; // " → "
    const SUFFIX: usize = 2; // "  │" (two spaces + │ are part of format)
    let space = INNER.saturating_sub(PREFIX + ARROW + used + SUFFIX);
    // Return a static slice from a fixed-length spaces string.
    &"                                         "[..space.min(41)]
}

fn check_for_update() -> Option<UpdateNotice> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let cache_path = cache_file_path()?;
    let latest_tag = cached_or_fetch(&cache_path);
    let latest = latest_tag?;
    let latest_ver = strip_v(&latest).to_string();
    if is_newer(&latest_ver, &current) {
        Some(UpdateNotice {
            current,
            latest: latest_ver,
        })
    } else {
        None
    }
}

fn cached_or_fetch(cache_path: &PathBuf) -> Option<String> {
    if let Some((ts, tag)) = read_cache(cache_path) {
        let age = SystemTime::now()
            .duration_since(ts)
            .unwrap_or(CACHE_MAX_AGE);
        if age < CACHE_MAX_AGE {
            return tag;
        }
    }
    let tag = fetch_latest_tag();
    write_cache(cache_path, tag.as_deref());
    tag
}

fn cache_file_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".cache").join("capsule");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("update-check"))
}

/// Cache format: two lines — unix timestamp (secs) and tag name (or empty for "no release").
fn read_cache(path: &PathBuf) -> Option<(SystemTime, Option<String>)> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let secs: u64 = lines.next()?.trim().parse().ok()?;
    let ts = UNIX_EPOCH + Duration::from_secs(secs);
    let tag_line = lines.next().unwrap_or("").trim().to_string();
    let tag = if tag_line.is_empty() {
        None
    } else {
        Some(tag_line)
    };
    Some((ts, tag))
}

fn write_cache(path: &PathBuf, tag: Option<&str>) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let content = format!("{}\n{}\n", secs, tag.unwrap_or(""));
    let _ = std::fs::write(path, content);
}

fn fetch_latest_tag() -> Option<String> {
    let output = std::process::Command::new("curl")
        .args([
            "--silent",
            "--max-time",
            "5",
            "--header",
            "User-Agent: capsule-update-check",
            RELEASES_URL,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout);
    extract_tag_name(&body)
}

/// Extracts `tag_name` value from GitHub releases API JSON response.
fn extract_tag_name(json: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let start = json.find(key)?;
    let after_key = &json[start + key.len()..];
    let colon = after_key.find(':')? + 1;
    let after_colon = after_key[colon..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

fn strip_v(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Returns true if `latest` is strictly newer than `current` (semver X.Y.Z).
pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.splitn(3, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_detects_patch_bump() {
        assert!(is_newer("0.1.3", "0.1.2"));
    }

    #[test]
    fn is_newer_detects_minor_bump() {
        assert!(is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn is_newer_detects_major_bump() {
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn is_newer_same_version_is_false() {
        assert!(!is_newer("0.1.2", "0.1.2"));
    }

    #[test]
    fn is_newer_older_is_false() {
        assert!(!is_newer("0.1.1", "0.1.2"));
    }

    #[test]
    fn is_newer_strips_v_prefix() {
        assert!(is_newer("v0.1.3", "0.1.2"));
        assert!(is_newer("0.1.3", "v0.1.2"));
    }

    #[test]
    fn extract_tag_name_parses_github_response() {
        let json = r#"{"url":"https://api.github.com/repos/x/y/releases/1","tag_name":"v0.1.3","name":"v0.1.3"}"#;
        assert_eq!(extract_tag_name(json), Some("v0.1.3".to_string()));
    }

    #[test]
    fn extract_tag_name_returns_none_on_invalid_json() {
        assert_eq!(extract_tag_name("{}"), None);
        assert_eq!(extract_tag_name("not json"), None);
    }

    #[test]
    fn cache_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check").to_path_buf();
        write_cache(&path, Some("v0.1.3"));
        let (ts, tag) = read_cache(&path).unwrap();
        let age = SystemTime::now().duration_since(ts).unwrap();
        assert!(age < Duration::from_secs(5), "timestamp should be recent");
        assert_eq!(tag, Some("v0.1.3".to_string()));
    }

    #[test]
    fn cache_round_trip_none_tag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check").to_path_buf();
        write_cache(&path, None);
        let (_ts, tag) = read_cache(&path).unwrap();
        assert_eq!(tag, None);
    }
}
