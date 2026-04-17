use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Check that the Docker daemon is reachable.
///
/// Returns an error with an actionable message if Docker is not installed or
/// the daemon is not running.
pub fn check_docker() -> Result<()> {
    let status = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context(
            "Docker is not installed or not in PATH — ensure Docker is installed and running",
        )?;

    if !status.success() {
        anyhow::bail!(
            "Docker daemon is not reachable — ensure Docker is running (`docker info` failed)"
        );
    }
    Ok(())
}

/// Check whether `<capsule_dir>/.env` is listed in `.gitignore`.
///
/// Returns `Some(warning_message)` if the file exists but is NOT gitignored.
/// Returns `None` if the file is absent, gitignored, or the check cannot be
/// determined (e.g. git is not installed).
pub fn env_gitignore_warning(capsule_dir: &Path) -> Option<String> {
    let env_path = capsule_dir.join(".env");
    if !env_path.exists() {
        return None;
    }

    // `git check-ignore -q <path>` exits 0 when the path is ignored, 1 when it
    // is not ignored, and 128 on a fatal error. We treat anything other than
    // exit 0 as "not ignored" so we err on the side of showing the warning.
    //
    // Run from capsule_dir so git uses the repository that owns the .env file,
    // not whatever repo the capsule process itself was launched from.
    //
    // Canonicalize env_path to an absolute path so that git resolves it
    // correctly regardless of current_dir. Without this, if capsule_dir is
    // relative (e.g. ".capsule"), git would interpret ".capsule/.env" relative
    // to its new CWD and look for ".capsule/.capsule/.env" instead.
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
