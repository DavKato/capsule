use anyhow::Result;
use capsule::config::{resolve, CliOverrides, GitIdentity};
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

    for i in 1..=cfg.iterations {
        println!("── Iteration {} / {} ──", i, cfg.iterations);
    }

    Ok(())
}
