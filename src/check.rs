use crate::config::{Config, OnFail, PipelineConfig, PipelineEntry};
use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Debug)]
pub struct CheckIssue {
    pub severity: Severity,
    pub location: String,
    pub message: String,
    pub fix_hint: Option<String>,
}

pub type CheckReport = Vec<CheckIssue>;

pub fn check(cfg: &Config) -> CheckReport {
    let mut issues = Vec::new();
    check_dockerfile(&cfg.capsule_dir, &mut issues);
    check_prompt_files(&cfg.pipeline, &cfg.capsule_dir, &mut issues);
    check_hook_scripts(cfg, &mut issues);
    check_max_failure_with_retry(&cfg.pipeline, &mut issues);
    issues
}

fn check_dockerfile(capsule_dir: &Path, issues: &mut CheckReport) {
    if !capsule_dir.join("Dockerfile").exists() {
        issues.push(CheckIssue {
            severity: Severity::Hint,
            location: "Dockerfile".to_string(),
            message: "no Dockerfile — the base capsule image will be used as-is".to_string(),
            fix_hint: Some("add a Dockerfile to install extra dependencies".to_string()),
        });
    }
}

fn check_prompt_files(pipeline: &PipelineConfig, capsule_dir: &Path, issues: &mut CheckReport) {
    for stage in all_stages(pipeline) {
        if let Some(ref prompt_path) = stage.prompt {
            let full_path = capsule_dir.join(prompt_path);
            if !full_path.exists() {
                issues.push(CheckIssue {
                    severity: Severity::Error,
                    location: format!("stages.{}", stage.name),
                    message: format!("prompt file not found: {prompt_path}"),
                    fix_hint: Some(format!("create the file at {}", full_path.display())),
                });
            }
        }
    }
}

fn check_hook_scripts(cfg: &Config, issues: &mut CheckReport) {
    for name in ["before-all.sh", "before-each.sh"] {
        let path = cfg.capsule_dir.join(name);
        if path.exists() {
            issues.push(CheckIssue {
                severity: Severity::Error,
                location: name.to_string(),
                message: format!(
                    "{name} is no longer used — migrate to the `setup` field in config.yml"
                ),
                fix_hint: Some(
                    "add `setup: <command-or-script>` at the top level or per stage".to_string(),
                ),
            });
        }
    }

    if let Some(ref value) = cfg.setup {
        check_setup_value(value, "setup", &cfg.capsule_dir, issues);
    }
    for stage in all_stages(&cfg.pipeline) {
        if let Some(ref value) = stage.setup {
            let location = format!("stages.{}.setup", stage.name);
            check_setup_value(value, &location, &cfg.capsule_dir, issues);
        }
    }
}

fn check_setup_value(value: &str, location: &str, capsule_dir: &Path, issues: &mut CheckReport) {
    use crate::config::SetupCommand;

    let parsed = match SetupCommand::parse(value, capsule_dir) {
        Ok(p) => p,
        Err(e) => {
            issues.push(CheckIssue {
                severity: Severity::Error,
                location: location.to_string(),
                message: format!("{e:#}"),
                fix_hint: None,
            });
            return;
        }
    };

    if let SetupCommand::File(path) = parsed {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.permissions().mode() & 0o111 == 0 {
                    issues.push(CheckIssue {
                        severity: Severity::Hint,
                        location: location.to_string(),
                        message: format!("{value} is not executable"),
                        fix_hint: Some(format!("chmod +x {}", path.display())),
                    });
                }
            }
        }
    }
}

fn check_max_failure_with_retry(pipeline: &PipelineConfig, issues: &mut CheckReport) {
    for stage in all_stages(pipeline) {
        if stage.max_failure.is_some() && matches!(stage.on_fail, OnFail::Retry) {
            issues.push(CheckIssue {
                severity: Severity::Warning,
                location: format!("stages.{}", stage.name),
                message: "`max_failure` is set but `on_fail` is `retry` — \
                     `max_failure` counts every fail verdict and will exit the pipeline \
                     regardless of retries; consider whether this is intentional"
                    .to_string(),
                fix_hint: Some(
                    "set `on_fail: exit` if you want max_failure to be the only exit condition, \
                     or remove `max_failure` if retry exhaustion alone should control exit"
                        .to_string(),
                ),
            });
        }
    }
}

fn all_stages(pipeline: &PipelineConfig) -> Vec<&crate::config::StageConfig> {
    let mut stages = Vec::new();
    for entry in &pipeline.entries {
        match entry {
            PipelineEntry::Stage(s) => stages.push(s),
            PipelineEntry::Loop(l) => {
                for s in &l.stages {
                    stages.push(s);
                }
            }
        }
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        OnFail, OnPass, PipelineConfig, PipelineEntry, StageConfig, MAX_RETRIES_DEFAULT,
    };

    fn stage_with(name: &str, on_fail: OnFail, max_failure: Option<u32>) -> StageConfig {
        StageConfig {
            name: name.to_string(),
            prompt: None,
            model: None,
            on_pass: OnPass::Next,
            on_fail,
            max_retries: MAX_RETRIES_DEFAULT,
            max_failure,
            setup: None,
            volumes: vec![],
        }
    }

    fn pipeline_with_stage(s: StageConfig) -> PipelineConfig {
        PipelineConfig {
            entries: vec![PipelineEntry::Stage(s)],
            max_stages: 1000,
        }
    }

    #[test]
    fn max_failure_with_retry_produces_warning() {
        let s = stage_with("impl", OnFail::Retry, Some(5));
        let pipeline = pipeline_with_stage(s);
        let mut issues = Vec::new();
        check_max_failure_with_retry(&pipeline, &mut issues);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("max_failure"));
        assert!(issues[0].location.contains("impl"));
    }

    #[test]
    fn max_failure_with_exit_produces_no_warning() {
        let s = stage_with("impl", OnFail::Exit, Some(5));
        let pipeline = pipeline_with_stage(s);
        let mut issues = Vec::new();
        check_max_failure_with_retry(&pipeline, &mut issues);
        assert!(issues.is_empty());
    }

    #[test]
    fn retry_without_max_failure_produces_no_warning() {
        let s = stage_with("impl", OnFail::Retry, None);
        let pipeline = pipeline_with_stage(s);
        let mut issues = Vec::new();
        check_max_failure_with_retry(&pipeline, &mut issues);
        assert!(issues.is_empty());
    }
}
