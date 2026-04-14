use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
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

/// Configuration for a single iteration's `docker run`.
pub struct RunConfig {
    /// Docker image to run (base or derived).
    pub image: String,
    /// Prompt content to mount as `/home/claude/prompt.txt`.
    pub prompt: String,
    /// Host working directory — mounted as `/workspace`.
    pub pwd: PathBuf,
    /// Capsule directory (unused in this slice; reserved for future mounts).
    pub capsule_dir: PathBuf,
    /// Optional model override passed via `-e CAPSULE_MODEL`.
    pub model: Option<String>,
    /// When true, print unfiltered container output in addition to jq-filtered view.
    pub verbose: bool,
}

/// Returns `true` if the given line of container output signals an authentication failure.
pub fn contains_auth_failure(line: &str) -> bool {
    line.contains("authentication_failed")
}

/// Run one iteration: mount prompt, stream output through jq, propagate exit code.
///
/// # Errors
/// - Container exits non-zero → error naming the exit code.
/// - Output contains `authentication_failed` → error with remediation hint.
pub fn run_iteration(cfg: &RunConfig) -> Result<()> {
    // Write prompt to a named temp file so it can be bind-mounted.
    let mut prompt_file = tempfile::Builder::new()
        .prefix("capsule-prompt-")
        .suffix(".txt")
        .tempfile()
        .context("failed to create prompt temp file")?;
    prompt_file
        .write_all(cfg.prompt.as_bytes())
        .context("failed to write prompt to temp file")?;
    prompt_file.flush().context("failed to flush prompt file")?;
    let prompt_path = prompt_file.path().to_owned();

    let mut docker_args = vec![
        "run".to_string(),
        "--rm".to_string(),
        format!("-v={}:/home/claude/prompt.txt:ro", prompt_path.display()),
        format!("-v={}:/workspace", cfg.pwd.display()),
    ];

    if let Some(model) = &cfg.model {
        docker_args.push(format!("-e=CAPSULE_MODEL={model}"));
    }

    docker_args.push(cfg.image.clone());

    // Spawn docker with stdout piped; stderr goes to the terminal.
    let mut docker_child = Command::new("docker")
        .args(&docker_args)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn `docker run`")?;

    let docker_stdout = docker_child.stdout.take().expect("stdout piped");
    let reader = BufReader::new(docker_stdout);

    // Spawn jq for human-readable display.
    let jq_filter = STREAM_DISPLAY_JQ;
    let mut jq_child = Command::new("jq")
        .args(["-R", "-r", jq_filter])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `jq`")?;

    let mut jq_stdin = jq_child.stdin.take().expect("jq stdin piped");

    let mut auth_failed = false;

    for line in reader.lines() {
        let line = line.context("error reading docker stdout")?;

        if contains_auth_failure(&line) {
            auth_failed = true;
        }

        if cfg.verbose {
            eprintln!("{line}");
        }

        // Feed to jq (ignore write errors — jq may exit early).
        let _ = writeln!(jq_stdin, "{line}");
    }

    // Close jq stdin so it can flush and exit.
    drop(jq_stdin);
    let _ = jq_child.wait();

    let status = docker_child.wait().context("docker run did not complete")?;

    if auth_failed {
        bail!(
            "Claude authentication failed. Run `claude` on the host to refresh credentials, then retry."
        );
    }

    if !status.success() {
        bail!("container exited with code {}", status.code().unwrap_or(-1));
    }

    Ok(())
}
