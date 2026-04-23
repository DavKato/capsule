use anyhow::Result;
use capsule::config::{CliOverrides, GitIdentity, GithubScope};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::path::PathBuf;

mod run;
use capsule::mcp_server;
use run::RunSession;

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
#[command(
    name = "capsule",
    about = "Prompt-agnostic Claude container launcher",
    subcommand_required = true,
    arg_required_else_help = true,
    version,
    disable_version_flag = true
)]
struct Cli {
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the Claude iteration loop
    Run {
        /// Number of iterations to run
        #[arg(short = 'i', long)]
        iterations: Option<u32>,

        /// Path to the prompt file (default: <capsule-dir>/prompt.md)
        #[arg(short = 'p', long)]
        prompt: Option<PathBuf>,

        /// Directory containing config, prompt, and hook scripts (default: ./.capsule)
        #[arg(long, default_value = ".capsule")]
        capsule_dir: PathBuf,

        /// Force a clean rebuild, bypassing the layer cache
        #[arg(long)]
        rebuild: bool,

        /// Claude model to use inside the container
        #[arg(short = 'm', long)]
        model: Option<String>,

        /// Print verbose diagnostic output
        #[arg(long)]
        verbose: bool,

        /// Git commit identity: host user config or a generic Capsule identity
        #[arg(long, value_enum, default_value = "user")]
        git_identity: CliGitIdentity,

        /// Inject GH_TOKEN into the container: 'local' reads from .capsule/.env,
        /// 'global' reads from process env (falls back to gh auth token).
        /// When absent, no token is injected.
        #[arg(long, value_enum)]
        github: Option<CliGithubScope>,
    },

    /// Print shell completion script to stdout
    Completion {
        /// Shell to generate completion for
        shell: Shell,
    },

    /// Download and install the latest capsule release
    Update,

    /// Run the MCP server over stdio (used inside the container by Claude Code)
    #[command(hide = true)]
    McpServe,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "capsule", &mut io::stdout());
            Ok(())
        }
        Commands::Update => {
            let curl = std::process::Command::new("curl")
                .args([
                    "-fsSL",
                    "https://raw.githubusercontent.com/DavKato/capsule/main/install.sh",
                ])
                .stdout(std::process::Stdio::piped())
                .spawn()?;
            let status = std::process::Command::new("bash")
                .stdin(curl.stdout.unwrap())
                .status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        Commands::McpServe => {
            mcp_server::run_server();
            Ok(())
        }
        Commands::Run {
            iterations,
            prompt,
            capsule_dir,
            rebuild,
            model,
            verbose,
            git_identity,
            github,
        } => {
            let git_identity = match git_identity {
                CliGitIdentity::User => Some(GitIdentity::User),
                CliGitIdentity::Capsule => Some(GitIdentity::Capsule),
            };
            let github = github.map(|s| match s {
                CliGithubScope::Local => GithubScope::Local,
                CliGithubScope::Global => GithubScope::Global,
            });
            let overrides = CliOverrides {
                iterations,
                prompt,
                rebuild,
                model,
                verbose,
                git_identity,
                github,
            };
            match RunSession::prepare(capsule_dir, overrides)?.execute()? {
                run::ExitDecision::Success => {
                    println!("Claude submitted a pass verdict.");
                    Ok(())
                }
                run::ExitDecision::Failure(msg) => {
                    eprintln!("{msg}");
                    std::process::exit(1);
                }
            }
        }
    }
}
