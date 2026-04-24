use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, Config, GithubScope};
use capsule::docker::{
    build_base_image, build_derived_image, detect_compose_network, run_iteration, IterationOutcome,
    RunConfig,
};
use capsule::env::{load_dotenv, parse_dotenv, resolve_gh_token};
use capsule::git::resolve_git_identity;
use capsule::hooks::run_before_all;
use capsule::preflight::{check_docker, env_gitignore_warning};
use capsule::prompt::{prepend_preamble, resolve_prompt};
use capsule::update_check;
use capsule::verdict::{Verdict, VerdictStatus};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
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

pub(crate) fn exit_decision(verdict: Option<&Verdict>) -> ExitDecision {
    match verdict {
        Some(v) if v.status == VerdictStatus::Pass => ExitDecision::Success,
        Some(v) => ExitDecision::Failure(
            v.notes
                .clone()
                .unwrap_or_else(|| "fail verdict (no notes provided)".to_string()),
        ),
        None => ExitDecision::Failure("capsule exhausted iterations without a verdict".to_string()),
    }
}

pub(crate) struct RunSession {
    cfg: Config,
    image: String,
    prompt: String,
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
}

impl RunSession {
    /// Phases 1-10: resolve config, load env/tokens, build images,
    /// detect infrastructure, register Ctrl-C handler.
    pub(crate) fn prepare(capsule_dir: PathBuf, overrides: CliOverrides) -> Result<Self> {
        let cfg = resolve(&capsule_dir, overrides)?;

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

        let prompt_bytes = resolve_prompt(&cfg.capsule_dir, cfg.prompt.clone())?;
        let user_prompt = String::from_utf8_lossy(&prompt_bytes).into_owned();
        let prompt = prepend_preamble(&user_prompt);

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
            prompt,
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
        })
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

    /// Phase 11: run the iteration loop until Done or iterations exhausted.
    /// Returns ExitDecision so main() owns process::exit and RunSession drops
    /// before the process terminates (ensures NamedTempFile cleanup runs).
    pub(crate) fn execute(self) -> Result<ExitDecision> {
        let update_rx = update_check::spawn_check();
        let mut final_verdict: Option<Verdict> = None;
        for i in 1..=self.cfg.iterations {
            println!("── Iteration {} / {} ──", i, self.cfg.iterations);
            let run_cfg = RunConfig {
                image: self.image.clone(),
                prompt: self.prompt.clone(),
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
                credentials_file: self
                    .credentials_guard
                    .as_ref()
                    .map(|g| g.path().to_path_buf()),
            };
            if let IterationOutcome::Done(verdict) =
                run_iteration(&run_cfg, i, &self.active_container)?
            {
                final_verdict = Some(verdict);
                break;
            }
        }
        update_check::maybe_print_notice(update_rx);
        Ok(exit_decision(final_verdict.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
