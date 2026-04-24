use crate::stream_parser::StreamParser;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// The base Dockerfile embedded at compile time.
pub const DOCKERFILE: &str = include_str!("../templates/Dockerfile");

/// The container entrypoint script embedded at compile time.
pub const ENTRYPOINT_SH: &str = include_str!("../templates/entrypoint.sh");

/// The jq stream-display filter embedded at compile time.
pub const STREAM_DISPLAY_JQ: &str = include_str!("../templates/stream_display.jq");

const BASE_IMAGE: &str = "capsule";
const DOCKERFILE_HASH_LABEL: &str = "capsule.dockerfile.hash";

fn dockerfile_hash(content: &str) -> String {
    // FNV-1a with hardcoded constants — DefaultHasher is not stable across Rust versions.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn image_label(name: &str, label: &str) -> Option<String> {
    let out = Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            &format!("{{{{index .Config.Labels \"{label}\"}}}}"),
            name,
        ])
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

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
    let content = std::fs::read_to_string(&dockerfile)
        .with_context(|| format!("failed to read {}", dockerfile.display()))?;
    let hash = dockerfile_hash(&content);

    if !rebuild && image_exists(&name) {
        if image_label(&name, DOCKERFILE_HASH_LABEL).as_deref() == Some(&hash) {
            return Ok(Some(name));
        }
        eprintln!("Derived Dockerfile changed — rebuilding {name}…");
    } else {
        eprintln!("Building derived image {name}…");
    }

    let label = format!("{DOCKERFILE_HASH_LABEL}={hash}");
    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            &name,
            "--label",
            &label,
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
/// Skips the build when the image exists and its stored hash matches the
/// embedded Dockerfile. Auto-rebuilds (with layer cache) when the hash
/// differs. With `rebuild: true`, always rebuilds using `--no-cache`.
pub fn build_base_image(rebuild: bool) -> Result<()> {
    let hash = dockerfile_hash(DOCKERFILE);

    if !rebuild && image_exists(BASE_IMAGE) {
        if image_label(BASE_IMAGE, DOCKERFILE_HASH_LABEL).as_deref() == Some(&hash) {
            return Ok(());
        }
        eprintln!("Base Dockerfile changed — rebuilding {BASE_IMAGE}…");
    } else {
        eprintln!("Building {BASE_IMAGE} image…");
    }

    let ctx = tempfile::tempdir().context("failed to create build context tempdir")?;
    std::fs::write(ctx.path().join("Dockerfile"), DOCKERFILE)
        .context("failed to write Dockerfile to build context")?;
    std::fs::write(ctx.path().join("entrypoint.sh"), ENTRYPOINT_SH)
        .context("failed to write entrypoint.sh to build context")?;

    let ctx_path = ctx.path().to_string_lossy().into_owned();
    let label = format!("{DOCKERFILE_HASH_LABEL}={hash}");
    let mut build_args = vec!["build", "-t", BASE_IMAGE, "--label", &label];
    if rebuild {
        build_args.push("--no-cache");
    }
    build_args.push(&ctx_path);

    let status = Command::new("docker")
        .args(&build_args)
        .status()
        .context("failed to spawn `docker build`")?;
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
    /// Isolated copy of `~/.claude/.credentials.json`, mounted over the directory
    /// mount to prevent concurrent host/container token rotation from invalidating
    /// each other's sessions (issue #55). `None` when the credentials file does not
    /// exist on the host.
    pub credentials_file: Option<PathBuf>,
}

/// Outcome of a single iteration.
#[derive(Debug)]
pub enum IterationOutcome {
    /// Loop should continue to the next iteration.
    Continue,
    /// Claude submitted a verdict; loop should stop. Carries the verdict.
    Done(crate::verdict::Verdict),
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
    let workspace = cfg.pwd.to_string_lossy();
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        format!("-v={}:/home/claude/prompt.txt", prompt_path.display()),
        format!("-v={workspace}:{workspace}"),
        format!("--workdir={workspace}"),
        format!("-e=CAPSULE_WORKSPACE={workspace}"),
        format!("-v={}:/home/claude/.claude", cfg.claude_dir.display()),
    ];

    // Shadow the credentials file inside the directory mount with an isolated
    // per-run copy so the host and container never race over token rotation.
    if let Some(creds) = &cfg.credentials_file {
        args.push(format!(
            "-v={}:/home/claude/.claude/.credentials.json",
            creds.display()
        ));
    }

    // Protect the host git config from container mutations (issue #20).
    // If the workspace is a git repo, mount .git/config read-only so that
    // container processes (including Claude) cannot rewrite remote URLs or
    // other local settings back to the host.
    let git_config = cfg.pwd.join(".git").join("config");
    if git_config.exists() {
        args.push(format!(
            "-v={}:{workspace}/.git/config:ro",
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

fn stream_output(
    reader: BufReader<impl std::io::Read>,
    mut jq_stdin: impl Write,
    verbose: bool,
) -> Result<(bool, bool, Option<crate::verdict::Verdict>)> {
    let mut parser = StreamParser::new();

    for line in reader.lines() {
        let line = line.context("error reading docker stdout")?;
        parser.feed(&line);
        if verbose {
            eprintln!("{line}");
        }
        let _ = writeln!(jq_stdin, "{line}");
    }

    Ok((
        parser.auth_failed(),
        parser.submit_verdict_missing(),
        parser.verdict().cloned(),
    ))
}

/// Run one iteration: mount prompt, stream output through jq, propagate exit code.
///
/// `iteration` is used to derive a unique `--name` for the container so that a
/// registered ctrlc handler can call `docker stop <name>` on SIGINT.
/// `active_container` is a shared slot; this function writes the container name
/// before spawning and clears it after the container exits.
///
/// Returns [`IterationOutcome::Done`] when a `pass` verdict is observed in the stream.
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

    // Per-run MCP config: points `capsule mcp-serve` at the bind-mounted binary.
    const CAPSULE_CONTAINER_BIN: &str = "/usr/local/bin/capsule";
    let mcp_config = make_mcp_config(std::path::Path::new(CAPSULE_CONTAINER_BIN));
    let mut mcp_file = tempfile::Builder::new()
        .prefix("capsule-mcp-")
        .suffix(".json")
        .tempfile()
        .context("failed to create mcp config temp file")?;
    mcp_file
        .write_all(mcp_config.as_bytes())
        .context("failed to write mcp config")?;
    mcp_file.flush().context("failed to flush mcp config")?;
    let mcp_path = mcp_file.path().to_owned();

    let capsule_host_bin =
        std::env::current_exe().context("failed to resolve capsule binary path")?;

    let name = container_name_for(iteration);

    // Register the container name so the ctrlc handler can stop it.
    if let Ok(mut slot) = active_container.lock() {
        *slot = Some(name.clone());
    }

    let mut docker_args = build_docker_args(cfg, &prompt_path, &name);
    // Insert mcp mounts before the image name (last element).
    let image = docker_args
        .pop()
        .expect("docker args must end with image name");
    docker_args.push(format!(
        "--mount=type=bind,src={},dst={},readonly",
        capsule_host_bin.display(),
        CAPSULE_CONTAINER_BIN
    ));
    docker_args.push(format!(
        "--mount=type=bind,src={},dst=/home/claude/.mcp.json,readonly",
        mcp_path.display()
    ));
    docker_args.push(image);
    let docker_args = docker_args;

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
    let (auth_failed, submit_verdict_missing, verdict) =
        stream_output(reader, jq_stdin, cfg.verbose)?;

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

    if submit_verdict_missing {
        bail!(
            "The `submit_verdict` MCP tool was not registered. \
             Likely causes: the capsule binary is not on PATH inside the container, \
             or `.mcp.json` was not mounted. \
             Check that the capsule binary exists at /usr/local/bin/capsule inside the image."
        );
    }

    if !status.success() {
        bail!("container exited with code {}", status.code().unwrap_or(-1));
    }

    match verdict {
        Some(v) => Ok(IterationOutcome::Done(v)),
        None => Ok(IterationOutcome::Continue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Embedded assets ──────────────────────────────────────────────────────

    #[test]
    fn embedded_dockerfile_is_non_empty() {
        assert!(
            !DOCKERFILE.is_empty(),
            "embedded Dockerfile must not be empty"
        );
        assert!(
            DOCKERFILE.contains("FROM archlinux"),
            "Dockerfile must start from archlinux base"
        );
    }

    #[test]
    fn embedded_stream_display_jq_is_non_empty() {
        assert!(
            !STREAM_DISPLAY_JQ.is_empty(),
            "embedded stream_display.jq must not be empty"
        );
        assert!(
            STREAM_DISPLAY_JQ.contains("fromjson"),
            "jq filter must contain fromjson"
        );
    }

    // ── make_mcp_config ───────────────────────────────────────────────────────

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

    // ── container_name_for ───────────────────────────────────────────────────

    #[test]
    fn container_name_for_has_expected_format() {
        let name = container_name_for(3);
        assert!(
            name.starts_with("capsule-run-"),
            "name should start with capsule-run-: {name}"
        );
        assert!(
            name.ends_with("-3"),
            "name should end with iteration number: {name}"
        );
    }

    // ── derived_image_name ───────────────────────────────────────────────────

    #[test]
    fn derived_image_name_uses_basename_of_pwd() {
        let dir = tempfile::tempdir().expect("temp dir");
        let project_dir = dir.path().join("my-project");
        std::fs::create_dir(&project_dir).unwrap();
        let name = derived_image_name(&project_dir);
        assert_eq!(name, "capsule-my-project");
    }

    #[test]
    fn derived_image_name_handles_root_or_unnamed() {
        let name = derived_image_name(std::path::Path::new("/"));
        assert!(name.starts_with("capsule-"), "name={name}");
    }

    // ── build_derived_image (no Docker) ──────────────────────────────────────

    #[test]
    fn build_derived_image_returns_none_when_no_dockerfile() {
        let capsule_dir = tempfile::tempdir().expect("temp dir");
        let pwd = tempfile::tempdir().expect("temp dir");
        let result = build_derived_image(capsule_dir.path(), pwd.path(), false)
            .expect("should not error when Dockerfile absent");
        assert!(result.is_none(), "expected None when no Dockerfile");
    }

    // ── build_docker_args ────────────────────────────────────────────────────

    #[test]
    fn prompt_mount_is_not_read_only() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let prompt_arg = args.iter().find(|a| a.contains("prompt.txt")).unwrap();
        assert!(
            !prompt_arg.ends_with(":ro"),
            "prompt.txt must not be mounted read-only so before-each.sh can mutate it: {prompt_arg}"
        );
    }

    #[test]
    fn workspace_mounted_at_host_path_not_slash_workspace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("-v={pwd_str}:{pwd_str}")),
            "workspace must be mounted at host path, not /workspace: {joined}"
        );
        assert!(
            !joined.contains(":/workspace"),
            "must not mount workspace at /workspace: {joined}"
        );
    }

    #[test]
    fn workdir_set_to_host_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("--workdir={pwd_str}")),
            "expected --workdir set to host path in args: {joined}"
        );
    }

    #[test]
    fn capsule_workspace_env_var_set_to_host_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!("-e=CAPSULE_WORKSPACE={pwd_str}")),
            "expected -e=CAPSULE_WORKSPACE=<host-path> in args: {joined}"
        );
    }

    #[test]
    fn env_file_arg_present_when_file_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join(".env"), "FOO=bar\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            env_file: Some(dir.path().join(".env")),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("--env-file"),
            "expected --env-file in args: {joined}"
        );
        assert!(
            joined.contains(".env"),
            "expected .env path in args: {joined}"
        );
    }

    #[test]
    fn env_file_arg_absent_when_no_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains("--env-file"),
            "expected no --env-file when env_file is None: {joined}"
        );
    }

    #[test]
    fn gh_token_env_file_passed_when_present() {
        let dir = tempfile::tempdir().expect("temp dir");
        let token_file = dir.path().join("gh-token.env");
        std::fs::write(&token_file, "GH_TOKEN=ghs_testtoken\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            gh_token_env_file: Some(token_file.clone()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("--env-file"),
            "expected --env-file for gh token: {joined}"
        );
        assert!(
            joined.contains("gh-token.env"),
            "expected token file path in args: {joined}"
        );
    }

    #[test]
    fn gh_token_not_in_docker_args_when_env_file_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains("GH_TOKEN"),
            "token must not appear in docker args: {joined}"
        );
    }

    #[test]
    fn gh_token_never_appears_inline_in_docker_args() {
        // The only valid path for the token value is via --env-file.
        let dir = tempfile::tempdir().expect("temp dir");
        let token_file = dir.path().join("gh-token.env");
        std::fs::write(&token_file, "GH_TOKEN=ghs_secret\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            gh_token_env_file: Some(token_file),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        for arg in &args {
            assert!(
                !arg.contains("ghs_secret"),
                "token value must not appear inline: {arg}"
            );
        }
    }

    #[test]
    fn git_config_mounted_readonly_when_present() {
        let dir = tempfile::tempdir().expect("temp dir");
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            "[core]\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        let pwd_str = dir.path().to_string_lossy();
        assert!(
            joined.contains(&format!(".git/config:{pwd_str}/.git/config:ro")),
            "expected read-only git config mount at host path in args: {joined}"
        );
    }

    #[test]
    fn git_config_mount_absent_when_no_git_dir() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains(".git/config"),
            "expected no git config mount when .git/config absent: {joined}"
        );
    }

    #[test]
    fn git_identity_env_vars_present_in_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            git_author_name: "Bob Builder".to_string(),
            git_author_email: "bob@example.com".to_string(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("GIT_AUTHOR_NAME=Bob Builder"),
            "expected GIT_AUTHOR_NAME: {joined}"
        );
        assert!(
            joined.contains("GIT_AUTHOR_EMAIL=bob@example.com"),
            "expected GIT_AUTHOR_EMAIL: {joined}"
        );
        assert!(
            joined.contains("GIT_COMMITTER_NAME=Bob Builder"),
            "expected GIT_COMMITTER_NAME: {joined}"
        );
        assert!(
            joined.contains("GIT_COMMITTER_EMAIL=bob@example.com"),
            "expected GIT_COMMITTER_EMAIL: {joined}"
        );
    }

    #[test]
    fn git_identity_env_vars_present_when_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("GIT_AUTHOR_NAME="),
            "expected GIT_AUTHOR_NAME= in args: {joined}"
        );
        assert!(
            joined.contains("GIT_AUTHOR_EMAIL="),
            "expected GIT_AUTHOR_EMAIL= in args: {joined}"
        );
    }

    #[test]
    fn before_each_mounted_when_path_provided() {
        let dir = tempfile::tempdir().expect("temp dir");
        let before_each = dir.path().join("before-each.sh");
        std::fs::write(&before_each, "#!/bin/sh\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            before_each_path: Some(before_each.clone()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("/home/claude/before-each.sh:ro"),
            "expected before-each.sh mount in args: {joined}"
        );
        assert!(
            joined.contains(before_each.to_string_lossy().as_ref()),
            "expected host path in mount: {joined}"
        );
    }

    #[test]
    fn before_each_not_mounted_when_absent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains("before-each.sh"),
            "before-each.sh must not appear in args when path is None: {joined}"
        );
    }

    #[test]
    fn model_arg_present_when_model_set() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            model: Some("claude-opus-4-6".to_string()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("-e=CAPSULE_MODEL=claude-opus-4-6"),
            "expected -e=CAPSULE_MODEL=claude-opus-4-6 in args: {joined}"
        );
    }

    #[test]
    fn model_arg_absent_when_no_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains("CAPSULE_MODEL"),
            "CAPSULE_MODEL must not appear in args when model is None: {joined}"
        );
    }

    #[test]
    fn verbose_flag_not_added_to_docker_args() {
        // verbose is host-side behavior; it must not add extra docker flags.
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg_verbose = RunConfig {
            pwd: dir.path().to_path_buf(),
            verbose: true,
            ..RunConfig::default()
        };
        let cfg_quiet = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args_verbose = build_docker_args(&cfg_verbose, prompt_file.path(), "capsule-test");
        let args_quiet = build_docker_args(&cfg_quiet, prompt_file.path(), "capsule-test");
        assert_eq!(
            args_verbose, args_quiet,
            "verbose flag must not alter docker args"
        );
    }

    #[test]
    fn container_name_present_in_docker_args() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-run-12345-1");
        let joined = args.join(" ");
        assert!(
            joined.contains("--name capsule-run-12345-1"),
            "expected --name in args: {joined}"
        );
    }

    #[test]
    fn compose_network_arg_present_when_set() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            compose_network: Some("myproject_default".to_string()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains("--network myproject_default"),
            "expected --network in args: {joined}"
        );
    }

    #[test]
    fn compose_network_arg_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains("--network"),
            "expected no --network when compose_network is None: {joined}"
        );
    }

    #[test]
    fn credentials_file_shadowed_over_claude_dir_mount() {
        let dir = tempfile::tempdir().expect("temp dir");
        let creds_file = tempfile::NamedTempFile::new().unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            credentials_file: Some(creds_file.path().to_path_buf()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains(":/home/claude/.claude/.credentials.json"),
            "expected credentials shadow mount in args: {joined}"
        );
        assert!(
            joined.contains(creds_file.path().to_string_lossy().as_ref()),
            "expected temp credentials path in mount: {joined}"
        );
    }

    #[test]
    fn credentials_file_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            credentials_file: None,
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            !joined.contains(".credentials.json"),
            "expected no credentials mount when credentials_file is None: {joined}"
        );
    }

    #[test]
    fn claude_dir_mounted_at_home_claude_dot_claude() {
        let dir = tempfile::tempdir().expect("temp dir");
        let claude_dir = tempfile::tempdir().expect("claude temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            claude_dir: claude_dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let joined = args.join(" ");
        assert!(
            joined.contains(":/home/claude/.claude"),
            "expected ~/.claude mount in args: {joined}"
        );
        assert!(
            joined.contains(claude_dir.path().to_string_lossy().as_ref()),
            "expected host claude_dir path in mount: {joined}"
        );
    }
}
