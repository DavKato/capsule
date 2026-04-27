use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, Config, GithubScope, PipelineEntry};
use capsule::docker::{
    build_base_image, build_derived_image, container_name_for, detect_compose_network,
    post_stream_error, run_container, run_iteration, token_remaining_minutes, IterationOutcome,
    RunConfig,
};
use capsule::env::{load_dotenv, parse_dotenv, resolve_gh_token};
use capsule::git::resolve_git_identity;
use capsule::hooks::run_before_all;
use capsule::pipeline::{
    CapHitKind, PipelineExecutor, PipelineState, RunSummary, StageRunner, TerminalReason,
};
use capsule::preflight::{check_docker, env_gitignore_warning};
use capsule::prompt::{prepend_preamble, resolve_prompt};
use capsule::update_check;
use capsule::verdict::Verdict;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    // Held here so the temp file stays alive through execute().
    gh_token_tempfile: Option<tempfile::NamedTempFile>,
    credentials_guard: Option<CredentialsGuard>,
    active_container: Arc<Mutex<Option<String>>>,
    resume: Option<(String, PipelineState)>,
}

impl RunSession {
    /// Phases 1-10: resolve config, load env/tokens, build images,
    /// detect infrastructure, register Ctrl-C handler.
    pub(crate) fn prepare(capsule_dir: PathBuf, mut overrides: CliOverrides) -> Result<Self> {
        let input = overrides.input.take();
        let mut cfg = resolve(&capsule_dir, overrides)?;

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

        // For flat-form configs, the stage's prompt field holds the path string; replace it
        // with the resolved, preamble-prepended content so PipelineExecutor sees real content.
        if cfg.pipeline.is_flat_form {
            let prompt_bytes = resolve_prompt(&cfg.capsule_dir, cfg.prompt.clone())?;
            let user_prompt = String::from_utf8_lossy(&prompt_bytes).into_owned();
            let resolved = prepend_preamble(&user_prompt);
            if let Some(PipelineEntry::Loop(loop_cfg)) = cfg.pipeline.entries.first_mut() {
                if let Some(stage) = loop_cfg.stages.first_mut() {
                    stage.prompt = Some(resolved);
                }
            }
        }

        let pwd = std::env::current_dir().context("failed to get current directory")?;
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        let claude_dir = PathBuf::from(home).join(".claude");
        let credentials_guard = CredentialsGuard::new(&claude_dir)?;

        build_base_image(cfg.rebuild)?;

        let image = build_derived_image(&cfg.capsule_dir, &pwd, cfg.rebuild)?
            .unwrap_or_else(|| "capsule".to_string());

        run_before_all(&cfg.capsule_dir)?;

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
                    let _ = std::process::Command::new("docker")
                        .args(["stop", name])
                        .output();
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
            credentials_guard,
            active_container,
            resume: None,
        })
    }

    /// Read `last-run.json` from `capsule_dir`, extract saved state, then prepare
    /// the session identically to `prepare`. `execute()` will resume the pipeline.
    pub(crate) fn prepare_resume(capsule_dir: PathBuf) -> Result<Self> {
        let resume_data = parse_resume_state(&capsule_dir)?;
        let mut session = Self::prepare(capsule_dir, capsule::config::CliOverrides::default())?;
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
                    let _ = std::process::Command::new("gh")
                        .args(["auth", "status"])
                        .status();
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
        let base_cfg = RunConfig {
            image: self.image.clone(),
            prompt: String::new(),
            pwd: self.pwd.clone(),
            capsule_dir: self.cfg.capsule_dir.clone(),
            model: self.cfg.model.clone(),
            verbose: self.cfg.verbose,
            env_file: self.env_file.clone(),
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
            PipelineExecutor::resume(self.cfg.pipeline.clone(), runner, state).run()
        } else {
            PipelineExecutor::new(self.cfg.pipeline.clone(), runner)
                .with_input(self.input)
                .run()
        };
        if let Some(e) = last_error.lock().unwrap().take() {
            result.summary.session_id = last_session_id.lock().unwrap().take();
            let _ = write_last_run(
                &self.cfg.capsule_dir,
                &result.summary,
                Some(&result.pipeline_state),
            );
            return Err(e);
        }
        result.summary.session_id = last_session_id.lock().unwrap().take();
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
    base_cfg: RunConfig,
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
    fn run_resume(&mut self, cfg: &RunConfig, session_id: &str) -> Option<Verdict> {
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
    let json = build_last_run_json(summary, dirty, pipeline_state);
    let path = capsule_dir.join("last-run.json");
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("writing summary artifact {}", path.display()))?;
    Ok(())
}

fn is_workspace_dirty() -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn pipeline_state_to_json(state: &PipelineState) -> serde_json::Value {
    let fail_counts: serde_json::Map<String, serde_json::Value> = state
        .fail_counts
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::json!(v)))
        .collect();
    let loop_iterations: serde_json::Map<String, serde_json::Value> = state
        .loop_iterations
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();
    let last_verdict = state
        .last_verdict
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
    serde_json::json!({
        "current_idx": state.current_idx,
        "global_counter": state.global_counter,
        "fail_counts": fail_counts,
        "last_stage": state.last_stage,
        "last_verdict": last_verdict,
        "loop_iterations": loop_iterations,
    })
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

    let current_idx = state_json["current_idx"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("pipeline_state.current_idx missing"))?
        as usize;
    let global_counter = state_json["global_counter"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("pipeline_state.global_counter missing"))?
        as u32;
    let fail_counts: std::collections::HashMap<String, u32> = state_json["fail_counts"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as u32))
                .collect()
        })
        .unwrap_or_default();
    let last_stage = state_json["last_stage"].as_str().map(str::to_owned);
    let last_verdict: Option<capsule::verdict::Verdict> = if state_json["last_verdict"].is_null() {
        None
    } else {
        Some(
            serde_json::from_value(state_json["last_verdict"].clone())
                .context("failed to deserialize pipeline_state.last_verdict")?,
        )
    };
    let loop_iterations: std::collections::HashMap<usize, u32> = state_json["loop_iterations"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    k.parse::<usize>()
                        .ok()
                        .map(|ki| (ki, v.as_u64().unwrap_or(0) as u32))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok((
        session_id,
        PipelineState {
            current_idx,
            global_counter,
            fail_counts,
            last_stage,
            last_verdict,
            loop_iterations,
        },
    ))
}

fn build_last_run_json(
    summary: &RunSummary,
    workspace_dirty: bool,
    pipeline_state: Option<&PipelineState>,
) -> serde_json::Value {
    let terminal_reason = match summary.terminal_reason {
        TerminalReason::Done => "done",
        TerminalReason::Exit => "exit",
        TerminalReason::FailExit => "fail-exit",
        TerminalReason::CapHit => "cap-hit",
        TerminalReason::Ok => "ok",
    };

    let cap_hit_counter = match &summary.cap_hit {
        None => serde_json::Value::Null,
        Some(CapHitKind::LoopMaxIteration(idx)) => serde_json::json!({
            "type": "max_iteration",
            "loop_idx": idx,
        }),
        Some(CapHitKind::MaxPipelineIterations) => serde_json::json!({
            "type": "max_pipeline_iterations",
        }),
    };

    let last_verdict = summary
        .last_verdict
        .as_ref()
        .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));

    let loops: serde_json::Map<String, serde_json::Value> = summary
        .iteration_counters
        .loops
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();

    let ps = pipeline_state.map(pipeline_state_to_json);
    serde_json::json!({
        "terminal_reason": terminal_reason,
        "cap_hit_counter": cap_hit_counter,
        "last_stage": summary.last_stage,
        "last_verdict": last_verdict,
        "session_id": summary.session_id,
        "iteration_counters": {
            "global": summary.iteration_counters.global,
            "loops": loops,
        },
        "pipeline_state": ps,
        "timestamp": iso8601_now(),
        "workspace_dirty": workspace_dirty,
    })
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

fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Howard Hinnant's algorithm: days-from-civil
    let z = secs as i64 / 86400 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let h = (secs / 3600) % 24;
    let min = (secs / 60) % 60;
    let s = secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use capsule::pipeline::{IterationCounters, RunSummary};
    use capsule::verdict::VerdictStatus;
    use std::collections::HashMap;

    fn exit_decision(verdict: Option<&Verdict>) -> ExitDecision {
        match verdict {
            Some(v) if matches!(v.status, VerdictStatus::Pass | VerdictStatus::Done) => {
                ExitDecision::Success
            }
            Some(v) => ExitDecision::Failure(
                v.notes
                    .clone()
                    .unwrap_or_else(|| "fail verdict (no notes provided)".to_string()),
            ),
            None => {
                ExitDecision::Failure("capsule exhausted iterations without a verdict".to_string())
            }
        }
    }

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

    fn pass_verdict(notes: Option<&str>) -> Verdict {
        Verdict {
            status: VerdictStatus::Pass,
            notes: notes.map(str::to_owned),
        }
    }

    fn fail_verdict(notes: Option<&str>) -> Verdict {
        Verdict {
            status: VerdictStatus::Fail,
            notes: notes.map(str::to_owned),
        }
    }

    #[test]
    fn pass_is_success() {
        assert!(matches!(
            exit_decision(Some(&pass_verdict(None))),
            ExitDecision::Success
        ));
    }

    #[test]
    fn fail_with_notes_is_failure_containing_notes() {
        let v = fail_verdict(Some("build broke"));
        let ExitDecision::Failure(msg) = exit_decision(Some(&v)) else {
            panic!("expected Failure")
        };
        assert!(msg.contains("build broke"), "message was: {msg}");
    }

    #[test]
    fn fail_without_notes_is_failure() {
        assert!(matches!(
            exit_decision(Some(&fail_verdict(None))),
            ExitDecision::Failure(_)
        ));
    }

    #[test]
    fn no_verdict_is_implicit_fail() {
        assert!(matches!(exit_decision(None), ExitDecision::Failure(_)));
    }

    #[test]
    fn done_is_success() {
        let v = Verdict {
            status: VerdictStatus::Done,
            notes: Some("scope complete".to_string()),
        };
        assert!(matches!(exit_decision(Some(&v)), ExitDecision::Success));
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
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["terminal_reason"], "done");
        assert!(v["cap_hit_counter"].is_null());
        assert!(!v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_fail_exit_terminal_reason() {
        let s = minimal_summary(TerminalReason::FailExit);
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["terminal_reason"], "fail-exit");
    }

    #[test]
    fn json_ok_terminal_reason() {
        let s = minimal_summary(TerminalReason::Ok);
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["terminal_reason"], "ok");
    }

    #[test]
    fn json_cap_hit_loop_max_iteration() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::LoopMaxIteration(0));
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["terminal_reason"], "cap-hit");
        assert_eq!(v["cap_hit_counter"]["type"], "max_iteration");
        assert_eq!(v["cap_hit_counter"]["loop_idx"], 0);
    }

    #[test]
    fn json_cap_hit_max_pipeline_iterations() {
        let mut s = minimal_summary(TerminalReason::CapHit);
        s.cap_hit = Some(CapHitKind::MaxPipelineIterations);
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["cap_hit_counter"]["type"], "max_pipeline_iterations");
        assert!(v["cap_hit_counter"]["loop_idx"].is_null());
    }

    #[test]
    fn json_last_verdict_null_when_none() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_last_run_json(&s, false, None);
        assert!(v["last_verdict"].is_null());
    }

    #[test]
    fn json_last_verdict_serialized_when_present() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.last_verdict = Some(Verdict {
            status: VerdictStatus::Pass,
            notes: Some("all good".to_string()),
        });
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["last_verdict"]["status"], "pass");
        assert_eq!(v["last_verdict"]["notes"], "all good");
    }

    #[test]
    fn json_workspace_dirty_flag() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_last_run_json(&s, true, None);
        assert!(v["workspace_dirty"].as_bool().unwrap());
    }

    #[test]
    fn json_iteration_counters_with_loop() {
        let mut s = minimal_summary(TerminalReason::Done);
        s.iteration_counters.global = 5;
        s.iteration_counters.loops.insert(0, 3);
        let v = build_last_run_json(&s, false, None);
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
        let v = build_last_run_json(&s, false, None);
        assert_eq!(v["session_id"], "sess_abc123");
    }

    #[test]
    fn json_session_id_null_when_absent() {
        let s = minimal_summary(TerminalReason::Done);
        let v = build_last_run_json(&s, false, None);
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
        }
    }

    #[test]
    fn json_pipeline_state_present_for_fail_exit() {
        let mut s = minimal_summary(TerminalReason::FailExit);
        s.session_id = Some("sess_abc".to_string());
        let state = make_pipeline_state();
        let v = build_last_run_json(&s, false, Some(&state));
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
        let v = build_last_run_json(&s, false, None);
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
}
