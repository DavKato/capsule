use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Git commit identity mode.
#[derive(Debug, Clone, PartialEq)]
pub enum GitIdentity {
    User,
    Capsule,
}

/// GitHub token injection scope.
#[derive(Debug, Clone, PartialEq)]
pub enum GithubScope {
    /// Read GH_TOKEN from `.capsule/.env` only.
    Local,
    /// Read GH_TOKEN from process environment; fall back to `gh auth token`.
    Global,
}

/// Resolved configuration used by all downstream modules.
#[derive(Debug, Clone)]
pub struct Config {
    pub iterations: u32,
    pub prompt: Option<PathBuf>,
    pub capsule_dir: PathBuf,
    pub rebuild: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub git_identity: GitIdentity,
    /// When Some, inject GH_TOKEN into the container from the specified source.
    /// When None, no token is injected.
    pub github: Option<GithubScope>,
}

/// CLI-supplied overrides. `None` means "not provided on the command line".
/// Bool flags default `false` when absent (there is no "unset" for booleans in clap,
/// but callers may leave them false when they were not passed).
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub iterations: Option<u32>,
    pub prompt: Option<PathBuf>,
    pub rebuild: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub git_identity: Option<GitIdentity>,
    pub github: Option<GithubScope>,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    iterations: Option<u32>,
    prompt: Option<String>,
    rebuild: Option<bool>,
    model: Option<String>,
    verbose: Option<bool>,
    git_identity: Option<String>,
    github: Option<String>,
}

fn parse_file(yaml: &str) -> Result<ConfigFile> {
    serde_yaml::from_str(yaml).map_err(anyhow::Error::from)
}

fn git_identity_from_str(s: &str) -> Option<GitIdentity> {
    match s.to_ascii_lowercase().as_str() {
        "user" => Some(GitIdentity::User),
        "capsule" => Some(GitIdentity::Capsule),
        _ => None,
    }
}

fn github_scope_from_str(s: &str) -> Option<GithubScope> {
    match s.to_ascii_lowercase().as_str() {
        "local" => Some(GithubScope::Local),
        "global" => Some(GithubScope::Global),
        _ => None,
    }
}

/// Resolve configuration by merging (highest → lowest priority):
///   CLI overrides → environment variables → config file → compiled-in defaults.
///
/// `env` is the full environment map; pass `&std::env::vars().collect()` in
/// production, or a controlled map in tests.
pub fn resolve(
    capsule_dir: &Path,
    cli: CliOverrides,
    env: &HashMap<String, String>,
) -> Result<Config> {
    let config_path = capsule_dir.join("config.yml");
    let file = if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        parse_file(&raw).with_context(|| format!("parsing {}", config_path.display()))?
    } else {
        ConfigFile::default()
    };

    let iterations = cli
        .iterations
        .or_else(|| {
            env.get("CAPSULE_ITERATIONS")
                .and_then(|s| s.parse::<u32>().ok())
        })
        .or(file.iterations)
        .ok_or_else(|| anyhow::anyhow!("--iterations is required (no CLI flag, env var CAPSULE_ITERATIONS, or config.yml value found)"))?;

    let prompt = cli.prompt.or_else(|| {
        env.get("CAPSULE_PROMPT")
            .map(PathBuf::from)
            .or_else(|| file.prompt.map(PathBuf::from))
    });

    let rebuild = cli.rebuild
        || env
            .get("CAPSULE_REBUILD")
            .map(|s| matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
        || file.rebuild.unwrap_or(false);

    let model = cli
        .model
        .or_else(|| env.get("CAPSULE_MODEL").cloned().or(file.model));

    let verbose = cli.verbose
        || env
            .get("CAPSULE_VERBOSE")
            .map(|s| matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false)
        || file.verbose.unwrap_or(false);

    let git_identity = cli
        .git_identity
        .or_else(|| {
            env.get("CAPSULE_GIT_IDENTITY")
                .and_then(|s| git_identity_from_str(s))
        })
        .or_else(|| file.git_identity.as_deref().and_then(git_identity_from_str))
        .unwrap_or(GitIdentity::User);

    let github = cli
        .github
        .or_else(|| {
            env.get("CAPSULE_GITHUB")
                .and_then(|s| github_scope_from_str(s))
        })
        .or_else(|| file.github.as_deref().and_then(github_scope_from_str));

    Ok(Config {
        iterations,
        prompt,
        capsule_dir: capsule_dir.to_path_buf(),
        rebuild,
        model,
        verbose,
        git_identity,
        github,
    })
}
