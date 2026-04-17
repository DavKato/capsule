use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

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

/// Returns the derived image name for the given working directory.
///
/// Format: `capsule-<basename(pwd)>`. Falls back to `capsule-project` when the
/// directory has no file-name component (e.g. `/`).
pub fn derived_image_name(pwd: &std::path::Path) -> String {
    let basename = pwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    format!("capsule-{basename}")
}

/// Build a derived Docker image from `${capsule_dir}/Dockerfile` if it exists.
///
/// Returns `Ok(None)` when no `Dockerfile` is found in `capsule_dir`.
/// Returns `Ok(Some(name))` with the derived image name when the image exists or
/// was successfully built.
///
/// The derived image is named `capsule-<basename(pwd)>` and uses `capsule_dir`
/// as its build context so relative `COPY` instructions resolve correctly.
///
/// If `rebuild` is `false` and the derived image already exists, the build is
/// skipped and the cached image name is returned.
pub fn build_derived_image(
    capsule_dir: &std::path::Path,
    pwd: &std::path::Path,
    rebuild: bool,
) -> Result<Option<String>> {
    let dockerfile = capsule_dir.join("Dockerfile");
    if !dockerfile.exists() {
        return Ok(None);
    }

    let name = derived_image_name(pwd);

    if !rebuild && image_exists(&name) {
        return Ok(Some(name));
    }

    eprintln!("Building derived image {name}…");

    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            &name,
            "-f",
            &dockerfile.to_string_lossy(),
            &capsule_dir.to_string_lossy(),
        ])
        .status()
        .context("failed to spawn `docker build` for derived image")?;

    if !status.success() {
        bail!(
            "docker build for derived image {name} exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    eprintln!("Derived image ready.");
    Ok(Some(name))
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
#[derive(Default)]
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
    /// Path to the `.env` file to pass via `--env-file` (None → omitted).
    pub env_file: Option<PathBuf>,
    /// Path to a temp env-file containing `GH_TOKEN=<token>` (None → no token injected).
    /// Passed as a second `--env-file` so the token never appears in the process arg list.
    pub gh_token_env_file: Option<PathBuf>,
    /// Git author/committer name passed as `GIT_AUTHOR_NAME` and `GIT_COMMITTER_NAME`.
    pub git_author_name: String,
    /// Git author/committer email passed as `GIT_AUTHOR_EMAIL` and `GIT_COMMITTER_EMAIL`.
    pub git_author_email: String,
    /// Path to `before-each.sh` on the host. When Some, mounted read-only into
    /// the container at `/home/claude/before-each.sh`.
    pub before_each_path: Option<PathBuf>,
    /// Docker network to attach the container to. Detected from a running Compose
    /// project at `pwd`; None when no project is found.
    pub compose_network: Option<String>,
    /// Host `~/.claude` directory, mounted writable at `/home/claude/.claude` so
    /// the container can authenticate and share memory/sessions with the host.
    pub claude_dir: PathBuf,
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

/// Returns a unique container name for the given iteration.
///
/// Format: `capsule-run-<pid>-<iteration>`.  Unique per process per iteration
/// so the ctrlc handler can call `docker stop <name>` when the user interrupts.
pub fn container_name_for(iteration: u32) -> String {
    format!("capsule-run-{}-{}", std::process::id(), iteration)
}

/// Build the `docker run` argument list for one iteration.
///
/// Extracted for testability. Adds a read-only bind-mount of `.git/config` when
/// present in `cfg.pwd`, preventing container processes from mutating the host
/// repository's remote URLs or other local git config.
pub fn build_docker_args(
    cfg: &RunConfig,
    prompt_path: &std::path::Path,
    container_name: &str,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        format!("-v={}:/home/claude/prompt.txt:ro", prompt_path.display()),
        format!("-v={}:/workspace", cfg.pwd.display()),
        format!("-v={}:/home/claude/.claude", cfg.claude_dir.display()),
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

    if let Some(env_file) = &cfg.env_file {
        args.push(format!("--env-file={}", env_file.display()));
    }

    if let Some(token_file) = &cfg.gh_token_env_file {
        args.push(format!("--env-file={}", token_file.display()));
    }

    if let Some(model) = &cfg.model {
        args.push(format!("-e=CAPSULE_MODEL={model}"));
    }

    // Pass git identity to the container entrypoint so it can configure
    // `git config --global user.name/email`. The entrypoint falls back to
    // `Capsule <capsule@localhost>` when these are empty.
    args.push(format!("-e=GIT_AUTHOR_NAME={}", cfg.git_author_name));
    args.push(format!("-e=GIT_AUTHOR_EMAIL={}", cfg.git_author_email));
    args.push(format!("-e=GIT_COMMITTER_NAME={}", cfg.git_author_name));
    args.push(format!("-e=GIT_COMMITTER_EMAIL={}", cfg.git_author_email));

    if let Some(before_each) = &cfg.before_each_path {
        args.push(format!(
            "-v={}:/home/claude/before-each.sh:ro",
            before_each.display()
        ));
    }

    if let Some(network) = &cfg.compose_network {
        args.push("--network".to_string());
        args.push(network.clone());
    }

    args.push(cfg.image.clone());
    args
}

/// Detect the Docker network of a Compose project running with `working_dir` equal to `pwd`.
///
/// Runs `docker ps` to find containers from a Compose project at `pwd`, then inspects
/// those containers to find the associated network name. Returns `None` if no Compose
/// project is running at `pwd` or if any Docker call fails (best-effort).
pub fn detect_compose_network(pwd: &std::path::Path) -> Option<String> {
    let pwd_str = pwd.to_string_lossy();

    // Find container IDs from any Compose project running at pwd.
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

    // Inspect the container to get its network names.
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

/// Scan lines from `reader`, forward each to `jq_stdin`, and detect sentinel values.
///
/// Returns `(auth_failed, no_more_tasks)`. Dropping `jq_stdin` at the end of this
/// function signals EOF to the jq subprocess — same timing as the previous inline loop.
fn stream_output(
    reader: BufReader<impl std::io::Read>,
    mut jq_stdin: impl Write,
    verbose: bool,
) -> Result<(bool, bool)> {
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
        if verbose {
            eprintln!("{line}");
        }
        let _ = writeln!(jq_stdin, "{line}");
    }

    Ok((auth_failed, no_more_tasks))
}

/// Run one iteration: mount prompt, stream output through jq, propagate exit code.
///
/// `iteration` is used to derive a unique `--name` for the container so that a
/// registered ctrlc handler can call `docker stop <name>` on SIGINT.
/// `active_container` is a shared slot; this function writes the container name
/// before spawning and clears it after the container exits.
///
/// Returns [`IterationOutcome::Done`] when the output contains the NO MORE TASKS marker.
///
/// # Errors
/// - Container exits non-zero → error naming the exit code.
/// - Output contains `authentication_failed` → error with remediation hint.
pub fn run_iteration(
    cfg: &RunConfig,
    iteration: u32,
    active_container: &Arc<Mutex<Option<String>>>,
) -> Result<IterationOutcome> {
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

    let name = container_name_for(iteration);

    // Register the container name so the ctrlc handler can stop it.
    if let Ok(mut slot) = active_container.lock() {
        *slot = Some(name.clone());
    }

    let docker_args = build_docker_args(cfg, &prompt_path, &name);

    let mut docker_child = Command::new("docker")
        .args(&docker_args)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn `docker run`")?;

    let reader = BufReader::new(docker_child.stdout.take().expect("stdout piped"));

    let mut jq_child = Command::new("jq")
        .args(["-R", "-r", STREAM_DISPLAY_JQ])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn `jq`")?;

    let jq_stdin = jq_child.stdin.take().expect("jq stdin piped");

    // stream_output drops jq_stdin on return, signalling EOF to jq.
    let (auth_failed, no_more_tasks) = stream_output(reader, jq_stdin, cfg.verbose)?;

    let _ = jq_child.wait();
    let status = docker_child.wait().context("docker run did not complete")?;

    // Container has exited — clear the shared slot so the handler becomes a no-op.
    if let Ok(mut slot) = active_container.lock() {
        *slot = None;
    }

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
