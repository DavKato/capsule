use anyhow::Result;
use capsule::check::{CheckIssue, CheckReport, Severity};
use capsule::config::{CliOverrides, GitIdentity, GithubScope};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::io;
use std::io::IsTerminal;
use std::path::PathBuf;

mod run;
use capsule::explain;
use capsule::init;
use capsule::mcp_server;
use capsule::templates;
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
        /// Maximum number of pipeline stages to execute
        #[arg(long)]
        max_stages: Option<u32>,

        /// Directory containing config, prompt, and setup scripts (default: ./.capsule)
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
        commit_as: CliGitIdentity,

        /// Inject GH_TOKEN into the container: 'local' reads from .capsule/.env,
        /// 'global' reads from process env (falls back to gh auth token).
        /// When absent, no token is injected.
        #[arg(long, value_enum)]
        github_token_from: Option<CliGithubScope>,

        /// Runtime input injected into the first stage's first invocation only.
        #[arg(long)]
        input: Option<String>,

        /// Inject KEY=VALUE into the container environment and setup commands for this run.
        /// Repeatable. Values override same-named keys in .capsule/.env.
        /// CAPSULE_* keys are reserved and rejected.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// Write run output to a file in addition to the terminal
        #[arg(long)]
        log_file: Option<PathBuf>,
    },

    /// Resume pipeline from the last interrupted run (reads last-run.json)
    Resume {
        /// Directory containing config, prompt, and setup scripts (default: ./.capsule)
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

    /// Validate the .capsule/ directory structure
    Check {
        /// Directory containing config, prompt, and setup scripts (default: ./.capsule)
        #[arg(long, default_value = ".capsule")]
        capsule_dir: PathBuf,
    },

    /// Browse and copy pre-built .capsule/ skeletons
    Templates {
        #[command(subcommand)]
        command: TemplatesCommands,
    },

    /// Show agent-targeted documentation topics
    Explain {
        /// Topics to load (e.g. mental-model setup-files)
        topics: Vec<String>,

        /// Load all topics
        #[arg(long)]
        all: bool,
    },

    /// Bootstrap a new .capsule/ from a template
    Init {
        /// Template name to copy (use `capsule templates list` to see options)
        #[arg(long)]
        template: Option<String>,

        /// Overwrite an existing .capsule/ directory
        #[arg(long)]
        force: bool,
    },

    /// Run the MCP server over stdio (used inside the container by Claude Code)
    #[command(hide = true)]
    McpServe,
}

#[derive(Debug, Subcommand)]
enum TemplatesCommands {
    /// List available templates with descriptions
    List,
}

fn build_check_report(capsule_dir: &std::path::Path) -> CheckReport {
    let result = capsule::config::resolve(capsule_dir, capsule::config::CliOverrides::default());
    match result {
        Ok(cfg) => capsule::check::check(&cfg),
        Err(e) => {
            // Use the full error chain so callers see all context (e.g., "unknown stage `X`").
            let msg = format!("{e:#}");
            let fix_hint = try_typo_hint(&msg, capsule_dir);
            vec![CheckIssue {
                severity: Severity::Error,
                location: "config.yml".to_string(),
                message: msg,
                fix_hint,
            }]
        }
    }
}

fn try_typo_hint(error_msg: &str, capsule_dir: &std::path::Path) -> Option<String> {
    let prefix = "unknown stage `";
    let start = error_msg.find(prefix)?;
    let after = &error_msg[start + prefix.len()..];
    let end = after.find('`')?;
    let unknown = &after[..end];

    let config_path = capsule_dir.join("config.yml");
    let yaml = std::fs::read_to_string(config_path).ok()?;
    let known = capsule::config::raw_stage_names_from_yaml(&yaml);

    let closest = known
        .iter()
        .map(|n| (n, levenshtein(unknown, n)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d);

    closest.map(|(name, _)| format!("did you mean `{name}`?"))
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    #[allow(clippy::needless_range_loop)]
    for i in 0..=m {
        dp[i][0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }
    dp[m][n]
}

fn print_report(report: &CheckReport) {
    for issue in report {
        let tag = match issue.severity {
            Severity::Error => "[ERROR]",
            Severity::Warning => "[WARN]",
            Severity::Hint => "[HINT]",
        };
        capsule::display::println(&format!("{tag} {}: {}", issue.location, issue.message));
        if let Some(ref hint) = issue.fix_hint {
            capsule::display::println(&format!("  hint: {hint}"));
        }
    }
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

fn format_template_line(
    entry: &templates::TemplateEntry,
    name_width: usize,
    numbered: Option<usize>,
) -> String {
    match numbered {
        Some(i) => format!(
            "  [{}] {:<width$}  {}",
            i,
            entry.name,
            entry.description,
            width = name_width
        ),
        None => format!(
            "{:<width$}  {}",
            entry.name,
            entry.description,
            width = name_width
        ),
    }
}

fn pick_template_interactive() -> Result<String> {
    let entries = templates::list();
    let name_width = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
    capsule::display::println("Available templates:");
    for (i, entry) in entries.iter().enumerate() {
        capsule::display::println(&format_template_line(entry, name_width, Some(i + 1)));
    }
    capsule::display::print(&format!("Select template (1-{}): ", entries.len()));
    let mut line = String::new();
    io::BufRead::read_line(&mut io::stdin().lock(), &mut line)?;
    let n: usize = line
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid selection"))?;
    if n < 1 || n > entries.len() {
        anyhow::bail!("selection out of range");
    }
    Ok(entries[n - 1].name.clone())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Resume { capsule_dir, env } => {
            let env = parse_env_pairs(env)?;
            let session = RunSession::prepare_resume(capsule_dir, env)?;
            if let Some(path) = session.log_file() {
                capsule::display::set_log_file(path)?;
            }
            capsule::display::init();
            let result = session.execute();
            capsule::display::teardown();
            match result? {
                run::ExitDecision::Success => Ok(()),
                run::ExitDecision::Failure(notes) => {
                    if !notes.is_empty() {
                        capsule::display::info(&notes);
                    }
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
            let current = env!("CARGO_PKG_VERSION");
            capsule::display::dim_info(&format!("Current version: {current}"));
            capsule::display::dim_info("Checking for updates...");

            let run_install = || -> anyhow::Result<()> {
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
            };

            match capsule::update_check::fetch_latest_tag() {
                Some(tag) => {
                    let latest = capsule::update_check::strip_v(&tag);
                    if capsule::update_check::is_newer(&tag, current) {
                        capsule::display::dim_info(&format!("Updating {current} → {latest}..."));
                        run_install()?;
                        capsule::display::info(&format!("Successfully updated to {latest}"));
                    } else {
                        capsule::display::info(&format!("Already up to date ({current})"));
                    }
                }
                None => {
                    capsule::display::info(
                        "Could not check latest version, running install script anyway",
                    );
                    run_install()?;
                }
            }

            Ok(())
        }
        Commands::Templates {
            command: TemplatesCommands::List,
        } => {
            let entries = templates::list();
            let name_width = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
            for entry in &entries {
                capsule::display::println(&format_template_line(entry, name_width, None));
            }
            capsule::display::println("");
            capsule::display::println(
                "For guidance on choosing a shape: capsule explain pipeline-shapes",
            );
            Ok(())
        }
        Commands::Check { capsule_dir } => {
            let report = build_check_report(&capsule_dir);
            print_report(&report);
            if report.iter().any(|i| i.severity == Severity::Error) {
                std::process::exit(1);
            }
            Ok(())
        }
        Commands::Explain { topics, all } => {
            if all {
                capsule::display::print(&explain::load_all());
            } else if topics.is_empty() {
                capsule::display::print(explain::index());
            } else {
                let refs: Vec<&str> = topics.iter().map(String::as_str).collect();
                match explain::load(&refs) {
                    Ok(content) => capsule::display::print(&content),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            Ok(())
        }
        Commands::Init { template, force } => {
            let capsule_dir = std::path::Path::new(".capsule");
            let template_name = match template {
                Some(t) => t,
                None => {
                    if !io::stdin().is_terminal() {
                        eprintln!("capsule init: interactive mode requires a TTY.");
                        eprintln!("For non-interactive use (e.g., from scripts or AI agents):");
                        eprintln!("  capsule templates list");
                        eprintln!("  capsule init --template <name>");
                        std::process::exit(1);
                    }
                    pick_template_interactive()?
                }
            };
            init::init(&template_name, capsule_dir, force)?;
            Ok(())
        }
        Commands::McpServe => {
            mcp_server::run_server();
            Ok(())
        }
        Commands::Run {
            max_stages,
            capsule_dir,
            rebuild,
            model,
            verbose,
            commit_as,
            github_token_from,
            input,
            env,
            log_file,
        } => {
            let commit_as = match commit_as {
                CliGitIdentity::User => Some(GitIdentity::User),
                CliGitIdentity::Capsule => Some(GitIdentity::Capsule),
            };
            let github_token_from = github_token_from.map(|s| match s {
                CliGithubScope::Local => GithubScope::Local,
                CliGithubScope::Global => GithubScope::Global,
            });
            let env = parse_env_pairs(env)?;
            let overrides = CliOverrides {
                max_stages,
                rebuild,
                model,
                verbose,
                commit_as,
                github_token_from,
                input,
                env,
                log_file,
            };
            let session = RunSession::prepare(capsule_dir, overrides)?;
            if let Some(path) = session.log_file() {
                capsule::display::set_log_file(path)?;
            }
            capsule::display::init();
            let result = session.execute();
            capsule::display::teardown();
            match result? {
                run::ExitDecision::Success => Ok(()),
                run::ExitDecision::Failure(notes) => {
                    if !notes.is_empty() {
                        capsule::display::info(&notes);
                    }
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
