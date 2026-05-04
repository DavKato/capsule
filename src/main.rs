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

        /// Runtime input injected into the first stage's first invocation only.
        #[arg(long)]
        input: Option<String>,

        /// Minimum remaining token lifetime (minutes) before prompting to refresh.
        #[arg(long)]
        min_token_lifetime_minutes: Option<u32>,

        /// Inject KEY=VALUE into the container environment and hook scripts for this run.
        /// Repeatable. Values override same-named keys in .capsule/.env.
        /// CAPSULE_* keys are reserved and rejected.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
    },

    /// Resume pipeline from the last interrupted run (reads last-run.json)
    Resume {
        /// Directory containing config, prompt, and hook scripts (default: ./.capsule)
        #[arg(long, default_value = ".capsule")]
        capsule_dir: PathBuf,

        /// Override or add KEY=VALUE pairs on top of persisted run environment.
        /// Repeatable. CLI values win per key. CAPSULE_* keys are reserved.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
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

fn parse_env_pairs(raw: Vec<String>) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for s in raw {
        let Some((k, v)) = s.split_once('=') else {
            anyhow::bail!("--env: invalid format {s:?} — expected KEY=VALUE");
        };
        if k.contains(['\n', '\r', '\0']) || v.contains(['\n', '\r', '\0']) {
            anyhow::bail!("--env: key or value must not contain newline or null characters");
        }
        if k.starts_with("CAPSULE_") {
            anyhow::bail!("--env: key {k:?} is reserved — CAPSULE_* keys are owned by capsule");
        }
        if k.is_empty()
            || !k.bytes().enumerate().all(|(i, b)| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
                b'0'..=b'9' if i > 0 => true,
                _ => false,
            })
        {
            anyhow::bail!(
                "--env: key {k:?} is not a valid POSIX name — must match [A-Za-z_][A-Za-z0-9_]*"
            );
        }
        pairs.push((k.to_string(), v.to_string()));
    }
    Ok(pairs)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Resume { capsule_dir, env } => {
            let env = parse_env_pairs(env)?;
            match RunSession::prepare_resume(capsule_dir, env)?.execute()? {
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
            input,
            min_token_lifetime_minutes,
            env,
        } => {
            let git_identity = match git_identity {
                CliGitIdentity::User => Some(GitIdentity::User),
                CliGitIdentity::Capsule => Some(GitIdentity::Capsule),
            };
            let github = github.map(|s| match s {
                CliGithubScope::Local => GithubScope::Local,
                CliGithubScope::Global => GithubScope::Global,
            });
            let env = parse_env_pairs(env)?;
            let overrides = CliOverrides {
                iterations,
                prompt,
                rebuild,
                model,
                verbose,
                git_identity,
                github,
                input,
                min_token_lifetime_minutes,
                env,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_valid_key_value() {
        let pairs = parse_env_pairs(vec!["FOO=bar".to_string()]).unwrap();
        assert_eq!(pairs, vec![("FOO".to_string(), "bar".to_string())]);
    }

    #[test]
    fn parse_env_value_with_equals() {
        let pairs = parse_env_pairs(vec!["FOO=a=b".to_string()]).unwrap();
        assert_eq!(pairs, vec![("FOO".to_string(), "a=b".to_string())]);
    }

    #[test]
    fn parse_env_multiple_pairs() {
        let pairs = parse_env_pairs(vec!["A=1".to_string(), "B=2".to_string()]).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("A".to_string(), "1".to_string()));
        assert_eq!(pairs[1], ("B".to_string(), "2".to_string()));
    }

    #[test]
    fn parse_env_missing_equals_is_error() {
        let err = parse_env_pairs(vec!["NOEQUALS".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("KEY=VALUE"),
            "error should mention KEY=VALUE format: {err}"
        );
    }

    #[test]
    fn parse_env_capsule_prefix_rejected() {
        let err = parse_env_pairs(vec!["CAPSULE_FOO=x".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("reserved"),
            "error should mention reserved: {err}"
        );
    }

    #[test]
    fn parse_env_empty_vec_ok() {
        let pairs = parse_env_pairs(vec![]).unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn parse_env_empty_key_rejected() {
        let err = parse_env_pairs(vec!["=value".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("POSIX"),
            "error should mention POSIX naming: {err}"
        );
    }

    #[test]
    fn parse_env_newline_in_value_rejected() {
        let err = parse_env_pairs(vec!["KEY=val\nINJECT=bad".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("newline"),
            "error should mention newline: {err}"
        );
    }

    #[test]
    fn parse_env_newline_in_key_rejected() {
        let err = parse_env_pairs(vec!["KEY\nINJECT=val".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("newline"),
            "error should mention newline: {err}"
        );
    }

    #[test]
    fn parse_env_key_starting_with_digit_rejected() {
        let err = parse_env_pairs(vec!["1FOO=bar".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("POSIX"),
            "error should mention POSIX naming: {err}"
        );
    }

    #[test]
    fn parse_env_key_with_hash_rejected() {
        let err = parse_env_pairs(vec!["#COMMENT=bar".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("POSIX"),
            "error should mention POSIX naming: {err}"
        );
    }

    #[test]
    fn parse_env_key_with_space_rejected() {
        let err = parse_env_pairs(vec!["MY KEY=bar".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("POSIX"),
            "error should mention POSIX naming: {err}"
        );
    }

    #[test]
    fn parse_env_valid_key_with_underscores_and_digits() {
        let pairs = parse_env_pairs(vec!["_MY_VAR_2=hello".to_string()]).unwrap();
        assert_eq!(pairs, vec![("_MY_VAR_2".to_string(), "hello".to_string())]);
    }

    #[test]
    fn parse_env_capsule_bypass_via_newline_rejected() {
        // Regression: newline in value must not let CAPSULE_* slip through.
        let err = parse_env_pairs(vec!["TASK=foo\nCAPSULE_MODEL=evil".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("newline"),
            "newline bypass of CAPSULE_* guard must be caught: {err}"
        );
    }
}
