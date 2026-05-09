use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, Config, ResolveMode};
use capsule::container_execution::{
    detect_compose_network, token_remaining_minutes, CredentialsGuard, DockerStageRunner,
    ExecutionConfig,
};
use capsule::image_build::{build_base_image, build_derived_image, BuildConfig};
use capsule::pipeline::{PipelineExecutor, PipelineState, TerminalReason};
use capsule::update_check;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

mod env;
mod git;
mod summary;

pub(crate) enum ExitDecision {
    Success,
    Failure,
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

        if let Some(warning) = env::env_gitignore_warning(&cfg.capsule_dir) {
            eprintln!("{warning}");
        }

        // Capture environment snapshot before .env is sourced (needed for 'global' scope).
        let pre_dotenv_env: HashMap<String, String> = std::env::vars().collect();

        // Parse .env file into a map for 'local' scope token resolution.
        let dotenv_path = cfg.capsule_dir.join(".env");
        let dotenv_map = if dotenv_path.exists() {
            let content = std::fs::read_to_string(&dotenv_path)
                .with_context(|| format!("reading {}", dotenv_path.display()))?;
            env::parse_dotenv(&content)
        } else {
            HashMap::new()
        };

        env::load_dotenv(&cfg.capsule_dir)?;

        let gh_token_tempfile = env::setup_gh_token(&cfg, &pre_dotenv_env, &dotenv_map)?;

        let process_env: HashMap<String, String> = std::env::vars().collect();
        let (git_author_name, git_author_email) =
            git::resolve_git_identity(&cfg.git_identity, &process_env);

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

        let extra_env_tempfile = env::build_extra_env_tempfile(&env_pairs)?;

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
        let resume_data = summary::parse_resume_state(&capsule_dir)?;
        let merged_env = env::merge_env(&resume_data.1.env, &cli_env);
        let overrides = capsule::config::CliOverrides {
            env: merged_env,
            ..Default::default()
        };
        let mut session = Self::prepare(capsule_dir, overrides)?;
        session.resume = Some(resume_data);
        Ok(session)
    }

    /// Phase 11: run the pipeline until terminal or cap hit.
    /// Returns ExitDecision so main() owns process::exit and RunSession drops
    /// before the process terminates (ensures NamedTempFile cleanup runs).
    pub(crate) fn execute(mut self) -> Result<ExitDecision> {
        let update_rx = update_check::spawn_check();
        if let Some(warning) = env::token_lifetime_warning(
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
        let resume = self.resume.take();
        let resume_session_id = resume.as_ref().map(|(id, _)| id.clone());
        let runner = DockerStageRunner::new(
            base_cfg,
            Arc::clone(&self.active_container),
            credentials_guard,
            resume_session_id,
        );
        let (mut result, runner) = if let Some((_, state)) = resume {
            PipelineExecutor::resume(self.cfg.pipeline.clone(), runner, state)
                .with_capsule_dir(self.cfg.capsule_dir.clone())
                .run()?
        } else {
            PipelineExecutor::new(self.cfg.pipeline.clone(), runner)
                .with_capsule_dir(self.cfg.capsule_dir.clone())
                .with_input(self.input)
                .run()?
        };
        result.summary.session_id = runner.session_id().map(String::from);
        result.pipeline_state.env = self.env_pairs.clone();
        let state_to_write = match result.summary.terminal_reason {
            TerminalReason::FailExit | TerminalReason::CapHit => Some(&result.pipeline_state),
            _ => None,
        };
        summary::write_last_run(&self.cfg.capsule_dir, &result.summary, state_to_write)?;
        if let Some(hint) = summary::resume_hint(
            result.summary.session_id.as_deref(),
            &result.summary.terminal_reason,
        ) {
            eprintln!("\n{hint}");
        }
        update_check::maybe_print_notice(update_rx);
        Ok(summary::exit_decision_from_summary(&result.summary))
    }
}

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

/// Runs `before-all.sh` if present. Absent script is Ok; non-zero exit is Err.
fn run_before_all(capsule_dir: &std::path::Path, env_pairs: &[(String, String)]) -> Result<()> {
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
        anyhow::bail!(
            "before-all.sh exited with code {}",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_docker, run_before_all};
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

    #[test]
    #[capsule_macros::requires_docker]
    fn docker_available_check_succeeds() {
        check_docker().expect("docker check should succeed when Docker is running");
    }
}
