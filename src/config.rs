use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const MAX_STAGES_DEFAULT: u32 = 1000;
pub const MAX_RETRIES_DEFAULT: u32 = 3;

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
    pub max_retries: u32,
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
    pub max_stages: u32,
}

/// Resolved configuration used by all downstream modules.
#[derive(Debug, Clone)]
pub struct Config {
    pub capsule_dir: PathBuf,
    pub rebuild: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub commit_as: GitIdentity,
    /// When Some, inject GH_TOKEN into the container from the specified source.
    /// When None, no token is injected.
    pub github_token_from: Option<GithubScope>,
    pub pipeline: PipelineConfig,
    /// When Some, tee all display output to this file path.
    pub log_file: Option<PathBuf>,
}

/// CLI-supplied overrides. `None` means "not provided on the command line".
/// Bool flags default `false` when absent (there is no "unset" for booleans in clap,
/// but callers may leave them false when they were not passed).
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub max_stages: Option<u32>,
    pub rebuild: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub commit_as: Option<GitIdentity>,
    pub github_token_from: Option<GithubScope>,
    pub input: Option<String>,
    /// KEY=VALUE pairs injected into every container and hook invocation for this run.
    pub env: Vec<(String, String)>,
    /// When Some, tee all display output to this file path.
    pub log_file: Option<PathBuf>,
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
    max_stages: Option<u32>,
    model: Option<String>,
    verbose: Option<bool>,
    commit_as: Option<String>,
    github_token_from: Option<String>,
    log_file: Option<String>,
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

fn check_old_field_names(val: &serde_yaml::Value) -> Result<()> {
    const OLD_TO_NEW: &[(&str, &str)] = &[
        ("git_identity", "commit_as"),
        ("github", "github_token_from"),
        ("max_pipeline_iterations", "max_stages"),
    ];
    const REMOVED: &[(&str, &str)] = &[(
        "min_token_lifetime_minutes",
        "this field has been removed; token lifetime is now checked automatically",
    )];
    if let serde_yaml::Value::Mapping(map) = val {
        for (old, new) in OLD_TO_NEW {
            if map.contains_key(*old) {
                anyhow::bail!("unknown field `{old}` — did you mean `{new}`?");
            }
        }
        for (field, reason) in REMOVED {
            if map.contains_key(*field) {
                anyhow::bail!("unknown field `{field}` — {reason}");
            }
        }
    }
    Ok(())
}

fn parse_config_file(yaml: &str) -> Result<MultiStageConfigFile> {
    let val: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(anyhow::Error::from)?;
    check_old_field_names(&val)?;
    if val.get("stages").is_none() {
        anyhow::bail!(
            "config.yml is missing a `stages:` key.\n\
             \n\
             Flat-form config (iterations/prompt at the top level) is no longer supported.\n\
             Migrate to multi-stage format:\n\
             \n\
             Before:\n\
             \n\
             \x20 iterations: 5\n\
             \x20 prompt: prompts/implement.md\n\
             \x20 model: claude-opus-4-7\n\
             \n\
             After:\n\
             \n\
             \x20 stages:\n\
             \x20   - name: main\n\
             \x20     prompt: prompts/implement.md\n\
             \x20     model: claude-opus-4-7\n\
             \x20 max_stages: 5\n"
        );
    }
    let cfg: MultiStageConfigFile = serde_yaml::from_value(val).map_err(anyhow::Error::from)?;
    Ok(cfg)
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
        max_retries: raw.max_retries.unwrap_or(MAX_RETRIES_DEFAULT),
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

fn validate_unique_stage_names(entries: &[PipelineEntry]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for name in collect_stage_names(entries) {
        if !seen.insert(name.clone()) {
            anyhow::bail!("config.yml: duplicate stage name `{name}`");
        }
    }
    Ok(())
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
    let mut entries: Vec<PipelineEntry> = Vec::new();
    for raw_entry in cfg.stages {
        match raw_entry {
            PipelineEntryRaw::Loop(l) => {
                entries.push(PipelineEntry::Loop(convert_loop(l.loop_block)));
            }
            PipelineEntryRaw::Stage(s) => {
                entries.push(PipelineEntry::Stage(convert_stage(s)));
            }
        }
    }

    validate_unique_stage_names(&entries)?;
    validate_route_targets(&entries)?;

    Ok(PipelineConfig {
        entries,
        max_stages: cfg.max_stages.unwrap_or(MAX_STAGES_DEFAULT),
    })
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
    let file_cfg = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        Some(
            parse_config_file(&content)
                .with_context(|| format!("parsing {}", config_path.display()))?,
        )
    } else {
        None
    };

    let model = cli
        .model
        .or_else(|| file_cfg.as_ref().and_then(|f| f.model.clone()));
    let verbose = cli.verbose || file_cfg.as_ref().and_then(|f| f.verbose).unwrap_or(false);
    let commit_as = cli
        .commit_as
        .or_else(|| {
            file_cfg
                .as_ref()
                .and_then(|f| f.commit_as.as_deref())
                .and_then(git_identity_from_str)
        })
        .unwrap_or(GitIdentity::User);
    let github_token_from = cli.github_token_from.or_else(|| {
        file_cfg
            .as_ref()
            .and_then(|f| f.github_token_from.as_deref())
            .and_then(github_scope_from_str)
    });
    let log_file = cli.log_file.or_else(|| {
        file_cfg
            .as_ref()
            .and_then(|f| f.log_file.as_ref())
            .map(PathBuf::from)
    });
    let rebuild = cli.rebuild;

    let mut pipeline = if let Some(multi) = file_cfg {
        build_pipeline_from_multi_stage(multi)
            .with_context(|| format!("validating {}", config_path.display()))?
    } else {
        PipelineConfig {
            entries: Vec::new(),
            max_stages: MAX_STAGES_DEFAULT,
        }
    };
    if let Some(n) = cli.max_stages {
        pipeline.max_stages = n;
    }

    Ok(Config {
        capsule_dir: capsule_dir.to_path_buf(),
        rebuild,
        model,
        verbose,
        commit_as,
        github_token_from,
        pipeline,
        log_file,
    })
}

/// Extract all stage names from a raw YAML string without strict validation.
/// Used to compute typo suggestions when route-target validation fails.
pub fn raw_stage_names_from_yaml(yaml: &str) -> Vec<String> {
    let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect_names_from_value(&val, &mut names);
    names
}

fn collect_names_from_value(val: &serde_yaml::Value, names: &mut Vec<String>) {
    match val {
        serde_yaml::Value::Mapping(m) => {
            if let Some(serde_yaml::Value::String(n)) = m.get("name") {
                names.push(n.clone());
            }
            for (_, v) in m {
                collect_names_from_value(v, names);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                collect_names_from_value(item, names);
            }
        }
        _ => {}
    }
}
