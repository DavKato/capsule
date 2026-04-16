use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, GitIdentity, GithubScope};
use capsule::docker::{
    build_base_image, build_derived_image, detect_compose_network, run_iteration, IterationOutcome,
    RunConfig,
};
use capsule::env::{load_dotenv, parse_dotenv, resolve_gh_token};
use capsule::git::resolve_git_identity;
use capsule::hooks::run_before_all;
use capsule::preflight::{check_docker, env_gitignore_warning};
use capsule::prompt::resolve_prompt;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, ValueEnum)]
enum CliGitIdentity {
    User,
    Capsule,
}

#[derive(Debug, Clone, ValueEnum)]
enum CliGithubScope {
    Local,
    Global,
}

#[derive(Debug, Parser)]
#[command(name = "capsule", about = "Prompt-agnostic Claude container launcher")]
struct Cli {
    /// Number of iterations to run
    #[arg(short = 'i', long)]
    iterations: Option<u32>,

    /// Path to the prompt file (default: <capsule-dir>/prompt.md)
    #[arg(short = 'p', long)]
    prompt: Option<PathBuf>,

    /// Directory containing config, prompt, and hook scripts (default: ./.capsule)
    #[arg(long, default_value = ".capsule")]
    capsule_dir: PathBuf,

    /// Force a fresh Docker image build even when one already exists
    #[arg(long)]
    rebuild: bool,

    /// Claude model to use inside the container
    #[arg(short = 'm', long)]
    model: Option<String>,

    /// Print verbose diagnostic output
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Git commit identity: host user config or a generic Capsule identity
    #[arg(long, value_enum, default_value = "user")]
    git_identity: CliGitIdentity,

    /// Inject GH_TOKEN into the container: 'local' reads from .capsule/.env,
    /// 'global' reads from process env (falls back to gh auth token).
    /// When absent, no token is injected.
    #[arg(long, value_enum)]
    github: Option<CliGithubScope>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print shell completion script to stdout
    Completion {
        /// Shell to generate completion for
        shell: Shell,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Completion { shell }) = cli.command {
        let mut cmd = Cli::command();
        generate(shell, &mut cmd, "capsule", &mut io::stdout());
        return Ok(());
    }

    let git_identity = match cli.git_identity {
        CliGitIdentity::User => Some(GitIdentity::User),
        CliGitIdentity::Capsule => Some(GitIdentity::Capsule),
    };

    let github = cli.github.map(|s| match s {
        CliGithubScope::Local => GithubScope::Local,
        CliGithubScope::Global => GithubScope::Global,
    });

    let overrides = CliOverrides {
        iterations: cli.iterations,
        prompt: cli.prompt,
        rebuild: cli.rebuild,
        model: cli.model,
        verbose: cli.verbose,
        git_identity,
        github,
    };

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cfg = resolve(&cli.capsule_dir, overrides, &env)?;

    check_docker()?;

    if let Some(warning) = env_gitignore_warning(&cfg.capsule_dir) {
        eprintln!("{warning}");
    }

    // Capture environment snapshot before .env is sourced (needed for 'global' scope).
    let pre_dotenv_env: std::collections::HashMap<String, String> = std::env::vars().collect();

    // Parse .env file into a map for 'local' scope token resolution.
    let dotenv_path = cfg.capsule_dir.join(".env");
    let dotenv_map = if dotenv_path.exists() {
        let content = std::fs::read_to_string(&dotenv_path)
            .with_context(|| format!("reading {}", dotenv_path.display()))?;
        parse_dotenv(&content)
    } else {
        std::collections::HashMap::new()
    };

    load_dotenv(&cfg.capsule_dir)?;

    // Resolve GH_TOKEN when --github flag is set; write to a temp env-file so
    // the token never appears in `docker run` args.
    let gh_token_tempfile: Option<tempfile::NamedTempFile> = match &cfg.github {
        None => None,
        Some(scope) => {
            let token = resolve_gh_token(scope, &pre_dotenv_env, &dotenv_map)?;

            // Print startup confirmation line (and optionally prompt for global fallback).
            match scope {
                GithubScope::Local => {
                    eprintln!("GH_TOKEN: local (.capsule/.env)");
                }
                GithubScope::Global => {
                    if pre_dotenv_env.contains_key("GH_TOKEN") {
                        eprintln!("GH_TOKEN: global (process environment)");
                    } else {
                        // Fell back to gh auth token — show scopes and ask for confirmation.
                        eprintln!("GH_TOKEN not found in process environment — falling back to gh auth token");
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
            Some(tmp)
        }
    };

    let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let (git_author_name, git_author_email) = resolve_git_identity(&cfg.git_identity, &process_env);

    let prompt_bytes = resolve_prompt(&cfg.capsule_dir, cfg.prompt.clone())?;
    let prompt = String::from_utf8_lossy(&prompt_bytes).into_owned();

    let pwd = std::env::current_dir().context("failed to get current directory")?;
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let claude_dir = PathBuf::from(home).join(".claude");

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

    // Shared slot for the currently-running container name.
    // The ctrlc handler reads this and calls `docker stop <name>`.
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

    for i in 1..=cfg.iterations {
        println!("── Iteration {} / {} ──", i, cfg.iterations);
        let run_cfg = RunConfig {
            image: image.clone(),
            prompt: prompt.clone(),
            pwd: pwd.clone(),
            capsule_dir: cfg.capsule_dir.clone(),
            model: cfg.model.clone(),
            verbose: cfg.verbose,
            env_file: env_file.clone(),
            gh_token_env_file: gh_token_tempfile.as_ref().map(|f| f.path().to_path_buf()),
            git_author_name: git_author_name.clone(),
            git_author_email: git_author_email.clone(),
            before_each_path: before_each_path.clone(),
            compose_network: compose_network.clone(),
            claude_dir: claude_dir.clone(),
        };
        if run_iteration(&run_cfg, i, &active_container)? == IterationOutcome::Done {
            println!("Claude signalled completion after iteration {i}. No more tasks.");
            break;
        }
    }

    Ok(())
}
