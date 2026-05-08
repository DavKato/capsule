use crate::stream_parser::StreamParser;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// The jq stream-display filter embedded at compile time.
pub const STREAM_DISPLAY_JQ: &str = include_str!("../base-image/stream_display.jq");

/// Configuration for a single iteration's `docker run`.
#[derive(Default, Clone)]
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
    /// Path to a temp env-file written from `--env KEY=VALUE` pairs.
    /// Emitted after `env_file` so user overrides take precedence over `.capsule/.env`.
    pub extra_env_file: Option<PathBuf>,
    /// Path to a temp env-file containing `GH_TOKEN=<token>` (None → no token injected).
    /// Passed as a final `--env-file` so the token never appears in the process arg list.
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
    Continue { session_id: Option<String> },
    /// Claude submitted a verdict; loop should stop.
    Done {
        verdict: crate::verdict::Verdict,
        session_id: Option<String>,
    },
    /// Auth failed but the host token is still valid; caller should re-copy
    /// credentials, call `CredentialsGuard::reset_baseline`, and retry with
    /// `run_container(..., Some(session_id))`.
    AuthFailedResumable { session_id: String },
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

    if let Some(extra_file) = &cfg.extra_env_file {
        args.push(format!("--env-file={}", extra_file.display()));
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

fn read_expires_at(claude_dir: &std::path::Path) -> Option<u64> {
    let path = claude_dir.join(".credentials.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
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

pub struct StreamResult {
    pub auth_failed: bool,
    pub submit_verdict_missing: bool,
    pub verdict: Option<crate::verdict::Verdict>,
    pub session_id: Option<String>,
}

pub fn post_stream_error(
    result: &StreamResult,
    status: &std::process::ExitStatus,
    context: &str,
) -> Option<anyhow::Error> {
    if result.auth_failed {
        return Some(anyhow::anyhow!(
            "Claude authentication failed on {context}. \
             Run `claude auth login` on the host to refresh credentials, then retry."
        ));
    }
    if result.submit_verdict_missing {
        return Some(anyhow::anyhow!(
            "The `submit_verdict` MCP tool was not registered. \
             Likely causes: the base image is stale (run `capsule run --rebuild` to force a rebuild), \
             the capsule binary is not on PATH inside the container, \
             or `.mcp.json` was not mounted."
        ));
    }
    if !status.success() {
        return Some(anyhow::anyhow!(
            "container exited with code {} during {context}",
            status.code().unwrap_or(-1)
        ));
    }
    None
}

fn stream_output(
    reader: BufReader<impl std::io::Read>,
    mut jq_stdin: impl Write,
    verbose: bool,
) -> Result<StreamResult> {
    let mut parser = StreamParser::new();

    for line in reader.lines() {
        let line = line.context("error reading docker stdout")?;
        parser.feed(&line);
        if verbose {
            eprintln!("{line}");
        }
        let _ = writeln!(jq_stdin, "{line}");
    }

    Ok(StreamResult {
        auth_failed: parser.auth_failed(),
        submit_verdict_missing: parser.submit_verdict_missing(),
        verdict: parser.verdict().cloned(),
        session_id: parser.session_id().map(str::to_owned),
    })
}

/// Shared scaffolding for one container run: temp files, docker args, MCP config,
/// jq piping, streaming, wait, and active-container slot management.
///
/// `resume_session_id` — when `Some`, adds `-e=CAPSULE_RESUME_SESSION=<id>` so the
/// entrypoint invokes `claude --resume` instead of piping `prompt.txt`.
///
/// Returns the parsed stream result and the container exit status. Callers own all
/// post-stream policy (retry, bail, verdict routing).
pub fn run_container(
    cfg: &RunConfig,
    container_name: &str,
    active_container: &Arc<Mutex<Option<String>>>,
    resume_session_id: Option<&str>,
) -> Result<(StreamResult, std::process::ExitStatus)> {
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

    if let Ok(mut slot) = active_container.lock() {
        *slot = Some(container_name.to_string());
    }

    let mut docker_args = build_docker_args(cfg, &prompt_path, container_name);
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
    if let Some(sid) = resume_session_id {
        docker_args.push(format!("-e=CAPSULE_RESUME_SESSION={sid}"));
    }
    docker_args.push(image);

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
    let result = stream_output(reader, jq_stdin, cfg.verbose)?;

    let _ = jq_child.wait();
    let status = docker_child.wait().context("docker run did not complete")?;

    if let Ok(mut slot) = active_container.lock() {
        *slot = None;
    }

    Ok((result, status))
}

/// Returns true when an auth failure can be retried via `--resume`.
///
/// All three conditions must hold: there is a captured session ID to resume,
/// a credentials file to re-copy, and the host token has not already expired.
fn should_attempt_resume(
    session_id: Option<&str>,
    has_credentials: bool,
    host_token_expired: bool,
) -> bool {
    session_id.is_some() && has_credentials && !host_token_expired
}

/// Run one iteration: mount prompt, stream output through jq, propagate exit code.
///
/// `iteration` is used to derive a unique `--name` for the container so that a
/// registered ctrlc handler can call `docker stop <name>` on SIGINT.
/// `active_container` is a shared slot; this function writes the container name
/// before spawning and clears it after the container exits.
///
/// Returns [`IterationOutcome::AuthFailedResumable`] when auth failed but the host
/// token is still valid — the caller is responsible for re-copying credentials,
/// resetting the guard baseline, and launching the resume container.
///
/// # Errors
/// - Container exits non-zero → error naming the exit code.
/// - Auth failed and host token is already expired → error with remediation hint.
pub fn run_iteration(
    cfg: &RunConfig,
    iteration: u32,
    active_container: &Arc<Mutex<Option<String>>>,
) -> Result<IterationOutcome> {
    let name = container_name_for(iteration);
    let (result, status) = run_container(cfg, &name, active_container, None)?;

    if result.auth_failed
        && should_attempt_resume(
            result.session_id.as_deref(),
            cfg.credentials_file.is_some(),
            host_token_is_expired(&cfg.claude_dir),
        )
    {
        return Ok(IterationOutcome::AuthFailedResumable {
            session_id: result
                .session_id
                .expect("session_id is Some when should_attempt_resume returns true"),
        });
    }

    if let Some(e) = post_stream_error(&result, &status, "iteration") {
        return Err(e);
    }

    match result.verdict {
        Some(verdict) => Ok(IterationOutcome::Done {
            verdict,
            session_id: result.session_id,
        }),
        None => Ok(IterationOutcome::Continue {
            session_id: result.session_id,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Embedded assets ──────────────────────────────────────────────────────

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
    fn extra_env_file_emitted_after_primary_env_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let primary = dir.path().join(".env");
        let extra = dir.path().join("extra.env");
        std::fs::write(&primary, "FOO=default\n").unwrap();
        std::fs::write(&extra, "FOO=override\n").unwrap();
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            env_file: Some(primary.clone()),
            extra_env_file: Some(extra.clone()),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        let primary_pos = args
            .iter()
            .position(|a| a.contains(".env") && !a.contains("extra"))
            .expect("primary env-file not found");
        let extra_pos = args
            .iter()
            .position(|a| a.contains("extra.env"))
            .expect("extra env-file not found");
        assert!(
            primary_pos < extra_pos,
            "primary .env must appear before extra.env (override ordering)"
        );
    }

    #[test]
    fn extra_env_file_absent_when_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        let prompt_file = tempfile::NamedTempFile::new().unwrap();
        let cfg = RunConfig {
            pwd: dir.path().to_path_buf(),
            ..RunConfig::default()
        };
        let args = build_docker_args(&cfg, prompt_file.path(), "capsule-test");
        // Without extra_env_file only the primary --env-file (if any) may appear.
        // Since env_file is also None here, no --env-file at all.
        let joined = args.join(" ");
        assert!(
            !joined.contains("extra"),
            "extra env-file must not appear when None: {joined}"
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

    // ── should_attempt_resume ─────────────────────────────────────────────────

    #[test]
    fn resume_attempted_when_session_id_valid_token_and_credentials() {
        assert!(should_attempt_resume(Some("sess_01"), true, false));
    }

    #[test]
    fn resume_not_attempted_when_host_token_expired() {
        assert!(!should_attempt_resume(Some("sess_01"), true, true));
    }

    #[test]
    fn resume_not_attempted_when_no_session_id() {
        assert!(!should_attempt_resume(None, true, false));
    }

    #[test]
    fn resume_not_attempted_when_no_credentials_file() {
        assert!(!should_attempt_resume(Some("sess_01"), false, false));
    }

    #[test]
    fn host_token_expired_when_expires_at_in_past() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        // expiresAt = 1000 (epoch ms) → way in the past
        std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":1000}}"#).unwrap();
        assert!(host_token_is_expired(dir.path()));
    }

    #[test]
    fn host_token_not_expired_when_expires_at_in_future() {
        let dir = tempfile::tempdir().unwrap();
        let creds = dir.path().join(".credentials.json");
        // expiresAt far in the future (year ~2050)
        std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":2524608000000}}"#).unwrap();
        assert!(!host_token_is_expired(dir.path()));
    }

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
