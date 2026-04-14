use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, GitIdentity};
use capsule::docker::{
    build_base_image, build_derived_image, detect_compose_network, run_iteration, IterationOutcome,
    RunConfig,
};
use capsule::env::{load_dotenv, resolve_gh_token};
use capsule::git::resolve_git_identity;
use capsule::hooks::run_before_all;
use capsule::preflight::{check_docker, env_gitignore_warning};
use capsule::prompt::resolve_prompt;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, ValueEnum)]
enum CliGitIdentity {
    User,
    Capsule,
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

    let overrides = CliOverrides {
        iterations: cli.iterations,
        prompt: cli.prompt,
        rebuild: cli.rebuild,
        model: cli.model,
        verbose: cli.verbose,
        git_identity,
    };

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cfg = resolve(&cli.capsule_dir, overrides, &env)?;

    // Preflight: Docker daemon must be reachable before anything else.
    check_docker()?;

    // Preflight: warn if .env is not gitignored.
    if let Some(warning) = env_gitignore_warning(&cfg.capsule_dir) {
        eprintln!("{warning}");
    }

    // Source .env into the process environment before anything else runs.
    load_dotenv(&cfg.capsule_dir)?;

    // Resolve GH_TOKEN and git identity (post-source) for container injection.
    let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let gh_token = resolve_gh_token(&process_env);
    let (git_author_name, git_author_email) = resolve_git_identity(&cfg.git_identity, &process_env);

    // Resolve the prompt file (errors here exit with a clear message).
    let prompt_bytes = resolve_prompt(&cfg.capsule_dir, cfg.prompt.clone())?;
    let prompt = String::from_utf8_lossy(&prompt_bytes).into_owned();

    let pwd = std::env::current_dir().context("failed to get current directory")?;

    // Build (or skip) the base image.
    build_base_image(cfg.rebuild)?;

    // Build derived image if ${capsule_dir}/Dockerfile exists; use it for iterations.
    let image = build_derived_image(&cfg.capsule_dir, &pwd, cfg.rebuild)?
        .unwrap_or_else(|| "capsule".to_string());

    // Run before-all.sh on the host if present. Non-zero exit aborts.
    run_before_all(&cfg.capsule_dir)?;

    // Pass .env file path to docker run if it exists.
    let env_file_path = cfg.capsule_dir.join(".env");
    let env_file = if env_file_path.exists() {
        Some(env_file_path)
    } else {
        None
    };

    // Mount before-each.sh into container if present.
    let before_each_script = cfg.capsule_dir.join("before-each.sh");
    let before_each_path = if before_each_script.exists() {
        Some(before_each_script)
    } else {
        None
    };

    // Detect Docker Compose network at pwd (best-effort; None if not found).
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
            gh_token: gh_token.clone(),
            git_author_name: git_author_name.clone(),
            git_author_email: git_author_email.clone(),
            before_each_path: before_each_path.clone(),
            compose_network: compose_network.clone(),
        };
        if run_iteration(&run_cfg, i, &active_container)? == IterationOutcome::Done {
            println!("Claude signalled completion after iteration {i}. No more tasks.");
            break;
        }
    }

    Ok(())
}
