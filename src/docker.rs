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

/// Outcome of a single iteration.
#[derive(Debug, PartialEq)]
pub enum IterationOutcome {
    /// Loop should continue to the next iteration.
    Continue,
    /// Claude signalled completion; loop should stop.
    Done,
}

/// Returns `true` if the given line of container output signals an authentication failure.
pub fn contains_auth_failure(line: &str) -> bool {
    line.contains("authentication_failed")
}

/// Returns `true` if the given line contains the NO MORE TASKS completion marker.
pub fn contains_no_more_tasks(line: &str) -> bool {
    line.contains("<promise>NO MORE TASKS</promise>")
}

/// Build the `docker run` argument list for one iteration.
///
/// Extracted for testability. Adds a read-only bind-mount of `.git/config` when
/// present in `cfg.pwd`, preventing container processes from mutating the host
/// repository's remote URLs or other local git config.
pub fn build_docker_args(cfg: &RunConfig, prompt_path: &std::path::Path) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        format!("-v={}:/home/claude/prompt.txt:ro", prompt_path.display()),
        format!("-v={}:/workspace", cfg.pwd.display()),
    ];

    // Protect the host git config from container mutations (issue #20).
    // If the workspace is a git repo, mount .git/config read-only so that
    // container processes (including Claude) cannot rewrite remote URLs or
    // other local settings back to the host.
    let git_config = cfg.pwd.join(".git").join("config");
    if git_config.exists() {
        args.push(format!(
            "-v={}:/workspace/.git/config:ro",
            git_config.display()
        ));
    }

    if let Some(model) = &cfg.model {
        args.push(format!("-e=CAPSULE_MODEL={model}"));
    }

    args.push(cfg.image.clone());
    args
}

/// Run one iteration: mount prompt, stream output through jq, propagate exit code.
///
/// Returns [`IterationOutcome::Done`] when the output contains the NO MORE TASKS marker.
///
/// # Errors
/// - Container exits non-zero → error naming the exit code.
/// - Output contains `authentication_failed` → error with remediation hint.
pub fn run_iteration(cfg: &RunConfig) -> Result<IterationOutcome> {
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

    let docker_args = build_docker_args(cfg, &prompt_path);

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
    let mut no_more_tasks = false;

    for line in reader.lines() {
        let line = line.context("error reading docker stdout")?;

        if contains_auth_failure(&line) {
            auth_failed = true;
        }

        if contains_no_more_tasks(&line) {
            no_more_tasks = true;
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

    if no_more_tasks {
        Ok(IterationOutcome::Done)
    } else {
        Ok(IterationOutcome::Continue)
    }
}
