//! Abstraction over where the host's Claude credentials live.
//!
//! On Linux/Windows (and macOS when the file exists) credentials are read from
//! `~/.claude/.credentials.json`. On macOS without that file, Claude Code stores
//! its OAuth credentials in the login Keychain as a generic-password item named
//! `Claude Code-credentials`. [`CredentialsSource`] hides that difference behind
//! a single small interface (`detect` / `read` / `revision` / `write`), selected
//! by compile-time platform dispatch rather than runtime injection.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// The Keychain generic-password item Claude Code uses on macOS.
#[cfg(target_os = "macos")]
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Opaque marker of the host credentials' current state, used to detect a
/// concurrent host-side token rotation between snapshot and write-back.
///
/// For files this is the file mtime (cheap, no content read); for the Keychain
/// it is the secret bytes themselves (the Keychain has no usable timestamp).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRevision {
    /// No credentials present on the host.
    Missing,
    /// File-backed: last-modified time of `.credentials.json`.
    Mtime(std::time::SystemTime),
    /// Keychain-backed: the raw secret bytes.
    #[cfg(target_os = "macos")]
    Bytes(Vec<u8>),
}

/// Source of the host's Claude credentials. Variants are `cfg`-gated so only the
/// platform-appropriate ones are compiled.
pub enum CredentialsSource {
    File(FileSource),
    #[cfg(target_os = "macos")]
    Keychain(KeychainSource),
}

impl CredentialsSource {
    /// Pick a source for `claude_dir`. A `.credentials.json` that actually holds
    /// an OAuth token wins (every platform); otherwise macOS falls back to the
    /// Keychain. The file is checked for real credentials, not mere
    /// presence/size: Claude Code on macOS leaves a zero-byte (or otherwise
    /// content-less) `.credentials.json` placeholder while storing the secret in
    /// the Keychain, and a missing/empty/malformed/incomplete file should never
    /// shadow a valid Keychain item.
    pub fn detect(claude_dir: &Path) -> Self {
        let file = claude_dir.join(".credentials.json");
        if file_has_oauth_token(&file) {
            return Self::File(FileSource { path: file });
        }
        #[cfg(target_os = "macos")]
        {
            Self::Keychain(KeychainSource::new(CLAUDE_KEYCHAIN_SERVICE))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::File(FileSource { path: file })
        }
    }

    /// Read the current credentials, or `Ok(None)` when none exist on the host.
    pub fn read(&self) -> Result<Option<Vec<u8>>> {
        match self {
            Self::File(s) => s.read(),
            #[cfg(target_os = "macos")]
            Self::Keychain(s) => s.read(),
        }
    }

    /// Capture the host credential state for later concurrent-rotation detection.
    pub fn revision(&self) -> Result<HostRevision> {
        match self {
            Self::File(s) => s.revision(),
            #[cfg(target_os = "macos")]
            Self::Keychain(s) => s.revision(),
        }
    }

    /// Persist refreshed credentials back to the host.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::File(s) => s.write(bytes),
            #[cfg(target_os = "macos")]
            Self::Keychain(s) => s.write(bytes),
        }
    }
}

/// JSON pointer to the OAuth access token within a Claude credentials blob.
/// The single field that proves a source actually holds a usable credential.
const OAUTH_ACCESS_TOKEN_POINTER: &str = "/claudeAiOauth/accessToken";

/// True when `bytes` parse as JSON carrying a non-empty OAuth access token.
/// Used to decide whether a credential source is usable rather than merely
/// present — an empty, malformed, or incomplete blob is not.
fn bytes_have_oauth_token(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .as_ref()
        .and_then(|json| json.pointer(OAUTH_ACCESS_TOKEN_POINTER))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|token| !token.is_empty())
}

/// True when the file at `path` holds a usable OAuth credential. Any read or
/// parse failure (absent, empty, malformed, incomplete) is treated as "no".
fn file_has_oauth_token(path: &Path) -> bool {
    std::fs::read(path)
        .map(|bytes| bytes_have_oauth_token(&bytes))
        .unwrap_or(false)
}

/// File-backed credentials at `~/.claude/.credentials.json`.
pub struct FileSource {
    path: PathBuf,
}

impl FileSource {
    fn read(&self) -> Result<Option<Vec<u8>>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => {
                Err(anyhow::Error::new(e)
                    .context(format!("failed to read {}", self.path.display())))
            }
        }
    }

    fn revision(&self) -> Result<HostRevision> {
        match self.path.metadata().and_then(|m| m.modified()) {
            Ok(mtime) => Ok(HostRevision::Mtime(mtime)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HostRevision::Missing),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("failed to read mtime of {}", self.path.display()))),
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        std::fs::write(&self.path, bytes).map_err(|e| {
            anyhow::Error::new(e).context(format!("failed to write {}", self.path.display()))
        })
    }
}

/// macOS Keychain-backed credentials, accessed via the `security` CLI.
#[cfg(target_os = "macos")]
pub struct KeychainSource {
    service: String,
}

#[cfg(target_os = "macos")]
impl KeychainSource {
    fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    /// `security find-generic-password -s <service> -w` — prints the secret to
    /// stdout, prompt-free (security owns the read). Returns `Ok(None)` when no
    /// item exists.
    fn read(&self) -> Result<Option<Vec<u8>>> {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", &self.service, "-w"])
            .output()
            .map_err(|e| anyhow::Error::new(e).context("failed to invoke `security`"))?;
        if !out.status.success() {
            // Non-zero typically means "item not found"; treat as absent.
            return Ok(None);
        }
        // `-w` appends a trailing newline to the password value; the stored
        // secret is single-line JSON, so strip exactly the trailing newline.
        let mut bytes = out.stdout;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    fn revision(&self) -> Result<HostRevision> {
        match self.read()? {
            Some(bytes) => Ok(HostRevision::Bytes(bytes)),
            None => Ok(HostRevision::Missing),
        }
    }

    /// Update the existing item in place via
    /// `security add-generic-password -U -a "" -s <service> -w`, feeding the
    /// secret twice over stdin (security prompts for password + confirmation).
    /// The secret never touches the process arg list. JSON is normalized to a
    /// single line first because the stdin reader is line-based.
    fn write(&self, bytes: &[u8]) -> Result<()> {
        use std::io::Write;

        let normalized = normalize_json_single_line(bytes)?;
        let mut child = std::process::Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                "",
                "-s",
                &self.service,
                "-w",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::Error::new(e).context("failed to invoke `security`"))?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("failed to open `security` stdin"))?;
            // The value, then the confirmation — both line-terminated.
            stdin
                .write_all(normalized.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"))
                .and_then(|()| stdin.write_all(normalized.as_bytes()))
                .and_then(|()| stdin.write_all(b"\n"))
                .map_err(|e| {
                    anyhow::Error::new(e).context("failed to write secret to `security`")
                })?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| anyhow::Error::new(e).context("failed to wait on `security`"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("`security add-generic-password` failed: {}", stderr.trim());
        }
        Ok(())
    }
}

/// Normalize JSON to a single line so it can be fed to a line-based stdin reader.
/// Raw newlines in Claude's credentials only ever appear as formatting
/// whitespace, so a compact re-serialization is lossless.
#[cfg(target_os = "macos")]
fn normalize_json_single_line(bytes: &[u8]) -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| anyhow::Error::new(e).context("credentials are not valid JSON"))?;
    serde_json::to_string(&value)
        .map_err(|e| anyhow::Error::new(e).context("failed to serialize credentials"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CREDS: &[u8] = br#"{"claudeAiOauth":{"accessToken":"sk-ant-xyz","expiresAt":123}}"#;

    #[test]
    fn detect_uses_file_when_it_has_oauth_token() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".credentials.json"), VALID_CREDS).unwrap();
        assert!(matches!(
            CredentialsSource::detect(dir.path()),
            CredentialsSource::File(_)
        ));
    }

    #[test]
    fn file_has_oauth_token_distinguishes_real_credentials() {
        assert!(bytes_have_oauth_token(VALID_CREDS));
        assert!(!bytes_have_oauth_token(b"")); // empty placeholder
        assert!(!bytes_have_oauth_token(b"not json")); // malformed
        assert!(!bytes_have_oauth_token(b"{}")); // valid JSON, no oauth block
        assert!(!bytes_have_oauth_token(br#"{"claudeAiOauth":{}}"#)); // no token
        assert!(!bytes_have_oauth_token(
            br#"{"claudeAiOauth":{"accessToken":""}}"# // empty token
        ));
    }

    // A content-less or incomplete `.credentials.json` (e.g. the zero-byte
    // placeholder Claude Code leaves on macOS) must not win over the Keychain.
    #[cfg(target_os = "macos")]
    #[test]
    fn detect_falls_back_to_keychain_when_file_lacks_oauth_token() {
        for content in [&b""[..], b"{}", b"not json", br#"{"claudeAiOauth":{}}"#] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(".credentials.json"), content).unwrap();
            assert!(
                matches!(
                    CredentialsSource::detect(dir.path()),
                    CredentialsSource::Keychain(_)
                ),
                "content {content:?} should fall back to Keychain"
            );
        }
    }

    // The File adapter is exercised directly (not via `detect`) so these tests
    // are platform-independent: on macOS, `detect` on a missing file falls back
    // to the real Keychain, which is not what we're testing here.
    fn file_source(dir: &std::path::Path) -> CredentialsSource {
        CredentialsSource::File(FileSource {
            path: dir.join(".credentials.json"),
        })
    }

    #[test]
    fn file_read_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(file_source(dir.path()).read().unwrap(), None);
    }

    #[test]
    fn file_revision_missing_then_mtime() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            file_source(dir.path()).revision().unwrap(),
            HostRevision::Missing
        );

        std::fs::write(dir.path().join(".credentials.json"), b"data").unwrap();
        assert!(matches!(
            file_source(dir.path()).revision().unwrap(),
            HostRevision::Mtime(_)
        ));
    }

    #[test]
    fn file_round_trips_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".credentials.json"), b"original").unwrap();
        let source = file_source(dir.path());
        assert_eq!(source.read().unwrap().as_deref(), Some(&b"original"[..]));
        source.write(b"updated").unwrap();
        assert_eq!(source.read().unwrap().as_deref(), Some(&b"updated"[..]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_round_trips_secret() {
        // Use a throwaway, uniquely-named service so we never touch the real
        // Claude item, and `security` owns it so no prompt appears.
        let service = format!("capsule-test-{}-credentials", std::process::id());
        let source = CredentialsSource::Keychain(KeychainSource::new(&service));

        // Absent before first write.
        assert_eq!(source.read().unwrap(), None);
        assert_eq!(source.revision().unwrap(), HostRevision::Missing);

        let secret = br#"{"claudeAiOauth":{"accessToken":"a","expiresAt":123}}"#;
        source.write(secret).unwrap();

        let read_back = source.read().unwrap().expect("secret present");
        assert_eq!(read_back, secret);
        assert_eq!(
            source.revision().unwrap(),
            HostRevision::Bytes(secret.to_vec())
        );

        // Update in place — no duplicate item.
        let updated = br#"{"claudeAiOauth":{"accessToken":"b","expiresAt":456}}"#;
        source.write(updated).unwrap();
        assert_eq!(source.read().unwrap().as_deref(), Some(&updated[..]));

        // Clean up.
        let _ = std::process::Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .output();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn normalize_collapses_pretty_json() {
        let pretty = b"{\n  \"a\": 1\n}";
        assert_eq!(normalize_json_single_line(pretty).unwrap(), "{\"a\":1}");
    }
}
