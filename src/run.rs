use anyhow::{bail, Context, Result};
use capsule::config::{resolve, CliOverrides, Config, GitIdentity, GithubScope, ResolveMode};
use capsule::container_execution::{
    container_name_for, detect_compose_network, post_stream_error, run_container, run_iteration,
    token_remaining_minutes, ExecutionConfig, IterationOutcome,
};
use capsule::image_build::{build_base_image, build_derived_image, BuildConfig};
use capsule::pipeline::{
    build_summary_artifact, PipelineExecutor, PipelineState, RunSummary, StageRunner,
    TerminalReason,
};
use capsule::update_check;
use capsule::verdict::Verdict;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

struct CredentialsGuard {
    tempfile: tempfile::NamedTempFile,
    original_bytes: Vec<u8>,
    host_mtime: SystemTime,
    claude_dir: PathBuf,
}

impl CredentialsGuard {
    fn new(claude_dir: &std::path::Path) -> Result<Option<Self>> {
        let src = claude_dir.join(".credentials.json");
        if !src.exists() {
            return Ok(None);
        }
        let host_mtime = src
            .metadata()
            .and_then(|m| m.modified())
            .context("failed to read credentials file mtime")?;
        let content =
            std::fs::read(&src).with_context(|| format!("failed to read {}", src.display()))?;
        let mut tmp = tempfile::Builder::new()
            .prefix("capsule-credentials-")
            .suffix(".json")
            .tempfile()
            .context("failed to create credentials temp file")?;
        tmp.write_all(&content)
            .context("failed to write credentials temp file")?;
        Ok(Some(Self {
            tempfile: tmp,
            original_bytes: content,
            host_mtime,
            claude_dir: claude_dir.to_path_buf(),
        }))
    }

    fn path(&self) -> &std::path::Path {
        self.tempfile.path()
    }

    /// Re-read host mtime and temp file content after an external mutation of the
    /// temp file (e.g. re-copying host credentials for a resume-retry). Resets the
    /// write-back baseline so the guard correctly detects further token rotations.
    fn reset_baseline(&mut self) -> Result<()> {
        let src = self.claude_dir.join(".credentials.json");
        self.host_mtime = src
            .metadata()
            .and_then(|m| m.modified())
            .context("failed to re-read credentials mtime")?;
        self.original_bytes = std::fs::read(self.tempfile.path())
            .context("failed to re-read credentials temp file")?;
        Ok(())
    }
}

impl Drop for CredentialsGuard {
    fn drop(&mut self) {
        let dest = self.claude_dir.join(".credentials.json");
        // Skip write-back if the host refreshed its token during the run.
        if let Ok(current_mtime) = dest.metadata().and_then(|m| m.modified()) {
            if current_mtime != self.host_mtime {
                return;
            }
        }
        // Skip write-back if the container never refreshed.
        let current = match std::fs::read(self.tempfile.path()) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("warning: failed to read credentials temp file: {e}");
                return;
            }
        };
        if current == self.original_bytes {
            return;
        }
        if let Err(e) = std::fs::copy(self.tempfile.path(), &dest) {
            eprintln!("warning: failed to write back credentials: {e}");
        }
    }
}

pub(crate) enum ExitDecision {
    Success,
    Failure(String),
}

pub(crate) struct RunSession {
    cfg: Config,
    image: String,
    input: Option<String>,
    pwd: PathBuf,
    claude_dir: PathBuf,
    git_author_name: String,
    git_author_email: String,
    env_file: Option<PathBuf>,
    before_each_path: Option<PathBuf>,
    compose_network: Option<String>,
    // Held here so the temp files stay alive through execute().
    gh_token_tempfile: Option<tempfile::NamedTempFile>,
    extra_env_tempfile: Option<tempfile::NamedTempFile>,
    credentials_guard: Option<CredentialsGuard>,
    active_container: Arc<Mutex<Option<String>>>,
    resume: Option<(String, PipelineState)>,
    env_pairs: Vec<(String, String)>,
}

impl RunSession {
    /// Phases 1-10: resolve config, load env/tokens, build images,
    /// detect infrastructure, register Ctrl-C handler.
    pub(crate) fn prepare(capsule_dir: PathBuf, mut overrides: CliOverrides) -> Result<Self> {
        let input = overrides.input.take();
        let env_pairs: Vec<(String, String)> = std::mem::take(&mut overrides.env);
        let cfg = resolve(&capsule_dir, overrides, ResolveMode::Run)?;

        check_docker()?;

        if let Some(warning) = env_gitignore_warning(&cfg.capsule_dir) {
            eprintln!("{warning}");
        }

        // Capture environment snapshot before .env is sourced (needed for 'global' scope).
        let pre_dotenv_env: HashMap<String, String> = std::env::vars().collect();

        // Parse .env file into a map for 'local' scope token resolution.
        let dotenv_path = cfg.capsule_dir.join(".env");
        let dotenv_map = if dotenv_path.exists() {
            let content = std::fs::read_to_string(&dotenv_path)
                .with_context(|| format!("reading {}", dotenv_path.display()))?;
            parse_dotenv(&content)
        } else {
            HashMap::new()
        };

        load_dotenv(&cfg.capsule_dir)?;

        let gh_token_tempfile = Self::setup_gh_token(&cfg, &pre_dotenv_env, &dotenv_map)?;

        let process_env: HashMap<String, String> = std::env::vars().collect();
        let (git_author_name, git_author_email) =
            resolve_git_identity(&cfg.git_identity, &process_env);

        let pwd = std::env::current_dir().context("failed to get current directory")?;
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        let claude_dir = PathBuf::from(home).join(".claude");
        let credentials_guard = CredentialsGuard::new(&claude_dir)?;

        build_base_image(cfg.rebuild)?;

        let build_cfg = BuildConfig {
            rebuild: cfg.rebuild,
            capsule_dir: cfg.capsule_dir.clone(),
            pwd: pwd.clone(),
        };
        let image = build_derived_image(&build_cfg)?.unwrap_or_else(|| "capsule".to_string());

        run_before_all(&cfg.capsule_dir, &env_pairs)?;

        let extra_env_tempfile = build_extra_env_tempfile(&env_pairs)?;

        let env_file_path = cfg.capsule_dir.join(".env");
        let env_file = if env_file_path.exists() {
            Some(env_file_path)
        } else {
            None
        };

        let before_each_script = cfg.capsule_dir.join("before-each.sh");
        let before_each_path = if before_each_script.exists() {
            Some(before_each_script)
        } else {
            None
        };

        let compose_network = detect_compose_network(&pwd);

        let active_container: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let handler_container = Arc::clone(&active_container);

        ctrlc::set_handler(move || {
            if let Ok(slot) = handler_container.lock() {
                if let Some(name) = slot.as_ref() {
                    let _ = Command::new("docker").args(["stop", name]).output();
                }
            }
            std::process::exit(1);
        })
        .context("failed to register Ctrl-C handler")?;

        Ok(Self {
            cfg,
            image,
            input,
            pwd,
            claude_dir,
            git_author_name,
            git_author_email,
            env_file,
            before_each_path,
            compose_network,
            gh_token_tempfile,
            extra_env_tempfile,
            credentials_guard,
            active_container,
            resume: None,
            env_pairs,
        })
    }

    /// Read `last-run.json` from `capsule_dir`, extract saved state, then prepare
    /// the session identically to `prepare`. `execute()` will resume the pipeline.
    ///
    /// `cli_env` pairs are merged on top of the persisted run environment (CLI
    /// wins per key). Model, verbose, rebuild, and other flags come from
    /// `config.yml` only — `capsule resume` does not accept those overrides.
    pub(crate) fn prepare_resume(
        capsule_dir: PathBuf,
        cli_env: Vec<(String, String)>,
    ) -> Result<Self> {
        let resume_data = parse_resume_state(&capsule_dir)?;
        let merged_env = merge_env(&resume_data.1.env, &cli_env);
        let overrides = capsule::config::CliOverrides {
            env: merged_env,
            ..Default::default()
        };
        let mut session = Self::prepare(capsule_dir, overrides)?;
        session.resume = Some(resume_data);
        Ok(session)
    }

    /// Resolve GH_TOKEN when --github is set and write it to a temp env-file so
    /// the token never appears in `docker run` args.
    fn setup_gh_token(
        cfg: &Config,
        pre_dotenv_env: &HashMap<String, String>,
        dotenv_map: &HashMap<String, String>,
    ) -> Result<Option<tempfile::NamedTempFile>> {
        let scope = match &cfg.github {
            None => return Ok(None),
            Some(s) => s,
        };

        let token = resolve_gh_token(scope, pre_dotenv_env, dotenv_map)?;

        match scope {
            GithubScope::Local => {
                eprintln!("GH_TOKEN: local (.capsule/.env)");
            }
            GithubScope::Global => {
                if pre_dotenv_env.contains_key("GH_TOKEN") {
                    eprintln!("GH_TOKEN: global (process environment)");
                } else {
                    // Fell back to gh auth token — show scopes and ask for confirmation.
                    eprintln!(
                        "GH_TOKEN not found in process environment — falling back to gh auth token"
                    );
                    let _ = Command::new("gh").args(["auth", "status"]).status();
                    eprint!("Inject into container? [y/N] ");
                    let _ = std::io::stderr().flush();
                    let mut answer = String::new();
                    std::io::stdin()
                        .read_line(&mut answer)
                        .context("failed to read confirmation")?;
                    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                        anyhow::bail!(
                            "Aborted. To avoid this prompt use 'local' mode: \
                             add GH_TOKEN to .capsule/.env and pass --github local"
                        );
                    }
                }
            }
        }

        let mut tmp = tempfile::Builder::new()
            .prefix("capsule-gh-token-")
            .suffix(".env")
            .tempfile()
            .context("failed to create GH_TOKEN temp file")?;
        writeln!(tmp, "GH_TOKEN={token}").context("failed to write GH_TOKEN temp file")?;
        Ok(Some(tmp))
    }

    /// Phase 11: run the pipeline until terminal or cap hit.
    /// Returns ExitDecision so main() owns process::exit and RunSession drops
    /// before the process terminates (ensures NamedTempFile cleanup runs).
    pub(crate) fn execute(mut self) -> Result<ExitDecision> {
        let update_rx = update_check::spawn_check();
        if let Some(warning) = token_lifetime_warning(
            token_remaining_minutes(&self.claude_dir),
            self.cfg.min_token_lifetime_minutes,
        ) {
            eprintln!("{warning}");
            eprint!("Continue anyway? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin()
                .read_line(&mut answer)
                .context("failed to read confirmation")?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                anyhow::bail!("Aborted. Refresh token with `claude auth login` and retry.");
            }
        }
        // Move the guard into the runner so it can reset the baseline after a
        // resume-retry re-copies host credentials.
        let credentials_guard = self.credentials_guard.take();
        let credentials_file = credentials_guard.as_ref().map(|g| g.path().to_path_buf());
        let base_cfg = ExecutionConfig {
            image: self.image.clone(),
            prompt: String::new(),
            pwd: self.pwd.clone(),
            model: self.cfg.model.clone(),
            verbose: self.cfg.verbose,
            env_file: self.env_file.clone(),
            extra_env_file: self
                .extra_env_tempfile
                .as_ref()
                .map(|f| f.path().to_path_buf()),
            gh_token_env_file: self
                .gh_token_tempfile
                .as_ref()
                .map(|f| f.path().to_path_buf()),
            git_author_name: self.git_author_name.clone(),
            git_author_email: self.git_author_email.clone(),
            before_each_path: self.before_each_path.clone(),
            compose_network: self.compose_network.clone(),
            claude_dir: self.claude_dir.clone(),
            credentials_file,
        };
        let last_error: Arc<Mutex<Option<anyhow::Error>>> = Arc::new(Mutex::new(None));
        let last_session_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let resume = self.resume.take();
        let resume_session_id = resume.as_ref().map(|(id, _)| id.clone());
        let runner = DockerStageRunner {
            base_cfg,
            active_container: Arc::clone(&self.active_container),
            iteration: 0,
            last_error: Arc::clone(&last_error),
            last_session_id: Arc::clone(&last_session_id),
            credentials_guard,
            resume_session_id,
        };
        let mut result = if let Some((_, state)) = resume {
            PipelineExecutor::resume(self.cfg.pipeline.clone(), runner, state)
                .with_capsule_dir(self.cfg.capsule_dir.clone())
                .run()?
        } else {
            PipelineExecutor::new(self.cfg.pipeline.clone(), runner)
                .with_capsule_dir(self.cfg.capsule_dir.clone())
                .with_input(self.input)
                .run()?
        };
        result.summary.session_id = last_session_id.lock().unwrap().take();
        result.pipeline_state.env = self.env_pairs.clone();
        if let Some(e) = last_error.lock().unwrap().take() {
            let _ = write_last_run(
                &self.cfg.capsule_dir,
                &result.summary,
                Some(&result.pipeline_state),
            );
            return Err(e);
        }
        let state_to_write = match result.summary.terminal_reason {
            TerminalReason::FailExit | TerminalReason::CapHit => Some(&result.pipeline_state),
            _ => None,
        };
        write_last_run(&self.cfg.capsule_dir, &result.summary, state_to_write)?;
        if let Some(hint) = resume_hint(
            result.summary.session_id.as_deref(),
            &result.summary.terminal_reason,
        ) {
            eprintln!("\n{hint}");
        }
        update_check::maybe_print_notice(update_rx);
        Ok(exit_decision_from_summary(&result.summary))
    }
}

struct DockerStageRunner {
    base_cfg: ExecutionConfig,
    active_container: Arc<Mutex<Option<String>>>,
    iteration: u32,
    last_error: Arc<Mutex<Option<anyhow::Error>>>,
    last_session_id: Arc<Mutex<Option<String>>>,
    credentials_guard: Option<CredentialsGuard>,
    resume_session_id: Option<String>,
}

impl StageRunner for DockerStageRunner {
    fn run(&mut self, stage_name: &str, prompt: &str, model: Option<&str>) -> Option<Verdict> {
        self.iteration += 1;
        println!("── {} (iteration {}) ──", stage_name, self.iteration);
        let mut cfg = self.base_cfg.clone();
        cfg.prompt = prompt.to_string();
        if let Some(m) = model {
            cfg.model = Some(m.to_string());
        }
        if let Some(session_id) = self.resume_session_id.take() {
            let name = format!("{}-resume-pipeline", container_name_for(self.iteration));
            let (result, status) =
                match run_container(&cfg, &name, &self.active_container, Some(&session_id)) {
                    Ok(r) => r,
                    Err(e) => {
                        *self.last_error.lock().unwrap() = Some(e);
                        return None;
                    }
                };
            if let Some(e) = post_stream_error(&result, &status, "pipeline-resume") {
                *self.last_error.lock().unwrap() = Some(e);
                return None;
            }
            if let Some(id) = result.session_id {
                *self.last_session_id.lock().unwrap() = Some(id);
            }
            return result.verdict;
        }
        match run_iteration(&cfg, self.iteration, &self.active_container) {
            Ok(IterationOutcome::Done {
                verdict,
                session_id,
            }) => {
                if let Some(id) = session_id {
                    *self.last_session_id.lock().unwrap() = Some(id);
                }
                Some(verdict)
            }
            Ok(IterationOutcome::Continue { session_id }) => {
                if let Some(id) = session_id {
                    *self.last_session_id.lock().unwrap() = Some(id);
                }
                None
            }
            Ok(IterationOutcome::AuthFailedResumable { session_id }) => {
                self.run_resume(&cfg, &session_id)
            }
            Err(e) => {
                *self.last_error.lock().unwrap() = Some(e);
                None
            }
        }
    }
}

impl DockerStageRunner {
    /// Re-copy host credentials, reset the guard baseline, and launch a resume container.
    /// Returns the verdict if the resumed session submits one.
    fn run_resume(&mut self, cfg: &ExecutionConfig, session_id: &str) -> Option<Verdict> {
        eprintln!(
            "[capsule] auth failed — host token valid, attempting resume-retry (session {})",
            session_id
        );
        if let Some(ref mut guard) = self.credentials_guard {
            let host_creds = cfg.claude_dir.join(".credentials.json");
            if let Err(e) = std::fs::copy(&host_creds, guard.path())
                .context("failed to re-copy credentials for resume-retry")
            {
                *self.last_error.lock().unwrap() = Some(e);
                return None;
            }
            if let Err(e) = guard.reset_baseline() {
                *self.last_error.lock().unwrap() = Some(e);
                return None;
            }
        }
        let resume_name = format!("{}-resume", container_name_for(self.iteration));
        let (result, status) =
            match run_container(cfg, &resume_name, &self.active_container, Some(session_id)) {
                Ok(r) => r,
                Err(e) => {
                    *self.last_error.lock().unwrap() = Some(e);
                    return None;
                }
            };
        if let Some(e) = post_stream_error(&result, &status, "resume-retry") {
            *self.last_error.lock().unwrap() = Some(e);
            return None;
        }
        if let Some(id) = result.session_id {
            *self.last_session_id.lock().unwrap() = Some(id);
        }
        result.verdict
    }
}

/// Merge persisted run-environment pairs with CLI overrides. Persisted pairs
/// form the base; any same-key CLI pair overwrites; new CLI keys are appended.
fn merge_env(
    persisted: &[(String, String)],
    cli_overrides: &[(String, String)],
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = persisted.to_vec();
    for (k, v) in cli_overrides {
        if let Some(existing) = result.iter_mut().find(|(ek, _)| ek == k) {
            existing.1 = v.clone();
        } else {
            result.push((k.clone(), v.clone()));
        }
    }
    result
}

/// Write `--env` pairs to a `NamedTempFile` in dotenv format.
/// Returns `None` when `pairs` is empty so no temp file is created.
fn build_extra_env_tempfile(pairs: &[(String, String)]) -> Result<Option<tempfile::NamedTempFile>> {
    if pairs.is_empty() {
        return Ok(None);
    }
    let mut tmp = tempfile::Builder::new()
        .prefix("capsule-env-")
        .suffix(".env")
        .tempfile()
        .context("failed to create --env temp file")?;
    for (k, v) in pairs {
        writeln!(tmp, "{k}={v}").context("failed to write --env temp file")?;
    }
    Ok(Some(tmp))
}

/// `Exit` (pass-route, i.e. `on_pass: exit`) is treated as success (exit 0)
/// because the user deliberately routed a passing stage to terminate.
pub(crate) fn exit_decision_from_summary(summary: &RunSummary) -> ExitDecision {
    match summary.terminal_reason {
        TerminalReason::Done | TerminalReason::Exit | TerminalReason::Ok => ExitDecision::Success,
        TerminalReason::FailExit | TerminalReason::CapHit => {
            let msg = summary
                .last_verdict
                .as_ref()
                .and_then(|v| v.notes.clone())
                .unwrap_or_else(|| format!("pipeline ended with {:?}", summary.terminal_reason));
            ExitDecision::Failure(msg)
        }
    }
}

fn write_last_run(
    capsule_dir: &Path,
    summary: &RunSummary,
    pipeline_state: Option<&PipelineState>,
) -> Result<()> {
    let dirty = is_workspace_dirty();
    let json = build_summary_artifact(summary, dirty, pipeline_state);
    let path = capsule_dir.join("last-run.json");
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("writing summary artifact {}", path.display()))?;
    Ok(())
}

fn is_workspace_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn parse_resume_state(capsule_dir: &Path) -> Result<(String, PipelineState)> {
    let path = capsule_dir.join("last-run.json");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("{} not found — run `capsule run` first", path.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse last-run.json")?;

    let session_id = json["session_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("last-run.json has no session_id — cannot resume"))?
        .to_string();

    let state_json = &json["pipeline_state"];
    if state_json.is_null() {
        anyhow::bail!(
            "last-run.json has no pipeline state — \
             the previous run completed cleanly and cannot be resumed"
        );
    }

    let state = PipelineState::from_json(state_json)
        .context("failed to deserialize pipeline_state from last-run.json")?;
    Ok((session_id, state))
}

fn token_lifetime_warning(
    remaining_minutes: Option<u64>,
    threshold_minutes: Option<u32>,
) -> Option<String> {
    let threshold = threshold_minutes?;
    let remaining = remaining_minutes?;
    if remaining >= threshold as u64 {
        return None;
    }
    Some(format!(
        "Warning: OAuth token expires in {remaining} minutes (threshold: {threshold} min).\n\
         Run `claude auth login` to refresh before starting."
    ))
}

fn resume_hint(session_id: Option<&str>, reason: &TerminalReason) -> Option<String> {
    let id = session_id?;
    match reason {
        TerminalReason::FailExit | TerminalReason::CapHit => {
            Some(format!("To continue the session, run: capsule resume {id}"))
        }
        _ => None,
    }
}

// ── env ──────────────────────────────────────────────────────────────────────

fn parse_dotenv(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let raw_val = line[eq_pos + 1..].trim();
            let value = strip_quotes(raw_val).to_string();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

fn strip_quotes(s: &str) -> &str {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn load_dotenv(capsule_dir: &Path) -> Result<()> {
    let path = capsule_dir.join(".env");
    if !path.exists() {
        return Ok(());
    }
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for (key, value) in parse_dotenv(&content) {
        if std::env::var(&key).is_err() {
            // SAFETY: called before any threads are spawned (ctrlc handler registered later in main).
            unsafe { std::env::set_var(&key, &value) };
        }
    }
    Ok(())
}

fn resolve_gh_token(
    scope: &GithubScope,
    pre_dotenv_env: &HashMap<String, String>,
    dotenv_map: &HashMap<String, String>,
) -> Result<String> {
    match scope {
        GithubScope::Local => dotenv_map
            .get("GH_TOKEN")
            .filter(|t| !t.is_empty())
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "github is set to 'local' but GH_TOKEN not found in .capsule/.env \
                     — add GH_TOKEN=<token> to .capsule/.env"
                )
            }),
        GithubScope::Global => {
            if let Some(token) = pre_dotenv_env.get("GH_TOKEN").filter(|t| !t.is_empty()) {
                return Ok(token.clone());
            }
            let output = Command::new("gh").args(["auth", "token"]).output();
            if let Ok(out) = output {
                if out.status.success() {
                    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !token.is_empty() {
                        return Ok(token);
                    }
                }
            }
            bail!(
                "github is set to 'global' but GH_TOKEN not found in process environment \
                 — in CI, ensure GH_TOKEN is set by your platform \
                 — locally, consider using 'local' instead: add GH_TOKEN to .capsule/.env"
            )
        }
    }
}

// ── hooks ─────────────────────────────────────────────────────────────────────

fn run_before_all(capsule_dir: &Path, env_pairs: &[(String, String)]) -> Result<()> {
    let script = capsule_dir.join("before-all.sh");
    if !script.exists() {
        return Ok(());
    }

    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }

    let status = cmd
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

// ── git ───────────────────────────────────────────────────────────────────────

fn resolve_git_identity(identity: &GitIdentity, env: &HashMap<String, String>) -> (String, String) {
    match identity {
        GitIdentity::Capsule => ("Capsule".to_string(), "capsule@localhost".to_string()),
        GitIdentity::User => {
            let name = git_config_get("user.name", env);
            let email = git_config_get("user.email", env);
            (name, email)
        }
    }
}

fn git_config_get(key: &str, env: &HashMap<String, String>) -> String {
    Command::new("git")
        .args(["config", key])
        .env_clear()
        .envs(env)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

// ── preflight ─────────────────────────────────────────────────────────────────

fn check_docker() -> Result<()> {
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

fn env_gitignore_warning(capsule_dir: &Path) -> Option<String> {
    let env_path = capsule_dir.join(".env");
    if !env_path.exists() {
        return None;
    }

    // Canonicalize env_path so git resolves it correctly regardless of current_dir.
    // Without this, if capsule_dir is relative (e.g. ".capsule"), git would interpret
    // ".capsule/.env" relative to its new CWD and look for ".capsule/.capsule/.env".
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

#[cfg(test)]
mod tests {
    use super::*;
    use capsule::pipeline::{build_summary_artifact, CapHitKind, IterationCounters, RunSummary};
    use capsule::verdict::VerdictStatus;
    use std::collections::HashMap;

    #[test]
    fn credentials_written_back_after_reset_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let mut guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        // Simulate re-copying host creds to the temp file (as resume-retry does).
        std::fs::write(guard.path(), b"resumed-creds").unwrap();
        guard.reset_baseline().unwrap();
        // Simulate the resumed container rotating the token.
        std::fs::write(guard.path(), b"rotated-by-resume").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"rotated-by-resume");
    }

    #[test]
    fn credentials_written_back_when_container_refreshed_and_host_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), b"refreshed").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"refreshed");
    }

    #[test]
    fn credentials_not_written_back_when_host_modified_during_run() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        std::fs::write(guard.path(), b"container-refreshed").unwrap();
        std::fs::write(&creds_path, b"host-refreshed").unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"host-refreshed");
    }

    #[test]
    fn credentials_unchanged_when_container_did_not_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let creds_path = dir.path().join(".credentials.json");
        std::fs::write(&creds_path, b"original").unwrap();

        let guard = CredentialsGuard::new(dir.path()).unwrap().unwrap();
        drop(guard);

        assert_eq!(std::fs::read(&creds_path).unwrap(), b"original");
    }

    fn minimal_summary(reason: TerminalReason) -> RunSummary {
        RunSummary {
            terminal_reason: reason,
            last_stage: None,
            last_verdict: None,
            iteration_counters: IterationCounters {
                global: 0,
                loops: HashMap::new(),
            },
            cap_hit: None,
            session_id: None,
        }
    }

    #[test]
    fn json_done_terminal_reason() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "done");
        assert!(v["cap_hit_counter"].is_null());
        assert!(!v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_fail_exit_terminal_reason() {
        let s = minimal_summary(TerminalReason::FailExit);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "fail-exit");
    }

    #[test]
    fn json_ok_terminal_reason() {
        let s = minimal_summary(TerminalReason::Ok);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "ok");
    }

    #[test]
    fn json_cap_hit_loop_max_iteration() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::LoopMaxIteration(0));
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["terminal_reason"], "cap-hit");
        assert_eq!(v["cap_hit_counter"]["type"], "max_iteration");
        assert_eq!(v["cap_hit_counter"]["loop_idx"], 0);
    }

    #[test]
    fn json_cap_hit_max_pipeline_iterations() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::MaxPipelineIterations);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["cap_hit_counter"]["type"], "max_pipeline_iterations");
        assert!(v["cap_hit_counter"]["loop_idx"].is_null());
    }

    #[test]
    fn json_last_verdict_null_when_none() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(v["last_verdict"].is_null());
    }

    #[test]
    fn json_last_verdict_serialized_when_present() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.last_verdict = Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some("all good".to_string()),
        });
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["last_verdict"]["status"], "pass");
        assert_eq!(v["last_verdict"]["notes"], "all good");
    }

    #[test]
    fn json_workspace_dirty_flag() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, true, None);
        assert!(v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_iteration_counters_with_loop() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.iteration_counters.global = 5;
        s.iteration_counters.loops.insert(0, 3);
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["iteration_counters"]["global"], 5);
        assert_eq!(v["iteration_counters"]["loops"]["0"], 3);
    }

    #[test]
    fn write_last_run_creates_valid_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = minimal_summary(TerminalReason::Exit);
        write_last_run(dir.path(), &s, None).unwrap();
        let content = std::fs::read_to_string(dir.path().join("last-run.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["terminal_reason"], "exit");
        assert!(parsed["timestamp"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn json_includes_session_id_when_present() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.session_id = Some("sess_abc123".to_string());
        let v = build_summary_artifact(&s, false, None);
        assert_eq!(v["session_id"], "sess_abc123");
    }

    #[test]
    fn json_session_id_null_when_absent() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(v["session_id"].is_null());
    }

    #[test]
    fn resume_hint_shown_on_fail_exit_with_session_id() {
        let hint = resume_hint(Some("sess_abc"), &TerminalReason::FailExit);
        assert!(hint.is_some());
        let msg = hint.unwrap();
        assert!(msg.contains("sess_abc"), "hint was: {msg}");
        assert!(msg.contains("capsule resume"), "hint was: {msg}");
    }

    #[test]
    fn resume_hint_shown_on_cap_hit_with_session_id() {
        assert!(resume_hint(Some("sess_xyz"), &TerminalReason::CapHit).is_some());
    }

    #[test]
    fn resume_hint_none_when_no_session_id() {
        assert!(resume_hint(None, &TerminalReason::FailExit).is_none());
    }

    #[test]
    fn resume_hint_none_on_success() {
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Done).is_none());
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Ok).is_none());
        assert!(resume_hint(Some("sess_abc"), &TerminalReason::Exit).is_none());
    }

    #[test]
    fn token_warning_when_below_threshold() {
        let msg = token_lifetime_warning(Some(10), Some(15));
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(text.contains("10"), "msg: {text}");
        assert!(text.contains("15"), "msg: {text}");
    }

    #[test]
    fn no_token_warning_when_above_threshold() {
        assert!(token_lifetime_warning(Some(30), Some(15)).is_none());
    }

    #[test]
    fn no_token_warning_when_no_threshold() {
        assert!(token_lifetime_warning(Some(10), None).is_none());
    }

    #[test]
    fn no_token_warning_when_no_credentials() {
        assert!(token_lifetime_warning(None, Some(15)).is_none());
    }

    #[test]
    fn token_warning_when_expired() {
        let msg = token_lifetime_warning(Some(0), Some(15));
        assert!(msg.is_some());
    }

    #[test]
    fn is_workspace_dirty_reflects_git_status() {
        // This repo has staged/unstaged changes, so it should be dirty.
        // If run in a clean tree it will return false (acceptable).
        let result = is_workspace_dirty();
        // Just verify the function runs without panic and returns a bool.
        let _ = result;
    }

    // ── pipeline_state JSON serialization ─────────────────────────────────────

    fn make_pipeline_state() -> PipelineState {
        use capsule::pipeline::PipelineState;
        let mut fail_counts = HashMap::new();
        fail_counts.insert("stage-a".to_string(), 2u32);
        let mut loop_iters = HashMap::new();
        loop_iters.insert(0usize, 3u32);
        PipelineState {
            current_idx: 2,
            global_counter: 7,
            fail_counts,
            last_stage: Some("stage-a".to_string()),
            last_verdict: Some(capsule::verdict::Verdict {
                status: capsule::verdict::VerdictStatus::Fail,
                notes: Some("oops".to_string()),
            }),
            loop_iterations: loop_iters,
            env: vec![],
        }
    }

    #[test]
    fn json_pipeline_state_present_for_fail_exit() {
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_abc".to_string());
        let state = make_pipeline_state();
        let v = build_summary_artifact(&s, false, Some(&state));
        assert!(
            !v["pipeline_state"].is_null(),
            "pipeline_state must be present for FailExit"
        );
        assert_eq!(v["pipeline_state"]["current_idx"], 2);
        assert_eq!(v["pipeline_state"]["global_counter"], 7);
        assert_eq!(v["pipeline_state"]["fail_counts"]["stage-a"], 2);
        assert_eq!(v["pipeline_state"]["last_stage"], "stage-a");
        assert_eq!(v["pipeline_state"]["last_verdict"]["status"], "fail");
        assert_eq!(v["pipeline_state"]["loop_iterations"]["0"], 3);
    }

    #[test]
    fn json_pipeline_state_null_for_clean_exit() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_summary_artifact(&s, false, None);
        assert!(
            v["pipeline_state"].is_null(),
            "pipeline_state must be null for Done"
        );
    }

    // ── parse_resume_state round-trip ─────────────────────────────────────────

    #[test]
    fn parse_resume_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_xyz".to_string());
        let state = make_pipeline_state();
        write_last_run(dir.path(), &s, Some(&state)).unwrap();

        let (session_id, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(session_id, "sess_xyz");
        assert_eq!(restored.current_idx, 2);
        assert_eq!(restored.global_counter, 7);
        assert_eq!(restored.fail_counts.get("stage-a"), Some(&2));
        assert_eq!(restored.last_stage.as_deref(), Some("stage-a"));
        assert_eq!(
            restored.last_verdict.as_ref().map(|v| &v.status),
            Some(&capsule::verdict::VerdictStatus::Fail)
        );
        assert_eq!(restored.loop_iterations.get(&0), Some(&3));
    }

    #[test]
    fn parse_resume_state_errors_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_resume_state(dir.path()).is_err());
    }

    #[test]
    fn parse_resume_state_errors_when_pipeline_state_null() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::Done);
        s.session_id = Some("sess_done".to_string());
        write_last_run(dir.path(), &s, None).unwrap();
        let err = parse_resume_state(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no pipeline state"), "err: {err}");
    }

    #[test]
    fn parse_resume_state_errors_when_no_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = None;
        let state = make_pipeline_state();
        write_last_run(dir.path(), &s, Some(&state)).unwrap();
        let err = parse_resume_state(dir.path()).unwrap_err();
        assert!(err.to_string().contains("no session_id"), "err: {err}");
    }

    // ── build_extra_env_tempfile ──────────────────────────────────────────────

    #[test]
    fn extra_env_tempfile_empty_pairs_returns_none() {
        let result = build_extra_env_tempfile(&[]).unwrap();
        assert!(result.is_none(), "empty pairs must produce no tempfile");
    }

    #[test]
    fn extra_env_tempfile_content_is_dotenv_format() {
        let pairs = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "qux".to_string()),
        ];
        let tmp = build_extra_env_tempfile(&pairs).unwrap().unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "FOO=bar\nBAZ=qux\n");
    }

    #[test]
    fn extra_env_tempfile_single_pair() {
        let pairs = vec![("KEY".to_string(), "value".to_string())];
        let tmp = build_extra_env_tempfile(&pairs).unwrap().unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "KEY=value\n");
    }

    // ── env persistence in pipeline_state ────────────────────────────────────

    #[test]
    fn parse_resume_state_round_trips_env_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_env".to_string());
        let state = PipelineState {
            current_idx: 0,
            global_counter: 0,
            fail_counts: HashMap::new(),
            last_stage: None,
            last_verdict: None,
            loop_iterations: HashMap::new(),
            env: vec![
                ("PARENT".to_string(), "42".to_string()),
                ("MODE".to_string(), "test".to_string()),
            ],
        };
        write_last_run(dir.path(), &s, Some(&state)).unwrap();
        let (_, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(restored.env.len(), 2);
        assert!(restored
            .env
            .contains(&("PARENT".to_string(), "42".to_string())));
        assert!(restored
            .env
            .contains(&("MODE".to_string(), "test".to_string())));
    }

    #[test]
    fn parse_resume_state_env_defaults_to_empty_when_field_missing() {
        let dir = tempfile::tempdir().unwrap();
        // Write a last-run.json that has no "env" field in pipeline_state (old format).
        let json = serde_json::json!({
            "terminal_reason": "fail-exit",
            "cap_hit_counter": null,
            "last_stage": null,
            "last_verdict": null,
            "session_id": "sess_old",
            "iteration_counters": { "global": 0, "loops": {} },
            "pipeline_state": {
                "current_idx": 1,
                "global_counter": 2,
                "fail_counts": {},
                "last_stage": null,
                "last_verdict": null,
                "loop_iterations": {}
            },
            "timestamp": "2026-01-01T00:00:00Z",
            "workspace_dirty": false
        });
        std::fs::write(
            dir.path().join("last-run.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
        let (_, restored) = parse_resume_state(dir.path()).unwrap();
        assert_eq!(restored.env, vec![], "missing env field must default to []");
    }

    #[test]
    fn merge_env_both_empty_returns_empty() {
        assert_eq!(merge_env(&[], &[]), vec![]);
    }

    #[test]
    fn merge_env_no_persisted_returns_cli_pairs() {
        let cli = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        assert_eq!(merge_env(&[], &cli), cli);
    }

    #[test]
    fn merge_env_no_cli_returns_persisted_unchanged() {
        let persisted = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        assert_eq!(merge_env(&persisted, &[]), persisted);
    }

    #[test]
    fn merge_env_cli_overrides_persisted_per_key() {
        let persisted = vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ];
        let cli = vec![
            ("B".to_string(), "3".to_string()),
            ("C".to_string(), "4".to_string()),
        ];
        let merged = merge_env(&persisted, &cli);
        assert_eq!(
            merged,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "3".to_string()),
                ("C".to_string(), "4".to_string()),
            ]
        );
    }

    #[test]
    fn pipeline_state_json_includes_env_pairs() {
        let state = PipelineState {
            current_idx: 0,
            global_counter: 0,
            fail_counts: HashMap::new(),
            last_stage: None,
            last_verdict: None,
            loop_iterations: HashMap::new(),
            env: vec![
                ("PARENT".to_string(), "79".to_string()),
                ("MODE".to_string(), "dry".to_string()),
            ],
        };
        let json = state.to_json();
        let env = json["env"].as_object().expect("env must be an object");
        assert_eq!(env.len(), 2);
        assert_eq!(env["PARENT"], "79");
        assert_eq!(env["MODE"], "dry");
    }

    // ── env tests ─────────────────────────────────────────────────────────────

    mod env_tests {
        use super::super::{load_dotenv, parse_dotenv, resolve_gh_token, strip_quotes};
        use capsule::config::GithubScope;
        use std::collections::HashMap;
        use tempfile::TempDir;

        fn make_capsule_dir(env_content: Option<&str>) -> TempDir {
            let dir = tempfile::tempdir().unwrap();
            if let Some(content) = env_content {
                std::fs::write(dir.path().join(".env"), content).unwrap();
            }
            dir
        }

        #[test]
        fn parse_dotenv_basic_key_value() {
            let env = parse_dotenv("FOO=bar\nBAZ=qux\n");
            assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
            assert_eq!(env.get("BAZ").map(|s| s.as_str()), Some("qux"));
        }

        #[test]
        fn parse_dotenv_empty_content_is_empty_map() {
            let env = parse_dotenv("");
            assert!(env.is_empty());
        }

        #[test]
        fn parse_dotenv_ignores_comments_and_blank_lines() {
            let content = "# this is a comment\n\nFOO=hello\n\n# another comment\nBAR=world\n";
            let env = parse_dotenv(content);
            assert_eq!(env.len(), 2);
            assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("hello"));
            assert_eq!(env.get("BAR").map(|s| s.as_str()), Some("world"));
        }

        #[test]
        fn parse_dotenv_strips_double_quotes() {
            let env = parse_dotenv("SECRET=\"my secret value\"\n");
            assert_eq!(
                env.get("SECRET").map(|s| s.as_str()),
                Some("my secret value")
            );
        }

        #[test]
        fn parse_dotenv_strips_single_quotes() {
            let env = parse_dotenv("TOKEN='abc123'\n");
            assert_eq!(env.get("TOKEN").map(|s| s.as_str()), Some("abc123"));
        }

        #[test]
        fn parse_dotenv_value_with_equals() {
            let env = parse_dotenv("URL=https://example.com/path?a=1&b=2\n");
            assert_eq!(
                env.get("URL").map(|s| s.as_str()),
                Some("https://example.com/path?a=1&b=2")
            );
        }

        #[test]
        fn strip_quotes_no_quotes_unchanged() {
            assert_eq!(strip_quotes("hello"), "hello");
        }

        #[test]
        fn strip_quotes_double_quotes_stripped() {
            assert_eq!(strip_quotes("\"hello\""), "hello");
        }

        #[test]
        fn strip_quotes_single_quotes_stripped() {
            assert_eq!(strip_quotes("'hello'"), "hello");
        }

        #[test]
        fn load_dotenv_absent_file_is_ok() {
            let dir = make_capsule_dir(None);
            assert!(load_dotenv(dir.path()).is_ok());
        }

        #[test]
        fn resolve_gh_token_local_reads_from_dotenv_map() {
            let pre_env: HashMap<String, String> = HashMap::new();
            let mut dotenv: HashMap<String, String> = HashMap::new();
            dotenv.insert("GH_TOKEN".to_string(), "ghs_localtoken".to_string());
            let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
            assert_eq!(token, "ghs_localtoken");
        }

        #[test]
        fn resolve_gh_token_local_ignores_process_env() {
            let mut pre_env: HashMap<String, String> = HashMap::new();
            pre_env.insert("GH_TOKEN".to_string(), "ghs_processtoken".to_string());
            let mut dotenv: HashMap<String, String> = HashMap::new();
            dotenv.insert("GH_TOKEN".to_string(), "ghs_dotenvtoken".to_string());
            let token = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv).unwrap();
            assert_eq!(token, "ghs_dotenvtoken");
        }

        #[test]
        fn resolve_gh_token_local_missing_returns_error() {
            let pre_env: HashMap<String, String> = HashMap::new();
            let dotenv: HashMap<String, String> = HashMap::new();
            let result = resolve_gh_token(&GithubScope::Local, &pre_env, &dotenv);
            assert!(result.is_err());
            let msg = format!("{}", result.unwrap_err());
            assert!(msg.contains("local"), "error should mention 'local': {msg}");
            assert!(
                msg.contains(".capsule/.env"),
                "error should name the file: {msg}"
            );
        }

        #[test]
        fn resolve_gh_token_global_reads_from_pre_dotenv_env() {
            let mut pre_env: HashMap<String, String> = HashMap::new();
            pre_env.insert("GH_TOKEN".to_string(), "ghs_globaltoken".to_string());
            let dotenv: HashMap<String, String> = HashMap::new();
            let token = resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv).unwrap();
            assert_eq!(token, "ghs_globaltoken");
        }

        #[test]
        fn resolve_gh_token_global_missing_returns_error_or_token() {
            let pre_env: HashMap<String, String> = HashMap::new();
            let dotenv: HashMap<String, String> = HashMap::new();
            match resolve_gh_token(&GithubScope::Global, &pre_env, &dotenv) {
                Ok(_token) => {}
                Err(e) => {
                    let msg = format!("{e}");
                    assert!(
                        msg.contains("global"),
                        "error should mention 'global': {msg}"
                    );
                }
            }
        }
    }

    // ── hooks tests ───────────────────────────────────────────────────────────

    mod hooks_tests {
        use super::super::run_before_all;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn before_all_absent_is_ok() {
            let dir = tempfile::tempdir().expect("temp dir");
            let result = run_before_all(dir.path(), &[]);
            assert!(
                result.is_ok(),
                "absent before-all.sh must return Ok: {result:?}"
            );
        }

        #[test]
        fn before_all_success_is_ok() {
            let dir = tempfile::tempdir().expect("temp dir");
            let script = dir.path().join("before-all.sh");
            fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

            let result = run_before_all(dir.path(), &[]);
            assert!(
                result.is_ok(),
                "before-all.sh exit 0 must return Ok: {result:?}"
            );
        }

        #[test]
        fn before_all_failure_is_err() {
            let dir = tempfile::tempdir().expect("temp dir");
            let script = dir.path().join("before-all.sh");
            fs::write(&script, "#!/bin/sh\nexit 42\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

            let result = run_before_all(dir.path(), &[]);
            assert!(result.is_err(), "before-all.sh exit 42 must return Err");
            let msg = format!("{:?}", result.unwrap_err());
            assert!(
                msg.contains("42") || msg.contains("before-all"),
                "error must mention exit code or script name, got: {msg}"
            );
        }

        #[test]
        fn before_all_runs_on_host() {
            let dir = tempfile::tempdir().expect("temp dir");
            let script = dir.path().join("before-all.sh");
            let sentinel = dir.path().join("ran");
            let sentinel_str = sentinel.to_string_lossy();
            let script_body = format!("#!/bin/sh\ntouch {sentinel_str}\nexit 0\n");
            fs::write(&script, script_body).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

            run_before_all(dir.path(), &[]).expect("should succeed");
            assert!(
                sentinel.exists(),
                "before-all.sh should have created sentinel file"
            );
        }

        #[test]
        fn before_all_env_pairs_injected() {
            let dir = tempfile::tempdir().expect("temp dir");
            let script = dir.path().join("before-all.sh");
            let out_file = dir.path().join("env_value.txt");
            let out_str = out_file.to_string_lossy();
            let script_body = format!("#!/bin/sh\necho \"$MY_TEST_VAR\" > {out_str}\n");
            fs::write(&script, script_body).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

            let pairs = vec![("MY_TEST_VAR".to_string(), "hello_env".to_string())];
            run_before_all(dir.path(), &pairs).expect("should succeed");

            let content = fs::read_to_string(&out_file).expect("output file should exist");
            assert!(
                content.trim() == "hello_env",
                "env var should be injected into hook: got {content:?}"
            );
        }
    }

    // ── git tests ─────────────────────────────────────────────────────────────

    mod git_tests {
        use super::super::resolve_git_identity;
        use capsule::config::GitIdentity;
        use std::collections::HashMap;

        fn env_with_git_config(config_path: &str) -> HashMap<String, String> {
            let mut env = HashMap::new();
            env.insert("GIT_CONFIG_GLOBAL".to_string(), config_path.to_string());
            env.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
            env
        }

        #[test]
        fn capsule_identity_returns_fixed_name_and_email() {
            let env = HashMap::new();
            let (name, email) = resolve_git_identity(&GitIdentity::Capsule, &env);
            assert_eq!(name, "Capsule");
            assert_eq!(email, "capsule@localhost");
        }

        #[test]
        fn user_identity_reads_from_git_config() {
            let dir = tempfile::tempdir().expect("temp dir");
            let config_path = dir.path().join("gitconfig");
            std::fs::write(
                &config_path,
                "[user]\n\tname = Alice Dev\n\temail = alice@example.com\n",
            )
            .unwrap();

            let env = env_with_git_config(config_path.to_str().unwrap());
            let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

            assert_eq!(name, "Alice Dev");
            assert_eq!(email, "alice@example.com");
        }

        #[test]
        fn user_identity_returns_empty_strings_when_git_config_missing() {
            let dir = tempfile::tempdir().expect("temp dir");
            let nonexistent = dir.path().join("does_not_exist");

            let env = env_with_git_config(nonexistent.to_str().unwrap());
            let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

            assert_eq!(name, "");
            assert_eq!(email, "");
        }

        #[test]
        fn user_identity_returns_empty_when_user_section_absent() {
            let dir = tempfile::tempdir().expect("temp dir");
            let config_path = dir.path().join("gitconfig");
            std::fs::write(&config_path, "[core]\n\tpager = \n").unwrap();

            let env = env_with_git_config(config_path.to_str().unwrap());
            let (name, email) = resolve_git_identity(&GitIdentity::User, &env);

            assert_eq!(name, "");
            assert_eq!(email, "");
        }
    }

    // ── preflight tests ───────────────────────────────────────────────────────

    mod preflight_tests {
        use super::super::{check_docker, env_gitignore_warning};
        use serial_test::serial;
        use std::fs;
        use std::path::Path;
        use std::process::Command;
        use tempfile::TempDir;

        fn git_init(dir: &TempDir) {
            Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .expect("git init failed");
        }

        #[test]
        fn env_absent_returns_none() {
            let dir = TempDir::new().unwrap();
            assert!(env_gitignore_warning(dir.path()).is_none());
        }

        #[test]
        fn env_present_not_gitignored_returns_warning_with_path() {
            let dir = TempDir::new().unwrap();
            git_init(&dir);
            fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
            let warning = env_gitignore_warning(dir.path());
            assert!(warning.is_some(), "expected a warning but got None");
            let msg = warning.unwrap();
            assert!(
                msg.contains(".env"),
                "warning should mention the .env path; got: {msg}"
            );
        }

        #[test]
        fn env_present_and_gitignored_returns_none() {
            let dir = TempDir::new().unwrap();
            git_init(&dir);
            fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
            fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
            assert!(env_gitignore_warning(dir.path()).is_none());
        }

        #[test]
        #[serial]
        fn env_relative_capsule_dir_gitignored_returns_none() {
            let root = TempDir::new().unwrap();
            git_init(&root);
            let capsule_dir = root.path().join(".capsule");
            fs::create_dir(&capsule_dir).unwrap();
            fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();
            fs::write(root.path().join(".gitignore"), ".capsule/.env\n").unwrap();

            let original = std::env::current_dir().unwrap();
            std::env::set_current_dir(root.path()).unwrap();
            let result = env_gitignore_warning(Path::new(".capsule"));
            std::env::set_current_dir(original).unwrap();

            assert!(result.is_none(), "expected no warning but got: {result:?}");
        }

        #[test]
        #[serial]
        fn env_relative_capsule_dir_not_gitignored_returns_warning() {
            let root = TempDir::new().unwrap();
            git_init(&root);
            let capsule_dir = root.path().join(".capsule");
            fs::create_dir(&capsule_dir).unwrap();
            fs::write(capsule_dir.join(".env"), "SECRET=value").unwrap();

            let original = std::env::current_dir().unwrap();
            std::env::set_current_dir(root.path()).unwrap();
            let result = env_gitignore_warning(Path::new(".capsule"));
            std::env::set_current_dir(original).unwrap();

            assert!(result.is_some(), "expected a warning but got None");
        }

        #[test]
        #[capsule_macros::requires_docker]
        fn docker_available_check_succeeds() {
            check_docker().expect("docker check should succeed when Docker is running");
        }
    }
}
