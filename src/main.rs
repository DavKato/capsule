use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, ValueEnum)]
enum GitIdentity {
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
    git_identity: GitIdentity,

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

    let iterations = cli.iterations.ok_or_else(|| {
        // clap already errors before reaching here if iterations is required,
        // but this handles the case when iterations is not provided with subcommand
        anyhow::anyhow!("--iterations is required")
    })?;

    for i in 1..=iterations {
        println!("── Iteration {} / {} ──", i, iterations);
    }

    Ok(())
}
