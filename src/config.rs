use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const MAX_PIPELINE_ITERATIONS_DEFAULT: u32 = 1000;

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

/// Routing target for `on_pass`.
#[derive(Debug, Clone, PartialEq)]
pub enum OnPass {
    /// Advance to next stage in the surrounding `stages:` array (default).
    Next,
    /// Jump to named stage.
    Stage(String),
    /// Terminate pipeline non-zero.
    Exit,
}

/// Routing target for `on_fail`.
#[derive(Debug, Clone, PartialEq)]
pub enum OnFail {
    /// Terminate pipeline non-zero (default).
    Exit,
    /// Re-run the same stage.
    Retry,
    /// Jump to named stage.
    Stage(String),
}

/// One stage in a pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct StageConfig {
    pub name: String,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub on_pass: OnPass,
    pub on_fail: OnFail,
    pub max_retries: Option<u32>,
}

/// A `loop:` block containing an ordered list of stages.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopConfig {
    pub max_iteration: Option<u32>,
    pub stages: Vec<StageConfig>,
}

/// An entry in the top-level `stages:` array.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEntry {
    Stage(StageConfig),
    Loop(LoopConfig),
}

/// The parsed + validated pipeline execution graph.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub entries: Vec<PipelineEntry>,
    pub max_pipeline_iterations: u32,
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
    /// Parsed pipeline execution graph (present for both flat-form and multi-stage configs).
    pub pipeline: PipelineConfig,
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

// ── Flat-form serde types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FlatConfigFile {
    iterations: Option<u32>,
    prompt: Option<String>,
    model: Option<String>,
    verbose: Option<bool>,
    git_identity: Option<String>,
    github: Option<String>,
}

// ── Multi-stage serde types ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct StageConfigRaw {
    name: String,
    prompt: Option<String>,
    model: Option<String>,
    on_pass: Option<String>,
    on_fail: Option<String>,
    max_retries: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct LoopConfigRaw {
    max_iteration: Option<u32>,
    stages: Vec<StageConfigRaw>,
}

#[derive(Debug, Deserialize)]
struct LoopEntryRaw {
    #[serde(rename = "loop")]
    loop_block: LoopConfigRaw,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PipelineEntryRaw {
    Loop(LoopEntryRaw),
    Stage(StageConfigRaw),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MultiStageConfigFile {
    stages: Vec<PipelineEntryRaw>,
    max_pipeline_iterations: Option<u32>,
    model: Option<String>,
    verbose: Option<bool>,
    git_identity: Option<String>,
    github: Option<String>,
    /// Present to produce a clear error when combined with `stages:`.
    iterations: Option<u32>,
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

enum RawConfig {
    Flat(FlatConfigFile),
    MultiStage(MultiStageConfigFile),
}

fn parse_config_file(yaml: &str) -> Result<RawConfig> {
    let val: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(anyhow::Error::from)?;
    if val.get("stages").is_some() {
        let cfg: MultiStageConfigFile = serde_yaml::from_value(val).map_err(anyhow::Error::from)?;
        Ok(RawConfig::MultiStage(cfg))
    } else {
        let cfg: FlatConfigFile = serde_yaml::from_value(val).map_err(anyhow::Error::from)?;
        Ok(RawConfig::Flat(cfg))
    }
}

fn parse_on_pass(s: &str) -> Option<OnPass> {
    match s {
        "exit" => Some(OnPass::Exit),
        name => Some(OnPass::Stage(name.to_string())),
    }
}

fn parse_on_fail(s: &str) -> Option<OnFail> {
    match s {
        "exit" => Some(OnFail::Exit),
        "retry" => Some(OnFail::Retry),
        name => Some(OnFail::Stage(name.to_string())),
    }
}

fn convert_stage(raw: StageConfigRaw) -> StageConfig {
    let on_pass = raw
        .on_pass
        .as_deref()
        .and_then(parse_on_pass)
        .unwrap_or(OnPass::Next);
    let on_fail = raw
        .on_fail
        .as_deref()
        .and_then(parse_on_fail)
        .unwrap_or(OnFail::Exit);
    StageConfig {
        name: raw.name,
        prompt: raw.prompt,
        model: raw.model,
        on_pass,
        on_fail,
        max_retries: raw.max_retries,
    }
}

fn convert_loop(raw: LoopConfigRaw) -> LoopConfig {
    LoopConfig {
        max_iteration: raw.max_iteration,
        stages: raw.stages.into_iter().map(convert_stage).collect(),
    }
}

/// Collect all stage names across all pipeline entries (including loop bodies).
fn collect_stage_names(entries: &[PipelineEntry]) -> Vec<String> {
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            PipelineEntry::Stage(s) => names.push(s.name.clone()),
            PipelineEntry::Loop(l) => {
                for s in &l.stages {
                    names.push(s.name.clone());
                }
            }
        }
    }
    names
}

/// Validate `on_pass`/`on_fail` stage references.
fn validate_route_targets(entries: &[PipelineEntry]) -> Result<()> {
    let all_names = collect_stage_names(entries);
    let check = |target: &str| -> Result<()> {
        if !all_names.contains(&target.to_string()) {
            anyhow::bail!("config.yml: `on_pass`/`on_fail` references unknown stage `{target}`");
        }
        Ok(())
    };
    for entry in entries {
        match entry {
            PipelineEntry::Stage(s) => {
                if let OnPass::Stage(ref t) = s.on_pass {
                    check(t)?;
                }
                if let OnFail::Stage(ref t) = s.on_fail {
                    check(t)?;
                }
            }
            PipelineEntry::Loop(l) => {
                for s in &l.stages {
                    if let OnPass::Stage(ref t) = s.on_pass {
                        check(t)?;
                    }
                    if let OnFail::Stage(ref t) = s.on_fail {
                        check(t)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn build_pipeline_from_multi_stage(cfg: MultiStageConfigFile) -> Result<PipelineConfig> {
    if cfg.iterations.is_some() {
        anyhow::bail!(
            "config.yml: `iterations:` cannot be combined with `stages:` — \
             use `loop: {{ max_iteration: N }}` instead"
        );
    }

    let mut entries: Vec<PipelineEntry> = Vec::new();
    for raw_entry in cfg.stages {
        match raw_entry {
            PipelineEntryRaw::Loop(l) => {
                // Reject nested loops: loop body stages must not themselves be loops.
                // (The raw type only allows StageConfigRaw inside loops, so nested loops
                // are structurally impossible from the YAML layer. No extra check needed.)
                entries.push(PipelineEntry::Loop(convert_loop(l.loop_block)));
            }
            PipelineEntryRaw::Stage(s) => {
                entries.push(PipelineEntry::Stage(convert_stage(s)));
            }
        }
    }

    validate_route_targets(&entries)?;

    Ok(PipelineConfig {
        entries,
        max_pipeline_iterations: cfg
            .max_pipeline_iterations
            .unwrap_or(MAX_PIPELINE_ITERATIONS_DEFAULT),
    })
}

fn desugar_flat_form(iterations: u32, prompt: Option<&str>) -> PipelineConfig {
    let stage = StageConfig {
        name: "main".to_string(),
        prompt: prompt.map(str::to_string),
        model: None,
        on_pass: OnPass::Next,
        on_fail: OnFail::Exit,
        max_retries: None,
    };
    PipelineConfig {
        entries: vec![PipelineEntry::Loop(LoopConfig {
            max_iteration: Some(iterations),
            stages: vec![stage],
        })],
        max_pipeline_iterations: MAX_PIPELINE_ITERATIONS_DEFAULT,
    }
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
///   CLI overrides → config file → compiled-in defaults.
pub fn resolve(capsule_dir: &Path, cli: CliOverrides) -> Result<Config> {
    let config_path = capsule_dir.join("config.yml");
    let raw = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        Some(
            parse_config_file(&content)
                .with_context(|| format!("parsing {}", config_path.display()))?,
        )
    } else {
        None
    };

    let (file_flat, file_multi) = match raw {
        Some(RawConfig::Flat(f)) => (Some(f), None),
        Some(RawConfig::MultiStage(m)) => (None, Some(m)),
        None => (None, None),
    };

    // Shared fields (model, verbose, git_identity, github).
    let file_model = file_flat
        .as_ref()
        .and_then(|f| f.model.clone())
        .or_else(|| file_multi.as_ref().and_then(|m| m.model.clone()));
    let file_verbose = file_flat
        .as_ref()
        .and_then(|f| f.verbose)
        .or_else(|| file_multi.as_ref().and_then(|m| m.verbose));
    let file_git_identity = file_flat
        .as_ref()
        .and_then(|f| f.git_identity.clone())
        .or_else(|| file_multi.as_ref().and_then(|m| m.git_identity.clone()));
    let file_github = file_flat
        .as_ref()
        .and_then(|f| f.github.clone())
        .or_else(|| file_multi.as_ref().and_then(|m| m.github.clone()));

    let model = cli.model.or(file_model);
    let verbose = cli.verbose || file_verbose.unwrap_or(false);
    let git_identity = cli
        .git_identity
        .or_else(|| file_git_identity.as_deref().and_then(git_identity_from_str))
        .unwrap_or(GitIdentity::User);
    let github = cli
        .github
        .or_else(|| file_github.as_deref().and_then(github_scope_from_str));

    let rebuild = cli.rebuild;

    let (iterations, prompt, pipeline) = if let Some(multi) = file_multi {
        let pipeline = build_pipeline_from_multi_stage(multi)
            .with_context(|| format!("validating {}", config_path.display()))?;
        // iterations is not applicable for multi-stage; use max_pipeline_iterations.
        (pipeline.max_pipeline_iterations, None, pipeline)
    } else {
        let file_flat = file_flat.unwrap_or_default();
        let iterations = cli.iterations.or(file_flat.iterations).ok_or_else(|| {
            anyhow::anyhow!("--iterations is required (no CLI flag or config.yml value found)")
        })?;
        let prompt = cli.prompt.or_else(|| file_flat.prompt.map(PathBuf::from));
        let pipeline = desugar_flat_form(iterations, prompt.as_ref().and_then(|p| p.to_str()));
        (iterations, prompt, pipeline)
    };

    Ok(Config {
        iterations,
        prompt,
        capsule_dir: capsule_dir.to_path_buf(),
        rebuild,
        model,
        verbose,
        git_identity,
        github,
        pipeline,
    })
}
