use anyhow::{bail, Context, Result};
use std::process::{Command, Stdio};

/// The base Dockerfile embedded at compile time.
pub const DOCKERFILE: &str = include_str!("../templates/Dockerfile");

/// The jq stream-display filter embedded at compile time.
pub const STREAM_DISPLAY_JQ: &str = include_str!("../templates/stream_display.jq");

const BASE_IMAGE: &str = "capsule";

/// Returns `true` if a Docker image with the given name exists locally.
fn image_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build the base `capsule` Docker image from the embedded Dockerfile.
///
/// If `rebuild` is `false` and the image already exists, the build is skipped.
/// If `rebuild` is `true`, the image is always rebuilt.
pub fn build_base_image(rebuild: bool) -> Result<()> {
    if !rebuild && image_exists(BASE_IMAGE) {
        return Ok(());
    }

    eprintln!("Building {BASE_IMAGE} image…");

    let mut child = Command::new("docker")
        .args(["build", "-t", BASE_IMAGE, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn `docker build`")?;

    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin piped");
        stdin
            .write_all(DOCKERFILE.as_bytes())
            .context("failed to write Dockerfile to docker stdin")?;
    }

    let status = child.wait().context("docker build did not complete")?;
    if !status.success() {
        bail!(
            "docker build exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    eprintln!("Image ready.");
    Ok(())
}
