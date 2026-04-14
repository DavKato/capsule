use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Run `${capsule_dir}/before-all.sh` on the host if it exists.
///
/// The script runs with the current process environment (which includes any
/// variables sourced from `${capsule_dir}/.env`). Non-zero exit aborts the
/// entire run. Absent script → Ok(()).
pub fn run_before_all(capsule_dir: &Path) -> Result<()> {
    let script = capsule_dir.join("before-all.sh");
    if !script.exists() {
        return Ok(());
    }

    let status = Command::new("bash")
        .arg(&script)
        .status()
        .with_context(|| format!("failed to run before-all.sh at {}", script.display()))?;

    if !status.success() {
        bail!(
            "before-all.sh exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
}
