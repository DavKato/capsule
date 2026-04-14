use anyhow::{Context, Result};
use capsule::config::{resolve, CliOverrides, GitIdentity};
use capsule::docker::{run_iteration, IterationOutcome, RunConfig};
use capsule::env::{load_dotenv, resolve_gh_token};
use capsule::preflight::env_gitignore_warning;
use capsule::prompt::resolve_prompt;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::path::PathBuf;

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

    // Preflight: warn if .env is not gitignored.
    if let Some(warning) = env_gitignore_warning(&cfg.capsule_dir) {
        eprintln!("{warning}");
    }

    // Source .env into the process environment before anything else runs.
    load_dotenv(&cfg.capsule_dir)?;

    // Resolve GH_TOKEN (post-source) for explicit container injection.
    let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let gh_token = resolve_gh_token(&process_env);

    // Resolve the prompt file (errors here exit with a clear message).
    let prompt_bytes = resolve_prompt(&cfg.capsule_dir, cfg.prompt.clone())?;
    let prompt = String::from_utf8_lossy(&prompt_bytes).into_owned();

    let image = "capsule".to_string();
    let pwd = std::env::current_dir().context("failed to get current directory")?;

    // Pass .env file path to docker run if it exists.
    let env_file_path = cfg.capsule_dir.join(".env");
    let env_file = if env_file_path.exists() {
        Some(env_file_path)
    } else {
        None
    };

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
        };
        if run_iteration(&run_cfg)? == IterationOutcome::Done {
            println!("Claude signalled completion after iteration {i}. No more tasks.");
            break;
        }
    }

    Ok(())
}
